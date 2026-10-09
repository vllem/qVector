#![recursion_limit = "512"]
//! vector-core: the Matrix core of Vector, built on matrix-sdk (E2EE through matrix-sdk-crypto / vodozemac).
//!
//! The Qt front end (C++) talks to this crate through a small C interface (`capi`, header `vector_app.h`) on the engine in `app`. Everything
//! Matrix-specific (login, sync, rooms, encryption, backup, verification, media) is matrix-sdk's job; this crate adds
//! what is Vector's own.

use std::path::Path;

use matrix_sdk::{config::SyncSettings, Client};
use matrix_sdk_ui::timeline::RoomExt;

/// Build a client for `homeserver` whose state (including all encryption keys) lives in an encrypted sqlite store under `dir`.
pub async fn open_client(homeserver: &str, dir: &Path, passphrase: &str) -> Result<Client, matrix_sdk::ClientBuildError> {
    Client::builder().homeserver_url(homeserver).sqlite_store(dir, Some(passphrase)).build().await
}

/// Timelines are fed from the event cache, and its redecryptor (which re-reads messages that were unreadable when a room key arrives later: from a
/// backup, a verification, another session) only works when the cache is subscribed AFTER the session exists. Subscribing before the login left it
/// shut down, and old messages stayed unreadable after entering the recovery key.
pub fn start_event_cache(client: &Client) { let _ = client.event_cache().subscribe(); }

/// Password login; `device_name` is what other sessions see in their device list.
pub async fn login_password(client: &Client, user: &str, password: &str, device_name: &str) -> matrix_sdk::Result<()> {
    client.matrix_auth().login_username(user, password).initial_device_display_name(device_name).send().await?;
    start_event_cache(client);
    Ok(())
}

/// One round of /sync.
pub async fn sync_once(client: &Client) -> matrix_sdk::Result<()> {
    client.sync_once(SyncSettings::default().timeout(std::time::Duration::from_millis(0))).await?;
    Ok(())
}

#[derive(Debug, PartialEq)]
pub struct RoomRow { pub id: String, pub name: String }

#[derive(Debug, PartialEq)]
pub struct MessageRow { pub event_id: String, pub sender: String, pub body: String }

pub async fn room_list(client: &Client) -> Vec<RoomRow> {
    let mut out = Vec::new();
    for r in client.rooms() {
        let name = r.display_name().await.map(|n| n.to_string()).unwrap_or_default();
        out.push(RoomRow { id: r.room_id().to_string(), name });
    }
    out
}

/// The messages of a room as the SDK's timeline shows them (decrypted, edits and redactions applied).
pub async fn timeline_messages(client: &Client, room_id: &str) -> Option<Vec<MessageRow>> {
    let room = client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(room_id).ok()?)?;
    let timeline = room.timeline().await.ok()?;
    let _keep_subscribed = timeline.subscribe().await; /* the timeline fills from the event cache in the background */
    let mut out = Vec::new();
    for _ in 0..40 {
        out.clear();
        for item in timeline.items().await.iter() {
            if let Some(ev) = item.as_event() {
                if let Some(msg) = ev.content().as_message() {
                    out.push(MessageRow {
                        event_id: ev.event_id().map(|e| e.to_string()).unwrap_or_default(),
                        sender: ev.sender().to_string(),
                        body: msg.body().to_string(),
                    });
                }
            }
        }
        if !out.is_empty() { break; }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Some(out)
}

#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

#[cfg(test)]
mod stress;

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn login_creates_a_device_and_uploads_its_keys() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/_matrix/client/versions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"versions": ["v1.11"]})))
            .mount(&server).await;
        Mock::given(method("POST")).and(path("/_matrix/client/v3/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "user_id": "@me:hs", "access_token": "tok", "device_id": "DEV1"})))
            .mount(&server).await;
        Mock::given(method("POST")).and(path("/_matrix/client/v3/keys/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"one_time_key_counts": {"signed_curve25519": 50}})))
            .mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let client = open_client(&server.uri(), dir.path(), "pw").await.unwrap();
        login_password(&client, "me", "secret", "Vector desktop").await.unwrap();
        assert_eq!(client.user_id().unwrap().as_str(), "@me:hs");
        assert_eq!(client.device_id().unwrap().as_str(), "DEV1");
        let keys = client.encryption().ed25519_key().await;
        assert!(keys.is_some(), "the machine owns an ed25519 identity key after login");
    }

    async fn mock_with_login() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/_matrix/client/versions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"versions": ["v1.11"]}))).mount(&server).await;
        Mock::given(method("POST")).and(path("/_matrix/client/v3/login"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"user_id": "@me:hs", "access_token": "tok", "device_id": "DEV1"}))).mount(&server).await;
        Mock::given(method("POST")).and(path("/_matrix/client/v3/keys/upload"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"one_time_key_counts": {"signed_curve25519": 50}}))).mount(&server).await;
        server
    }

    #[tokio::test]
    async fn sync_gives_a_room_list_and_a_timeline() {
        let server = mock_with_login().await;
        Mock::given(method("GET")).and(path("/_matrix/client/v3/sync"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "next_batch": "s1",
                "rooms": {"join": {"!a:hs": {
                    "state": {"events": [
                        {"type": "m.room.create", "state_key": "", "sender": "@me:hs", "event_id": "$c", "origin_server_ts": 1, "content": {"creator": "@me:hs"}},
                        {"type": "m.room.name", "state_key": "", "sender": "@me:hs", "event_id": "$n", "origin_server_ts": 2, "content": {"name": "Lobby"}}]},
                    "timeline": {"events": [
                        {"type": "m.room.message", "sender": "@bob:hs", "event_id": "$m1", "origin_server_ts": 3, "content": {"msgtype": "m.text", "body": "hello"}}],
                        "limited": false}}}}
            }))).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let client = open_client(&server.uri(), dir.path(), "pw").await.unwrap();
        login_password(&client, "me", "secret", "Vector desktop").await.unwrap();
        sync_once(&client).await.unwrap();
        assert_eq!(room_list(&client).await, vec![RoomRow { id: "!a:hs".into(), name: "Lobby".into() }]);
        let msgs = timeline_messages(&client, "!a:hs").await.unwrap();
        assert_eq!(msgs, vec![MessageRow { event_id: "$m1".into(), sender: "@bob:hs".into(), body: "hello".into() }]);
    }

    #[tokio::test]
    async fn an_encrypted_message_travels_from_alice_to_bob_and_is_not_plaintext_on_the_wire() {
        use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
        let hs = testkit::FakeHs::start().await;
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let alice = open_client(&hs.uri(), da.path(), "pw").await.unwrap();
        let bob = open_client(&hs.uri(), db.path(), "pw").await.unwrap();
        login_password(&alice, "alice", "x", "Vector desktop").await.unwrap_or_else(|e| panic!("{e:?} {:#?}", hs.log()));
        login_password(&bob, "bob", "x", "Vector desktop").await.unwrap();
        sync_once(&alice).await.unwrap_or_else(|e| panic!("{e:?} {:#?}", hs.log()));
        sync_once(&bob).await.unwrap();
        let room = alice.get_room(<&matrix_sdk::ruma::RoomId>::try_from(testkit::ROOM).unwrap()).expect("alice knows the room");
        room.send(RoomMessageEventContent::text_plain("the secret plan")).await.unwrap();
        sync_once(&bob).await.unwrap();
        sync_once(&bob).await.unwrap();
        let msgs = timeline_messages(&bob, testkit::ROOM).await.unwrap();
        assert_eq!(msgs.len(), 1, "bob decrypted one message: {msgs:?}; server log: {:#?}", hs.log());
        assert_eq!(msgs[0].body, "the secret plan");
        assert_eq!(msgs[0].sender, "@alice:hs");
        let wire = hs.room_events();
        assert_eq!(wire.len(), 1);
        assert_eq!(wire[0]["type"], "m.room.encrypted");
        assert!(!wire[0].to_string().contains("secret plan"), "the server only ever sees ciphertext");
    }
}
pub mod session;
pub mod crypto;
pub mod emoji;
pub mod app;
pub mod avatars;
pub mod capi;
pub mod index;
pub mod bookmarks;
pub mod ui;
pub mod emotes;
pub mod rtc_peer;
pub mod calls;
pub mod group_calls;
pub mod calls_audio;
pub mod video;
pub mod vp8;
pub use ui::{ui_messages, ui_rooms, UiMessage, UiRoom};
