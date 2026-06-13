#!/usr/bin/env bash
# Capture faithful, headless screenshots of every edit-mode state for the AI UX-critic
# pass (ROADMAP §8). Drives the `--render-state` harness in crates/app/src/main.rs, which
# invokes the app's real edit callbacks so each panel is populated authentically, then
# software-renders one frame to a PNG (no GPU, no compositor, no clicking).
#
# Usage:  scripts/ux-screenshots.sh [out_dir] [config.conf]
# Default: out_dir=/tmp/uxcrit, config=examples/hhkb.conf (layout hhkb).
#
# Re-run after any edit-mode UI change, then feed the folder to the persona-critic
# Workflow (ux-critic-edit-mode).
set -euo pipefail

OUT="${1:-/tmp/uxcrit}"
CONF="${2:-examples/hhkb.conf}"
BIN="target/release/keydviz"
[ -x "$BIN" ] || BIN="target/debug/keydviz"
[ -x "$BIN" ] || { echo "build first: cargo build -p keydviz" >&2; exit 1; }

mkdir -p "$OUT"
STATES=(base edit key-selected tap-hold macro picker chord global \
        new-layer rename-layer delete-layer discard apply-summary)

i=0
for s in "${STATES[@]}"; do
  i=$((i + 1))
  n=$(printf '%02d' "$i")
  "$BIN" "$CONF" --layout hhkb --render-state "$s" \
    --render "$OUT/$n-$s.png" --render-delay 500
done
echo "captured ${#STATES[@]} states -> $OUT"
