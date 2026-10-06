#!/usr/bin/env bash
# Packs the application binary, the Open CAD Studio binary that goes beside
# it and the licence texts into an archive.
#
#   build-archive.sh BINARY CAD_BINARY NUMBER TARGET OUT_DIR
#
# CAD_BINARY is what build-open-cad-studio.sh built for the same system; for
# Windows a build with the MSVC toolchain (--target x86_64-pc-windows-msvc),
# as the Packages workflow makes it. A build with the GNU toolchain is
# refused: it loads the C++ runtime of MinGW, which the package does not
# carry, and would start only where that lies on the search path.
# TARGET is windows-x64, macos-universal, linux-amd64 or linux-arm64. Writes
# OUT_DIR/open-pointcloud-studio_NUMBER_TARGET.zip for Windows and .tar.gz for
# the others, each with a .sha256, and leaves the packed folder of the same
# name in OUT_DIR: the Windows installer is built from it.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[[ $# -eq 5 ]] || fail "usage: build-archive.sh BINARY CAD_BINARY NUMBER TARGET OUT_DIR"
binary=$1
cad_binary=$2
number=$3
target=$4
out_dir=$5

[[ -f "$binary" ]] || fail "$binary does not exist"
[[ -f "$cad_binary" ]] || fail "$cad_binary does not exist; build it with build-open-cad-studio.sh"
case "$target" in
    windows-*)
        cad_name=$CAD_BINARY_NAME.exe
        refuse_mingw_runtime "$cad_binary"
        ;;
    *) cad_name=$CAD_BINARY_NAME ;;
esac
package="${BINARY_NAME}_${number}_${target}"

mkdir -p "$out_dir"
rm -rf "${out_dir:?}/$package"
mkdir "$out_dir/$package"
cp "$binary" "$out_dir/$package/"
# A binary downloaded from another job has lost its executable bit.
install -m755 "$cad_binary" "$out_dir/$package/$cad_name"
copy_licences "$out_dir/$package" "$number"

cd "$out_dir"
case "$target" in
    windows-*)
        archive="$package.zip"
        rm -f "$archive"
        7z a -bso0 "$archive" "$package"
        ;;
    *)
        archive="$package.tar.gz"
        # The variable keeps the tar of macOS from adding "._" entries with
        # file attributes, which show up as stray files on other systems.
        COPYFILE_DISABLE=1 tar -czf "$archive" "$package"
        ;;
esac
write_sha256 "$archive"
echo "$out_dir/$archive"
