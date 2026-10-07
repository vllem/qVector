//! Signing in, and remembering the session: the login (tokens, device id) is kept in an encrypted file next to the SDK's own sqlite store
//! (which holds the encryption keys). Both are protected by one secret: either a random key kept in a key file (no questions at start-up) or a
//! passphrase the user types at every start.

use std::path::{Path, PathBuf};

use matrix_sdk::{authentication::matrix::MatrixSession, store::RoomLoadSettings, Client};
use matrix_sdk_store_encryption::StoreCipher;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Saved { homeserver: String, session: MatrixSession }

/// How the saved session is protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection { KeyFile, Passphrase }

fn store_dir(dir: &Path) -> PathBuf { dir.join("store") }
fn cipher_file(dir: &Path) -> PathBuf { dir.join("cipher.bin") }
fn session_file(dir: &Path) -> PathBuf { dir.join("session.enc") }
fn key_file(dir: &Path) -> PathBuf { dir.join("key") }
fn mode_file(dir: &Path) -> PathBuf { dir.join("mode") }

/// Is there a session to restore?
pub fn has_saved_session(dir: &Path) -> bool { session_file(dir).exists() && cipher_file(dir).exists() }

/// How the saved session is protected (None when there is none).
pub fn protection(dir: &Path) -> Option<Protection> {
    if !has_saved_session(dir) { return None; }
    Some(if std::fs::read_to_string(mode_file(dir)).map(|m| m.trim() == "passphrase").unwrap_or(false) { Protection::Passphrase } else { Protection::KeyFile })
}

/// Delete everything saved (sign out).
pub fn forget(dir: &Path) { let _ = std::fs::remove_dir_all(dir); }

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
    std::fs::write(path, data)?;
    #[cfg(unix)]
    { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?; }
    Ok(())
}

/// The secret for a session saved without a passphrase: a random key in a file only the user can read.
fn key_secret(dir: &Path, create: bool) -> Result<String, String> {
    if let Ok(k) = std::fs::read_to_string(key_file(dir)) { return Ok(k.trim().to_string()); }
    if !create { return Err("the key file is missing".into()); }
    use rand::RngCore;
    let mut raw = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut raw);
    let secret = base64::Engine::encode(&base64::engine::general_purpose::STANDARD_NO_PAD, raw);
    write_private(&key_file(dir), secret.as_bytes()).map_err(|e| e.to_string())?;
    Ok(secret)
}

/// The secret to open a saved session with when no passphrase is needed (reads the key file).
pub fn saved_key_secret(dir: &Path) -> Result<String, String> { key_secret(dir, false) }

async fn build_client(homeserver: &str, dir: &Path, secret: &str) -> Result<Client, String> {
    let client = Client::builder()
        .server_name_or_homeserver_url(homeserver.trim())
        .sqlite_store(store_dir(dir), Some(secret))
        .build()
        .await
        .map_err(|e| format!("cannot reach {homeserver}: {e}"))?;
    Ok(client) /* the event cache is subscribed once a session exists (see `start_event_cache`) */
}

fn save(dir: &Path, secret: &str, protection: Protection, homeserver: &str, session: MatrixSession) -> Result<(), String> {
    let cipher = StoreCipher::new().map_err(|e| e.to_string())?;
    let exported = match protection {
        Protection::KeyFile => { /* a random 32-byte secret: no stretching needed */
            let key = sha256(secret.as_bytes());
            cipher.export_with_key(&key).map_err(|e| e.to_string())?
        }
        Protection::Passphrase => cipher.export(secret).map_err(|e| e.to_string())?,
    };
    let blob = cipher.encrypt_value(&Saved { homeserver: homeserver.to_string(), session }).map_err(|e| e.to_string())?;
    write_private(&cipher_file(dir), &exported).map_err(|e| e.to_string())?;
    write_private(&session_file(dir), &blob).map_err(|e| e.to_string())?;
    write_private(&mode_file(dir), if protection == Protection::Passphrase { b"passphrase" } else { b"key" }).map_err(|e| e.to_string())
}

fn sha256(b: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(b).into()
}

/// An error for a person: what went wrong with the sign-in, in plain words.
pub fn friendly(e: &matrix_sdk::Error) -> String {
    use matrix_sdk::ruma::api::error::ErrorKind;
    match e.client_api_error_kind() {
        Some(ErrorKind::Forbidden { .. }) => "Wrong user name or password.".into(),
        Some(ErrorKind::UserDeactivated) => "This account has been deactivated.".into(),
        Some(ErrorKind::LimitExceeded { .. }) => "Too many attempts: wait a moment and try again.".into(),
        Some(ErrorKind::Unrecognized) => "This server does not offer password sign-in (single sign-on only?).".into(),
        _ => format!("Could not sign in: {e}"),
    }
}

/// Sign in with a password on a fresh store; the session is saved for later. `homeserver` may be a server name (looked up) or a URL.
pub async fn sign_in(dir: &Path, homeserver: &str, user: &str, password: &str, passphrase: Option<&str>, device_name: &str) -> Result<Client, String> {
    forget(dir);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (protection, secret) = match passphrase.filter(|p| !p.is_empty()) {
        Some(p) => (Protection::Passphrase, p.to_string()),
        None => (Protection::KeyFile, key_secret(dir, true)?),
    };
    let result = async {
        let client = build_client(homeserver, dir, &secret).await?;
        client.matrix_auth().login_username(user, password).initial_device_display_name(device_name).send().await.map_err(|e| friendly(&e))?;
        let session = client.matrix_auth().session().ok_or("the server did not give a session")?;
        save(dir, &secret, protection, homeserver, session)?;
        crate::start_event_cache(&client);
        Ok::<Client, String>(client)
    }.await;
    if result.is_err() { forget(dir); }
    result
}

/// Open the saved session. `secret` is the passphrase (or, for a key-file session, the key from [`saved_key_secret`]).
pub async fn restore(dir: &Path, secret: &str) -> Result<Client, String> {
    let prot = protection(dir).ok_or("no saved session")?;
    let exported = std::fs::read(cipher_file(dir)).map_err(|e| e.to_string())?;
    let cipher = match prot {
        Protection::KeyFile => StoreCipher::import_with_key(&sha256(secret.as_bytes()), &exported),
        Protection::Passphrase => StoreCipher::import(secret, &exported),
    }.map_err(|_| if prot == Protection::Passphrase { "wrong passphrase".to_string() } else { "the saved key does not match".to_string() })?;
    let blob = std::fs::read(session_file(dir)).map_err(|e| e.to_string())?;
    let saved: Saved = cipher.decrypt_value(&blob).map_err(|e| format!("the saved session is unreadable: {e}"))?;
    let client = build_client(&saved.homeserver, dir, secret).await?;
    client.matrix_auth().restore_session(saved.session, RoomLoadSettings::default()).await.map_err(|e| e.to_string())?;
    crate::start_event_cache(&client);
    Ok(client)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::FakeHs;

    #[tokio::test]
    async fn a_signed_in_session_is_saved_encrypted_and_comes_back_with_the_same_device() {
        let hs = FakeHs::start().await;
        let dir = tempfile::tempdir().unwrap();
        let client = sign_in(dir.path(), &hs.uri(), "alice", "pw", None, "Vector").await.unwrap();
        let device = client.device_id().unwrap().to_string();
        assert!(has_saved_session(dir.path()));
        assert_eq!(protection(dir.path()), Some(Protection::KeyFile));
        /* nothing readable on disk: no access token, no user name */
        for f in ["session.enc", "cipher.bin"] {
            let bytes = std::fs::read(dir.path().join(f)).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            assert!(!text.contains("tok_alice") && !text.contains("@alice"), "{f} must not hold the session in clear");
        }
        #[cfg(unix)]
        { use std::os::unix::fs::PermissionsExt; assert_eq!(std::fs::metadata(dir.path().join("key")).unwrap().permissions().mode() & 0o777, 0o600); }
        drop(client);
        let secret = saved_key_secret(dir.path()).unwrap();
        let again = restore(dir.path(), &secret).await.unwrap();
        assert_eq!(again.user_id().unwrap().as_str(), "@alice:hs");
        assert_eq!(again.device_id().unwrap().as_str(), device, "the same device: its encryption keys are still in the store");
        crate::sync_once(&again).await.unwrap();
        assert_eq!(crate::room_list(&again).await.len(), 1, "the restored session syncs");
    }

    #[tokio::test]
    async fn a_passphrase_protects_the_session_and_a_wrong_one_is_refused() {
        let hs = FakeHs::start().await;
        let dir = tempfile::tempdir().unwrap();
        drop(sign_in(dir.path(), &hs.uri(), "alice", "pw", Some("correct horse"), "Vector").await.unwrap());
        assert_eq!(protection(dir.path()), Some(Protection::Passphrase));
        assert!(!dir.path().join("key").exists(), "no key file when a passphrase is used");
        assert_eq!(restore(dir.path(), "battery staple").await.err().as_deref(), Some("wrong passphrase"));
        assert!(restore(dir.path(), "correct horse").await.is_ok());
    }

    #[tokio::test]
    async fn signing_out_and_a_failed_sign_in_leave_nothing_behind() {
        let hs = FakeHs::start().await;
        let dir = tempfile::tempdir().unwrap();
        let bad = sign_in(dir.path(), "http://127.0.0.1:1", "alice", "pw", None, "Vector").await;
        assert!(bad.is_err());
        assert!(!has_saved_session(dir.path()), "a failed sign-in saves nothing");
        drop(sign_in(dir.path(), &hs.uri(), "alice", "pw", None, "Vector").await.unwrap());
        assert!(has_saved_session(dir.path()));
        forget(dir.path());
        assert!(!has_saved_session(dir.path()) && !dir.path().join("key").exists());
    }
}
