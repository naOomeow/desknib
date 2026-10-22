#!/usr/bin/env bash
# Renders Nib's sprite strips from nib.html with headless Chromium.
# Needs ImageMagick (convert) for cropping.
# Usage: tools/nib-art/render.sh [path-to-chromium]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
out="$here/../../characters/nib"
chrome="${1:-${CHROMIUM:-chromium}}"
declare -A frames=([idle]=8 [sit]=4 [sleep]=4 [dance]=8)
for anim in "${!frames[@]}"; do
  width=$((160 * frames[$anim]))
  "$chrome" --headless --no-sandbox --disable-gpu --hide-scrollbars \
    --default-background-color=00000000 --window-size="$width,400" \
    --screenshot="$out/$anim.png" "file://$here/nib.html?anim=$anim" 2>/dev/null
  # Headless Chromium pads the capture, so crop back to the strip itself.
  convert "$out/$anim.png" -crop "${width}x184+0+0" +repage "$out/$anim.png"
done
