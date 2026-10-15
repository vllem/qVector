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
cargo test -p vector-core --lib --features testkit              # 109 tests, ~5 min (10 more are `#[ignore]`d stress tests: `-- --ignored`)
scripts/ui_smoke.sh build/qvector                                # every window/dialog offscreen
QT_QPA_PLATFORM=offscreen XDG_CONFIG_HOME=$(mktemp -d) ./build/qvector --demo --data $(mktemp -d) --room bob --screenshot out.png
```

* **Do not set LD_LIBRARY_PATH to ~/Qt** (a Qt 6.10 install used by the dropped QtBridge experiment): the Widgets app uses the system Qt 6.8.
* `--demo` starts the in-process fake homeserver (feature `testkit`, CMake option `VC_DEMO`) with alice (you), bob, a space, an invite, 40 history messages and a photo from bob (`--dialog picture` opens it as a click does, `--dialog nothumbs` clears the Preferences checkbox).
  Dev aids: `--room NAME --members --search T --thread T --dialog poll|saved|prefs|verify|recovery|settings --delay MS --data DIR --server URL --user U --password P
  --verify --confirm`. Screenshots grab the main window (or the dialog being demonstrated); popups/menus are not captured.
* A real run signs in a *new device* on the account and needs re-verification: ask the user before pointing it at their real account. Use `core-rs/examples/fakehs.rs`
  (`cargo run -p vector-core --features testkit --example fakehs`) for a standalone fake server with alice's other session (it prints a recovery key).
* Use a throw-away config/data dir when running the app for tests: `XDG_CONFIG_HOME=$(mktemp -d)` (QSettings writes drafts, themes, tray choices) and `--data`.
* The desktop is shared: never kill processes with `pkill -f <pattern>`; do not open windows on the user's display (offscreen only).

## Measuring and the sync loop

* `perf` is blocked here (`perf_event_paranoid=3`, no sudo), so flamegraph/samply do not work. What does: `strace -c -f -p PID` (syscalls; add `-e trace=write,writev,sendto -s 120`
  and grep `GET|POST|PUT` for the requests a client makes), `pidstat -t -p PID 15 1` (CPU per thread), `heaptrack` on a **dev** build (the stripped release build gives
  misleading leaf symbols), sampling with `gdb -p PID -batch -ex 'thread apply all bt 30'` in a loop, and the stress suites (`core-rs/src/stress.rs`, `qvector_stress`).
  Compare builds of the same kind only (a dev build uses 2-3x the CPU of a release build). Find the real PID: `timeout` and `setsid` leave wrapper processes.
* `app.rs` `sync_loop` is two halves in one task: `network_loop` long-polls `/sync` (30 s, at least 700 ms apart; ended session and back-off handled there) and
  `housekeeping` (rooms event, avatars, crawler, alerts, key restore, session status) runs when a sync returned or `Inner::wake` is poked, and every 5 s. Anything that must
  show up without a sync (a recovery, a verification) has to poke `wake`: `Inner::emit` does it for the `verification`, `recovery` and `sessions` events, which also ask for our own identity
  again (`identity_due`; otherwise every 10 minutes: it is a `/keys/query`). Do not put work back on a fixed timer. The `rooms` event is sent only when the list changed.
* The fake homeserver holds an idle `/sync` that asks for a `timeout` for up to 1.5 s (the tests' own syncs ask for none), so the demo behaves like a long-polling server.
* The CMake cache directories (`build`, `build-perf`, `build-release`) remember the checkout they were configured in; after moving the repository delete and configure them again.
* Known: with the default `--delay` an unoptimised demo may exit while the simulated user "bob" (`testkit::bob_says`) is still opening its client: a panic message from the SDK's
  store on stderr at exit, nothing to do with the app; the smoke script uses a long delay.

## Memory (what costs what, and what keeps it down)

Measured with `qvector_stress` (the steps called `memory: ...`: resident anonymous memory after a settle and `malloc_trim(0)`, each with a budget that is
multiplied by the display scale squared: run it with `QT_SCALE_FACTOR=2` too), with heaptrack on a dev build for the engine, and with a counting
`#[global_allocator]` in a scratch example for data structures (not committed; the numbers are in the commit messages). The demo is too small to show most
of this: the stress steps make the pictures, rooms and people themselves (real JPEG/PNG files of 4000x3000 and 512x512, `QFile::link` to count them as many).
The figures below and the budgets are for `qvector_stress memory` run on its own (a fresh process); in a full run the memory steps come last and start from a warm
heap that reuses freed memory, so they read lower (a regression of 10 MB in the sidebar would pass there; gross ones still do not).

* Pictures: never keep a decode of a file bigger than what is drawn. `vc::loadFitted` (`qt/imageload.h`) decodes straight to the draw size (JPEG, WebP; PNG is
  decoded in full and shrunk). The timeline keeps its finished pictures in a `QCache` by byte cost whose limit follows what the last redraw used
  (`TimelineView::drawn_`), avatars one 32/64 px square per file (`avatarSquare`). A `QHash` of decoded files that nothing empties was +727 MB for ten rooms with
  14 photos, +244 MB for 600 sidebar avatars, +1.2 GB for 3 000 members; the same steps now are +26, +6 and +38 MB. Pickers read icons at icon size.
* `render()` makes a new `QTextDocument`: `setUndoRedoEnabled(false)` after `setHtml` (it switches undo back on, and the partial redraw edits the document: 10 undo
  steps and about 5 KB for every message that arrives, for as long as the room stays open).
* What a message costs in Qt: about 1.6 KB as rows (the engine leaves out every field that has its default: a text message has 8 of the 29; it was 2.2 KB with all of them) and 6 KB more while it is in the document (see the window below). A member
  row about 12 KB with its picture; a closed member list gives its rows back (not when the window is minimised). Remote video pictures wait in one slot per person
  (`Core::pictureTrampoline`): a queue of them was 200 MB in a few seconds of GUI stall.
* The timeline document is a window on the room's messages, `[shownFirst(), shownEnd())`, never more than `TimelineView::kWindowMax` (300): it started as the newest 100 and grew by
  100 for every scroll to the top, to the end of the room and without a limit (30 pages up through a room of 5 000 was +33 MB and 40 pages up through 20 000 took 100 s of layout, a
  jump to a message 3 000 back laid out 3 000). Now scrolling to the top (or "Show earlier messages") adds 100 above and, past 300, drops the newest, which then wait behind a "Show newer messages" link;
  scrolling to the bottom of a window that stops short of the newest message adds 100 below and drops the oldest; a jump (`revealMessage`) shows 30 before and 150 after the message.
  The window is kept as two message ids (`windowStart_`, `windowEnd_`; no end follows the room), so older messages arriving in front change nothing, and new messages arriving while the
  window stops short of the end do not touch the document at all; `stickToBottom()` (sending) and the link bring it back to the newest. The same scenarios now: +1.4 MB, 12 s
  (each page lays out the whole window again, about 250 ms on this machine) and +0.6 MB for the jump.
* The engine: the timeline and emote-pack events are serialized from borrowed structs, not `json!` (a `Value` tree was five times the text and 200 000 allocations
  for 5 000 rows); the C sink moves the String into the C string; the open room's loop shares the map of downloaded pictures (`Arc`). A camera picture that already
  fits is not copied before encoding (`Frame::fitted_ref`).
* glibc: `M_ARENA_MAX` 2 and a `malloc_trim` every 30 s (`main.cpp`). glibc raises the size above which a block gets a mapping of its own to the biggest block it has freed, so
  the buffers of a decoded picture stay in the heap when freed (25 MB of free heap after the pictures step): `TimelineView::render` calls `giveBackFreedMemory()` (a
  `malloc_trim`) once when it decoded files. Do not fix `M_MMAP_THRESHOLD` instead (tried, 256 KB to 4 MB): it also stops that, but every big layout array and video frame becomes
  a mapping, 4x the page faults (13 k -> 50 k in the resize step, 3% slower, and it broke that step's budget, which is within 3% of the limit even on the old tree).
* The SDK: its event cache keeps every live event of a room nobody subscribes to (372 events after 300 messages): `ui::shrink_event_caches` asks the rooms a sync
  touched every 5 minutes, and its test fails if the SDK ever does it itself. Each SQLite connection is capped at 500 KB of page cache (default 2 MB, a dozen of them).
* The message index (`index.rs`): rows packed into one text arena, 190 bytes a message (was 541), saved as sealed segment files (`seg-NNNNNN.enc`: a save writes what
  changed; many segments are written again as few; the old single file is read and replaced).
* Link: `--gc-sections` takes 38 MB off the release binary and 2 MB off its resident size. The demo removes its own temporary folders (`/tmp` was 370 MB of them).
* Timeline thumbnails (Preferences > Pictures, pref `timelineThumbnails`, default ON, engine call `set_thumbnails`): a 25 MB photo is about 50 MB at once in the engine (file plus
  HTTP/2 buffers), its thumbnail about 50 KB. `ui::fetch_images` asks the server for `thumbnail/..?width=960&height=640&method=scale` (what the timeline draws at twice the pixels), then
  the sender's `info.thumbnail_source`, then the file itself; an encrypted picture has only the sender's thumbnail (the server cannot scale it), and a picture the message
  declares under 256 KB is fetched whole. `Inner.images` maps event id to `ui::Pic {path, thumb}`; a row with `image_thumb` has the smaller one, and `MainWindow::openPicture` then
  asks for `fetch_media` and `media_file` opens the viewer (a gallery item or "Save as" always fetches the original; `media_copy` keeps the opened file in `media/open/` and uses it the next time). Files are named `<id>.<ext>` (the picture) and `<id>-t.img`
  (a thumbnail), written through `.part` and renamed, and found again after a restart without asking the server (the picture wins over a thumbnail). A call fetches at most 8 pictures,
  the newest first. Not covered: the pictures qVector sends itself carry no thumbnail (so in an encrypted room others download them whole), GIFs are still frames in the timeline anyway.
* The sync filter (`crate::sync_settings`, used by `network_loop` and `sync_once`): `lazy_load_members` and no presence. A first sign-in used to download every member of every
  room (tens of MB for a 20 000-person room) and the SDK stored presence events nobody reads. Now a room's state has the people whose events the sync carries, ourselves and the
  heroes; the SDK marks the list incomplete by itself when a sync is `limited`, and `are_members_synced` is only set by a `/members`. `ui::room_details` lists everybody at once for a
  room of up to `MEMBERS_AT_OPEN` (1000) people (one `/members`); a bigger one lists the known people with `members_complete: false` and the room's `member_count`, and
  `load_members {room_id}` (member panel opened, an `@` completion) asks for the rest. Timeline names and pictures come from the SDK's `sender_profile` (`Timeline::fetch_members`
  fills the missing ones when the list is in; a sender whose member event never came, e.g. old history of a big room, shows the user name until then). The first message sent into a
  big encrypted room still downloads the whole list (the SDK needs it to share keys). The fake server does lazy loading (heroes, summary, member events only for senders;
  `/members` has everybody); `VC_DEMO_MEMBERS=N` makes the demo room a big one with N more people.
* Not done, with what it would give: a non-PIE link (another 4 MB resident, but no ASLR for our code); `max_blocking_threads`. Qt loads all 11 image plugins (1.6 MB) at the first
  `QLineEdit::setClearButtonEnabled`, whatever the app asks of `QImageReader`.

## The fake homeserver (`core-rs/src/testkit.rs`)

wiremock-based, several users/devices: login, keys upload/query/claim/signatures/device_signing (optional UIA password), to-device, send/redact/state (state PUT is
echoed through the sync), members, messages (`set_history(n)`), media (+ seeded avatars), account data, key backup, receipts, typing, tags, spaces, invites,
createRoom/join/leave, user directory, public rooms, link previews. A request it does not know is logged as `UNHANDLED`; error JSON with `errcode` becomes 4xx.
Add endpoints there with a test in `ui.rs`/`crypto.rs`. Every fake server must use its own temp dirs (parallel tests shared one once and flaked). wiremock keeps the servers of finished tests in a process-wide pool and
gives them (port and all) to the next test: anything a test keeps on disk under a name made from the port (the stores of `bob_says` and `alice_other_session`) must be removed
first, or the next test opens the old one with its keys and room key (the reply tests then never decrypted bob's message: it looked like a load flake, and happened more often the more tests there were). `ROOM` doubles as
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
  Pictures/avatars/shields are `QTextDocument` resources. Performance: only a window of at most 300 messages is laid out (the newest 100 while following; `windowStart_`/`windowEnd_`), redraws are skipped when html and
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
`/hierarchy` (`space_rooms`), voice calls are `rtc_peer.rs` (webrtc-rs 0.21: one Opus audio connection, SDP in and out, candidates gathered before the invite/answer), `calls.rs` (Matrix VoIP v1 over `room.send_raw`, a raw `m.call.*` event handler, a 60 s ring, busy and answered-elsewhere handling) `calls_audio.rs` (cpal; a thread owns the streams) and `video.rs` + `vp8.rs` (VP8 through libvpx, preferred and royalty-free, and OpenH264 H.264 Constrained Baseline as the fallback; 15 fps, key frame every 30; the video sender packetises itself after the SDP exchange because a sample track fixes its codec at creation, and the transceiver gets codec preferences for both; encoder and decoder each on their own OS thread; pictures are RGBA `Frame`s shrunk to 960x540). A video call is chosen when placing it (`place_call {video: true}`) or reached later with `add_call_video` (`m.call.negotiate`: a new offer on the running connection; a remote offer that adds video is answered at once; if both sides offer at the same time the callee is polite: it rolls back its own offer and answers the caller's, the caller ignores the callee's); the camera toggle (`set_call_camera`) only starts or stops sending and sends `m.call.sdp_stream_metadata_changed`. Pictures cross the C ABI as raw RGBA: `vcr_video_push` (camera in) and `vcr_video_set_sink` (other side out), never through JSON. The camera itself is Qt (`QCamera` + `QVideoSink` in `CallDialog`), so the engine tests use synthetic pictures. The sound card is an injectable `AudioFactory`, so the two-user test in `calls.rs` uses a tone and a counter. The final link needs libopus and ALSA (`pkg-config` in CMakeLists.txt). NOTE: a plain `cargo build` in the same `target/` replaces the testkit library that CMake builds, and the demo then shows only the login page; `touch core-rs/src/lib.rs` and rebuild through cmake.  QR verification is `Verifier::with_qr` (needs the peer to advertise `m.qr_code.scan.v1`, as phones do: the SDK itself only shows codes).
Group calls are `group_calls.rs`: a mesh between qVector clients (membership = room state `dev.qvector.call.member`; whoever is already in the call rings a newcomer with an `m.call.invite` that carries `dev.qvector.group_id`, which `calls.rs` ignores; the sound of all connections is mixed for the speakers; one encoder per connection, so max 5 people). Events `group_call` / `group_presence`; methods `join_group_call`, `leave_group_call`, `set_group_muted`, `set_group_camera`; the video callback of the C ABI carries `who`. The three-person test needs `FakeHs::add_member`. Not done / ideas: Element Call (LiveKit + MatrixRTC + media key exchange: needs a LiveKit server to test against), a second (screen) stream next to the camera (sharing replaces the camera picture instead) (Element Call / MatrixRTC uses LiveKit, not plain WebRTC), scanning a QR code with a camera, a sticker /
emoji pack editor, tweet pictures, identity-change banner and per-user trust state beyond the shield on messages, resetting cross-signing, sign-out of other sessions,
location messages, spoilers, (image paste, drag-and-drop with a caption bar and multi-picture galleries are done),
notification text with sender and preview (alerts only carry a count), packaging (AppImage/.deb), a real-server run of everything.

## Stress tests

`core-rs/src/stress.rs` (ignored by default) and `qt/stress_main.cpp` (`qvector_stress`, not part of `all`) hit the engine and the real widgets with huge or hostile input
and fail when a step is over its budget. Budgets are for a loaded 4-core laptop; a loosened budget needs a reason in the code. Message bodies are cut at 20 000 characters in the
timeline (a 5 MB message used to freeze the layout for minutes); "Copy message text" still copies all of it.
