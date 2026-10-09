//! Bytes from the network go straight into the VP8 decoder (libvpx): it must never crash, hang or run out of memory.
use vector_core::video::{Codec, Decoder};

fn main() {
    afl::fuzz!(|data: &[u8]| {
        let mut d = Decoder::new(Codec::Vp8).unwrap();
        // the input is cut into several "packets" so the decoder state after bad data is exercised too
        for chunk in data.chunks(1 + data.first().copied().unwrap_or(64) as usize * 4) {
            if let Some(f) = d.decode(chunk) {
                assert!(f.w as usize * f.h as usize * 4 == f.rgba.len());
            }
        }
    });
}
