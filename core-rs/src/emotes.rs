//! Custom emoji and stickers (the `im.ponies.*` packs of MSC2545): reading the packs of a room, of the user and of the rooms the user
//! chose, putting `:shortcode:` pictures into outgoing messages and finding the pictures in incoming ones.

use std::collections::{HashMap, HashSet};

use matrix_sdk::{ruma::events::{GlobalAccountDataEventType, StateEventType}, Client};
use matrix_sdk::deserialized_responses::RawAnySyncOrStrippedState;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiEmote {
    pub shortcode: String,
    pub mxc: String,
    /// the downloaded picture (empty until the app has fetched it)
    pub path: String,
    /// usable as an emoji inside a message / as a sticker of its own (a pack that does not say is both)
    pub emoji: bool,
    pub sticker: bool,
    pub width: u32,
    pub height: u32,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiPack {
    /// where it comes from: "room", "user" or the title of another room
    pub source: String,
    pub name: String,
    pub emotes: Vec<UiEmote>,
}

/// The pack in the content of an `im.ponies.room_emotes` event / the user's `im.ponies.user_emotes` data. Pictures without a usable `mxc://`
/// address and shortcodes with odd characters are dropped.
pub fn parse_pack(content: &Value, source: &str, fallback_name: &str) -> UiPack {
    let pack_usage = |v: &Value| -> (bool, bool) {
        match v["usage"].as_array() {
            Some(u) if !u.is_empty() => (u.iter().any(|x| x == "emoticon"), u.iter().any(|x| x == "sticker")),
            _ => (true, true),
        }
    };
    let (pack_emoji, pack_sticker) = pack_usage(&content["pack"]);
    let name = content["pack"]["display_name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(fallback_name).to_string();
    let mut emotes: Vec<UiEmote> = content["images"].as_object().map(|m| m.iter().filter_map(|(code, img)| {
        let mxc = img["url"].as_str().filter(|u| u.starts_with("mxc://") && !u.contains('"') && !u.contains(' '))?;
        if code.is_empty() || !code.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '+')) { return None; }
        let (e, s) = if img["usage"].is_array() { pack_usage(img) } else { (pack_emoji, pack_sticker) };
        let num = |k: &str| img["info"][k].as_u64().unwrap_or(0) as u32;
        Some(UiEmote { shortcode: code.clone(), mxc: mxc.to_string(), emoji: e, sticker: s, width: num("w"), height: num("h"), mime: img["info"]["mimetype"].as_str().unwrap_or("").to_string(), ..Default::default() })
    }).collect()).unwrap_or_default();
    emotes.sort_by(|a, b| a.shortcode.to_lowercase().cmp(&b.shortcode.to_lowercase()));
    UiPack { source: source.into(), name, emotes }
}

fn state_json(ev: &RawAnySyncOrStrippedState) -> Option<Value> {
    let raw = match ev { RawAnySyncOrStrippedState::Sync(r) => r.json().get(), RawAnySyncOrStrippedState::Stripped(r) => r.json().get() };
    serde_json::from_str(raw).ok()
}

/// The packs that apply in a room: its own, the user's, and those of the rooms listed in the user's `im.ponies.emote_rooms`.
pub async fn emote_packs(client: &Client, room_id: &str) -> Vec<UiPack> {
    let ty = StateEventType::from("im.ponies.room_emotes");
    let mut packs = Vec::new();
    let rid = <&matrix_sdk::ruma::RoomId>::try_from(room_id).ok();
    if let Some(room) = rid.and_then(|r| client.get_room(r)) {
        for ev in room.get_state_events(ty.clone()).await.unwrap_or_default() {
            let Some(v) = state_json(&ev) else { continue };
            let p = parse_pack(&v["content"], "room", v["state_key"].as_str().filter(|s| !s.is_empty()).unwrap_or("Room emoji"));
            if !p.emotes.is_empty() { packs.push(p); }
        }
    }
    let account_json = |t: &str| {
        let client = client.clone();
        let t = t.to_string();
        async move {
            let raw = client.account().account_data_raw(GlobalAccountDataEventType::from(t.as_str())).await.ok().flatten()?;
            serde_json::from_str::<Value>(raw.json().get()).ok()
        }
    };
    if let Some(v) = account_json("im.ponies.user_emotes").await {
        let p = parse_pack(&v, "user", "My emoji");
        if !p.emotes.is_empty() { packs.push(p); }
    }
    if let Some(v) = account_json("im.ponies.emote_rooms").await {
        for (other, keys) in v["rooms"].as_object().into_iter().flatten() {
            if Some(other.as_str()) == rid.map(|r| r.as_str()) { continue; }
            let Some(room) = <&matrix_sdk::ruma::RoomId>::try_from(other.as_str()).ok().and_then(|r| client.get_room(r)) else { continue };
            let title = room.cached_display_name().map(|n| n.to_string()).unwrap_or_else(|| other.clone());
            for key in keys.as_object().into_iter().flat_map(|k| k.keys()) {
                let Ok(Some(ev)) = room.get_state_event(ty.clone(), key).await else { continue };
                let Some(v) = state_json(&ev) else { continue };
                let p = parse_pack(&v["content"], &title, if key.is_empty() { &title } else { key });
                if !p.emotes.is_empty() { packs.push(p); }
            }
        }
    }
    packs
}

/// shortcode -> address of the pictures that can be used as emoji in a message (the first pack that has a shortcode wins).
pub fn emoji_map(packs: &[UiPack]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for p in packs { for e in p.emotes.iter().filter(|e| e.emoji) { m.entry(e.shortcode.clone()).or_insert_with(|| e.mxc.clone()); } }
    m
}

fn escape(s: &str) -> String { s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;") }

/// Put the pictures for known `:shortcode:`s into the HTML of a message. `None` when the text uses none of them (the message then goes out as it was).
pub fn with_custom_emoji(text: &str, html: Option<&str>, emoji: &HashMap<String, String>) -> Option<String> {
    let used: Vec<(&String, &String)> = emoji.iter().filter(|(c, _)| text.contains(&format!(":{c}:"))).collect();
    if used.is_empty() { return None; }
    let mut out = match html { Some(h) => h.to_string(), None => escape(text).replace('\n', "<br>") };
    let mut used = used;
    used.sort_by(|a, b| b.0.len().cmp(&a.0.len())); /* longer names first, so `:a_b:` is not eaten by `:a:` */
    for (code, mxc) in used {
        let tag = format!("<img data-mx-emoticon height=\"32\" src=\"{}\" alt=\":{c}:\" title=\":{c}:\" />", escape(mxc), c = escape(code));
        out = out.replace(&format!(":{code}:"), &tag);
    }
    Some(out)
}

/// The `mxc://` pictures an incoming message shows inline (custom emoji), each once, in order.
pub fn emoji_sources(html: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(i) = rest.find("src=\"mxc://") {
        let after = &rest[i + 5..];
        let end = after.find('"').unwrap_or(after.len());
        let mxc = &after[..end];
        if seen.insert(mxc.to_string()) { out.push(mxc.to_string()); }
        rest = &after[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_pack_keeps_good_pictures_and_reads_usage_per_picture_or_per_pack() {
        let p = parse_pack(&json!({
            "pack": {"display_name": "Cats", "usage": ["emoticon"]},
            "images": {
                "cat": {"url": "mxc://hs/cat"},
                "wave": {"url": "mxc://hs/wave", "usage": ["sticker"], "info": {"w": 128, "h": 96, "mimetype": "image/png"}},
                "bad code!": {"url": "mxc://hs/x"},
                "web": {"url": "https://example.org/x.png"},
            }
        }), "room", "x");
        assert_eq!(p.name, "Cats");
        let got: Vec<(&str, bool, bool)> = p.emotes.iter().map(|e| (e.shortcode.as_str(), e.emoji, e.sticker)).collect();
        assert_eq!(got, vec![("cat", true, false), ("wave", false, true)], "{p:#?}");
        assert_eq!((p.emotes[1].width, p.emotes[1].height, p.emotes[1].mime.as_str()), (128, 96, "image/png"));
        assert_eq!(parse_pack(&json!({"images": {"a": {"url": "mxc://h/a"}}}), "user", "My emoji").emotes[0].emoji, true, "no usage: both");
        assert_eq!(parse_pack(&json!({"images": {"a": {"url": "mxc://h/a"}}}), "user", "My emoji").name, "My emoji");
    }

    #[test]
    fn shortcodes_become_pictures_only_when_known_and_markup_is_escaped() {
        let m: HashMap<String, String> = [("cat".to_string(), "mxc://hs/cat".to_string()), ("cat_2".to_string(), "mxc://hs/c2".to_string())].into();
        assert_eq!(with_custom_emoji("no emoji <here> :dog:", None, &m), None);
        let h = with_custom_emoji("hi <b> :cat: & :cat_2:", None, &m).unwrap();
        assert!(h.starts_with("hi &lt;b&gt; <img data-mx-emoticon") && h.contains("src=\"mxc://hs/cat\"") && h.contains("src=\"mxc://hs/c2\"") && h.contains("&amp;"), "{h}");
        assert_eq!(h.matches(":cat:").count(), 2, "only the alt and title text of the picture keep the shortcode: {h}");
        let h = with_custom_emoji("**x** :cat:", Some("<strong>x</strong> :cat:"), &m).unwrap();
        assert!(h.starts_with("<strong>x</strong> <img"), "{h}");
        assert_eq!(emoji_sources(&h), vec!["mxc://hs/cat".to_string()]);
        assert!(emoji_sources("<img src=\"https://x/y.png\"> plain").is_empty());
        assert_eq!(emoji_sources("<img src=\"mxc://a/b\"><img src=\"mxc://a/b\"><img src=\"mxc://a/c\">"), vec!["mxc://a/b".to_string(), "mxc://a/c".to_string()]);
    }
}
