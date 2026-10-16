//! A local, encrypted index of message text, so search reaches back through encrypted rooms (the server cannot search those).
//!
//! In memory the rows are packed: one text arena for the bodies and one for the event ids, 32 bytes of numbers per message, rooms and senders named once
//! (about 150 bytes a message; a row of strings in a map was 540). On disk the index is a log of sealed segment files (`seg-000001.enc`, ...), each
//! holding the rows that were new or changed since the one before, so a save costs what changed and not the whole index (it sealed all of it every
//! twenty seconds, three times its size in memory at a time); many segments are written again as a few big ones. The plain text is never written to
//! disk. Fed from the timelines the app already shows, and from `crawl_room`, which pages back through a room's history.

use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use matrix_sdk_store_encryption::StoreCipher;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ui::{ui_messages, UiMessage};
use matrix_sdk_ui::timeline::TimelineItem;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexRow { pub room_id: String, pub event_id: String, pub sender: String, pub time: String, pub ts: u64, pub body: String }

/// Case-insensitive substring test without allocating; `needle` is already lowercase. Non-ASCII needles take the slow path.
fn contains_ci(hay: &str, needle: &str) -> bool { contains_ci_at(hay, needle, rarest_byte(needle)) }

/// Where in `needle` its rarest byte is: a search works it out once per word and not once per message.
fn rarest_byte(needle: &str) -> usize {
    needle.bytes().enumerate().min_by_key(|&(_, b)| byte_rank(b)).map_or(0, |(j, _)| j)
}

/// `contains_ci` with `k` = `rarest_byte(needle)`.
fn contains_ci_at(hay: &str, needle: &str, k: usize) -> bool {
    if !needle.is_ascii() { return hay.to_lowercase().contains(needle); }
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() { return true; }
    if n.len() > h.len() { return false; }
    // Scan for the needle's rarest byte rather than its first: "the" starts with a common letter and would stop at nearly every word.
    let (lo, up) = (n[k].to_ascii_lowercase(), n[k].to_ascii_uppercase());
    let mut i = 0; // where the needle would start
    let last = h.len() - n.len();
    while i <= last {
        match find_either(&h[i + k..=last + k], lo, up) {
            None => return false,
            Some(p) => i += p,
        }
        if h[i..i + n.len()].eq_ignore_ascii_case(n) { return true; }
        i += 1;
    }
    false
}

/// How common a byte is in chat text (lower = rarer); only used to pick which byte of a needle to scan for.
fn byte_rank(b: u8) -> usize {
    const COMMON: &[u8] = b"zqxjkvbpygfwmucldrhsnioate ";
    COMMON.iter().position(|&c| c == b.to_ascii_lowercase()).unwrap_or(0)
}

/// Where the first byte that is `a` or `b` is.
fn find_either(h: &[u8], a: u8, b: u8) -> Option<usize> {
    #[cfg(feature = "simd")]
    return find_either_simd(h, a, b);
    #[cfg(not(feature = "simd"))]
    memchr::memchr2(a, b, h) /* picks AVX2/SSE2 at run time */
}

/// `find_either` 32 bytes at a time.
#[cfg(feature = "simd")]
fn find_either_simd(h: &[u8], a: u8, b: u8) -> Option<usize> {
    use std::simd::prelude::*;
    let (sa, sb) = (u8x32::splat(a), u8x32::splat(b));
    let mut chunks = h.chunks_exact(32);
    for (n, c) in chunks.by_ref().enumerate() {
        let v = u8x32::from_slice(c);
        if let Some(i) = (v.simd_eq(sa) | v.simd_eq(sb)).first_set() { return Some(n * 32 + i); }
    }
    let done = h.len() - chunks.remainder().len();
    chunks.remainder().iter().position(|&c| c == a || c == b).map(|i| done + i)
}

/// A search looks at this many different words of the query at most.
const MAX_QUERY_WORDS: usize = 16;
/// The most rows one segment file holds (a save of more writes several): sealing one takes about three times its text in memory for a moment.
const SEGMENT_ROWS: usize = 20_000;
/// An index with more text than this is searched on several threads.
const PARALLEL_BYTES: usize = 4 << 20;
/// More segment files than this are written again as a few big ones.
const MAX_SEGMENTS: usize = 12;

/// What a segment file holds: the rooms forgotten since the last one (their earlier rows are void) and then the rows that are new or changed.
#[derive(Default, Serialize, Deserialize)]
struct Segment {
    #[serde(default)]
    forgot: Vec<String>,
    #[serde(default)]
    rows: Vec<IndexRow>,
}

/// A segment as it is written: the same JSON as `Segment`, made from the arenas without copying a row's text into strings of its own first.
struct SegmentOut<'a> { forgot: &'a [String], index: &'a MessageIndex, rows: &'a [u32] }

impl Serialize for SegmentOut<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        struct RowOut<'a> { index: &'a MessageIndex, n: u32 }
        impl Serialize for RowOut<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeStruct;
                let (ix, r) = (self.index, &self.index.rows[self.n as usize]);
                let mut o = s.serialize_struct("IndexRow", 6)?;
                o.serialize_field("room_id", ix.rooms.get(r.room))?;
                o.serialize_field("event_id", ix.id_of(r))?;
                o.serialize_field("sender", ix.senders.get(r.sender))?;
                o.serialize_field("time", &crate::ui::clock(r.ts))?;
                o.serialize_field("ts", &r.ts)?;
                o.serialize_field("body", ix.body_of(r))?;
                o.end()
            }
        }
        struct Rows<'a>(&'a MessageIndex, &'a [u32]);
        impl Serialize for Rows<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> { s.collect_seq(self.1.iter().map(|&n| RowOut { index: self.0, n })) }
        }
        let mut o = s.serialize_struct("Segment", 2)?;
        o.serialize_field("forgot", self.forgot)?;
        o.serialize_field("rows", &Rows(self.index, self.rows))?;
        o.end()
    }
}

/// One message: numbers, and where its event id and body are in the text arena.
#[derive(Clone, Copy)]
struct Row { ts: u64, room: u32, sender: u32, id_off: u32, body_off: u32, body_len: u32, id_len: u16 }

/// Names (room ids, sender names) kept once each, numbered in the order they came.
#[derive(Default)]
struct Names { list: Vec<Arc<str>>, number: HashMap<Arc<str>, u32> }

impl Names {
    fn id(&mut self, name: &str) -> u32 {
        if let Some(&n) = self.number.get(name) { return n; }
        let n = self.list.len() as u32;
        let shared: Arc<str> = name.into();
        self.list.push(shared.clone());
        self.number.insert(shared, n);
        n
    }
    fn find(&self, name: &str) -> Option<u32> { self.number.get(name).copied() }
    fn get(&self, n: u32) -> &str { &self.list[n as usize] }
}

pub struct MessageIndex {
    dir: PathBuf,
    cipher: Arc<StoreCipher>,
    rooms: Names,
    senders: Names,
    /// the bodies of all rows, one after the other
    text: String,
    /// the event ids of all rows, apart from the bodies: a search scans `text` and never loads an id into the cache
    ids: String,
    rows: Vec<Row>,
    /// open-addressing table of row number + 1 (0 = free) by (room, event id); its length is a power of two
    slots: Vec<u32>,
    hasher: std::collections::hash_map::RandomState,
    /// rows that are new or changed since the last save
    pending: Vec<u32>,
    /// rooms forgotten since the last save
    forgot: Vec<String>,
    /// the segment files on disk, oldest first, and the number the next one gets
    segments: Vec<u32>,
    next_segment: u32,
    /// the index was read from the old single file: the next save writes everything as segments and removes it
    legacy: bool,
    /// bytes of `text` that no row refers to any more (a body that was edited)
    garbage: usize,
    /// a body was edited: its new text is at the end of the arena, so the rows are no longer in the arena's order (until the next repack)
    unordered: bool,
    dirty: bool,
}

/// The key files written before the secret was stretched (one bare SHA-256, which makes a passphrase cheap to guess); still read, then rewritten.
fn old_key_of(secret: &str) -> [u8; 32] { Sha256::digest(secret.as_bytes()).into() }

/// The cipher for the sealed file `<stem>.enc` in `dir`, kept (itself sealed with the secret, through a salted key derivation) in `<stem>-cipher.bin`.
/// When the secret does not open it, a new cipher replaces it and the old sealed data is dropped.
pub(crate) fn open_cipher(dir: &Path, stem: &str, secret: &str) -> Result<StoreCipher, String> {
    let _ = std::fs::create_dir_all(dir);
    let cipher_file = dir.join(format!("{stem}-cipher.bin"));
    let bytes = std::fs::read(&cipher_file).ok();
    if let Some(c) = bytes.as_deref().and_then(|b| StoreCipher::import(secret, b).ok()) { return Ok(c); }
    if let Some(c) = bytes.as_deref().and_then(|b| StoreCipher::import_with_key(&old_key_of(secret), b).ok()) {
        std::fs::write(&cipher_file, c.export(secret).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?; /* upgrade in place */
        return Ok(c);
    }
    let c = StoreCipher::new().map_err(|e| e.to_string())?;
    std::fs::write(&cipher_file, c.export(secret).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(dir.join(format!("{stem}.enc"))); /* sealed with the old cipher: unreadable now */
    Ok(c)
}

fn segment_path(dir: &Path, n: u32) -> PathBuf { dir.join(format!("seg-{n:06}.enc")) }

/// The numbers of the segment files in `dir`, oldest first.
fn segment_numbers(dir: &Path) -> Vec<u32> {
    let mut out: Vec<u32> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix("seg-")?.strip_suffix(".enc")?.parse().ok()))
        .collect();
    out.sort_unstable();
    out
}

impl MessageIndex {
    /// Open (or start) the index in `dir`. What cannot be opened with this secret is treated as lost and replaced.
    pub fn open(dir: &Path, secret: &str) -> Result<MessageIndex, String> {
        let cipher = Arc::new(open_cipher(dir, "index", secret)?);
        let mut idx = MessageIndex {
            dir: dir.to_path_buf(), cipher, rooms: Names::default(), senders: Names::default(), text: String::new(), ids: String::new(), rows: Vec::new(), slots: Vec::new(),
            hasher: Default::default(), pending: Vec::new(), forgot: Vec::new(), segments: Vec::new(), next_segment: 1, legacy: false, garbage: 0, unordered: false, dirty: false,
        };
        /* the index of the earlier versions: one sealed file with every row */
        if let Some(rows) = std::fs::read(dir.join("index.enc")).ok().and_then(|b| idx.cipher.decrypt_value::<Vec<IndexRow>>(&b).ok()) {
            for r in &rows { idx.upsert(&r.room_id, &r.event_id, &r.sender, r.ts, &r.body); }
            idx.legacy = true;
        }
        // Reading and unsealing a file does not depend on the others; applying them does (a forgotten room, an edit). So a few at a time are unsealed on
        // threads of their own and applied in order (all at once would hold the whole index twice).
        let numbers = segment_numbers(dir);
        let wave = std::thread::available_parallelism().map_or(1, |n| n.get()).clamp(1, 4);
        for wave in numbers.chunks(wave) {
            let cipher = &idx.cipher;
            let unseal = |n: u32| std::fs::read(segment_path(dir, n)).ok().and_then(|b| cipher.decrypt_value::<Segment>(&b).ok());
            let segments: Vec<Option<Segment>> = if wave.len() == 1 { vec![unseal(wave[0])] } else {
                std::thread::scope(|sc| {
                    let unseal = &unseal;
                    let runs: Vec<_> = wave.iter().map(|&n| sc.spawn(move || unseal(n))).collect();
                    runs.into_iter().map(|h| h.join().unwrap_or(None)).collect()
                })
            };
            for (&n, seg) in wave.iter().zip(segments) {
                idx.next_segment = n + 1;
                match seg {
                    Some(seg) => {
                        for room in &seg.forgot { idx.drop_room(room); }
                        for r in &seg.rows { idx.upsert(&r.room_id, &r.event_id, &r.sender, r.ts, &r.body); }
                        idx.segments.push(n);
                    }
                    None => { let _ = std::fs::remove_file(segment_path(dir, n)); } /* sealed with another secret, or damaged */
                }
            }
        }
        idx.pending = Vec::new(); /* what was read is saved already */
        idx.forgot.clear();
        idx.dirty = idx.legacy;
        idx.text.shrink_to_fit();
        idx.ids.shrink_to_fit(); /* the buffers grew by doubling while the files were read */
        idx.rows.shrink_to_fit();
        Ok(idx)
    }

    pub fn len(&self) -> usize { self.rows.len() }

    /// About how many bytes the rows take in memory (what the text arena, the rows, the table and the list of unsaved rows hold; not the names). The
    /// unused end of a buffer that grew is not counted: it was never touched, so it is not resident.
    pub fn approx_bytes(&self) -> usize { self.text.len() + self.ids.len() + self.rows.len() * std::mem::size_of::<Row>() + self.slots.len() * 4 + self.pending.len() * 4 }

    fn id_of(&self, r: &Row) -> &str { &self.ids[r.id_off as usize..r.id_off as usize + r.id_len as usize] }
    fn body_of(&self, r: &Row) -> &str { &self.text[r.body_off as usize..r.body_off as usize + r.body_len as usize] }

    /// The row of an event of a room, if it is indexed.
    fn find(&self, room: u32, event_id: &str) -> Option<u32> {
        if self.slots.is_empty() { return None; }
        let mask = self.slots.len() - 1;
        let mut i = self.hasher.hash_one((room, event_id)) as usize & mask;
        loop {
            let s = self.slots[i];
            if s == 0 { return None; }
            let r = &self.rows[(s - 1) as usize];
            if r.room == room && self.id_of(r) == event_id { return Some(s - 1); }
            i = (i + 1) & mask;
        }
    }

    /// Is this event indexed already?
    pub fn knows(&self, room_id: &str, event_id: &str) -> bool { self.rooms.find(room_id).map_or(false, |room| self.find(room, event_id).is_some()) }

    /// The table is made again, big enough for the rows there are (never more than 70% full, 35% after it is made).
    fn rebuild_slots(&mut self) {
        self.slots = vec![0; (self.rows.len() * 2).next_power_of_two().max(16)];
        let mask = self.slots.len() - 1;
        for (n, r) in self.rows.iter().enumerate() {
            let mut i = self.hasher.hash_one((r.room, self.id_of(r))) as usize & mask;
            while self.slots[i] != 0 { i = (i + 1) & mask; }
            self.slots[i] = n as u32 + 1;
        }
    }

    /// Put row `n` (just added to `rows`) in the table.
    fn place(&mut self, n: u32) {
        if self.rows.len() * 10 > self.slots.len() * 7 { self.rebuild_slots(); return; } /* includes row n */
        let mask = self.slots.len() - 1;
        let r = self.rows[n as usize];
        let mut i = self.hasher.hash_one((r.room, self.id_of(&r))) as usize & mask;
        while self.slots[i] != 0 { i = (i + 1) & mask; }
        self.slots[i] = n + 1;
    }

    fn push_text(&mut self, s: &str) -> (u32, u32) {
        let off = self.text.len();
        self.text.push_str(s);
        (off as u32, s.len() as u32)
    }

    /// Add a message, or change the one with this event id (an edit). True when the index changed.
    fn upsert(&mut self, room_id: &str, event_id: &str, sender: &str, ts: u64, body: &str) -> bool {
        if event_id.len() > u16::MAX as usize || self.text.len() + body.len() >= u32::MAX as usize || self.ids.len() + event_id.len() >= u32::MAX as usize { return false; } /* no room for it */
        let (room, sender) = (self.rooms.id(room_id), self.senders.id(sender));
        if let Some(n) = self.find(room, event_id) {
            let r = self.rows[n as usize];
            if r.ts == ts && r.sender == sender && self.body_of(&r) == body { return false; }
            let (off, len) = self.push_text(body); /* the new text goes at the end, the old one is garbage until the arena is packed */
            self.garbage += r.body_len as usize;
            self.unordered = true;
            let r = &mut self.rows[n as usize];
            (r.ts, r.sender, r.body_off, r.body_len) = (ts, sender, off, len);
            self.pending.push(n);
        } else {
            let (id_off, id_len) = (self.ids.len() as u32, event_id.len() as u32);
            self.ids.push_str(event_id);
            let (body_off, body_len) = self.push_text(body);
            let n = self.rows.len() as u32;
            self.rows.push(Row { ts, room, sender, id_off, body_off, body_len, id_len: id_len as u16 });
            self.place(n);
            self.pending.push(n);
        }
        self.dirty = true;
        true
    }

    #[cfg(test)]
    pub(crate) fn insert_row(&mut self, row: IndexRow) { self.upsert(&row.room_id, &row.event_id, &row.sender, row.ts, &row.body); }

    /// Add the readable messages of these timeline items (what is not decrypted yet, pending or empty is skipped).
    pub fn add_items(&mut self, room_id: &str, items: &[Arc<TimelineItem>]) -> usize { self.add_messages(room_id, &ui_messages(items, "")) }

    /// `add_items` for rows that are already made (the open timeline has just turned its items into rows for the UI: no second pass over them).
    /// Messages the index has already cost nothing: they are looked up by their text, not copied.
    pub fn add_messages(&mut self, room_id: &str, messages: &[UiMessage]) -> usize {
        let mut added = 0;
        for m in messages {
            if m.pending || m.id.starts_with('~') || m.body.is_empty() || !matches!(m.kind.as_str(), "text" | "emote" | "notice" | "image" | "video" | "audio" | "file" | "poll") { continue; }
            if self.upsert(room_id, &m.id, &m.sender, m.ts, &m.body) { added += 1; }
        }
        added
    }

    /// A row as the callers know it.
    fn materialize(&self, n: u32) -> IndexRow {
        let r = &self.rows[n as usize];
        IndexRow { room_id: self.rooms.get(r.room).into(), event_id: self.id_of(r).into(), sender: self.senders.get(r.sender).into(), time: crate::ui::clock(r.ts), ts: r.ts, body: self.body_of(r).into() }
    }

    /// The rows whose body contains `w` (ASCII, lowercase; `k` = `rarest_byte(w)`) or is cut by it, found by one scan of the whole text arena instead of one
    /// call per message (a message is about 80 bytes, so the call cost more than the scan). Ascending row numbers. Needs the rows in arena order (`!unordered`).
    /// `None` when the word is common (more than every fourth message of the first sixteenth has it): then nearly every row is a candidate and the scan per row is cheaper.
    fn arena_candidates(&self, w: &str, k: usize, threads: u32) -> Option<Vec<u32>> {
        let (n, text) = (w.as_bytes(), self.text.as_bytes());
        let (lo, up) = (n[k].to_ascii_lowercase(), n[k].to_ascii_uppercase());
        // the rows whose match starts in text[from..to]
        let scan = |from: usize, to: usize| -> Vec<u32> {
            let mut out = Vec::new();
            let mut at = from;
            let end = (to + k).min(text.len());
            while at + k < end {
                let Some(p) = find_either(&text[at + k..end], lo, up) else { break };
                let s = at + p;
                if s + n.len() <= text.len() && text[s..s + n.len()].eq_ignore_ascii_case(n) {
                    let r = self.rows.partition_point(|r| (r.body_off as usize) <= s);
                    if r > 0 {
                        let row = &self.rows[r - 1];
                        let e = row.body_off as usize + row.body_len as usize;
                        if s < e { out.push((r - 1) as u32); at = e; continue; } /* the rest of this body has nothing more to tell */
                    }
                } /* else: stale text of an edited message, or a match across two bodies (the rows are checked whole afterwards) */
                at = s + 1;
            }
            out
        };
        let sample = (text.len() / 16).max(1 << 16).min(text.len());
        let mut rows = scan(0, sample);
        if rows.len() * 4 * (text.len() / self.rows.len().max(1)).max(1) > sample { return None; }
        let rest = text.len() - sample;
        let threads = threads.max(1) as usize;
        let step = rest.div_ceil(threads).max(1);
        if rest > 0 {
            if threads == 1 { rows.extend(scan(sample, text.len())); } else {
                std::thread::scope(|sc| {
                    let scan = &scan;
                    let runs: Vec<_> = (sample..text.len()).step_by(step).map(|from| sc.spawn(move || scan(from, (from + step).min(text.len())))).collect();
                    for h in runs { rows.extend(h.join().unwrap_or_default()); }
                });
            }
        }
        rows.dedup(); /* a body can hold matches in two of the runs */
        Some(rows)
    }

    /// Messages containing every word of `query` (case-insensitive), in one room or all, newest first, at most `limit`.
    pub fn search(&self, query: &str, room_id: Option<&str>, limit: usize) -> Vec<IndexRow> { self.search_on(query, room_id, limit, None) }

    /// `search` on `threads` threads (`None`: as many as the index is worth).
    fn search_on(&self, query: &str, room_id: Option<&str>, limit: usize, threads: Option<u32>) -> Vec<IndexRow> {
        let mut words: Vec<String> = Vec::new();
        for w in query.split_whitespace().map(|w| w.to_lowercase()) {
            if !words.contains(&w) { words.push(w); } /* a word said twice asks nothing more (a pasted "rust rust rust ..." cost seconds per search) */
            if words.len() == MAX_QUERY_WORDS { break; }
        }
        words.sort_by_key(|w| std::cmp::Reverse(w.len())); /* the longest word is the most selective: most rows fail on it first */
        if words.is_empty() || limit == 0 { return Vec::new(); }
        let room = match room_id { Some(id) => match self.rooms.find(id) { Some(n) => Some(n), None => return Vec::new() }, None => None };
        // Which words each sender's name contains (bit j = word j), worked out once: the scan below then reads this small array (it stays in L1)
        // instead of following a pointer to a separately allocated name for every row whose body misses a word.
        let rare: Vec<usize> = words.iter().map(|w| rarest_byte(w)).collect();
        let by_sender: Vec<u16> = self.senders.list.iter().map(|s| words.iter().enumerate().fold(0u16, |m, (j, w)| m | (contains_ci_at(s, w, rare[j]) as u16) << j)).collect();
        let all_words: u16 = ((1u32 << words.len()) - 1) as u16;
        let keep = |n: u32| {
            let r = &self.rows[n as usize];
            if !room.map_or(true, |x| x == r.room) { return false; }
            let named = by_sender[r.sender as usize];
            named == all_words || { let body = self.body_of(r); words.iter().enumerate().all(|(j, w)| named >> j & 1 != 0 || contains_ci_at(body, w, rare[j])) }
        };
        let total = self.rows.len() as u32;
        let threads = if let Some(t) = threads { t } else if self.text.len() < PARALLEL_BYTES { 1 } else { std::thread::available_parallelism().map_or(1, |n| n.get()).min(8) as u32 };
        // The first word (the longest) is looked for in the whole arena at once, when no sender's name can stand in for it and the rows are in arena order.
        let arena = if !self.unordered && words[0].is_ascii() && by_sender.iter().all(|m| m & 1 == 0) { self.arena_candidates(&words[0], rare[0], threads) } else { None };
        let mut hits: Vec<u32> = if let Some(rows) = arena {
            rows.into_iter().filter(|&n| keep(n)).collect()
        } else if threads <= 1 { (0..total).filter(|&n| keep(n)).collect() } else {
            // Every row is judged on its own, so a big index is cut into runs of rows, one thread each (a small one is not worth the threads).
            let step = total.div_ceil(threads).max(1);
            std::thread::scope(|sc| {
                let keep = &keep;
                let runs: Vec<_> = (0..total).step_by(step as usize).map(|from| sc.spawn(move || (from..(from + step).min(total)).filter(|&n| keep(n)).collect::<Vec<u32>>())).collect();
                runs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
            })
        };
        let newest = |a: &u32, b: &u32| {
            let (ra, rb) = (&self.rows[*a as usize], &self.rows[*b as usize]);
            rb.ts.cmp(&ra.ts).then_with(|| self.id_of(rb).cmp(self.id_of(ra)))
        };
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit - 1, newest);
            hits.truncate(limit);
        }
        hits.sort_by(newest);
        hits.into_iter().map(|n| self.materialize(n)).collect()
    }

    /// The text arena made again with the rows' text only, without the rooms of `skip` (whose rows go). Row numbers change when rows go: the result says
    /// where each old row went (`u32::MAX`: nowhere).
    fn repack(&mut self, skip: Option<u32>) -> Vec<u32> {
        let mut remap = vec![u32::MAX; self.rows.len()];
        let mut rows = Vec::with_capacity(self.rows.len());
        let mut text = String::with_capacity(self.text.len().saturating_sub(self.garbage));
        let mut ids = String::with_capacity(self.ids.len());
        for (old, r) in self.rows.iter().enumerate() {
            if Some(r.room) == skip { continue; }
            let mut r = *r;
            let id = &self.ids[r.id_off as usize..r.id_off as usize + r.id_len as usize];
            r.id_off = ids.len() as u32;
            ids.push_str(id);
            let body = &self.text[r.body_off as usize..r.body_off as usize + r.body_len as usize];
            r.body_off = text.len() as u32;
            text.push_str(body);
            remap[old] = rows.len() as u32;
            rows.push(r);
        }
        self.rows = rows;
        self.text = text;
        self.ids = ids;
        self.garbage = 0;
        self.unordered = false;
        self.rebuild_slots();
        remap
    }

    /// Forget the rows of a room (in memory). True when there were any.
    fn drop_room(&mut self, room_id: &str) -> bool {
        let Some(room) = self.rooms.find(room_id) else { return false };
        if !self.rows.iter().any(|r| r.room == room) { return false; }
        let remap = self.repack(Some(room));
        self.pending = self.pending.iter().filter_map(|&n| Some(remap[n as usize]).filter(|&m| m != u32::MAX)).collect();
        true
    }

    /// Forget everything of a room (we left it).
    pub fn forget_room(&mut self, room_id: &str) {
        if self.drop_room(room_id) { self.forgot.push(room_id.to_string()); self.dirty = true; }
    }

    /// Seal these rows (by number) and the rooms forgotten into a file of its own; returns its number.
    fn write_segment(&mut self, forgot: &[String], rows: &[u32]) -> Result<u32, String> {
        let sealed = self.cipher.encrypt_value(&SegmentOut { forgot, index: self, rows }).map_err(|e| e.to_string())?;
        let n = self.next_segment;
        let path = segment_path(&self.dir, n);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, sealed).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string())?;
        self.next_segment += 1;
        self.segments.push(n);
        Ok(n)
    }

    /// Seal rows (by number) and the rooms forgotten since the last save into new segment files, SEGMENT_ROWS rows to a file.
    fn write_pending(&mut self, pending: &[u32]) -> Result<(), String> {
        let mut forgot = self.forgot.clone();
        let mut chunks: Vec<&[u32]> = pending.chunks(SEGMENT_ROWS).collect();
        if chunks.is_empty() && !forgot.is_empty() { chunks.push(&[]); }
        for chunk in chunks {
            self.write_segment(&std::mem::take(&mut forgot), chunk)?;
        }
        Ok(())
    }

    /// Write what changed since the last save as a new segment file (several when it is a lot); when there are many segments, all rows are written
    /// again as a few and the old files go.
    pub fn save(&mut self) -> Result<(), String> {
        if !self.dirty { return Ok(()); }
        if self.legacy || self.segments.len() >= MAX_SEGMENTS { return self.compact(); }
        self.pending.sort_unstable();
        self.pending.dedup();
        let pending = std::mem::take(&mut self.pending);
        if let Err(e) = self.write_pending(&pending) { self.pending = pending; return Err(e); }
        self.forgot.clear();
        self.dirty = false;
        if self.garbage > (1 << 20) && self.garbage * 3 > self.text.len() { self.repack(None); } /* edited bodies: the old texts are given back */
        Ok(())
    }

    /// Write every row again as segments of their own and remove the older files (and the single file of the earlier versions).
    fn compact(&mut self) -> Result<(), String> {
        let old = self.segments.clone();
        let total = self.rows.len();
        let mut from = 0;
        while from < total {
            let to = (from + SEGMENT_ROWS).min(total);
            let rows: Vec<u32> = (from as u32..to as u32).collect();
            self.write_segment(&[], &rows)?;
            from = to;
        }
        for n in &old { let _ = std::fs::remove_file(segment_path(&self.dir, *n)); }
        let _ = std::fs::remove_file(self.dir.join("index.enc"));
        self.segments.retain(|n| !old.contains(n));
        self.legacy = false;
        self.pending.clear();
        self.forgot.clear();
        self.dirty = false;
        if self.garbage > 0 { self.repack(None); }
        Ok(())
    }
}

/// Page back through a room's history (up to `pages` pages of 30 events) and index what is readable. Uses its own timeline, so the one on
/// screen is not disturbed. Returns true when the start of the room was reached.
pub async fn crawl_room(room: &matrix_sdk::Room, index: Option<&std::sync::Mutex<MessageIndex>>, pages: usize) -> Result<bool, String> {
    let timeline = crate::ui::open_timeline(room).await?;
    let room_id = room.room_id().to_string();
    let mut reached = false;
    for _ in 0..pages {
        reached = timeline.paginate_backwards(30).await.map_err(|e| e.to_string())?;
        let items: Vec<_> = timeline.items().await.iter().cloned().collect();
        if let Some(index) = index {
            let mut index = index.lock().map_err(|_| "index lock poisoned")?;
            /* every page shows the timeline again from its newest item: only what the earlier pages did not index is turned into rows */
            let fresh: Vec<_> = items.iter().filter(|i| i.as_event().map_or(false, |e| e.event_id().map_or(true, |id| !index.knows(&room_id, id.as_str())))).cloned().collect();
            index.add_items(&room_id, &fresh);
        }
        if reached { break; }
    }
    Ok(reached)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{FakeHs, ROOM};
    use matrix_sdk_ui::timeline::RoomExt;

    #[test]
    fn a_word_is_found_in_any_case_wherever_it_starts() {
        for at in [0, 1, 30, 31, 32, 33, 63, 64, 65, 100, 127] { /* around the 32-byte blocks the scan reads, and in the bytes left over after them */
            for len in [at + 5, at + 6, at + 40, 200] {
                let mut hay = "x".repeat(len);
                hay.replace_range(at..at + 5, "HeLlo");
                assert!(contains_ci(&hay, "hello") && contains_ci(&hay, "HELLO"), "at {at} of {len}");
                assert!(!contains_ci(&hay, "help"), "at {at} of {len}");
                hay.replace_range(at..at + 1, "h");
                assert!(contains_ci(&hay, "hEllo"), "at {at} of {len}");
            }
        }
        assert!(contains_ci(&format!("{}hhhhhhelp", "h".repeat(70)), "help"), "a first letter repeated before the word");
        assert!(!contains_ci("hel", "help") && contains_ci("abc", "") && !contains_ci("", "a"));
        assert!(contains_ci("the cat and the hat", "hat") && contains_ci("xxxxxQ", "xq") && !contains_ci("xxxxx", "xq"), "rarest byte not first");
        assert!(contains_ci("Grüße aus KÖLN", "köln") && contains_ci("ÜBER alles", "über"), "a word with other letters comes lowercased (the search lowercases it)");
    }

    #[cfg(feature = "simd")]
    #[test]
    fn the_simd_scan_finds_the_same_byte_as_the_plain_one() {
        let mut s = 5u32;
        for len in [0, 1, 31, 32, 33, 64, 95, 1000] {
            for _ in 0..50 {
                let h: Vec<u8> = (0..len).map(|_| { s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); b'a' + (s >> 24) as u8 % 20 }).collect();
                for (a, b) in [(b'q', b'Q'), (b'a', b'b'), (b'z', b'Z')] {
                    assert_eq!(find_either_simd(&h, a, b), h.iter().position(|&c| c == a || c == b), "{len} bytes");
                }
            }
        }
    }

    #[test]
    fn a_big_index_searched_on_many_threads_gives_what_one_thread_gives() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        let words = ["the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog"];
        let mut s = 7u32;
        for n in 0..70_000u64 {
            let mut body = String::new();
            for _ in 0..12 { s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); body.push_str(words[(s >> 20) as usize % 8]); body.push(' '); }
            idx.insert_row(IndexRow { room_id: format!("!r{}:hs", n % 3), event_id: format!("$e{n}"), sender: format!("user{}", n % 7), time: String::new(), ts: n, body });
        }
        assert!(idx.text.len() >= PARALLEL_BYTES);
        for q in ["fox dog", "user3 lazy", "zebra", "QUICK brown fox"] {
            let t = std::time::Instant::now();
            let one = idx.search_on(q, None, 50, Some(1));
            let t1 = t.elapsed();
            let t = std::time::Instant::now();
            let many = idx.search(q, None, 50);
            eprintln!("{q:16} 1 thread {t1:?}, default {:?}", t.elapsed());
            assert_eq!(one, many, "{q}");
            idx.unordered = true;
            let t = std::time::Instant::now();
            let per_row = idx.search_on(q, None, 50, Some(1));
            eprintln!("{q:16} per-row, 1 thread {:?}", t.elapsed());
            idx.unordered = false;
            assert_eq!(one, per_row, "{q} per row");
            assert_eq!(idx.search_on(q, Some("!r1:hs"), 5, Some(3)), idx.search_on(q, Some("!r1:hs"), 5, Some(1)), "{q} in a room");
        }
    }

    #[test]
    fn the_arena_scan_ignores_matches_across_bodies_and_stale_text() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        let row = |n: u64, body: &str| IndexRow { room_id: "!r:hs".into(), event_id: format!("$e{n}"), sender: "alice".into(), time: String::new(), ts: n, body: body.into() };
        for (n, b) in ["xxab", "cdxx", "plain zebra", "", "last QUARTZ"].iter().enumerate() { idx.insert_row(row(n as u64, b)); }
        let ids = |idx: &MessageIndex, q: &str| idx.search(q, None, 10).into_iter().map(|r| r.event_id).collect::<Vec<_>>();
        assert!(ids(&idx, "abcd").is_empty(), "a word is not found across two messages");
        assert_eq!(ids(&idx, "zebra"), vec!["$e2"]);
        assert_eq!(ids(&idx, "quartz"), vec!["$e4"]);
        idx.insert_row(row(2, "plain horse")); /* an edit: the old text stays in the arena as garbage */
        assert!(ids(&idx, "zebra").is_empty(), "stale text is not found");
        assert_eq!(ids(&idx, "horse"), vec!["$e2"]);
        idx.repack(None);
        assert!(ids(&idx, "zebra").is_empty() && ids(&idx, "quartz") == vec!["$e4"] && ids(&idx, "horse") == vec!["$e2"]);
    }

    #[test]
    fn segments_unsealed_together_are_still_applied_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let row = |room: &str, n: u64, body: &str| IndexRow { room_id: room.into(), event_id: format!("$e{n}"), sender: "alice".into(), time: String::new(), ts: n, body: body.into() };
        {
            let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
            for n in 0..6u64 { idx.insert_row(row("!a:hs", n, &format!("first {n}"))); idx.insert_row(row("!b:hs", 100 + n, &format!("other {n}"))); idx.save().unwrap(); }
            idx.insert_row(row("!a:hs", 2, "edited two")); idx.save().unwrap(); /* an edit in a later segment than the row */
            idx.forget_room("!b:hs"); idx.save().unwrap(); /* a room forgotten after its rows were saved */
            idx.insert_row(row("!b:hs", 200, "came back")); idx.save().unwrap();
            assert!(idx.segments.len() > 4 && idx.segments.len() < MAX_SEGMENTS, "{} segments", idx.segments.len());
        }
        let idx = MessageIndex::open(dir.path(), "s").unwrap();
        let bodies = |q: &str| idx.search(q, None, 50).into_iter().map(|r| r.body).collect::<Vec<_>>();
        assert_eq!(bodies("edited"), vec!["edited two"]);
        assert!(bodies("other").is_empty(), "the forgotten room's rows stay forgotten");
        assert_eq!(bodies("came"), vec!["came back"]);
        assert_eq!(idx.len(), 7);
    }

    #[test]
    #[ignore]
    fn opening_a_big_index_time() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
            for n in 0..200_000u64 { idx.insert_row(IndexRow { room_id: format!("!r{}:hs", n % 9), event_id: format!("$event{n}:hs"), sender: format!("user{}", n % 40), time: String::new(), ts: n, body: format!("message number {n} with some words in it to make it longer") }); }
            idx.save().unwrap();
        }
        let t = std::time::Instant::now();
        let idx = MessageIndex::open(dir.path(), "s").unwrap();
        eprintln!("open {} rows in {} segments: {:?}", idx.len(), idx.segments.len(), t.elapsed());
    }

    #[test]
    fn a_segment_written_from_the_arenas_is_the_json_of_its_rows() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        for n in 0..5u64 { idx.insert_row(IndexRow { room_id: format!("!r{}:hs", n % 2), event_id: format!("$e{n}"), sender: "ünï \"q\"".into(), time: String::new(), ts: n * 61_000, body: format!("line\n{n} \u{1F600} \"quoted\"") }); }
        let rows = [4u32, 0, 2];
        let forgot = vec!["!old:hs".to_string()];
        let owned = Segment { forgot: forgot.clone(), rows: rows.iter().map(|&n| idx.materialize(n)).collect() };
        let out = serde_json::to_string(&SegmentOut { forgot: &forgot, index: &idx, rows: &rows }).unwrap();
        assert_eq!(out, serde_json::to_string(&owned).unwrap());
    }

    #[test]
    #[ignore]
    fn making_a_segment_time() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        for n in 0..20_000u64 { idx.insert_row(IndexRow { room_id: format!("!r{}:hs", n % 9), event_id: format!("$event{n}:hs"), sender: format!("user{}", n % 40), time: String::new(), ts: n, body: format!("message number {n} with some words in it to make it longer") }); }
        let rows: Vec<u32> = (0..20_000).collect();
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let owned = serde_json::to_vec(&Segment { forgot: Vec::new(), rows: rows.iter().map(|&n| idx.materialize(n)).collect() }).unwrap();
            let t1 = t.elapsed();
            let t = std::time::Instant::now();
            let borrowed = serde_json::to_vec(&SegmentOut { forgot: &[], index: &idx, rows: &rows }).unwrap();
            eprintln!("owned rows {t1:?}, from the arenas {:?} ({} bytes)", t.elapsed(), borrowed.len());
            assert_eq!(owned, borrowed);
        }
    }

    #[test]
    fn repeated_query_words_cost_nothing_and_long_queries_are_cut() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        for (n, body) in ["the quick brown fox", "the lazy dog", "Brown FOX jumps"].iter().enumerate() {
            let row = IndexRow { room_id: "!r:hs".into(), event_id: format!("$e{n}"), sender: "alice".into(), time: String::new(), ts: n as u64, body: body.to_string() };
            idx.insert_row(row);
        }
        let ids = |q: &str| idx.search(q, None, 10).into_iter().map(|r| r.event_id).collect::<Vec<_>>();
        assert_eq!(ids("fox"), ids(&"fox ".repeat(5000)));
        assert_eq!(ids("brown fox"), vec!["$e2", "$e0"]);
        assert_eq!(ids("FOX brown fox Brown"), ids("brown fox"));
        assert!(ids("fox dog").is_empty());
        // only the first 16 different words count
        let many: Vec<String> = (0..MAX_QUERY_WORDS).map(|n| format!("word{n}")).collect();
        let row = IndexRow { room_id: "!r:hs".into(), event_id: "$many".into(), sender: "alice".into(), time: String::new(), ts: 9, body: many.join(" ") };
        idx.insert_row(row);
        assert_eq!(idx.search(&many.join(" "), None, 10).len(), 1);
        assert_eq!(idx.search(&format!("{} nowhere", many.join(" ")), None, 10).len(), 1, "the 17th different word is not looked at");
        assert!(idx.search(&format!("nowhere {}", many[..MAX_QUERY_WORDS - 1].join(" ")), None, 10).is_empty(), "the first 16 still have to match");
    }

    fn row(room: &str, id: &str, who: &str, ts: u64, body: &str) -> IndexRow { IndexRow { room_id: room.into(), event_id: id.into(), sender: who.into(), time: String::new(), ts, body: body.into() } }

    fn segment_files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("seg-")).collect();
        v.sort();
        v
    }

    #[test]
    fn a_word_may_match_the_sender_or_the_body_and_every_word_must_match_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut ix = MessageIndex::open(dir.path(), "secret").unwrap();
        ix.insert_row(row("!r", "$1", "Alice", 1, "lunch at noon"));
        ix.insert_row(row("!r", "$2", "Bob", 2, "alice said lunch"));
        ix.insert_row(row("!r", "$3", "Bob", 3, "dinner"));
        let ids = |q: &str| ix.search(q, None, 10).into_iter().map(|r| r.event_id).collect::<Vec<_>>();
        assert_eq!(ids("alice lunch"), ["$2", "$1"]); /* name + body, and body + body */
        assert_eq!(ids("ALICE"), ["$2", "$1"]);
        assert_eq!(ids("bob dinner"), ["$3"]);
        assert_eq!(ids("alice dinner"), Vec::<String>::new());
    }

    #[test]
    fn a_save_writes_only_what_changed_and_everything_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        for i in 0..50 { idx.insert_row(row("!a:hs", &format!("$a{i}"), "Alice", 1000 + i, &format!("first batch {i}"))); }
        idx.save().unwrap();
        assert_eq!(segment_files(dir.path()).len(), 1);
        let first = std::fs::metadata(dir.path().join("seg-000001.enc")).unwrap().len();
        idx.save().unwrap();
        assert_eq!(segment_files(dir.path()).len(), 1, "a save with nothing new writes nothing");
        for i in 0..3 { idx.insert_row(row("!b:hs", &format!("$b{i}"), "Bob", 5000 + i, &format!("second batch {i}"))); }
        idx.save().unwrap();
        assert_eq!(segment_files(dir.path()).len(), 2);
        assert!(std::fs::metadata(dir.path().join("seg-000002.enc")).unwrap().len() * 5 < first, "the second segment holds three rows, not all fifty-three");
        /* an edit: same event, new text; a room we leave */
        assert!(!idx.knows("!a:hs", "$nothing") && idx.knows("!a:hs", "$a7") && !idx.knows("!b:hs", "$a7"));
        idx.insert_row(row("!a:hs", "$a7", "Alice", 1007, "first batch seven, edited"));
        idx.forget_room("!b:hs");
        idx.save().unwrap();
        let again = MessageIndex::open(dir.path(), "s").unwrap();
        assert_eq!(again.len(), 50, "the room that was forgotten stays forgotten");
        assert_eq!(again.search("edited", None, 10).len(), 1);
        assert!(again.search("first batch seven", None, 10).iter().all(|r| r.body.contains("edited")), "the old text of an edited message is gone");
        assert!(again.search("second batch", None, 10).is_empty());
        assert_eq!(again.search("batch 7", Some("!a:hs"), 10).len(), 4, "every word has to be in the text: 17, 27, 37 and 47 (7 itself was edited)");
        assert_eq!(again.search("Alice batch", Some("!a:hs"), 100).len(), 50, "the sender's name counts too");
    }

    #[test]
    fn many_segments_are_written_again_as_few_and_the_old_files_go() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        for round in 0..(MAX_SEGMENTS + 4) {
            for i in 0..5 { idx.insert_row(row("!a:hs", &format!("$r{round}e{i}"), "Alice", (round * 10 + i) as u64, &format!("round {round} message {i}"))); }
            idx.save().unwrap();
            assert!(segment_files(dir.path()).len() <= MAX_SEGMENTS + 1, "{:?}", segment_files(dir.path()));
        }
        assert!(segment_files(dir.path()).len() < MAX_SEGMENTS, "the files were written again as fewer: {:?}", segment_files(dir.path()));
        let again = MessageIndex::open(dir.path(), "s").unwrap();
        assert_eq!(again.len(), (MAX_SEGMENTS + 4) * 5);
        assert_eq!(idx.search("message", None, 1000).len(), again.search("message", None, 1000).len());
    }

    #[test]
    fn an_index_of_the_earlier_single_file_form_is_read_and_becomes_segments() {
        let dir = tempfile::tempdir().unwrap();
        let cipher = open_cipher(dir.path(), "index", "s").unwrap();
        let old = vec![row("!a:hs", "$1", "Alice", 10, "an old message"), row("!a:hs", "$2", "Bob", 20, "another old one")];
        std::fs::write(dir.path().join("index.enc"), cipher.encrypt_value(&old).unwrap()).unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        assert_eq!(idx.len(), 2);
        assert_eq!(idx.search("another", None, 5)[0].event_id, "$2");
        idx.save().unwrap();
        assert!(!dir.path().join("index.enc").exists(), "the old file is replaced");
        assert_eq!(segment_files(dir.path()).len(), 1);
        assert_eq!(MessageIndex::open(dir.path(), "s").unwrap().len(), 2);
    }

    #[test]
    fn segments_sealed_with_another_secret_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "right").unwrap();
        idx.insert_row(row("!a:hs", "$1", "Alice", 10, "secret words"));
        idx.save().unwrap();
        assert_eq!(segment_files(dir.path()).len(), 1);
        let other = MessageIndex::open(dir.path(), "wrong").unwrap();
        assert_eq!(other.len(), 0);
        assert!(segment_files(dir.path()).is_empty(), "what cannot be read is removed");
    }

    #[test]
    fn a_message_costs_about_two_hundred_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut idx = MessageIndex::open(dir.path(), "s").unwrap();
        for i in 0..20_000u64 {
            idx.insert_row(row(&format!("!room{}abcdefghijklmnop:example.org", i % 50), &format!("$e{i}abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJ"), &format!("Person {}", i % 40), i,
                &format!("message {i}: the quick brown fox jumps over the lazy dog, again and again until the line wraps")));
        }
        let per = idx.approx_bytes() / idx.len();
        assert!(per < 220, "{per} bytes a message: 150 of text, 32 of numbers and the table (it was 540 as strings in a map)");
        assert_eq!(idx.search("19999", None, 5).len(), 1);
        let hit = &idx.search("again", Some("!room3abcdefghijklmnop:example.org"), 1)[0];
        assert_eq!(hit.time, crate::ui::clock(hit.ts));
    }

    #[test]
    fn the_cipher_key_is_stretched_and_old_key_files_are_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        // a file written the old way (bare SHA-256 of the secret) still opens, and is rewritten
        let c = StoreCipher::new().unwrap();
        std::fs::write(dir.path().join("x-cipher.bin"), c.export_with_key(&old_key_of("pass")).unwrap()).unwrap();
        let sealed = c.encrypt_value(&"hello".to_string()).unwrap();
        let opened = open_cipher(dir.path(), "x", "pass").unwrap();
        assert_eq!(opened.decrypt_value::<String>(&sealed).unwrap(), "hello");
        let bytes = std::fs::read(dir.path().join("x-cipher.bin")).unwrap();
        assert!(StoreCipher::import_with_key(&old_key_of("pass"), &bytes).is_err(), "still sealed with the bare hash");
        assert!(StoreCipher::import("pass", &bytes).is_ok());
        assert_eq!(open_cipher(dir.path(), "x", "pass").unwrap().decrypt_value::<String>(&sealed).unwrap(), "hello");
        // a wrong secret starts over
        let fresh = open_cipher(dir.path(), "x", "other").unwrap();
        assert!(fresh.decrypt_value::<String>(&sealed).is_err());
    }

    async fn client_with_room(hs: &FakeHs, dir: &Path) -> (matrix_sdk::Client, matrix_sdk::Room) {
        let client = crate::session::sign_in(dir, &hs.uri(), "alice", "pw", None, "Vector").await.unwrap();
        crate::sync_once(&client).await.unwrap();
        let room = client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        (client, room)
    }

    /// What the SDK keeps in memory for a room: the events of its in-memory cache.
    async fn events_in_memory(room: &matrix_sdk::Room) -> usize {
        let (cache, _guard) = room.event_cache().await.unwrap();
        cache.events().await.unwrap().len()
    }

    /// Wait (the SDK shrinks a cache from a task of its own) until the room holds fewer than `limit` events in memory.
    async fn wait_for_fewer_than(room: &matrix_sdk::Room, limit: usize) -> usize {
        let mut held = events_in_memory(room).await;
        for _ in 0..60 { if held < limit { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; held = events_in_memory(room).await; }
        held
    }

    #[tokio::test]
    async fn a_crawled_room_and_a_room_nobody_opened_give_their_events_back() {
        use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
        let hs = FakeHs::start().await;
        hs.set_history(200);
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (alice, room) = client_with_room(&hs, da.path()).await;
        let index = std::sync::Mutex::new(MessageIndex::open(&da.path().join("index"), "s").unwrap());
        for _ in 0..10 { if crawl_room(&room, Some(&index), 3).await.unwrap() { break; } }
        let held = wait_for_fewer_than(&room, 150).await;
        assert!(held < 150, "the crawl pages 200 events through a timeline of its own: when that goes, the SDK shrinks the room's cache ({held} events still held)");

        /* Live events: the SDK adds every one of them to the cache of the room and, with nobody subscribed, never takes them out again. */
        let bob = crate::open_client(&hs.uri(), db.path(), "pw").await.unwrap();
        crate::login_password(&bob, "bob", "x", "bob's phone").await.unwrap();
        crate::sync_once(&bob).await.unwrap();
        let bob_room = bob.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
        for i in 0..300 {
            bob_room.send(RoomMessageEventContent::text_plain(format!("live message {i}"))).await.unwrap();
            if i % 10 == 9 { crate::sync_once(&alice).await.unwrap(); }
        }
        crate::sync_once(&alice).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let piled = events_in_memory(&room).await;
        {
            let (cache, _g) = room.event_cache().await.unwrap();
            let events = cache.events().await.unwrap();
            let json: usize = events.iter().map(|e| e.raw().json().get().len()).sum();
            println!("events held in memory for a room nobody opened, after 300 live messages: {piled} ({} bytes of event JSON, {} bytes per TimelineEvent itself)", json, std::mem::size_of::<matrix_sdk::deserialized_responses::TimelineEvent>());
        }
        assert!(piled >= 300, "the SDK keeps every live event of a room without subscribers ({piled}); if it shrinks them by itself now, `shrink_event_caches` is not needed any more");

        /* the open room is left alone, the others are asked */
        let id = room.room_id().to_owned();
        assert_eq!(crate::ui::shrink_event_caches(&alice, &[id.clone()], Some(id.as_str())).await, 0);
        assert_eq!(crate::ui::shrink_event_caches(&alice, &[id.clone()], None).await, 1);
        let held = wait_for_fewer_than(&room, 200).await;
        println!("events held in memory after the shrink: {held}");
        assert!(held < 200, "{held} events still held after the shrink");
        /* what was dropped is still there for the timeline: it comes from the store */
        let timeline = crate::ui::open_timeline(&room).await.unwrap();
        let _ = timeline.subscribe().await;
        for _ in 0..40 { if timeline.items().await.len() >= 20 { break; } tokio::time::sleep(std::time::Duration::from_millis(50)).await; }
        assert!(timeline.items().await.iter().filter_map(|i| i.as_event()).any(|e| e.content().as_message().map_or(false, |m| m.body().starts_with("live message"))));
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
        for _ in 0..3 { if crawl_room(&room, Some(&index), 1).await.unwrap() { reached = true; break; } }
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
        let sealed = std::fs::read(idx_dir.join("seg-000001.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&sealed).contains("quick brown fox"), "the file is sealed");
        let again = MessageIndex::open(&idx_dir, "secret").unwrap();
        assert_eq!(again.search("quick brown", None, 10).len(), 1, "it comes back with the same secret");
        let other = MessageIndex::open(&idx_dir, "wrong secret").unwrap();
        assert_eq!(other.len(), 0, "a wrong secret opens an empty index, not the old text");
    }
}
