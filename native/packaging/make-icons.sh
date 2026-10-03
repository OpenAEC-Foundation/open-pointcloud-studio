#!/usr/bin/env bash
# Makes the icons of the Linux and macOS packages from the logo in
# native/assets/icons.
#
#   make-icons.sh OUT_DIR [--svg-only]
#
# The logo (logo.svg, nine points) is the only drawing that is kept by hand.
# This script puts it on the dark rounded tile of icon.png and renders:
#
#   OUT_DIR/icon.svg                      the tile icon as a vector drawing
#   OUT_DIR/icon-macos.svg                the same, inset as macOS expects
#   OUT_DIR/hicolor/NxN/apps/ID.png       32 to 512 pixels, for Linux
#   OUT_DIR/hicolor/scalable/apps/ID.svg
#   OUT_DIR/AppIcon.iconset/              the ten sizes that `iconutil` turns
#                                         into AppIcon.icns on macOS
#
# Rendering needs rsvg-convert (package librsvg2-bin); --svg-only writes the
# two drawings and stops, for a machine without it.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

out=${1:?usage: make-icons.sh OUT_DIR [--svg-only]}
svg_only=${2:-}
logo=$native_dir/assets/icons/logo.svg

# Tile colour, corner radius and placement of the points are those of
# icon.png (512 pixels), which the window and the Windows executable use: the
# 20-unit drawing of the logo is scaled to 426.67 pixels and centred.
tile_colour='#27272a'
points_colour=$(sed -n 's/.*<svg[^>]* fill="\([^"]*\)".*/\1/p' "$logo")
circles=$(grep '<circle' "$logo" | sed 's/^ */    /')
[[ -n "$points_colour" && -n "$circles" ]] || fail "$logo no longer has a fill colour and circles"

mkdir -p "$out"
cat > "$out/icon.svg" <<EOF
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512">
  <rect width="512" height="512" rx="112" fill="$tile_colour"/>
  <g transform="translate(42.667 42.667) scale(21.3333)" fill="$points_colour">
$circles
  </g>
</svg>
EOF

# macOS draws no tile of its own: an icon fills 824 of 1024 pixels and leaves
# the rest transparent, or it looks larger than its neighbours in the Dock.
cat > "$out/icon-macos.svg" <<EOF
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">
  <g transform="translate(100 100) scale(1.609375)">
    <rect width="512" height="512" rx="112" fill="$tile_colour"/>
    <g transform="translate(42.667 42.667) scale(21.3333)" fill="$points_colour">
$circles
    </g>
  </g>
</svg>
EOF

if [[ "$svg_only" == "--svg-only" ]]; then
    exit 0
fi
command -v rsvg-convert >/dev/null 2>&1 || fail "rsvg-convert is not installed (package librsvg2-bin)"

render() {
    local source=$1 size=$2 target=$3
    mkdir -p "$(dirname "$target")"
    rsvg-convert -w "$size" -h "$size" "$source" -o "$target"
}

for size in 32 48 64 128 256 512; do
    render "$out/icon.svg" "$size" "$out/hicolor/${size}x${size}/apps/$APP_ID.png"
done
mkdir -p "$out/hicolor/scalable/apps"
cp "$out/icon.svg" "$out/hicolor/scalable/apps/$APP_ID.svg"

# Each size once at normal and once at double density; the names are fixed.
for size in 16 32 128 256 512; do
    render "$out/icon-macos.svg" "$size" "$out/AppIcon.iconset/icon_${size}x${size}.png"
    render "$out/icon-macos.svg" "$((size * 2))" "$out/AppIcon.iconset/icon_${size}x${size}@2x.png"
done
