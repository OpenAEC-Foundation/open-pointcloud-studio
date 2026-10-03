#!/usr/bin/env bash
# Prints the version of the application in the forms the packages need, as
# name=value lines that a workflow appends to its step output.
#
#   version.sh [EXPECTED]
#
# The only source is `[workspace.package] version` in native/Cargo.toml.
# EXPECTED is a tag (v1.2.3) or a number (1.2.3); when it is given and differs
# from that version the script fails, so a tag can never publish a binary that
# calls itself something else.
#
#   number      1.2.3 or 1.2.3-rc1   what the binary prints, and the file names
#   tag         v1.2.3
#   numeric     1.2.3                without a pre-release suffix, for version
#                                    fields that take numbers only
#   deb         1.2.3 or 1.2.3~rc1   sorts a pre-release before its release
#   prerelease  true or false
#   date        YYYY-MM-DD (UTC)     the day of this build
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

expected=${1:-}

number=$(awk '
    /^\[/ { section = $0 }
    section ~ /^\[workspace\.package\]/ && $1 == "version" {
        gsub(/["\r]/, "", $3)
        print $3
        exit
    }
' "$native_dir/Cargo.toml")
[[ -n "$number" ]] || fail "no [workspace.package] version in native/Cargo.toml"
[[ "$number" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]] \
    || fail "version '$number' is not NUMBER.NUMBER.NUMBER with an optional -suffix"

# The build uses --locked, so a lock file that was not refreshed after the
# version changed would only fail much later, on every runner.
locked=$(awk '
    { gsub(/\r/, "") }
    $0 == "name = \"open-pointcloud-studio-native\"" { getline; gsub(/["\r]/, "", $3); print $3; exit }
' "$native_dir/Cargo.lock")
[[ "$locked" == "$number" ]] \
    || fail "native/Cargo.lock has version '$locked' for the application, native/Cargo.toml has '$number'; run cargo check in native/"

if [[ -n "$expected" && "${expected#v}" != "$number" ]]; then
    fail "asked for ${expected}, but native/Cargo.toml has version $number"
fi

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
