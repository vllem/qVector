//! Video for calls: pictures in and out as RGBA; VP8 (libvpx, preferred: every WebRTC client has it and it is royalty-free)
//! or H.264 (OpenH264, Constrained Baseline) on the wire, whichever the other side picks in the SDP.
//!
//! Both directions run on their own OS thread, because encoding and decoding are CPU-heavy and must not stall the
//! async runtime that also carries the audio.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;

use crate::vp8::{I420, Vp8Decoder, Vp8Encoder};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Codec {
    Vp8 = 0,
    H264 = 1,
}

impl Codec {
    pub fn from_mime(mime: &str) -> Option<Codec> {
        match mime.to_ascii_lowercase().as_str() {
            "video/vp8" => Some(Codec::Vp8),
            "video/h264" => Some(Codec::H264),
            _ => None,
        }
    }

    fn from_u8(n: u8) -> Codec {
        if n == 1 { Codec::H264 } else { Codec::Vp8 }
    }

    /// Whether a frame (as the depacketizer returns it) starts a decodable sequence.
    pub fn is_key(self, frame: &[u8]) -> bool {
        match self {
            Codec::Vp8 => frame.first().map(|b| b & 1 == 0).unwrap_or(false),
            Codec::H264 => has_key(frame),
        }
    }
}

/// Whether an Annex B access unit holds a key frame (IDR picture or parameter sets).
pub fn has_key(unit: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < unit.len() {
        if unit[i] == 0 && unit[i + 1] == 0 && unit[i + 2] == 1 {
            if matches!(unit[i + 3] & 0x1f, 5 | 7) {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}

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

/// The encoder: RGBA pictures in, one coded frame out (H.264 as Annex B with SPS/PPS on every key frame).
pub struct Encoder {
    codec: Codec,
    h264: Option<openh264::encoder::Encoder>,
    vp8: Option<Vp8Encoder>,
    size: (u32, u32),
    key: bool,
}

impl Encoder {
    pub fn new(codec: Codec) -> Encoder {
        Encoder { codec, h264: None, vp8: None, size: (0, 0), key: true }
    }

    fn open(&mut self, w: u32, h: u32) -> Result<(), String> {
        self.h264 = None;
        self.vp8 = None;
        match self.codec {
            Codec::H264 => {
                let config = EncoderConfig::new()
                    .bitrate(BitRate::from_bps(700_000))
                    .max_frame_rate(FrameRate::from_hz(FPS))
                    .usage_type(UsageType::CameraVideoRealTime)
                    .profile(Profile::Baseline)
                    .intra_frame_period(IntraFramePeriod::from_num_frames(KEY_EVERY));
                self.h264 = Some(
                    openh264::encoder::Encoder::with_api_config(openh264::OpenH264API::from_source(), config).map_err(|e| e.to_string())?,
                );
            }
            Codec::Vp8 => self.vp8 = Some(Vp8Encoder::new(w as usize, h as usize, FPS as u32, 700, KEY_EVERY)?),
        }
        self.size = (w, h);
        Ok(())
    }

    /// Ask for a key frame at the next picture (a new picture size does this by itself).
    pub fn force_key(&mut self) {
        self.key = true;
        if let Some(e) = self.h264.as_mut() {
            e.force_intra_frame();
        }
    }

    pub fn encode(&mut self, f: &Frame) -> Result<Vec<u8>, String> {
        if !f.is_valid() {
            return Err("bad picture".into());
        }
        let f = f.fitted(MAX_W, MAX_H);
        if (self.h264.is_none() && self.vp8.is_none()) || self.size != (f.w, f.h) {
            self.open(f.w, f.h)?;
            self.key = true;
        }
        let yuv = YUVBuffer::from_rgb_source(RgbaSliceU8::new(&f.rgba, (f.w as usize, f.h as usize)));
        if let Some(enc) = self.h264.as_mut() {
            return Ok(enc.encode(&yuv).map_err(|e| e.to_string())?.to_vec());
        }
        let (w, h) = (f.w as usize, f.h as usize);
        let mut data = Vec::with_capacity(w * h * 3 / 2);
        let (ys, us, vs) = yuv.strides();
        for r in 0..h {
            data.extend_from_slice(&yuv.y()[r * ys..r * ys + w]);
        }
        for r in 0..h / 2 {
            data.extend_from_slice(&yuv.u()[r * us..r * us + w / 2]);
        }
        for r in 0..h / 2 {
            data.extend_from_slice(&yuv.v()[r * vs..r * vs + w / 2]);
        }
        let key = std::mem::take(&mut self.key);
        self.vp8.as_mut().ok_or("no encoder")?.encode(&I420 { w, h, data }, key)
    }
}

/// The decoder: coded frames in, RGBA pictures out. Frames before the first key frame give nothing.
pub struct Decoder {
    h264: Option<openh264::decoder::Decoder>,
    vp8: Option<Vp8Decoder>,
}

impl Decoder {
    pub fn new(codec: Codec) -> Result<Decoder, String> {
        Ok(match codec {
            Codec::H264 => Decoder { h264: Some(openh264::decoder::Decoder::new().map_err(|e| e.to_string())?), vp8: None },
            Codec::Vp8 => Decoder { h264: None, vp8: Some(Vp8Decoder::new()?) },
        })
    }

    pub fn decode(&mut self, coded: &[u8]) -> Option<Frame> {
        if let Some(d) = self.vp8.as_mut() {
            let (w, h, rgba) = d.decode(coded)?;
            return Some(Frame { w, h, rgba });
        }
        let yuv = self.h264.as_mut()?.decode(coded).ok()??;
        let (w, h) = yuv.dimensions();
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        Some(Frame { w: w as u32, h: h as u32, rgba })
    }
}

/// Runs the encoder on its own thread. `push` never blocks (a picture is dropped while the thread is busy);
/// `key` asks for a key frame; `set_codec` switches the codec once the SDP exchange has decided it.
/// Coded frames come out of `out`.
#[derive(Clone)]
pub struct EncoderThread {
    tx: std_mpsc::SyncSender<Msg>,
    codec: Arc<AtomicU8>,
}

enum Msg {
    Frame(Frame),
    Key,
}

impl EncoderThread {
    pub fn spawn(codec: Codec, out: tokio::sync::mpsc::Sender<Vec<u8>>) -> EncoderThread {
        let (tx, rx) = std_mpsc::sync_channel::<Msg>(2);
        let shared = Arc::new(AtomicU8::new(codec as u8));
        let shared_in = shared.clone();
        std::thread::Builder::new()
            .name("video-encode".into())
            .spawn(move || {
                let mut enc = Encoder::new(Codec::from_u8(shared_in.load(Ordering::Relaxed)));
                while let Ok(m) = rx.recv() {
                    let wanted = Codec::from_u8(shared_in.load(Ordering::Relaxed));
                    if wanted != enc.codec {
                        enc = Encoder::new(wanted);
                    }
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
        EncoderThread { tx, codec: shared }
    }

    pub fn push(&self, f: Frame) {
        let _ = self.tx.try_send(Msg::Frame(f));
    }

    pub fn key(&self) {
        let _ = self.tx.try_send(Msg::Key);
    }

    pub fn codec(&self) -> Codec {
        Codec::from_u8(self.codec.load(Ordering::Relaxed))
    }

    pub fn set_codec(&self, c: Codec) {
        self.codec.store(c as u8, Ordering::Relaxed);
    }
}

/// Runs the decoder on its own thread; each decoded picture goes to `sink`.
pub struct DecoderThread {
    tx: std_mpsc::SyncSender<Vec<u8>>,
}

impl DecoderThread {
    pub fn spawn(codec: Codec, sink: Arc<dyn Fn(Frame) + Send + Sync>) -> DecoderThread {
        let (tx, rx) = std_mpsc::sync_channel::<Vec<u8>>(8);
        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                let Ok(mut dec) = Decoder::new(codec) else { return };
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
        for codec in [Codec::H264, Codec::Vp8] {
            one_round_trip(codec);
        }
    }

    fn one_round_trip(codec: Codec) {
        let mut enc = Encoder::new(codec);
        let mut dec = Decoder::new(codec).unwrap();
        let src = picture(0, 320, 240);
        let mut got = None;
        for n in 0..5 {
            let bytes = enc.encode(&picture(n, 320, 240)).unwrap();
            assert!(!bytes.is_empty());
            if n == 0 {
                assert!(codec.is_key(&bytes), "{codec:?}: the first frame is a key frame");
                if codec == Codec::H264 {
                    assert_eq!(&bytes[..4], &[0, 0, 0, 1], "Annex B start code");
                }
            }
            got = dec.decode(&bytes).or(got);
        }
        let out = got.expect("a decoded picture");
        assert_eq!((out.w, out.h), (320, 240), "{codec:?}");
        let shift = (luma(&out) - luma(&picture(4, 320, 240))).abs();
        assert!(shift < 6.0, "{codec:?}: mean brightness drifted by {shift} (source {})", luma(&src));
    }

    #[test]
    fn big_pictures_shrink_and_odd_sizes_become_even() {
        let f = picture(0, 1280, 720).fitted(MAX_W, MAX_H);
        assert_eq!((f.w, f.h), (640, 360));
        assert!(f.is_valid());
        let small = picture(0, 321, 241).fitted(MAX_W, MAX_H);
        assert_eq!((small.w, small.h), (320, 240));
        assert!(small.is_valid());
        let mut enc = Encoder::new(Codec::Vp8);
        assert!(enc.encode(&picture(0, 1280, 720)).is_ok());
        assert!(enc.encode(&Frame { w: 3, h: 3, rgba: vec![0; 36] }).is_err());
    }
}
