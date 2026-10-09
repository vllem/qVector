//! Video for calls: pictures in and out as RGBA, H.264 (OpenH264, Constrained Baseline) on the wire.
//!
//! Both directions run on their own OS thread, because encoding and decoding are CPU-heavy and must not stall the
//! async runtime that also carries the audio. The codec sits behind `Encoder`/`Decoder` so that another codec (VP8)
//! can be added without touching the peer or the signalling.

use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use openh264::encoder::{BitRate, EncoderConfig, FrameRate, IntraFramePeriod, Profile, UsageType};
use openh264::formats::{RgbaSliceU8, YUVBuffer, YUVSource};

/// A picture: `rgba` holds `w * h * 4` bytes, rows top to bottom.
#[derive(Clone, Debug)]
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn is_valid(&self) -> bool {
        self.w >= 16 && self.h >= 16 && self.rgba.len() == self.w as usize * self.h as usize * 4
    }

    /// The picture shrunk (never enlarged) to fit `max_w` x `max_h`, with even sides as H.264 wants.
    pub fn fitted(&self, max_w: u32, max_h: u32) -> Frame {
        let scale = (max_w as f32 / self.w as f32).min(max_h as f32 / self.h as f32).min(1.0);
        let w = (((self.w as f32 * scale) as u32) & !1).max(16);
        let h = (((self.h as f32 * scale) as u32) & !1).max(16);
        if (w, h) == (self.w, self.h) {
            return self.clone();
        }
        let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
        for y in 0..h {
            let sy = (y as u64 * self.h as u64 / h as u64) as usize;
            for x in 0..w {
                let sx = (x as u64 * self.w as u64 / w as u64) as usize;
                let i = (sy * self.w as usize + sx) * 4;
                rgba.extend_from_slice(&self.rgba[i..i + 4]);
            }
        }
        Frame { w, h, rgba }
    }
}

/// Pictures larger than this are shrunk before encoding (keeps the bitrate and the CPU modest).
pub const MAX_W: u32 = 640;
pub const MAX_H: u32 = 480;
/// A key frame (everything a late joiner or a lossy link needs) at least this often, in frames at `FPS`.
pub const FPS: f32 = 15.0;
const KEY_EVERY: u32 = 30;

/// The encoder: RGBA pictures in, Annex B access units out (SPS/PPS come with every key frame).
pub struct Encoder {
    inner: Option<openh264::encoder::Encoder>,
    size: (u32, u32),
}

impl Encoder {
    pub fn new() -> Encoder {
        Encoder { inner: None, size: (0, 0) }
    }

    fn open(&mut self, w: u32, h: u32) -> Result<(), String> {
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(700_000))
            .max_frame_rate(FrameRate::from_hz(FPS))
            .usage_type(UsageType::CameraVideoRealTime)
            .profile(Profile::Baseline)
            .intra_frame_period(IntraFramePeriod::from_num_frames(KEY_EVERY));
        self.inner = Some(
            openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), config).map_err(|e| e.to_string())?,
        );
        self.size = (w, h);
        Ok(())
    }

    /// Ask for a key frame at the next picture (a new picture size does this by itself).
    pub fn force_key(&mut self) {
        if let Some(e) = self.inner.as_mut() {
            e.force_intra_frame();
        }
    }

    pub fn encode(&mut self, f: &Frame) -> Result<Vec<u8>, String> {
        if !f.is_valid() {
            return Err("bad picture".into());
        }
        let f = f.fitted(MAX_W, MAX_H);
        if self.inner.is_none() || self.size != (f.w, f.h) {
            self.open(f.w, f.h)?;
        }
        let yuv = YUVBuffer::from_rgb_source(RgbaSliceU8::new(&f.rgba, (f.w as usize, f.h as usize)));
        let enc = self.inner.as_mut().ok_or("no encoder")?;
        let out = enc.encode(&yuv).map_err(|e| e.to_string())?;
        Ok(out.to_vec())
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// The decoder: Annex B access units in, RGBA pictures out. Pictures before the first key frame are dropped.
pub struct Decoder {
    inner: openh264::decoder::Decoder,
}

impl Decoder {
    pub fn new() -> Result<Decoder, String> {
        Ok(Decoder { inner: openh264::decoder::Decoder::new().map_err(|e| e.to_string())? })
    }

    pub fn decode(&mut self, annexb: &[u8]) -> Option<Frame> {
        let yuv = self.inner.decode(annexb).ok()??;
        let (w, h) = yuv.dimensions();
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        Some(Frame { w: w as u32, h: h as u32, rgba })
    }
}

/// Runs the encoder on its own thread. `push` never blocks (a picture is dropped while the thread is busy);
/// `key` asks for a key frame. Encoded access units come out of `out`.
pub struct EncoderThread {
    tx: std_mpsc::SyncSender<Msg>,
}

enum Msg {
    Frame(Frame),
    Key,
}

impl EncoderThread {
    pub fn spawn(out: tokio::sync::mpsc::Sender<Vec<u8>>) -> EncoderThread {
        let (tx, rx) = std_mpsc::sync_channel::<Msg>(2);
        std::thread::Builder::new()
            .name("video-encode".into())
            .spawn(move || {
                let mut enc = Encoder::new();
                while let Ok(m) = rx.recv() {
                    match m {
                        Msg::Key => enc.force_key(),
                        Msg::Frame(f) => {
                            if let Ok(bytes) = enc.encode(&f) {
                                if !bytes.is_empty() && out.blocking_send(bytes).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                }
            })
            .ok();
        EncoderThread { tx }
    }

    pub fn push(&self, f: Frame) {
        let _ = self.tx.try_send(Msg::Frame(f));
    }

    pub fn key(&self) {
        let _ = self.tx.try_send(Msg::Key);
    }
}

/// Runs the decoder on its own thread; each decoded picture goes to `sink`.
pub struct DecoderThread {
    tx: std_mpsc::SyncSender<Vec<u8>>,
}

impl DecoderThread {
    pub fn spawn(sink: Arc<dyn Fn(Frame) + Send + Sync>) -> DecoderThread {
        let (tx, rx) = std_mpsc::sync_channel::<Vec<u8>>(8);
        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                let Ok(mut dec) = Decoder::new() else { return };
                while let Ok(unit) = rx.recv() {
                    if let Some(f) = dec.decode(&unit) {
                        sink(f);
                    }
                }
            })
            .ok();
        DecoderThread { tx }
    }

    pub fn push(&self, unit: Vec<u8>) {
        let _ = self.tx.try_send(unit);
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A moving bright square on a dark grey ground.
    pub fn picture(n: u32, w: u32, h: u32) -> Frame {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        let (sx, sy) = ((n * 7) % (w - 40), (n * 3) % (h - 40));
        for y in 0..h {
            for x in 0..w {
                let on = x >= sx && x < sx + 40 && y >= sy && y < sy + 40;
                let v = if on { 235 } else { 40 };
                let i = ((y * w + x) * 4) as usize;
                rgba[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        Frame { w, h, rgba }
    }

    pub fn luma(f: &Frame) -> f32 {
        f.rgba.chunks(4).map(|p| p[0] as f32).sum::<f32>() / (f.w * f.h) as f32
    }

    #[test]
    fn a_picture_survives_encoding_and_decoding() {
        let mut enc = Encoder::new();
        let mut dec = Decoder::new().unwrap();
        let src = picture(0, 320, 240);
        let mut got = None;
        for n in 0..5 {
            let bytes = enc.encode(&picture(n, 320, 240)).unwrap();
            assert!(!bytes.is_empty());
            if n == 0 {
                assert_eq!(&bytes[..4], &[0, 0, 0, 1], "Annex B start code");
            }
            got = dec.decode(&bytes).or(got);
        }
        let out = got.expect("a decoded picture");
        assert_eq!((out.w, out.h), (320, 240));
        let shift = (luma(&out) - luma(&picture(4, 320, 240))).abs();
        assert!(shift < 6.0, "mean brightness drifted by {shift} (source {})", luma(&src));
    }

    #[test]
    fn big_pictures_shrink_and_odd_sizes_become_even() {
        let f = picture(0, 1280, 720).fitted(MAX_W, MAX_H);
        assert_eq!((f.w, f.h), (640, 360));
        assert!(f.is_valid());
        let small = picture(0, 321, 241).fitted(MAX_W, MAX_H);
        assert_eq!((small.w, small.h), (320, 240));
        assert!(small.is_valid());
        let mut enc = Encoder::new();
        assert!(enc.encode(&picture(0, 1280, 720)).is_ok());
        assert!(enc.encode(&Frame { w: 3, h: 3, rgba: vec![0; 36] }).is_err());
    }
}
