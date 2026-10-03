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

footer=$(mktemp)
trap 'rm -f "$footer"' EXIT
fill_template "$packaging_dir/release-notes-footer.md" "$footer" \
    "VERSION=$number" "WINDOWS_SIGNING=$signing"

echo "$section"
echo
cat "$footer"
