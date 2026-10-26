#!/usr/bin/env bash
# Packs the C library of one system into the archive that the release
# carries: the library, its header, the example, the README, the licence
# texts and the notices of the Rust crates it is built from.
#
#   build-sdk-archive.sh LIB_DIR NUMBER TARGET OUT_DIR
#
# LIB_DIR  where the release build left the library: libops_pointcloud.so
#          (Linux), libops_pointcloud.dylib (macOS, both halves joined with
#          lipo), or ops_pointcloud.dll and ops_pointcloud.dll.lib (Windows,
#          built for x86_64-pc-windows-msvc with the static C runtime)
# NUMBER   the version YYYY.W.N
# TARGET   windows-x64-dll, linux-amd64-so, linux-arm64-so or
#          macos-universal-dylib
#
# Writes OUT_DIR/open-pointcloud-studio-sdk_NUMBER_TARGET.zip for Windows
# and .tar.gz for the others, each with a .sha256, and leaves the packed
# folder of the same name in OUT_DIR. The names end in none of the endings
# that the download buttons of the foundation's website look for (see
# native/packaging/expected-assets.sh).
#
# THIRD-PARTY-NOTICES.txt is written by native/tools/third-party-notices.py
# for the Rust targets of TARGET, without the network: run this after the
# release build, which downloaded every crate the notices name.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../../packaging/common.sh"

[[ $# -eq 4 ]] || fail "usage: build-sdk-archive.sh LIB_DIR NUMBER TARGET OUT_DIR"
lib_dir=$1
number=$2
target=$3
out_dir=$4
sdk_dir=$native_dir/sdk

case "$target" in
    windows-x64-dll) triples=(x86_64-pc-windows-msvc) ;;
    linux-amd64-so) triples=(x86_64-unknown-linux-gnu) ;;
    linux-arm64-so) triples=(aarch64-unknown-linux-gnu) ;;
    macos-universal-dylib) triples=(aarch64-apple-darwin x86_64-apple-darwin) ;;
    *) fail "unknown target $target: windows-x64-dll, linux-amd64-so, linux-arm64-so or macos-universal-dylib" ;;
esac

# The Python of the build machine: python3 where it exists, as on Linux and
# macOS, else python, as on Windows.
python=
for candidate in python3 python; do
    if "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 8))' > /dev/null 2>&1; then
        python=$candidate
        break
    fi
done
[[ -n "$python" ]] || fail "no Python 3.8 or newer found, which writes the notices"

need() {
    [[ -f "$lib_dir/$1" ]] || fail "$lib_dir/$1 does not exist; build the library first"
}

package="${BINARY_NAME}-sdk_${number}_${target}"
mkdir -p "$out_dir"
folder="$out_dir/$package"
rm -rf "$folder"
mkdir -p "$folder/include" "$folder/lib" "$folder/examples"

case "$target" in
    linux-*)
        need libops_pointcloud.so
        # The file carries the name the library is linked by (its soname,
        # with the major version of the interface); the link without the
        # number is what a linker looks for with -lops_pointcloud.
        cp "$lib_dir/libops_pointcloud.so" "$folder/lib/libops_pointcloud.so.1"
        (cd "$folder/lib" && ln -s libops_pointcloud.so.1 libops_pointcloud.so)
        ;;
    macos-*)
        need libops_pointcloud.dylib
        cp "$lib_dir/libops_pointcloud.dylib" "$folder/lib/"
        ;;
    windows-*)
        need ops_pointcloud.dll
        need ops_pointcloud.dll.lib
        mkdir -p "$folder/bin"
        cp "$lib_dir/ops_pointcloud.dll" "$folder/bin/"
        cp "$lib_dir/ops_pointcloud.dll.lib" "$folder/lib/"
        ;;
esac
cp "$sdk_dir/include/ops_pointcloud.h" "$folder/include/"
cp "$sdk_dir/examples/inspect.c" "$folder/examples/"
cp "$sdk_dir/README.md" "$folder/README.md"
cp "$repo_dir/LICENSE.md" "$folder/LICENSE-LGPL-3.0.md"
cp "$native_dir/desktop/LICENSE-GPL-3.0" "$folder/LICENSE-GPL-3.0.txt"

target_arguments=()
for triple in "${triples[@]}"; do
    target_arguments+=(--target "$triple")
done
CARGO_NET_OFFLINE=true "$python" -I "$native_dir/tools/third-party-notices.py" \
    --manifest-path "$native_dir/Cargo.toml" --package pointcloud-sdk "${target_arguments[@]}" \
    --output "$folder/THIRD-PARTY-NOTICES.txt"

cd "$out_dir"
case "$target" in
    windows-*)
        archive="$package.zip"
        rm -f "$archive"
        "$python" -I -m zipfile -c "$archive" "$package"
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
