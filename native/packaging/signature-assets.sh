#!/usr/bin/env bash
# Prints, one per line, the files a release carries besides those that
# expected-assets.sh lists: SHA256SUMS always, and with KEEP_SIGNATURES=true
# the OpenPGP signatures that sign-linux.sh writes, SHA256SUMS.asc and an
# .asc for each AppImage. They have no .sha256 of their own.
#
#   signature-assets.sh NUMBER
#
# The release workflow uploads them, and prune-release-assets.sh keeps them
# on the draft.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

number=${1:?usage: signature-assets.sh NUMBER}

echo SHA256SUMS
if [[ "${KEEP_SIGNATURES:-false}" == true ]]; then
    echo SHA256SUMS.asc
    bash "$packaging_dir/expected-assets.sh" "$number" | sed -n 's/\.AppImage$/.AppImage.asc/p'
fi
