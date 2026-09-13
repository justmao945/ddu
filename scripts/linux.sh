#!/bin/sh
# Linux counterpart of dev.sh / install.sh. The macOS scripts exist
# because a window must be launched through LaunchServices as a signed
# .app bundle; on Linux ddu is a plain binary, launched from the
# workspace root it should open on.
#
# Usage:
#   scripts/linux.sh run       build --release, then run the fresh binary
#                              from $DDU_DIR (default: this repo)
#   scripts/linux.sh install   build --release, install to ~/.local/bin
#                              plus a desktop entry (menu launcher)
#
# DDU_INSTALL_DIR overrides the install prefix, DDU_DIR the workspace
# root the app opens on. Nothing here touches the macOS bundle flow —
# a Linux machine has no target/ddu-dev.app to keep in sync.
set -e

SCRIPTS=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
ROOT=$(CDPATH= cd -- "$SCRIPTS/.." && pwd)
BIN="$ROOT/target/release/ddu"

cargo build --release --manifest-path "$ROOT/Cargo.toml"

case "${1:-run}" in
    run)
        # `initial_projects()` seeds from the process cwd, so the run
        # directory IS the workspace the window opens on.
        cd "${DDU_DIR:-$ROOT}"
        exec "$BIN"
        ;;
    install)
        PREFIX="${DDU_INSTALL_DIR:-$HOME/.local/bin}"
        APPS="$HOME/.local/share/applications"
        ICONS="$HOME/.local/share/icons/hicolor/scalable/apps"
        mkdir -p "$PREFIX" "$APPS" "$ICONS"
        install -m 0755 "$BIN" "$PREFIX/ddu"
        install -m 0644 "$ROOT/assets/icon.svg" "$ICONS/ddu.svg"
        # `Path` is the launch directory (= the project ddu opens on)
        # because a desktop launch would otherwise use $HOME.
        cat > "$APPS/ddu.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Day Day Up
Comment=Workspace for AI coding agents: sessions, terminal, live git diff
Exec="$PREFIX/ddu"
Path=${DDU_DIR:-$HOME}
Icon=ddu
Terminal=false
Categories=Development;
StartupWMClass=ddu
DESKTOP
        update-desktop-database "$APPS" 2>/dev/null || true
        echo "installed $PREFIX/ddu (menu entry: $APPS/ddu.desktop)"
        ;;
    *)
        echo "usage: $0 [run|install]" >&2
        exit 2
        ;;
esac
