//! Bookmarks: messages the user saved for later, kept locally in one sealed file (like the message index; the text is never on disk in the clear).

use std::path::{Path, PathBuf};

use matrix_sdk_store_encryption::StoreCipher;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark { pub room_id: String, pub event_id: String, pub sender: String, pub time: String, pub body: String }

pub struct Bookmarks { file: PathBuf, cipher: StoreCipher, list: Vec<Bookmark> }

impl Bookmarks {
    pub fn open(dir: &Path, secret: &str) -> Result<Bookmarks, String> {
        let cipher = crate::index::open_cipher(dir, "bookmarks", secret)?;
        let file = dir.join("bookmarks.enc");
        let list: Vec<Bookmark> = std::fs::read(&file).ok().and_then(|b| cipher.decrypt_value(&b).ok()).unwrap_or_default();
        Ok(Bookmarks { file, cipher, list })
    }

    pub fn contains(&self, event_id: &str) -> bool { self.list.iter().any(|b| b.event_id == event_id) }

    /// Newest saved first.
    pub fn list(&self) -> Vec<Bookmark> { self.list.iter().rev().cloned().collect() }

    /// Remove a saved message (true if it was there).
    pub fn remove(&mut self, event_id: &str) -> Result<bool, String> {
        let Some(i) = self.list.iter().position(|x| x.event_id == event_id) else { return Ok(false) };
        let b = self.list.remove(i);
        self.toggle_write().map(|_| { drop(b); true })
    }

    fn toggle_write(&self) -> Result<(), String> {
        let sealed = self.cipher.encrypt_value(&self.list).map_err(|e| e.to_string())?;
        let tmp = self.file.with_extension("tmp");
        std::fs::write(&tmp, sealed).and_then(|_| std::fs::rename(&tmp, &self.file)).map_err(|e| e.to_string())
    }

    /// Save a message (or remove it when it is already saved); returns true when it is saved afterwards. Written at once.
    pub fn toggle(&mut self, b: Bookmark) -> Result<bool, String> {
        let saved = if let Some(i) = self.list.iter().position(|x| x.event_id == b.event_id) { self.list.remove(i); false } else { self.list.push(b); true };
        self.toggle_write()?;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bm(id: &str, text: &str) -> Bookmark { Bookmark { room_id: "!r:hs".into(), event_id: id.into(), sender: "bob".into(), time: "10:00".into(), body: text.into() } }

    #[test]
    fn bookmarks_toggle_persist_sealed_and_need_the_right_secret() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = Bookmarks::open(dir.path(), "s3cret").unwrap();
        assert!(b.toggle(bm("$1", "remember the milk")).unwrap());
        assert!(b.toggle(bm("$2", "second")).unwrap());
        assert_eq!(b.list().iter().map(|x| x.event_id.as_str()).collect::<Vec<_>>(), vec!["$2", "$1"], "newest first");
        assert!(!b.toggle(bm("$2", "second")).unwrap(), "toggling again removes it");
        assert!(!std::fs::read(dir.path().join("bookmarks.enc")).unwrap().windows(8).any(|w| w == b"the milk"), "sealed");
        assert!(b.remove("$1").unwrap() && !b.remove("$1").unwrap());
        assert!(b.toggle(bm("$1", "remember the milk")).unwrap());
        let again = Bookmarks::open(dir.path(), "s3cret").unwrap();
        assert!(again.contains("$1") && !again.contains("$2"));
        assert!(Bookmarks::open(dir.path(), "other").unwrap().list().is_empty());
    }
}
