//! A tiny in-process fake homeserver (wiremock) for tests that need two real clients talking to each other:
//! login, key upload/query/claim, to-device, one encrypted room, send and sync. Not a spec-complete server.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use wiremock::{matchers::{method, path_regex}, Mock, MockServer, Request, Respond, ResponseTemplate};

pub const ROOM: &str = "!r:hs";

#[derive(Default)]
struct Dev {
    name: String, /* display name given at login */
    keys: Option<Value>,
    otks: BTreeMap<String, Value>,
    fallback: Option<(String, Value)>,
    to_device: VecDeque<Value>,
    room_pos: usize,
    state_sent: bool,
    told: std::collections::BTreeSet<String>, /* extra rooms already announced, "inv:<id>" / "join:<id>" */
}

#[derive(Default)]
struct User {
    devices: BTreeMap<String, Dev>, /* by device id */
    account_data: BTreeMap<String, Value>, /* global account data by type */
    backup: Option<(String, Value)>,       /* (version, body) of the key backup */
    backup_keys: BTreeMap<String, Value>,  /* room id -> {"sessions": {...}} */
    cross: BTreeMap<String, Value>, /* master_keys, self_signing_keys, user_signing_keys */
}

#[derive(Default)]
struct State {
    users: BTreeMap<String, User>, /* by localpart */
    events: Vec<Value>,            /* timeline of ROOM */
    counter: u64,
    syncs: u64, /* makes every next_batch distinct: the SDK drops a response whose token it has already seen */
    log: Vec<String>,
    history: Vec<Value>, /* older events of ROOM, oldest first, served by /messages */
    tags: BTreeMap<String, Vec<String>>, /* room id -> m.tag names (alice's) */
    reads: Vec<(String, String)>, /* (user local part, event id) read receipts of other people in ROOM */
    typing: Vec<String>, /* who is typing in ROOM (local parts) */
    extra_rooms: Vec<(String, String, bool, Vec<String>)>, /* (room id, name, joined, children): invites, rooms created at run time and spaces (a room with children) */
    uia_password: Option<String>, /* when set, cross-signing uploads need this password (user-interactive auth) */
    media: BTreeMap<String, Vec<u8>>, /* uploaded files by id */
}

#[derive(Clone)]
pub struct FakeHs { pub server: Arc<MockServer>, st: Arc<Mutex<State>> }

/// Tokens look like `tok_<user>_<device>`; returns "<user>_<device>" (the caller splits it).
fn local_of_token(req: &Request) -> Option<String> {
    let h = req.headers.get("authorization")?.to_str().ok()?;
    h.strip_prefix("Bearer tok_").map(|s| s.to_string())
}

fn user_of(who: &str) -> String { split_who(who).0 }

fn split_who(who: &str) -> (String, String) {
    match who.split_once('_') { Some((u, d)) => (u.to_string(), d.to_string()), None => (who.to_string(), format!("DEV_{who}")) }
}

struct Handler { st: Arc<Mutex<State>>, f: fn(&mut State, &str, &Request) -> Value }

impl Respond for Handler {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.st.lock().unwrap();
        let who = local_of_token(req).unwrap_or_default();
        st.log.push(format!("{} {} ({})", req.method, req.url.path(), who));
        let v = (self.f)(&mut st, &who, req);
        let status = if v.get("flows").is_some() { 401 } else { match v["errcode"].as_str() { Some("M_NOT_FOUND") => 404, Some(_) => 400, None => 200 } };
        ResponseTemplate::new(status).set_body_json(v)
    }
}

fn body(req: &Request) -> Value { serde_json::from_slice(&req.body).unwrap_or(Value::Null) }

fn member_event(user: &str, n: u64) -> Value {
    json!({"type": "m.room.member", "state_key": format!("@{user}:hs"), "sender": format!("@{user}:hs"), "event_id": format!("$mem_{user}"),
           "origin_server_ts": n, "content": {"membership": "join", "displayname": user, "avatar_url": format!("mxc://hs/av_{user}")}})
}

fn room_state(users: &[&str]) -> Vec<Value> {
    let mut v = vec![
        json!({"type": "m.room.create", "state_key": "", "sender": "@alice:hs", "event_id": "$create", "origin_server_ts": 1, "content": {"room_version": "10", "creator": "@alice:hs"}}),
        json!({"type": "m.room.power_levels", "state_key": "", "sender": "@alice:hs", "event_id": "$pl", "origin_server_ts": 2, "content": {"users": {"@alice:hs": 100}}}),
        json!({"type": "m.room.join_rules", "state_key": "", "sender": "@alice:hs", "event_id": "$jr", "origin_server_ts": 3, "content": {"join_rule": "invite"}}),
        json!({"type": "m.room.history_visibility", "state_key": "", "sender": "@alice:hs", "event_id": "$hv", "origin_server_ts": 4, "content": {"history_visibility": "shared"}}),
        json!({"type": "m.room.topic", "state_key": "", "sender": "@alice:hs", "event_id": "$topic", "origin_server_ts": 6, "content": {"topic": "Where alice and bob test the Rust rebuild"}}),
        json!({"type": "m.room.encryption", "state_key": "", "sender": "@alice:hs", "event_id": "$enc", "origin_server_ts": 5, "content": {"algorithm": "m.megolm.v1.aes-sha2"}}),
    ];
    for (i, u) in users.iter().enumerate() { v.push(member_event(u, 10 + i as u64)); }
    v
}

fn otk_counts(d: &Dev) -> Value { json!({"signed_curve25519": d.otks.len()}) }

struct Upload(Arc<Mutex<State>>);
impl Respond for Upload {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let mut st = self.0.lock().unwrap();
        st.counter += 1;
        let id = format!("media{}", st.counter);
        st.media.insert(id.clone(), req.body.clone());
        st.log.push(format!("{} {} (media, {} bytes)", req.method, req.url.path(), req.body.len()));
        ResponseTemplate::new(200).set_body_json(json!({"content_uri": format!("mxc://hs/{id}")}))
    }
}

struct Download(Arc<Mutex<State>>);
impl Respond for Download {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let st = self.0.lock().unwrap();
        let id = req.url.path().rsplit('/').next().unwrap_or("").to_string();
        match st.media.get(&id) {
            Some(bytes) => ResponseTemplate::new(200).insert_header("content-type", "application/octet-stream").set_body_bytes(bytes.clone()),
            None => ResponseTemplate::new(404).set_body_json(json!({"errcode": "M_NOT_FOUND", "error": "no such file"})),
        }
    }
}

impl FakeHs {
    /// A server with the encrypted room `ROOM` that `alice` and `bob` have joined.
    pub async fn start() -> FakeHs {
        let server = Arc::new(MockServer::start().await);
        let st = Arc::new(Mutex::new(State::default()));
        for u in ["alice", "bob"] { st.lock().unwrap().media.insert(format!("av_{u}"), include_bytes!("avatar_demo.png").to_vec()); }
        let hs = FakeHs { server: server.clone(), st: st.clone() };
        let mount = |m: &'static str, re: &'static str, f: fn(&mut State, &str, &Request) -> Value| {
            let st = st.clone();
            let server = server.clone();
            async move { Mock::given(method(m)).and(path_regex(re)).respond_with(Handler { st, f }).mount(&*server).await }
        };
        mount("GET", r"^/_matrix/client/versions$", |_, _, _| json!({"versions": ["v1.11"]})).await;
        mount("POST", r"^/_matrix/client/v3/login$", |st, _, req| {
            let b = body(req);
            let user = b["identifier"]["user"].as_str().unwrap_or("x").to_string();
            let dev = b["device_id"].as_str().map(String::from).unwrap_or_else(|| format!("DEV_{user}"));
            let d = st.users.entry(user.clone()).or_default().devices.entry(dev.clone()).or_default();
            if let Some(n) = b["initial_device_display_name"].as_str() { d.name = n.to_string(); }
            json!({"user_id": format!("@{user}:hs"), "access_token": format!("tok_{user}_{dev}"), "device_id": dev})
        }).await;
        mount("POST", r"^/_matrix/client/v3/keys/upload$", |st, who, req| {
            let b = body(req);
            let (user, dev) = split_who(who);
            let d = st.users.entry(user).or_default().devices.entry(dev).or_default();
            if let Some(dk) = b.get("device_keys").filter(|v| v.is_object()) { d.keys = Some(dk.clone()); }
            if let Some(o) = b.get("one_time_keys").and_then(|v| v.as_object()) { for (k, v) in o { d.otks.insert(k.clone(), v.clone()); } }
            if let Some(f) = b.get("fallback_keys").and_then(|v| v.as_object()) { if let Some((k, v)) = f.iter().next() { d.fallback = Some((k.clone(), v.clone())); } }
            json!({"one_time_key_counts": otk_counts(d)})
        }).await;
        mount("POST", r"^/_matrix/client/v3/keys/query$", |st, _, req| {
            let b = body(req);
            let mut dk = serde_json::Map::new();
            let (mut mk, mut ssk, mut usk) = (serde_json::Map::new(), serde_json::Map::new(), serde_json::Map::new());
            for uid in b["device_keys"].as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()).unwrap_or_default() {
                let local = uid.trim_start_matches('@').split(':').next().unwrap_or("").to_string();
                if let Some(u) = st.users.get(&local) {
                    let devs: serde_json::Map<String, Value> = u.devices.iter().filter_map(|(id, d)| d.keys.clone().map(|mut k| { if !d.name.is_empty() { k["unsigned"] = json!({"device_display_name": d.name}); } (id.clone(), k) })).collect();
                    if !devs.is_empty() { dk.insert(uid.clone(), Value::Object(devs)); }
                    if let Some(k) = u.cross.get("master_keys") { mk.insert(uid.clone(), k.clone()); }
                    if let Some(k) = u.cross.get("self_signing_keys") { ssk.insert(uid.clone(), k.clone()); }
                    if let Some(k) = u.cross.get("user_signing_keys") { usk.insert(uid.clone(), k.clone()); }
                }
            }
            json!({"device_keys": dk, "failures": {}, "master_keys": mk, "self_signing_keys": ssk, "user_signing_keys": usk})
        }).await;
        mount("POST", r"^/_matrix/client/v3/keys/claim$", |st, _, req| {
            let b = body(req);
            let mut out = serde_json::Map::new();
            for (uid, devs) in b["one_time_keys"].as_object().cloned().unwrap_or_default() {
                let local = uid.trim_start_matches('@').split(':').next().unwrap_or("").to_string();
                for (dev, _alg) in devs.as_object().cloned().unwrap_or_default() {
                    if let Some(u) = st.users.get_mut(&local).and_then(|u| u.devices.get_mut(&dev)) {
                        let key = u.otks.keys().next().cloned();
                        let claimed = match key { Some(k) => u.otks.remove(&k).map(|v| json!({k: v})), None => u.fallback.clone().map(|(k, v)| json!({k: v})) };
                        if let Some(c) = claimed { out.entry(uid.clone()).or_insert_with(|| json!({})).as_object_mut().unwrap().insert(dev, c); }
                    }
                }
            }
            json!({"one_time_keys": out, "failures": {}})
        }).await;
        mount("PUT", r"^/_matrix/client/v3/sendToDevice/", |st, who, req| {
            let ty = req.url.path().split('/').rev().nth(1).unwrap_or("").to_string();
            let b = body(req);
            let (sender, _) = split_who(who);
            for (uid, devs) in b["messages"].as_object().cloned().unwrap_or_default() {
                let local = uid.trim_start_matches('@').split(':').next().unwrap_or("").to_string();
                for (dev, content) in devs.as_object().cloned().unwrap_or_default() {
                    if let Some(u) = st.users.get_mut(&local) {
                        let targets: Vec<String> = if dev == "*" { u.devices.keys().cloned().collect() } else { vec![dev.clone()] };
                        for t in targets {
                            if let Some(d) = u.devices.get_mut(&t) {
                                d.to_device.push_back(json!({"type": ty, "sender": format!("@{sender}:hs"), "content": content}));
                            }
                        }
                    }
                }
            }
            json!({})
        }).await;
        mount("PUT", r"^/_matrix/client/v3/rooms/[^/]+/send/", |st, who, req| {
            let ty = req.url.path().split('/').rev().nth(1).unwrap_or("").to_string();
            st.counter += 1;
            let id = format!("$ev{}", st.counter);
            let (sender, _) = split_who(who);
            st.events.push(json!({"type": ty, "sender": format!("@{sender}:hs"), "event_id": id, "origin_server_ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1000) + st.counter, "content": body(req)}));
            json!({"event_id": id})
        }).await;
        Mock::given(method("POST")).and(path_regex(r"^/_matrix/media/v3/upload$")).respond_with(Upload(st.clone())).mount(&*server).await;
        Mock::given(method("GET")).and(path_regex(r"^/_matrix/(client/v1/media|media/v3)/download/")).respond_with(Download(st.clone())).mount(&*server).await;
        mount("GET", r"^/_matrix/(client/v1/media|media/v3)/config$", |_, _, _| json!({"m.upload.size": 50_000_000})).await;
        mount("PUT", r"^/_matrix/client/v3/rooms/[^/]+/redact/", |st, who, req| {
            let parts: Vec<&str> = req.url.path().split('/').collect(); /* .../redact/<event id>/<txn> */
            let target = parts.iter().rev().nth(1).map(|t| t.replace("%24", "$")).unwrap_or_default();
            st.counter += 1;
            let id = format!("$red{}", st.counter);
            let (sender, _) = split_who(who);
            st.events.push(json!({"type": "m.room.redaction", "sender": format!("@{sender}:hs"), "event_id": id, "redacts": target,
                                  "origin_server_ts": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1000) + st.counter,
                                  "content": body(req)}));
            json!({"event_id": id})
        }).await;
        mount("POST", r"^/_matrix/client/v3/keys/device_signing/upload$", |st, who, req| {
            let b = body(req);
            if let Some(pw) = st.uia_password.clone() {
                let ok = b["auth"]["type"] == "m.login.password" && b["auth"]["password"] == pw.as_str() && b["auth"]["session"] == "uia1";
                if !ok {
                    st.log.push("uia: password demanded".into());
                    return json!({"flows": [{"stages": ["m.login.password"]}], "params": {}, "session": "uia1"});
                }
            }
            let (user, _) = split_who(who);
            let u = st.users.entry(user).or_default();
            for k in ["master_key", "self_signing_key", "user_signing_key"] { if let Some(v) = b.get(k) { u.cross.insert(format!("{k}s"), v.clone()); } }
            json!({})
        }).await;
        mount("POST", r"^/_matrix/client/v3/keys/signatures/upload$", |st, _, req| {
            let b = body(req);
            for (uid, items) in b.as_object().cloned().unwrap_or_default() {
                let local = uid.trim_start_matches('@').split(':').next().unwrap_or("").to_string();
                for (target, obj) in items.as_object().cloned().unwrap_or_default() {
                    let sigs = obj["signatures"].as_object().cloned().unwrap_or_default();
                    let Some(u) = st.users.get_mut(&local) else { continue };
                    /* a device (merge the new signatures into its keys) or one of the cross-signing keys */
                    if let Some(keys) = u.devices.get_mut(&target).and_then(|d| d.keys.as_mut()) {
                        let all = keys["signatures"].as_object().cloned().unwrap_or_default();
                        let mut merged = all;
                        for (signer, per) in sigs { let e = merged.entry(signer).or_insert_with(|| json!({})); if let (Some(o), Some(n)) = (e.as_object_mut(), per.as_object()) { for (k, v) in n { o.insert(k.clone(), v.clone()); } } }
                        keys["signatures"] = Value::Object(merged);
                    } else {
                        for kind in ["master_keys", "self_signing_keys", "user_signing_keys"] {
                            if let Some(k) = u.cross.get_mut(kind) {
                                let has = k["keys"].as_object().map(|m| m.keys().any(|id| id.ends_with(&target))).unwrap_or(false);
                                if has {
                                    let mut merged = k["signatures"].as_object().cloned().unwrap_or_default();
                                    for (signer, per) in sigs.clone() { let e = merged.entry(signer).or_insert_with(|| json!({})); if let (Some(o), Some(n)) = (e.as_object_mut(), per.as_object()) { for (kk, v) in n { o.insert(kk.clone(), v.clone()); } } }
                                    k["signatures"] = Value::Object(merged);
                                }
                            }
                        }
                    }
                }
            }
            json!({"failures": {}})
        }).await;
        mount("GET", r"^/_matrix/client/v3/sync$", |st, who, _| {
            st.syncs += 1;
            let n = st.events.len();
            let state_events = room_state(&["alice", "bob"]);
            let (user, dev) = split_who(who);
            let u = st.users.entry(user).or_default().devices.entry(dev).or_default();
            let td: Vec<Value> = u.to_device.drain(..).collect();
            if !td.is_empty() { st.log.push(format!("sync delivers {} to-device events ({}) to {who}", td.len(), td.iter().filter_map(|e| e["type"].as_str()).collect::<Vec<_>>().join(","))); }
            let from = u.room_pos;
            u.room_pos = n;
            let first = !u.state_sent;
            u.state_sent = true;
            let counts = otk_counts(u);
            let events = st.events[from..].to_vec();
            let mut invite = serde_json::Map::new();
            let mut joined = serde_json::Map::new();
            let rooms = st.extra_rooms.clone();
            let u = st.users.entry(user_of(who)).or_default().devices.entry(split_who(who).1).or_default();
            for (id, name, is_joined, children) in rooms {
                let key = format!("{}:{id}", if is_joined { "join" } else { "inv" });
                if !u.told.insert(key) { continue; }
                let name_ev = json!({"type": "m.room.name", "state_key": "", "sender": "@bob:hs", "event_id": format!("$n{id}"), "origin_server_ts": 20, "content": {"name": name}});
                if is_joined {
                    let me = json!({"type": "m.room.member", "state_key": "@alice:hs", "sender": "@alice:hs", "event_id": format!("$m{id}"), "origin_server_ts": 21, "content": {"membership": "join", "displayname": "alice"}});
                    let mut state = vec![name_ev, me];
                    if !children.is_empty() {
                        state.push(json!({"type": "m.room.create", "state_key": "", "sender": "@alice:hs", "event_id": format!("$c{id}"), "origin_server_ts": 19, "content": {"room_version": "10", "type": "m.space", "creator": "@alice:hs"}}));
                        for (n, c) in children.iter().enumerate() {
                            state.push(json!({"type": "m.space.child", "state_key": c, "sender": "@alice:hs", "event_id": format!("$sc{n}{id}"), "origin_server_ts": 22, "content": {"via": ["hs"]}}));
                        }
                    }
                    joined.insert(id, json!({"state": {"events": state}, "timeline": {"events": [], "limited": false}}));
                } else {
                    let me = json!({"type": "m.room.member", "state_key": "@alice:hs", "sender": "@bob:hs", "origin_server_ts": 21, "content": {"membership": "invite"}});
                    invite.insert(id, json!({"invite_state": {"events": [name_ev, me]}}));
                }
            }
            let mut receipts = serde_json::Map::new();
            for (user, ev) in &st.reads { receipts.entry(ev.clone()).or_insert_with(|| json!({"m.read": {}}))["m.read"][format!("@{user}:hs")] = json!({"ts": 1_700_000_000_000u64}); }
            let typing: Vec<String> = st.typing.iter().map(|u| format!("@{u}:hs")).collect();
            for (room, tags) in &st.tags {
                let tag_map: serde_json::Map<String, Value> = tags.iter().map(|t| (t.clone(), json!({}))).collect();
                let acc = json!({"events": [{"type": "m.tag", "content": {"tags": tag_map}}]});
                match joined.get_mut(room) { Some(j) => { j["account_data"] = acc; } None => { if room != ROOM { joined.insert(room.clone(), json!({"account_data": acc})); } } }
            }
            joined.insert(ROOM.to_string(), json!({
                "account_data": json!({"events": [{"type": "m.tag", "content": {"tags": st.tags.get(ROOM).map(|t| t.iter().map(|x| (x.clone(), json!({}))).collect::<serde_json::Map<String, Value>>()).unwrap_or_default()}}]}),
                "ephemeral": {"events": [{"type": "m.typing", "content": {"user_ids": typing}}, {"type": "m.receipt", "content": receipts}]},
                "state": {"events": if first { state_events } else { vec![] }},
                "timeline": {"events": events, "limited": false, "prev_batch": "pb0"}
            }));
            let me_local = user_of(who);
            let other = if me_local == "alice" { "bob" } else { "alice" }; /* ROOM doubles as the direct chat between the two */
            let mut account: Vec<Value> = st.users.get(&me_local).map(|u| u.account_data.iter().map(|(t, c)| json!({"type": t, "content": c})).collect()).unwrap_or_default();
            account.push(json!({"type": "m.direct", "content": {format!("@{other}:hs"): [ROOM]}}));
            json!({
                "next_batch": format!("s{n}_{}_{}", st.counter, st.syncs),
                "account_data": {"events": account},
                "to_device": {"events": td},
                "device_one_time_keys_count": counts,
                "device_unused_fallback_key_types": [],
                "rooms": {"join": joined, "invite": invite}
            })
        }).await;
        mount("POST", r"^/_matrix/client/v3/rooms/[^/]+/receipt/[^/]+/[^/]+$", |st, _, req| {
            let parts: Vec<&str> = req.url.path().split('/').collect();
            st.log.push(format!("receipt {} {}", parts[parts.len() - 2], parts[parts.len() - 1].replace("%24", "$")));
            json!({})
        }).await;
        mount("POST", r"^/_matrix/client/v3/rooms/[^/]+/read_markers$", |st, _, req| {
            st.log.push(format!("read_markers {}", body(req)));
            json!({})
        }).await;
        mount("PUT", r"^/_matrix/client/v3/rooms/[^/]+/typing/[^/]+$", |st, _, req| {
            st.log.push(format!("typing {}", body(req)["typing"]));
            json!({})
        }).await;
        mount("POST", r"^/_matrix/client/v3/rooms/[^/]+/invite$", |st, _, req| {
            let target = body(req)["user_id"].as_str().unwrap_or("").to_string();
            st.log.push(format!("invite {target}"));
            st.counter += 1;
            let ev = json!({"type": "m.room.member", "state_key": target, "sender": "@alice:hs", "event_id": format!("$inv{}", st.counter), "origin_server_ts": 50 + st.counter, "content": {"membership": "invite"}});
            st.events.push(ev);
            json!({})
        }).await;
        mount("POST", r"^/_matrix/client/v3/rooms/[^/]+/(kick|ban|unban)$", |st, _, req| {
            let what = req.url.path().rsplit('/').next().unwrap_or("").to_string();
            let b = body(req);
            let target = b["user_id"].as_str().unwrap_or("").to_string();
            st.log.push(format!("moderation {what} {target} {}", b["reason"].as_str().unwrap_or("")));
            let membership = match what.as_str() { "ban" => "ban", _ => "leave" };
            st.counter += 1;
            let local = target.trim_start_matches('@').split(':').next().unwrap_or("").to_string();
            let ev = json!({"type": "m.room.member", "state_key": target, "sender": "@alice:hs", "event_id": format!("$mod{}", st.counter), "origin_server_ts": 50 + st.counter, "content": {"membership": membership, "displayname": local}});
            st.events.push(ev);
            json!({})
        }).await;
        mount("GET", r"^/_matrix/client/v3/rooms/[^/]+/state/[^/]+(/[^/]*)?$", |st, _, req| {
            let parts: Vec<&str> = req.url.path().split('/').collect();
            let at = parts.iter().position(|p| *p == "state").unwrap_or(0);
            let ty = parts.get(at + 1).copied().unwrap_or("");
            let key = parts.get(at + 2).copied().unwrap_or("");
            let found = st.events.iter().rev().find(|e| e["type"] == ty && e["state_key"] == key).map(|e| e["content"].clone())
                .or_else(|| room_state(&["alice", "bob"]).into_iter().find(|e| e["type"] == ty && e["state_key"] == key).map(|e| e["content"].clone()));
            found.unwrap_or_else(|| json!({"errcode": "M_NOT_FOUND", "error": "no such state"}))
        }).await;
        mount("PUT", r"^/_matrix/client/v3/rooms/[^/]+/state/[^/]+(/[^/]*)?$", |st, who, req| {
            let parts: Vec<&str> = req.url.path().split('/').collect();
            let at = parts.iter().position(|p| *p == "state").unwrap_or(0);
            let ty = parts.get(at + 1).copied().unwrap_or("").to_string();
            let key = parts.get(at + 2).copied().unwrap_or("").to_string();
            st.counter += 1;
            let id = format!("$state{}", st.counter);
            st.log.push(format!("state {ty} {}", body(req)));
            let (sender, _) = split_who(who);
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1000) + st.counter;
            st.events.push(json!({"type": ty, "state_key": key, "sender": format!("@{sender}:hs"), "event_id": id, "origin_server_ts": ts, "content": body(req)})); /* the room's state follows */
            json!({"event_id": id})
        }).await;
        mount("GET", r"^/_matrix/client/v3/rooms/[^/]+/event/[^/]+$", |st, _, req| {
            let id = req.url.path().rsplit('/').next().unwrap_or("").replace("%24", "$");
            match st.events.iter().find(|e| e["event_id"] == id.as_str()) {
                Some(e) => { let mut e = e.clone(); e["room_id"] = json!(ROOM); e }
                None => json!({"errcode": "M_NOT_FOUND", "error": "no such event"}),
            }
        }).await;
        mount("GET", r"^/_matrix/client/v1/media/preview_url$", |_, _, req| {
            let url = req.url.query_pairs().find(|(k, _)| k == "url").map(|(_, v)| v.to_string()).unwrap_or_default();
            if url.contains("example.org") { json!({"og:title": "Example page", "og:description": "A page the fake server can preview", "og:site_name": "Example"}) } else { json!({}) }
        }).await;
        mount("PUT", r"^/_matrix/client/v3/user/[^/]+/rooms/[^/]+/tags/[^/]+$", |st, _, req| {
            let parts: Vec<&str> = req.url.path().split('/').collect();
            let (room, tag) = (parts[parts.len() - 3].replace("%21", "!").replace("%3A", ":").replace("%3a", ":"), parts[parts.len() - 1].replace("%2E", ".").to_string());
            let e = st.tags.entry(room.clone()).or_default();
            if !e.contains(&tag) { e.push(tag.clone()); }
            st.log.push(format!("tag add {room} {tag}"));
            json!({})
        }).await;
        {
            let st2 = st.clone();
            Mock::given(method("DELETE")).and(path_regex(r"^/_matrix/client/v3/user/[^/]+/rooms/[^/]+/tags/[^/]+$")).respond_with(Handler { st: st2, f: |st, _, req| {
                let parts: Vec<&str> = req.url.path().split('/').collect();
                let (room, tag) = (parts[parts.len() - 3].replace("%21", "!").replace("%3A", ":").replace("%3a", ":"), parts[parts.len() - 1].to_string());
                if let Some(e) = st.tags.get_mut(&room) { e.retain(|t| *t != tag); }
                st.log.push(format!("tag remove {room} {tag}"));
                json!({})
            } }).mount(&*server).await;
        }
        mount("POST", r"^/_matrix/client/v3/user_directory/search$", |_, _, req| {
            let term = body(req)["search_term"].as_str().unwrap_or("").to_lowercase();
            let all = [("@alice:hs", "alice"), ("@bob:hs", "bob"), ("@carol:hs", "carol")];
            let results: Vec<Value> = all.iter().filter(|(id, n)| id.contains(&term) || n.contains(&term)).map(|(id, n)| json!({"user_id": id, "display_name": n})).collect();
            json!({"results": results, "limited": false})
        }).await;
        mount("POST", r"^/_matrix/client/v3/publicRooms$", |_, _, req| {
            let term = body(req)["filter"]["generic_search_term"].as_str().unwrap_or("").to_lowercase();
            let all = [("!pub1:hs", "Rust talk", "#rust:hs", "All things Rust", 42), ("!pub2:hs", "Cooking", "#cook:hs", "Recipes", 7)];
            let chunk: Vec<Value> = all.iter().filter(|(_, n, a, ..)| n.to_lowercase().contains(&term) || a.contains(&term)).map(|(id, n, a, t, m)| json!({"room_id": id, "name": n, "canonical_alias": a, "topic": t, "num_joined_members": m, "world_readable": false, "guest_can_join": false})).collect();
            json!({"chunk": chunk, "total_room_count_estimate": chunk.len()})
        }).await;
        mount("POST", r"^/_matrix/client/v3/createRoom$", |st, _, req| {
            let b = body(req);
            st.counter += 1;
            let id = format!("!new{}:hs", st.counter);
            let name = b["name"].as_str().map(String::from).or_else(|| b["invite"][0].as_str().map(|u| u.trim_start_matches('@').split(':').next().unwrap_or("").to_string())).unwrap_or_default();
            st.log.push(format!("createRoom name={name} direct={} encrypted={}", b["is_direct"], b["initial_state"].as_array().map(|a| a.iter().any(|e| e["type"] == "m.room.encryption")).unwrap_or(false)));
            st.extra_rooms.push((id.clone(), name, true, vec![]));
            json!({"room_id": id})
        }).await;
        mount("POST", r"^/_matrix/client/v3/(join/[^/]+|rooms/[^/]+/join)$", |st, _, req| {
            let parts: Vec<&str> = req.url.path().split('/').collect();
            let raw = if parts[parts.len() - 1] == "join" { parts[parts.len() - 2] } else { parts[parts.len() - 1] };
            let id = raw.replace("%21", "!").replace("%3A", ":").replace("%3a", ":");
            for r in st.extra_rooms.iter_mut() { if r.0 == id { r.2 = true; } }
            st.log.push(format!("join {id}"));
            json!({"room_id": id})
        }).await;
        mount("POST", r"^/_matrix/client/v3/rooms/[^/]+/(leave|forget)$", |st, _, req| {
            let parts: Vec<&str> = req.url.path().split('/').collect();
            let id = parts[parts.len() - 2].replace("%21", "!").replace("%3A", ":").replace("%3a", ":");
            st.extra_rooms.retain(|r| r.0 != id);
            st.log.push(format!("leave {id}"));
            json!({})
        }).await;
        mount("PUT", r"^/_matrix/client/v3/user/[^/]+/account_data/", |st, who, req| {
            let ty = req.url.path().rsplit('/').next().unwrap_or("").to_string();
            st.users.entry(user_of(who)).or_default().account_data.insert(ty, body(req));
            json!({})
        }).await;
        mount("GET", r"^/_matrix/client/v3/user/[^/]+/account_data/", |st, who, req| {
            let ty = req.url.path().rsplit('/').next().unwrap_or("").to_string();
            st.users.get(&user_of(who)).and_then(|u| u.account_data.get(&ty).cloned()).unwrap_or_else(|| json!({"errcode": "M_NOT_FOUND", "error": "not set"}))
        }).await;
        mount("POST", r"^/_matrix/client/v3/room_keys/version$", |st, who, req| {
            st.counter += 1;
            let v = st.counter.to_string();
            let u = st.users.entry(user_of(who)).or_default();
            u.backup = Some((v.clone(), body(req)));
            u.backup_keys.clear();
            json!({"version": v})
        }).await;
        mount("GET", r"^/_matrix/client/v3/room_keys/version", |st, who, _| {
            match st.users.get(&user_of(who)).and_then(|u| u.backup.clone()) {
                Some((v, b)) => json!({"version": v, "algorithm": b["algorithm"], "auth_data": b["auth_data"], "count": 0, "etag": "0"}),
                None => json!({"errcode": "M_NOT_FOUND", "error": "No backup"}),
            }
        }).await;
        mount("PUT", r"^/_matrix/client/v3/room_keys/keys", |st, who, req| {
            let u = st.users.entry(user_of(who)).or_default();
            if let Some(rooms) = body(req)["rooms"].as_object() {
                for (r, v) in rooms {
                    let e = u.backup_keys.entry(r.clone()).or_insert_with(|| json!({"sessions": {}}));
                    if let Some(ss) = v["sessions"].as_object() { for (k, x) in ss { e["sessions"][k] = x.clone(); } }
                }
            }
            let n: usize = u.backup_keys.values().map(|r| r["sessions"].as_object().map(|o| o.len()).unwrap_or(0)).sum();
            st.log.push(format!("backup upload: {n} sessions stored"));
            json!({"etag": "1", "count": n})
        }).await;
        mount("GET", r"^/_matrix/client/v3/room_keys/keys", |st, who, req| {
            let parts: Vec<String> = req.url.path().split('/').map(|p| p.replace("%21", "!").replace("%3A", ":").replace("%3a", ":").replace("%2B", "+").replace("%2F", "/")).collect();
            let at = parts.iter().position(|p| p == "keys").unwrap_or(0);
            let all = st.users.get(&user_of(who)).map(|u| u.backup_keys.clone()).unwrap_or_default();
            st.log.push(format!("backup download {}", parts[at + 1..].join("/")));
            match (parts.get(at + 1), parts.get(at + 2)) {
                (Some(room), Some(session)) => all.get(room.as_str()).and_then(|r| r["sessions"].get(session.as_str())).cloned().unwrap_or_else(|| json!({"errcode": "M_NOT_FOUND", "error": "no such key"})),
                (Some(room), None) => all.get(room.as_str()).cloned().unwrap_or_else(|| json!({"sessions": {}})),
                _ => json!({"rooms": all}),
            }
        }).await;
        mount("GET", r"^/_matrix/client/v3/rooms/[^/]+/members$", |_, _, _| {
            let chunk: Vec<Value> = ["alice", "bob"].iter().enumerate().map(|(i, u)| { let mut e = member_event(u, 10 + i as u64); e["room_id"] = json!(ROOM); e }).collect();
            json!({"chunk": chunk})
        }).await;
        mount("GET", r"^/_matrix/client/v3/rooms/[^/]+/messages$", |st, _, req| {
            let from = req.url.query_pairs().find(|(k, _)| k == "from").map(|(_, v)| v.to_string()).unwrap_or_default();
            st.log.push(format!("messages from={from}"));
            if st.history.is_empty() || from != "pb0" { return json!({"chunk": [], "start": from}); }
            json!({"chunk": st.history.iter().rev().cloned().collect::<Vec<_>>(), "start": "pb0"}) /* no "end": the start of the room */
        }).await;
        {
            let st = st.clone();
            Mock::given(path_regex(".*")).respond_with(Handler { st, f: |st, who, req| { st.log.push(format!("UNHANDLED {} {} ({})", req.method, req.url.path(), who)); json!({"errcode": "M_UNRECOGNIZED", "error": "unrecognized"}) } })
                .with_priority(255).mount(&*server).await;
        }
        hs
    }

    /// Has this user's device uploaded its keys yet?
    pub fn has_device_keys(&self, user: &str) -> bool {
        self.st.lock().unwrap().users.get(user).map(|u| u.devices.values().any(|d| d.keys.is_some())).unwrap_or(false)
    }

    /// The files uploaded so far, in upload order.
    pub fn uploaded(&self) -> Vec<Vec<u8>> { self.st.lock().unwrap().media.iter().filter(|(k, _)| !k.starts_with("av_")).map(|(_, v)| v.clone()).collect() } /* what the client uploaded (not the seeded avatars) */

    /// From now on the server asks for this password before accepting cross-signing keys.
    pub fn require_password(&self, pw: &str) { self.st.lock().unwrap().uia_password = Some(pw.into()); }

    /// Bob invites alice to a room called `name`; returns its id.
    pub fn invite_alice(&self, name: &str) -> String {
        let mut st = self.st.lock().unwrap();
        st.counter += 1;
        let id = format!("!inv{}:hs", st.counter);
        st.extra_rooms.push((id.clone(), name.into(), false, vec![]));
        id
    }

    /// These users (local parts, e.g. "bob") are typing in the room from the next sync on.
    pub fn set_typing(&self, users: &[&str]) { self.st.lock().unwrap().typing = users.iter().map(|u| u.to_string()).collect(); }

    /// `user` (a local part, e.g. "bob") has read up to this event, from the next sync on.
    pub fn read_by(&self, user: &str, event_id: &str) { let mut st = self.st.lock().unwrap(); st.reads.retain(|r| r.0 != user); st.reads.push((user.into(), event_id.into())); }

    /// `count` old plain-text messages from bob that only /messages knows (the sync does not bring them), oldest first.
    pub fn set_history(&self, count: usize) {
        let mut st = self.st.lock().unwrap();
        st.history = (0..count).map(|i| json!({"type": "m.room.message", "room_id": ROOM, "sender": "@bob:hs", "event_id": format!("$old{i}"), "origin_server_ts": 1_000 + i as u64,
            "content": {"msgtype": "m.text", "body": format!("old message {i}")}})).collect();
    }

    /// A room alice is already in, called `name`; returns its id.
    pub fn add_room(&self, name: &str) -> String {
        let mut st = self.st.lock().unwrap();
        st.counter += 1;
        let id = format!("!room{}:hs", st.counter);
        st.extra_rooms.push((id.clone(), name.into(), true, vec![]));
        id
    }

    /// A joined space called `name` that contains these rooms (ids); returns the space's id.
    pub fn add_space(&self, name: &str, children: &[&str]) -> String {
        let mut st = self.st.lock().unwrap();
        st.counter += 1;
        let id = format!("!space{}:hs", st.counter);
        st.extra_rooms.push((id.clone(), name.into(), true, children.iter().map(|c| c.to_string()).collect()));
        id
    }

    pub fn uri(&self) -> String { self.server.uri() }
    pub fn room_events(&self) -> Vec<Value> { self.st.lock().unwrap().events.clone() }
    pub fn log(&self) -> Vec<String> { self.st.lock().unwrap().log.clone() }
}

/// Dev aid for running the UI without a real account: once `alice` has logged in (her device keys are on the server), a second client
/// "bob" says `lines` in the encrypted room, so the UI has encrypted messages to decrypt. Runs until done; spawn it.
pub async fn bob_says(hs: FakeHs, lines: Vec<String>) {
    use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
    for _ in 0..600 { if hs.has_device_keys("alice") { break; } tokio::time::sleep(std::time::Duration::from_millis(100)).await; }
    let dir = std::env::temp_dir().join(format!("vector-fake-bob-{}-{}", std::process::id(), hs.server.address().port()));
    let Ok(bob) = crate::open_client(&hs.uri(), &dir, "pw").await else { return };
    if crate::login_password(&bob, "bob", "x", "bob's phone").await.is_err() { return; } /* also starts the event cache */
    let _ = crate::sync_once(&bob).await;
    let Ok(room_id) = <&matrix_sdk::ruma::RoomId>::try_from(ROOM) else { return };
    let Some(room) = bob.get_room(room_id) else { return };
    for l in lines {
        let _ = room.send(RoomMessageEventContent::text_plain(l)).await;
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
    hs.set_typing(&["bob"]); /* a UI under test then shows "bob is typing" */
    loop { tokio::time::sleep(std::time::Duration::from_secs(3600)).await; } /* keep the temp store alive while the UI runs */
}

/// Dev aid: bob reads whatever alice writes (his read receipt follows her newest message).
pub async fn bob_reads_alice(hs: FakeHs) {
    let mut last = String::new();
    loop {
        let newest = hs.room_events().iter().rev().find(|e| e["sender"] == "@alice:hs").and_then(|e| e["event_id"].as_str().map(String::from));
        if let Some(id) = newest { if id != last { hs.read_by("bob", &id); last = id; } }
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }
}

/// Dev aid: "alice's other session" ("OTHER"): it owns the account's cross-signing identity, accepts any verification request and confirms the
/// emoji by itself, so a UI can be shown verifying itself. Runs until stopped; spawn it.
pub async fn alice_other_session(hs: FakeHs) {
    use crate::crypto::Verifier;
    let dir = std::env::temp_dir().join(format!("vector-fake-other-{}-{}", std::process::id(), hs.server.address().port()));
    let Ok(client) = crate::open_client(&hs.uri(), &dir, "pw").await else { return };
    if client.matrix_auth().login_username("alice", "x").device_id("OTHER").initial_device_display_name("Alice's laptop").send().await.is_err() { return; }
    crate::start_event_cache(&client);
    let _ = crate::sync_once(&client).await;
    match crate::crypto::enable_recovery(&client, None).await { Ok(k) => eprintln!("fake recovery key: {k}"), Err(_) => return }
    let state = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let seen = state.clone();
    let verifier = Verifier::new(client.clone(), move |s| { *seen.lock().unwrap() = s; });
    let me = client.user_id().unwrap().to_owned();
    loop {
        let _ = crate::sync_once(&client).await;
        let _ = client.encryption().request_user_identity(&me).await;
        let current: serde_json::Value = serde_json::from_str(&state.lock().unwrap()).unwrap_or(json!({}));
        match current["state"].as_str() {
            Some("incoming") => { let _ = verifier.accept().await; }
            Some("emoji") => { let _ = verifier.confirm().await; }
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    }
}
