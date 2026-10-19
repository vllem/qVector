#!/bin/sh
# Build an AppImage of qVector (Linux). Usage: scripts/package_appimage.sh [tools-dir]
# tools-dir holds linuxdeploy-x86_64.AppImage and linuxdeploy-plugin-qt-x86_64.AppImage (downloaded there when missing).
# Build on the oldest distro you want to support: the AppImage needs at least the build machine's glibc and libstdc++.
set -e
cd "$(dirname "$0")/.."
TOOLS=${1:-$HOME/appimage-tools}
mkdir -p "$TOOLS"
for t in linuxdeploy linuxdeploy-plugin-qt; do
  [ -x "$TOOLS/$t-x86_64.AppImage" ] || {
    wget -O "$TOOLS/$t-x86_64.AppImage" "https://github.com/linuxdeploy/$t/releases/download/continuous/$t-x86_64.AppImage"
    chmod +x "$TOOLS/$t-x86_64.AppImage"; }
done
export PATH="$TOOLS:$PATH"
export APPIMAGE_EXTRACT_AND_RUN=1

cmake -S . -B build-release -DCMAKE_BUILD_TYPE=Release -DVC_DEMO=OFF -DCMAKE_INSTALL_PREFIX=/usr
cmake --build build-release -j"$(nproc)"
rm -rf AppDir
DESTDIR="$PWD/AppDir" cmake --install build-release

# every icon size (the install step only has 256 px); the window icon itself is drawn by the program
for s in 16 32 48 64 128 256 512; do
  d=AppDir/usr/share/icons/hicolor/${s}x${s}/apps
  mkdir -p "$d" && cp "packaging/icons/icon-$s.png" "$d/qvector.png"
done

# the theme plugins (qt6ct, GTK) are only bundled when asked for by this variable
export DEPLOY_PLATFORM_THEMES=1
export EXTRA_QT_PLUGINS="multimedia;imageformats;iconengines;wayland-shell-integration;wayland-decoration-client"
# without the Wayland plugin the program runs through XWayland, and a Wayland desktop (COSMIC, GNOME) then shows no icon in its dock:
# it finds the icon through the window's app id and qvector.desktop, which an X11 window does not carry
export EXTRA_PLATFORM_PLUGINS="libqwayland-generic.so;libqwayland-egl.so"
export QMAKE=${QMAKE:-$(command -v qmake6 || command -v qmake)}
export LINUXDEPLOY_OUTPUT_VERSION=$(git describe --tags --always)
linuxdeploy-x86_64.AppImage --appdir AppDir --plugin qt -d AppDir/usr/share/applications/qvector.desktop

# linuxdeploy-plugin-qt does not pick up the Wayland-EGL client buffer integration (Qt then logs 'Failed to load client buffer
# integration: "wayland-egl"' and the window is not set up the way the unbundled build's is): copy it and its libraries
QTPLUGINS=$("$QMAKE" -query QT_INSTALL_PLUGINS)
mkdir -p AppDir/usr/plugins/wayland-graphics-integration-client
cp "$QTPLUGINS"/wayland-graphics-integration-client/libqt-plugin-wayland-egl.so AppDir/usr/plugins/wayland-graphics-integration-client/
linuxdeploy-x86_64.AppImage --appdir AppDir --deploy-deps-only AppDir/usr/plugins/wayland-graphics-integration-client

# only the FFmpeg backend is used (QCamera / QVideoSink): drop the GStreamer one and the libraries only it needed
rm -f AppDir/usr/plugins/multimedia/libgstreamermediaplugin.so
rm -f AppDir/usr/lib/libgst*.so* AppDir/usr/lib/liborc-*.so*

linuxdeploy-x86_64.AppImage --appdir AppDir --output appimage -d AppDir/usr/share/applications/qvector.desktop
ls -l qVector-*.AppImage
