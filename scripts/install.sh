#!/bin/sh
# Install ddu into /Applications (override: DDU_INSTALL_DIR=...) under the
# official identity — "Day Day Up", dev.just.ddu. Builds first, then bundles
# straight into the destination: no staging copy in target/, so
# LaunchServices only ever registers the installed path. The dev copy from
# scripts/dev.sh keeps a different bundle id, so the two never get confused.
#
# Usage: scripts/install.sh [--open]   (--open launches the fresh install)
set -e
cd "$(dirname "$0")/.."

OPEN=
for arg in "$@"; do
  case "$arg" in
    --open) OPEN=1 ;;
    *) echo "usage: $0 [--open]" >&2; exit 2 ;;
  esac
done

cargo build --release

DEST="${DDU_INSTALL_DIR:-/Applications}"
APP="$DEST/ddu.app"

# Quit the copy being replaced so it doesn't keep running the old binary
# from a deleted bundle.
pkill -f "$APP/Contents/MacOS/ddu.bin" 2>/dev/null || true
n=0
while pgrep -f "$APP/Contents/MacOS/ddu.bin" >/dev/null 2>&1; do
  n=$((n + 1))
  [ "$n" -gt 50 ] && { pkill -9 -f "$APP/Contents/MacOS/ddu.bin"; break; }
done

rm -rf "$APP"
scripts/make-bundle.sh "$APP" "Day Day Up" dev.just.ddu || {
  echo "hint: rerun with sudo, or DDU_INSTALL_DIR=\"$HOME/Applications\" $0" >&2
  exit 1
}
echo "installed: $APP"

if [ -n "$OPEN" ]; then
  # Retire any legacy same-identity instance (the old target/ddu.app flow) —
  # a live same-bundle-id copy would make `open` activate it instead of ours.
  pkill -f "target/ddu.app/Contents/MacOS/ddu.bin" 2>/dev/null || true
  n=0
  while pgrep -f "target/ddu.app/Contents/MacOS/ddu.bin" >/dev/null 2>&1; do
    n=$((n + 1))
    [ "$n" -gt 50 ] && { pkill -9 -f "target/ddu.app/Contents/MacOS/ddu.bin"; break; }
  done
  open "$APP"
fi
