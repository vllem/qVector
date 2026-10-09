//! The sound card for calls (cpal): the default microphone and speakers, converted to and from 48 kHz mono.
//!
//! cpal streams are not `Send` on every platform, so a small thread owns them until the call is over.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use tokio::sync::mpsc;

use crate::calls::{AudioFactory, AudioSession};
use crate::rtc_peer::{Playback, FRAME};

const RATE: f32 = 48000.0;
/* what we keep for the speakers: more than this is dropped (the oldest first), so a stall never turns into delay */
const MAX_QUEUED: usize = 48000 / 5;

/// Mono samples at `from` Hz to 48 kHz by linear interpolation; `carry` is the previous sample so frames join up.
struct Resampler {
    ratio: f32, /* input samples per output sample */
    pos: f32,
    last: f32,
}

impl Resampler {
    fn new(from: f32, to: f32) -> Resampler { Resampler { ratio: from / to, pos: 0.0, last: 0.0 } }

    fn run(&mut self, input: &[f32], mut out: impl FnMut(f32)) {
        for &x in input {
            while self.pos < 1.0 {
                out(self.last + (x - self.last) * self.pos);
                self.pos += self.ratio;
            }
            self.pos -= 1.0;
            self.last = x;
        }
    }
}

fn to_i16(x: f32) -> i16 { (x.clamp(-1.0, 1.0) * 32767.0) as i16 }

/// The device called `wanted`, or `None` (use the default) when nothing is chosen or it is not there any more.
fn choose<T: std::fmt::Display>(devices: Vec<T>, wanted: &str) -> Option<T> {
    if wanted.is_empty() {
        return None;
    }
    devices.into_iter().find(|d| d.to_string() == wanted)
}

/// Names of the microphones and of the speakers, for the preferences.
pub fn devices() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let names = |it: Result<Vec<cpal::Device>, _>| -> Vec<String> {
        let mut v: Vec<String> = it.map(|d| d.iter().map(|d| d.to_string()).collect()).unwrap_or_default();
        v.sort();
        v.dedup();
        v
    };
    (names(host.input_devices().map(|i| i.collect())), names(host.output_devices().map(|i| i.collect())))
}

fn open(microphone: &str, speakers: &str) -> Result<(mpsc::Receiver<Vec<i16>>, Playback, mpsc::Sender<()>), String> {
    let host = cpal::default_host();
    let input = match host.input_devices().ok().and_then(|d| choose(d.collect(), microphone)) {
        Some(d) => d,
        None => host.default_input_device().ok_or("no microphone")?,
    };
    let output = match host.output_devices().ok().and_then(|d| choose(d.collect(), speakers)) {
        Some(d) => d,
        None => host.default_output_device().ok_or("no speakers")?,
    };
    let in_cfg = input.default_input_config().map_err(|e| format!("microphone: {e}"))?;
    let out_cfg = output.default_output_config().map_err(|e| format!("speakers: {e}"))?;

    let (frames_tx, frames_rx) = mpsc::channel::<Vec<i16>>(32);
    let queue: Arc<Mutex<VecDeque<i16>>> = Default::default();

    /* microphone: any channel count and sample rate -> 20 ms frames of 48 kHz mono */
    let in_channels = in_cfg.channels() as usize;
    let mut resampler = Resampler::new(in_cfg.sample_rate() as f32, RATE);
    let mut pending: Vec<i16> = Vec::with_capacity(FRAME * 2);
    let mut mono: Vec<f32> = Vec::new();
    let mut feed = move |samples: &mut dyn Iterator<Item = f32>| {
        mono.clear();
        let mut acc = 0.0;
        let mut n = 0;
        for s in samples {
            acc += s;
            n += 1;
            if n == in_channels { mono.push(acc / in_channels as f32); acc = 0.0; n = 0; }
        }
        resampler.run(&mono, |x| pending.push(to_i16(x)));
        while pending.len() >= FRAME {
            let frame: Vec<i16> = pending.drain(..FRAME).collect();
            let _ = frames_tx.try_send(frame);
        }
    };
    let cfg: StreamConfig = in_cfg.clone().into();
    let err = |e: cpal::Error| eprintln!("call: microphone: {e}");
    let in_stream = match in_cfg.sample_format() {
        SampleFormat::F32 => input.build_input_stream(cfg, move |d: &[f32], _| feed(&mut d.iter().copied()), err, None),
        SampleFormat::I16 => input.build_input_stream(cfg, move |d: &[i16], _| feed(&mut d.iter().map(|&s| s as f32 / 32768.0)), err, None),
        other => return Err(format!("microphone format {other:?} is not supported")),
    }
    .map_err(|e| format!("microphone: {e}"))?;

    /* speakers: 48 kHz mono queue -> the device's rate and channels */
    let out_channels = out_cfg.channels() as usize;
    let step = RATE / out_cfg.sample_rate() as f32; /* queue samples per output frame */
    let q = queue.clone();
    let (mut pos, mut prev, mut cur) = (0.0f32, 0.0f32, 0.0f32);
    let mut next_frame = move || -> f32 {
        let mut q = q.lock().unwrap();
        while pos >= 1.0 {
            prev = cur;
            cur = q.pop_front().map(|s| s as f32 / 32768.0).unwrap_or(0.0);
            pos -= 1.0;
        }
        let v = prev + (cur - prev) * pos;
        pos += step;
        v
    };
    let ocfg: StreamConfig = out_cfg.clone().into();
    let oerr = |e: cpal::Error| eprintln!("call: speakers: {e}");
    let out_stream = match out_cfg.sample_format() {
        SampleFormat::F32 => output.build_output_stream(ocfg, move |d: &mut [f32], _| {
            for f in d.chunks_mut(out_channels) { let v = next_frame(); f.iter_mut().for_each(|s| *s = v); }
        }, oerr, None),
        SampleFormat::I16 => output.build_output_stream(ocfg, move |d: &mut [i16], _| {
            for f in d.chunks_mut(out_channels) { let v = to_i16(next_frame()); f.iter_mut().for_each(|s| *s = v); }
        }, oerr, None),
        other => return Err(format!("speakers format {other:?} is not supported")),
    }
    .map_err(|e| format!("speakers: {e}"))?;

    in_stream.play().map_err(|e| format!("microphone: {e}"))?;
    out_stream.play().map_err(|e| format!("speakers: {e}"))?;

    /* the streams live on this thread until the call ends */
    let (stop_tx, mut stop_rx) = mpsc::channel::<()>(1);
    std::thread::spawn(move || {
        let _keep = (in_stream, out_stream);
        let _ = stop_rx.blocking_recv();
    });
    let playback: Playback = Arc::new(move |pcm: &[i16]| {
        let mut q = queue.lock().unwrap();
        q.extend(pcm.iter().copied());
        let over = q.len().saturating_sub(MAX_QUEUED);
        if over > 0 { q.drain(..over); }
    });
    Ok((frames_rx, playback, stop_tx))
}

/// The microphone and speakers the user chose (`pick` gives their names, empty for the defaults, each time a call starts).
pub fn sound_card(pick: Arc<dyn Fn() -> (String, String) + Send + Sync>) -> AudioFactory {
    Arc::new(move || {
        let (microphone, speakers) = pick();
        let (capture, playback, stop) = open(&microphone, &speakers)?;
        Ok(AudioSession { capture, playback, guard: Box::new(stop) })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chosen_device_is_found_by_name_and_anything_else_means_the_default() {
        let names = vec!["Built-in".to_string(), "USB headset".to_string()];
        assert_eq!(choose(names.clone(), "USB headset"), Some("USB headset".to_string()));
        assert_eq!(choose(names.clone(), ""), None);
        assert_eq!(choose(names, "unplugged"), None);
    }

    #[test]
    fn resampling_keeps_the_pitch_and_the_length() {
        for rate in [44100.0f32, 48000.0, 16000.0, 96000.0] {
            let mut r = Resampler::new(rate, RATE);
            let input: Vec<f32> = (0..(rate as usize / 10)).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / rate).sin()).collect();
            let mut out = Vec::new();
            r.run(&input, |x| out.push(x));
            let expect = (RATE as usize) / 10;
            assert!((out.len() as i64 - expect as i64).abs() <= 2, "{rate}: {} vs {expect}", out.len());
            let crossings = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
            assert!((crossings as i64 - 44).abs() <= 2, "{rate}: {crossings} cycles in 100 ms");
        }
    }
}
