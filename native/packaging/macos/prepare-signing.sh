#!/usr/bin/env bash
# Import an optional Developer ID certificate and App Store Connect API key.
# A partially configured release must fail instead of silently shipping ad hoc.
#
# The six values are the organisation secrets that every OpenAEC app uses:
#   APPLE_CERTIFICATE           the Developer ID Application certificate, a .p12 in base64
#   APPLE_CERTIFICATE_PASSWORD  the password of that .p12
#   APPLE_SIGNING_IDENTITY      the full identity, "Developer ID Application: ..."
#   APPLE_API_KEY_CONTENT       the App Store Connect .p8 key, as its text or in base64
#   APPLE_API_KEY               the key ID of that key
#   APPLE_API_ISSUER            its issuer ID
set -euo pipefail

fields=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD
        APPLE_SIGNING_IDENTITY APPLE_API_KEY_CONTENT
        APPLE_API_KEY APPLE_API_ISSUER)
present=0
for field in "${fields[@]}"; do
    [[ -z "${!field:-}" ]] || ((present += 1))
done
if ((present == 0)); then
    echo 'Developer ID credentials are absent; the macOS package will be ad hoc signed.'
    echo 'macos_signed=false' >> "$GITHUB_OUTPUT"
    exit 0
fi
if ((present != ${#fields[@]})); then
    echo '::error::Apple signing credentials are incomplete; configure all six APPLE_* secrets.'
    exit 1
fi

certificate="$RUNNER_TEMP/ops-developer-id.p12"
notary_key="$RUNNER_TEMP/ops-notary-key.p8"
keychain="$RUNNER_TEMP/ops-signing.keychain-db"
printf '%s' "$APPLE_CERTIFICATE" | base64 -D > "$certificate"
# The key is kept as the text of the .p8 file by the other OpenAEC apps; a
# base64 form is accepted as well.
if [[ "$APPLE_API_KEY_CONTENT" == *"-----BEGIN"* ]]; then
    printf '%s\n' "$APPLE_API_KEY_CONTENT" > "$notary_key"
else
    printf '%s' "$APPLE_API_KEY_CONTENT" | base64 -D > "$notary_key"
fi
chmod 600 "$certificate" "$notary_key"
password=$(uuidgen)
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$certificate" -k "$keychain" -P "$APPLE_CERTIFICATE_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain"
security list-keychains -d user -s "$keychain"
security find-identity -v -p codesigning "$keychain" | grep -Fq "$APPLE_SIGNING_IDENTITY" \
    || { echo '::error::the requested Developer ID Application identity was not imported'; exit 1; }
rm -f "$certificate"
{
    echo "MACOS_SIGN_IDENTITY=$APPLE_SIGNING_IDENTITY"
    echo "MACOS_SIGN_KEYCHAIN=$keychain"
    echo "MACOS_NOTARY_KEY=$notary_key"
    echo "MACOS_NOTARY_KEY_ID=$APPLE_API_KEY"
    echo "MACOS_NOTARY_ISSUER_ID=$APPLE_API_ISSUER"
} >> "$GITHUB_ENV"
echo 'macos_signed=true' >> "$GITHUB_OUTPUT"
echo 'Developer ID signing and Apple notarization are enabled.'
