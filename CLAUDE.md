# qVector: notes for the next engineer (human or LLM)

A Matrix desktop client: a **Rust engine** (`core-rs/`, crate `vector-core`, on matrix-sdk 0.19.1) with a **Qt6 Widgets (C++17) front end** (`qt/`) laid out like
Ripcord. Desktop/Linux only (macOS/Windows sources exist, nobody has run them). Read `README.md` for the user-facing feature list and build steps; this file is
what is *not* obvious.

Naming: the project is called **qVector** (window title, executable `qvector`, desktop file, packaging). Deliberately unchanged: the Qt application/organization name `vector`
(so the saved session under `~/.local/share/vector/vector/account` and the QSettings keep working), the crate `vector-core`, the `vcr_*` C ABI and `vector_app.h`.

History: this repository was split out of the original all-C99 Vector client (kept in its own repository, the user's `VectorChat` folder). The C core was replaced
by matrix-sdk; an intermediate QML/QtBridge front end was tried and dropped (the user found it ugly: "use the old Qt look"); what is here is the old Widgets UI ported
onto the Rust engine. The history of the port lives in the old repository's `rust-crypto` branch; this one starts from its final state.

## Layers

```
qt/                  Widgets UI. Core (core.h) wraps the engine; MainWindow dispatches engine events; TimelineView draws rich text from JSON rows
core-rs/src/capi.rs  C ABI: vcr_app_new / vcr_app_call(method, json) / vcr_string_free / vcr_app_free + one event callback (header: core-rs/vector_app.h)
core-rs/src/app.rs   the engine: App owns a tokio runtime; call() starts work, results come back as events (name, json) on any thread
core-rs/src/ui.rs    UI-facing functions on top of matrix-sdk (rooms, messages, actions) with their tests; crypto.rs verification/recovery/sessions;
                     index.rs / bookmarks.rs sealed local files; avatars.rs; emoji.rs; session.rs the saved session; testkit.rs the fake homeserver
```

* **New behaviour belongs in `core-rs` with a test; the Qt layer only presents it.** Qt never touches matrix-sdk types: it sends `call("method", {args})` and reads
  events. `Core::event(name, payload)` is emitted on the GUI thread (the C callback runs on engine threads and is queued over).
* Events: `state` {screen: login|unlock|main, status, busy}, `rooms` (UiRoom list, already grouped/ordered: Invites, Favourites, DMs, spaces, Rooms, Low priority),
  `timeline` {room_id, rows} (UiMessage rows), `details` (name/topic/members/permissions), `typing`, `older`, `notice` (plain text), `session` {has_identity, verified,
  recovery}, `verification` {state: idle|incoming|waiting|emoji|confirmed|done|cancelled, user, emoji...}, `recovery`, `sessions`, `thread`, `bookmarks`, `search`,
  `users`, `directory`, `edit_history`, `open_file` / `media_file`, `alerts`. The authoritative list of methods is the `match` in `App::call`.
* The engine is single-room: `select_room` re-subscribes the open timeline; one thread timeline at a time. Per-tab state in the UI is only the room id.
* Everything the SDK owns (sync, E2EE, timeline aggregation of edits/reactions/replies/threads/polls, event cache) must stay the SDK's job. Do not reintroduce a
  hand-written protocol layer.

## Build, run, test

```sh
cmake -S . -B build && cmake --build build -j8                  # dev: cargo dev profile in the repo's target/, Qt from the system (6.8)
cmake -S . -B build-release -DCMAKE_BUILD_TYPE=Release && cmake --build build-release -j8
cargo test -p vector-core --lib --features testkit              # 46 tests, ~5 min; a parallel-load flake in the reply/shield tests is known (UTD retry window)
scripts/ui_smoke.sh build/qvector                                # every window/dialog offscreen
QT_QPA_PLATFORM=offscreen XDG_CONFIG_HOME=$(mktemp -d) ./build/qvector --demo --data $(mktemp -d) --room bob --screenshot out.png
```

* **Do not set LD_LIBRARY_PATH to ~/Qt** (a Qt 6.10 install used by the dropped QtBridge experiment): the Widgets app uses the system Qt 6.8.
* `--demo` starts the in-process fake homeserver (feature `testkit`, CMake option `VC_DEMO`) with alice (you), bob, a space, an invite and 40 history messages.
  Dev aids: `--room NAME --members --search T --thread T --dialog poll|saved|prefs|verify|recovery|settings --delay MS --data DIR --server URL --user U --password P
  --verify --confirm`. Screenshots grab the main window (or the dialog being demonstrated); popups/menus are not captured.
* A real run signs in a *new device* on the account and needs re-verification: ask the user before pointing it at their real account. Use `core-rs/examples/fakehs.rs`
  (`cargo run -p vector-core --features testkit --example fakehs`) for a standalone fake server with alice's other session (it prints a recovery key).
* Use a throw-away config/data dir when running the app for tests: `XDG_CONFIG_HOME=$(mktemp -d)` (QSettings writes drafts, themes, tray choices) and `--data`.
* The desktop is shared: never kill processes with `pkill -f <pattern>`; do not open windows on the user's display (offscreen only).

## The fake homeserver (`core-rs/src/testkit.rs`)

wiremock-based, several users/devices: login, keys upload/query/claim/signatures/device_signing (optional UIA password), to-device, send/redact/state (state PUT is
echoed through the sync), members, messages (`set_history(n)`), media (+ seeded avatars), account data, key backup, receipts, typing, tags, spaces, invites,
createRoom/join/leave, user directory, public rooms, link previews. A request it does not know is logged as `UNHANDLED`; error JSON with `errcode` becomes 4xx.
Add endpoints there with a test in `ui.rs`/`crypto.rs`. Every fake server must use its own temp dirs (parallel tests shared one once and flaked). `ROOM` doubles as
the alice/bob direct chat (`m.direct`).

## Matrix behaviour learned the hard way

* The SDK handles relations in encrypted rooms, replies (no quoted fallback, `m.mentions`), threads, edits; test with the fake before assuming it is wrong.
* Verification: emoji (SAS) works to-device for our own sessions and in the direct chat for other people; after a verification the other side's identity has to be
  re-queried (`request_user_identity`) before it shows as trusted. `Verifier::dismiss` once deadlocked (lock taken twice in one statement): never hold two locks in one
  expression.
* `enable_recovery` does not bootstrap cross-signing; do it first, and retry with the password only after the server asked (a failed first attempt already created keys
  locally, so use `bootstrap_cross_signing`, not the "if needed" variant).
* Pagination reports "start reached" one call late with the fake; unread/notification counts come from the server; read receipts only while the window has focus.
* Qt Multimedia IS available (system Qt); WebEngine is optional (`VC_HAVE_WEBENGINE`).
* Servers differ: media may need the authenticated `/_matrix/client/v1/media` endpoints with a fallback to the legacy ones; link previews use the same fallback
  (`link_preview`).

## Reference clients and licences

* Cinny (https://github.com/cinnyapp/cinny) is the user's favourite client and a good source of *behaviour* to test against. It is **AGPL-3.0: do not copy its code or
  its regular expressions** into this MIT project; read what it does and write our own implementation and tests.

## Qt pitfalls

* `QTextBrowser` rendering: messages are HTML built in `TimelineView::render()` (one small table per message, rows mapped to events in `rowEvents_` for the context menu).
  Pictures/avatars/shields are `QTextDocument` resources. Performance: only the newest 100 messages are laid out (`shownLimit_`), redraws are skipped when html and
  pictures are unchanged, bursts are merged (150 ms), and a change at the end of the room replaces only the tables from the first changed one (partial update).
* The default font family is the alias "Sans Serif"; `main.cpp` resolves the real family before adding the colour emoji font (`ensureEmojiFont`). Symbols that exist as
  emoji (◀ ▶ ☺) are written with U+FE0E. The user dislikes coloured-emoji look-alikes for plain symbols and layout glitches: look at a screenshot before reporting done.
* `QLayout::takeAt()` on a nested layout returns the layout itself: delete it once, not "the item" too. Popups must be clamped to the screen (`EmojiPicker::popupAt`).
* Offscreen screenshots cannot show video surfaces (`QVideoWidget`); everything else renders.
* Do not add `<unistd.h>`, `getpid`, `/tmp` or D-Bus uses outside the platform seams (`qt/notifier.cpp` is D-Bus on Linux, tray messages elsewhere); use Qt APIs.

## Working with the user (important)

* The user tests against real accounts and clients (Element, Cinny) and reports concrete symptoms with logs. Real interop is the acceptance bar; nothing here has been
  exercised against a real server yet.
* Short status reports: what changed, where, the exact command to run. Commit after each working piece with a clear message and the `Co-Authored-By` line given in your
  system prompt. Do not push; the user does that from VS Code.
* Git history was rewritten once to remove private data; do not add logs, certificates, tokens or account identifiers to the repo. The commit identity is neutral.
* Disk is tight: keep one shared Cargo target dir (`target/`), `cargo clean` if it fills up; `build*/` and `target/` are ignored.

## What is done and what is not

Done (engine tests + fake server only): see README. New engine modules: `emotes.rs` (MSC2545 packs, `:shortcode:` -> `<img data-mx-emoticon>`, stickers). Notification levels use the SDK's
`NotificationSettings` (the fake serves `/pushrules`), presence is asked per member after the details (`presence_of`, one request each, max 150), space exploring uses
`/hierarchy` (`space_rooms`), voice calls are `rtc_peer.rs` (webrtc-rs 0.21: one Opus audio connection, SDP in and out, candidates gathered before the invite/answer), `calls.rs` (Matrix VoIP v1 over `room.send_raw`, a raw `m.call.*` event handler, a 60 s ring, busy and answered-elsewhere handling) and `calls_audio.rs` (cpal; a thread owns the streams); the sound card is an injectable `AudioFactory`, so the two-user test in `calls.rs` uses a tone and a counter. The final link needs libopus and ALSA (`pkg-config` in CMakeLists.txt). NOTE: a plain `cargo build` in the same `target/` replaces the testkit library that CMake builds, and the demo then shows only the login page; `touch core-rs/src/lib.rs` and rebuild through cmake.  QR verification is `Verifier::with_qr` (needs the peer to advertise `m.qr_code.scan.v1`, as phones do: the SDK itself only shows codes).
Not done / ideas: video calls, group calls (Element Call / MatrixRTC uses LiveKit, not plain WebRTC), a ring tone, choosing the audio devices, scanning a QR code with a camera, a sticker /
emoji pack editor, tweet pictures, identity-change banner and per-user trust state beyond the shield on messages, resetting cross-signing, sign-out of other sessions,
location messages, spoilers, (image paste, drag-and-drop with a caption bar and multi-picture galleries are done),
notification text with sender and preview (alerts only carry a count), packaging (AppImage/.deb), a real-server run of everything.
