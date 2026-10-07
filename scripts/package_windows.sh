#!/bin/bash
# Builds dist/Vector-Windows.zip from an existing build directory, in an MSYS2 UCRT64 shell (default build directory: build).
#   cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DVC_SANITIZE=OFF -DVC_WERROR=OFF -DVC_CONSOLE=OFF
#   cmake --build build && scripts/package_windows.sh build
# The folder in the zip runs on a machine without MSYS2: Qt (windeployqt) and every other DLL the program needs are copied next to it.
set -euo pipefail
cd "$(dirname "$0")/.."
BUILD="${1:-build}"
EXE="$BUILD/vector.exe"
[ -f "$EXE" ] || { echo "no $EXE: build first (see README.md)" >&2; exit 1; }
OUT=dist/Vector
rm -rf "$OUT"
mkdir -p "$OUT/fonts"
cp "$EXE" "$OUT/Vector.exe"
cp third_party/fonts/NotoColorEmoji-Regular.ttf third_party/fonts/OFL.txt "$OUT/fonts/"
cp LICENSE "$OUT/" 2>/dev/null || true
DEPLOY="$(command -v windeployqt-qt6 || command -v windeployqt6 || command -v windeployqt || true)"
[ -n "$DEPLOY" ] || { echo "windeployqt not found (install mingw-w64-ucrt-x86_64-qt6-tools)" >&2; exit 1; }
"$DEPLOY" --no-translations --no-opengl-sw "$OUT/Vector.exe"
# whatever else the executable (and the Qt plugins) link against outside Windows itself, e.g. OpenSSL and the compiler runtime
for bin in "$OUT/Vector.exe" $(find "$OUT" -name '*.dll'); do
  ldd "$bin" 2>/dev/null | awk '/=> \/ucrt64\// {print $3}'
done | sort -u | while read -r dll; do
  [ -f "$OUT/$(basename "$dll")" ] || cp "$dll" "$OUT/"
done
rm -f dist/Vector-Windows.zip
(cd dist && zip -qr Vector-Windows.zip Vector)
echo "wrote dist/Vector-Windows.zip"
