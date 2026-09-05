#!/bin/sh
# Build the app icon: assets/icon.svg → PNG sizes → .icns.
#
# Pipeline: `qlmanage` (QuickLook / CoreSVG) rasterizes the SVG at 1024px,
# `sips` downscales the macOS iconset ladder, `iconutil` packs the .icns.
#
# Usage: scripts/make-icon.sh [out.icns]   (default: /tmp/ddu.icns)
set -e
cd "$(dirname "$0")/.."

OUT="${1:-/tmp/ddu.icns}"
SRC=assets/icon.svg
SET="$(mktemp -d)/ddu.iconset"
mkdir -p "$SET"

qlmanage -t -s 1024 -o "$(dirname "$SRC")" "$SRC" >/dev/null 2>&1
PNG="$(dirname "$SRC")/icon.svg.png"
trap 'rm -f "$PNG"' EXIT

for s in 16 32 128 256 512; do
  sips -z $s $s "$PNG" --out "$SET/icon_${s}x${s}.png" >/dev/null
  d=$((s * 2))
  sips -z $d $d "$PNG" --out "$SET/icon_${s}x${s}@2x.png" >/dev/null
done

iconutil -c icns "$SET" -o "$OUT"
echo "icon: $OUT"
