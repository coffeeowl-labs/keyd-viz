#!/usr/bin/env bash
#
# Install (or update) keyd-viz's privileged companions:
#  - keydviz-helperd — the zero-permission broker that streams keyd's layers
#    (and optionally keypresses) to the keyd-viz GUI
#  - keydviz-apply + its polkit action — the one-shot pkexec tool behind Edit
#    Mode's one-click apply (root-owned 0755, NOT setuid; pkexec carries the
#    privilege and the policy's exec.path binds to /usr/bin/keydviz-apply)
#
# Run as a NORMAL user (not root): it builds as you and uses sudo only for the
# privileged install steps. Re-running it cleanly updates an existing install.
#
#   ./packaging/install.sh            # layers only (safe default)
#   ./packaging/install.sh --keys     # also enable keypress glow (reads /dev/input)
#   ./packaging/install.sh --no-build # use an already-built release binary
#
# Uninstall with packaging/uninstall.sh. See packaging/README.md for the security model.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$REPO/target/release/keydviz-helperd"
APPLY_BIN="$REPO/target/release/keydviz-apply"
POLICY="$REPO/packaging/polkit/io.github.coffeeowl-labs.keydviz.policy"
KEYS=0
BUILD=1
for a in "$@"; do
	case "$a" in
	--keys) KEYS=1 ;;
	--no-build) BUILD=0 ;;
	-h | --help)
		grep '^#' "$0" | grep -v '^#!' | sed 's/^# \?//'
		exit 0
		;;
	*)
		echo "install.sh: unknown argument: $a" >&2
		exit 2
		;;
	esac
done

SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

if [ "$BUILD" -eq 1 ]; then
	echo "==> building keydviz-helperd + keydviz-apply (release)"
	cargo build --release --manifest-path "$REPO/Cargo.toml" -p keydviz-helper -p keydviz-apply
fi
for b in "$BIN" "$APPLY_BIN"; do
	[ -x "$b" ] || {
		echo "install.sh: missing $b — build it first (drop --no-build)" >&2
		exit 1
	}
done

echo "==> installing binary -> /usr/bin/keydviz-helperd"
$SUDO install -Dm755 "$BIN" /usr/bin/keydviz-helperd

echo "==> installing one-click apply tool -> /usr/bin/keydviz-apply (+ polkit action)"
$SUDO install -Dm755 "$APPLY_BIN" /usr/bin/keydviz-apply
$SUDO install -Dm644 "$POLICY" \
	/usr/share/polkit-1/actions/io.github.coffeeowl-labs.keydviz.policy

echo "==> creating system user 'keyd-viz' (sysusers)"
$SUDO install -Dm644 "$REPO/packaging/sysusers.d/keyd-viz.conf" /usr/lib/sysusers.d/keyd-viz.conf
$SUDO systemd-sysusers

echo "==> installing service unit"
$SUDO install -Dm644 "$REPO/packaging/systemd/keydviz-helperd.service" \
	/usr/lib/systemd/system/keydviz-helperd.service

DROPIN=/etc/systemd/system/keydviz-helperd.service.d/keypresses.conf
if [ "$KEYS" -eq 1 ]; then
	echo "==> enabling keypress glow (drop-in: $DROPIN)"
	$SUDO install -Dm644 "$REPO/packaging/systemd/keydviz-helperd.service.d/keypresses.conf" "$DROPIN"
else
	echo "==> layers-only (pass --keys to also glow keypresses)"
	$SUDO rm -f "$DROPIN" # ensure a re-run without --keys reverts to layers-only
fi

echo "==> reloading systemd + (re)starting service"
$SUDO systemctl daemon-reload
$SUDO systemctl enable keydviz-helperd
$SUDO systemctl restart keydviz-helperd

echo "==> done. launch the GUI (keydviz) and it auto-connects. status:"
systemctl --no-pager --lines=0 status keydviz-helperd || true
