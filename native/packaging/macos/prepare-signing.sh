#!/usr/bin/env bash
# Import an optional Developer ID certificate and App Store Connect API key.
# A partially configured release must fail instead of silently shipping ad hoc.
set -euo pipefail

fields=(APPLE_DEVELOPER_ID_P12_BASE64 APPLE_DEVELOPER_ID_P12_PASSWORD
        APPLE_DEVELOPER_IDENTITY APPLE_NOTARY_KEY_BASE64
        APPLE_NOTARY_KEY_ID APPLE_NOTARY_ISSUER_ID)
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
printf '%s' "$APPLE_DEVELOPER_ID_P12_BASE64" | base64 -D > "$certificate"
printf '%s' "$APPLE_NOTARY_KEY_BASE64" | base64 -D > "$notary_key"
chmod 600 "$certificate" "$notary_key"
password=$(uuidgen)
security create-keychain -p "$password" "$keychain"
security set-keychain-settings -lut 21600 "$keychain"
security unlock-keychain -p "$password" "$keychain"
security import "$certificate" -k "$keychain" -P "$APPLE_DEVELOPER_ID_P12_PASSWORD" -T /usr/bin/codesign
security set-key-partition-list -S apple-tool:,apple: -s -k "$password" "$keychain"
security list-keychains -d user -s "$keychain"
security find-identity -v -p codesigning "$keychain" | grep -Fq "$APPLE_DEVELOPER_IDENTITY" \
    || { echo '::error::the requested Developer ID Application identity was not imported'; exit 1; }
rm -f "$certificate"
{
    echo "MACOS_SIGN_IDENTITY=$APPLE_DEVELOPER_IDENTITY"
    echo "MACOS_SIGN_KEYCHAIN=$keychain"
    echo "MACOS_NOTARY_KEY=$notary_key"
    echo "MACOS_NOTARY_KEY_ID=$APPLE_NOTARY_KEY_ID"
    echo "MACOS_NOTARY_ISSUER_ID=$APPLE_NOTARY_ISSUER_ID"
} >> "$GITHUB_ENV"
echo 'macos_signed=true' >> "$GITHUB_OUTPUT"
echo 'Developer ID signing and Apple notarization are enabled.'
