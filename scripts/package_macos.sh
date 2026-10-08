#!/bin/bash
# Builds dist/qVector-macOS.dmg from an existing macOS build directory (default: build).
#   cmake -S . -B build -DCMAKE_BUILD_TYPE=Release -DVC_SANITIZE=OFF -DVC_WERROR=OFF -DCMAKE_PREFIX_PATH="$(brew --prefix qt)" -DOPENSSL_ROOT_DIR="$(brew --prefix openssl@3)"
#   cmake --build build && scripts/package_macos.sh build
# macdeployqt copies Qt (and OpenSSL from Homebrew) into the bundle. The result is signed ad hoc only, so Gatekeeper asks once:
# right-click the app > Open.
set -euo pipefail
cd "$(dirname "$0")/.."
BUILD="${1:-build}"
APP="$BUILD/qVector.app"
[ -d "$APP" ] || { echo "no $APP: build for macOS first (see README.md)" >&2; exit 1; }
MACDEPLOYQT="$(command -v macdeployqt || true)"
[ -n "$MACDEPLOYQT" ] || MACDEPLOYQT="$(brew --prefix qt)/bin/macdeployqt"
mkdir -p dist
rm -rf dist/qVector.app
cp -R "$APP" dist/qVector.app
"$MACDEPLOYQT" dist/qVector.app -always-overwrite
codesign --force --deep --sign - dist/qVector.app
rm -f dist/qVector-macOS.dmg
hdiutil create -volname qVector -srcfolder dist/qVector.app -ov -format UDZO dist/qVector-macOS.dmg
echo "wrote dist/qVector-macOS.dmg"
