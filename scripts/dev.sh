#!/bin/sh
# Dev loop: build the release binary, bundle it as "Day Day Up Dev"
# (dev.just.ddu.dev) at target/ddu-dev.app with state isolated under
# ~/Library/Application Support/ddu-dev/, and launch through
# LaunchServices. The distinct bundle id means a dev copy and an
# installed copy (scripts/install.sh) run side by side; `open` can
# never activate the wrong one.
#
# Usage: scripts/dev.sh   (extra args are passed to `open`)
#
# DDU_VERIFY_SETTINGS / DDU_VERIFY_EXIT / DDU_VERIFY_CLOSE / DDU_DEBUG
# and explicit DDU_STATE_PATH / DDU_SETTINGS_PATH pass through and are
# baked into the bundle launcher (see scripts/make-bundle.sh).
set -e
cd "$(dirname "$0")/.."

cargo build --release

# Isolate dev state from the installed app unless the caller pinned it.
: "${DDU_STATE_PATH:=$HOME/Library/Application Support/ddu-dev/state.json}"
: "${DDU_SETTINGS_PATH:=$HOME/Library/Application Support/ddu-dev/settings.json}"
export DDU_STATE_PATH DDU_SETTINGS_PATH
mkdir -p "$(dirname "$DDU_STATE_PATH")" "$(dirname "$DDU_SETTINGS_PATH")"

scripts/make-bundle.sh target/ddu-dev.app "Day Day Up Dev" dev.just.ddu.dev

# `open` only ACTIVATES an already-running app — kill our own previous
# instance first so the fresh build actually appears. Never touches the
# installed copy: different bundle id, different path.
pkill -f "target/ddu-dev.app/Contents/MacOS/ddu.bin" 2>/dev/null || true
n=0
while pgrep -f "target/ddu-dev.app/Contents/MacOS/ddu.bin" >/dev/null 2>&1; do
  n=$((n + 1))
  [ "$n" -gt 50 ] && { pkill -9 -f "target/ddu-dev.app/Contents/MacOS/ddu.bin"; break; }
done

DDU_DIR="${DDU_DIR:-$HOME/Code/ddu}" exec open "$@" target/ddu-dev.app
