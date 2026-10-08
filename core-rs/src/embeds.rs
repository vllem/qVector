//! YouTube and X (Twitter) links shown as cards: the link is recognised here, and the card (title, channel, post text, picture) is fetched straight from
//! the site's public oEmbed endpoint, never from the pasted URL: every request is built from an id that passed the checks below. The app only does it when
//! the user switched it on (the sites see the address of this computer then, which is why it is an explicit opt-in).

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind { Youtube, Tweet }

/// What a card shows.
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
pub struct UiEmbed {
    /// "youtube" or "tweet"
    pub kind: String,
    /// the video id, or `user/status-number`
    pub id: String,
    /// where a browser should go
    pub url: String,
    pub title: String,
    pub author: String,
    pub text: String,
    pub byline: String,
    /// the downloaded video picture (empty for a post)
    pub thumb_path: String,
}

/// Where the sites are (tests and the demo point these at a fake server).
#[derive(Debug, Clone)]
pub struct Hosts { pub youtube_oembed: String, pub x_oembed: String, pub thumbnails: String }

impl Default for Hosts {
    fn default() -> Self { Hosts { youtube_oembed: "https://www.youtube.com/oembed".into(), x_oembed: "https://publish.x.com/oembed".into(), thumbnails: "https://i.ytimg.com".into() } }
}

impl Hosts {
    /// Everything on one server (the fake one of the demo and the tests).
    pub fn all_on(base: &str) -> Hosts { Hosts { youtube_oembed: format!("{base}/youtube/oembed"), x_oembed: format!("{base}/x/oembed"), thumbnails: format!("{base}/ytimg") } }
}

/// Lower-cased host of an http(s) link and the rest (path, query), or None; links with `user@host` tricks are refused.
fn split_url(url: &str) -> Option<(String, &str)> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let end = rest.find(|c| matches!(c, '/' | '?' | '#')).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') { return None; }
    let host = authority.split(':').next().unwrap_or("");
    if host.is_empty() { return None; }
    Some((host.to_ascii_lowercase(), &rest[end..]))
}

fn host_is(host: &str, domain: &str) -> bool { host == domain || host.strip_suffix(domain).map(|p| p.ends_with('.')).unwrap_or(false) } /* a subdomain, not eviltwitter.com */

fn ok_id(s: &str, digits_only: bool) -> bool {
    !s.is_empty() && s.len() < 64 && s.chars().all(|c| if digits_only { c.is_ascii_digit() } else { c.is_ascii_alphanumeric() || c == '_' || c == '-' })
}

fn segment(s: &str) -> &str { &s[..s.find(|c| matches!(c, '/' | '?' | '#')).unwrap_or(s.len())] }

fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    let q = query.strip_prefix('?')?;
    let q = &q[..q.find('#').unwrap_or(q.len())];
    q.split('&').find_map(|kv| kv.strip_prefix(name).and_then(|r| r.strip_prefix('=')))
}

/// What a link is: a YouTube video or an X post, with its checked id; None for anything else (look-alike hosts included).
pub fn classify(url: &str) -> Option<(Kind, String)> {
    let (host, path) = split_url(url)?;
    let yt = |id: &str| if ok_id(id, false) && id.len() >= 6 { Some((Kind::Youtube, id.to_string())) } else { None };
    if host_is(&host, "youtu.be") { return yt(segment(path.strip_prefix('/')?)); }
    if host_is(&host, "youtube.com") || host_is(&host, "youtube-nocookie.com") {
        if let Some(rest) = path.strip_prefix("/watch") { return yt(query_value(rest, "v")?); }
        for p in ["/shorts/", "/embed/", "/live/"] { if let Some(r) = path.strip_prefix(p) { return yt(segment(r)); } }
        return None;
    }
    if host_is(&host, "twitter.com") || host_is(&host, "x.com") {
        let rest = path.strip_prefix('/')?;
        let (user, after) = rest.split_once('/')?;
        let num = after.strip_prefix("status/")?;
        let num = segment(num);
        if ok_id(user, false) && user.len() <= 30 && ok_id(num, true) && num.len() <= 25 { return Some((Kind::Tweet, format!("{user}/{num}"))); }
    }
    None
}

fn valid_id(kind: Kind, id: &str) -> bool {
    match kind {
        Kind::Youtube => ok_id(id, false),
        Kind::Tweet => id.split_once('/').map(|(u, n)| ok_id(u, false) && ok_id(n, true)).unwrap_or(false),
    }
}

/// The page a browser should open; None for an id that did not pass the checks.
pub fn page_url(kind: Kind, id: &str) -> Option<String> {
    if !valid_id(kind, id) { return None; }
    Some(match kind {
        Kind::Youtube => format!("https://www.youtube.com/watch?v={id}"),
        Kind::Tweet => { let (u, n) = id.split_once('/')?; format!("https://twitter.com/{u}/status/{n}") }
    })
}

/// The oEmbed request for an embed (the page address inside it is escaped: only `:/?=` can occur in it).
pub fn oembed_url(kind: Kind, id: &str, hosts: &Hosts) -> Option<String> {
    let page = page_url(kind, id)?;
    let esc: String = page.chars().map(|c| if ":/?=".contains(c) { format!("%{:02X}", c as u32) } else { c.to_string() }).collect();
    Some(match kind {
        Kind::Youtube => format!("{}?format=json&url={esc}", hosts.youtube_oembed),
        Kind::Tweet => format!("{}?omit_script=true&dnt=true&url={esc}", hosts.x_oembed),
    })
}

pub fn thumbnail_url(id: &str, hosts: &Hosts) -> Option<String> { if valid_id(Kind::Youtube, id) { Some(format!("{}/vi/{id}/hqdefault.jpg", hosts.thumbnails)) } else { None } }

fn decode_entities(s: &str) -> String {
    let mut out = s.to_string();
    for (e, r) in [("&lt;", "<"), ("&gt;", ">"), ("&quot;", "\""), ("&#39;", "'"), ("&apos;", "'"), ("&mdash;", "\u{2014}"), ("&nbsp;", " "), ("&hellip;", "\u{2026}"), ("&amp;", "&")] { out = out.replace(e, r); }
    out
}

/// The text between tags: tags removed, `<br>` a new line, entities decoded.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&rest[..lt]);
        let Some(gt) = rest[lt..].find('>') else { rest = ""; break };
        if rest[lt..].starts_with("<br") { out.push('\n'); }
        rest = &rest[lt + gt + 1..];
    }
    out.push_str(rest);
    decode_entities(&out)
}

/// The readable text of an oEmbed post: its first paragraph.
pub fn tweet_text(html: &str) -> String {
    let Some(p) = html.find("<p") else { return String::new() };
    let Some(gt) = html[p..].find('>') else { return String::new() };
    let body = &html[p + gt + 1..];
    let Some(end) = body.find("</p>") else { return String::new() };
    plain(&body[..end])
}

/// The line the post ends with ("— Name (@handle) October 6, 2026").
pub fn tweet_byline(html: &str) -> String {
    let Some(p) = html.find("</p>") else { return String::new() };
    let rest = &html[p + 4..];
    plain(&rest[..rest.find("</blockquote>").unwrap_or(rest.len())]).trim().to_string()
}

/// Ask the site for the card of a link (and download a video's picture into `cache`). None when the site does not answer.
pub async fn fetch(http: &matrix_sdk::reqwest::Client, kind: Kind, id: &str, cache: &Path, hosts: &Hosts) -> Option<UiEmbed> {
    let url = oembed_url(kind, id, hosts)?;
    let resp = http.get(&url).timeout(std::time::Duration::from_secs(12)).send().await.ok()?;
    if !resp.status().is_success() { return None; }
    let v: Value = serde_json::from_slice(&resp.bytes().await.ok()?).ok()?;
    let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
    let mut e = UiEmbed { kind: match kind { Kind::Youtube => "youtube", Kind::Tweet => "tweet" }.into(), id: id.into(), url: page_url(kind, id)?, author: s("author_name"), ..Default::default() };
    match kind {
        Kind::Youtube => {
            e.title = s("title");
            if let Some(t) = thumbnail_url(id, hosts) {
                if let Ok(r) = http.get(&t).timeout(std::time::Duration::from_secs(12)).send().await {
                    if r.status().is_success() {
                        if let Ok(bytes) = r.bytes().await {
                            let _ = std::fs::create_dir_all(cache);
                            let path = cache.join(format!("yt-{id}.img"));
                            if std::fs::write(&path, bytes).is_ok() { e.thumb_path = path.to_string_lossy().into_owned(); }
                        }
                    }
                }
            }
        }
        Kind::Tweet => { let html = s("html"); e.text = tweet_text(&html); e.byline = tweet_byline(&html); }
    }
    if e.title.is_empty() && e.text.is_empty() { return None; }
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_links_in_every_usual_shape() {
        for u in ["https://www.youtube.com/watch?v=dQw4w9WgXcQ", "https://youtube.com/watch?feature=share&v=dQw4w9WgXcQ&t=42s", "https://youtu.be/dQw4w9WgXcQ?t=3",
                  "https://m.youtube.com/shorts/dQw4w9WgXcQ", "http://www.youtube.com/embed/dQw4w9WgXcQ", "https://WWW.YOUTUBE.COM/watch?v=dQw4w9WgXcQ"] {
            assert_eq!(classify(u), Some((Kind::Youtube, "dQw4w9WgXcQ".to_string())), "{u}");
        }
    }

    #[test]
    fn look_alikes_and_other_pages_are_not_embeds() {
        for u in ["https://evilyoutube.com/watch?v=dQw4w9WgXcQ", "https://youtube.com.evil.org/watch?v=dQw4w9WgXcQ", "https://youtube.com@evil.org/watch?v=dQw4w9WgXcQ",
                  "https://www.youtube.com/channel/UC123", "https://www.youtube.com/watch?v=a", "https://www.youtube.com/watch?v=abc<script>", "ftp://youtu.be/dQw4w9WgXcQ",
                  "https://nottwitter.com/a/status/1", "https://twitter.com/jack", "https://twitter.com/jack/status/abc", ""] {
            assert_eq!(classify(u), None, "{u}");
        }
    }

    #[test]
    fn tweet_links_and_the_requests_built_from_ids() {
        assert_eq!(classify("https://twitter.com/jack/status/20?s=20"), Some((Kind::Tweet, "jack/20".to_string())));
        assert_eq!(classify("https://x.com/some_one/status/1234567890123456789"), Some((Kind::Tweet, "some_one/1234567890123456789".to_string())));
        assert_eq!(classify("https://mobile.twitter.com/jack/status/20"), Some((Kind::Tweet, "jack/20".to_string())));
        let h = Hosts::default();
        assert_eq!(oembed_url(Kind::Tweet, "jack/20", &h).unwrap(), "https://publish.x.com/oembed?omit_script=true&dnt=true&url=https%3A%2F%2Ftwitter.com%2Fjack%2Fstatus%2F20");
        assert_eq!(oembed_url(Kind::Youtube, "dQw4w9WgXcQ", &h).unwrap(), "https://www.youtube.com/oembed?format=json&url=https%3A%2F%2Fwww.youtube.com%2Fwatch%3Fv%3DdQw4w9WgXcQ");
        assert_eq!(thumbnail_url("dQw4w9WgXcQ", &h).unwrap(), "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg");
        assert_eq!(page_url(Kind::Tweet, "jack/20").unwrap(), "https://twitter.com/jack/status/20");
        assert_eq!(oembed_url(Kind::Youtube, "bad id&x=1", &h), None, "never a request from an unchecked id");
        assert_eq!(oembed_url(Kind::Tweet, "jack", &h), None);
    }

    #[test]
    fn the_text_of_a_post_comes_out_of_the_oembed_html() {
        let html = "<blockquote class=\"twitter-tweet\"><p lang=\"en\" dir=\"ltr\">just setting up my twttr<br>line two &amp; more &lt;3 <a href=\"https://t.co/x\">pic.twitter.com/x</a></p>\
                    &mdash; jack (@jack) <a href=\"https://twitter.com/jack/status/20?ref_src=twsrc%5Etfw\">March 21, 2006</a></blockquote>\n";
        assert_eq!(tweet_text(html), "just setting up my twttr\nline two & more <3 pic.twitter.com/x");
        assert_eq!(tweet_byline(html), "\u{2014} jack (@jack) March 21, 2006");
        assert_eq!(tweet_text("<div>no paragraph</div>"), "");
    }

    #[cfg(feature = "testkit")]
    #[tokio::test]
    async fn cards_are_fetched_from_the_oembed_endpoints() {
        let hs = crate::testkit::FakeHs::start().await;
        let hosts = Hosts::all_on(&hs.uri());
        let http = matrix_sdk::reqwest::Client::new();
        let dir = tempfile::tempdir().unwrap();
        let v = fetch(&http, Kind::Youtube, "dQw4w9WgXcQ", dir.path(), &hosts).await.expect("a video card");
        assert_eq!((v.kind.as_str(), v.title.as_str(), v.author.as_str(), v.url.as_str()), ("youtube", "Never Gonna Give You Up (Official Video)", "Rick Astley", "https://www.youtube.com/watch?v=dQw4w9WgXcQ"));
        assert!(std::fs::metadata(&v.thumb_path).map(|m| m.len() > 0).unwrap_or(false), "the picture was downloaded: {v:?}");
        assert!(fetch(&http, Kind::Youtube, "unknownvid1", dir.path(), &hosts).await.is_none(), "the site does not know it");
        let t = fetch(&http, Kind::Tweet, "jack/20", dir.path(), &hosts).await.expect("a post card");
        assert_eq!((t.text.as_str(), t.byline.as_str(), t.author.as_str()), ("just setting up my twttr", "\u{2014} jack (@jack) March 21, 2006", "jack"));
        assert!(fetch(&http, Kind::Tweet, "not valid", dir.path(), &hosts).await.is_none(), "an unchecked id never reaches the network");
    }
}
