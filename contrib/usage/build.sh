#!/bin/sh
# Build UsageTray.app into build/
set -euo pipefail
cd "$(dirname "$0")"

swift build -c release

APP="build/UsageTray.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp .build/release/UsageTray "$APP/Contents/MacOS/UsageTray"
cp Resources/Info.plist "$APP/Contents/Info.plist"
codesign --force -s - "$APP" >/dev/null 2>&1 || true
echo "Built $APP"
