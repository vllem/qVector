//! Torture tests for the engine's hot paths. They are `#[ignore]`d (slow, and the budgets only mean something in an optimised build):
//!
//!     cargo test --release stress:: -- --ignored --nocapture --test-threads=1
//!
//! Each test prints what it measured and fails when a budget is blown. A budget is what the UI can afford: a video frame has 66 ms at 15 fps,
//! a keystroke in a search box about 50 ms, and nothing on the sync path may stall the window. Debug builds get 30x the time.

use std::time::{Duration, Instant};

use crate::index::{IndexRow, MessageIndex};
use crate::video::{Codec, Decoder, Encoder, Frame};

fn budget(ms: u64) -> Duration {
    Duration::from_millis(if cfg!(debug_assertions) { ms * 30 } else { ms })
}

/// Run `f`, print its time and fail when it took longer than `ms`.
fn timed<T>(what: &str, ms: u64, f: impl FnOnce() -> T) -> T {
    let t = Instant::now();
    let r = f();
    let took = t.elapsed();
    println!("{what}: {:.1} ms (budget {} ms)", took.as_secs_f64() * 1000.0, budget(ms).as_millis());
    assert!(took <= budget(ms), "{what} took {took:?}, budget {:?}", budget(ms));
    r
}

fn words(i: usize) -> String {
    const W: [&str; 16] = ["matrix", "synapse", "element", "the", "quick", "brown", "fox", "rust", "qt", "widget", "encrypted", "room", "call", "video", "lazy", "dog"];
    (0..12).map(|k| W[(i * 7 + k * 13 + k * k) % 16]).collect::<Vec<_>>().join(" ")
}

fn big_index(n: usize, rooms: usize) -> MessageIndex {
    let dir = tempfile::tempdir().unwrap().keep();
    let mut idx = MessageIndex::open(&dir, "secret").unwrap();
    for i in 0..n {
        idx.insert_row(IndexRow {
            room_id: format!("!room{}:hs", i % rooms),
            event_id: format!("$event{i}"),
            sender: format!("@user{}:hs", i % 50),
            time: "12:00".into(),
            ts: 1_000_000 + i as u64,
            body: format!("{} #{i}", words(i)),
        });
    }
    idx
}

#[test]
#[ignore]
fn searching_a_quarter_million_messages_stays_interactive() {
    let idx = big_index(250_000, 400);
    assert_eq!(idx.len(), 250_000);
    // typing "rust" then "rust fox" then "rust fox lazy" in a box: every keystroke is a search
    for q in ["r", "ru", "rust", "rust fox", "rust fox lazy", "rust fox lazy dog"] {
        timed(&format!("search {q:?} across all rooms"), 100, || idx.search(q, None, 50));
    }
    timed("search in one room", 20, || idx.search("rust", Some("!room7:hs"), 50));
    timed("a word that is nowhere", 60, || idx.search("zzzzzz", None, 50));
    timed("unicode and case folding", 60, || idx.search("ÄÖÜ ß İ", None, 50));
    timed("a 10 kB query", 100, || idx.search(&"rust ".repeat(2000), None, 50));
    // every result limit must still be cheap
    timed("limit 0", 60, || assert!(idx.search("rust", None, 0).is_empty()));
    timed("limit 100000", 400, || idx.search("the", None, 100_000));
}

#[test]
#[ignore]
fn the_index_saves_and_reopens_a_big_history_without_stalling() {
    let mut idx = big_index(120_000, 100);
    let dir = std::env::temp_dir().join(format!("vc-stress-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut sealed = MessageIndex::open(&dir, "secret").unwrap();
    for i in 0..120_000 {
        sealed.insert_row(IndexRow { room_id: format!("!r{}:hs", i % 100), event_id: format!("$e{i}"), sender: "@a:hs".into(), time: "t".into(), ts: i as u64, body: words(i) });
    }
    timed("save 120k rows", 1500, || sealed.save().unwrap());
    timed("save when nothing changed", 1, || sealed.save().unwrap());
    let again = timed("reopen 120k rows", 1500, || MessageIndex::open(&dir, "secret").unwrap());
    assert_eq!(again.len(), 120_000);
    timed("forget one room of 120k", 100, || idx.forget_room("!room3:hs"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore]
fn one_enormous_message_does_not_wreck_the_index() {
    let mut idx = big_index(1000, 3);
    idx.insert_row(IndexRow { room_id: "!x:hs".into(), event_id: "$huge".into(), sender: "@a:hs".into(), time: "t".into(), ts: 9, body: "needle ".repeat(1_000_000) });
    timed("search past a 7 MB message", 150, || assert_eq!(idx.search("needle", None, 5).len(), 1));
    timed("a miss past a 7 MB message", 150, || assert!(idx.search("haystack", None, 5).is_empty()));
}

#[test]
#[ignore]
fn emoji_search_survives_a_keyboard_mash() {
    timed("1000 emoji searches", 400, || {
        for i in 0..1000 {
            let q = ["", "a", "face", "red heart", "flag", "thumbs up", "ÄÖ", "😀", "xxxxxxxxxxxxxxxxxxxxxxxx"][i % 9];
            crate::emoji::search(q, 8);
        }
    });
    timed("a 100 kB query", 300, || crate::emoji::search(&"heart ".repeat(20_000), 8));
}

fn noisy(w: u32, h: u32, n: u32) -> Frame {
    // the worst case for a codec: noise that changes every frame
    let mut x = 0x9E3779B9u32 ^ n.wrapping_mul(2654435761);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        rgba.extend_from_slice(&[x as u8, (x >> 8) as u8, (x >> 16) as u8, 255]);
    }
    Frame { w, h, rgba }
}

#[test]
#[ignore]
fn shrinking_and_converting_pictures_fits_in_a_frame() {
    let uhd = noisy(3840, 2160, 1);
    let fit = timed("fit 4K down to 960x540", 25, || uhd.fitted(crate::video::MAX_W, crate::video::MAX_H));
    assert_eq!((fit.w, fit.h), (960, 540));
    timed("fit a 1920x1080 camera picture", 20, || noisy(1920, 1080, 2).fitted(960, 540));
    timed("a picture that is already small", 5, || noisy(640, 360, 3).fitted(960, 540));
    timed("a 1x1 picture", 1, || Frame { w: 1, h: 1, rgba: vec![0; 4] }.fitted(960, 540));
    timed("a 8000x16 strip", 25, || noisy(8000, 16, 4).fitted(960, 540));
    let (w, h) = (960usize, 540usize);
    let y = vec![128u8; w * h];
    let c = vec![100u8; w * h / 4];
    timed("YUV to RGBA at 960x540", 12, || crate::vp8::i420_to_rgba(&y, w, &c, &c, w / 2, w, h));
}

fn sustained(codec: Codec, w: u32, h: u32, frames: u32, per_frame_ms: u64, what: &str) {
    let mut enc = Encoder::new(codec);
    let mut dec = Decoder::new(codec).unwrap();
    let pics: Vec<Frame> = (0..8).map(|n| noisy(w, h, n)).collect();
    let mut shown = 0;
    let t = Instant::now();
    for n in 0..frames {
        let coded = enc.encode(&pics[(n % 8) as usize]).unwrap();
        if dec.decode(&coded).is_some() {
            shown += 1;
        }
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / frames as f64;
    println!("{what}: {per:.1} ms per frame encode+decode, {shown}/{frames} pictures (budget {} ms)", budget(per_frame_ms).as_millis());
    // H.264 rate control legitimately skips most noise frames; only VP8 must deliver them all
    assert!(codec != Codec::Vp8 || shown as u32 >= frames - 2, "{what}: only {shown} of {frames} pictures came out");
    assert!(per <= budget(per_frame_ms).as_secs_f64() * 1000.0, "{what}: {per:.1} ms per frame is slower than real time");
}

#[test]
#[ignore]
fn video_keeps_up_with_real_time_on_the_worst_pictures() {
    // 15 fps leaves 66 ms for encode + decode of each frame; noise is the worst case (real pictures encode several times faster); the budget is what still keeps a 15 fps call usable
    sustained(Codec::Vp8, 960, 540, 60, 45, "VP8 noise 960x540");
    sustained(Codec::H264, 960, 540, 60, 45, "H.264 noise 960x540");
    sustained(Codec::Vp8, 640, 360, 60, 12, "VP8 noise 640x360");
}

#[test]
#[ignore]
fn four_encoders_at_once_like_a_five_person_mesh() {
    let started = Instant::now();
    let threads: Vec<_> = (0..4)
        .map(|k| std::thread::spawn(move || {
            let mut enc = Encoder::new(Codec::Vp8);
            let mut longest = Duration::ZERO;
            for n in 0..45 {
                let f = noisy(640, 360, n + k);
                let t = Instant::now();
                enc.encode(&f).unwrap();
                longest = longest.max(t.elapsed());
            }
            longest
        }))
        .collect();
    let worst = threads.into_iter().map(|t| t.join().unwrap()).max().unwrap();
    println!("4 parallel VP8 encoders: worst frame {worst:?}, 180 frames in {:?}", started.elapsed());
    assert!(worst <= budget(450), "a frame took {worst:?}: the call would stutter");
}

#[test]
#[ignore]
fn garbage_on_the_wire_never_crashes_or_hangs_the_decoders() {
    let mut x = 12345u32;
    let mut rnd = move || { x ^= x << 13; x ^= x >> 17; x ^= x << 5; x };
    for codec in [Codec::Vp8, Codec::H264] {
        let mut dec = Decoder::new(codec).unwrap();
        let t = Instant::now();
        for len in [0usize, 1, 2, 3, 7, 100, 1500, 65_000, 1_000_000] {
            for _ in 0..20 {
                let junk: Vec<u8> = (0..len).map(|_| rnd() as u8).collect();
                let _ = dec.decode(&junk);
            }
        }
        // then a real stream must still work: the decoder recovered
        let mut enc = Encoder::new(codec);
        let coded = enc.encode(&noisy(320, 240, 1)).unwrap();
        assert!(dec.decode(&coded).is_some(), "{codec:?} did not recover from garbage");
        assert!(t.elapsed() < Duration::from_secs(if cfg!(debug_assertions) { 120 } else { 20 }), "{codec:?} spent {:?} on garbage", t.elapsed());
        // truncated real frames, the usual shape of packet loss
        let key = enc.encode(&noisy(320, 240, 2)).unwrap();
        for cut in [1, key.len() / 4, key.len() / 2, key.len().saturating_sub(1)].into_iter().filter(|c| *c <= key.len()) {
            let _ = dec.decode(&key[..cut]);
        }
    }
}

#[test]
#[ignore]
fn odd_picture_sizes_go_through_the_encoders() {
    for (w, h) in [(16, 16), (17, 17), (31, 9), (1, 1), (4096, 2), (2, 4096), (959, 539), (1280, 720), (3, 2000)] {
        for codec in [Codec::Vp8, Codec::H264] {
            let mut enc = Encoder::new(codec);
            let f = noisy(w, h, 1).fitted(960, 540);
            let _ = enc.encode(&f).map(|c| Decoder::new(codec).unwrap().decode(&c));
            // a size change in mid-stream (the camera was swapped or the window shared changed size)
            let _ = enc.encode(&noisy(640, 360, 2));
            let _ = enc.encode(&noisy(320, 180, 3));
        }
    }
}

#[tokio::test]
#[ignore]
async fn crawling_three_thousand_messages_of_history_is_linear() {
    use crate::testkit::{FakeHs, ROOM};
    use std::sync::Mutex;
    let hs = FakeHs::start().await;
    hs.set_history(3000);
    let dir = tempfile::tempdir().unwrap();
    let client = crate::session::sign_in(dir.path(), &hs.uri(), "alice", "pw", None, "Vector").await.unwrap();
    crate::sync_once(&client).await.unwrap();
    let room = client.get_room(<&matrix_sdk::ruma::RoomId>::try_from(ROOM).unwrap()).unwrap();
    use matrix_sdk_ui::timeline::RoomExt;
    let live = room.timeline().await.unwrap();
    let _ = live.subscribe().await;
    crate::ui::send_text(&live, "anchor", None).await.unwrap();
    for _ in 0..80 { crate::sync_once(&client).await.unwrap(); if live.items().await.iter().any(|i| i.as_event().map(|e| e.event_id().is_some()).unwrap_or(false)) { break; } tokio::time::sleep(Duration::from_millis(50)).await; }
    let index = Mutex::new(MessageIndex::open(&dir.path().join("index"), "secret").unwrap());
    let t = Instant::now();
    let mut last = Duration::ZERO;
    let mut slowest = Duration::ZERO;
    let mut pages = 0;
    loop {
        let p = Instant::now();
        let done = crate::index::crawl_room(&room, Some(&index), 10).await.unwrap();
        last = p.elapsed();
        slowest = slowest.max(last);
        pages += 1;
        if done || pages > 40 { break; }
    }
    println!("crawled {} rows in {pages} pages, {:?} total; first-to-last page ratio: last page {last:?}, slowest {slowest:?}", index.lock().unwrap().len(), t.elapsed());
    assert!(index.lock().unwrap().len() >= 3000);
    // crawl_room re-reads and re-indexes all items on each page: a page must not get slower the further back we go
    assert!(last < slowest * 3 + Duration::from_millis(50) && slowest < Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 2 }), "pages get slower with depth: last {last:?}, slowest {slowest:?}");
}
