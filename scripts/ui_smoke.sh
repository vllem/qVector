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
for args in "" "--members" "--search message" "--thread Hello" "--dialog poll" "--dialog saved" "--dialog prefs" "--dialog verify" "--dialog recovery" "--dialog settings"; do
    log="$out/log"
    # shellcheck disable=SC2086
    if ! timeout 120 "$bin" --demo --data "$out/data" --room bob --delay 20000 $args --screenshot "$out/shot.png" >"$log" 2>&1; then echo "FAILED ($args): exit status"; tail -5 "$log"; fail=1; continue; fi
    [ -s "$out/shot.png" ] || { echo "FAILED ($args): no screenshot"; fail=1; continue; }
    echo "ok: ${args:-main window}"
done
exit $fail
