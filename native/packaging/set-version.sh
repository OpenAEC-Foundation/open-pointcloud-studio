#!/usr/bin/env bash
# Works out, writes and checks the version of the application. A version is
# YYYY.W.N: the ISO week-year and the ISO week of the release, the week
# without a leading zero, and a sequence number from 0 within that week, such
# as 2026.43.0. A second release in that week is 2026.43.1, the first of the
# week after 2026.44.0. Its tag is the version with a v in front.
#
#   set-version.sh next [--date YYYY-MM-DD] [--offline]
#   set-version.sh set VERSION|next [--date YYYY-MM-DD] [--offline] [--changelog DATE]
#   set-version.sh check [VERSION]
#
# next prints the version that the next release gets. The week is that of
# today in UTC, or that of --date. N is one more than the highest N of that
# week among the local tags, the tags of origin and the releases on GitHub,
# drafts included, because a draft has no tag until it is published; it is 0
# when there is none, and a gap is not filled. --offline counts the local
# tags only. The releases are those of GITHUB_REPOSITORY, or else of the
# GitHub repository of origin, read with the command-line client gh; only a
# token that may write releases sees the drafts.
#
# set writes VERSION, or the version that next prints, as
# [workspace.package] version in native/Cargo.toml and as the version of
# every member of the workspace that takes it (version.workspace = true) in
# native/Cargo.lock. A member with a version of its own, such as the vendored
# ui crate, keeps it. A version that is not above the highest local tag is
# refused. With --changelog DATE the section "## Unreleased" of CHANGELOG.md
# becomes "## VERSION - DATE", with a new empty "## Unreleased" above it.
# Nothing is written unless all of it can be; then it checks as check does.
#
# check writes nothing. It prints the version of each place and fails when
# one is not YYYY.W.N, when they differ, or when VERSION (a tag vYYYY.W.N or a
# number YYYY.W.N) is given and differs from them.
#
# Runs with bash 3.2 (macOS) and with the bash of Git for Windows.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

# YYYY.W.N without leading zeros. N has at most two digits, as the third
# number of the version of a macOS bundle does.
version_pattern='^[1-9][0-9]{3}\.([1-9]|[1-4][0-9]|5[0-3])\.(0|[1-9][0-9]?)$'
version_rule="YYYY.W.N, such as 2026.43.0: the ISO week-year, the ISO week without a leading zero and a sequence number from 0 to 99"

# The last release numbered the old way. native/Cargo.toml holds it until the
# first release numbered YYYY.W.N, and check accepts it as long as no
# version is asked for, so that the checks of every commit pass until then.
last_old_version=0.9.5

cargo_toml=$native_dir/Cargo.toml
cargo_lock=$native_dir/Cargo.lock
changelog=$repo_dir/CHANGELOG.md

usage() {
    echo "usage: set-version.sh next [--date YYYY-MM-DD] [--offline]" >&2
    echo "       set-version.sh set VERSION|next [--date YYYY-MM-DD] [--offline] [--changelog DATE]" >&2
    echo "       set-version.sh check [VERSION]" >&2
    exit 2
}

is_version() {
    [[ "$1" =~ $version_pattern ]]
}

# version_key VERSION prints a number that orders versions YYYY.W.N.
version_key() {
    local year week sequence
    IFS=. read -r year week sequence <<< "$1"
    echo $((year * 10000 + week * 100 + sequence))
}

# check_date DATE fails unless DATE is a day written YYYY-MM-DD.
check_date() {
    [[ "$1" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || fail "'$1' is not a date YYYY-MM-DD"
}

# week_of [DATE] prints the ISO week-year and ISO week of DATE, or of today,
# in UTC, as YYYY.W.
week_of() {
    local answer
    if [[ -z "${1:-}" ]]; then
        answer=$(date -u +%Y-%m-%d.%G.%V)
    else
        check_date "$1"
        # GNU date, as on Linux and in Git for Windows, or else that of BSD
        # and macOS. A day that does not exist, such as 2026-02-30, is
        # refused by the one and moved to another day by the other.
        answer=$(date -u -d "$1" +%Y-%m-%d.%G.%V 2>/dev/null) \
            || answer=$(date -u -j -f %Y-%m-%d "$1" +%Y-%m-%d.%G.%V 2>/dev/null) \
            || fail "'$1' is not a date"
        [[ "${answer%%.*}" == "$1" ]] || fail "'$1' is not a date"
    fi
    answer=${answer#*.}
    echo "${answer%.*}.$((10#${answer#*.}))"
}

# highest_sequence WEEK reads tag names, one per line, and prints the highest
# N of the tags vWEEK.N among them, or -1 when there is none.
highest_sequence() {
    local week=$1 highest=-1 name
    while read -r name; do
        if [[ "$name" == "v$week".* ]] && is_version "${name#v}" && [[ "${name##*.}" -gt "$highest" ]]; then
            highest=${name##*.}
        fi
    done
    echo "$highest"
}

# The GitHub repository of the releases, as OWNER/NAME.
github_repository() {
    local url
    if [[ -n "${GITHUB_REPOSITORY:-}" ]]; then
        echo "$GITHUB_REPOSITORY"
        return
    fi
    url=$(git -C "$repo_dir" remote get-url origin 2>/dev/null) \
        || fail "this checkout has no remote origin; --offline counts the local tags only"
    url=${url%/}
    url=${url%.git}
    case $url in
        *github.com[:/]*/*) echo "${url#*github.com[:/]}" ;;
        *) fail "origin ($url) is not a repository on GitHub; set GITHUB_REPOSITORY=OWNER/NAME, or use --offline" ;;
    esac
}

# next_version DATE OFFLINE prints the version that the next release gets.
next_version() {
    local date=$1 offline=$2 week tags remote repository releases highest
    # This also runs in a command substitution, where bash does not stop at
    # a failing command by itself.
    week=$(week_of "$date") || exit 1
    tags=$(git -C "$repo_dir" tag --list "v$week.*") || exit 1
    if ! $offline; then
        remote=$(git -C "$repo_dir" ls-remote --tags origin "refs/tags/v$week.*") \
            || fail "the tags of origin cannot be read; --offline counts the local tags only"
        command -v gh > /dev/null 2>&1 \
            || fail "gh, the command-line client of GitHub, is needed to see the draft releases; --offline counts the local tags only"
        repository=$(github_repository) || exit 1
        releases=$(gh api "repos/$repository/releases" --paginate --jq '.[].tag_name') \
            || fail "the releases of $repository cannot be listed with gh; --offline counts the local tags only"
        # ls-remote prints HASH<tab>refs/tags/NAME, and NAME^{} once more for
        # the commit of an annotated tag.
        tags="$tags
$(sed -n 's|^[0-9a-f]*[[:space:]]*refs/tags/\([^^]*\).*$|\1|p' <<< "$remote")
$releases"
    fi
    highest=$(highest_sequence "$week" <<< "$tags")
    [[ "$highest" -lt 99 ]] || fail "week $week has a release $week.99 already, and N has at most two digits"
    echo "$week.$((highest + 1))"
}

# The package names of the members of the workspace that take its version,
# one per line.
numbered_members() {
    local folder
    awk '
        { sub(/\r$/, "") }
        /^\[/ && !inside { section = $0 }
        section == "[workspace]" && /^members[ \t]*=/ { inside = 1 }
        inside {
            line = $0
            sub(/#.*/, "", line)
            while (match(line, /"[^"]*"/)) {
                print substr(line, RSTART + 1, RLENGTH - 2)
                line = substr(line, RSTART + RLENGTH)
            }
            if ($0 ~ /\]/) inside = 0
        }
    ' "$cargo_toml" | while read -r folder; do
        [[ -f "$native_dir/$folder/Cargo.toml" ]] \
            || fail "the member $folder of the workspace has no native/$folder/Cargo.toml"
        awk '
            { sub(/\r$/, "") }
            /^\[/ { section = $0 }
            section == "[package]" && /^name[ \t]*=/ {
                name = $0
                sub(/^name[ \t]*=[ \t]*"/, "", name)
                sub(/".*/, "", name)
            }
            section == "[package]" && (/^version\.workspace[ \t]*=[ \t]*true/ || /^version[ \t]*=[ \t]*\{[ \t]*workspace[ \t]*=[ \t]*true/) {
                numbered = 1
            }
            END { if (numbered) print name }
        ' "$native_dir/$folder/Cargo.toml"
    done
}

# lock_versions NAMES prints "NAME VERSION" for each package of the workspace
# in native/Cargo.lock whose name is one of NAMES (separated by spaces); the
# packages of the workspace are those without a source.
#
# lock_versions NAMES VERSION prints native/Cargo.lock with VERSION for each
# of them instead, and the rest as it is.
lock_versions() {
    awk -v BINMODE=3 -v names=" $1 " -v version="${2:-}" '
        function flush(    i) {
            if (name != "" && source == 0 && index(names, " " name " ")) {
                if (version == "") print name, found
                else sub(/"[^"]*"/, "\"" version "\"", lines[version_line])
            }
            if (version != "") for (i = 1; i <= count; i++) print lines[i]
            count = 0
            name = found = ""
            source = version_line = 0
        }
        /^\[/ { flush() }
        { lines[++count] = $0; line = $0; sub(/\r$/, "", line) }
        line ~ /^name = "/ { name = line; gsub(/^name = "|"$/, "", name) }
        line ~ /^version = "/ { found = line; gsub(/^version = "|"$/, "", found); version_line = count }
        line ~ /^source = / { source = 1 }
        END { flush() }
    ' "$cargo_lock"
}

# check_versions [VERSION] checks native/Cargo.toml and native/Cargo.lock,
# and that they hold VERSION when it is given.
check_versions() {
    local requested=${1:-} number members name locked
    if [[ -n "$requested" ]]; then
        is_version "${requested#v}" || fail "'$requested' is not $version_rule"
    fi
    number=$(workspace_version)
    [[ -n "$number" ]] || fail "no [workspace.package] version in native/Cargo.toml"
    echo "native/Cargo.toml: $number"
    if [[ -n "$requested" && "${requested#v}" != "$number" ]]; then
        fail "asked for $requested, but native/Cargo.toml has version $number; set-version.sh set ${requested#v} --changelog YYYY-MM-DD writes it"
    fi
    if ! is_version "$number"; then
        [[ -z "$requested" && "$number" == "$last_old_version" ]] \
            || fail "version '$number' in native/Cargo.toml is not $version_rule"
        echo "$number is the last release numbered the old way; the next one is numbered YYYY.W.N"
    fi
    members=$(numbered_members)
    [[ -n "$members" ]] || fail "no member of the workspace takes the version of native/Cargo.toml"
    for name in $members; do
        locked=$(lock_versions "$name" | awk '{ print $2 }')
        echo "native/Cargo.lock $name: ${locked:-missing}"
        # The builds use --locked, so a lock file that does not follow
        # native/Cargo.toml would only fail much later, on every machine.
        [[ "$locked" == "$number" ]] \
            || fail "native/Cargo.lock has version '$locked' for $name, native/Cargo.toml has '$number'; cargo check in native/ writes it"
    done
}

# set_version VERSION [DATE] writes VERSION, and the changelog section of
# DATE when it is given.
set_version() {
    local version=${1#v} date=${2:-} tags tag highest= members status=0
    is_version "$version" || fail "'$1' is not $version_rule"
    tags=$(git -C "$repo_dir" tag --list 'v*')
    for tag in $tags; do
        if is_version "${tag#v}" && { [[ -z "$highest" ]] || [[ $(version_key "${tag#v}") -gt $(version_key "$highest") ]]; }; then
            highest=${tag#v}
        fi
    done
    if [[ -n "$highest" ]] && [[ $(version_key "$version") -le $(version_key "$highest") ]]; then
        fail "$version is not above v$highest, the highest tag"
    fi
    members=$(numbered_members)
    [[ -n "$members" ]] || fail "no member of the workspace takes the version of native/Cargo.toml"

    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    awk -v BINMODE=3 -v version="$version" '
        { line = $0; sub(/\r$/, "", line) }
        line ~ /^\[/ { section = line }
        section == "[workspace.package]" && line ~ /^version[ \t]*=/ && !done {
            sub(/"[^"]*"/, "\"" version "\"")
            done = 1
        }
        { print }
        END { exit done ? 0 : 1 }
    ' "$cargo_toml" > "$work/Cargo.toml" || fail "no [workspace.package] version in native/Cargo.toml"
    lock_versions "$(echo $members)" "$version" > "$work/Cargo.lock"
    if [[ -n "$date" ]]; then
        check_date "$date"
        [[ -f "$changelog" ]] || fail "CHANGELOG.md is missing at the top of the repository"
        # The new heading keeps the line ends of the file.
        awk -v BINMODE=3 -v heading="## $version - $date" -v version="$version" '
            BEGIN { number = version; gsub(/\./, "\\.", number) }
            {
                line = $0
                end = sub(/\r$/, "", line) ? "\r" : ""
            }
            line ~ ("^## v?" number "([ \t]|$)") { taken = 1 }
            inside && line ~ /^## / { inside = 0 }
            inside && line ~ /^- / { items = 1 }
            line ~ /^## Unreleased[ \t]*$/ && !found {
                found = inside = 1
                print line end
                print end
                print heading end
                next
            }
            { print }
            END { exit taken ? 4 : !found ? 2 : !items ? 3 : 0 }
        ' "$changelog" > "$work/CHANGELOG.md" || status=$?
        case $status in
            0) ;;
            2) fail "CHANGELOG.md has no section '## Unreleased'" ;;
            3) fail "the section '## Unreleased' of CHANGELOG.md has no '- ' item, so the release would say nothing" ;;
            *) fail "CHANGELOG.md has a section for $version already" ;;
        esac
    fi

    cat "$work/Cargo.toml" > "$cargo_toml"
    cat "$work/Cargo.lock" > "$cargo_lock"
    if [[ -n "$date" ]]; then
        cat "$work/CHANGELOG.md" > "$changelog"
    fi
    check_versions "$version"
}

[[ $# -gt 0 ]] || usage
subcommand=$1
shift
date=
offline=false
changelog_date=
argument=
while [[ $# -gt 0 ]]; do
    case $1 in
        --date)
            [[ $# -ge 2 ]] || usage
            date=$2
            shift 2
            ;;
        --offline)
            offline=true
            shift
            ;;
        --changelog)
            [[ $# -ge 2 && -n "$2" ]] || usage
            changelog_date=$2
            shift 2
            ;;
        -*) usage ;;
        *)
            [[ -z "$argument" ]] || usage
            argument=$1
            shift
            ;;
    esac
done

case $subcommand in
    next)
        [[ -z "$argument" && -z "$changelog_date" ]] || usage
        next_version "$date" "$offline"
        ;;
    set)
        [[ -n "$argument" ]] || usage
        if [[ "$argument" == next ]]; then
            version=$(next_version "$date" "$offline")
        else
            [[ -z "$date" ]] && ! $offline || usage
            version=$argument
        fi
        set_version "$version" "$changelog_date"
        ;;
    check)
        [[ -z "$date" && -z "$changelog_date" ]] && ! $offline || usage
        check_versions "$argument"
        ;;
    *) usage ;;
esac
