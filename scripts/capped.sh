#!/bin/sh
# Run a command inside a systemd user scope that carries a CPUQuota, one at a
# time, so builds and test runs cannot take the desktop down with them.
#
# `cargo build --release` / `cargo test` unwrapped is minutes of every core
# pinned, and cargo has no quota of its own: `[build] jobs` counts rustc
# *processes*, so the LLVM thread pool inside one crate still fills the
# machine. A cgroup quota does not care how many processes share it, which is
# why this wraps the command instead.
#
# The quota alone is per command, though: three agents building in three
# terminals would take three halves of the machine and pin it anyway. So a
# wrapped command also takes a per-user flock and waits for its turn
# (`DDU_CAPPED_LOCKED=1` in the environment — set by the wrapper itself, and
# inherited by whatever the command spawns — skips the queue).
#
# Usage: scripts/capped.sh cargo test -p ddu-core
#        scripts/capped.sh cargo build --release
#
# DDU_BUILD_CPU_QUOTA takes a systemd percentage ("400%", the suffix is
# required — systemd rejects a bare number) or `off`; the default is half the
# machine's cores. The scope and the lock both die with the command, so
# nothing a caller does afterwards is throttled or queued:
# `scripts/linux.sh run` builds through here and then `exec`s the app outside.
set -e

if [ "$#" = 0 ]; then
    echo "usage: $0 <command> [args...]" >&2
    exit 2
fi

# The lock is an flock, so a killed build can never wedge the queue, and a
# nested call from inside a wrapped command re-enters without queueing behind
# its own parent.
if [ -z "${DDU_CAPPED_LOCKED-}" ] && command -v flock >/dev/null 2>&1; then
    lock="${XDG_RUNTIME_DIR:-${TMPDIR:-/tmp}}/ddu-build.lock"
    if ! flock -n "$lock" true 2>/dev/null; then
        echo "capped.sh: another build holds the machine — queued behind it" >&2
    fi
    DDU_CAPPED_LOCKED=1
    export DDU_CAPPED_LOCKED
    exec flock "$lock" "$0" "$@"
fi

if ! command -v systemd-run >/dev/null 2>&1; then
    # Nothing to cap with (macOS, a container); only an explicit request is
    # worth a word about.
    if [ -n "${DDU_BUILD_CPU_QUOTA-}" ]; then
        echo "capped.sh: no systemd-run on this host — ignoring" \
            "DDU_BUILD_CPU_QUOTA" >&2
    fi
    exec "$@"
fi

quota="${DDU_BUILD_CPU_QUOTA-}"
if [ -z "$quota" ]; then
    cores=$(nproc 2>/dev/null || echo 0)
    case "$cores" in
        # No core count to halve: better uncapped than a quota of zero.
        0 | '' | *[!0-9]*) exec "$@" ;;
    esac
    quota=$(printf '%s%%' "$(( cores * 50 ))")
elif [ "$quota" != off ]; then
    case "$quota" in
        *[!0-9%]* | *%% | % | '') bad_quota ;;
    esac
    case "$quota" in
        [0-9]*%) ;;
        *) bad_quota ;;
    esac
fi

if [ "$quota" = off ]; then
    exec "$@"
fi

# `--scope` leaves the command in this terminal — stdin/stdout/stderr are
# inherited, so cargo's progress and a prompt still work, and Ctrl-C reaches
# it — while blocking with its exit status. The probe costs one fork and tells
# a host without a usable user manager apart from a rejected quota: the latter
# must not silently run uncapped.
if systemd-run --user --scope --quiet -p "CPUQuota=$quota" true 2>/dev/null; then
    exec systemd-run --user --scope --quiet -p "CPUQuota=$quota" "$@"
fi
echo "capped.sh: no systemd user session — running with no CPU cap" >&2
exec "$@"

bad_quota() {
    echo "capped.sh: DDU_BUILD_CPU_QUOTA='${DDU_BUILD_CPU_QUOTA-}' is not a" \
        "CPUQuota value (a percentage such as 400%, or off)" >&2
    exit 2
}
