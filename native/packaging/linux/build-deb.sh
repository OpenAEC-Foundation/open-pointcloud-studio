#!/usr/bin/env bash
# Builds the .deb package for the processor type of this machine.
#
#   build-deb.sh BINARY CAD_BINARY ICONS NUMBER DEB_VERSION DATE OUT_DIR
#
# Writes OUT_DIR/open-pointcloud-studio_NUMBER_ARCH.deb with a .sha256, where
# ARCH is what dpkg calls this machine (amd64, arm64). DEB_VERSION is the
# version inside the package, which writes a pre-release as 1.2.3~rc1.
# CAD_BINARY is the Open CAD Studio that build-open-cad-studio.sh built for
# this machine; stage-tree.sh says where it goes.
#
# The package has no maintainer scripts: the file triggers of the MIME
# database, the desktop entry cache and the icon cache refresh them when the
# package is installed or removed.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

[[ $# -eq 7 ]] || fail "usage: build-deb.sh BINARY CAD_BINARY ICONS NUMBER DEB_VERSION DATE OUT_DIR"
binary=$1
cad_binary=$2
icons=$3
number=$4
deb_version=$5
date=$6
out_dir=$7
here=$packaging_dir/linux

arch=$(dpkg --print-architecture)
mkdir -p "$out_dir"
out_dir=$(cd "$out_dir" && pwd)
package="$out_dir/${BINARY_NAME}_${number}_${arch}.deb"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
umask 022
# Not the temporary folder itself: its private mode would become the mode of
# the top folder in the package.
root=$work/root
mkdir "$root"

bash "$here/stage-tree.sh" "$root" "$binary" "$cad_binary" "$icons" "$number" "$date"
install -Dm644 "$here/copyright" "$root/usr/share/doc/$BINARY_NAME/copyright"
desktop-file-validate "$root/usr/share/applications/$APP_ID.desktop"

# The size is that of the installed files, so it is taken before the control
# folder exists.
size=$(du -sk "$root" | cut -f1)
mkdir -p "$root/DEBIAN"
fill_template "$here/control.in" "$root/DEBIAN/control" \
    "DEBVERSION=$deb_version" "ARCH=$arch" "SIZE=$size"

# xz, because the zstd default of newer tools cannot be read by every dpkg
# that is still in use.
dpkg-deb --root-owner-group -Zxz --build "$root" "$package"
dpkg-deb --info "$package"
dpkg-deb --contents "$package"
write_sha256 "$package"
echo "$package"
