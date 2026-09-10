#!/bin/sh
# Helpers shared by dev.sh / install.sh (sourced, never executed).

# PIDs of the processes running the app bundle at $1 (a relative path is
# resolved against the caller's cwd).
#
# `pgrep`/`pkill` are unusable here: both hide the calling process AND all
# of its ancestors from their own listing. When the caller runs inside
# ddu's own terminal (a session in a ddu panel — the normal way these
# scripts get run in this project) the app being replaced IS an ancestor,
# so `pkill -f <bundle>/Contents/MacOS/ddu.bin` silently matches nothing:
# the script "succeeds", the stale instance keeps running the deleted
# binary, and `open` just re-activates it. `ps` has no such blind spot.
#
# The match is anchored at the start of the command line, so this scan can
# never match itself or its caller (their argv holds the path as an
# argument, not as argv[0]). Both the logical and the resolved path are
# tried: launchd hands the kernel the path `open` was given, which is not
# necessarily the one `cd` would report for a symlinked parent.
app_pids() {
    bundle=$1
    case $bundle in
        /*) logical=$bundle ;;
        ./*) logical=$PWD/${bundle#./} ;;
        *) logical=$PWD/$bundle ;;
    esac
    physical=$(CDPATH= cd -- "$bundle" 2>/dev/null && pwd -P) ||
        physical=$logical
    for bin in "$logical/Contents/MacOS/ddu.bin" "$physical/Contents/MacOS/ddu.bin"; do
        ps -axww -o pid=,command= |
            awk -v bin="$bin" '
                {
                    pid = $1
                    sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "")
                    if (index($0, bin) == 1) print pid
                }'
    done | sort -u
}

# True when pid $1 is this script or one of its ancestors.
is_self_or_ancestor() {
    target=$1
    pid=$$
    while [ -n "$pid" ]; do
        if [ "$pid" = "$target" ]; then
            return 0
        fi
        pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d '[:space:]')
        case $pid in
            '' | *[!0-9]*) return 1 ;;
        esac
    done
    return 1
}

# Stop the app bundle at $1 and wait for it to exit — SIGTERM, then
# SIGKILL after $2 seconds (default 5).
#
# Returns 0 when nothing is left running, 2 when the copy hosts this very
# shell (killing it would take the caller down mid-script, so it is left
# alone and the caller decides), 1 when a SIGKILLed process still lingers.
stop_app() {
    bundle=$1
    limit=$(( ${2:-5} * 10 ))
    pids=$(app_pids "$bundle")
    [ -n "$pids" ] || return 0
    for pid in $pids; do
        if is_self_or_ancestor "$pid"; then
            echo "running copy of $bundle hosts this shell (pid $pid) — not stopping it" >&2
            return 2
        fi
    done
    echo "stopping running copy: $bundle (pid $(echo $pids | tr '\n' ' '))" >&2
    kill $pids 2>/dev/null || true
    n=0
    while [ -n "$(app_pids "$bundle")" ]; do
        n=$((n + 1))
        if [ "$n" -eq "$limit" ]; then
            echo "  no exit after $((limit / 10))s — SIGKILL" >&2
            kill -9 $(app_pids "$bundle") 2>/dev/null || true
        fi
        if [ "$n" -gt $((limit + 20)) ]; then
            echo "  still running after SIGKILL" >&2
            return 1
        fi
        sleep 0.1
    done
    return 0
}
