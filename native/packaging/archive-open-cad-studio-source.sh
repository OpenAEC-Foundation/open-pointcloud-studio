#!/usr/bin/env bash
# Writes the source of the Open CAD Studio that the packages carry, for the
# release page: the program is GPL-3.0 and links crates under the MPL-2.0 and
# the LGPL, so whoever gets it from a release can get the source of all of it
# from the same place.
#
#   archive-open-cad-studio-source.sh OUT_DIR
#
# Writes two files to OUT_DIR, each with a .sha256, and prints their paths.
# SHORT is the first eight characters of the commit that open-cad-studio.pin
# names.
#
#   open-cad-studio-source_SHORT.tar.gz  every file of that commit, as
#       `git archive` writes it, under the folder open-cad-studio-SHORT/. The
#       commit is fetched and checked as build-open-cad-studio.sh does it, and
#       the archive is checked to hold as many files as the tree of the
#       commit.
#   open-cad-studio-vendor_SHORT.tar.gz  the crates that the Cargo.lock of
#       that commit takes from git repositories, as
#       `cargo vendor --locked --versioned-dirs` writes them, under
#       open-cad-studio-SHORT/vendor/, with vendor/config.toml, which puts
#       them in place of those repositories. The crates from crates.io are
#       not in it: crates.io keeps the source of every version it published.
#
# The crates are vendored into native/target/open-cad-studio/vendor, and a
# folder there that names the same commit is used again without fetching
# from their repositories; the Packages workflow keeps it in its cache.
#
# Before anything is written, the notices are held to the Cargo.lock of the
# commit: OpenCADStudio-NOTICE.txt.in has to name every git repository with
# its URL and commit, and every crate from one as NAME VERSION (LICENCE);
# every crate under a copyleft licence without a permissive alternative, or
# without a licence, has to be named there as well, and in NOTICE.txt. Last,
# both archives are unpacked together and Cargo has to resolve the source
# with the vendored crates in place of the git repositories.
#
# Needs git, tar, gzip and cargo. Runs with bash 3.2 (macOS) and with the bash
# of Git for Windows.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

[[ $# -eq 1 ]] || fail "usage: archive-open-cad-studio-source.sh OUT_DIR"
mkdir -p "$1"
out_dir=$(cd "$1" && pwd)

read_cad_pin
fetch_cad_source
check_cad_source
name=$(cad_source_archive_name)
vendor_name=$(cad_vendor_archive_name)
prefix=open-cad-studio-${cad_commit:0:8}/
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# ---- The crates from git repositories ---------------------------------------

# NAME-VERSION of every package that Cargo.lock takes from a git repository,
# as `cargo vendor --versioned-dirs` names its folder, and the sources.
lock=$(tr -d '\r' < "$cad_source_dir/Cargo.lock")
git_packages=$(awk '
    /^\[\[package\]\]/ { if (source ~ /^git\+/) print name "-" version; name = version = source = "" }
    /^name = / { name = $3 }
    /^version = / { version = $3 }
    /^source = / { source = $3 }
    END { if (source ~ /^git\+/) print name "-" version }
' <<< "${lock//\"/}" | LC_ALL=C sort -u)
git_sources=$(sed -n 's/^source = "\(git+[^"]*\)"$/\1/p' <<< "$lock" | LC_ALL=C sort -u)

if (check_cad_vendor "$cad_vendor_dir") 2>/dev/null; then
    echo "using the crates of commit $cad_commit vendored before in $cad_vendor_dir" >&2
else
    echo "vendoring the crates of commit $cad_commit into $cad_vendor_dir" >&2
    # cargo vendor writes every crate; only those from git repositories stay.
    all=$cad_target_dir/vendor-all
    rm -rf "$all" "$cad_vendor_dir.new"
    if ! (cd "$cad_source_dir" && cargo vendor --locked --versioned-dirs "$all") \
        > "$work/vendor.toml" 2> "$work/vendor.log"; then
        tail -n 20 "$work/vendor.log" >&2
        fail "cargo vendor failed on commit $cad_commit"
    fi
    mkdir -p "$cad_vendor_dir.new"
    while IFS= read -r package; do
        [[ -n "$package" ]] || continue
        [[ -f "$all/$package/Cargo.toml" ]] || fail "cargo vendor wrote no $package"
        mv "$all/$package" "$cad_vendor_dir.new/"
    done <<< "$git_packages"
    # What cargo vendor prints for the configuration, without the
    # replacement of crates.io and with the folder where it lies.
    {
        cad_vendor_header
        tr -d '\r' < "$work/vendor.toml" | awk '
            /^\[source\./ { started = 1; skip = ($0 == "[source.crates-io]") }
            !started || skip { next }
            /^directory = / { print "directory = \"vendor\""; next }
            { print }
        '
    } > "$cad_vendor_dir.new/config.toml"
    rm -rf "$all" "$cad_vendor_dir"
    mv "$cad_vendor_dir.new" "$cad_vendor_dir"
fi
held=$(ls -1 "$cad_vendor_dir" | grep -vx config.toml | LC_ALL=C sort || true)
[[ "$held" == "$git_packages" ]] \
    || fail "$cad_vendor_dir holds '$(echo $held)', but Cargo.lock takes '$(echo $git_packages)' from git repositories; remove it, and it is vendored again"
while IFS= read -r source; do
    [[ -n "$source" ]] || continue
    url=${source#git+}
    url=${url%%#*}
    url=${url%%\?*}
    grep -qxF "git = \"$url\"" "$cad_vendor_dir/config.toml" \
        || fail "$cad_vendor_dir/config.toml does not replace the repository $url"
done <<< "$git_sources"

# ---- The notices -------------------------------------------------------------

# Every crate with its licence, as Cargo resolves the workspace for every
# system and feature: KIND NAME VERSION LICENCE, separated by tabs, where KIND
# is git, registry or path (a crate of Open CAD Studio itself).
if ! (cd "$cad_source_dir" && cargo tree --locked --config "$cad_vendor_dir/config.toml" \
    --workspace --all-features --target all --edges normal,build,dev \
    --prefix none --format '{p}|{l}') > "$work/tree.txt" 2> "$work/tree.log"; then
    tail -n 20 "$work/tree.log" >&2
    fail "cargo tree failed on commit $cad_commit"
fi
crates=$(tr -d '\r' < "$work/tree.txt" | sed 's/ (\*)$//' | LC_ALL=C sort -u | awk -F'|' '
    # The blank line between the trees of the crates of the workspace.
    NF < 2 { next }
    {
        spec = $1
        sub(/ \(proc-macro\)$/, "", spec)
        n = split(spec, word, " ")
        version = word[2]
        sub(/^v/, "", version)
        source = ""
        if (n > 2) { source = spec; sub(/^[^(]*\(/, "", source); sub(/\)$/, "", source) }
        if (source == "") kind = "registry"
        else if (source ~ /^[a-z+]+:\/\/.*#[0-9a-f]+$/) kind = "git"
        else kind = "path"
        print kind "\t" word[1] "\t" version "\t" $2
    }
')

# copyleft_only LICENCE succeeds when the licence expression names a copyleft
# licence and offers no permissive alternative to it, or names none at all.
copyleft_only() {
    local licence=$1 copyleft='(GPL|MPL|EPL|CDDL|EUPL|OSL|CPL)'
    [[ -n "$licence" ]] || return 0
    [[ "$licence" =~ $copyleft ]] || return 1
    if [[ "$licence" == *" OR "* || "$licence" == */* ]] && [[ "$licence" != *" AND "* ]]; then
        return 1
    fi
    return 0
}

missing=
missing_notice=
while IFS=$'\t' read -r kind crate version licence; do
    [[ -n "$kind" && "$kind" != path ]] || continue
    if [[ "$kind" == git ]] || copyleft_only "$licence"; then
        line="$crate $version (${licence:-no licence named})"
        grep -qF -- "$line" "$cad_notice_template" || missing="$missing"$'\n'"        $line"
    fi
    if copyleft_only "$licence"; then
        grep -qw -- "$crate" "$notice_file" || missing_notice="$missing_notice $crate"
        # The notice says that these carry their licence text.
        if [[ "$kind" == git ]] \
            && [[ "$(ls -1 "$cad_vendor_dir/$crate-$version" | grep -ciE '^(licen[cs]e|copying)' || true)" -eq 0 ]]; then
            fail "$crate $version from a git repository carries no licence text; the notice of Open CAD Studio has to give it"
        fi
    fi
done <<< "$crates"
pinned_commits=
while IFS= read -r source; do
    [[ -n "$source" ]] || continue
    url=${source#git+}
    url=${url%%#*}
    url=${url%%\?*}
    url=${url%.git}
    pinned_commits="$pinned_commits ${source##*#}"
    grep -qxF "    $url" "$cad_notice_template" || missing="$missing"$'\n'"    $url"
    grep -qxF "    commit ${source##*#}" "$cad_notice_template" || missing="$missing"$'\n'"    commit ${source##*#}"
done <<< "$git_sources"
[[ -z "$missing" ]] \
    || fail "$cad_notice_template does not name, as the Cargo.lock of commit $cad_commit has them:$missing"
[[ -z "$missing_notice" ]] \
    || fail "$notice_file does not name the crates under a copyleft licence that Open CAD Studio links:$missing_notice"
for listed in $(sed -n 's/^    commit \([0-9a-f]*\)$/\1/p' "$cad_notice_template"); do
    [[ " $pinned_commits " == *" $listed "* ]] \
        || fail "$cad_notice_template names commit $listed, which the Cargo.lock of commit $cad_commit does not take a crate from"
done

# ---- The source of the commit ------------------------------------------------

# The archive is the tree as it is: export-ignore and export-subst in the
# .gitattributes of the commit must not leave a file out or change one, and
# a submodule would be left out.
submodules=$(git -C "$cad_source_dir" ls-tree -r "$cad_commit" | awk '$2 == "commit" { print $4 }')
[[ -z "$submodules" ]] \
    || fail "commit $cad_commit has submodules, whose source the archive would not hold: $(echo $submodules)"
attributes=$(git -C "$cad_source_dir" rev-parse --git-path info/attributes)
case "$attributes" in
    /* | [A-Za-z]:*) ;;
    *) attributes=$cad_source_dir/$attributes ;;
esac
mkdir -p "$(dirname "$attributes")"
printf '* -export-ignore -export-subst\n' > "$attributes"

rm -f "$out_dir/$name" "$out_dir/$name.sha256" "$out_dir/$vendor_name" "$out_dir/$vendor_name.sha256"
git -C "$cad_source_dir" archive --format=tar.gz --prefix="$prefix" -o "$out_dir/$name" "$cad_commit"

files=$(git -C "$cad_source_dir" ls-tree -r --name-only "$cad_commit" | wc -l | tr -d ' ')
listing=$(tar -tzf "$out_dir/$name")
archived=$(grep -cv '/$' <<< "$listing" || true)
[[ "$archived" -eq "$files" ]] \
    || fail "$name holds $archived files, but commit $cad_commit has $files"
grep -qxF "${prefix}LICENSE" <<< "$listing" || fail "$name holds no ${prefix}LICENSE"

# ---- The crates, in the folder vendor/ of that source --------------------------

mkdir -p "$work/stage/$prefix"
cp -R "$cad_vendor_dir" "$work/stage/${prefix}vendor"
tar_options=()
# Read whole, not piped into grep -q: tar could die of the closed pipe, which
# set -o pipefail would take for an answer.
tar_version=$(tar --version 2>/dev/null || true)
if [[ "$tar_version" == *"GNU tar"* ]]; then
    # The same file for the same crates: the names in order, no owner, and
    # the date of the commit.
    tar_options=(--sort=name --owner=0 --group=0 --numeric-owner --mtime="$cad_date 00:00:00Z")
fi
(cd "$work/stage" && COPYFILE_DISABLE=1 tar ${tar_options[@]+"${tar_options[@]}"} -cf - "${prefix%/}") \
    | gzip -n -9 > "$out_dir/$vendor_name"
listing=$(tar -tzf "$out_dir/$vendor_name")
grep -qxF "${prefix}vendor/config.toml" <<< "$listing" || fail "$vendor_name holds no ${prefix}vendor/config.toml"
while IFS= read -r package; do
    [[ -n "$package" ]] || continue
    for file in Cargo.toml .cargo-checksum.json; do
        grep -qxF "${prefix}vendor/$package/$file" <<< "$listing" \
            || fail "$vendor_name holds no ${prefix}vendor/$package/$file"
    done
done <<< "$git_packages"

# Both archives unpacked in one folder: Cargo resolves the source with the
# vendored crates, without the git repositories.
mkdir -p "$work/unpacked"
tar -xzf "$out_dir/$name" -C "$work/unpacked"
tar -xzf "$out_dir/$vendor_name" -C "$work/unpacked"
if ! (cd "$work/unpacked/$prefix" && cargo metadata --locked --format-version 1 \
    --config vendor/config.toml) > /dev/null 2> "$work/metadata.log"; then
    tail -n 20 "$work/metadata.log" >&2
    fail "the source in $name with the crates in $vendor_name does not resolve without the git repositories"
fi

write_sha256 "$out_dir/$name"
write_sha256 "$out_dir/$vendor_name"
echo "$out_dir/$name"
echo "$out_dir/$vendor_name"
