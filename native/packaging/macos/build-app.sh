#!/usr/bin/env bash
# Builds the macOS application bundle and the disk image that carries it.
#
#   build-app.sh BINARY CAD_BINARY ICONSET NUMBER OUT_DIR
#
# BINARY      the universal binary (arm64 and x86-64 joined with lipo)
# CAD_BINARY  the universal binary of Open CAD Studio, built for both by
#             build-open-cad-studio.sh and joined the same way; it goes beside
#             BINARY in Contents/MacOS, where the application looks for it
# ICONSET     the AppIcon.iconset folder that make-icons.sh wrote
# NUMBER      the version YYYY.W.N, for the file name and the bundle, whose
#             version takes three numbers
#
# Writes OUT_DIR/open-pointcloud-studio_NUMBER_macos-universal.dmg with a
# .sha256 and leaves the bundle in OUT_DIR/Open Pointcloud Studio.app.
#
# Without Developer ID credentials the bundle is signed ad hoc, which arm64
# requires. With credentials it is hardened, notarized and stapled.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

[[ $# -eq 5 ]] || fail "usage: build-app.sh BINARY CAD_BINARY ICONSET NUMBER OUT_DIR"
binary=$1
cad_binary=$2
iconset=$3
number=$4
out_dir=$5
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
fill_template "$here/Info.plist.in" "$app/Contents/Info.plist" "VERSION=$number"
plutil -lint "$app/Contents/Info.plist"
iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
copy_licences "$app/Contents/Resources" "$number"

# Apple's notary and ticket services are reached over the network, which now
# and then drops a connection on a build machine: such a step is tried up to
# three times before the build fails.
with_retries() {
    local attempt
    for attempt in 1 2 3; do
        if "$@"; then
            return 0
        fi
        [[ "$attempt" -lt 3 ]] || return 1
        echo "build-app.sh: attempt $attempt failed, trying again in 20 s: $*" >&2
        sleep 20
    done
}

# Sign nested code before the bundle that seals it. Notarization also requires
# the hardened runtime and a secure timestamp for both executables.
if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
    for program in "$CAD_BINARY_NAME" "$BINARY_NAME"; do
        codesign --force --sign "$MACOS_SIGN_IDENTITY" \
            --keychain "$MACOS_SIGN_KEYCHAIN" --options runtime --timestamp \
            "$app/Contents/MacOS/$program"
    done
    codesign --force --sign "$MACOS_SIGN_IDENTITY" \
        --keychain "$MACOS_SIGN_KEYCHAIN" --options runtime --timestamp "$app"
    codesign --verify --strict --deep --verbose=2 "$app"
    zip="$out_dir/notarization.zip"
    ditto -c -k --keepParent "$app" "$zip"
    with_retries xcrun notarytool submit "$zip" --key "$MACOS_NOTARY_KEY" \
        --key-id "$MACOS_NOTARY_KEY_ID" --issuer "$MACOS_NOTARY_ISSUER_ID" \
        --wait --output-format json > "$out_dir/notary-app.json"
    python3 -c 'import json,sys; assert json.load(open(sys.argv[1]))["status"] == "Accepted"' \
        "$out_dir/notary-app.json" || fail "Apple rejected the application; see notary-app.json"
    with_retries xcrun stapler staple "$app"
    with_retries xcrun stapler validate "$app"
    rm -f "$zip" "$out_dir/notary-app.json"
else
    codesign --force --sign - --timestamp=none "$app/Contents/MacOS/$CAD_BINARY_NAME"
    codesign --force --sign - --timestamp=none "$app"
    codesign --verify --strict --verbose=2 "$app"
    codesign -dv "$app" 2>&1 | tee "$out_dir/codesign.txt"
    grep -q 'Signature=adhoc' "$out_dir/codesign.txt" || fail "the bundle is not signed ad hoc"
    rm "$out_dir/codesign.txt"
fi

# What the mounted image shows: the application, the folder to drag it to,
# and the note about the first start.
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
ditto "$app" "$stage/$APP_NAME.app"
ln -s /Applications "$stage/Applications"
if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
    cp "$here/first-start-signed.txt" "$stage/First start.txt"
else
    cp "$here/first-start.txt" "$stage/First start.txt"
fi

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
if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
    with_retries xcrun notarytool submit "$image" --key "$MACOS_NOTARY_KEY" \
        --key-id "$MACOS_NOTARY_KEY_ID" --issuer "$MACOS_NOTARY_ISSUER_ID" \
        --wait --output-format json > "$out_dir/notary-dmg.json"
    python3 -c 'import json,sys; assert json.load(open(sys.argv[1]))["status"] == "Accepted"' \
        "$out_dir/notary-dmg.json" || fail "Apple rejected the disk image; see notary-dmg.json"
    with_retries xcrun stapler staple "$image"
    with_retries xcrun stapler validate "$image"
    rm -f "$out_dir/notary-dmg.json"
fi
write_sha256 "$image"
echo "$image"
