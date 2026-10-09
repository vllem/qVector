//! Small pure functions that take outside data: key-frame detection, picture fitting, emoji search.
use vector_core::video::{has_key, Codec, Frame};

fn main() {
    afl::fuzz!(|data: &[u8]| {
        has_key(data);
        Codec::Vp8.is_key(data);
        Codec::H264.is_key(data);
        if let Ok(s) = std::str::from_utf8(data) {
            let _ = vector_core::emoji::search(s, 20);
            let _ = Codec::from_mime(s);
        }
        if data.len() >= 6 {
            let (w, h) = (u16::from_le_bytes([data[0], data[1]]) as u32 % 2048, u16::from_le_bytes([data[2], data[3]]) as u32 % 2048);
            let (mw, mh) = (data[4] as u32, data[5] as u32);
            let f = Frame { w: w.max(16), h: h.max(16), rgba: vec![7; w.max(16) as usize * h.max(16) as usize * 4] };
            let g = f.fitted(mw, mh);
            assert!(g.is_valid());
        }
    });
}
