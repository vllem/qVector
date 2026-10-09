//! One-to-one voice calls over Matrix (VoIP version 1: `m.call.invite|answer|candidates|hangup|reject`).
//!
//! The signalling goes through the room like any message (so it is end-to-end encrypted in encrypted rooms); the
//! audio is an Opus WebRTC connection (`rtc_peer`). Candidates are gathered before the invite or answer is sent and
//! travel inside the SDP; candidates that the other side sends later are added as they arrive.

use std::any::Any;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use matrix_sdk::ruma::events::AnySyncTimelineEvent;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, Room};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::rtc_peer::{has_video, CallPeer, PeerEvent, Playback, VideoDisplay};
use crate::video::Frame;

/// What the sound card gives and takes. `guard` keeps the streams alive until the call is over.
pub struct AudioSession {
    pub capture: mpsc::Receiver<Vec<i16>>,
    pub playback: Playback,
    pub guard: Box<dyn Any + Send>,
}

pub type AudioFactory = Arc<dyn Fn() -> Result<AudioSession, String> + Send + Sync>;

const RING_SECONDS: u64 = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Ringing,    /* incoming, not answered yet */
    Calling,    /* outgoing, waiting for an answer */
    Connecting, /* both sides know each other, media not flowing yet */
    Connected,
}

struct Active {
    call_id: String,
    room_id: String,
    party: String,
    user_id: String,
    name: String,
    incoming: bool,
    phase: Phase,
    offer: Option<String>,
    peer: Option<Arc<CallPeer>>,
    remote_set: bool,
    early: Vec<Value>,
    muted: bool,
    audio: Option<Box<dyn Any + Send>>,
    /// The call carries pictures; whether our camera is on; whether the other side shows a picture.
    video: bool,
    camera: bool,
    remote_video: bool,
    /// We sent a `m.call.negotiate` offer and wait for the answer.
    negotiating: bool,
}

#[derive(Clone)]
pub struct Calls {
    client: Client,
    report: Arc<dyn Fn(String) + Send + Sync>,
    audio: Arc<Mutex<AudioFactory>>,
    bind: Arc<Vec<String>>,
    video_sink: Arc<Mutex<Option<VideoDisplay>>>,
    group_flag: Arc<Mutex<Arc<std::sync::atomic::AtomicBool>>>,
    active: Arc<Mutex<Option<Active>>>,
}

/// STUN/TURN servers the homeserver offers for calls (none: only direct connections then).
pub(crate) async fn turn_servers(client: &Client) -> Vec<(Vec<String>, String, String)> {
    use matrix_sdk::ruma::api::client::voip::get_turn_server_info::v3::Request;
    match client.send(Request::new()).await {
        Ok(r) if !r.uris.is_empty() => vec![(r.uris, r.username, r.password)],
        _ => vec![],
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Calls {
    /// `report` gets `{"state": "incoming|outgoing|connecting|connected|ended", "room_id", "call_id", "user_id", "name", "reason", "muted"}`.
    pub fn new(client: Client, audio: AudioFactory, report: impl Fn(String) + Send + Sync + 'static) -> Calls {
        let calls = Calls {
            client: client.clone(),
            report: Arc::new(report),
            audio: Arc::new(Mutex::new(audio)),
            bind: Arc::new(vec!["0.0.0.0:0".to_string()]),
            video_sink: Default::default(),
            group_flag: Default::default(),
            active: Default::default(),
        };
        let handler = calls.clone();
        client.add_event_handler(move |ev: Raw<AnySyncTimelineEvent>, room: Room| {
            let c = handler.clone();
            async move {
                let Ok(v) = ev.deserialize_as_unchecked::<Value>() else { return };
                let ty = v["type"].as_str().unwrap_or("");
                if ty.starts_with("m.call.") {
                    c.on_event(&room, ty, &v).await;
                }
            }
        });
        calls
    }

    /// For tests: where the sockets bind (loopback) and a replacement for the sound card.
    pub fn with_bind(mut self, bind: Vec<String>) -> Calls {
        self.bind = Arc::new(bind);
        self
    }

    /// Where the other side's pictures go.
    pub fn set_video_sink(&self, sink: VideoDisplay) {
        *self.video_sink.lock().unwrap() = Some(sink);
    }

    pub fn set_audio(&self, audio: AudioFactory) {
        *self.audio.lock().unwrap() = audio;
    }

    fn emit(&self, state: &str, a: &Active, reason: &str) {
        (self.report)(
            json!({"state": state, "room_id": a.room_id, "call_id": a.call_id, "user_id": a.user_id, "name": a.name, "reason": reason, "muted": a.muted, "incoming": a.incoming, "video": a.video, "camera": a.camera, "remote_video": a.remote_video})
                .to_string(),
        );
    }

    fn me(&self) -> String {
        self.client.user_id().map(|u| u.to_string()).unwrap_or_default()
    }

    async fn ice_servers(&self) -> Vec<(Vec<String>, String, String)> {
        turn_servers(&self.client).await
    }

    /// Whether a one-to-one call is going on (ringing, connecting or connected).
    pub fn is_busy(&self) -> bool {
        self.active.lock().unwrap().is_some()
    }

    /// The flag a group call raises while we are in one: no one-to-one call can start or ring then.
    pub fn set_group_flag(&self, flag: Arc<std::sync::atomic::AtomicBool>) {
        *self.group_flag.lock().unwrap() = flag;
    }

    fn display(&self) -> VideoDisplay {
        self.video_sink.lock().unwrap().clone().unwrap_or_else(|| Arc::new(|_| {}))
    }

    async fn make_peer(&self, video: bool) -> Result<(Arc<CallPeer>, Box<dyn Any + Send>), String> {
        let factory = self.audio.lock().unwrap().clone();
        let session = factory().map_err(|e| format!("No sound device: {e}"))?;
        let display = if video { Some(self.display()) } else { None };
        let peer = CallPeer::new(self.ice_servers().await, (*self.bind).clone(), session.capture, session.playback, display).await?;
        Ok((Arc::new(peer), session.guard))
    }

    /// What each stream is (MSC3077), so that the other client knows our camera is off and shows our picture instead.
    fn metadata(a: &Active) -> Value {
        json!({"vector-call": {"purpose": "m.usermedia", "audio_muted": a.muted, "video_muted": !a.camera}})
    }

    async fn send(&self, room_id: &str, ty: &str, mut content: Value, call_id: &str, party: &str) {
        content["call_id"] = json!(call_id);
        content["party_id"] = json!(party);
        content["version"] = json!("1");
        let Ok(rid) = <&matrix_sdk::ruma::RoomId>::try_from(room_id) else { return };
        let Some(room) = self.client.get_room(rid) else { return };
        if let Err(e) = room.send_raw(ty, content).await {
            eprintln!("call: cannot send {ty}: {e}");
        }
    }

    /// Ring the other person in this room (a direct chat or any room with exactly two people).
    pub async fn place(&self, room_id: &str, video: bool) -> Result<(), String> {
        if self.active.lock().unwrap().is_some() || self.group_flag.lock().unwrap().load(std::sync::atomic::Ordering::Relaxed) {
            return Err("You are already in a call".into());
        }
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(|e| e.to_string())?;
        let room = self.client.get_room(rid).ok_or("No such room")?;
        let me = self.me();
        let members = room.members(matrix_sdk::RoomMemberships::JOIN).await.map_err(|e| e.to_string())?;
        let others: Vec<_> = members.iter().filter(|m| m.user_id().as_str() != me).collect();
        if others.len() != 1 {
            return Err("Calls work in a chat with one other person".into());
        }
        let (user_id, name) = (others[0].user_id().to_string(), others[0].display_name().unwrap_or(others[0].user_id().as_str()).to_string());
        let call_id = format!("c{}", rand::random::<u64>());
        let party = format!("p{}", rand::random::<u32>());
        let (peer, guard) = self.make_peer(video).await?;
        let a = Active {
            call_id: call_id.clone(),
            room_id: room_id.into(),
            party: party.clone(),
            user_id: user_id.clone(),
            name,
            incoming: false,
            phase: Phase::Calling,
            offer: None,
            peer: Some(peer.clone()),
            remote_set: false,
            early: vec![],
            muted: false,
            audio: Some(guard),
            video,
            camera: false,
            remote_video: video,
            negotiating: false,
        };
        self.emit("outgoing", &a, "");
        let meta = Self::metadata(&a);
        *self.active.lock().unwrap() = Some(a);
        let sdp = match peer.offer().await {
            Ok(s) => s,
            Err(e) => {
                self.finish(&call_id, "error", false).await;
                return Err(e);
            }
        };
        self.send(
            room_id,
            "m.call.invite",
            json!({"lifetime": RING_SECONDS * 1000, "offer": {"type": "offer", "sdp": sdp}, "invitee": user_id,
                "org.matrix.msc3077.sdp_stream_metadata": meta, "sdp_stream_metadata": meta}),
            &call_id,
            &party,
        )
        .await;
        self.watch(peer, call_id.clone());
        let me = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(RING_SECONDS)).await;
            let unanswered = me.active.lock().unwrap().as_ref().map(|a| a.call_id == call_id && a.phase == Phase::Calling).unwrap_or(false);
            if unanswered {
                me.finish(&call_id, "invite_timeout", true).await;
            }
        });
        Ok(())
    }

    /// Pick up the ringing call.
    pub async fn answer(&self) -> Result<(), String> {
        let (call_id, room_id, party, offer, video) = {
            let g = self.active.lock().unwrap();
            let a = g.as_ref().filter(|a| a.phase == Phase::Ringing).ok_or("Nobody is calling")?;
            (a.call_id.clone(), a.room_id.clone(), a.party.clone(), a.offer.clone().unwrap_or_default(), a.video)
        };
        let (peer, guard) = match self.make_peer(video).await {
            Ok(p) => p,
            Err(e) => {
                self.finish(&call_id, "error", true).await;
                return Err(e);
            }
        };
        let sdp = match peer.answer(&offer).await {
            Ok(s) => s,
            Err(e) => {
                self.finish(&call_id, "error", true).await;
                return Err(e);
            }
        };
        let early = {
            let mut g = self.active.lock().unwrap();
            match g.as_mut().filter(|a| a.call_id == call_id) {
                Some(a) => {
                    a.peer = Some(peer.clone());
                    a.audio = Some(guard);
                    a.phase = Phase::Connecting;
                    a.remote_set = true;
                    self.emit("connecting", a, "");
                    Some((std::mem::take(&mut a.early), Self::metadata(a)))
                }
                None => None,
            }
        };
        let Some((early, meta)) = early else {
            peer.close().await;
            return Err("The call ended".into());
        };
        for c in early {
            Self::add_candidates(&peer, &c).await;
        }
        self.send(&room_id, "m.call.answer", json!({"answer": {"type": "answer", "sdp": sdp},
            "org.matrix.msc3077.sdp_stream_metadata": meta, "sdp_stream_metadata": meta}), &call_id, &party).await;
        self.watch(peer, call_id);
        Ok(())
    }

    /// Decline a ringing call or end the current one.
    pub async fn hangup(&self) {
        let id = self.active.lock().unwrap().as_ref().map(|a| a.call_id.clone());
        if let Some(id) = id {
            self.finish(&id, "user_hangup", true).await;
        }
    }

    pub fn set_muted(&self, muted: bool) {
        let mut g = self.active.lock().unwrap();
        if let Some(a) = g.as_mut() {
            a.muted = muted;
            if let Some(p) = &a.peer {
                p.set_muted(muted);
            }
            self.emit(Self::state_name(a), a, "");
        }
    }

    fn state_name(a: &Active) -> &'static str {
        if a.phase == Phase::Connected { "connected" } else { "connecting" }
    }

    /// A picture from the camera (ignored unless the camera is on).
    pub fn push_video(&self, f: Frame) {
        let peer = self.active.lock().unwrap().as_ref().and_then(|a| if a.camera { a.peer.clone() } else { None });
        if let Some(p) = peer {
            p.push_video(f);
        }
    }

    /// Turn a connected voice call into a video call: a new offer goes over `m.call.negotiate`; the camera stays off until switched on.
    pub async fn add_video(&self) -> Result<(), String> {
        let (peer, room_id, call_id, party) = {
            let g = self.active.lock().unwrap();
            let a = g.as_ref().filter(|a| a.phase == Phase::Connected && !a.video && !a.negotiating).ok_or("Video can be added to a connected voice call")?;
            (a.peer.clone().ok_or("No connection")?, a.room_id.clone(), a.call_id.clone(), a.party.clone())
        };
        peer.add_video(self.display()).await?;
        let sdp = peer.renegotiate().await?;
        let meta = {
            let mut g = self.active.lock().unwrap();
            let a = g.as_mut().filter(|a| a.call_id == call_id).ok_or("The call ended")?;
            a.video = true;
            a.camera = false;
            a.remote_video = true;
            a.negotiating = true;
            self.emit(Self::state_name(a), a, "");
            Self::metadata(a)
        };
        self.send(&room_id, "m.call.negotiate", json!({"description": {"type": "offer", "sdp": sdp}, "lifetime": 60000,
            "org.matrix.msc3077.sdp_stream_metadata": meta, "sdp_stream_metadata": meta}), &call_id, &party).await;
        Ok(())
    }

    /// Turn our camera on or off in a call that carries video; the other side is told so it can show a placeholder.
    pub async fn set_camera(&self, on: bool) {
        let (room_id, call_id, party, meta) = {
            let mut g = self.active.lock().unwrap();
            let Some(a) = g.as_mut().filter(|a| a.video && a.camera != on) else { return };
            a.camera = on;
            if let Some(p) = &a.peer {
                p.set_camera(on);
            }
            self.emit(Self::state_name(a), a, "");
            (a.room_id.clone(), a.call_id.clone(), a.party.clone(), Self::metadata(a))
        };
        self.send(&room_id, "m.call.sdp_stream_metadata_changed",
            json!({"org.matrix.msc3077.sdp_stream_metadata": meta, "sdp_stream_metadata": meta}), &call_id, &party).await;
    }

    /// End the call `call_id` (if it is still the current one); tell the other side when `notify`.
    async fn finish(&self, call_id: &str, reason: &str, notify: bool) {
        let a = {
            let mut g = self.active.lock().unwrap();
            if g.as_ref().map(|a| a.call_id != call_id).unwrap_or(true) {
                return;
            }
            g.take().unwrap()
        };
        if notify {
            let (ty, content) = if a.incoming && a.phase == Phase::Ringing {
                ("m.call.reject", json!({}))
            } else {
                ("m.call.hangup", json!({"reason": reason}))
            };
            self.send(&a.room_id, ty, content, &a.call_id, &a.party).await;
        }
        self.emit("ended", &a, reason);
        if let Some(p) = &a.peer {
            p.close().await;
        }
    }

    /// Follow the connection: connected, or lost.
    fn watch(&self, peer: Arc<CallPeer>, call_id: String) {
        let Some(mut events) = peer.events.lock().unwrap().take() else { return };
        let me = self.clone();
        tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                match ev {
                    PeerEvent::Connected => {
                        let mut g = me.active.lock().unwrap();
                        if let Some(a) = g.as_mut().filter(|a| a.call_id == call_id) {
                            a.phase = Phase::Connected;
                            me.emit("connected", a, "");
                        }
                    }
                    PeerEvent::Failed => {
                        me.finish(&call_id, "ice_failed", true).await;
                        return;
                    }
                    PeerEvent::Closed => {
                        me.finish(&call_id, "ice_timeout", true).await;
                        return;
                    }
                }
            }
        });
    }

    /// Whether the other side's metadata says its camera is on (`default` when it says nothing).
    fn remote_shows_picture(content: &Value, default: bool) -> bool {
        for key in ["org.matrix.msc3077.sdp_stream_metadata", "sdp_stream_metadata"] {
            if let Some(m) = content[key].as_object() {
                for stream in m.values() {
                    if let Some(muted) = stream["video_muted"].as_bool() {
                        return !muted;
                    }
                }
            }
        }
        default
    }

    async fn add_candidates(peer: &CallPeer, content: &Value) {
        for c in content["candidates"].as_array().cloned().unwrap_or_default() {
            let cand = c["candidate"].as_str().unwrap_or("");
            if cand.is_empty() {
                continue;
            }
            peer.add_candidate(cand, c["sdpMid"].as_str().map(String::from), c["sdpMLineIndex"].as_u64().map(|n| n as u16)).await;
        }
    }

    async fn on_event(&self, room: &Room, ty: &str, ev: &Value) {
        let content = &ev["content"];
        let sender = ev["sender"].as_str().unwrap_or("");
        let call_id = content["call_id"].as_str().unwrap_or("").to_string();
        if call_id.is_empty() || content[crate::group_calls::GROUP_KEY].is_string() {
            return; /* a group call's connection: group_calls.rs handles it */
        }
        let from_me = sender == self.me();
        match ty {
            "m.call.invite" => {
                if from_me && content["invitee"].as_str() != Some(&self.me()) {
                    return; /* we rang from another device of ours */
                }
                if self.active.lock().unwrap().as_ref().map(|a| a.call_id == call_id).unwrap_or(false) {
                    return; /* the echo of our own invite */
                }
                let age = now_ms().saturating_sub(ev["origin_server_ts"].as_u64().unwrap_or(now_ms()));
                let lifetime = content["lifetime"].as_u64().unwrap_or(60_000).min(120_000);
                if age > lifetime + 30_000 {
                    return; /* stopped ringing long ago */
                }
                let sdp = content["offer"]["sdp"].as_str().unwrap_or("").to_string();
                if sdp.is_empty() {
                    return;
                }
                if self.active.lock().unwrap().is_some() {
                    self.send(room.room_id().as_str(), "m.call.hangup", json!({"reason": "user_busy"}), &call_id, "busy").await;
                    return;
                }
                let name = match room.get_member_no_sync(<&matrix_sdk::ruma::UserId>::try_from(sender).unwrap_or(self.client.user_id().unwrap())).await {
                    Ok(Some(m)) => m.display_name().unwrap_or(sender).to_string(),
                    _ => sender.to_string(),
                };
                let sdp_text = sdp.clone();
                let a = Active {
                    call_id: call_id.clone(),
                    room_id: room.room_id().to_string(),
                    party: format!("p{}", rand::random::<u32>()),
                    user_id: sender.to_string(),
                    name,
                    incoming: true,
                    phase: Phase::Ringing,
                    offer: Some(sdp_text),
                    peer: None,
                    remote_set: false,
                    early: vec![],
                    muted: false,
                    audio: None,
                    video: has_video(&sdp),
                    camera: false,
                    remote_video: Self::remote_shows_picture(content, true),
                    negotiating: false,
                };
                {
                    let mut g = self.active.lock().unwrap();
                    if g.is_some() {
                        return;
                    }
                    self.emit("incoming", &a, "");
                    *g = Some(a);
                }
                let me = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(lifetime)).await;
                    let ringing = me.active.lock().unwrap().as_ref().map(|a| a.call_id == call_id && a.phase == Phase::Ringing).unwrap_or(false);
                    if ringing {
                        me.finish(&call_id, "invite_timeout", false).await;
                    }
                });
            }
            "m.call.answer" => {
                let (peer, ours) = {
                    let g = self.active.lock().unwrap();
                    match g.as_ref().filter(|a| a.call_id == call_id) {
                        Some(a) if !a.incoming && a.phase == Phase::Calling => (a.peer.clone(), true),
                        /* we picked up on another device of ours: stop ringing here */
                        Some(a) if a.incoming && a.phase == Phase::Ringing && from_me => (None, false),
                        _ => return,
                    }
                };
                if !ours {
                    self.finish(&call_id, "answered_elsewhere", false).await;
                    return;
                }
                let Some(peer) = peer else { return };
                let sdp = content["answer"]["sdp"].as_str().unwrap_or("");
                if peer.accept_answer(sdp).await.is_err() {
                    self.finish(&call_id, "error", true).await;
                    return;
                }
                let (early, party, room_id) = {
                    let mut g = self.active.lock().unwrap();
                    let Some(a) = g.as_mut().filter(|a| a.call_id == call_id) else { return };
                    a.phase = Phase::Connecting;
                    a.remote_set = true;
                    a.remote_video = a.video && Self::remote_shows_picture(content, true);
                    self.emit("connecting", a, "");
                    (std::mem::take(&mut a.early), a.party.clone(), a.room_id.clone())
                };
                for c in early {
                    Self::add_candidates(&peer, &c).await;
                }
                if let Some(p) = content["party_id"].as_str() {
                    self.send(&room_id, "m.call.select_answer", json!({"selected_party_id": p}), &call_id, &party).await;
                }
            }
            "m.call.candidates" => {
                let peer = {
                    let mut g = self.active.lock().unwrap();
                    let Some(a) = g.as_mut().filter(|a| a.call_id == call_id) else { return };
                    if from_me && a.incoming == false {
                        return;
                    }
                    if !a.remote_set {
                        a.early.push(content.clone());
                        return;
                    }
                    a.peer.clone()
                };
                if let Some(p) = peer {
                    Self::add_candidates(&p, content).await;
                }
            }
            "m.call.negotiate" => {
                if from_me {
                    return;
                }
                let kind = content["description"]["type"].as_str().unwrap_or("");
                let sdp = content["description"]["sdp"].as_str().unwrap_or("").to_string();
                let (peer, negotiating, room_id, party, polite) = {
                    let g = self.active.lock().unwrap();
                    match g.as_ref().filter(|a| a.call_id == call_id && a.phase == Phase::Connected) {
                        Some(a) => (a.peer.clone(), a.negotiating, a.room_id.clone(), a.party.clone(), a.incoming),
                        None => return,
                    }
                };
                let Some(peer) = peer else { return };
                /* Both sides offered at once: the callee is polite and takes the caller's offer instead of its own, the caller ignores ours. */
                if kind == "offer" && negotiating {
                    if !polite || peer.rollback().await.is_err() {
                        return;
                    }
                    if let Some(a) = self.active.lock().unwrap().as_mut().filter(|a| a.call_id == call_id) {
                        a.negotiating = false;
                    }
                }
                if kind == "offer" {
                    if has_video(&sdp) && !peer.has_video_out() && peer.add_video(self.display()).await.is_err() {
                        return;
                    }
                    let Ok(answer) = peer.answer(&sdp).await else { return };
                    let meta = {
                        let mut g = self.active.lock().unwrap();
                        let Some(a) = g.as_mut().filter(|a| a.call_id == call_id) else { return };
                        if has_video(&sdp) {
                            a.video = true;
                            a.remote_video = Self::remote_shows_picture(content, true);
                        }
                        self.emit(Self::state_name(a), a, "");
                        Self::metadata(a)
                    };
                    self.send(&room_id, "m.call.negotiate", json!({"description": {"type": "answer", "sdp": answer},
                        "org.matrix.msc3077.sdp_stream_metadata": meta, "sdp_stream_metadata": meta}), &call_id, &party).await;
                } else if kind == "answer" && negotiating {
                    if peer.accept_answer(&sdp).await.is_ok() {
                        let mut g = self.active.lock().unwrap();
                        if let Some(a) = g.as_mut().filter(|a| a.call_id == call_id) {
                            a.negotiating = false;
                            a.remote_video = Self::remote_shows_picture(content, true);
                            self.emit(Self::state_name(a), a, "");
                        }
                    }
                }
            }
            "m.call.sdp_stream_metadata_changed" => {
                if from_me {
                    return;
                }
                let mut g = self.active.lock().unwrap();
                if let Some(a) = g.as_mut().filter(|a| a.call_id == call_id && a.video) {
                    a.remote_video = Self::remote_shows_picture(content, a.remote_video);
                    self.emit(Self::state_name(a), a, "");
                }
            }
            "m.call.hangup" | "m.call.reject" => {
                let ours_to_end = {
                    let g = self.active.lock().unwrap();
                    match g.as_ref().filter(|a| a.call_id == call_id) {
                        /* our own other device declined: only matters while ringing */
                        Some(a) if from_me => a.incoming && a.phase == Phase::Ringing,
                        Some(_) => true,
                        None => false,
                    }
                };
                if ours_to_end {
                    let reason = content["reason"].as_str().unwrap_or(if ty == "m.call.reject" { "rejected" } else { "hangup" });
                    self.finish(&call_id, reason, false).await;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::FakeHs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A fake sound card: speaks a tone, counts the loud frames it hears.
    fn card(speak: bool, loud: Arc<AtomicUsize>) -> AudioFactory {
        Arc::new(move || {
            let (tx, rx) = mpsc::channel(8);
            let l = loud.clone();
            if speak {
                tokio::spawn(async move {
                    for n in 0usize.. {
                        let f: Vec<i16> = (0..960).map(|i| (((n * 960 + i) as f32 * 0.1).sin() * 8000.0) as i16).collect();
                        if tx.send(f).await.is_err() {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                });
            }
            Ok(AudioSession {
                capture: rx,
                playback: Arc::new(move |pcm: &[i16]| {
                    if pcm.iter().any(|s| s.abs() > 2000) {
                        l.fetch_add(1, Ordering::Relaxed);
                    }
                }),
                guard: Box::new(()),
            })
        })
    }

    fn states(log: &Arc<Mutex<Vec<Value>>>) -> Vec<String> {
        log.lock().unwrap().iter().map(|v| v["state"].as_str().unwrap_or("").to_string()).collect()
    }

    async fn pump(a: &Client, b: &Client, log_a: &Arc<Mutex<Vec<Value>>>, log_b: &Arc<Mutex<Vec<Value>>>, until: impl Fn(&[String], &[String]) -> bool) {
        for _ in 0..400 {
            crate::sync_once(a).await.unwrap();
            crate::sync_once(b).await.unwrap();
            if until(&states(log_a), &states(log_b)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out; alice {:?} bob {:?}", states(log_a), states(log_b));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_voice_call_rings_connects_carries_sound_and_ends() {
        let hs = FakeHs::start().await;
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let alice = crate::session::sign_in(da.path(), &hs.uri(), "alice", "x", None, "a").await.unwrap();
        let bob = crate::session::sign_in(db.path(), &hs.uri(), "bob", "x", None, "b").await.unwrap();
        crate::sync_once(&alice).await.unwrap();
        crate::sync_once(&bob).await.unwrap();
        let room = alice.joined_rooms().first().unwrap().room_id().to_string();

        let (la, lb): (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>) = Default::default();
        let (heard_by_alice, heard_by_bob) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (ca, cb) = (la.clone(), lb.clone());
        let lo = vec!["127.0.0.1:0".to_string()];
        let ac = Calls::new(alice.clone(), card(true, heard_by_alice.clone()), move |s| ca.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo.clone());
        let bc = Calls::new(bob.clone(), card(false, heard_by_bob.clone()), move |s| cb.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo);

        ac.place(&room, false).await.unwrap();
        assert_eq!(states(&la), ["outgoing"]);
        pump(&alice, &bob, &la, &lb, |_, b| b.contains(&"incoming".to_string())).await;
        let incoming = lb.lock().unwrap().iter().find(|v| v["state"] == "incoming").cloned().unwrap();
        assert_eq!(incoming["user_id"], "@alice:hs");
        bc.answer().await.unwrap();
        pump(&alice, &bob, &la, &lb, |a, b| a.contains(&"connected".to_string()) && b.contains(&"connected".to_string())).await;
        for _ in 0..60 {
            if heard_by_bob.load(Ordering::Relaxed) > 20 { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(heard_by_bob.load(Ordering::Relaxed) > 20, "bob heard {}", heard_by_bob.load(Ordering::Relaxed));
        bc.set_muted(true);
        ac.hangup().await;
        pump(&alice, &bob, &la, &lb, |_, b| b.last().map(|s| s == "ended").unwrap_or(false)).await;
        assert_eq!(states(&la).last().unwrap(), "ended");
        let unhandled: Vec<_> = hs.log().into_iter().filter(|l| l.contains("UNHANDLED") && !l.contains("well-known")).collect();
        assert!(unhandled.is_empty(), "{unhandled:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_video_call_carries_pictures_and_the_camera_state() {
        use crate::video::tests::{luma, picture};
        let hs = FakeHs::start().await;
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let alice = crate::session::sign_in(da.path(), &hs.uri(), "alice", "x", None, "a").await.unwrap();
        let bob = crate::session::sign_in(db.path(), &hs.uri(), "bob", "x", None, "b").await.unwrap();
        crate::sync_once(&alice).await.unwrap();
        crate::sync_once(&bob).await.unwrap();
        let room = alice.joined_rooms().first().unwrap().room_id().to_string();

        let (la, lb): (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>) = Default::default();
        let (ca, cb) = (la.clone(), lb.clone());
        let lo = vec!["127.0.0.1:0".to_string()];
        let quiet = || card(false, Arc::new(AtomicUsize::new(0)));
        let ac = Calls::new(alice.clone(), quiet(), move |s| ca.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo.clone());
        let bc = Calls::new(bob.clone(), quiet(), move |s| cb.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo);
        let seen: Arc<Mutex<Vec<(u32, u32, f32)>>> = Default::default();
        let s2 = seen.clone();
        bc.set_video_sink(Arc::new(move |f: Frame| s2.lock().unwrap().push((f.w, f.h, luma(&f)))));

        ac.place(&room, true).await.unwrap();
        pump(&alice, &bob, &la, &lb, |_, b| b.contains(&"incoming".to_string())).await;
        let incoming = lb.lock().unwrap().iter().find(|v| v["state"] == "incoming").cloned().unwrap();
        assert_eq!(incoming["video"], true);
        assert_eq!(incoming["camera"], false);
        bc.answer().await.unwrap();
        pump(&alice, &bob, &la, &lb, |a, b| a.contains(&"connected".to_string()) && b.contains(&"connected".to_string())).await;

        ac.set_camera(true).await;
        for n in 0..60 {
            ac.push_video(picture(n, 320, 240));
            tokio::time::sleep(Duration::from_millis(66)).await;
        }
        for _ in 0..20 {
            crate::sync_once(&bob).await.unwrap();
            if lb.lock().unwrap().iter().any(|v| v["remote_video"] == true) && seen.lock().unwrap().len() > 20 { break; }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(seen.lock().unwrap().len() > 20, "bob saw {} pictures", seen.lock().unwrap().len());
        assert!(seen.lock().unwrap().iter().all(|&(w, h, _)| (w, h) == (320, 240)));
        let last = lb.lock().unwrap().last().cloned().unwrap();
        assert_eq!(last["remote_video"], true);

        ac.set_camera(false).await;
        pump(&alice, &bob, &la, &lb, |_, _| lb.lock().unwrap().last().map(|v| v["remote_video"] == false).unwrap_or(false)).await;
        assert_eq!(la.lock().unwrap().last().unwrap()["camera"], false);
        bc.hangup().await;
        pump(&alice, &bob, &la, &lb, |a, _| a.last().map(|s| s == "ended").unwrap_or(false)).await;
        let unhandled: Vec<_> = hs.log().into_iter().filter(|l| l.contains("UNHANDLED") && !l.contains("well-known")).collect();
        assert!(unhandled.is_empty(), "{unhandled:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_voice_call_can_become_a_video_call() {
        use crate::video::tests::{luma, picture};
        let hs = FakeHs::start().await;
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let alice = crate::session::sign_in(da.path(), &hs.uri(), "alice", "x", None, "a").await.unwrap();
        let bob = crate::session::sign_in(db.path(), &hs.uri(), "bob", "x", None, "b").await.unwrap();
        crate::sync_once(&alice).await.unwrap();
        crate::sync_once(&bob).await.unwrap();
        let room = alice.joined_rooms().first().unwrap().room_id().to_string();

        let (la, lb): (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>) = Default::default();
        let (ca, cb) = (la.clone(), lb.clone());
        let lo = vec!["127.0.0.1:0".to_string()];
        let quiet = || card(false, Arc::new(AtomicUsize::new(0)));
        let ac = Calls::new(alice.clone(), quiet(), move |s| ca.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo.clone());
        let bc = Calls::new(bob.clone(), quiet(), move |s| cb.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo);
        let (seen_a, seen_b): (Arc<Mutex<Vec<f32>>>, Arc<Mutex<Vec<f32>>>) = Default::default();
        let (sa, sb) = (seen_a.clone(), seen_b.clone());
        ac.set_video_sink(Arc::new(move |f: Frame| sa.lock().unwrap().push(luma(&f))));
        bc.set_video_sink(Arc::new(move |f: Frame| sb.lock().unwrap().push(luma(&f))));

        ac.place(&room, false).await.unwrap();
        pump(&alice, &bob, &la, &lb, |_, b| b.contains(&"incoming".to_string())).await;
        bc.answer().await.unwrap();
        pump(&alice, &bob, &la, &lb, |a, b| a.contains(&"connected".to_string()) && b.contains(&"connected".to_string())).await;
        assert_eq!(lb.lock().unwrap().last().unwrap()["video"], false);
        assert!(ac.add_video().await.is_ok());
        assert!(ac.add_video().await.is_err(), "once is enough");
        pump(&alice, &bob, &la, &lb, |_, _| lb.lock().unwrap().last().map(|v| v["video"] == true).unwrap_or(false) && la.lock().unwrap().iter().any(|v| v["video"] == true)).await;
        for _ in 0..100 {
            crate::sync_once(&alice).await.unwrap();
            crate::sync_once(&bob).await.unwrap();
            if !ac.active.lock().unwrap().as_ref().map(|a| a.negotiating).unwrap_or(true) { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        ac.set_camera(true).await;
        bc.set_camera(true).await;
        for n in 0..70 {
            ac.push_video(picture(n, 320, 240));
            bc.push_video(picture(n, 320, 240));
            tokio::time::sleep(Duration::from_millis(66)).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(seen_b.lock().unwrap().len() > 15, "bob saw {} pictures", seen_b.lock().unwrap().len());
        assert!(seen_a.lock().unwrap().len() > 15, "alice saw {} pictures", seen_a.lock().unwrap().len());
        bc.hangup().await;
        pump(&alice, &bob, &la, &lb, |a, _| a.last().map(|s| s == "ended").unwrap_or(false)).await;
        let unhandled: Vec<_> = hs.log().into_iter().filter(|l| l.contains("UNHANDLED") && !l.contains("well-known")).collect();
        assert!(unhandled.is_empty(), "{unhandled:?}");
    }

    /// Both people press "Add video" at the same moment: the callee yields, and both ends get pictures.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn simultaneous_video_offers_settle_with_the_caller_winning() {
        use crate::video::tests::{luma, picture};
        let hs = FakeHs::start().await;
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let alice = crate::session::sign_in(da.path(), &hs.uri(), "alice", "x", None, "a").await.unwrap();
        let bob = crate::session::sign_in(db.path(), &hs.uri(), "bob", "x", None, "b").await.unwrap();
        crate::sync_once(&alice).await.unwrap();
        crate::sync_once(&bob).await.unwrap();
        let room = alice.joined_rooms().first().unwrap().room_id().to_string();

        let (la, lb): (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>) = Default::default();
        let (ca, cb) = (la.clone(), lb.clone());
        let lo = vec!["127.0.0.1:0".to_string()];
        let quiet = || card(false, Arc::new(AtomicUsize::new(0)));
        let ac = Calls::new(alice.clone(), quiet(), move |s| ca.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo.clone());
        let bc = Calls::new(bob.clone(), quiet(), move |s| cb.lock().unwrap().push(serde_json::from_str(&s).unwrap())).with_bind(lo);
        let (seen_a, seen_b): (Arc<Mutex<Vec<f32>>>, Arc<Mutex<Vec<f32>>>) = Default::default();
        let (sa, sb) = (seen_a.clone(), seen_b.clone());
        ac.set_video_sink(Arc::new(move |f: Frame| sa.lock().unwrap().push(luma(&f))));
        bc.set_video_sink(Arc::new(move |f: Frame| sb.lock().unwrap().push(luma(&f))));

        ac.place(&room, false).await.unwrap();
        pump(&alice, &bob, &la, &lb, |_, b| b.contains(&"incoming".to_string())).await;
        bc.answer().await.unwrap();
        pump(&alice, &bob, &la, &lb, |a, b| a.contains(&"connected".to_string()) && b.contains(&"connected".to_string())).await;

        let (r1, r2) = tokio::join!(ac.add_video(), bc.add_video());
        assert!(r1.is_ok() && r2.is_ok(), "{r1:?} {r2:?}");
        for _ in 0..200 {
            crate::sync_once(&alice).await.unwrap();
            crate::sync_once(&bob).await.unwrap();
            let idle = |c: &Calls| c.active.lock().unwrap().as_ref().map(|a| !a.negotiating).unwrap_or(false);
            if idle(&ac) && idle(&bc) { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!ac.active.lock().unwrap().as_ref().unwrap().negotiating && !bc.active.lock().unwrap().as_ref().unwrap().negotiating, "still negotiating");
        ac.set_camera(true).await;
        bc.set_camera(true).await;
        for n in 0..70 {
            ac.push_video(picture(n, 320, 240));
            bc.push_video(picture(n, 320, 240));
            tokio::time::sleep(Duration::from_millis(66)).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(seen_b.lock().unwrap().len() > 15, "bob saw {} pictures", seen_b.lock().unwrap().len());
        assert!(seen_a.lock().unwrap().len() > 15, "alice saw {} pictures", seen_a.lock().unwrap().len());
        bc.hangup().await;
    }
}
