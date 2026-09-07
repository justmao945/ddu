#!/bin/sh
# Launch ddu through LaunchServices as a proper macOS app.
#
# Why not `cargo run` / direct child process: on macOS 26 a background-
# launched (unactivated) process never gets NSWindowOcclusionStateVisible
# and cannot self-activate, so gpui's display link never starts and the
# window freezes after its first frame. `open`-ing a .app bundle makes
# LaunchServices activate the app at launch, which keeps the frame
# pipeline alive exactly like Zed.
#
# LaunchServices starts apps with cwd=/, so the bundle's executable is a
# tiny launcher that cds to DDU_DIR (default: ~/Code/ddu) before exec.
#
# Usage: scripts/ddu-app.sh   (run `cargo build --release` first)
set -e
cd "$(dirname "$0")/.."

BIN=target/release/ddu
APP=target/ddu.app

[ -x "$BIN" ] || { echo "run: cargo build --release" >&2; exit 1; }

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
scripts/make-icon.sh "$APP/Contents/Resources/ddu.icns"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Day Day Up</string>
  <key>CFBundleDisplayName</key><string>Day Day Up</string>
  <key>CFBundleIdentifier</key><string>dev.just.ddu</string>
  <key>CFBundleExecutable</key><string>launch.sh</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleIconFile</key><string>ddu</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
cp "$BIN" "$APP/Contents/MacOS/ddu.bin"
cat > "$APP/Contents/MacOS/launch.sh" <<LAUNCH
#!/bin/sh
cd "\${DDU_DIR:-\$HOME/Code/ddu}" || exit 1
# LaunchServices starts the app with the bare system environment
# (PATH=/usr/bin:/bin:/usr/sbin:/sbin), so agent CLIs installed via
# homebrew/mise/nvm/cargo/… are not found. Rebuild the environment
# from the user's login+interactive shell first; the DDU_* forwards
# below then win by order. -i matters: most users export PATH in
# ~/.zshrc, which only an interactive shell sources. The dump prints
# each exported var as a plain sh export NAME=value (zsh's stock
# export -p emits export -T PATH path=(...) arrays that sh cannot
# eval), with (q) shell-quoting values.
if [ -x /bin/zsh ]; then
  eval "\$(/bin/zsh -ilc 'for k in \${(k)parameters[(R)*export*]}; do print -r -- "export \$k=\${(q)\${(P)k}}"; done' 2>/dev/null)" || true
fi
# Forward every DDU_* variable (DDU_DIR, DDU_STATE_PATH, DDU_DEBUG, …)
# so a dev relaunch can point the app at a scratch state file.
for v in \$(env | sed -n 's/^\\(DDU_[A-Za-z0-9_]*\\)=.*/\\1/p'); do
  eval "export \$v"
done
# Dev hook baked at bundle time (see main.rs): auto-open Settings. Omitted
# entirely when unset so a clean bundle can't leak an empty-string var.
${DDU_VERIFY_SETTINGS:+export DDU_VERIFY_SETTINGS="${DDU_VERIFY_SETTINGS}"}
exec "\$(dirname "\$0")/ddu.bin"
LAUNCH
chmod +x "$APP/Contents/MacOS/launch.sh"

# `open` only ACTIVATES an already-running app — without this kill the
# old binary keeps running and the fresh build never appears.
pkill -f "$APP/Contents/MacOS/ddu.bin" 2>/dev/null || true
# Bounded teardown wait (pgrep re-checks; no sleeps).
n=0
while pgrep -f "$APP/Contents/MacOS/ddu.bin" >/dev/null 2>&1; do
  n=$((n + 1))
  [ "$n" -gt 50 ] && { pkill -9 -f "$APP/Contents/MacOS/ddu.bin"; break; }
done

DDU_DIR="${DDU_DIR:-$HOME/Code/ddu}" exec open "$@" "$APP"
