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
#   number  2026.43.0         what the binary prints, the file names and
#                             the version fields of every package, which
#                             take three numbers
#   tag     v2026.43.0
#   date    YYYY-MM-DD (UTC)  the day of this build
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

expected=${1:-}

# What is checked goes to the log, not to the step output.
bash "$packaging_dir/set-version.sh" check ${expected:+"$expected"} >&2
number=$(workspace_version)

echo "number=$number"
echo "tag=v$number"
echo "date=$(date -u +%Y-%m-%d)"
