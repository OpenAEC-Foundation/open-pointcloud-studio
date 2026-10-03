#!/usr/bin/env bash
# Packs the application binary with the licence texts into an archive.
#
#   build-archive.sh BINARY NUMBER TARGET OUT_DIR
#
# TARGET is windows-x64, macos-universal, linux-amd64 or linux-arm64. Writes
# OUT_DIR/open-pointcloud-studio_NUMBER_TARGET.zip for Windows and .tar.gz for
# the others, each with a .sha256, and leaves the packed folder of the same
# name in OUT_DIR: the Windows installer is built from it.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[[ $# -eq 4 ]] || fail "usage: build-archive.sh BINARY NUMBER TARGET OUT_DIR"
binary=$1
number=$2
target=$3
out_dir=$4

[[ -f "$binary" ]] || fail "$binary does not exist"
package="${BINARY_NAME}_${number}_${target}"

mkdir -p "$out_dir"
rm -rf "${out_dir:?}/$package"
mkdir "$out_dir/$package"
cp "$binary" "$out_dir/$package/"
copy_licences "$out_dir/$package"

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
