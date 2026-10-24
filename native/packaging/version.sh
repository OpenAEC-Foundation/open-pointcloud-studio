#!/usr/bin/env bash
# Prints the version of the application in the forms the packages need, as
# name=value lines that a workflow appends to its step output.
#
#   version.sh [EXPECTED]
#
# The only source is `[workspace.package] version` in native/Cargo.toml,
# which set-version.sh writes: YYYY.W.N, such as 2026.43.0. EXPECTED is a tag
# (v2026.43.0) or a number (2026.43.0); when it is given and differs from that
# version the script fails, so a tag can never publish a binary that calls
# itself something else. The checks are those of set-version.sh check, which
# also refuses a native/Cargo.lock in which a member of the workspace has
# another version.
#
#   number      2026.43.0            what the binary prints, and the file names
#   tag         v2026.43.0
#   numeric     1.2.3                without a pre-release suffix, for version
#                                    fields that take numbers only
#   deb         1.2.3 or 1.2.3~rc1   sorts a pre-release before its release
#   prerelease  true or false
#   date        YYYY-MM-DD (UTC)     the day of this build
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

expected=${1:-}

# What is checked goes to the log, not to the step output.
bash "$packaging_dir/set-version.sh" check ${expected:+"$expected"} >&2
number=$(workspace_version)

numeric=${number%%-*}
if [[ "$number" == "$numeric" ]]; then
    deb=$number
    prerelease=false
else
    deb="$numeric~${number#*-}"
    prerelease=true
fi

echo "number=$number"
echo "tag=v$number"
echo "numeric=$numeric"
echo "deb=$deb"
echo "prerelease=$prerelease"
echo "date=$(date -u +%Y-%m-%d)"
