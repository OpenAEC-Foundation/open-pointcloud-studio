#!/usr/bin/env bash
# Headless test of a built or installed application binary. No window opens
# and nothing is downloaded: the test data is a grid of 1000 coloured points
# that the script writes itself.
#
#   smoke-test.sh BINARY NUMBER
#
# NUMBER is the version the binary has to report (1.2.3). Checked are the
# version in the answer of the MCP server, --version and --help, --list-scans,
# an export chain PLY -> LAZ -> E57 -> XYZ that must keep all 1000 points, and
# an octree index of the LAZ file.
#
# Environment:
#   SMOKE_CLI_FLAGS=0   skip --version and --help, for a binary from before
#                       those flags existed (it would open a window instead)
#   SMOKE_KEEP_DIR=DIR  write the files into DIR and leave them there, for
#                       tests that follow (file types, the window test)
#   SMOKE_RUN_PREFIX    words put in front of every start of the binary, for
#                       example "arch -x86_64" to run the other half of a
#                       universal binary
#
# Runs with bash 3.2 (macOS) and with the bash of Git for Windows.
set -euo pipefail

if [[ $# -ne 2 ]]; then
    echo "usage: smoke-test.sh BINARY NUMBER" >&2
    exit 2
fi
binary=$1
number=$2
points=1000

if [[ "$binary" == */* ]]; then
    [[ -f "$binary" ]] || { echo "smoke-test: $binary does not exist" >&2; exit 2; }
    # An absolute path, so that the test may change folders.
    binary="$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")"
fi

if [[ -n "${SMOKE_KEEP_DIR:-}" ]]; then
    mkdir -p "$SMOKE_KEEP_DIR"
    work=$(cd "$SMOKE_KEEP_DIR" && pwd)
else
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
fi
# A Windows program does not know /tmp; C:/... is understood by both sides.
if command -v cygpath >/dev/null 2>&1; then
    work=$(cygpath -m "$work")
fi
mkdir -p "$work/scans" "$work/out" "$work/cache" "$work/config"

# Keep the index cache and the settings of the test away from those of the
# person or runner that starts it.
export XDG_CACHE_HOME="$work/cache"
export XDG_CONFIG_HOME="$work/config"

# Every start gets a time limit: a binary that opens a window where it should
# print and exit must fail the test, not hang it.
if timeout --version >/dev/null 2>&1; then
    limit() { timeout "$@"; }
else
    limit() { perl -e 'alarm shift; exec @ARGV or die "cannot run $ARGV[0]: $!\n"' "$@"; }
fi
# shellcheck disable=SC2086  # the prefix is a list of words on purpose
run() { limit 120 ${SMOKE_RUN_PREFIX:-} "$binary" "$@"; }

step() { echo "smoke-test: $*"; }
failed() {
    echo "smoke-test: FAILED: $*" >&2
    exit 1
}

# 1. The MCP server answers `initialize` with the version of the binary. This
#    mode reads standard input and ends when it closes, so it cannot hang on a
#    window, and it tells the version of every build.
step "version in the MCP answer"
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"smoke-test","version":"1"}}}' > "$work/initialize.json"
run --mcp < "$work/initialize.json" > "$work/initialize-answer.json" 2> "$work/mcp.log" \
    || { cat "$work/mcp.log" >&2; failed "--mcp ended with an error"; }
reported=$(sed -n 's/.*"serverInfo":{[^}]*"version":"\([^"]*\)".*/\1/p' "$work/initialize-answer.json")
[[ "$reported" == "$number" ]] \
    || failed "the MCP server reports version '$reported', expected '$number'"

# 2. --version and --help print and exit.
if [[ "${SMOKE_CLI_FLAGS:-1}" != "0" ]]; then
    step "--version and --help"
    line=$(run --version | tr -d '\r') || failed "--version did not exit with success"
    [[ "$line" == "open-pointcloud-studio $number" ]] \
        || failed "--version printed '$line', expected 'open-pointcloud-studio $number'"
    run --help > "$work/help.txt" || failed "--help did not exit with success"
    grep -q -- '--export' "$work/help.txt" || failed "--help does not name --export"
    grep -q -- '--mcp' "$work/help.txt" || failed "--help does not name --mcp"
else
    step "--version and --help skipped (SMOKE_CLI_FLAGS=0)"
fi

# 3. The test data: a 10 x 10 x 10 grid, 0.25 apart, each point in its own
#    colour.
awk -v n=10 'BEGIN {
    print "ply"
    print "format ascii 1.0"
    print "element vertex " n * n * n
    print "property float x"
    print "property float y"
    print "property float z"
    print "property uchar red"
    print "property uchar green"
    print "property uchar blue"
    print "end_header"
    for (x = 0; x < n; x++)
        for (y = 0; y < n; y++)
            for (z = 0; z < n; z++)
                printf "%.2f %.2f %.2f %d %d %d\n", x * 0.25, y * 0.25, z * 0.25, 15 + 25 * x, 15 + 25 * y, 15 + 25 * z
}' > "$work/scans/grid.ply"

step "--list-scans"
run --list-scans "$work/scans" > "$work/list.txt" 2> "$work/list.log" \
    || { cat "$work/list.log" >&2; failed "--list-scans ended with an error"; }
[[ $(($(wc -l < "$work/list.txt"))) -eq 1 ]] && grep -q 'grid\.ply' "$work/list.txt" \
    || { cat "$work/list.txt" >&2; failed "--list-scans did not print exactly grid.ply"; }

# 4. Through every writer and reader with a compressed or packed format, and
#    back to text where the points can be counted.
export_step() {
    step "--export $1 -> $2"
    run --export "$work/$1" "$work/$2" || failed "export of $1 to $2 ended with an error"
    [[ -s "$work/$2" ]] || failed "export of $1 wrote no $2"
}
export_step scans/grid.ply out/grid.laz
export_step out/grid.laz out/grid.e57
export_step out/grid.e57 out/back.xyz
lines=$(($(wc -l < "$work/out/back.xyz")))
[[ "$lines" -eq "$points" ]] || failed "back.xyz has $lines lines, expected $points"
# Further formats for the tests that follow; they also cover two more writers.
export_step scans/grid.ply out/grid.las
export_step scans/grid.ply out/grid.pts

step "--index"
run --index "$work/out/grid.laz" > "$work/index.txt" 2> "$work/index.log" \
    || { cat "$work/index.log" >&2; failed "--index ended with an error"; }
grep -q "Index ready: $points points" "$work/index.txt" \
    || { cat "$work/index.txt" >&2; failed "--index did not report $points points"; }

echo "smoke-test: passed for $binary ($number)"
