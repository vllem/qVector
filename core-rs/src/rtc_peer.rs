//! One WebRTC connection (webrtc-rs): Opus audio and, for video calls, H.264 video over RTP, in both directions.
//!
//! The peer knows nothing about Matrix: callers pass SDP strings in and out. Audio goes through two plain
//! interfaces so that tests and the sound card use the same code: `capture` delivers 20 ms mono frames
//! (960 samples at 48 kHz) and `playback` is called with each decoded frame. Video goes in through
//! `push_video` (RGBA pictures from the camera) and comes out through the `VideoDisplay` closure.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rtc::interceptor::Registry;
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_H264, MIME_TYPE_OPUS, MIME_TYPE_VP8};
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceServer;
use rtc::rtp::codec::h264::H264Packet;
use rtc::rtp::codec::vp8::Vp8Packet;
use rtc::rtp::packetizer::Depacketizer;
use rtc::rtp_transceiver::rtp_sender::{
    RTCPFeedback, RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tokio::sync::{mpsc, watch};
use rtc::rtp::packetizer::{new_packetizer, Packetizer};
use rtc::rtp::sequence::new_random_sequencer;
use webrtc::media_stream::track_local::static_rtp::TrackLocalStaticRTP;
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
};
use webrtc::runtime::{default_runtime, Runtime};

use crate::video::{Codec, DecoderThread, EncoderThread, Frame};

/// Samples in one 20 ms mono frame at 48 kHz.
pub const FRAME: usize = 960;

pub type Playback = Arc<dyn Fn(&[i16]) + Send + Sync>;
/// Receives each decoded picture of the other side (on a decoder thread).
pub type VideoDisplay = Arc<dyn Fn(Frame) + Send + Sync>;

/// The stream both tracks belong to, so that the other client shows one feed with sound and picture.
const STREAM: &str = "vector-call";

/// Whether an SDP carries a video section.
pub fn has_video(sdp: &str) -> bool {
    sdp.lines().any(|l| l.starts_with("m=video") && !l.starts_with("m=video 0 "))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerEvent {
    Connected,
    Failed,
    Closed,
}

struct Handler {
    events: mpsc::UnboundedSender<PeerEvent>,
    gathered: watch::Sender<bool>,
    playback: Playback,
    display: Option<VideoDisplay>,
    runtime: Arc<dyn Runtime>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.send(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        let _ = self.events.send(match state {
            RTCPeerConnectionState::Connected => PeerEvent::Connected,
            RTCPeerConnectionState::Failed => PeerEvent::Failed,
            RTCPeerConnectionState::Closed | RTCPeerConnectionState::Disconnected => PeerEvent::Closed,
            _ => return,
        });
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        if track.kind().await == RtpCodecKind::Video {
            let Some(display) = self.display.clone() else { return };
            self.runtime.spawn(Box::pin(async move {
                let mut decoder: Option<(Codec, DecoderThread)> = None;
                let mut h264 = H264Packet::default();
                let mut vp8 = Vp8Packet::default();
                let mut unit: Vec<u8> = Vec::new();
                let mut last_seq: Option<u16> = None;
                let mut waiting_for_key = true;
                let mut asked = Instant::now() - Duration::from_secs(10);
                while let Some(ev) = track.poll().await {
                    let TrackRemoteEvent::OnRtpPacket(p) = ev else { continue };
                    if decoder.is_none() {
                        let mime = track.codec(p.header.ssrc).await.map(|c| c.mime_type).unwrap_or_default();
                        let Some(codec) = Codec::from_mime(&mime) else { continue };
                        decoder = Some((codec, DecoderThread::spawn(codec, display.clone())));
                    }
                    let Some((codec, decoder)) = decoder.as_ref() else { continue };
                    let gap = last_seq.map(|l| p.header.sequence_number != l.wrapping_add(1)).unwrap_or(false);
                    last_seq = Some(p.header.sequence_number);
                    if gap {
                        unit.clear();
                        waiting_for_key = true;
                    }
                    if waiting_for_key && asked.elapsed() > Duration::from_secs(1) {
                        asked = Instant::now();
                        let pli = rtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication {
                            sender_ssrc: 0,
                            media_ssrc: p.header.ssrc,
                        };
                        let _ = track.write_rtcp(vec![Box::new(pli)]).await;
                    }
                    let part = match codec {
                        Codec::H264 => h264.depacketize(&p.payload),
                        Codec::Vp8 => vp8.depacketize(&p.payload),
                    };
                    if let Ok(b) = part {
                        unit.extend_from_slice(&b);
                    }
                    if p.header.marker {
                        let done = std::mem::take(&mut unit);
                        if waiting_for_key && codec.is_key(&done) {
                            waiting_for_key = false;
                        }
                        if !waiting_for_key && !done.is_empty() {
                            decoder.push(done);
                        }
                    }
                }
            }));
            return;
        }
        let playback = self.playback.clone();
        self.runtime.spawn(Box::pin(async move {
            let Ok(mut dec) = opus::Decoder::new(48000, opus::Channels::Mono) else { return };
            let mut pcm = vec![0i16; 5760];
            while let Some(ev) = track.poll().await {
                if let TrackRemoteEvent::OnRtpPacket(p) = ev {
                    if let Ok(n) = dec.decode(&p.payload, &mut pcm, false) {
                        playback(&pcm[..n]);
                    }
                }
            }
        }));
    }
}

pub struct CallPeer {
    pc: Arc<dyn PeerConnection>,
    gathered: watch::Receiver<bool>,
    muted: Arc<AtomicBool>,
    camera: Arc<AtomicBool>,
    video_out: Option<EncoderThread>,
    pub events: Mutex<Option<mpsc::UnboundedReceiver<PeerEvent>>>,
}

fn opus_codec() -> RTCRtpCodecParameters {
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_OPUS.to_owned(),
            clock_rate: 48000,
            channels: 2,
            sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
            rtcp_feedback: vec![],
        },
        payload_type: 111,
        ..Default::default()
    }
}

fn vp8_codec() -> RTCRtpCodecParameters {
    let fb = |typ: &str, parameter: &str| RTCPFeedback { typ: typ.into(), parameter: parameter.into() };
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_VP8.to_owned(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: String::new(),
            rtcp_feedback: vec![fb("nack", ""), fb("nack", "pli"), fb("ccm", "fir")],
        },
        payload_type: 96,
        ..Default::default()
    }
}

fn h264_codec() -> RTCRtpCodecParameters {
    let fb = |typ: &str, parameter: &str| RTCPFeedback { typ: typ.into(), parameter: parameter.into() };
    RTCRtpCodecParameters {
        rtp_codec: RTCRtpCodec {
            mime_type: MIME_TYPE_H264.to_owned(),
            clock_rate: 90000,
            channels: 0,
            sdp_fmtp_line: "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".to_owned(),
            rtcp_feedback: vec![fb("nack", ""), fb("nack", "pli"), fb("ccm", "fir")],
        },
        payload_type: 102,
        ..Default::default()
    }
}

impl CallPeer {
    /// `ice` are STUN/TURN urls with optional credentials; `bind` the local UDP addresses. With a `video` display the
    /// connection carries pictures too (offered, or accepted from an offer).
    pub async fn new(
        ice: Vec<(Vec<String>, String, String)>,
        bind: Vec<String>,
        capture: mpsc::Receiver<Vec<i16>>,
        playback: Playback,
        video: Option<VideoDisplay>,
    ) -> Result<CallPeer, String> {
        Self::with_codecs(ice, bind, capture, playback, video, &[Codec::Vp8, Codec::H264]).await
    }

    /// Like `new`, with the video codecs this side offers, best first (tests use it to force a fallback).
    pub async fn with_codecs(
        ice: Vec<(Vec<String>, String, String)>,
        bind: Vec<String>,
        mut capture: mpsc::Receiver<Vec<i16>>,
        playback: Playback,
        video: Option<VideoDisplay>,
        video_codecs: &[Codec],
    ) -> Result<CallPeer, String> {
        let e = |x: &dyn std::fmt::Display| x.to_string();
        let runtime = default_runtime().ok_or("no async runtime")?;
        let mut engine = MediaEngine::default();
        let codec = opus_codec();
        engine.register_codec(codec.clone(), RtpCodecKind::Audio).map_err(|x| e(&x))?;
        // VP8 first: the offer lists it first, so it wins when both sides have it.
        let mut video_params = Vec::new();
        for c in video_codecs {
            let params = if *c == Codec::Vp8 { vp8_codec() } else { h264_codec() };
            engine.register_codec(params.clone(), RtpCodecKind::Video).map_err(|x| e(&x))?;
            video_params.push(params);
        }
        let first_video = video_params.first().cloned().unwrap_or_else(vp8_codec);
        let registry = register_default_interceptors(Registry::new(), &mut engine).map_err(|x| e(&x))?;
        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(
                ice.into_iter()
                    .map(|(urls, username, credential)| RTCIceServer { urls, username, credential, ..Default::default() })
                    .collect(),
            )
            .build();
        let (events_tx, events_rx) = mpsc::unbounded_channel();
        let (gathered_tx, gathered) = watch::channel(false);
        let handler = Arc::new(Handler { events: events_tx, gathered: gathered_tx, playback, display: video.clone(), runtime: runtime.clone() });
        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(engine)
            .with_interceptor_registry(registry)
            .with_handler(handler)
            .with_runtime(runtime.clone())
            .with_udp_addrs(bind)
            .build()
            .await
            .map_err(|x| e(&x))?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);

        let ssrc = rand::random::<u32>();
        let track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    STREAM.into(),
                    "vector-audio".into(),
                    "audio".into(),
                    RtpCodecKind::Audio,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(ssrc), ..Default::default() },
                        codec: codec.rtp_codec.clone(),
                        ..Default::default()
                    }],
                ),
            )
            .map_err(|x| e(&x))?,
        );
        let sender = pc.add_track(track.clone() as Arc<dyn TrackLocal>).await.map_err(|x| e(&x))?;

        // Send what the microphone delivers once the connection is up.
        let muted = Arc::new(AtomicBool::new(false));
        let send_muted = muted.clone();
        runtime.spawn(Box::pin(async move {
            let Ok(mut enc) = opus::Encoder::new(48000, opus::Channels::Mono, opus::Application::Voip) else { return };
            let mut out = vec![0u8; 1500];
            let mut payload_type = None;
            while let Some(frame) = capture.recv().await {
                if send_muted.load(Ordering::Relaxed) || frame.len() != FRAME {
                    continue;
                }
                if payload_type.is_none() {
                    payload_type = match sender.get_parameters().await {
                        Ok(p) => p.rtp_parameters.codecs.first().map(|c| c.payload_type),
                        Err(_) => None,
                    };
                }
                let Some(pt) = payload_type else { continue };
                let Ok(n) = enc.encode(&frame, &mut out) else { continue };
                let _ = track
                    .sample_writer(ssrc, pt)
                    .write_sample(&Sample {
                        data: bytes::Bytes::copy_from_slice(&out[..n]),
                        duration: Duration::from_millis(20),
                        ..Sample::new(Instant::now())
                    })
                    .await;
            }
        }));

        let camera = Arc::new(AtomicBool::new(false));
        let video_out = if video.is_some() {
            let vssrc = rand::random::<u32>();
            // Packetising is done here, after the SDP exchange has chosen the codec: a sample track fixes it at creation.
            let vtrack = Arc::new(TrackLocalStaticRTP::new(MediaStreamTrack::new(
                STREAM.into(),
                "vector-video".into(),
                "video".into(),
                RtpCodecKind::Video,
                vec![RTCRtpEncodingParameters {
                    rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(vssrc), ..Default::default() },
                    codec: first_video.rtp_codec.clone(),
                    ..Default::default()
                }],
            )));
            let vsender = pc.add_track(vtrack.clone() as Arc<dyn TrackLocal>).await.map_err(|x| e(&x))?;
            // The track names one codec; the section must offer (or accept) all of ours.
            let mine: rtc::rtp_transceiver::RTCRtpTransceiverId = vsender.id().into();
            for t in pc.get_transceivers().await {
                if t.id() == mine {
                    let _ = t.set_codec_preferences(video_params.clone()).await;
                }
            }
            let (units_tx, mut units) = mpsc::channel::<Vec<u8>>(4);
            let encoder = EncoderThread::spawn(Codec::Vp8, units_tx);
            let control = encoder.clone();
            runtime.spawn(Box::pin(async move {
                let mut sending: Option<Box<dyn Packetizer>> = None;
                let mut last = Instant::now();
                while let Some(unit) = units.recv().await {
                    if sending.is_none() {
                        let Ok(params) = vsender.get_parameters().await else { continue };
                        let Some(negotiated) = params.rtp_parameters.codecs.first().cloned() else { continue };
                        let Some(codec) = Codec::from_mime(&negotiated.rtp_codec.mime_type) else { continue };
                        let Ok(payloader) = negotiated.rtp_codec.payloader() else { continue };
                        sending = Some(Box::new(new_packetizer(
                            Instant::now(),
                            1200,
                            negotiated.payload_type,
                            vssrc,
                            payloader,
                            Box::new(new_random_sequencer()),
                            negotiated.rtp_codec.clock_rate,
                        )));
                        if codec != control.codec() {
                            // This frame was coded for another codec: switch and start over with a key frame.
                            control.set_codec(codec);
                            control.key();
                            continue;
                        }
                    }
                    let Some(packetizer) = sending.as_mut() else { continue };
                    let now = Instant::now();
                    let duration = now.duration_since(last).clamp(Duration::from_millis(10), Duration::from_millis(500));
                    last = now;
                    let samples = (duration.as_secs_f64() * 90000.0) as u32;
                    let Ok(packets) = packetizer.packetize(now, &bytes::Bytes::from(unit), samples) else { continue };
                    for pkt in packets {
                        let _ = vtrack.write_rtp_with_extensions(pkt, &[]).await;
                    }
                }
            }));
            Some(encoder)
        } else {
            None
        };

        Ok(CallPeer { pc, gathered, muted, camera, video_out, events: Mutex::new(Some(events_rx)) })
    }

    /// A picture from the camera; sent when the camera is on (and the call carries video).
    pub fn push_video(&self, f: Frame) {
        if let (Some(out), true) = (&self.video_out, self.camera.load(Ordering::Relaxed)) {
            out.push(f);
        }
    }

    /// Start or stop sending pictures. Starting begins with a key frame.
    pub fn set_camera(&self, on: bool) {
        if on && !self.camera.swap(true, Ordering::Relaxed) {
            if let Some(out) = &self.video_out {
                out.key();
            }
        } else if !on {
            self.camera.store(false, Ordering::Relaxed);
        }
    }

    pub fn has_video_out(&self) -> bool {
        self.video_out.is_some()
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }

    async fn local_sdp(&self) -> Result<String, String> {
        let mut g = self.gathered.clone();
        let _ = tokio::time::timeout(Duration::from_secs(4), async {
            while !*g.borrow() {
                if g.changed().await.is_err() {
                    break;
                }
            }
        })
        .await;
        let d = self.pc.local_description().await.ok_or("no local description")?;
        Ok(d.sdp)
    }

    /// Offer with all candidates gathered into the SDP.
    pub async fn offer(&self) -> Result<String, String> {
        let o = self.pc.create_offer(None).await.map_err(|x| x.to_string())?;
        self.pc.set_local_description(o).await.map_err(|x| x.to_string())?;
        self.local_sdp().await
    }

    pub async fn answer(&self, offer_sdp: &str) -> Result<String, String> {
        let offer = RTCSessionDescription::offer(offer_sdp.to_string()).map_err(|x| x.to_string())?;
        self.pc.set_remote_description(offer).await.map_err(|x| x.to_string())?;
        let a = self.pc.create_answer(None).await.map_err(|x| x.to_string())?;
        self.pc.set_local_description(a).await.map_err(|x| x.to_string())?;
        self.local_sdp().await
    }

    pub async fn accept_answer(&self, sdp: &str) -> Result<(), String> {
        let a = RTCSessionDescription::answer(sdp.to_string()).map_err(|x| x.to_string())?;
        self.pc.set_remote_description(a).await.map_err(|x| x.to_string())
    }

    pub async fn add_candidate(&self, candidate: &str, mid: Option<String>, index: Option<u16>) {
        let init = rtc::peer_connection::transport::RTCIceCandidateInit {
            candidate: candidate.to_string(),
            sdp_mid: mid,
            sdp_mline_index: index,
            ..Default::default()
        };
        let _ = self.pc.add_ice_candidate(init).await;
    }

    pub async fn close(&self) {
        let _ = self.pc.close().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    fn tone(frame: usize) -> Vec<i16> {
        (0..FRAME).map(|i| (((frame * FRAME + i) as f32 * 0.1).sin() * 8000.0) as i16).collect()
    }

    /// A feeds a tone; B must hear loud frames, and A (silent) feeds nothing back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn audio_flows_between_two_peers_on_this_machine() {
        let heard = Arc::new(AtomicUsize::new(0));
        let loud = Arc::new(AtomicUsize::new(0));
        let (h, l) = (heard.clone(), loud.clone());
        let hear: Playback = Arc::new(move |pcm: &[i16]| {
            h.fetch_add(1, Ordering::Relaxed);
            if pcm.iter().any(|s| s.abs() > 2000) {
                l.fetch_add(1, Ordering::Relaxed);
            }
        });
        let nothing: Playback = Arc::new(|_: &[i16]| {});
        let (tx_a, rx_a) = mpsc::channel(16);
        let (_tx_b, rx_b) = mpsc::channel(16);
        let bind = || vec!["127.0.0.1:0".to_string()];
        let a = CallPeer::new(vec![], bind(), rx_a, nothing, None).await.unwrap();
        let b = CallPeer::new(vec![], bind(), rx_b, hear, None).await.unwrap();
        let offer = a.offer().await.unwrap();
        assert!(offer.contains("opus/48000/2"), "{offer}");
        let answer = b.answer(&offer).await.unwrap();
        a.accept_answer(&answer).await.unwrap();

        let mut ev_a = a.events.lock().unwrap().take().unwrap();
        let mut ev_b = b.events.lock().unwrap().take().unwrap();
        for ev in [&mut ev_a, &mut ev_b] {
            let up = tokio::time::timeout(Duration::from_secs(10), async {
                while let Some(e) = ev.recv().await {
                    if e == PeerEvent::Connected {
                        return true;
                    }
                }
                false
            })
            .await;
            assert_eq!(up, Ok(true), "peers did not connect");
        }
        for n in 0..100 {
            tx_a.send(tone(n)).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(heard.load(Ordering::Relaxed) > 50, "heard {}", heard.load(Ordering::Relaxed));
        assert!(loud.load(Ordering::Relaxed) > 40, "loud {}", loud.load(Ordering::Relaxed));
        a.close().await;
        b.close().await;
    }

    /// A films a moving square; B must see pictures of the right size and brightness. Run with VP8 (both sides have it),
    /// and with a B that only knows H.264: the fallback must negotiate and carry pictures too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pictures_flow_between_two_peers_on_this_machine() {
        picture_round(&[Codec::Vp8, Codec::H264], &[Codec::Vp8, Codec::H264], "VP8").await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pictures_fall_back_to_h264_when_the_other_side_has_no_vp8() {
        picture_round(&[Codec::Vp8, Codec::H264], &[Codec::H264], "H264").await;
    }

    async fn picture_round(a_codecs: &[Codec], b_codecs: &[Codec], expect: &str) {
        use crate::video::tests::{luma, picture};
        let seen: Arc<Mutex<Vec<(u32, u32, f32)>>> = Default::default();
        let s2 = seen.clone();
        let display: VideoDisplay = Arc::new(move |f: Frame| s2.lock().unwrap().push((f.w, f.h, luma(&f))));
        let ignore: VideoDisplay = Arc::new(|_| {});
        let quiet: Playback = Arc::new(|_: &[i16]| {});
        let (_ta, rx_a) = mpsc::channel(16);
        let (_tb, rx_b) = mpsc::channel(16);
        let bind = || vec!["127.0.0.1:0".to_string()];
        let a = CallPeer::with_codecs(vec![], bind(), rx_a, quiet.clone(), Some(ignore), a_codecs).await.unwrap();
        let b = CallPeer::with_codecs(vec![], bind(), rx_b, quiet, Some(display), b_codecs).await.unwrap();
        let offer = a.offer().await.unwrap();
        assert!(has_video(&offer), "{offer}");
        assert!(offer.contains("VP8/90000") && offer.contains("H264/90000") && offer.contains("42e01f"), "{offer}");
        let answer = b.answer(&offer).await.unwrap();
        assert!(has_video(&answer), "{answer}");
        assert!(answer.contains(&format!("{expect}/90000")), "{answer}");
        a.accept_answer(&answer).await.unwrap();
        let mut ev_a = a.events.lock().unwrap().take().unwrap();
        let up = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(e) = ev_a.recv().await {
                if e == PeerEvent::Connected {
                    return true;
                }
            }
            false
        })
        .await;
        assert_eq!(up, Ok(true), "peers did not connect");

        a.push_video(picture(0, 320, 240));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(seen.lock().unwrap().is_empty(), "nothing is sent while the camera is off");
        a.set_camera(true);
        for n in 0..60 {
            a.push_video(picture(n, 320, 240));
            tokio::time::sleep(Duration::from_millis(66)).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        let seen = seen.lock().unwrap().clone();
        assert!(seen.len() > 20, "{expect}: pictures seen: {}", seen.len());
        assert!(seen.iter().all(|&(w, h, _)| (w, h) == (320, 240)));
        let mean = seen.iter().map(|x| x.2).sum::<f32>() / seen.len() as f32;
        let want = luma(&picture(10, 320, 240));
        assert!((mean - want).abs() < 6.0, "{expect}: mean {mean} want {want}");
        assert!(a.has_video_out());
        a.set_camera(false);
        a.close().await;
        b.close().await;
    }

    #[test]
    fn key_frames_are_recognised_in_an_access_unit() {
        assert!(crate::video::has_key(&[0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x65, 9]));
        assert!(crate::video::has_key(&[0, 0, 1, 0x65, 9]));
        assert!(!crate::video::has_key(&[0, 0, 0, 1, 0x41, 1, 2, 3]));
        assert!(!crate::video::has_key(&[]));
    }
}
