#!/usr/bin/env bash
# Creates the draft of a release with its notes, or takes over the draft that
# an earlier run left for the same tag.
#
#   draft-release.sh TAG NOTES_FILE PRERELEASE
#
# PRERELEASE is true or false. GITHUB_REPOSITORY and GITHUB_SHA name the
# repository and the commit the packages were built from; the command-line
# client needs a GH_TOKEN that may write releases.
#
# Publishing a draft creates its tag at the commit the draft points to. A
# draft taken over from an earlier run points to the commit of that run, so
# it is moved as well: otherwise the tag and the source archives of the
# release would be older than the packages beside them.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

if [[ $# -ne 3 ]]; then
    echo "usage: draft-release.sh TAG NOTES_FILE PRERELEASE" >&2
    exit 2
fi
tag=$1
notes=$2
prerelease=$3

options=(--repo "$GITHUB_REPOSITORY" --target "$GITHUB_SHA"
    --title "$APP_NAME $tag" --notes-file "$notes")
if [[ "$prerelease" == "true" ]]; then
    options+=(--prerelease)
fi

# A draft cannot be looked up by its tag through the API; the command-line
# client finds drafts by name.
if gh release view "$tag" --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1; then
    gh release edit "$tag" "${options[@]}"
else
    gh release create "$tag" --draft "${options[@]}"
fi
