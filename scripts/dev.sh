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
# DDU_DIR and explicit DDU_STATE_PATH / DDU_SETTINGS_PATH (dev state
# isolation) pass through and are baked into the bundle launcher (see
# scripts/make-bundle.sh).
set -e
SCRIPTS=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
. "$SCRIPTS/lib.sh"
cd "$SCRIPTS/.."

cargo build --release

# Isolate dev state from the installed app unless the caller pinned it.
: "${DDU_STATE_PATH:=$HOME/Library/Application Support/ddu-dev/state.json}"
: "${DDU_SETTINGS_PATH:=$HOME/Library/Application Support/ddu-dev/settings.json}"
export DDU_STATE_PATH DDU_SETTINGS_PATH
mkdir -p "$(dirname "$DDU_STATE_PATH")" "$(dirname "$DDU_SETTINGS_PATH")"

scripts/make-bundle.sh target/ddu-dev.app "Day Day Up Dev" dev.just.ddu.dev

# `open` only ACTIVATES an already-running app — stop our own previous
# instance first so the fresh build actually appears. Never touches the
# installed copy: different bundle id, different path.
rc=0
stop_app target/ddu-dev.app || rc=$?
if [ "$rc" = 2 ]; then
    # The dev copy hosts this shell: it keeps the old binary until it is
    # quit, and `open` below would only re-activate it.
    echo "fresh build is bundled at target/ddu-dev.app; quit the running copy and reopen to use it" >&2
    exit 0
elif [ "$rc" != 0 ]; then
    echo "could not stop the running dev copy — quit it and rerun" >&2
    exit 1
fi

DDU_DIR="${DDU_DIR:-$HOME/Code/ddu}" exec open "$@" target/ddu-dev.app
