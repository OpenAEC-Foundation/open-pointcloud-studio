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
# anything else makes the notes say that they do not.
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

footer=$(mktemp)
trap 'rm -f "$footer"' EXIT
read_cad_pin
fill_template "$packaging_dir/release-notes-footer.md" "$footer" \
    "VERSION=$number" "WINDOWS_SIGNING=$signing" \
    "MACOS_SIGNING=$macos_signing" "MACOS_FIRST_START=$macos_first_start" \
    "MACOS_FIRST_START_NL=$macos_first_start_nl" \
    "CAD_COMMIT=$cad_commit" "CAD_DATE=$cad_date" "CAD_URL=${cad_url%.git}" \
    "CAD_SOURCE=$(cad_source_archive_name)" "CAD_VENDOR=$(cad_vendor_archive_name)"

echo "$section"
echo
cat "$footer"
