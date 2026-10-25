#!/usr/bin/env bash
# Writes the notes of a release to standard output: the section of that
# version in CHANGELOG.md at the top of the repository, followed by
# release-notes-footer.md, which says which file is for which system.
#
#   release-notes.sh NUMBER
#   release-notes.sh --check NUMBER    only test that the section exists
#
# A section starts at a line "## NUMBER" or "## vNUMBER", optionally followed
# by a date, and ends before the next "## " line. The script fails when the
# section is missing or has no "- " item, so a version cannot be released
# without saying what changed.
#
# WINDOWS_SIGNED=true states that the Windows files carry a valid signature;
# anything else makes the notes say that they do not. MACOS_SIGNED=true does
# the same for the macOS app and disk image, and SDK_WINDOWS_SIGNED=true and
# SDK_MACOS_SIGNED=true for the C library of each system.
#
# API_IMAGE_DIGEST is the digest (sha256:...) of the image of the cloud API,
# which a tag run pushes before it writes the notes; empty, as in a draft
# run, the notes say that the image comes with the release.
#
# LINUX_GPG_SIGNED=true states that SHA256SUMS and the AppImages carry
# OpenPGP signatures (sign-linux.sh) by the key LINUX_GPG_FINGERPRINT; the
# notes then say how to check them.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

check_only=false
if [[ "${1:-}" == "--check" ]]; then
    check_only=true
    shift
fi
number=${1:?usage: release-notes.sh [--check] NUMBER}
changelog=$repo_dir/CHANGELOG.md
[[ -f "$changelog" ]] || fail "CHANGELOG.md is missing at the top of the repository"

section=$(awk -v number="$number" '
    { sub(/\r$/, "") }
    /^## / {
        if (inside) exit
        title = $2
        sub(/^v/, "", title)
        if (title == number) { inside = 1; next }
    }
    inside { lines[++count] = $0 }
    END {
        # Without the blank lines around the section.
        first = 1
        while (first <= count && lines[first] ~ /^[ \t]*$/) first++
        last = count
        while (last >= first && lines[last] ~ /^[ \t]*$/) last--
        for (i = first; i <= last; i++) print lines[i]
    }
' "$changelog")
if ! grep -q '^- ' <<< "$section"; then
    fail "CHANGELOG.md has no section '## $number' with at least one '- ' item"
fi
if $check_only; then
    echo "CHANGELOG.md has a section for $number"
    exit 0
fi

if [[ "${WINDOWS_SIGNED:-false}" == "true" ]]; then
    signing="Installer and application carry a valid code signature."
else
    signing="Installer and application are not code-signed in this release, so Windows may warn about an unknown publisher before the first start: choose More info and then Run anyway."
fi
if [[ "${MACOS_SIGNED:-false}" == "true" ]]; then
    macos_signing="The app and disk image are Developer ID signed, notarized by Apple and stapled. Open the app normally after copying it to Applications."
    macos_first_start="This release is signed with a Developer ID Application certificate and notarized by Apple. Open the application normally."
    macos_first_start_nl="Deze release is met een Developer ID Application-certificaat ondertekend en door Apple genotariseerd. Open de app normaal."
else
    macos_signing="The app is signed ad hoc and is not notarized; see the first-start instructions below."
    macos_first_start='The application is not signed with a paid developer certificate and not notarised, so macOS refuses to start it the first time. On macOS 15 and later, use System Settings > Privacy and Security > Open Anyway; on macOS 11 to 14, Control-click the app and choose Open. From Terminal, this also works: `xattr -dr com.apple.quarantine "/Applications/Open Pointcloud Studio.app"`.'
    macos_first_start_nl="Deze app is alleen ad hoc ondertekend en niet genotariseerd. macOS 15 en nieuwer: gebruik Systeeminstellingen > Privacy en beveiliging > Open toch. macOS 11 tot en met 14: Control-klik op de app en kies Open."
fi

if [[ "${SDK_WINDOWS_SIGNED:-false}" == "true" ]]; then
    sdk_windows="The C library for Windows carries a valid code signature"
    sdk_windows_nl="De C-bibliotheek voor Windows heeft een geldige code-handtekening"
else
    sdk_windows="The C library for Windows is not code-signed in this release"
    sdk_windows_nl="De C-bibliotheek voor Windows is in deze release niet ondertekend"
fi
if [[ "${SDK_MACOS_SIGNED:-false}" == "true" ]]; then
    sdk_macos="the one for macOS is signed with a developer certificate and notarized"
    sdk_macos_nl="die voor macOS is met een ontwikkelaarscertificaat ondertekend en genotariseerd"
else
    sdk_macos="the one for macOS is signed ad hoc and not notarized"
    sdk_macos_nl="die voor macOS is alleen ad hoc ondertekend en niet genotariseerd"
fi

api_image=ghcr.io/openaec-foundation/open-pointcloud-studio-api
digest=${API_IMAGE_DIGEST:-}
if [[ -n "$digest" ]]; then
    [[ "$digest" =~ ^sha256:[0-9a-f]{64}$ ]] \
        || fail "API_IMAGE_DIGEST is not a digest sha256:<64 hex digits>: '$digest'"
    verify_image="gh attestation verify oci://$api_image@$digest --repo OpenAEC-Foundation/open-pointcloud-studio"
    image_digest="Its digest is \`$digest\`, and \`$verify_image\` checks its build attestation."
    image_digest_nl="De digest is \`$digest\`, en \`$verify_image\` controleert de build-attestatie."
else
    image_digest="It is published with the release; its digest, and how to check its build attestation, are given here then."
    image_digest_nl="Het image wordt met de release gepubliceerd; de digest, en hoe je de build-attestatie controleert, staan hier dan."
fi

linux_signing=
linux_signing_nl=
if [[ "${LINUX_GPG_SIGNED:-false}" == "true" ]]; then
    fingerprint=$(tr -d ' ' <<< "${LINUX_GPG_FINGERPRINT:-}" | tr 'a-f' 'A-F')
    [[ "$fingerprint" =~ ^[0-9A-F]{40}([0-9A-F]{24})?$ ]] \
        || fail "LINUX_GPG_SIGNED=true needs the fingerprint of the key in LINUX_GPG_FINGERPRINT"
    fetch="gpg --keyserver hkps://keys.openpgp.org --recv-keys $fingerprint"
    linux_signing="\`SHA256SUMS.asc\` and the \`.asc\` beside each AppImage are OpenPGP signatures by the key of the OpenAEC Foundation with the fingerprint \`$fingerprint\`. Fetch the key with \`$fetch\` and check with \`gpg --verify SHA256SUMS.asc SHA256SUMS\` and \`gpg --verify FILE.AppImage.asc FILE.AppImage\`."
    linux_signing_nl="\`SHA256SUMS.asc\` en de \`.asc\` naast elke AppImage zijn OpenPGP-handtekeningen met de sleutel van de OpenAEC Foundation, vingerafdruk \`$fingerprint\`. Haal de sleutel op met \`$fetch\` en controleer met \`gpg --verify SHA256SUMS.asc SHA256SUMS\` en \`gpg --verify BESTAND.AppImage.asc BESTAND.AppImage\`."
fi

footer=$(mktemp)
trap 'rm -f "$footer"' EXIT
read_cad_pin
fill_template "$packaging_dir/release-notes-footer.md" "$footer" \
    "VERSION=$number" "WINDOWS_SIGNING=$signing" \
    "MACOS_SIGNING=$macos_signing" "MACOS_FIRST_START=$macos_first_start" \
    "MACOS_FIRST_START_NL=$macos_first_start_nl" \
    "SDK_SIGNING=$sdk_windows; $sdk_macos." "SDK_SIGNING_NL=$sdk_windows_nl; $sdk_macos_nl." \
    "API_IMAGE=$image_digest" "API_IMAGE_NL=$image_digest_nl" \
    "LINUX_SIGNING=$linux_signing" "LINUX_SIGNING_NL=$linux_signing_nl" \
    "CAD_COMMIT=$cad_commit" "CAD_DATE=$cad_date" "CAD_URL=${cad_url%.git}" \
    "CAD_SOURCE=$(cad_source_archive_name)" "CAD_VENDOR=$(cad_vendor_archive_name)"

echo "$section"
echo
cat "$footer"
