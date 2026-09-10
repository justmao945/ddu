#!/bin/sh
# Build a self-contained ddu .app bundle (Info.plist + icon + launcher).
#
# Usage: scripts/make-bundle.sh <app-dir> <display-name> <bundle-id>
#   (run `cargo build --release` first; requires target/release/ddu)
#
# Callers: scripts/dev.sh ("Day Day Up Dev" / dev.just.ddu.dev at
# target/ddu-dev.app) and scripts/install.sh ("Day Day Up" /
# dev.just.ddu, straight into /Applications). The distinct bundle ids
# are what let a dev copy and an installed copy run side by side without
# LaunchServices activating the wrong one.
#
# DDU_* env present at bundle time is baked into the generated launcher:
# DDU_VERIFY_SETTINGS / DDU_VERIFY_EXIT / DDU_VERIFY_CLOSE (dev hooks,
# see main.rs), DDU_STATE_PATH / DDU_SETTINGS_PATH (state isolation),
# DDU_DEBUG (trace). Omitted entirely when unset so a clean bundle can't
# leak empty vars.
set -e
cd "$(dirname "$0")/.."

APP="${1:?usage: $0 <app-dir> <display-name> <bundle-id>}"
NAME="${2:?usage: $0 <app-dir> <display-name> <bundle-id>}"
ID="${3:?usage: $0 <app-dir> <display-name> <bundle-id>}"
BIN=target/release/ddu

[ -x "$BIN" ] || { echo "run: cargo build --release" >&2; exit 1; }

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
scripts/make-icon.sh "$APP/Contents/Resources/ddu.icns"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>$ID</string>
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
LAUNCH
# Dev hooks baked at bundle time (see main.rs / app/mod.rs): auto-open
# Settings on a page, freeze the Exiting overlay, close the current
# session, point at a scratch state file, or enable the debug trace.
# Written OUTSIDE the heredoc — inside it, ${v:+A="$v"} expansion strips
# the inner quotes (verified), which truncates unquoted values containing
# spaces (e.g. "Application Support").
{
  [ -n "${DDU_VERIFY_SETTINGS:-}" ] && printf 'export DDU_VERIFY_SETTINGS="%s"\n' "$DDU_VERIFY_SETTINGS"
  [ -n "${DDU_VERIFY_EXIT:-}" ] && printf 'export DDU_VERIFY_EXIT="%s"\n' "$DDU_VERIFY_EXIT"
  [ -n "${DDU_VERIFY_CLOSE:-}" ] && printf 'export DDU_VERIFY_CLOSE="%s"\n' "$DDU_VERIFY_CLOSE"
  [ -n "${DDU_STATE_PATH:-}" ] && printf 'export DDU_STATE_PATH="%s"\n' "$DDU_STATE_PATH"
  [ -n "${DDU_SETTINGS_PATH:-}" ] && printf 'export DDU_SETTINGS_PATH="%s"\n' "$DDU_SETTINGS_PATH"
  [ -n "${DDU_DEBUG:-}" ] && printf 'export DDU_DEBUG="%s"\n' "$DDU_DEBUG"
} >> "$APP/Contents/MacOS/launch.sh"
# The exec MUST stay the launcher's last line. Appending anything after it
# (the baked exports used to land here) writes code the shell never
# reaches, so the values were silently ignored and every launch fell back
# to whatever environment LaunchServices happened to provide.
cat >> "$APP/Contents/MacOS/launch.sh" <<'LAUNCH_TAIL'
exec "$(dirname "$0")/ddu.bin"
LAUNCH_TAIL
chmod +x "$APP/Contents/MacOS/launch.sh"
