//! Emoji for pickers: search by name over the Unicode list (`scripts/gen_emoji_json.py` makes `emoji.json`; do not edit it).

use std::sync::OnceLock;

fn all() -> &'static Vec<(String, String, String)> {
    static ALL: OnceLock<Vec<(String, String, String)>> = OnceLock::new();
    ALL.get_or_init(|| serde_json::from_str(include_str!("emoji.json")).unwrap_or_default())
}

/// Emoji whose name contains every word of `query` (case-insensitive), at most `limit`; an empty query gives the first ones (faces).
/// Each result is (glyph, name).
pub fn search(query: &str, limit: usize) -> Vec<(String, String)> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    all().iter().filter(|(_, name, _)| { let n = name.to_lowercase(); words.iter().all(|w| n.contains(w)) })
        .take(limit).map(|(g, n, _)| (g.clone(), n.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_searched_by_all_words() {
        assert!(all().len() > 1500);
        assert_eq!(search("", 10).len(), 10);
        assert!(search("thumbs up", 5).iter().any(|(g, _)| g == "\u{1F44D}"));
        let r = search("red heart", 20);
        assert!(r.iter().all(|(_, n)| n.contains("red") && n.contains("heart")) && !r.is_empty(), "{r:?}");
        assert!(search("no such emoji name", 5).is_empty());
    }
}
