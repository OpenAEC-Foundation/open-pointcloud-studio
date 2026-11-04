#!/usr/bin/env bash
# Verify release signatures offline with a public key pinned by fingerprint.
# No keyserver, user keyring or secret key is needed.
#   verify-linux-signatures.sh ASSET_DIR NUMBER FINGERPRINT
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/common.sh"
[[ $# -eq 3 ]] || fail "usage: verify-linux-signatures.sh ASSET_DIR NUMBER FINGERPRINT"
assets=$1
number=$2
expected=$(tr -d ' ' <<< "$3" | tr 'a-f' 'A-F')
[[ "$expected" =~ ^[0-9A-F]{40}([0-9A-F]{24})?$ ]] || fail "invalid signing fingerprint"
public_key=$assets/open-pointcloud-studio-signing-key.asc
[[ -s "$public_key" ]] || fail "the release public signing key is missing"
signing_home=$(mktemp -d "${TMPDIR:-/tmp}/ops-verify.XXXXXX")
cleanup() {
    gpgconf --homedir "$signing_home" --kill all > /dev/null 2>&1 || true
    rm -rf "$signing_home"
}
trap cleanup EXIT
chmod 700 "$signing_home"
key_info=$(gpg --homedir "$signing_home" --batch --no-tty --with-colons \
    --import-options show-only --import "$public_key" 2> /dev/null)
[[ $(awk -F: '$1 == "pub" {n++} END {print n+0}' <<< "$key_info") -eq 1 ]] \
    || fail "the release must contain exactly one public primary key"
awk -F: '$1 == "fpr" {print $10}' <<< "$key_info" | grep -qxF "$expected" \
    || fail "the public signing key does not match the expected fingerprint $expected"
gpg --homedir "$signing_home" --batch --no-tty --import "$public_key" > /dev/null 2>&1
files=(SHA256SUMS)
while IFS= read -r name; do files+=("$name"); done \
    < <(bash "$packaging_dir/expected-assets.sh" "$number" | grep '\.AppImage$')
for file in "${files[@]}"; do
    status=$(gpg --homedir "$signing_home" --batch --no-tty --status-fd 1 \
        --verify "$assets/$file.asc" "$assets/$file" 2> /dev/null) \
        || fail "the signature of $file does not verify"
    awk -v expected="$expected" \
        '$1 == "[GNUPG:]" && $2 == "VALIDSIG" && ($3 == expected || $NF == expected) {valid=1} END {exit !valid}' \
        <<< "$status" || fail "the signature of $file is not by $expected"
    echo "verified $file: $expected"
done
