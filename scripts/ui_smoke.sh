#!/bin/sh
# Opens the main window and every dialog of the demo workspace (a fake homeserver, no network) on an offscreen display and fails on a crash or a timeout.
# usage: ui_smoke.sh path/to/qvector
bin="$1"
[ -x "$bin" ] || { echo "usage: $0 path/to/qvector" >&2; exit 2; }
out="${TMPDIR:-/tmp}/vc_ui_smoke.$$"
trap 'rm -rf "$out"' EXIT
mkdir -p "$out/cfg"
export QT_QPA_PLATFORM=offscreen XDG_CONFIG_HOME="$out/cfg" XDG_CACHE_HOME="$out/cache" XDG_DATA_HOME="$out/data"
fail=0
for args in "" "--members" "--search message" "--thread Hello" "--dialog poll" "--dialog saved" "--dialog prefs" "--dialog verify" "--dialog recovery" "--dialog settings" "--dialog explore" "--dialog verifyqr" "--dialog call" "--dialog callon" "--dialog videocall" "--dialog videooff" "--dialog group" "--dialog picture"; do
    log="$out/log"
    # shellcheck disable=SC2086
    if ! timeout 120 "$bin" --demo --data "$out/data" --room bob --delay 20000 $args --screenshot "$out/shot.png" >"$log" 2>&1; then echo "FAILED ($args): exit status"; tail -5 "$log"; fail=1; continue; fi
    [ -s "$out/shot.png" ] || { echo "FAILED ($args): no screenshot"; fail=1; continue; }
    echo "ok: ${args:-main window}"
done
# the status bar's "Sending..." bar must appear when a message is sent and go away when it shows up in the room
printf 'a message sent through the composer' > "$out/msg.txt"
if VC_SEND_LOG=1 timeout 120 "$bin" --demo --data "$out/data" --room bob --delay 20000 --send-text "$out/msg.txt" --screenshot "$out/shot.png" >"$out/log" 2>&1 \
   && grep -q "sendbar: shown 'Sending...'" "$out/log" && grep -q "sendbar: hidden after" "$out/log"; then echo "ok: sending bar"; else echo "FAILED (sending bar)"; grep sendbar "$out/log"; fail=1; fi
exit $fail
