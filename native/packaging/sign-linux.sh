#!/usr/bin/env bash
# Signs the checksums and the AppImages of a release with the OpenPGP key of
# the organisation: SHA256SUMS.asc and NAME.AppImage.asc beside each of them
# in ASSET_DIR, detached and armoured, each verified after it is written.
#
#   sign-linux.sh ASSET_DIR NUMBER
#
# The key comes from three variables, which the release workflow fills from
# the secrets and variables of the organisation:
#
#   OPENAEC_GPG_PRIVATE_KEY   the secret signing subkey, armoured
#   OPENAEC_GPG_PASSPHRASE    its passphrase
#   OPENAEC_GPG_FINGERPRINT   the fingerprint of the key that has to sign
#
# While any of them is empty, as until the key exists, nothing is signed: the
# script says so and ends with success. A key whose fingerprint is not
# OPENAEC_GPG_FINGERPRINT is refused. The key lives in a temporary GnuPG home
# for the run only. The script writes gpg_signed=true or gpg_signed=false to
# GITHUB_OUTPUT when that is set, else to standard output.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[[ $# -eq 2 ]] || fail "usage: sign-linux.sh ASSET_DIR NUMBER"
assets=$1
number=$2

result() {
    if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
        echo "gpg_signed=$1" >> "$GITHUB_OUTPUT"
    else
        echo "gpg_signed=$1"
    fi
}

if [[ -z "${OPENAEC_GPG_PRIVATE_KEY:-}" || -z "${OPENAEC_GPG_PASSPHRASE:-}" || -z "${OPENAEC_GPG_FINGERPRINT:-}" ]]; then
    echo "::notice::The OpenPGP key of the organisation is not configured (OPENAEC_GPG_PRIVATE_KEY, OPENAEC_GPG_PASSPHRASE, OPENAEC_GPG_FINGERPRINT): SHA256SUMS and the AppImages are not signed."
    result false
    exit 0
fi
command -v gpg > /dev/null 2>&1 || fail "gpg is not installed, so nothing can be signed"

files=(SHA256SUMS)
while IFS= read -r name; do
    files+=("$name")
done < <(bash "$packaging_dir/expected-assets.sh" "$number" | grep '\.AppImage$')
for file in "${files[@]}"; do
    [[ -s "$assets/$file" ]] || fail "$assets/$file is missing"
done

expected=$(tr -d ' ' <<< "$OPENAEC_GPG_FINGERPRINT" | tr 'a-f' 'A-F')
[[ "$expected" =~ ^[0-9A-F]{40}([0-9A-F]{24})?$ ]] \
    || fail "OPENAEC_GPG_FINGERPRINT is not the fingerprint of a key (40 or 64 hex digits)"

# A short home folder: the agent puts its socket there.
home=$(mktemp -d "${TMPDIR:-/tmp}/gpg.XXXXXX")
cleanup() {
    gpgconf --homedir "$home" --kill all > /dev/null 2>&1 || true
    rm -rf "$home"
}
trap cleanup EXIT
chmod 700 "$home"
printf '%s' "$OPENAEC_GPG_PASSPHRASE" > "$home/passphrase"
gpg_() {
    gpg --homedir "$home" --batch --no-tty --pinentry-mode loopback --passphrase-file "$home/passphrase" "$@"
}

printf '%s\n' "$OPENAEC_GPG_PRIVATE_KEY" | gpg_ --import 2> "$home/import.log" \
    || { cat "$home/import.log" >&2; fail "the key could not be imported"; }
imported=$(gpg_ --list-secret-keys --with-colons --fingerprint | awk -F: '$1 == "fpr" { print $10 }')
grep -qxF "$expected" <<< "$imported" \
    || fail "the key is not the one of OPENAEC_GPG_FINGERPRINT ($expected); it has $(echo $imported)"

for file in "${files[@]}"; do
    rm -f "$assets/$file.asc"
    gpg_ --local-user "$expected" --armor --detach-sign --output "$assets/$file.asc" "$assets/$file"
    status=$(gpg --homedir "$home" --batch --no-tty --status-fd 1 --verify "$assets/$file.asc" "$assets/$file" 2> /dev/null || true)
    grep -E "^\[GNUPG:\] VALIDSIG " <<< "$status" | grep -qF "$expected" \
        || fail "the signature $file.asc does not verify as one of $expected"
    echo "signed $file: $file.asc"
done
result true
