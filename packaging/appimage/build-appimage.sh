#!/usr/bin/env bash
#
# Build a portable AppImage of the keyd-viz GUI.
#
# This bundles ONLY the GUI (keydviz). The broker daemon is a hardened system
# service (dedicated user, systemd sandbox) and is deliberately NOT in the AppImage;
# without it the GUI runs in direct-keyd fallback mode (needs keyd/input groups).
# For the full zero-permission live experience, install the AUR package or run
# packaging/install.sh.
#
# Downloads linuxdeploy on demand. Sets APPIMAGE_EXTRACT_AND_RUN so it works on
# FUSE-less CI runners. Output lands in target/appimage/.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO"

ARCH="${ARCH:-x86_64}"
VERSION="${VERSION:-$(grep -m1 '^version' crates/app/Cargo.toml | cut -d'"' -f2)}"
export VERSION ARCH
export APPIMAGE_EXTRACT_AND_RUN=1
# linuxdeploy bundles an old `strip` that chokes on modern glibc's DT_RELR
# (.relr.dyn) libraries on e.g. Arch hosts; cargo already strips our binary, so
# skip linuxdeploy's strip pass to stay host-agnostic.
export NO_STRIP=1

WORK="$REPO/target/appimage"
APPDIR="$WORK/AppDir"
TOOLS="$WORK/tools"

echo "==> building keydviz $VERSION (release)"
cargo build --release -p keydviz

echo "==> fetching linuxdeploy"
mkdir -p "$TOOLS" "$APPDIR"
LD="$TOOLS/linuxdeploy-$ARCH.AppImage"
if [ ! -x "$LD" ]; then
	curl -fsSL -o "$LD" \
		"https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$ARCH.AppImage"
	chmod +x "$LD"
fi

echo "==> bundling dependencies + producing AppImage"
rm -rf "$APPDIR"
mkdir -p "$APPDIR"
cd "$WORK" # linuxdeploy writes the .AppImage into the cwd
"$LD" \
	--appdir "$APPDIR" \
	--executable "$REPO/target/release/keydviz" \
	--desktop-file "$REPO/packaging/keyd-viz.desktop" \
	--icon-file "$REPO/assets/keyd-viz.svg" \
	--output appimage

echo "==> done:"
ls -1 "$WORK"/keyd-viz*.AppImage
