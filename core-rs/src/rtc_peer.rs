//! One audio WebRTC connection (webrtc-rs): Opus over RTP, in both directions.
//!
//! The peer knows nothing about Matrix: callers pass SDP strings in and out. Audio goes through two plain
//! interfaces so that tests and the sound card use the same code: `capture` delivers 20 ms mono frames
//! (960 samples at 48 kHz) and `playback` is called with each decoded frame.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rtc::interceptor::Registry;
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_OPUS};
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceServer;
use rtc::rtp_transceiver::rtp_sender::{
    RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind,
};
use tokio::sync::{mpsc, watch};
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{
    PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState,
};
use webrtc::runtime::{default_runtime, Runtime};

/// Samples in one 20 ms mono frame at 48 kHz.
pub const FRAME: usize = 960;

pub type Playback = Arc<dyn Fn(&[i16]) + Send + Sync>;

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

impl CallPeer {
    /// `ice` are STUN/TURN urls with optional credentials; `bind` the local UDP addresses.
    pub async fn new(
        ice: Vec<(Vec<String>, String, String)>,
        bind: Vec<String>,
        mut capture: mpsc::Receiver<Vec<i16>>,
        playback: Playback,
    ) -> Result<CallPeer, String> {
        let e = |x: &dyn std::fmt::Display| x.to_string();
        let runtime = default_runtime().ok_or("no async runtime")?;
        let mut engine = MediaEngine::default();
        let codec = opus_codec();
        engine.register_codec(codec.clone(), RtpCodecKind::Audio).map_err(|x| e(&x))?;
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
        let handler = Arc::new(Handler { events: events_tx, gathered: gathered_tx, playback, runtime: runtime.clone() });
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
                    "vector-audio".into(),
                    "vector-audio-track".into(),
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

        Ok(CallPeer { pc, gathered, muted, events: Mutex::new(Some(events_rx)) })
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
        let a = CallPeer::new(vec![], bind(), rx_a, nothing).await.unwrap();
        let b = CallPeer::new(vec![], bind(), rx_b, hear).await.unwrap();
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
}
