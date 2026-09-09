#!/bin/sh
# Build the app icon: assets/icon.svg → PNG sizes → .icns.
#
# Every iconset size is rasterized straight from the SVG with AppKit
# (`swift scripts/svg2png.swift`, keeps the rounded rect's alpha —
# `qlmanage` would bake an opaque white square behind it, a white halo in
# the Dock; `sips` downscales ring bright AA pixels into the corners at
# 16px). `iconutil` packs the .icns.
#
# Usage: scripts/make-icon.sh [out.icns]   (default: /tmp/ddu.icns)
set -e
cd "$(dirname "$0")/.."

OUT="${1:-/tmp/ddu.icns}"
SRC=assets/icon.svg
SET="$(mktemp -d)/ddu.iconset"
mkdir -p "$SET"

for s in 16 32 128 256 512; do
  swift scripts/svg2png.swift "$SRC" "$SET/icon_${s}x${s}.png" $s
  d=$((s * 2))
  swift scripts/svg2png.swift "$SRC" "$SET/icon_${s}x${s}@2x.png" $d
done

iconutil -c icns "$SET" -o "$OUT"
echo "icon: $OUT"
