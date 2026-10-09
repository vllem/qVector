# AFL fuzzing (afl.rs)

Separate crate (not in the workspace). Targets: `decode_vp8`, `decode_h264`, `misc` (key-frame sniffing, picture fitting, emoji search, MIME parsing).

    cargo install cargo-afl && cargo afl config --build        # once
    cd core-rs/fuzz-afl && cargo afl build --release           # slow the first time (whole matrix-sdk tree)
    AFL_SKIP_CPUFREQ=1 cargo afl fuzz -i seeds/vp8 -o out/vp8 target/release/decode_vp8

Crashes land in `out/<target>/default/crashes/`; replay with `target/release/<bin> < crashfile`. `out/` is git-ignored.
