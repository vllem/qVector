# qVector

**qVector** is a [Matrix](https://matrix.org) desktop client: a Qt6 Widgets interface laid out like [Ripcord](https://cancel.fm/ripcord/) (menu bar, a sidebar of
workspaces / direct messages / channels with unread badges, a tab strip of open rooms, a dense IRC-style timeline) on a Rust core built on
[matrix-sdk](https://github.com/matrix-org/matrix-rust-sdk) (end-to-end encryption by vodozemac / matrix-sdk-crypto).

```
core-rs/   the Rust engine (crate vector-core): session, sync, rooms, timeline, E2EE, verification, recovery, search index, ...
           app.rs = everything a UI needs behind call(method, json) + events; capi.rs / vector_app.h = its C ABI; testkit.rs = a fake homeserver
qt/        the Qt6 Widgets front end (C++17): it only draws what the engine sends and turns clicks into engine calls
```

## Features

Sign in with a homeserver URL or bare domain (with an optional passphrase for the saved session), rooms, direct messages, invitations, spaces as sidebar
sections, favourites / low priority, history paging, formatted messages (Markdown out, sanitized HTML in), replies, reactions with an emoji picker, edits,
deletes, forwarding, edit history, threads (own panel), polls, pinned messages, saved messages, pictures / files / video / audio (sending with encrypted upload,
viewer, save, inline video), link previews (through your homeserver, opt-in), read receipts and typing indicators (both ways), desktop notifications and a tray
icon, search of the open room and (opt-in, with a local encrypted index) of all your messages, member list with moderation (kick, ban, unban, roles, invite),
room settings, start a conversation (user directory search), create / browse / join rooms, avatars, light and dark theme, per-room notification levels (right-click a
room), online status dots in the member list (and Tools > My status), exploring the rooms of a space (right-click its section header), custom emoji and stickers
(MSC2545 packs of the room, of your account and of the rooms you chose).

Encryption: messages are end-to-end encrypted with who-wrote-this shields; interactive emoji verification of this session and of other people; recovery key
entry and creation (secret storage and key backup); the list of your account's sessions; QR codes for the other device to scan (scanning with this computer's camera is not built).

Not ported yet from the earlier all-C version: voice and video calls (and tweet pictures, sticker pack editing, scanning a QR code with a camera). Everything has only been tested against the built-in fake homeserver so far, not against a real server.
This is a hobby project and has not been audited; do not rely on it for anything sensitive.

## Building

Requirements: a Rust toolchain (>= 1.96, [rustup](https://rustup.rs)), CMake >= 3.20 and a C++17 compiler, Qt 6 Widgets / Multimedia / MultimediaWidgets
(optionally WebEngine). On Debian / Ubuntu:

```sh
sudo apt install build-essential cmake pkg-config qt6-base-dev qt6-multimedia-dev qt6-image-formats-plugins   # + qt6-webengine-dev
cmake -S . -B build-release -DCMAKE_BUILD_TYPE=Release && cmake --build build-release -j"$(nproc)"
./build-release/qvector
```

The first build compiles matrix-sdk (about 15 minutes in release mode). Development builds (`cmake -S . -B build && cmake --build build`) use cargo's dev profile
and the repository's `target/` directory.

Try the interface without an account or network (a fake homeserver with a few rooms and people):

```sh
./build-release/qvector --demo
QT_QPA_PLATFORM=offscreen ./build-release/qvector --demo --room bob --screenshot out.png     # without opening a window on your desktop
```

Dev aids: `--room NAME`, `--members`, `--search TEXT`, `--thread TEXT`, `--dialog poll|saved|prefs|verify|recovery|settings`, `--delay MS`, `--data DIR`,
`--server URL --user NAME --password PW` (sign in from the command line), `--verify` / `--confirm`. Set `-DVC_DEMO=OFF` to leave the fake homeserver out.

### Tests

```sh
cargo test -p vector-core --lib --features testkit      # the engine against the fake homeserver, several devices (about 5 minutes)
scripts/ui_smoke.sh build/qvector                        # every window and dialog, offscreen
```

## Where things are

Your session is stored in `~/.local/share/vector/vector/account` (the SDK's sqlite store and the session, sealed with a random key file or your passphrase); the
local message index, saved messages and downloaded media are there too. Treat that directory like a password store. `scripts/gen_emoji.py` regenerates the
picker's data (`qt/emoji_data.h`), `scripts/gen_emoji_json.py` the engine's (`core-rs/src/emoji.json`).

## License

[MIT](LICENSE). Third-party: [Noto Color Emoji](https://github.com/googlefonts/noto-emoji) (SIL Open Font License 1.1, `third_party/fonts/OFL.txt`) is used for emoji when the
system has no colour emoji font. Qt is used under its LGPL terms (dynamic linking).
