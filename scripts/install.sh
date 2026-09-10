#!/bin/sh
# Install ddu into /Applications (override: DDU_INSTALL_DIR=...) under the
# official identity — "Day Day Up", dev.just.ddu. Builds first, then bundles
# straight into the destination: no staging copy in target/, so
# LaunchServices only ever registers the installed path. The dev copy from
# scripts/dev.sh keeps a different bundle id, so the two never get confused.
#
# Usage: scripts/install.sh [--open]   (--open launches the fresh install)
set -e
SCRIPTS=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPTS/lib.sh"
cd "$SCRIPTS/.."

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
running=
rc=0
stop_app "$APP" || rc=$?
case $rc in
    0) ;;
    2) running=1 ;;
    *) echo "could not stop the running copy — quit it and rerun" >&2; exit 1 ;;
esac

rm -rf "$APP"
scripts/make-bundle.sh "$APP" "Day Day Up" dev.just.ddu || {
  echo "hint: rerun with sudo, or DDU_INSTALL_DIR=\"$HOME/Applications\" $0" >&2
  exit 1
}
echo "installed: $APP"

if [ -n "$OPEN" ] && [ -z "$running" ]; then
  # Retire any legacy same-identity instance (the old target/ddu.app flow) —
  # a live same-bundle-id copy would make `open` activate it instead of ours.
  stop_app target/ddu.app || true
  open "$APP"
elif [ -n "$running" ]; then
  # This shell runs inside the app being replaced: `open` would just
  # re-activate that stale instance, so say what to do instead.
  echo "the running copy keeps the old binary — quit it (⌘Q) and reopen to use the new build" >&2
fi
