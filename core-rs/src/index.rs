//! A local, encrypted index of message text, so search reaches back through encrypted rooms (the server cannot search those).
//! Rows are kept in memory and saved as one file sealed with a `StoreCipher` whose key comes from the session secret; the plain text is
//! never written to disk. Fed from the timelines the app already shows, and from `crawl_room`, which pages back through a room's history.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use matrix_sdk_store_encryption::StoreCipher;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ui::{ui_messages, UiMessage};
use matrix_sdk_ui::timeline::TimelineItem;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRow { pub room_id: String, pub event_id: String, pub sender: String, pub time: String, pub ts: u64, pub body: String }

pub struct MessageIndex {
    file: PathBuf,
    cipher: StoreCipher,
    rows: HashMap<String, IndexRow>,
    dirty: bool,
}

fn key_of(secret: &str) -> [u8; 32] { Sha256::digest(secret.as_bytes()).into() }

/// The cipher for the sealed file `<stem>.enc` in `dir`, kept (itself sealed with the secret) in `<stem>-cipher.bin`. When the secret does not open
/// it, a new cipher replaces it and the old sealed data is dropped.
pub(crate) fn open_cipher(dir: &Path, stem: &str, secret: &str) -> Result<StoreCipher, String> {
    let _ = std::fs::create_dir_all(dir);
    let cipher_file = dir.join(format!("{stem}-cipher.bin"));
    match std::fs::read(&cipher_file).ok().and_then(|b| StoreCipher::import_with_key(&key_of(secret), &b).ok()) {
        Some(c) => Ok(c),
        None => {
            let c = StoreCipher::new().map_err(|e| e.to_string())?;
            std::fs::write(&cipher_file, c.export_with_key(&key_of(secret)).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(dir.join(format!("{stem}.enc"))); /* sealed with the old cipher: unreadable now */
            Ok(c)
        }
    }
}

impl MessageIndex {
    /// Open (or start) the index in `dir`. A file that cannot be opened with this secret is treated as lost and replaced.
    pub fn open(dir: &Path, secret: &str) -> Result<MessageIndex, String> {
        let cipher = open_cipher(dir, "index", secret)?;
        let file = dir.join("index.enc");
        let rows: Vec<IndexRow> = std::fs::read(&file).ok().and_then(|b| cipher.decrypt_value(&b).ok()).unwrap_or_default();
        Ok(MessageIndex { file, cipher, rows: rows.into_iter().map(|r| (format!("{}|{}", r.room_id, r.event_id), r)).collect(), dirty: false })
    }

    pub fn len(&self) -> usize { self.rows.len() }

    /// Add the readable messages of these timeline items (what is not decrypted yet, pending or empty is skipped).
    pub fn add_items(&mut self, room_id: &str, items: &[Arc<TimelineItem>]) -> usize {
        let mut added = 0;
        for m in ui_messages(items, "") {
            if let Some(row) = self.row_of(room_id, &m) {
                let key = format!("{}|{}", row.room_id, row.event_id);
                if self.rows.get(&key) != Some(&row) { self.rows.insert(key, row); self.dirty = true; added += 1; }
            }
        }
        added
    }

    fn row_of(&self, room_id: &str, m: &UiMessage) -> Option<IndexRow> {
        if m.pending || m.id.starts_with('~') || m.body.is_empty() || !matches!(m.kind.as_str(), "text" | "emote" | "notice" | "image" | "video" | "audio" | "file" | "poll") { return None; }
        Some(IndexRow { room_id: room_id.into(), event_id: m.id.clone(), sender: m.sender.clone(), time: m.time.clone(), ts: m.ts, body: m.body.clone() })
    }

    /// Messages containing every word of `query` (case-insensitive), in one room or all, newest first, at most `limit`.
    pub fn search(&self, query: &str, room_id: Option<&str>, limit: usize) -> Vec<IndexRow> {
        let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
        if words.is_empty() { return Vec::new(); }
        let mut hits: Vec<&IndexRow> = self.rows.values().filter(|r| room_id.map(|id| id == r.room_id).unwrap_or(true) && {
            let hay = format!("{} {}", r.body, r.sender).to_lowercase();
            words.iter().all(|w| hay.contains(w))
        }).collect();
        hits.sort_by(|a, b| b.ts.cmp(&a.ts).then(b.event_id.cmp(&a.event_id)));
        hits.into_iter().take(limit).cloned().collect()
    }

    /// Write the index if it changed.
    pub fn save(&mut self) -> Result<(), String> {
        if !self.dirty { return Ok(()); }
        let rows: Vec<&IndexRow> = self.rows.values().collect();
        let sealed = self.cipher.encrypt_value(&rows).map_err(|e| e.to_string())?;
        let tmp = self.file.with_extension("tmp");
        std::fs::write(&tmp, sealed).and_then(|_| std::fs::rename(&tmp, &self.file)).map_err(|e| e.to_string())?;
        self.dirty = false;
        Ok(())
    }

    /// Forget everything of a room (we left it).
    pub fn forget_room(&mut self, room_id: &str) { let before = self.rows.len(); self.rows.retain(|_, r| r.room_id != room_id); if self.rows.len() != before { self.dirty = true; } }
}

/// Page back through a room's history (up to `pages` pages of 30 events) and index what is readable. Uses its own timeline, so the one on
/// screen is not disturbed. Returns true when the start of the room was reached.
pub async fn crawl_room(room: &matrix_sdk::Room, index: &std::sync::Mutex<MessageIndex>, pages: usize) -> Result<bool, String> {
    let timeline = crate::ui::open_timeline(room).await?;
    let room_id = room.room_id().to_string();
    let mut reached = false;
    for _ in 0..pages {
        reached = timeline.paginate_backwards(30).await.map_err(|e| e.to_string())?;
        let items: Vec<_> = timeline.items().await.iter().cloned().collect();
        index.lock().map_err(|_| "index lock poisoned")?.add_items(&room_id, &items);
        if reached { break; }
    }
    Ok(reached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{FakeHs, ROOM};
    use matrix_sdk_ui::timeline::RoomExt;

    async fn client_with_room(hs: &FakeHs, dir: &Path) -> (matrix_sdk::Client, matrix_sdk::Room) {
        let client = crate::session::sign_in(dir, &hs.uri(), "alice", "pw", None, "Vector").await.unwrap();
        crate::sync_once(&client).await.unwrap();
        let room = client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        (client, room)
    }

    #[tokio::test]
    async fn the_index_finds_old_encrypted_history_and_keeps_its_text_off_the_disk() {
        let hs = FakeHs::start().await;
        hs.set_history(6);
        let dir = tempfile::tempdir().unwrap();
        let (client, room) = client_with_room(&hs, dir.path()).await;
        let timeline = room.timeline().await.unwrap();
        let _ = timeline.subscribe().await;
        crate::ui::send_text(&timeline, "the quick brown fox", None).await.unwrap();
        for _ in 0..80 { crate::sync_once(&client).await.unwrap(); if timeline.items().await.iter().any(|i| i.as_event().map(|e| e.event_id().is_some()).unwrap_or(false)) { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }

        let idx_dir = dir.path().join("index");
        let index = std::sync::Mutex::new(MessageIndex::open(&idx_dir, "secret").unwrap());
        let mut reached = false;
        for _ in 0..3 { if crawl_room(&room, &index, 1).await.unwrap() { reached = true; break; } }
        assert!(reached);
        {
            let items: Vec<_> = timeline.items().await.iter().cloned().collect();
            index.lock().unwrap().add_items(ROOM, &items);
        }
        let i = index.lock().unwrap();
        assert_eq!(i.search("old message 3", None, 10).len(), 1, "history from /messages is searchable");
        assert_eq!(i.search("QUICK fox", Some(ROOM), 10).len(), 1, "the message we sent is too");
        assert!(i.search("quick", Some("!other:hs"), 10).is_empty(), "other rooms are separate");
        assert!(i.len() >= 7, "{}", i.len());
        drop(i);

        index.lock().unwrap().save().unwrap();
        let sealed = std::fs::read(idx_dir.join("index.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("quick brown fox"), "the file is sealed");
        let again = MessageIndex::open(&idx_dir, "secret").unwrap();
        assert_eq!(again.search("quick brown", None, 10).len(), 1, "it comes back with the same secret");
        let other = MessageIndex::open(&idx_dir, "wrong secret").unwrap();
        assert_eq!(other.len(), 0, "a wrong secret opens an empty index, not the old text");
    }
}
