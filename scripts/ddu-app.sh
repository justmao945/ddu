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
cat > "$APP/Contents/MacOS/launch.sh" <<'LAUNCH'
#!/bin/sh
cd "${DDU_DIR:-$HOME/Code/ddu}" || exit 1
exec "$(dirname "$0")/ddu.bin"
LAUNCH
chmod +x "$APP/Contents/MacOS/launch.sh"

DDU_DIR="${DDU_DIR:-$HOME/Code/ddu}" exec open "$APP"
