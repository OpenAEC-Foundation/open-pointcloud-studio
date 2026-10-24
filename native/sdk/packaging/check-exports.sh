#!/usr/bin/env bash
# Checks that a built C library exports exactly the ops_ functions that its
# header declares, no more and no fewer, and prints them.
#
#   check-exports.sh LIBRARY
#   check-exports.sh --declared
#
# LIBRARY is libops_pointcloud.so (read with nm -D), libops_pointcloud.dylib
# (nm -gU, both halves of a universal library) or ops_pointcloud.dll
# (dumpbin -exports, or objdump -p where dumpbin is not on the PATH).
# --declared prints the functions of the header only.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/../../packaging/common.sh"

header=$native_dir/sdk/include/ops_pointcloud.h

# The functions the header declares: every name after OPS_API.
declared() {
    sed -n 's/^OPS_API .*[ *]\(ops_[a-z0-9_]*\)(.*/\1/p' "$header" | sort -u
}

[[ $# -eq 1 ]] || fail "usage: check-exports.sh LIBRARY | --declared"
if [[ "$1" == --declared ]]; then
    declared
    exit 0
fi
library=$1
[[ -f "$library" ]] || fail "$library does not exist"

case "$library" in
    *.so | *.so.*)
        exported=$(nm -D --defined-only "$library" | awk '$2 == "T" && $3 ~ /^ops_/ { print $3 }')
        ;;
    *.dylib)
        exported=$(nm -gU "$library" | awk '$2 == "T" && $3 ~ /^_ops_/ { print substr($3, 2) }')
        ;;
    *.dll)
        if command -v dumpbin > /dev/null 2>&1; then
            exported=$(dumpbin -nologo -exports "$library" | tr -d '\r' | awk '$4 ~ /^ops_/ { print $4 }')
        else
            exported=$(objdump -p "$library" | tr -d '\r' | awk '$1 ~ /^\[/ && $NF ~ /^ops_/ { print $NF }')
        fi
        ;;
    *) fail "$library is not a .so, .dylib or .dll" ;;
esac
exported=$(printf '%s\n' "$exported" | sed '/^$/d' | sort -u)

echo "exported ops_ functions of $(basename "$library"):"
printf '%s\n' "$exported" | sed 's/^/  /'
if [[ "$exported" != "$(declared)" ]]; then
    diff <(declared) <(printf '%s\n' "$exported") >&2 || true
    fail "$library does not export what ops_pointcloud.h declares (< header, > library)"
fi
echo "$(printf '%s\n' "$exported" | wc -l | tr -d ' ') functions, as the header declares"
