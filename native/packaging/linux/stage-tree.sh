#!/usr/bin/env bash
# Lays out the files of the application the way a Linux system expects them
# under /usr. The .deb and the AppImage are both made from this tree, so
# their desktop entry, icons, file types and metadata cannot differ.
#
#   stage-tree.sh ROOT BINARY CAD_BINARY ICONS NUMBER DATE
#
# ROOT        folder that receives usr/...
# CAD_BINARY  the Open CAD Studio that build-open-cad-studio.sh built; it goes
#             to usr/lib/open-pointcloud-studio, out of the search path, where
#             the application looks for it
# ICONS       output folder of make-icons.sh
# NUMBER      version for the AppStream metadata, DATE its day (YYYY-MM-DD)
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

[[ $# -eq 6 ]] || fail "usage: stage-tree.sh ROOT BINARY CAD_BINARY ICONS NUMBER DATE"
root=$1
binary=$2
cad_binary=$3
icons=$4
number=$5
date=$6
[[ -f "$cad_binary" ]] || fail "$cad_binary does not exist; build it with build-open-cad-studio.sh"
here=$packaging_dir/linux

# Packages must not depend on the umask of whoever builds them.
umask 022

install -Dm755 "$binary" "$root/usr/bin/$BINARY_NAME"
install -Dm755 "$cad_binary" "$root/usr/lib/$BINARY_NAME/$CAD_BINARY_NAME"
install -Dm644 "$here/$APP_ID.desktop" "$root/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$here/$APP_ID.mime.xml" "$root/usr/share/mime/packages/$APP_ID.xml"

mkdir -p "$root/usr/share/metainfo"
fill_template "$here/$APP_ID.metainfo.xml.in" "$root/usr/share/metainfo/$APP_ID.metainfo.xml" \
    "VERSION=$number" "DATE=$date"

for size in 32 48 64 128 256 512; do
    install -Dm644 "$icons/hicolor/${size}x${size}/apps/$APP_ID.png" \
        "$root/usr/share/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
done
install -Dm644 "$icons/hicolor/scalable/apps/$APP_ID.svg" \
    "$root/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"

docs=$root/usr/share/doc/$BINARY_NAME
install -Dm644 "$packaging_dir/NOTICE.txt" "$docs/NOTICE.txt"
install -m644 "$native_dir"/assets/fonts/*-OFL.txt "$docs/"
# The .deb carries no licence texts of its own (see NOTICE.txt), so the
# notice of Open CAD Studio points at the GPL text the system keeps. The
# AppImage carries the texts and writes this notice again with copy_licences.
copy_cad_notice "$docs" "$number" /usr/share/common-licenses/GPL-3
