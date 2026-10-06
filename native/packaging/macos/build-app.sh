#!/usr/bin/env bash
# Builds the macOS application bundle and the disk image that carries it.
#
#   build-app.sh BINARY CAD_BINARY ICONSET NUMBER NUMERIC OUT_DIR
#
# BINARY      the universal binary (arm64 and x86-64 joined with lipo)
# CAD_BINARY  the universal binary of Open CAD Studio, built for both by
#             build-open-cad-studio.sh and joined the same way; it goes beside
#             BINARY in Contents/MacOS, where the application looks for it
# ICONSET     the AppIcon.iconset folder that make-icons.sh wrote
# NUMBER      version for the file name; NUMERIC the same without a
#             pre-release suffix, because the bundle takes three numbers only
#
# Writes OUT_DIR/open-pointcloud-studio_NUMBER_macos-universal.dmg with a
# .sha256 and leaves the bundle in OUT_DIR/Open Pointcloud Studio.app.
#
# The bundle is signed ad hoc: without any signature the arm64 half does not
# start at all. That is not a developer signature, so macOS still asks the
# user to allow the first start; first-start.txt in the image says how.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

[[ $# -eq 6 ]] || fail "usage: build-app.sh BINARY CAD_BINARY ICONSET NUMBER NUMERIC OUT_DIR"
binary=$1
cad_binary=$2
iconset=$3
number=$4
numeric=$5
out_dir=$6
here=$packaging_dir/macos

for file in "$binary" "$cad_binary"; do
    archs=$(lipo -archs "$file")
    [[ "$archs" == *arm64* && "$archs" == *x86_64* ]] \
        || fail "$file holds '$archs', expected arm64 and x86_64"
done

mkdir -p "$out_dir"
out_dir=$(cd "$out_dir" && pwd)
app="$out_dir/$APP_NAME.app"
image="$out_dir/${BINARY_NAME}_${number}_macos-universal.dmg"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/$BINARY_NAME"
chmod 755 "$app/Contents/MacOS/$BINARY_NAME"
cp "$cad_binary" "$app/Contents/MacOS/$CAD_BINARY_NAME"
chmod 755 "$app/Contents/MacOS/$CAD_BINARY_NAME"
fill_template "$here/Info.plist.in" "$app/Contents/Info.plist" "VERSION=$numeric"
plutil -lint "$app/Contents/Info.plist"
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
copy_licences "$app/Contents/Resources" "$number"

# The second program of the bundle is signed before the bundle that seals it.
codesign --force --sign - --timestamp=none "$app/Contents/MacOS/$CAD_BINARY_NAME"
codesign --force --sign - --timestamp=none "$app"
codesign --verify --strict --verbose=2 "$app"
codesign -dv "$app" 2>&1 | tee "$out_dir/codesign.txt"
grep -q 'Signature=adhoc' "$out_dir/codesign.txt" || fail "the bundle is not signed ad hoc"
rm "$out_dir/codesign.txt"

# What the mounted image shows: the application, the folder to drag it to,
# and the note about the first start.
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/$APP_NAME.app"
ln -s /Applications "$stage/Applications"
cp "$here/first-start.txt" "$stage/First start.txt"

# The disk tool now and then reports "resource busy" on a build machine.
rm -f "$image"
for attempt in 1 2 3; do
    if hdiutil create -volname "$APP_NAME" -srcfolder "$stage" -fs HFS+ -format UDZO -ov "$image"; then
        break
    fi
    [[ "$attempt" -lt 3 ]] || fail "the disk image could not be created"
    sleep 5
done
hdiutil verify "$image"
write_sha256 "$image"
echo "$image"
