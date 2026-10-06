#!/usr/bin/env bash
# Writes the source of the Open CAD Studio that the packages carry, for the
# release page: the program is GPL-3.0, so whoever gets it from a release
# can get its source from the same place.
#
#   archive-open-cad-studio-source.sh OUT_DIR
#
# Writes OUT_DIR/open-cad-studio-source_SHORT.tar.gz with a .sha256, where
# SHORT is the first eight characters of the commit that open-cad-studio.pin
# names, and prints its path. The archive holds every file of that commit,
# as `git archive` writes it, under the folder open-cad-studio-SHORT/; the
# commit is fetched and checked as build-open-cad-studio.sh does it, and the
# archive is checked to hold as many files as the tree of the commit.
#
# Runs with bash 3.2 (macOS) and with the bash of Git for Windows.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[[ $# -eq 1 ]] || fail "usage: archive-open-cad-studio-source.sh OUT_DIR"
mkdir -p "$1"
out_dir=$(cd "$1" && pwd)

read_cad_pin
fetch_cad_source
name=$(cad_source_archive_name)
prefix=open-cad-studio-${cad_commit:0:8}/

# The archive is the tree as it is: export-ignore and export-subst in the
# .gitattributes of the commit must not leave a file out or change one, and
# a submodule would be left out.
if git -C "$cad_source_dir" ls-tree -r "$cad_commit" | awk '$2 == "commit"' | grep -q .; then
    fail "commit $cad_commit has submodules, whose source the archive would not hold"
fi
attributes=$(git -C "$cad_source_dir" rev-parse --git-path info/attributes)
case "$attributes" in
    /* | [A-Za-z]:*) ;;
    *) attributes=$cad_source_dir/$attributes ;;
esac
mkdir -p "$(dirname "$attributes")"
printf '* -export-ignore -export-subst\n' > "$attributes"

rm -f "$out_dir/$name" "$out_dir/$name.sha256"
git -C "$cad_source_dir" archive --format=tar.gz --prefix="$prefix" -o "$out_dir/$name" "$cad_commit"

files=$(git -C "$cad_source_dir" ls-tree -r --name-only "$cad_commit" | wc -l | tr -d ' ')
listing=$(tar -tzf "$out_dir/$name")
archived=$(grep -cv '/$' <<< "$listing" || true)
[[ "$archived" -eq "$files" ]] \
    || fail "$name holds $archived files, but commit $cad_commit has $files"
grep -qxF "${prefix}LICENSE" <<< "$listing" || fail "$name holds no ${prefix}LICENSE"

write_sha256 "$out_dir/$name"
echo "$out_dir/$name"
