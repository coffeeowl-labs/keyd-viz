#!/usr/bin/env bash
#
# Remove the keydviz-helperd system service installed by packaging/install.sh.
# Run as a normal user (uses sudo for the privileged steps). The dedicated system
# user 'keyd-viz' is left in place by default — pass --purge to remove it too.
set -euo pipefail

PURGE=0
for a in "$@"; do
	case "$a" in
	--purge) PURGE=1 ;;
	-h | --help)
		grep '^#' "$0" | grep -v '^#!' | sed 's/^# \?//'
		exit 0
		;;
	*)
		echo "uninstall.sh: unknown argument: $a" >&2
		exit 2
		;;
	esac
done

SUDO=""
[ "$(id -u)" -ne 0 ] && SUDO="sudo"

echo "==> stopping + disabling service"
$SUDO systemctl disable --now keydviz-helperd 2>/dev/null || true

echo "==> removing files"
$SUDO rm -f /usr/lib/systemd/system/keydviz-helperd.service \
	/etc/systemd/system/keydviz-helperd.service.d/keypresses.conf \
	/usr/lib/sysusers.d/keyd-viz.conf \
	/usr/bin/keydviz-helperd
$SUDO rmdir /etc/systemd/system/keydviz-helperd.service.d 2>/dev/null || true
$SUDO systemctl daemon-reload

if [ "$PURGE" -eq 1 ]; then
	echo "==> removing system user 'keyd-viz'"
	$SUDO userdel keyd-viz 2>/dev/null || true
	$SUDO rm -f /usr/lib/sysusers.d/keyd-viz.conf
else
	echo "==> left system user 'keyd-viz' in place (use --purge to remove it)"
fi

echo "==> done."
