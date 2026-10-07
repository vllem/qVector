//! Encryption features on the SDK's own client: verifying this session against another one of ours (emoji comparison), as the UI needs it.
//! One verification at a time. State changes are reported as JSON through a callback, so any front end can draw them.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use matrix_sdk::{
    encryption::verification::{SasState, SasVerification, Verification, VerificationRequest, VerificationRequestState},
    ruma::events::key::verification::request::ToDeviceKeyVerificationRequestEvent,
    Client,
};
use serde_json::{json, Value};

type Report = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Default)]
struct Flow {
    request: Option<VerificationRequest>,
    sas: Option<SasVerification>,
    generation: u64,
}

/// The verification in progress (if any) with this account's other sessions.
#[derive(Clone)]
pub struct Verifier {
    client: Client,
    flow: Arc<Mutex<Flow>>,
    report: Report,
}

fn idle() -> String { json!({"state": "idle"}).to_string() }

impl Verifier {
    /// `report` is called with a JSON state ({"state": "idle|incoming|waiting|emoji|confirmed|done|cancelled", ...}) whenever it changes.
    pub fn new(client: Client, report: impl Fn(String) + Send + Sync + 'static) -> Verifier {
        let v = Verifier { client: client.clone(), flow: Arc::new(Mutex::new(Flow::default())), report: Arc::new(report) };
        /* requests from our other sessions arrive as to-device events during sync */
        let handler = v.clone();
        client.add_event_handler(move |ev: ToDeviceKeyVerificationRequestEvent, c: Client| {
            let v = handler.clone();
            async move {
                let found = c.encryption().get_verification_request(&ev.sender, &ev.content.transaction_id).await;
                if let Some(req) = found { v.adopt(req); }
            }
        });
        /* requests from other people arrive as messages in the direct chat with them */
        let handler = v.clone();
        client.add_event_handler(move |ev: matrix_sdk::ruma::events::room::message::OriginalSyncRoomMessageEvent, c: Client| {
            let v = handler.clone();
            async move {
                if !matches!(ev.content.msgtype, matrix_sdk::ruma::events::room::message::MessageType::VerificationRequest(_)) { return; }
                if c.user_id().map(|u| u == ev.sender).unwrap_or(false) { return; }
                if let Some(req) = c.encryption().get_verification_request(&ev.sender, &ev.event_id).await { v.adopt(req); }
            }
        });
        v
    }

    /// Ask another person to verify each other (in the direct chat with them; the SDK creates it if there is none).
    pub async fn request_user(&self, user_id: &str) -> Result<(), String> {
        let uid = <&matrix_sdk::ruma::UserId>::try_from(user_id).map_err(|e| format!("not a Matrix user id: {e}"))?;
        self.client.encryption().request_user_identity(uid).await.map_err(|e| e.to_string())?;
        let identity = self.client.encryption().get_user_identity(uid).await.map_err(|e| e.to_string())?
            .ok_or("that person has no cross-signing identity (they have not set up encryption)")?;
        let request = identity.request_verification().await.map_err(|e| e.to_string())?;
        self.adopt(request);
        Ok(())
    }

    /// Ask our other sessions to verify this one.
    pub async fn request_own(&self) -> Result<(), String> {
        let me = self.client.user_id().ok_or("not signed in")?.to_owned();
        let identity = self.client.encryption().get_user_identity(&me).await.map_err(|e| e.to_string())?
            .ok_or("this account has no cross-signing identity yet: set it up from another client first")?;
        let request = identity.request_verification().await.map_err(|e| e.to_string())?;
        self.adopt(request);
        Ok(())
    }

    /// Accept an incoming request.
    pub async fn accept(&self) -> Result<(), String> {
        let req = self.flow.lock().unwrap().request.clone().ok_or("nothing to accept")?;
        req.accept().await.map_err(|e| e.to_string())
    }

    /// The emoji match.
    pub async fn confirm(&self) -> Result<(), String> {
        let sas = self.flow.lock().unwrap().sas.clone().ok_or("no emoji comparison is running")?;
        sas.confirm().await.map_err(|e| e.to_string())
    }

    /// Decline, or the emoji do not match.
    pub async fn cancel(&self) -> Result<(), String> {
        let (req, sas) = { let f = self.flow.lock().unwrap(); (f.request.clone(), f.sas.clone()) };
        if let Some(s) = sas { s.cancel().await.map_err(|e| e.to_string())?; } else if let Some(r) = req { r.cancel().await.map_err(|e| e.to_string())?; }
        Ok(())
    }

    /// Forget a finished verification.
    pub fn dismiss(&self) {
        { let mut f = self.flow.lock().unwrap(); let generation = f.generation + 1; *f = Flow { generation, ..Flow::default() }; } /* one lock: taking it twice in one statement deadlocked the Close button */
        (self.report)(idle());
    }

    fn adopt(&self, request: VerificationRequest) {
        let gen = { let mut f = self.flow.lock().unwrap(); f.generation += 1; f.request = Some(request.clone()); f.sas = None; f.generation };
        let me = self.clone();
        tokio::spawn(async move {
            let mut changes = request.changes();
            if matches!(request.state(), VerificationRequestState::Ready { .. }) && request.we_started() { let _ = request.start_sas().await; }
            me.publish_request(&request);
            while let Some(state) = changes.next().await {
                if me.flow.lock().unwrap().generation != gen { return; }
                match state {
                    VerificationRequestState::Ready { .. } if request.we_started() => { let _ = request.start_sas().await; }
                    VerificationRequestState::Transitioned { verification: Verification::SasV1(sas) } => {
                        me.flow.lock().unwrap().sas = Some(sas.clone());
                        let inner = me.clone();
                        tokio::spawn(async move { inner.watch_sas(sas, gen).await });
                        return;
                    }
                    _ => {}
                }
                me.publish_request(&request);
            }
        });
    }

    async fn watch_sas(&self, sas: SasVerification, gen: u64) {
        let mut changes = sas.changes();
        let first = sas.state();
        if matches!(first, SasState::Started { .. }) && !sas.we_started() { let _ = sas.accept().await; } /* it may already have started */
        self.publish_sas(&sas, &first);
        while let Some(state) = changes.next().await {
            if self.flow.lock().unwrap().generation != gen { return; }
            if matches!(state, SasState::Started { .. }) && !sas.we_started() { let _ = sas.accept().await; }
            self.publish_sas(&sas, &state);
        }
    }

    fn publish_request(&self, r: &VerificationRequest) {
        let other = String::new(); /* the request does not name the other session until it moves to the emoji step */
        let user = r.other_user_id().to_string();
        let state = if r.is_done() { "done" } else if r.is_cancelled() { "cancelled" } else {
            match r.state() {
                VerificationRequestState::Requested { .. } => "incoming",
                VerificationRequestState::Done => "done",
                VerificationRequestState::Cancelled(_) => "cancelled",
                _ => "waiting",
            }
        };
        let reason = r.cancel_info().map(|c| c.reason().to_string()).unwrap_or_default();
        (self.report)(json!({"state": state, "other": other, "user": user, "reason": reason, "we_started": r.we_started()}).to_string());
    }

    fn publish_sas(&self, sas: &SasVerification, state: &SasState) {
        let other = sas.other_device().device_id().to_string();
        let user = sas.other_user_id().to_string();
        let v: Value = match state {
            SasState::KeysExchanged { emojis, decimals } => json!({
                "state": "emoji", "other": other, "user": user,
                "emoji": emojis.as_ref().map(|e| e.emojis.iter().map(|x| json!([x.symbol, x.description])).collect::<Vec<_>>()).unwrap_or_default(),
                "decimals": [decimals.0, decimals.1, decimals.2],
            }),
            SasState::Confirmed => json!({"state": "confirmed", "other": other, "user": user}),
            SasState::Done { .. } => {
                /* the signature we just uploaded makes their identity trusted: fetch it so the new state is seen */
                let (client, uid) = (self.client.clone(), sas.other_user_id().to_owned());
                tokio::spawn(async move { let _ = client.encryption().request_user_identity(&uid).await; });
                json!({"state": "done", "other": other, "user": user})
            }
            SasState::Cancelled(info) => json!({"state": "cancelled", "other": other, "user": user, "reason": info.reason()}),
            _ => json!({"state": "waiting", "other": other, "user": user}),
        };
        (self.report)(v.to_string());
    }
}

/// Has this account verified that person (their cross-signing identity)?
pub async fn user_verified(client: &Client, user_id: &str) -> bool {
    let Ok(uid) = <&matrix_sdk::ruma::UserId>::try_from(user_id) else { return false };
    matches!(client.encryption().get_user_identity(uid).await, Ok(Some(i)) if i.is_verified())
}

/// Is this session trusted by the account's cross-signing identity? {"has_identity", "verified"}.
pub async fn session_status(client: &Client) -> Value {
    let verified = match client.encryption().get_own_device().await { Ok(Some(d)) => d.is_cross_signed_by_owner(), _ => false };
    let has_identity = match client.user_id() {
        Some(me) => matches!(client.encryption().get_user_identity(me).await, Ok(Some(_))),
        None => false,
    };
    json!({"has_identity": has_identity, "verified": verified, "recovery": recovery_state(client)["state"]})
}

/// Where this account's secret storage / key backup stands for this session: {"state": "unknown|enabled|disabled|incomplete"}.
/// "incomplete" means the account has a recovery key but this session does not hold its secrets yet: ask the user for the key.
pub fn recovery_state(client: &Client) -> Value {
    use matrix_sdk::encryption::recovery::RecoveryState;
    let s = match client.encryption().recovery().state() {
        RecoveryState::Unknown => "unknown",
        RecoveryState::Enabled => "enabled",
        RecoveryState::Disabled => "disabled",
        RecoveryState::Incomplete => "incomplete",
    };
    json!({"state": s})
}

/// Unlock the account's secrets with the recovery key (or passphrase): cross-signing keys and the backup key come to this session,
/// and old messages are restored from the backup as they are needed.
pub async fn recover(client: &Client, key: &str) -> Result<(), String> {
    client.encryption().recovery().recover(key.trim()).await.map_err(|e| e.to_string())
}

/// Password asked by the server before it accepts new cross-signing keys: the UI shows a password field when `enable_recovery` fails with this.
pub const PASSWORD_REQUIRED: &str = "password_required";

/// Create secret storage and the key backup for an account that has none; returns the new recovery key (show it to the user once).
/// A server that wants the account password for the cross-signing keys makes this fail with `PASSWORD_REQUIRED`; call again with the password.
pub async fn enable_recovery(client: &Client, password: Option<&str>) -> Result<String, String> {
    use matrix_sdk::ruma::api::client::uiaa::{AuthData, MatrixUserIdentifier, Password, UserIdentifier};
    /* the secrets that go into storage include the cross-signing keys, so an account without them gets them first.
       With a password this is the retry after PASSWORD_REQUIRED: the failed first attempt already made keys locally, so
       they are uploaded again (bootstrap_cross_signing, not the "if needed" variant, which would see them and skip). */
    if let Some(pw) = password {
        let me = client.user_id().ok_or("not signed in")?.to_string();
        let session = match client.encryption().bootstrap_cross_signing(None).await {
            Ok(()) => None,
            Err(e) => match e.as_uiaa_response() { Some(info) => Some(info.session.clone()), None => return Err(e.to_string()) },
        };
        if let Some(session) = session {
            let mut auth = Password::new(UserIdentifier::Matrix(MatrixUserIdentifier::new(me)), pw.to_owned());
            auth.session = session;
            client.encryption().bootstrap_cross_signing(Some(AuthData::Password(auth))).await.map_err(|e| friendly_auth(&e))?;
        }
    } else if !client.encryption().cross_signing_status().await.map(|s| s.is_complete()).unwrap_or(false) {
        if let Err(e) = client.encryption().bootstrap_cross_signing_if_needed(None).await {
            return Err(if e.as_uiaa_response().is_some() { PASSWORD_REQUIRED.into() } else { e.to_string() });
        }
    }
    client.encryption().recovery().enable().await.map_err(|e| e.to_string())
}

fn friendly_auth(e: &matrix_sdk::Error) -> String {
    if e.as_uiaa_response().is_some() { "The password was not accepted.".into() } else { e.to_string() }
}

/// The account's sessions as the settings list shows them: [{"id", "name", "current", "verified"}], this session first.
pub async fn session_list(client: &Client) -> Result<Value, String> {
    let me = client.user_id().ok_or("not signed in")?.to_owned();
    client.encryption().request_user_identity(&me).await.map_err(|e| e.to_string())?;
    let devices = client.encryption().get_user_devices(&me).await.map_err(|e| e.to_string())?;
    let mut out: Vec<Value> = devices.devices().map(|d| {
        let current = client.device_id().map(|c| c == d.device_id()).unwrap_or(false);
        json!({
            "id": d.device_id().to_string(),
            "name": d.display_name().unwrap_or("(unnamed)"),
            "current": current,
            /* the SDK always trusts the session it runs on; what matters for it is whether the account vouches for it */
            "verified": if current { d.is_cross_signed_by_owner() } else { d.is_verified() },
        })
    }).collect();
    out.sort_by_key(|d| (d["current"] != true, d["name"].as_str().unwrap_or("").to_lowercase()));
    Ok(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::FakeHs;
    use std::time::Duration;

    async fn second_session(hs: &FakeHs, dir: &std::path::Path) -> Client {
        let client = crate::open_client(&hs.uri(), dir, "pw").await.unwrap();
        client.matrix_auth().login_username("alice", "x").device_id("TWO").initial_device_display_name("second").send().await.unwrap();
        client
    }

    fn last(states: &Arc<Mutex<Vec<Value>>>) -> Value { states.lock().unwrap().last().cloned().unwrap_or_else(|| json!({"state": "idle"})) }

    #[tokio::test]
    async fn two_sessions_of_one_account_verify_each_other_through_the_sdk() {
        if let Ok(f) = std::env::var("VC_TEST_LOG") { let _ = tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new(f)).with_test_writer().try_init(); }
        let hs = FakeHs::start().await;
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let one = crate::session::sign_in(d1.path(), &hs.uri(), "alice", "x", None, "first").await.unwrap();
        one.encryption().bootstrap_cross_signing(None).await.unwrap();
        crate::sync_once(&one).await.unwrap();
        let two = second_session(&hs, d2.path()).await;
        crate::sync_once(&two).await.unwrap();
        let me = two.user_id().unwrap().to_owned();
        two.encryption().request_user_identity(&me).await.unwrap();
        one.encryption().request_user_identity(&me).await.unwrap(); /* a real server announces the new device in device_lists.changed */
        assert_eq!(session_status(&two).await["has_identity"], true, "the second session learned the identity");
        assert_eq!(session_status(&two).await["verified"], false);

        let (s1, s2): (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>) = Default::default();
        let (c1, c2) = (s1.clone(), s2.clone());
        let v1 = Verifier::new(one.clone(), move |s| c1.lock().unwrap().push(serde_json::from_str(&s).unwrap()));
        let v2 = Verifier::new(two.clone(), move |s| c2.lock().unwrap().push(serde_json::from_str(&s).unwrap()));
        v2.request_own().await.unwrap();

        let (mut accepted, mut confirmed) = (false, false);
        let mut emoji = (Value::Null, Value::Null);
        for _ in 0..200 {
            crate::sync_once(&one).await.unwrap();
            crate::sync_once(&two).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let (a, b) = (last(&s1), last(&s2));
            if a["state"] == "incoming" && !accepted { accepted = true; v1.accept().await.unwrap(); }
            if a["state"] == "emoji" && b["state"] == "emoji" && !confirmed {
                confirmed = true;
                emoji = (a["emoji"].clone(), b["emoji"].clone());
                v1.confirm().await.unwrap();
                v2.confirm().await.unwrap();
            }
            if a["state"] == "done" && b["state"] == "done" { break; }
        }
        assert!(accepted, "the first session saw the request: {:?}; second: {:?}; server: {:#?}", last(&s1), s2.lock().unwrap(), hs.log().iter().filter(|l| l.contains("sendToDevice") || l.contains("UNHANDLED")).collect::<Vec<_>>());
        assert_eq!(emoji.0.as_array().map(|e| e.len()), Some(7), "seven emoji were shown; first: {:?}; second: {:?}", s1.lock().unwrap(), s2.lock().unwrap());
        assert_eq!(emoji.0, emoji.1, "both sides show the same emoji");
        assert_eq!(last(&s1)["state"], "done", "{:?}", s1.lock().unwrap());
        assert_eq!(last(&s2)["state"], "done");
        for _ in 0..20 { crate::sync_once(&two).await.unwrap(); two.encryption().request_user_identity(&me).await.unwrap(); tokio::time::sleep(Duration::from_millis(100)).await; if session_status(&two).await["verified"] == true { break; } }
        assert_eq!(session_status(&two).await["verified"], true, "the second session is cross-signed now");
    }

    #[tokio::test]
    async fn a_recovery_key_from_one_session_unlocks_another() {
        use matrix_sdk::encryption::recovery::RecoveryState;
        let hs = FakeHs::start().await;
        let unhandled = || hs.log().into_iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>();
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let one = crate::session::sign_in(d1.path(), &hs.uri(), "alice", "x", None, "first").await.unwrap();
        crate::sync_once(&one).await.unwrap();
        let key = enable_recovery(&one, None).await.unwrap_or_else(|e| panic!("{e}; {:#?}", unhandled()));
        assert!(!key.is_empty());
        for _ in 0..2 { crate::sync_once(&one).await.unwrap(); }
        assert_eq!(recovery_state(&one)["state"], "enabled", "{:#?}", hs.log());

        let two = second_session(&hs, d2.path()).await;
        for _ in 0..3 { crate::sync_once(&two).await.unwrap(); }
        assert_eq!(two.encryption().recovery().state(), RecoveryState::Incomplete, "{:#?}", unhandled());
        assert!(recover(&two, "not a key").await.is_err());
        recover(&two, &key).await.unwrap_or_else(|e| panic!("{e}; {:#?}", unhandled()));
        assert_eq!(recovery_state(&two)["state"], "enabled");
    }

    #[tokio::test]
    async fn a_server_that_wants_the_password_for_cross_signing_gets_it_once_asked() {
        let hs = FakeHs::start().await;
        hs.require_password("hunter2");
        let d = tempfile::tempdir().unwrap();
        let c = crate::session::sign_in(d.path(), &hs.uri(), "alice", "hunter2", None, "first").await.unwrap();
        crate::sync_once(&c).await.unwrap();
        assert_eq!(enable_recovery(&c, None).await.unwrap_err(), PASSWORD_REQUIRED, "{:#?}", hs.log());
        let wrong = enable_recovery(&c, Some("nope")).await.unwrap_err();
        assert_ne!(wrong, PASSWORD_REQUIRED, "a wrong password is an error of its own: {wrong}");
        let key = enable_recovery(&c, Some("hunter2")).await.unwrap_or_else(|e| panic!("{e}; {:#?}", hs.log()));
        assert!(!key.is_empty());
    }

    #[tokio::test]
    async fn the_session_list_has_this_session_first_and_the_other_ones() {
        let hs = FakeHs::start().await;
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let one = crate::session::sign_in(d1.path(), &hs.uri(), "alice", "x", None, "Desk").await.unwrap();
        one.encryption().bootstrap_cross_signing(None).await.unwrap();
        crate::sync_once(&one).await.unwrap();
        let two = second_session(&hs, d2.path()).await;
        crate::sync_once(&two).await.unwrap();
        let list = session_list(&two).await.unwrap();
        let rows = list.as_array().unwrap();
        assert_eq!(rows.len(), 2, "{list}");
        assert_eq!(rows[0]["current"], true, "this session first: {list}");
        assert_eq!(rows[0]["verified"], false);
        assert_eq!(rows[1]["name"], "Desk", "{list}");
        assert_eq!(rows[1]["verified"], false, "the second session does not trust the account yet, so it does not trust the first one either: {list}");
    }

    #[tokio::test]
    async fn alice_and_bob_verify_each_other_in_their_direct_chat() {
        let hs = FakeHs::start().await;
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let alice = crate::session::sign_in(da.path(), &hs.uri(), "alice", "x", None, "A").await.unwrap();
        let bob = crate::session::sign_in(db.path(), &hs.uri(), "bob", "x", None, "B").await.unwrap();
        alice.encryption().bootstrap_cross_signing(None).await.unwrap();
        bob.encryption().bootstrap_cross_signing(None).await.unwrap();
        for _ in 0..2 { crate::sync_once(&alice).await.unwrap(); crate::sync_once(&bob).await.unwrap(); }

        let (s1, s2): (Arc<Mutex<Vec<Value>>>, Arc<Mutex<Vec<Value>>>) = Default::default();
        let (c1, c2) = (s1.clone(), s2.clone());
        let va = Verifier::new(alice.clone(), move |s| c1.lock().unwrap().push(serde_json::from_str(&s).unwrap()));
        let vb = Verifier::new(bob.clone(), move |s| c2.lock().unwrap().push(serde_json::from_str(&s).unwrap()));
        assert!(!user_verified(&alice, "@bob:hs").await);
        va.request_user("@bob:hs").await.unwrap_or_else(|e| panic!("{e}; {:#?}", hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>()));

        let (mut accepted, mut confirmed) = (false, false);
        for _ in 0..200 {
            crate::sync_once(&alice).await.unwrap();
            crate::sync_once(&bob).await.unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            let (a, b) = (last(&s1), last(&s2));
            if b["state"] == "incoming" && !accepted { accepted = true; assert_eq!(b["user"], "@alice:hs"); vb.accept().await.unwrap(); }
            if a["state"] == "emoji" && b["state"] == "emoji" && !confirmed {
                confirmed = true;
                assert_eq!(a["emoji"], b["emoji"], "both sides show the same emoji");
                assert_eq!(a["user"], "@bob:hs");
                va.confirm().await.unwrap();
                vb.confirm().await.unwrap();
            }
            if a["state"] == "done" && b["state"] == "done" { break; }
        }
        assert!(accepted && confirmed, "alice: {:?}\nbob: {:?}\n{:#?}", s1.lock().unwrap(), s2.lock().unwrap(), hs.log().iter().filter(|l| l.contains("UNHANDLED")).collect::<Vec<_>>());
        assert_eq!(last(&s1)["state"], "done", "{:?}", s1.lock().unwrap());
        let bob_id = <&matrix_sdk::ruma::UserId>::try_from("@bob:hs").unwrap();
        for _ in 0..20 { crate::sync_once(&alice).await.unwrap(); alice.encryption().request_user_identity(bob_id).await.unwrap(); if user_verified(&alice, "@bob:hs").await { break; } tokio::time::sleep(Duration::from_millis(100)).await; }
        assert!(user_verified(&alice, "@bob:hs").await, "alice now trusts bob's identity: {:#?}", hs.log().iter().filter(|l| l.contains("signatures") || l.contains("UNHANDLED")).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn closing_a_finished_verification_does_not_hang() {
        let hs = FakeHs::start().await;
        let d = tempfile::tempdir().unwrap();
        let c = crate::session::sign_in(d.path(), &hs.uri(), "alice", "x", None, "A").await.unwrap();
        let seen: Arc<Mutex<Vec<Value>>> = Default::default();
        let sink = seen.clone();
        let v = Verifier::new(c, move |s| sink.lock().unwrap().push(serde_json::from_str(&s).unwrap()));
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || { v.dismiss(); v.dismiss(); let _ = tx.send(()); });
        assert!(rx.recv_timeout(Duration::from_secs(5)).is_ok(), "dismiss deadlocked");
        assert_eq!(last(&seen)["state"], "idle");
    }
}
