#!/usr/bin/env bash
# Builds the AppImage for the processor type of this machine.
#
#   build-appimage.sh BINARY CAD_BINARY ICONS NUMBER DATE OUT_DIR
#
# Writes OUT_DIR/open-pointcloud-studio_NUMBER_ARCH.AppImage with a .sha256;
# ARCH is amd64 or arm64, as for the .deb. CAD_BINARY is the Open CAD Studio
# that build-open-cad-studio.sh built for this machine; stage-tree.sh says
# where it goes.
#
# An AppImage is a small runtime followed by a compressed image of the
# application folder. No libraries are bundled: the application links only
# the C library, Open CAD Studio also the C++ runtime of the system
# (libstdc++), and window system and graphics driver libraries have to come
# from the system it runs on in any case. check-binary.sh holds both binaries
# to the versions of the C library and the C++ runtime that the oldest
# supported systems have.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

[[ $# -eq 6 ]] || fail "usage: build-appimage.sh BINARY CAD_BINARY ICONS NUMBER DATE OUT_DIR"
binary=$1
cad_binary=$2
icons=$3
number=$4
date=$5
out_dir=$6
here=$packaging_dir/linux

# The runtime is fetched at build time from a release that does not change,
# and refused unless it is the file that was looked at when it was pinned.
# NOTICE.txt and AppImage-runtime-LICENSE.txt name this release: change them
# together.
runtime_release=20251108
machine=$(uname -m)
case "$machine" in
    x86_64)
        arch=amd64
        runtime_sha256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
        ;;
    aarch64)
        arch=arm64
        runtime_sha256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
        ;;
    *)
        fail "no AppImage runtime is pinned for $machine"
        ;;
esac
runtime_url="https://github.com/AppImage/type2-runtime/releases/download/$runtime_release/runtime-$machine"

mkdir -p "$out_dir"
out_dir=$(cd "$out_dir" && pwd)
package="$out_dir/${BINARY_NAME}_${number}_${arch}.AppImage"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
umask 022

curl --fail --silent --show-error --location --retry 3 --output "$work/runtime" "$runtime_url"
found=$(sha256sum "$work/runtime" | cut -d' ' -f1)
[[ "$found" == "$runtime_sha256" ]] \
    || fail "$runtime_url has SHA-256 $found, expected $runtime_sha256"
# Bytes 8 to 10 mark the file as an AppImage of type 2 for tools that look.
[[ "$(od -An -tx1 -j8 -N3 "$work/runtime" | tr -d ' \n')" == "414902" ]] \
    || fail "the runtime does not carry the AppImage type 2 mark"

appdir=$work/AppDir
bash "$here/stage-tree.sh" "$appdir" "$binary" "$cad_binary" "$icons" "$number" "$date"

# Outside a Debian system there is no folder of common licence texts to
# point at, so this package carries them.
copy_licences "$appdir/usr/share/doc/$BINARY_NAME" "$number"
install -m644 "$here/AppImage-runtime-LICENSE.txt" "$appdir/usr/share/doc/$BINARY_NAME/"

# What the runtime and the desktop integration tools look for at the top.
ln -s "usr/bin/$BINARY_NAME" "$appdir/AppRun"
cp "$appdir/usr/share/applications/$APP_ID.desktop" "$appdir/$APP_ID.desktop"
echo "X-AppImage-Version=$number" >> "$appdir/$APP_ID.desktop"
cp "$appdir/usr/share/icons/hicolor/256x256/apps/$APP_ID.png" "$appdir/$APP_ID.png"
ln -s "$APP_ID.png" "$appdir/.DirIcon"
desktop-file-validate "$appdir/$APP_ID.desktop"

mksquashfs "$appdir" "$work/app.squashfs" -root-owned -noappend -no-progress \
    -comp zstd -Xcompression-level 19
cat "$work/runtime" "$work/app.squashfs" > "$package"
chmod 755 "$package"
write_sha256 "$package"
echo "$package"
