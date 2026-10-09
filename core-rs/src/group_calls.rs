//! Group calls between qVector clients: a full mesh (everyone has a WebRTC connection to everyone), a few people at most.
//!
//! This is NOT Element Call (which uses a LiveKit server): Element clients do not understand it. How it works:
//! * Being in a call is a room state event `dev.qvector.call.member` (state key = our user id, `{"group_id", "active", "ts"}`),
//!   so everybody, and anybody who opens the room later, sees who is in the call.
//! * When someone becomes active, each person already in the call rings them with a normal `m.call.invite` that carries
//!   `dev.qvector.group_id` (so one-to-one call code ignores it) and `invitee`; the newcomer answers. So the person who joined
//!   never has to ring anybody, and two people never ring each other at once.
//! * Sound: one microphone feeds every connection; the sound of all connections is mixed into one stream for the speakers.
//! * Pictures: every connection has its own encoder (the cost of a mesh), so keep groups small.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use matrix_sdk::ruma::events::AnySyncStateEvent;
use matrix_sdk::ruma::serde::Raw;
use matrix_sdk::{Client, Room};
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::calls::{turn_servers, AudioFactory};
use crate::rtc_peer::{CallPeer, PeerEvent, Playback, VideoDisplay};
use crate::video::Frame;

pub const GROUP_KEY: &str = "dev.qvector.group_id";
const MEMBER_EVENT: &str = "dev.qvector.call.member";
/// An "active" entry older than this is a client that died without leaving.
const STALE_MS: u64 = 12 * 3600 * 1000;
const FRAME: usize = crate::rtc_peer::FRAME;
/// At most this many people (including us): every extra person costs every other person another connection and encoder.
pub const MAX_PEOPLE: usize = 5;
/// Per connection we keep at most 200 ms of sound for the mixer.
const MAX_BUFFERED: usize = 9600;

pub type GroupVideoSink = Arc<dyn Fn(&str, Frame) + Send + Sync>;

struct Link {
    user_id: String,
    name: String,
    peer: Arc<CallPeer>,
    party: String,
    remote_set: bool,
    early: Vec<Value>,
    connected: bool,
    remote_video: bool,
    /// Where the microphone frames for this connection go.
    mic: mpsc::Sender<Vec<i16>>,
    /// What this connection heard, waiting for the mixer.
    heard: Arc<Mutex<VecDeque<i16>>>,
}

struct Group {
    room_id: String,
    group_id: String,
    muted: bool,
    camera: bool,
    links: HashMap<String, Link>, /* by call id */
    audio: Option<Box<dyn Any + Send>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

type Presence = HashMap<String, HashMap<String, (String, u64)>>; /* room -> user -> (group id, ts) of the active ones */

#[derive(Clone)]
pub struct GroupCalls {
    client: Client,
    report: Arc<dyn Fn(&str, String) + Send + Sync>,
    audio: Arc<Mutex<AudioFactory>>,
    video_sink: Arc<Mutex<Option<GroupVideoSink>>>,
    bind: Arc<Vec<String>>,
    group: Arc<Mutex<Option<Group>>>,
    presence: Arc<Mutex<Presence>>,
    in_group: Arc<AtomicBool>,
    one_to_one_busy: Arc<Mutex<Arc<dyn Fn() -> bool + Send + Sync>>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl GroupCalls {
    /// `report(name, json)`: `group_call` (our call: `{state: "active"|"ended", room_id, group_id, muted, camera, participants: [...]}`)
    /// and `group_presence` (`{room_id, group_id, participants: [user ids]}`: who is in a call in that room, empty for none).
    pub fn new(client: Client, audio: AudioFactory, in_group: Arc<AtomicBool>, report: impl Fn(&str, String) + Send + Sync + 'static) -> GroupCalls {
        let calls = GroupCalls {
            client: client.clone(),
            report: Arc::new(report),
            audio: Arc::new(Mutex::new(audio)),
            video_sink: Default::default(),
            bind: Arc::new(vec!["0.0.0.0:0".to_string()]),
            group: Default::default(),
            presence: Default::default(),
            in_group,
            one_to_one_busy: Arc::new(Mutex::new(Arc::new(|| false))),
        };
        let handler = calls.clone();
        client.add_event_handler(move |ev: Raw<matrix_sdk::ruma::events::AnySyncTimelineEvent>, room: Room| {
            let c = handler.clone();
            async move {
                let Ok(v) = ev.deserialize_as_unchecked::<Value>() else { return };
                let ty = v["type"].as_str().unwrap_or("");
                if ty.starts_with("m.call.") && v["content"][GROUP_KEY].is_string() {
                    c.on_signal(&room, ty, &v).await;
                }
            }
        });
        let handler = calls.clone();
        client.add_event_handler(move |ev: Raw<AnySyncStateEvent>, room: Room| {
            let c = handler.clone();
            async move {
                let Ok(v) = ev.deserialize_as_unchecked::<Value>() else { return };
                if v["type"] == MEMBER_EVENT {
                    c.on_member(&room, &v).await;
                }
            }
        });
        calls
    }

    pub fn with_bind(mut self, bind: Vec<String>) -> GroupCalls {
        self.bind = Arc::new(bind);
        self
    }

    pub fn set_video_sink(&self, sink: GroupVideoSink) {
        *self.video_sink.lock().unwrap() = Some(sink);
    }

    pub fn set_one_to_one_busy(&self, f: Arc<dyn Fn() -> bool + Send + Sync>) {
        *self.one_to_one_busy.lock().unwrap() = f;
    }

    fn me(&self) -> String {
        self.client.user_id().map(|u| u.to_string()).unwrap_or_default()
    }

    fn emit(&self, state: &str, g: &Group) {
        let mut people: Vec<_> = g.links.values().map(|l| json!({"user_id": l.user_id, "name": l.name, "connected": l.connected, "remote_video": l.remote_video})).collect();
        people.sort_by(|a, b| a["user_id"].as_str().cmp(&b["user_id"].as_str()));
        (self.report)(
            "group_call",
            json!({"state": state, "room_id": g.room_id, "group_id": g.group_id, "muted": g.muted, "camera": g.camera, "participants": people}).to_string(),
        );
    }

    fn emit_presence(&self, room_id: &str) {
        let me = self.me();
        let (group_id, people): (String, Vec<String>) = {
            let p = self.presence.lock().unwrap();
            let now = now_ms();
            let mut ids = String::new();
            let mut people: Vec<String> = p
                .get(room_id)
                .map(|m| {
                    m.iter()
                        .filter(|(u, (_, ts))| **u != me && now.saturating_sub(*ts) < STALE_MS)
                        .map(|(u, (g, _))| {
                            ids = g.clone();
                            u.clone()
                        })
                        .collect()
                })
                .unwrap_or_default();
            people.sort();
            (ids, people)
        };
        (self.report)("group_presence", json!({"room_id": room_id, "group_id": group_id, "participants": people}).to_string());
    }

    /// Others who are in a call in this room: (group id, user ids).
    fn others_in(&self, room_id: &str) -> (String, Vec<String>) {
        let me = self.me();
        let p = self.presence.lock().unwrap();
        let now = now_ms();
        let mut group = String::new();
        let mut people = vec![];
        if let Some(m) = p.get(room_id) {
            for (u, (g, ts)) in m {
                if *u != me && now.saturating_sub(*ts) < STALE_MS {
                    group = g.clone();
                    people.push(u.clone());
                }
            }
        }
        (group, people)
    }

    async fn set_membership(&self, room: &Room, group_id: &str, active: bool) {
        let content = json!({"group_id": group_id, "active": active, "ts": now_ms()});
        if let Err(e) = room.send_state_event_raw(MEMBER_EVENT, &self.me(), content).await {
            eprintln!("group call: cannot update membership: {e}");
        }
    }

    async fn send(&self, room_id: &str, ty: &str, mut content: Value, group_id: &str, call_id: &str, party: &str) {
        content["call_id"] = json!(call_id);
        content["party_id"] = json!(party);
        content["version"] = json!("1");
        content[GROUP_KEY] = json!(group_id);
        let Ok(rid) = <&matrix_sdk::ruma::RoomId>::try_from(room_id) else { return };
        let Some(room) = self.client.get_room(rid) else { return };
        if let Err(e) = room.send_raw(ty, content).await {
            eprintln!("group call: cannot send {ty}: {e}");
        }
    }

    pub fn is_active(&self) -> bool {
        self.group.lock().unwrap().is_some()
    }

    /// Join the call in this room, or start one when nobody is in it.
    pub async fn join(&self, room_id: &str) -> Result<(), String> {
        if (self.one_to_one_busy.lock().unwrap().clone())() {
            return Err("You are in a call already".into());
        }
        let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).map_err(|e| e.to_string())?;
        let room = self.client.get_room(rid).ok_or("No such room")?;
        let (existing, others) = self.others_in(room_id);
        if others.len() + 1 > MAX_PEOPLE {
            return Err(format!("A group call has room for {MAX_PEOPLE} people"));
        }
        let group_id = if existing.is_empty() { format!("g{}", rand::random::<u64>()) } else { existing };
        let factory = self.audio.lock().unwrap().clone();
        let session = factory().map_err(|e| format!("No sound device: {e}"))?;
        let mut capture = session.capture;
        let speakers: Playback = session.playback;
        {
            let mut g = self.group.lock().unwrap();
            if g.is_some() {
                return Err("You are in a call already".into());
            }
            *g = Some(Group { room_id: room_id.into(), group_id: group_id.clone(), muted: false, camera: false, links: HashMap::new(), audio: Some(session.guard), tasks: vec![] });
            self.in_group.store(true, Ordering::Relaxed);
        }
        // the microphone to every connection
        let me = self.clone();
        let mic = tokio::spawn(async move {
            while let Some(frame) = capture.recv().await {
                let senders: Vec<mpsc::Sender<Vec<i16>>> = match me.group.lock().unwrap().as_ref() {
                    Some(g) => g.links.values().map(|l| l.mic.clone()).collect(),
                    None => return,
                };
                for s in senders {
                    let _ = s.try_send(frame.clone());
                }
            }
        });
        // everything heard, mixed, to the speakers
        let me = self.clone();
        let mixer = tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            loop {
                tick.tick().await;
                let buffers: Vec<Arc<Mutex<VecDeque<i16>>>> = match me.group.lock().unwrap().as_ref() {
                    Some(g) => g.links.values().map(|l| l.heard.clone()).collect(),
                    None => return,
                };
                let mut mixed = vec![0i32; FRAME];
                let mut any = false;
                for b in buffers {
                    let mut q = b.lock().unwrap();
                    for m in mixed.iter_mut() {
                        match q.pop_front() {
                            Some(s) => {
                                *m += s as i32;
                                any = true;
                            }
                            None => break,
                        }
                    }
                }
                if any {
                    let out: Vec<i16> = mixed.into_iter().map(|s| s.clamp(-32768, 32767) as i16).collect();
                    speakers(&out);
                }
            }
        });
        if let Some(g) = self.group.lock().unwrap().as_mut() {
            g.tasks = vec![mic, mixer];
            self.emit("active", g);
        }
        self.set_membership(&room, &group_id, true).await;
        Ok(())
    }

    pub async fn leave(&self) {
        let g = {
            let mut g = self.group.lock().unwrap();
            self.in_group.store(false, Ordering::Relaxed);
            g.take()
        };
        let Some(g) = g else { return };
        for t in &g.tasks {
            t.abort();
        }
        if let Ok(rid) = <&matrix_sdk::ruma::RoomId>::try_from(g.room_id.as_str()) {
            if let Some(room) = self.client.get_room(rid) {
                self.set_membership(&room, &g.group_id, false).await;
            }
        }
        for (call_id, l) in &g.links {
            self.send(&g.room_id, "m.call.hangup", json!({"reason": "user_hangup"}), &g.group_id, call_id, &l.party).await;
            l.peer.close().await;
        }
        let mut ended = Group { links: HashMap::new(), ..g };
        ended.audio = None;
        self.emit("ended", &ended);
    }

    pub fn set_muted(&self, muted: bool) {
        let mut guard = self.group.lock().unwrap();
        if let Some(g) = guard.as_mut() {
            g.muted = muted;
            for l in g.links.values() {
                l.peer.set_muted(muted);
            }
            self.emit("active", g);
        }
    }

    /// Our camera on or off for everybody in the call.
    pub async fn set_camera(&self, on: bool) {
        let (room_id, group_id, targets) = {
            let mut guard = self.group.lock().unwrap();
            let Some(g) = guard.as_mut().filter(|g| g.camera != on) else { return };
            g.camera = on;
            for l in g.links.values() {
                l.peer.set_camera(on);
            }
            self.emit("active", g);
            (g.room_id.clone(), g.group_id.clone(), g.links.iter().map(|(c, l)| (c.clone(), l.party.clone())).collect::<Vec<_>>())
        };
        let meta = json!({"vector-call": {"purpose": "m.usermedia", "audio_muted": false, "video_muted": !on}});
        for (call_id, party) in targets {
            self.send(&room_id, "m.call.sdp_stream_metadata_changed", json!({"org.matrix.msc3077.sdp_stream_metadata": meta, "sdp_stream_metadata": meta}), &group_id, &call_id, &party).await;
        }
    }

    pub fn push_video(&self, f: Frame) {
        let peers: Vec<Arc<CallPeer>> = match self.group.lock().unwrap().as_ref() {
            Some(g) if g.camera => g.links.values().map(|l| l.peer.clone()).collect(),
            _ => return,
        };
        for p in peers {
            p.push_video(f.clone());
        }
    }

    /// A new connection to `user_id`: the peer, the microphone feed and the buffer of what it hears.
    async fn new_link(&self, user_id: &str, name: String, call_id: &str) -> Result<(Link, Arc<CallPeer>), String> {
        let heard: Arc<Mutex<VecDeque<i16>>> = Default::default();
        let buf = heard.clone();
        let playback: Playback = Arc::new(move |pcm: &[i16]| {
            let mut q = buf.lock().unwrap();
            q.extend(pcm.iter().copied());
            let over = q.len().saturating_sub(MAX_BUFFERED);
            if over > 0 {
                q.drain(..over);
            }
        });
        let (mic, rx) = mpsc::channel::<Vec<i16>>(8);
        let sink = self.video_sink.lock().unwrap().clone();
        let who = user_id.to_string();
        let display: VideoDisplay = Arc::new(move |f| {
            if let Some(s) = &sink {
                s(&who, f);
            }
        });
        let peer = Arc::new(CallPeer::new(turn_servers(&self.client).await, (*self.bind).clone(), rx, playback, Some(display)).await?);
        let (muted, camera) = self.group.lock().unwrap().as_ref().map(|g| (g.muted, g.camera)).unwrap_or((false, false));
        peer.set_muted(muted);
        peer.set_camera(camera);
        let _ = call_id;
        Ok((
            Link { user_id: user_id.into(), name, peer: peer.clone(), party: format!("p{}", rand::random::<u32>()), remote_set: false, early: vec![], connected: false, remote_video: true, mic, heard },
            peer,
        ))
    }

    async fn display_name(&self, room: &Room, user_id: &str) -> String {
        match <&matrix_sdk::ruma::UserId>::try_from(user_id) {
            Ok(u) => match room.get_member_no_sync(u).await {
                Ok(Some(m)) => m.display_name().unwrap_or(user_id).to_string(),
                _ => user_id.to_string(),
            },
            Err(_) => user_id.to_string(),
        }
    }

    /// Follow one connection: connected, or lost.
    fn watch(&self, peer: Arc<CallPeer>, call_id: String) {
        let Some(mut events) = peer.events.lock().unwrap().take() else { return };
        let me = self.clone();
        tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                match ev {
                    PeerEvent::Connected => {
                        let mut guard = me.group.lock().unwrap();
                        if let Some(g) = guard.as_mut() {
                            if let Some(l) = g.links.get_mut(&call_id) {
                                l.connected = true;
                            }
                            me.emit("active", g);
                        }
                    }
                    PeerEvent::Failed | PeerEvent::Closed => {
                        me.drop_link(&call_id, false).await;
                        return;
                    }
                }
            }
        });
    }

    async fn drop_link(&self, call_id: &str, notify: bool) {
        let (link, room_id, group_id) = {
            let mut guard = self.group.lock().unwrap();
            let Some(g) = guard.as_mut() else { return };
            let Some(l) = g.links.remove(call_id) else { return };
            self.emit("active", g);
            (l, g.room_id.clone(), g.group_id.clone())
        };
        if notify {
            self.send(&room_id, "m.call.hangup", json!({"reason": "user_hangup"}), &group_id, call_id, &link.party).await;
        }
        link.peer.close().await;
    }

    /// Someone's entry in the call list changed.
    async fn on_member(&self, room: &Room, ev: &Value) {
        let user = ev["state_key"].as_str().unwrap_or("").to_string();
        let c = &ev["content"];
        let active = c["active"].as_bool().unwrap_or(false);
        let group_id = c["group_id"].as_str().unwrap_or("").to_string();
        let ts = c["ts"].as_u64().unwrap_or(0);
        let room_id = room.room_id().to_string();
        if user.is_empty() || group_id.is_empty() {
            return;
        }
        {
            let mut p = self.presence.lock().unwrap();
            let m = p.entry(room_id.clone()).or_default();
            if active {
                m.insert(user.clone(), (group_id.clone(), ts));
            } else {
                m.remove(&user);
            }
        }
        self.emit_presence(&room_id);
        if user == self.me() {
            return;
        }
        let ours = self.group.lock().unwrap().as_ref().filter(|g| g.room_id == room_id && g.group_id == group_id).is_some();
        if !ours {
            return;
        }
        if !active {
            let ids: Vec<String> = self.group.lock().unwrap().as_ref().map(|g| g.links.iter().filter(|(_, l)| l.user_id == user).map(|(c, _)| c.clone()).collect()).unwrap_or_default();
            for id in ids {
                self.drop_link(&id, false).await;
            }
            return;
        }
        // Somebody joined: everybody who was here rings them. A stale replay of an old entry does not.
        if now_ms().saturating_sub(ts) > 60_000 {
            return;
        }
        let known = self.group.lock().unwrap().as_ref().map(|g| g.links.values().any(|l| l.user_id == user)).unwrap_or(true);
        if known {
            return;
        }
        if self.group.lock().unwrap().as_ref().map(|g| g.links.len() + 1 >= MAX_PEOPLE).unwrap_or(true) {
            return;
        }
        let call_id = format!("c{}", rand::random::<u64>());
        let name = self.display_name(room, &user).await;
        let Ok((link, peer)) = self.new_link(&user, name, &call_id).await else { return };
        let party = link.party.clone();
        {
            let mut guard = self.group.lock().unwrap();
            let Some(g) = guard.as_mut() else { return };
            g.links.insert(call_id.clone(), link);
            self.emit("active", g);
        }
        let sdp = match peer.offer().await {
            Ok(s) => s,
            Err(_) => {
                self.drop_link(&call_id, false).await;
                return;
            }
        };
        self.send(&room_id, "m.call.invite", json!({"lifetime": 60000, "offer": {"type": "offer", "sdp": sdp}, "invitee": user}), &group_id, &call_id, &party).await;
        self.watch(peer, call_id);
    }

    /// `m.call.*` events of a group call (they carry the group id).
    async fn on_signal(&self, room: &Room, ty: &str, ev: &Value) {
        let content = &ev["content"];
        let sender = ev["sender"].as_str().unwrap_or("").to_string();
        if sender == self.me() {
            return;
        }
        let call_id = content["call_id"].as_str().unwrap_or("").to_string();
        let group_id = content[GROUP_KEY].as_str().unwrap_or("").to_string();
        let room_id = room.room_id().to_string();
        let ours = self.group.lock().unwrap().as_ref().filter(|g| g.room_id == room_id && g.group_id == group_id).is_some();
        if call_id.is_empty() || !ours {
            return;
        }
        match ty {
            "m.call.invite" => {
                if content["invitee"].as_str() != Some(&self.me()) {
                    return;
                }
                let age = now_ms().saturating_sub(ev["origin_server_ts"].as_u64().unwrap_or(now_ms()));
                if age > 90_000 {
                    return;
                }
                let sdp = content["offer"]["sdp"].as_str().unwrap_or("").to_string();
                let known = self.group.lock().unwrap().as_ref().map(|g| g.links.contains_key(&call_id) || g.links.len() + 1 >= MAX_PEOPLE).unwrap_or(true);
                if sdp.is_empty() || known {
                    return;
                }
                let name = self.display_name(room, &sender).await;
                let Ok((mut link, peer)) = self.new_link(&sender, name, &call_id).await else { return };
                let party = link.party.clone();
                let answer = match peer.answer(&sdp).await {
                    Ok(a) => a,
                    Err(_) => {
                        peer.close().await;
                        return;
                    }
                };
                link.remote_set = true;
                {
                    let mut guard = self.group.lock().unwrap();
                    let Some(g) = guard.as_mut() else { return };
                    // an older connection to the same person (they rejoined) is replaced
                    g.links.retain(|_, l| l.user_id != sender);
                    g.links.insert(call_id.clone(), link);
                    self.emit("active", g);
                }
                self.send(&room_id, "m.call.answer", json!({"answer": {"type": "answer", "sdp": answer}}), &group_id, &call_id, &party).await;
                self.watch(peer, call_id);
            }
            "m.call.answer" => {
                let peer = {
                    let guard = self.group.lock().unwrap();
                    guard.as_ref().and_then(|g| g.links.get(&call_id)).filter(|l| !l.remote_set).map(|l| l.peer.clone())
                };
                let Some(peer) = peer else { return };
                if peer.accept_answer(content["answer"]["sdp"].as_str().unwrap_or("")).await.is_err() {
                    self.drop_link(&call_id, true).await;
                    return;
                }
                let early = {
                    let mut guard = self.group.lock().unwrap();
                    let Some(l) = guard.as_mut().and_then(|g| g.links.get_mut(&call_id)) else { return };
                    l.remote_set = true;
                    std::mem::take(&mut l.early)
                };
                for c in early {
                    add_candidates(&peer, &c).await;
                }
            }
            "m.call.candidates" => {
                let peer = {
                    let mut guard = self.group.lock().unwrap();
                    let Some(l) = guard.as_mut().and_then(|g| g.links.get_mut(&call_id)) else { return };
                    if !l.remote_set {
                        l.early.push(content.clone());
                        return;
                    }
                    l.peer.clone()
                };
                add_candidates(&peer, content).await;
            }
            "m.call.sdp_stream_metadata_changed" => {
                let shows = ["org.matrix.msc3077.sdp_stream_metadata", "sdp_stream_metadata"]
                    .iter()
                    .filter_map(|k| content[*k].as_object())
                    .flat_map(|m| m.values())
                    .find_map(|s| s["video_muted"].as_bool().map(|m| !m));
                let mut guard = self.group.lock().unwrap();
                if let Some(g) = guard.as_mut() {
                    if let (Some(l), Some(shows)) = (g.links.get_mut(&call_id), shows) {
                        l.remote_video = shows;
                    }
                    self.emit("active", g);
                }
            }
            "m.call.hangup" | "m.call.reject" => self.drop_link(&call_id, false).await,
            _ => {}
        }
    }

    /// Who is in a call in this room right now (for the room's "Join" button when the app starts).
    pub async fn refresh_presence(&self, room: &Room) {
        let Ok(events) = room.get_state_events(matrix_sdk::ruma::events::StateEventType::from(MEMBER_EVENT)).await else { return };
        for e in events {
            use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState as Raw2;
            let v = match e {
                Raw2::Sync(r) => r.deserialize_as_unchecked::<Value>().ok(),
                Raw2::Stripped(r) => r.deserialize_as_unchecked::<Value>().ok(),
            };
            if let Some(v) = v {
                self.on_member(room, &v).await;
            }
        }
    }
}

async fn add_candidates(peer: &CallPeer, content: &Value) {
    for c in content["candidates"].as_array().cloned().unwrap_or_default() {
        let cand = c["candidate"].as_str().unwrap_or("");
        if !cand.is_empty() {
            peer.add_candidate(cand, c["sdpMid"].as_str().map(String::from), c["sdpMLineIndex"].as_u64().map(|n| n as u16)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calls::AudioSession;
    use crate::testkit::FakeHs;
    use std::sync::atomic::AtomicUsize;

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

    struct Person {
        client: Client,
        calls: GroupCalls,
        log: Arc<Mutex<Vec<(String, Value)>>>,
        heard: Arc<AtomicUsize>,
        pictures: Arc<Mutex<Vec<String>>>,
    }

    async fn person(hs: &FakeHs, name: &str, speak: bool) -> Person {
        let dir = tempfile::tempdir().unwrap();
        let client = crate::session::sign_in(dir.path(), &hs.uri(), name, "x", None, name).await.unwrap();
        std::mem::forget(dir);
        let log: Arc<Mutex<Vec<(String, Value)>>> = Default::default();
        let l = log.clone();
        let heard = Arc::new(AtomicUsize::new(0));
        let calls = GroupCalls::new(client.clone(), card(speak, heard.clone()), Arc::new(AtomicBool::new(false)), move |n, j| l.lock().unwrap().push((n.to_string(), serde_json::from_str(&j).unwrap())))
            .with_bind(vec!["127.0.0.1:0".to_string()]);
        let pictures: Arc<Mutex<Vec<String>>> = Default::default();
        let p = pictures.clone();
        calls.set_video_sink(Arc::new(move |who: &str, _f: Frame| p.lock().unwrap().push(who.to_string())));
        Person { client, calls, log, heard, pictures }
    }

    fn connected(p: &Person) -> usize {
        p.calls.group.lock().unwrap().as_ref().map(|g| g.links.values().filter(|l| l.connected).count()).unwrap_or(0)
    }

    async fn sync_until(people: &[&Person], what: &str, until: impl Fn() -> bool) {
        for _ in 0..600 {
            for p in people {
                crate::sync_once(&p.client).await.unwrap();
            }
            if until() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn three_people_join_a_group_call_hear_each_other_and_leave() {
        let hs = FakeHs::start().await;
        hs.add_member("carol");
        let (alice, bob, carol) = (person(&hs, "alice", true).await, person(&hs, "bob", true).await, person(&hs, "carol", false).await);
        for p in [&alice, &bob, &carol] {
            crate::sync_once(&p.client).await.unwrap();
        }
        let room = alice.client.joined_rooms().first().unwrap().room_id().to_string();

        alice.calls.join(&room).await.unwrap();
        sync_until(&[&alice, &bob, &carol], "bob sees alice in the call", || {
            bob.log.lock().unwrap().iter().any(|(n, v)| n == "group_presence" && v["participants"].as_array().map(|a| a.len() == 1).unwrap_or(false))
        })
        .await;
        bob.calls.join(&room).await.unwrap();
        sync_until(&[&alice, &bob, &carol], "alice and bob connected", || connected(&alice) == 1 && connected(&bob) == 1).await;
        carol.calls.join(&room).await.unwrap();
        sync_until(&[&alice, &bob, &carol], "everybody connected to everybody", || connected(&alice) == 2 && connected(&bob) == 2 && connected(&carol) == 2).await;

        // sound: carol hears alice and bob mixed; alice hears bob
        for _ in 0..80 {
            if carol.heard.load(Ordering::Relaxed) > 20 && alice.heard.load(Ordering::Relaxed) > 20 { break; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(carol.heard.load(Ordering::Relaxed) > 20, "carol heard {}", carol.heard.load(Ordering::Relaxed));
        assert!(alice.heard.load(Ordering::Relaxed) > 20, "alice heard {}", alice.heard.load(Ordering::Relaxed));

        // pictures from alice reach both
        alice.calls.set_camera(true).await;
        for n in 0..60 {
            alice.calls.push_video(crate::video::tests::picture(n, 320, 240));
            tokio::time::sleep(Duration::from_millis(66)).await;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        for p in [&bob, &carol] {
            let seen = p.pictures.lock().unwrap().clone();
            assert!(seen.len() > 15 && seen.iter().all(|w| w == "@alice:hs"), "{} pictures: {:?}", seen.len(), seen.first());
        }

        // carol leaves: the others drop her and keep each other
        carol.calls.leave().await;
        sync_until(&[&alice, &bob, &carol], "carol gone", || {
            let n = |p: &Person| p.calls.group.lock().unwrap().as_ref().map(|g| g.links.len()).unwrap_or(99);
            n(&alice) == 1 && n(&bob) == 1
        })
        .await;
        assert_eq!(connected(&alice), 1);
        assert!(!carol.calls.is_active());
        alice.calls.leave().await;
        bob.calls.leave().await;
        assert!(carol.log.lock().unwrap().iter().any(|(n, v)| n == "group_call" && v["state"] == "ended"));
        let unhandled: Vec<_> = hs.log().into_iter().filter(|l| l.contains("UNHANDLED") && !l.contains("well-known")).collect();
        assert!(unhandled.is_empty(), "{unhandled:?}");
    }
}
