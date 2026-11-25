#!/usr/bin/env bash
# Signs the installers that the OpenAEC Installer updates: NAME.sig beside the
# Windows setup and each AppImage in ASSET_DIR, a minisign signature in the
# form the Tauri updater writes. The OpenAEC Installer checks it against the
# public key in its catalogue before it updates silently
# (OpenAEC-Foundation/OpenAEC-Blueprint#33).
#
#   sign-updates.sh ASSET_DIR NUMBER
#
# The key comes from two variables, which the release workflow fills from the
# secrets of this repository:
#
#   TAURI_SIGNING_PRIVATE_KEY            the secret updater key of the application
#   TAURI_SIGNING_PRIVATE_KEY_PASSWORD   its password
#
# It runs after the Authenticode and OpenPGP signing, because the signature
# has to cover the file as it is published. While a variable is empty nothing
# is signed: the script says so and ends with success. It writes
# updater_signed=true or updater_signed=false to GITHUB_OUTPUT when that is
# set, else to standard output.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[[ $# -eq 2 ]] || fail "usage: sign-updates.sh ASSET_DIR NUMBER"
assets=$1
number=$2

result() {
    if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
        echo "updater_signed=$1" >> "$GITHUB_OUTPUT"
    else
        echo "updater_signed=$1"
    fi
}

if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" || -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]]; then
    echo "::notice::The updater key is not configured (TAURI_SIGNING_PRIVATE_KEY, TAURI_SIGNING_PRIVATE_KEY_PASSWORD): the installers carry no .sig, and the OpenAEC Installer updates them without checking a signature."
    result false
    exit 0
fi

while IFS= read -r name; do
    [[ -f "$assets/$name" ]] || fail "$name is missing from $assets"
    rm -f "$assets/$name.sig"
    # The Tauri CLI writes NAME.sig; the key and its password come from the
    # two variables above, so they never appear on a command line.
    npx --yes @tauri-apps/cli@2 signer sign "$assets/$name" > /dev/null
    [[ -s "$assets/$name.sig" ]] || fail "no signature was written for $name"
    echo "signed $name for the OpenAEC Installer"
done < <(bash "$packaging_dir/expected-assets.sh" "$number" | grep -E '(_x64-setup\.exe|\.AppImage)$')

result true
