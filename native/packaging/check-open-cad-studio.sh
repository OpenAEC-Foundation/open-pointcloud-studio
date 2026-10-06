#!/usr/bin/env bash
# Checks that a package brought Open CAD Studio along: that it lies where the
# application looks for it, and that it starts on this machine.
#
#   check-open-cad-studio.sh BINARY
#
# BINARY is the application binary of an installed or unpacked package. Open
# CAD Studio has to lie beside it (Windows, the archives, the macOS bundle) or
# in ../lib/open-pointcloud-studio (the .deb and the unpacked AppImage), print
# its version, and convert a small drawing without a window: a DXF file with
# one line, written again as DXF.
#
# Runs with bash 3.2 (macOS) and with the bash of Git for Windows.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

binary=${1:?usage: check-open-cad-studio.sh BINARY}
[[ -f "$binary" ]] || fail "$binary does not exist"
folder=$(cd "$(dirname "$binary")" && pwd)

program=
for candidate in "$folder/$CAD_BINARY_NAME.exe" "$folder/$CAD_BINARY_NAME" \
    "$folder/../lib/$BINARY_NAME/$CAD_BINARY_NAME"; do
    if [[ -f "$candidate" ]]; then
        program=$candidate
        break
    fi
done
[[ -n "$program" ]] || fail "no $CAD_BINARY_NAME beside $binary or in ../lib/$BINARY_NAME"
[[ -x "$program" ]] || fail "$program is not executable"

# A program that opens a window where it should print and exit must fail the
# check, not hang it.
if timeout --version >/dev/null 2>&1; then
    limit() { timeout "$@"; }
else
    limit() { perl -e 'alarm shift; exec @ARGV or die "cannot run $ARGV[0]: $!\n"' "$@"; }
fi

version=$(limit 60 "$program" --version) || fail "$program --version ended with an error"
# The first line names the program and its version; revision and build follow.
version=${version%%$'\n'*}
[[ "$version" == "$CAD_BINARY_NAME "* ]] || fail "$program --version printed '$version'"
echo "ok    $program reports $version"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
# A Windows program does not know /tmp; C:/... is understood by both sides.
if command -v cygpath >/dev/null 2>&1; then
    work=$(cygpath -m "$work")
fi
# Its settings stay away from those of whoever runs the check.
export HOME="$work" APPDATA="$work/appdata" XDG_CONFIG_HOME="$work/config" XDG_CACHE_HOME="$work/cache"
printf '%s\n' 0 SECTION 2 ENTITIES 0 LINE 8 0 10 0.0 20 0.0 30 0.0 11 1000.0 21 500.0 31 0.0 \
    0 ENDSEC 0 EOF > "$work/line.dxf"
limit 120 "$program" --export "$work/line.dxf" "$work/again.dxf" > "$work/export.log" 2>&1 \
    || { cat "$work/export.log" >&2; fail "$program could not convert a DXF file"; }
grep -q LINE "$work/again.dxf" || fail "$program wrote a DXF file without the line"
echo "ok    $program converts a DXF file without a window"
