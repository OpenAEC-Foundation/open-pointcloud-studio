#!/usr/bin/env bash
# Removes from the draft of a release every file that a complete release of
# that version does not hold: what expected-assets.sh lists, each with its
# .sha256, stays.
#
#   prune-release-assets.sh TAG NUMBER
#
# A draft that a run takes over from an earlier run keeps the files that run
# uploaded. A file whose name changed in between would be published beside
# its successor: the source archives of Open CAD Studio, for one, are named
# after the pinned commit, so after a new pin the archives of the old commit
# would stay on the release page. A published release is refused.
#
# GITHUB_REPOSITORY names the repository; the command-line client needs a
# GH_TOKEN that may write releases.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[[ $# -eq 2 ]] || fail "usage: prune-release-assets.sh TAG NUMBER"
tag=$1
number=$2

expected=$(bash "$packaging_dir/expected-assets.sh" "$number")
draft=$(gh release view "$tag" --repo "$GITHUB_REPOSITORY" --json isDraft --jq .isDraft)
[[ "$draft" == "true" ]] || fail "release $tag is published; nothing is removed from it"
assets=$(gh release view "$tag" --repo "$GITHUB_REPOSITORY" --json assets --jq '.assets[].name')
while IFS= read -r asset; do
    [[ -n "$asset" ]] || continue
    if ! grep -qxF -- "${asset%.sha256}" <<< "$expected"; then
        echo "removing $asset, which release $tag does not hold"
        gh release delete-asset "$tag" "$asset" --repo "$GITHUB_REPOSITORY" --yes
    fi
done <<< "$assets"
