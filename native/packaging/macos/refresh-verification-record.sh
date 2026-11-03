#!/usr/bin/env bash
# Replace only the Intel Mac section with a successful new check; preserve
# passed platform records and point to both runs. Never alter package bytes.
set -euo pipefail
[[ $# == 4 ]] || { echo 'usage: refresh-verification-record.sh TAG OLD NEW OUTPUT' >&2; exit 2; }
tag=$1; old=$2; new=$3; output=$4
[[ "$tag" =~ ^v[1-9][0-9]{3}\.([1-9]|[1-4][0-9]|5[0-3])\.(0|[1-9][0-9]?)$ ]] || exit 2
[[ $(head -n 1 "$old") == "# Verification of Open Pointcloud Studio $tag" ]] || exit 2
[[ $(head -n 1 "$new") == '## macos-x86_64' ]] || exit 2
: "${SOURCE_COMMIT:?}" "${ORIGINAL_RUN:?}" "${CORRECTION_RUN:?}" "${CORRECTION_SHA:?}"
[[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ && "$ORIGINAL_RUN" =~ ^[0-9]+$ ]] || exit 2
[[ "$CORRECTION_SHA" =~ ^[0-9a-f]{40}$ ]] || exit 2
[[ $(sed -n 's/^- Commit: //p' "$old" | head -n 1) == "$SOURCE_COMMIT" ]] || exit 2
repo=${GITHUB_REPOSITORY:-OpenAEC-Foundation/open-pointcloud-studio}
server=${GITHUB_SERVER_URL:-https://github.com}
[[ "$CORRECTION_RUN" == "$server/$repo/actions/runs/"* ]] || exit 2
[[ "${CORRECTION_RUN##*/}" =~ ^[0-9]+$ ]] || exit 2
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
awk -v replacement="$new" '
    /^## windows$/ { keep=1 }
    /^## macos-x86_64$/ {
        while ((getline line < replacement) > 0) print line
        close(replacement); keep=0; next
    }
    /^## / && $0 != "## macos-x86_64" { if (seen) keep=1 }
    /^## windows$/ { seen=1 }
    keep { print }
' "$old" > "$work/body"
for part in windows macos-arm64 macos-x86_64 linux; do
    [[ $(grep -cx "## $part" "$work/body") == 1 ]] || exit 1
done
if grep -qF '**Failed:**' "$work/body" || grep -qF 'No record:' "$work/body"; then
    echo 'refusing to replace a record with failed or missing platform checks' >&2
    exit 1
fi
{
    printf '# Verification of Open Pointcloud Studio %s\n\n' "$tag"
    echo 'The published package bytes are unchanged. This record combines the successful original platform checks with a fresh Intel Mac check.'
    echo
    printf -- '- Original release run: %s/%s/actions/runs/%s\n' "$server" "$repo" "$ORIGINAL_RUN"
    printf -- '- Run: %s\n- Commit: %s\n- Verification code: %s\n- Result: success\n' "$CORRECTION_RUN" "$SOURCE_COMMIT" "$CORRECTION_SHA"
    printf -- '- Written: %s\n\n' "$(date -u +%Y-%m-%dT%H:%MZ)"
    echo 'The original macOS 15 Intel check hit the CloudKit network failure tracked in https://github.com/actions/runner-images/issues/14924. Gatekeeper accepted the files. The replacement check uses macOS 26 Intel and still requires valid signatures, notarization, a valid ticket, the application version and the native C SDK.'
    echo
    cat "$work/body"
} > "$output"
