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
# The last file is the source of the Open CAD Studio that every package
# carries, open-cad-studio-source_SHORT.tar.gz for the commit that
# open-cad-studio.pin names (archive-open-cad-studio-source.sh). Its name
# ends in none of the endings of the download buttons, so they never offer it.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

number=${1:?usage: expected-assets.sh NUMBER}
base="${BINARY_NAME}_${number}"

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
$(cad_source_archive_name)
EOF
