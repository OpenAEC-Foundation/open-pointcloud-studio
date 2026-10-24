#!/usr/bin/env bash
# Prints the file names of a complete release, one per line; every one of
# them is published with a NAME.sha256 beside it.
#
#   expected-assets.sh NUMBER
#
# One scheme for all: open-pointcloud-studio_VERSION_TARGET.EXTENSION. A
# package that says by its format which system it is for names only the
# processor (x64-setup.exe, amd64.deb, amd64.AppImage); an archive names the
# system too. The endings _x64-setup.exe, .dmg, .AppImage and _amd64.deb are
# what the download buttons of the foundation's website look for, and that
# script takes the first match in name order, so the x86-64 AppImage has to
# sort before the ARM one: amd64 and arm64 do, x86_64 and aarch64 would not.
#
# The four archives open-pointcloud-studio-sdk_VERSION_TARGET hold the C
# library (native/sdk/packaging/build-sdk-archive.sh). Their names end in
# none of the patterns of the website's download script (shared/downloads.js:
# _x64-setup.exe, .dmg, .AppImage, _amd64.deb, _windows-x64.zip,
# _macos-universal.tar.gz, _linux-amd64.tar.gz, linux-arm64.tar.gz): ending
# them in -dll, -dylib or -so keeps a download button from offering a
# library instead of the application. native/sdk/packaging/test-sdk-packaging.sh
# holds them against a copy of those patterns.
#
# The last two files are the source of the Open CAD Studio that every
# package carries, written by archive-open-cad-studio-source.sh for the
# commit that open-cad-studio.pin names: open-cad-studio-source_SHORT.tar.gz
# with every file of that commit, and open-cad-studio-vendor_SHORT.tar.gz with
# the crates it takes from git repositories. Their names end in none of the
# endings of the download buttons, so they never offer them.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

number=${1:?usage: expected-assets.sh NUMBER}
base="${BINARY_NAME}_${number}"
sdk="${BINARY_NAME}-sdk_${number}"

cat <<EOF
${base}_x64-setup.exe
${base}_windows-x64.zip
${base}_macos-universal.dmg
${base}_macos-universal.tar.gz
${base}_amd64.AppImage
${base}_arm64.AppImage
${base}_amd64.deb
${base}_arm64.deb
${base}_linux-amd64.tar.gz
${base}_linux-arm64.tar.gz
${sdk}_windows-x64-dll.zip
${sdk}_macos-universal-dylib.tar.gz
${sdk}_linux-amd64-so.tar.gz
${sdk}_linux-arm64-so.tar.gz
$(cad_source_archive_name)
$(cad_vendor_archive_name)
EOF
