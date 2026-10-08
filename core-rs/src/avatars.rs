//! Profile pictures: downloaded once into a cache directory, found again by their `mxc://` address.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use matrix_sdk::{media::{MediaFormat, MediaRequestParameters}, Client};

#[derive(Default)]
pub struct Avatars {
    /// mxc address -> the file (None: the server did not give it, do not ask again this session)
    files: Mutex<HashMap<String, Option<PathBuf>>>,
    /// where pictures are kept between runs: a file found there counts as downloaded, so pictures show at once after a restart
    dir: Mutex<Option<PathBuf>>,
}

fn file_name(mxc: &str) -> String { let n: String = mxc.chars().filter(|c| c.is_ascii_alphanumeric()).collect(); format!("{n}.img") }

impl Avatars {
    /// Say where the pictures live on disk (call once the account's folder is known).
    pub fn use_dir(&self, dir: &Path) { *self.dir.lock().unwrap() = Some(dir.to_path_buf()); }

    /// Register a picture that is already on disk from an earlier run.
    fn probe(&self, mxc: &str) {
        let mut files = self.files.lock().unwrap();
        if files.contains_key(mxc) { return; }
        if let Some(d) = self.dir.lock().unwrap().as_ref() {
            let p = d.join(file_name(mxc));
            if p.metadata().map(|m| m.len() > 0).unwrap_or(false) { files.insert(mxc.to_string(), Some(p)); }
        }
    }

    /// The cached file for an address ("" when not downloaded (yet) or the address is empty).
    pub fn path(&self, mxc: &str) -> String {
        if mxc.is_empty() { return String::new(); }
        self.probe(mxc);
        self.files.lock().unwrap().get(mxc).cloned().flatten().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Is there anything left to do for this address?
    pub fn wanted(&self, mxc: &str) -> bool { if mxc.is_empty() { return false; } self.probe(mxc); !self.files.lock().unwrap().contains_key(mxc) }

    /// Download one picture (a small thumbnail is not asked for: avatars are small files already) into `dir`.
    pub async fn fetch(&self, client: &Client, mxc: &str, dir: &Path) {
        let uri = <&matrix_sdk::ruma::MxcUri>::from(mxc);
        let req = MediaRequestParameters { source: matrix_sdk::ruma::events::room::MediaSource::Plain(uri.to_owned()), format: MediaFormat::File };
        let got = match client.media().get_media_content(&req, true).await {
            Ok(bytes) => {
                let _ = std::fs::create_dir_all(dir);
                let path = dir.join(file_name(mxc));
                std::fs::write(&path, bytes).ok().map(|_| path)
            }
            Err(_) => None,
        };
        self.files.lock().unwrap().insert(mxc.to_string(), got);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_on_disk_from_an_earlier_run_is_found_without_downloading() {
        let d = std::env::temp_dir().join(format!("vc_av_{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join(file_name("mxc://hs/x")), b"png").unwrap();
        let a = Avatars::default();
        a.use_dir(&d);
        assert!(!a.wanted("mxc://hs/x"));
        assert!(a.path("mxc://hs/x").ends_with("mxchsx.img"));
        assert!(a.wanted("mxc://hs/y"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_unknown_address_is_wanted_once_and_has_no_path() {
        let a = Avatars::default();
        assert!(a.wanted("mxc://hs/x"));
        assert_eq!(a.path("mxc://hs/x"), "");
        assert!(!a.wanted(""), "no address, nothing to fetch");
    }
}
