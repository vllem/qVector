//! The application engine behind the Qt front end: everything a UI needs, with no toolkit in it.
//!
//! A UI creates an `App` with an event sink, then drives it with `call(method, args_json)`. Calls return at once (the work runs on the
//! engine's own tokio runtime); what comes of them is reported through the sink as events `(name, json)`, possibly from other threads.
//! A few calls answer directly (preferences, account info, emoji search). `capi.rs` puts a C ABI on this, `tests` below drive it like a UI.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use matrix_sdk::{config::SyncSettings, Client};
use matrix_sdk_ui::timeline::Timeline;
use serde_json::{json, Value};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::bookmarks::{Bookmark, Bookmarks};
use crate::crypto::Verifier;
use crate::index::MessageIndex;
use crate::ui;

/// Where events go: `(name, json)`. Called from engine threads: a UI must hand the event over to its own thread.
pub type Sink = Arc<dyn Fn(&str, String) + Send + Sync>;

#[derive(Default)]
struct Tasks { timeline: Option<JoinHandle<()>>, typing: Option<JoinHandle<()>>, thread: Option<JoinHandle<()>>, sync: Option<JoinHandle<()>> }

/// What the engine's tasks share.
struct Inner {
    sink: Sink,
    data_dir: Mutex<PathBuf>,
    client: Mutex<Option<Client>>,
    secret: Mutex<Option<String>>,
    verifier: Mutex<Option<Verifier>>,
    tasks: Mutex<Tasks>,
    timeline: tokio::sync::Mutex<Option<Arc<Timeline>>>,
    thread: tokio::sync::Mutex<Option<Arc<Timeline>>>,
    images: Mutex<HashMap<String, PathBuf>>,
    previews: Mutex<HashMap<String, Option<ui::UiPreview>>>,
    index: Mutex<Option<Arc<Mutex<MessageIndex>>>>,
    avatars: crate::avatars::Avatars,
    user_mxc: Mutex<HashMap<String, Option<String>>>, /* "room|user" -> that member's avatar address */
    bookmarks: Mutex<Option<Bookmarks>>,
    focused: AtomicBool,
    previews_on: AtomicBool,
    busy: AtomicBool,
    screen: Mutex<String>,
    #[cfg(feature = "testkit")]
    fake: Mutex<Option<crate::testkit::FakeHs>>,
}

pub struct App {
    rt: Option<Runtime>,
    inner: Arc<Inner>,
}

fn s(v: &Value, k: &str) -> String { v[k].as_str().unwrap_or("").to_string() }
fn b(v: &Value, k: &str) -> bool { v[k].as_bool().unwrap_or(false) }

impl Inner {
    fn emit(&self, name: &str, payload: impl Into<String>) { (self.sink)(name, payload.into()); }
    fn emit_json(&self, name: &str, v: &impl serde::Serialize) { self.emit(name, serde_json::to_string(v).unwrap_or_else(|_| "null".into())); }
    fn notice(&self, text: impl Into<String>) { self.emit("notice", text); }
    fn dir(&self) -> PathBuf { self.data_dir.lock().unwrap().clone() }
    fn client(&self) -> Option<Client> { self.client.lock().unwrap().clone() }

    fn state(&self, status: &str) {
        self.emit_json("state", &json!({"screen": *self.screen.lock().unwrap(), "status": status, "busy": self.busy.load(Ordering::Relaxed)}));
    }
    fn set_screen(&self, screen: &str, status: &str) { *self.screen.lock().unwrap() = screen.to_string(); self.state(status); }
    fn set_busy(&self, busy: bool, status: &str) { self.busy.store(busy, Ordering::Relaxed); self.state(status); }

    fn pref(&self, key: &str) -> String {
        let all: Value = std::fs::read_to_string(self.dir().join("prefs.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        all[key].as_str().unwrap_or("").to_string()
    }

    fn set_pref(&self, key: &str, value: &str) {
        let (dir, path) = (self.dir(), self.dir().join("prefs.json"));
        let mut all: Value = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_else(|| json!({}));
        all[key] = Value::String(value.to_string());
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&path, all.to_string());
    }

    fn open_index(&self) {
        let Some(secret) = self.secret.lock().unwrap().clone() else { return };
        match MessageIndex::open(&self.dir().join("index"), &secret) {
            Ok(i) => { *self.index.lock().unwrap() = Some(Arc::new(Mutex::new(i))); }
            Err(e) => eprintln!("message index: {e}"),
        }
    }

    fn avatar_dir(&self) -> PathBuf { self.dir().join("media").join("avatars") }

    /// Room details with the members' pictures: those already downloaded are in, the rest are fetched and the details are sent again.
    async fn send_details(&self, client: &Client, room_id: &str) {
        let Ok(mut d) = ui::room_details(client, room_id).await else { return };
        let fill = |d: &mut ui::UiRoomDetails| { for m in d.members.iter_mut().chain(d.banned.iter_mut()) { m.avatar_path = self.avatars.path(&m.avatar_mxc); } };
        fill(&mut d);
        self.emit_json("details", &d);
        let wanted: Vec<String> = d.members.iter().chain(d.banned.iter()).map(|m| m.avatar_mxc.clone()).filter(|m| self.avatars.wanted(m)).take(8).collect();
        if wanted.is_empty() { return; }
        for m in &wanted { self.avatars.fetch(client, m, &self.avatar_dir()).await; }
        fill(&mut d);
        self.emit_json("details", &d);
    }

    async fn open_timeline_of(&self) -> Option<Arc<Timeline>> { self.timeline.lock().await.clone() }
}

impl App {
    /// `data_dir`: where the saved session, caches and preferences live.
    pub fn new(data_dir: PathBuf, sink: Sink) -> App {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
        let inner = Arc::new(Inner {
            sink, data_dir: Mutex::new(data_dir), client: Mutex::new(None), secret: Mutex::new(None), verifier: Mutex::new(None), tasks: Mutex::new(Tasks::default()),
            timeline: Default::default(), thread: Default::default(), images: Default::default(), previews: Default::default(), index: Default::default(), avatars: Default::default(), user_mxc: Default::default(),
            bookmarks: Default::default(), focused: AtomicBool::new(true), previews_on: AtomicBool::new(false), busy: AtomicBool::new(false),
            screen: Mutex::new("login".into()),
            #[cfg(feature = "testkit")]
            fake: Default::default(),
        });
        App { rt: Some(rt), inner }
    }

    fn rt(&self) -> &Runtime { self.rt.as_ref().expect("runtime") }

    /// Run one engine call. Unknown methods answer `{"error": ...}`.
    pub fn call(&self, method: &str, args: &Value) -> Value {
        let i = self.inner.clone();
        match method {
            "init" => self.init(),
            "login" => self.login(args),
            "unlock" => { if !i.busy.load(Ordering::Relaxed) { self.restore_with(s(args, "passphrase")); } Value::Null }
            "sign_out" => self.sign_out(),
            #[cfg(feature = "testkit")]
            "start_fake_server" => self.start_fake_server(),
            "select_room" => self.select_room(&s(args, "room_id")),
            "send" => { let (t, r) = (s(args, "text"), s(args, "reply_to")); if !t.trim().is_empty() { self.on_timeline(move |_, tl| async move { let _ = ui::send_text(&tl, &t, Some(r.as_str())).await; }); } Value::Null }
            "send_file" => { let p = PathBuf::from(s(args, "path").trim_start_matches("file://")); let cap = s(args, "caption"); self.on_timeline(move |i, tl| async move { if let Err(e) = ui::send_file(&tl, &p, Some(&cap)).await { i.notice(format!("Cannot send the file: {e}")); } }); Value::Null }
            "send_files" => { /* several files become one gallery message; `paths`: array of paths */
                let ps: Vec<PathBuf> = args.get("paths").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str()).map(|p| PathBuf::from(p.trim_start_matches("file://"))).collect()).unwrap_or_default();
                let cap = s(args, "caption");
                self.on_timeline(move |i, tl| async move { if let Err(e) = ui::send_gallery(&tl, &ps, Some(&cap)).await { i.notice(format!("Cannot send the files: {e}")); } }); Value::Null }
            "react" => { let (e, k) = (s(args, "event_id"), s(args, "key")); self.on_timeline(move |_, tl| async move { let _ = ui::toggle_reaction(&tl, &e, &k).await; }); Value::Null }
            "edit" => { let (e, t) = (s(args, "event_id"), s(args, "text")); self.on_timeline(move |_, tl| async move { let _ = ui::edit_text(&tl, &e, &t).await; }); Value::Null }
            "redact" => { let e = s(args, "event_id"); self.on_timeline(move |_, tl| async move { let _ = ui::redact(&tl, &e).await; }); Value::Null }
            "load_reply" => { let e = s(args, "event_id"); self.on_timeline(move |_, tl| async move { let _ = ui::load_reply(&tl, &e).await; }); Value::Null }
            "load_older" => { self.on_timeline(move |i, tl| async move { let reached = ui::load_older(&tl).await.unwrap_or(false); i.emit_json("older", &json!({"reached": reached})); }); Value::Null }
            "search_messages" => self.search_messages(&s(args, "query")),
            "search_all" => self.search_all(&s(args, "query")),
            "save_attachment" => self.save_attachment(&s(args, "event_id"), &s(args, "dest")),
            "open_attachment" => self.open_attachment(&s(args, "event_id"), "open_file"),
            "fetch_text" => { let id = s(args, "event_id"); self.on_timeline(move |i, tl| async move { if let Some(c) = i.client() { match ui::read_text_file(&c, &tl, &id, 2_000_000).await { Ok(t) => i.emit_json("text_file", &t), Err(e) => i.notice(format!("Cannot open the file: {e}")) } } }); Value::Null }
            "fetch_media" => self.open_attachment(&s(args, "event_id"), "media_file"),
            "accept_invite" => { let id = s(args, "room_id"); self.client_action("Joined the room", move |c| async move { ui::accept_invite(&c, &id).await.map(|_| id) }) }
            "leave_room" => { let id = s(args, "room_id"); self.client_action("Left the room", move |c| async move { ui::leave_room(&c, &id).await.map(|_| id) }) }
            "set_room_tag" => { let (id, k) = (s(args, "room_id"), s(args, "kind")); self.client_action("Room updated", move |c| async move { ui::set_room_tag(&c, &id, &k).await.map(|_| id) }) }
            "set_room_notify" => { let (id, l) = (s(args, "room_id"), s(args, "level")); self.client_action("Notification level changed", move |c| async move { ui::set_room_notification_level(&c, &id, &l).await.map(|_| id) }) }
            "create_room" => {
                let (n, t, e, p) = (s(args, "name"), s(args, "topic"), b(args, "encrypted"), b(args, "public"));
                let invites: Vec<String> = args["invites"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                self.client_action("Room created", move |c| async move { ui::create_room_with(&c, &n, &t, e, p, &invites).await })
            }
            "search_users" => { let (i, term) = (i.clone(), s(args, "term")); self.rt().spawn(async move { if let Some(c) = i.client() { match ui::search_users(&c, &term).await { Ok(u) => i.emit_json("users", &u), Err(e) => i.notice(format!("User search failed: {e}")) } } }); Value::Null }
            "public_rooms" => { let (i, term, server) = (i.clone(), s(args, "term"), s(args, "server")); self.rt().spawn(async move { if let Some(c) = i.client() { match ui::public_directory(&c, &term, &server).await { Ok(r) => i.emit_json("directory", &r), Err(e) => { i.emit("directory", "[]"); i.notice(format!("Room directory failed: {e}")); } } } }); Value::Null }
            "join_room" => { let a = s(args, "address"); self.client_action("Joined the room", move |c| async move { ui::join_by_address(&c, &a).await }) }
            "forward" => {
                let (id, rooms): (String, Vec<String>) = (s(args, "event_id"), args["room_ids"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default());
                self.on_timeline(move |i, tl| async move { if let Some(c) = i.client() { i.notice(match ui::forward_message(&c, &tl, &id, &rooms).await { Ok(n) => format!("Forwarded to {n} room(s)"), Err(e) => format!("Could not forward: {e}") }); } });
                Value::Null
            }
            "edit_history" => { let id = s(args, "event_id"); self.on_timeline(move |i, tl| async move { match ui::edit_history(&tl, &id).await { Ok(h) => i.emit_json("edit_history", &h), Err(e) => i.notice(format!("No edit history: {e}")) } }); Value::Null }
            "start_dm" => { let u = s(args, "user_id"); self.client_action("Conversation started", move |c| async move { ui::start_dm(&c, &u).await }) }
            "room_action" => self.room_edit(&s(args, "kind"), &s(args, "a"), &s(args, "b")),
            "set_focus" => self.set_focus(b(args, "focused")),
            "typing" => { let t = b(args, "typing"); let (i, rt) = (i.clone(), self.rt()); rt.spawn(async move { if let (Some(c), Some(tl)) = (i.client(), i.open_timeline_of().await) { let _ = ui::set_typing(&c, tl.room().room_id().as_str(), t).await; } }); Value::Null }
            "pin_message" => self.pin_message(&s(args, "event_id"), b(args, "pinned")),
            "create_poll" => {
                let (q, opts, multi) = (s(args, "question"), args["options"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default(), b(args, "multiple"));
                self.on_timeline(move |i, tl| async move { let n = if multi { opts.len() as u32 } else { 1 }; if let Err(e) = ui::create_poll(&tl, &q, &opts, n).await { i.notice(format!("Failed: {e}")); } });
                Value::Null
            }
            "vote_poll" => {
                let (id, answers) = (s(args, "poll_id"), args["answers"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default());
                self.on_timeline(move |_, tl| async move { let _ = ui::vote_poll(&tl, &id, answers).await; });
                Value::Null
            }
            "end_poll" => { let id = s(args, "poll_id"); self.on_timeline(move |_, tl| async move { let _ = ui::end_poll(&tl, &id).await; }); Value::Null }
            "open_thread" => self.open_thread(&s(args, "root_id")),
            "close_thread" => { if let Some(t) = i.tasks.lock().unwrap().thread.take() { t.abort(); } let i = i.clone(); self.rt().spawn(async move { *i.thread.lock().await = None; }); Value::Null }
            "send_thread" => { let (i, t) = (i.clone(), s(args, "text")); self.rt().spawn(async move { let tl = i.thread.lock().await.clone(); if let Some(tl) = tl { let _ = ui::send_text(&tl, &t, None).await; } }); Value::Null }
            "toggle_bookmark" => self.toggle_bookmark(&s(args, "event_id")),
            "remove_bookmark" => {
                let list = { let mut g = i.bookmarks.lock().unwrap(); match g.as_mut() { Some(bk) => { if bk.remove(&s(args, "event_id")).is_ok() { Some(bk.list()) } else { None } } None => None } };
                if let Some(l) = list { i.emit_json("bookmarks", &l); }
                Value::Null
            }
            "pref" => Value::String(i.pref(&s(args, "key"))),
            "set_pref" => { i.set_pref(&s(args, "key"), &s(args, "value")); Value::Null }
            "set_message_index" => {
                if b(args, "on") { i.open_index(); } else { *i.index.lock().unwrap() = None; let dir = i.dir().join("index"); self.rt().spawn(async move { let _ = std::fs::remove_dir_all(dir); }); }
                Value::Null
            }
            "set_previews" => { i.previews_on.store(b(args, "on"), Ordering::Relaxed); Value::Null }
            "search_emoji" => json!(crate::emoji::search(&s(args, "query"), args["limit"].as_u64().unwrap_or(160) as usize)),
            "account_info" => match i.client() {
                Some(c) => json!({"user_id": c.user_id().map(|u| u.to_string()), "device_id": c.device_id().map(|d| d.to_string()), "homeserver": c.homeserver().to_string()}),
                None => json!({}),
            },
            "load_sessions" => { let i = i.clone(); self.rt().spawn(async move { if let Some(c) = i.client() { let out = crate::crypto::session_list(&c).await.unwrap_or_else(|_| json!([])); i.emit("sessions", out.to_string()); } }); Value::Null }
            "recover" => { let (i, key) = (i.clone(), s(args, "key")); self.rt().spawn(async move { if let Some(c) = i.client() { let out = match crate::crypto::recover(&c, &key).await { Ok(()) => { let _ = crate::crypto::restore_keys(&c).await; json!({"state": "done"}) } Err(e) => json!({"state": "error", "message": e}) }; i.emit("recovery", out.to_string()); } }); Value::Null }
            "create_recovery" => {
                let (i, pw) = (i.clone(), s(args, "password"));
                self.rt().spawn(async move {
                    if let Some(c) = i.client() {
                        let out = match crate::crypto::enable_recovery(&c, Some(pw.as_str()).filter(|p| !p.is_empty())).await {
                            Ok(key) => json!({"state": "created", "key": key}),
                            Err(e) if e == crate::crypto::PASSWORD_REQUIRED => json!({"state": "password"}),
                            Err(e) => json!({"state": "error", "message": e}),
                        };
                        i.emit("recovery", out.to_string());
                    }
                });
                Value::Null
            }
            "request_verification" => self.verification(|v, i| async move { if let Err(e) = v.request_own().await { i.emit("verification", json!({"state": "cancelled", "reason": e}).to_string()); } }),
            "request_user_verification" => { let u = s(args, "user_id"); self.verification(move |v, i| async move { if let Err(e) = v.request_user(&u).await { i.emit("verification", json!({"state": "cancelled", "reason": e, "user": u}).to_string()); } }) }
            "accept_verification" => self.verification(|v, _| async move { let _ = v.accept().await; }),
            "confirm_verification" => self.verification(|v, _| async move { let _ = v.confirm().await; }),
            "cancel_verification" => self.verification(|v, _| async move { let _ = v.cancel().await; }),
            "dismiss_verification" => { if let Some(v) = i.verifier.lock().unwrap().as_ref() { v.dismiss(); } Value::Null }
            other => json!({"error": format!("unknown method {other}")}),
        }
    }

    // ---- helpers to run work against the open room / the client

    fn on_timeline<F, Fut>(&self, f: F) where F: FnOnce(Arc<Inner>, Arc<Timeline>) -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static {
        let i = self.inner.clone();
        self.rt().spawn(async move { if let Some(tl) = i.open_timeline_of().await { f(i.clone(), tl).await; } });
    }

    /// Run a client action and say what happened.
    fn client_action<F, Fut>(&self, ok: &'static str, f: F) -> Value where F: FnOnce(Client) -> Fut + Send + 'static, Fut: std::future::Future<Output = Result<String, String>> + Send + 'static {
        let i = self.inner.clone();
        self.rt().spawn(async move {
            if let Some(c) = i.client() { let text = match f(c).await { Ok(_) => ok.to_string(), Err(e) => format!("Failed: {e}") }; i.notice(text); }
        });
        Value::Null
    }

    fn verification<F, Fut>(&self, f: F) -> Value where F: FnOnce(Verifier, Arc<Inner>) -> Fut + Send + 'static, Fut: std::future::Future<Output = ()> + Send + 'static {
        let v = self.inner.verifier.lock().unwrap().clone();
        if let Some(v) = v { let i = self.inner.clone(); self.rt().spawn(async move { f(v, i).await; }); }
        Value::Null
    }

    // ---- session

    fn init(&self) -> Value {
        use crate::session::{self, Protection};
        let i = &self.inner;
        match session::protection(&i.dir()) {
            None => i.state(""),
            Some(Protection::Passphrase) => i.set_screen("unlock", ""),
            Some(Protection::KeyFile) => match session::saved_key_secret(&i.dir()) {
                Ok(secret) => self.restore_with(secret),
                Err(e) => i.state(&format!("The saved session cannot be opened: {e}")),
            },
        }
        Value::Null
    }

    fn login(&self, a: &Value) -> Value {
        let i = self.inner.clone();
        if i.busy.load(Ordering::Relaxed) { return Value::Null; }
        let (hs, user, pw, pass) = (s(a, "homeserver"), s(a, "user"), s(a, "password"), s(a, "passphrase"));
        if hs.trim().is_empty() || user.trim().is_empty() { i.state("Enter the server and your user name."); return Value::Null; }
        i.set_busy(true, "Signing in...");
        self.rt().spawn(async move {
            let dir = i.dir();
            match crate::session::sign_in(&dir, &hs, &user, &pw, Some(pass.as_str()).filter(|p| !p.is_empty()), "Vector").await {
                Ok(client) => {
                    let secret = if pass.is_empty() { crate::session::saved_key_secret(&dir).unwrap_or_default() } else { pass.clone() };
                    start_session(i, client, secret).await;
                }
                Err(e) => login_failed(&i, &e),
            }
        });
        Value::Null
    }

    fn restore_with(&self, secret: String) {
        let i = self.inner.clone();
        i.set_busy(true, "Opening your account...");
        self.rt().spawn(async move {
            match crate::session::restore(&i.dir(), &secret).await {
                Ok(client) => start_session(i, client, secret).await,
                Err(e) => login_failed(&i, &e),
            }
        });
    }

    fn stop_tasks(&self) {
        let mut t = self.inner.tasks.lock().unwrap();
        for h in [t.timeline.take(), t.typing.take(), t.thread.take(), t.sync.take()].into_iter().flatten() { h.abort(); }
    }

    fn sign_out(&self) -> Value {
        self.stop_tasks();
        let i = self.inner.clone();
        *i.index.lock().unwrap() = None;
        *i.bookmarks.lock().unwrap() = None;
        *i.verifier.lock().unwrap() = None;
        let client = i.client.lock().unwrap().take();
        let dir = i.dir();
        self.rt().spawn(async move {
            *i.timeline.lock().await = None;
            *i.thread.lock().await = None;
            if let Some(c) = client { let _ = c.matrix_auth().logout().await; }
            crate::session::forget(&dir);
        });
        self.inner.set_screen("login", "");
        self.inner.emit("rooms", "[]");
        self.inner.emit("timeline", json!({"room_id": "", "rows": []}).to_string());
        Value::Null
    }

    #[cfg(feature = "testkit")]
    fn start_fake_server(&self) -> Value {
        let hs = self.rt().block_on(crate::testkit::FakeHs::start());
        hs.invite_alice("Bob's club");
        hs.set_history(40);
        let plans = hs.add_room("Plans");
        hs.add_space("Rust club", &[plans.as_str()]);
        hs.add_room("Lounge");
        *self.inner.data_dir.lock().unwrap() = std::env::temp_dir().join(format!("vector-demo-{}", std::process::id())); /* never the real saved session */
        let lines = ["Hello alice, this room is end-to-end encrypted.", "You are reading it through matrix-sdk.", "Reply below!"].map(String::from).to_vec();
        self.rt().spawn(crate::testkit::bob_says(hs.clone(), lines));
        self.rt().spawn(crate::testkit::bob_reads_alice(hs.clone()));
        let uri = hs.uri();
        *self.inner.fake.lock().unwrap() = Some(hs);
        json!({"homeserver": uri})
    }

    // ---- rooms and the open timeline

    fn set_focus(&self, focused: bool) -> Value {
        self.inner.focused.store(focused, Ordering::Relaxed);
        if focused { self.on_timeline(|_, tl| async move { let _ = ui::mark_read(&tl).await; }); }
        Value::Null
    }

    fn select_room(&self, room_id: &str) -> Value {
        let Some(client) = self.inner.client() else { return Value::Null };
        self.inner.tasks.lock().unwrap().timeline.take().map(|t| t.abort());
        self.inner.tasks.lock().unwrap().typing.take().map(|t| t.abort());
        self.inner.tasks.lock().unwrap().thread.take().map(|t| t.abort());
        let i = self.inner.clone();
        let id = room_id.to_string();
        let cache = i.dir().join("media");
        i.emit("typing", "[]");
        {
            let _g = self.rt().enter();
            let sink = i.clone();
            let last = Mutex::new(String::from("[]"));
            let h = ui::watch_typing(&client, &id, move |names| {
                let now = serde_json::to_string(&names).unwrap_or_default();
                let mut l = last.lock().unwrap();
                if *l != now { *l = now.clone(); sink.emit("typing", now); } /* the SDK repeats the same list with every sync */
            }).ok();
            self.inner.tasks.lock().unwrap().typing = h;
        }
        {   /* name, topic, members: for the header and the details panel */
            let (client, id, i) = (client.clone(), id.clone(), i.clone());
            self.rt().spawn(async move { i.send_details(&client, &id).await; });
        }
        let handle = self.rt().spawn(async move {
            use futures_util::StreamExt;
            let Ok(rid) = <&matrix_sdk::ruma::RoomId>::try_from(id.as_str()) else { return };
            let Some(room) = client.get_room(rid) else { return };
            let Ok(timeline) = ui::open_timeline(&room).await else { return };
            let timeline = Arc::new(timeline);
            *i.timeline.lock().await = Some(timeline.clone());
            let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
            let (initial, mut stream) = timeline.subscribe().await;
            let mut items: Vec<_> = initial.iter().cloned().collect();
            loop {
                if items.iter().any(|x| x.as_event().map(|e| e.content().is_unable_to_decrypt()).unwrap_or(false)) { ui::retry_undecryptable(&timeline).await; }
                let have = i.images.lock().unwrap().clone();
                let mut rows = ui::ui_messages_with(&items, &me, &have);
                let pinned = ui::pinned_ids(&client, &id);
                for r in rows.iter_mut() { r.pinned = pinned.contains(&r.id); }
                {   /* the senders' pictures: what is downloaded is in the rows, the rest is looked up and fetched below */
                    let senders: Vec<String> = { let mut v: Vec<String> = rows.iter().map(|r| r.sender_id.clone()).collect(); v.sort(); v.dedup(); v };
                    for u in &senders {
                        let key = format!("{id}|{u}");
                        if !i.user_mxc.lock().unwrap().contains_key(&key) {
                            let mxc = match <&matrix_sdk::ruma::UserId>::try_from(u.as_str()) { Ok(uid) => room.get_member_no_sync(uid).await.ok().flatten().and_then(|m| m.avatar_url().map(|a| a.to_string())), Err(_) => None };
                            i.user_mxc.lock().unwrap().insert(key, mxc);
                        }
                    }
                    let known = i.user_mxc.lock().unwrap().clone();
                    for r in rows.iter_mut() { if let Some(Some(m)) = known.get(&format!("{id}|{}", r.sender_id)) { r.avatar_path = i.avatars.path(m); } }
                }
                let previews_on = i.previews_on.load(Ordering::Relaxed);
                if previews_on {
                    let known = i.previews.lock().unwrap();
                    for r in rows.iter_mut() { if let Some(u) = ui::first_url(&r.body) { r.preview = known.get(&u).cloned().flatten(); } }
                }
                i.emit("timeline", json!({"room_id": id, "rows": rows}).to_string());
                if i.focused.load(Ordering::Relaxed) { let _ = ui::mark_read(&timeline).await; }
                if let Some(index) = i.index.lock().unwrap().clone() { let _ = index.lock().map(|mut x| x.add_items(&id, &items)); }
                let added = ui::fetch_images(&client, &items, &cache, &have).await;
                if !added.is_empty() { i.images.lock().unwrap().extend(added); continue; } /* show the pictures that just arrived */
                {
                    let wanted: Vec<String> = { let known = i.user_mxc.lock().unwrap(); rows.iter().filter_map(|r| known.get(&format!("{id}|{}", r.sender_id)).cloned().flatten()).filter(|m| i.avatars.wanted(m)).take(4).collect() };
                    for m in &wanted { i.avatars.fetch(&client, m, &i.avatar_dir()).await; }
                    if !wanted.is_empty() { continue; }
                }
                if previews_on { /* a few new links per pass, the newest messages first */
                    let wanted: Vec<String> = { let known = i.previews.lock().unwrap(); rows.iter().rev().take(30).filter_map(|r| ui::first_url(&r.body)).filter(|u| !known.contains_key(u)).take(3).collect() };
                    for u in &wanted { let p = ui::link_preview(&client, u, &cache).await; i.previews.lock().unwrap().insert(u.clone(), p); }
                    if !wanted.is_empty() { continue; }
                }
                if stream.next().await.is_none() { break; }
                items = timeline.items().await.iter().cloned().collect();
            }
        });
        self.inner.tasks.lock().unwrap().timeline = Some(handle);
        Value::Null
    }

    fn open_thread(&self, root_id: &str) -> Value {
        let Some(client) = self.inner.client() else { return Value::Null };
        self.inner.tasks.lock().unwrap().thread.take().map(|t| t.abort());
        let (i, root) = (self.inner.clone(), root_id.to_string());
        let h = self.rt().spawn(async move {
            use futures_util::StreamExt;
            let Some(main) = i.open_timeline_of().await else { return };
            let room = main.room().clone();
            let Ok(timeline) = ui::open_thread(&room, &root).await else { return };
            let timeline = Arc::new(timeline);
            *i.thread.lock().await = Some(timeline.clone());
            let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
            let (initial, mut stream) = timeline.subscribe().await;
            let mut items: Vec<_> = initial.iter().cloned().collect();
            loop {
                i.emit_json("thread", &ui::ui_messages(&items, &me));
                if stream.next().await.is_none() { break; }
                items = timeline.items().await.iter().cloned().collect();
            }
        });
        self.inner.tasks.lock().unwrap().thread = Some(h);
        Value::Null
    }

    fn search_messages(&self, query: &str) -> Value {
        let (i, q) = (self.inner.clone(), query.to_string());
        self.rt().spawn(async move {
            let (Some(client), Some(tl)) = (i.client(), i.open_timeline_of().await) else { return };
            let items: Vec<_> = tl.items().await.iter().cloned().collect();
            let me = client.user_id().map(|u| u.to_string()).unwrap_or_default();
            let hits = ui::search_messages(&items, &me, &q, 50);
            let out: Vec<_> = hits.iter().map(|m| json!({"id": m.id, "sender": m.sender, "body": m.body, "time": m.time})).collect();
            i.emit_json("search", &out);
        });
        Value::Null
    }

    fn search_all(&self, query: &str) -> Value {
        let (i, q) = (self.inner.clone(), query.to_string());
        self.rt().spawn(async move {
            let Some(client) = i.client() else { return };
            let index = i.index.lock().unwrap().clone();
            let hits = index.map(|x| x.lock().map(|x| x.search(&q, None, 60)).unwrap_or_default()).unwrap_or_default();
            let mut out = Vec::new();
            for h in hits {
                let room = <&matrix_sdk::ruma::RoomId>::try_from(h.room_id.as_str()).ok().and_then(|id| client.get_room(id));
                let name = match room { Some(r) => r.display_name().await.map(|n| n.to_string()).unwrap_or_default(), None => String::new() };
                out.push(json!({"id": h.event_id, "room_id": h.room_id, "room": name, "sender": h.sender, "body": h.body, "time": h.time}));
            }
            i.emit_json("search", &out);
        });
        Value::Null
    }

    fn save_attachment(&self, event_id: &str, dest: &str) -> Value {
        let (i, id, dest) = (self.inner.clone(), event_id.to_string(), dest.strip_prefix("file://").unwrap_or(dest).to_string());
        self.rt().spawn(async move {
            let (Some(client), tl) = (i.client(), i.open_timeline_of().await) else { return };
            let text = match tl {
                Some(tl) => match ui::save_attachment(&client, &tl, &id, std::path::Path::new(&dest)).await { Ok(n) => format!("Saved {n} bytes to {dest}"), Err(e) => format!("Could not save: {e}") },
                None => "Open a room first".to_string(),
            };
            i.notice(text);
        });
        Value::Null
    }

    /// Download a picture/video/audio into the cache; `event` is "open_file" (the UI opens it with the system player: payload = the path)
    /// or "media_file" (the UI plays it itself: payload = {"event_id", "path"}).
    fn open_attachment(&self, event_id: &str, event: &'static str) -> Value {
        let (i, id) = (self.inner.clone(), event_id.to_string());
        self.rt().spawn(async move {
            let (Some(client), Some(tl)) = (i.client(), i.open_timeline_of().await) else { return };
            match ui::media_copy(&client, &tl, &id, &i.dir().join("media").join("open")).await {
                Ok(path) if event == "media_file" => i.emit_json("media_file", &json!({"event_id": id, "path": path.to_string_lossy()})),
                Ok(path) => i.emit(event, path.to_string_lossy().into_owned()),
                Err(e) => i.notice(format!("Cannot open: {e}")),
            }
        });
        Value::Null
    }

    /// Moderation and room editing in the open room: kick, ban, unban, invite, texts (a = name, b = topic), or a role (member/moderator/admin).
    fn room_edit(&self, kind: &str, a: &str, bb: &str) -> Value {
        let (i, kind, a, bb) = (self.inner.clone(), kind.to_string(), a.to_string(), bb.to_string());
        self.rt().spawn(async move {
            let (Some(client), Some(tl)) = (i.client(), i.open_timeline_of().await) else { return };
            let room = tl.room().room_id().to_string();
            let result = match kind.as_str() {
                "kick" => ui::kick_member(&client, &room, &a, &bb).await.map(|_| "Removed from the room"),
                "ban" => ui::ban_member(&client, &room, &a, &bb).await.map(|_| "Banned"),
                "unban" => ui::unban_member(&client, &room, &a).await.map(|_| "Unbanned"),
                "invite" => ui::invite_member(&client, &room, &a).await.map(|_| "Invitation sent"),
                "texts" => ui::set_room_texts(&client, &room, &a, &bb).await.map(|_| "Room updated"),
                role => ui::set_member_role(&client, &room, &a, role).await.map(|_| "Role changed"),
            };
            i.notice(match result { Ok(t) => t.to_string(), Err(e) => format!("Failed: {e}") });
            tokio::time::sleep(Duration::from_millis(800)).await; /* let the sync bring the new state */
            let _ = crate::sync_once(&client).await;
            i.send_details(&client, &room).await;
        });
        Value::Null
    }

    fn pin_message(&self, event_id: &str, pinned: bool) -> Value {
        let (i, e) = (self.inner.clone(), event_id.to_string());
        self.rt().spawn(async move {
            let (Some(client), Some(tl)) = (i.client(), i.open_timeline_of().await) else { return };
            let room = tl.room().room_id().to_string();
            i.notice(match ui::set_pinned(&client, &room, &e, pinned).await { Ok(()) => if pinned { "Pinned" } else { "Unpinned" }.to_string(), Err(e) => format!("Failed: {e}") });
        });
        Value::Null
    }

    fn toggle_bookmark(&self, event_id: &str) -> Value {
        let (i, id) = (self.inner.clone(), event_id.to_string());
        self.rt().spawn(async move {
            let Some(tl) = i.open_timeline_of().await else { return };
            let items: Vec<_> = tl.items().await.iter().cloned().collect();
            let room = tl.room().room_id().to_string();
            let Some(m) = ui::ui_messages(&items, "").into_iter().find(|m| m.id == id) else { return };
            let list = {
                let mut g = i.bookmarks.lock().unwrap();
                let Some(bk) = g.as_mut() else { return };
                if bk.toggle(Bookmark { room_id: room, event_id: m.id, sender: m.sender, time: m.time, body: m.body }).is_err() { return; }
                bk.list()
            };
            i.emit_json("bookmarks", &list);
        });
        Value::Null
    }
}

impl Drop for App {
    /// The SDK's stores need the runtime while they shut down: release them inside it, then stop the runtime.
    fn drop(&mut self) {
        let Some(rt) = self.rt.take() else { return };
        {
            let _g = rt.enter();
            self.stop_tasks();
            if let Ok(mut tl) = self.inner.timeline.try_lock() { tl.take(); }
            if let Ok(mut tl) = self.inner.thread.try_lock() { tl.take(); }
            *self.inner.verifier.lock().unwrap() = None;
            *self.inner.client.lock().unwrap() = None;
            #[cfg(feature = "testkit")]
            { *self.inner.fake.lock().unwrap() = None; }
        }
        rt.shutdown_timeout(Duration::from_secs(2));
    }
}

fn login_failed(i: &Arc<Inner>, message: &str) {
    /* a saved session that cannot be opened goes back to the sign-in with the reason; the passphrase prompt stays */
    let keep = *i.screen.lock().unwrap() == "unlock" && message.contains("passphrase");
    if !keep { *i.screen.lock().unwrap() = "login".into(); }
    i.set_busy(false, message);
}

/// A signed-in client: wire up verification, local stores and the sync loop, then tell the UI the account is open.
async fn start_session(i: Arc<Inner>, client: Client, secret: String) {
    *i.client.lock().unwrap() = Some(client.clone());
    *i.secret.lock().unwrap() = Some(secret.clone());
    let sink = i.clone();
    *i.verifier.lock().unwrap() = Some(Verifier::new(client.clone(), move |json| sink.emit("verification", json)));
    if i.pref("messageIndex") == "1" { i.open_index(); }
    if i.pref("linkPreviews") == "1" { i.previews_on.store(true, Ordering::Relaxed); }
    match Bookmarks::open(&i.dir().join("bookmarks"), &secret) {
        Ok(bk) => { let list = bk.list(); *i.bookmarks.lock().unwrap() = Some(bk); i.emit_json("bookmarks", &list); }
        Err(e) => eprintln!("bookmarks: {e}"),
    }
    i.busy.store(false, Ordering::Relaxed);
    i.set_screen("main", "");
    let task = i.clone();
    let handle = tokio::spawn(async move { sync_loop(task, client).await });
    i.tasks.lock().unwrap().sync = Some(handle);
}

/// Pages of 30 events fetched per room in the background: about 1000 messages deep.
const PREFETCH_PAGES: usize = 34;

/// Sync, then tell the UI about the room list, alerts and the session's trust state; also feeds the message index.
async fn sync_loop(i: Arc<Inner>, client: Client) {
    let mut last_status = String::new();
    let (mut last_save, mut crawled) = (std::time::Instant::now(), HashSet::<String>::new());
    let mut seen_unread = None;
    let prefetching = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut keys_restored = false;
    i.avatars.use_dir(&i.avatar_dir());
    let spaces = matrix_sdk_ui::spaces::SpaceService::new(client.clone()).await;
    loop {
        let _ = client.sync_once(SyncSettings::default().timeout(Duration::from_secs(0))).await;
        let mut rooms = ui::ui_rooms_with_spaces(&client, &spaces).await;
        for m in rooms.iter().map(|r| r.avatar_mxc.clone()).filter(|m| i.avatars.wanted(m)).take(3).collect::<Vec<_>>() { i.avatars.fetch(&client, &m, &i.avatar_dir()).await; }
        for r in rooms.iter_mut() { r.avatar_path = i.avatars.path(&r.avatar_mxc); }
        let index = i.index.lock().unwrap().clone();
        /* Keep about a thousand messages of every room on the computer (the SDK's event cache is on disk): one room at a time in the background, so
           replies, searches and scrolling back find their messages without the network. */
        if !prefetching.load(Ordering::Relaxed) {
            if let Some(r) = rooms.iter().find(|r| !r.invite && !crawled.contains(&r.id)) {
                crawled.insert(r.id.clone());
                if let Some(room) = <&matrix_sdk::ruma::RoomId>::try_from(r.id.as_str()).ok().and_then(|id| client.get_room(id)) {
                    prefetching.store(true, Ordering::Relaxed);
                    let (flag, index) = (prefetching.clone(), index.clone());
                    tokio::spawn(async move { let _ = crate::index::crawl_room(&room, index.as_deref(), PREFETCH_PAGES).await; flag.store(false, Ordering::Relaxed); });
                }
            }
        }
        if let Some(index) = &index {
            if last_save.elapsed() > Duration::from_secs(20) { last_save = std::time::Instant::now(); let _ = index.lock().map(|mut x| x.save()); }
        }
        i.emit_json("rooms", &rooms);
        let open = i.open_timeline_of().await.map(|t| t.room().room_id().to_string());
        let focused = i.focused.load(Ordering::Relaxed);
        let alerts = ui::new_alerts(&mut seen_unread, &rooms, if focused { open.as_deref() } else { None });
        if !alerts.is_empty() { i.emit_json("alerts", &alerts); }
        if let Some(me) = client.user_id() { let _ = client.encryption().request_user_identity(me).await; } /* learn about our other sessions' identity */
        if !keys_restored { if let Some(n) = crate::crypto::restore_keys(&client).await { keys_restored = true; i.notice(format!("Restoring old messages from your key backup ({n} rooms)...")); } } /* once the backup is usable here: after a recovery key, a verification, at start */
        let status = crate::crypto::session_status(&client).await.to_string();
        if status != last_status { last_status = status.clone(); i.emit("session", status); }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

#[cfg(all(test, feature = "testkit"))]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A UI stand-in: collects the events.
    struct Probe { rx: mpsc::Receiver<(String, String)>, seen: Vec<(String, String)> }
    impl Probe {
        fn wait(&mut self, what: &str, ok: impl Fn(&Value) -> bool) -> Value {
            let deadline = std::time::Instant::now() + Duration::from_secs(60);
            loop {
                for (n, p) in self.seen.iter().rev() { if n == what { if let Ok(v) = serde_json::from_str::<Value>(p) { if ok(&v) { return v; } } } }
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                match self.rx.recv_timeout(left.min(Duration::from_millis(500))) { Ok(e) => self.seen.push(e), Err(_) if std::time::Instant::now() > deadline => panic!("never saw {what}: {:?}", self.seen.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>()), Err(_) => {} }
            }
        }
    }

    fn app() -> (App, Probe, tempfile::TempDir) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let dir = tempfile::tempdir().unwrap();
        let sink: Sink = Arc::new(move |n, p| { let _ = tx.lock().unwrap().send((n.to_string(), p)); });
        (App::new(dir.path().to_path_buf(), sink), Probe { rx, seen: vec![] }, dir)
    }

    #[test]
    fn the_engine_logs_in_lists_rooms_shows_the_timeline_sends_and_reacts() {
        let (app, mut probe, _dir) = app();
        let fake = app.call("start_fake_server", &Value::Null);
        app.call("login", &json!({"homeserver": fake["homeserver"], "user": "alice", "password": "x"}));
        probe.wait("state", |v| v["screen"] == "main");
        let rooms = probe.wait("rooms", |v| v.as_array().map(|a| a.iter().any(|r| r["title"] == "bob")).unwrap_or(false));
        let room = rooms.as_array().unwrap().iter().find(|r| r["title"] == "bob").unwrap()["id"].as_str().unwrap().to_string();
        assert!(rooms.as_array().unwrap().iter().any(|r| r["section"] == "Rust club"), "{rooms}");
        app.call("select_room", &json!({"room_id": room}));
        let t = probe.wait("timeline", |v| v["rows"].as_array().map(|a| a.iter().any(|m| m["body"].as_str().unwrap_or("").starts_with("Hello alice"))).unwrap_or(false));
        assert_eq!(t["room_id"], room.as_str());
        app.call("send", &json!({"text": "hi **there**", "reply_to": ""}));
        let t = probe.wait("timeline", |v| v["rows"].as_array().map(|a| a.iter().any(|m| m["body"] == "hi **there**" && m["pending"] == false)).unwrap_or(false));
        let mine = t["rows"].as_array().unwrap().iter().find(|m| m["body"] == "hi **there**").unwrap().clone();
        assert!(mine["html"].as_str().unwrap().contains("<strong>there</strong>"));
        app.call("react", &json!({"event_id": mine["id"], "key": "x"}));
        probe.wait("timeline", |v| v["rows"].as_array().map(|a| a.iter().any(|m| m["reactions"].as_array().map(|r| !r.is_empty()).unwrap_or(false))).unwrap_or(false));
        assert_eq!(app.call("pref", &json!({"key": "nothing"})), json!(""));
        app.call("set_pref", &json!({"key": "k", "value": "v"}));
        assert_eq!(app.call("pref", &json!({"key": "k"})), json!("v"));
        assert_eq!(app.call("account_info", &Value::Null)["user_id"], "@alice:hs");
        assert!(app.call("search_emoji", &json!({"query": "red heart"})).as_array().map(|a| !a.is_empty()).unwrap_or(false));
        assert!(app.call("nonsense", &Value::Null)["error"].is_string());
    }

    #[test]
    fn a_wrong_server_keeps_the_login_screen_and_says_why() {
        let (app, mut probe, _dir) = app();
        app.call("login", &json!({"homeserver": "http://127.0.0.1:1", "user": "alice", "password": "x"}));
        let st = probe.wait("state", |v| v["busy"] == false && v["status"].as_str().map(|s| !s.is_empty() && s != "Signing in...").unwrap_or(false));
        assert_eq!(st["screen"], "login", "{st}");
    }
}
