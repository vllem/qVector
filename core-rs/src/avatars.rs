//! Profile pictures: downloaded once into a cache directory, found again by their `mxc://` address.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use matrix_sdk::{media::{MediaFormat, MediaRequestParameters}, Client};

#[derive(Default)]
pub struct Avatars {
    /// mxc address -> the file (None: the server did not give it, do not ask again this session)
    files: Mutex<HashMap<String, Option<PathBuf>>>,
}

impl Avatars {
    /// The cached file for an address ("" when not downloaded (yet) or the address is empty).
    pub fn path(&self, mxc: &str) -> String {
        if mxc.is_empty() { return String::new(); }
        self.files.lock().unwrap().get(mxc).cloned().flatten().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Is there anything left to do for this address?
    pub fn wanted(&self, mxc: &str) -> bool { !mxc.is_empty() && !self.files.lock().unwrap().contains_key(mxc) }

    /// Download one picture (a small thumbnail is not asked for: avatars are small files already) into `dir`.
    pub async fn fetch(&self, client: &Client, mxc: &str, dir: &Path) {
        let uri = <&matrix_sdk::ruma::MxcUri>::from(mxc);
        let req = MediaRequestParameters { source: matrix_sdk::ruma::events::room::MediaSource::Plain(uri.to_owned()), format: MediaFormat::File };
        let got = match client.media().get_media_content(&req, true).await {
            Ok(bytes) => {
                let _ = std::fs::create_dir_all(dir);
                let name: String = mxc.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                let path = dir.join(format!("{name}.img"));
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
    fn an_unknown_address_is_wanted_once_and_has_no_path() {
        let a = Avatars::default();
        assert!(a.wanted("mxc://hs/x"));
        assert_eq!(a.path("mxc://hs/x"), "");
        assert!(!a.wanted(""), "no address, nothing to fetch");
    }
}
