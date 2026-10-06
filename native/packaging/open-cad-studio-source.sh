# Sourced by common.sh: reading the pin file of Open CAD Studio, and
# fetching and checking the source of the commit it names. What decides the
# source that the program is built from is here and in
# build-open-cad-studio.sh; the Packages workflow keeps a built program in its
# cache under the hash of both files, so that a change elsewhere in the
# package scripts does not build it again.
#
# Sourced, not run. Written for the bash 3.2 of macOS as well.

# open-cad-studio.pin names the one commit of the repository of Open CAD
# Studio that is built, and fetch_cad_source fetches it into a folder of the
# target folder. The OCS_ variables move these places for the tests of the
# scripts.
cad_pin_file=${OCS_PIN_FILE:-$packaging_dir/open-cad-studio.pin}
cad_target_dir=${OCS_TARGET_DIR:-$native_dir/target/open-cad-studio}
cad_source_dir=${OCS_SOURCE_DIR:-$cad_target_dir/source}

# read_cad_pin sets cad_url, cad_commit, cad_tree and cad_date from the pin
# file, which holds four lines KEY=VALUE: the https URL of the repository,
# the full hash of the commit, the full hash of the tree of that commit and
# the date of the commit (YYYY-MM-DD). Anything else in it is refused.
read_cad_pin() {
    local line key value seen=''
    cad_url='' cad_commit='' cad_tree='' cad_date=''
    [[ -f "$cad_pin_file" ]] || fail "$cad_pin_file does not exist"
    while IFS= read -r line || [[ -n "$line" ]]; do
        line=${line%$'\r'}
        [[ "$line" == *=* ]] || fail "$cad_pin_file: '$line' is not KEY=VALUE"
        key=${line%%=*}
        value=${line#*=}
        case " $seen " in
            *" $key "*) fail "$cad_pin_file names $key twice" ;;
        esac
        seen="$seen $key"
        case "$key" in
            url) cad_url=$value ;;
            commit) cad_commit=$value ;;
            tree) cad_tree=$value ;;
            date) cad_date=$value ;;
            *) fail "$cad_pin_file: unknown key '$key'" ;;
        esac
    done < "$cad_pin_file"
    [[ "$cad_url" =~ ^https://[A-Za-z0-9./_~%+-]+$ ]] || fail "$cad_pin_file: url '$cad_url' is not an https URL"
    [[ "$cad_commit" =~ ^[0-9a-f]{40}$ ]] || fail "$cad_pin_file: commit '$cad_commit' is not a full commit hash"
    [[ "$cad_tree" =~ ^[0-9a-f]{40}$ ]] || fail "$cad_pin_file: tree '$cad_tree' is not a full tree hash"
    [[ "$cad_date" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || fail "$cad_pin_file: date '$cad_date' is not YYYY-MM-DD"
}

# fetch_cad_source makes cad_source_dir a git checkout of the pinned commit
# and fails unless it holds exactly the pinned tree. read_cad_pin first.
#
# The commit is fetched alone (git fetch --depth 1 URL COMMIT) from the pinned
# URL, or from OCS_FETCH_FROM, a local clone or a mirror, when that is set,
# such as the folder that build-open-cad-studio.sh --export writes; a
# checkout that is already at the commit is used without fetching. Files
# that differ from the commit are restored and files it does not have are
# removed, so a build never takes a change made by hand. A checkout that has
# the commit is not checked out again: the build script of the program
# watches .git/HEAD, and Cargo would compile its largest crate again.
fetch_cad_source() {
    local dir=$cad_source_dir from=${OCS_FETCH_FROM:-$cad_url} head tree date changed
    # git runs in the checkout, so a local folder is named by its full path.
    if [[ -d "$from" ]]; then
        from=$(cd "$from" && pwd)
    fi
    if [[ ! -e "$dir/.git" ]]; then
        if [[ -d "$dir" && -n "$(ls -A "$dir")" ]]; then
            fail "$dir is not empty and no git repository; remove it, and the source is fetched again"
        fi
        mkdir -p "$dir"
        git -C "$dir" init -q
        # The files as the commit has them, also on Windows.
        git -C "$dir" config core.autocrlf false
    fi
    head=$(git -C "$dir" rev-parse -q --verify 'HEAD^{commit}' 2>/dev/null || true)
    if [[ "$head" != "$cad_commit" ]]; then
        if ! git -C "$dir" cat-file -e "$cad_commit^{commit}" 2>/dev/null; then
            echo "fetching commit $cad_commit of $from" >&2
            git -C "$dir" fetch -q --depth 1 "$from" "$cad_commit" \
                || fail "commit $cad_commit could not be fetched from $from"
        fi
        git -C "$dir" -c advice.detachedHead=false checkout -q --force --detach "$cad_commit"
    fi
    changed=$(git -C "$dir" status --porcelain)
    if [[ -n "$changed" ]]; then
        echo "restoring $dir to commit $cad_commit; it had:" >&2
        sed 's/^/    /' <<< "$changed" >&2
        git -C "$dir" checkout -q --force -- .
        git -C "$dir" clean -q -f -d
    fi

    head=$(git -C "$dir" rev-parse HEAD)
    tree=$(git -C "$dir" rev-parse 'HEAD^{tree}')
    date=$(git -C "$dir" log -1 --format=%cs HEAD)
    [[ "$head" == "$cad_commit" ]] || fail "$dir is at $head, not at the pinned commit $cad_commit"
    [[ "$tree" == "$cad_tree" ]] \
        || fail "commit $cad_commit from $from has the tree $tree, but $cad_pin_file names the tree $cad_tree: this is not the pinned source, and it is not used"
    [[ "$date" == "$cad_date" ]] \
        || fail "commit $cad_commit is of $date, but $cad_pin_file names $cad_date"
    [[ -z "$(git -C "$dir" status --porcelain)" ]] || fail "$dir still differs from commit $cad_commit"
    echo "$dir holds commit $cad_commit of $cad_date, tree $tree" >&2
}

# check_cad_source checks the fetched source before it is built: that every
# git dependency in its Cargo.lock names the full hash of one commit, which
# `cargo build --locked` then builds, and that its licence text is the
# GPL-3.0 text that copy_cad_licence puts in the packages for it.
check_cad_source() {
    local dir=$cad_source_dir sources source hash rev
    [[ -f "$dir/Cargo.lock" ]] || fail "$dir has no Cargo.lock"
    sources=$(tr -d '\r' < "$dir/Cargo.lock" | sed -n 's/^source = "\(git+[^"]*\)"$/\1/p' | sort -u)
    while IFS= read -r source; do
        [[ -n "$source" ]] || continue
        hash=${source##*#}
        [[ "$source" == *#* && "$hash" =~ ^[0-9a-f]{40}$ ]] \
            || fail "Cargo.lock of commit $cad_commit names no commit for $source"
        # A rev written as a shortened hash has to be the start of the commit
        # it resolves to.
        if [[ "$source" == *\?rev=* ]]; then
            rev=${source#*\?rev=}
            rev=${rev%%#*}
            if [[ "$rev" =~ ^[0-9a-f]+$ && "$hash" != "$rev"* ]]; then
                fail "Cargo.lock of commit $cad_commit resolves rev $rev to $hash"
            fi
        fi
    done <<< "$sources"
    if ! cmp -s <(git -C "$dir" show "HEAD:LICENSE" | tr -d '\r') <(tr -d '\r' < "$native_dir/desktop/LICENSE-GPL-3.0"); then
        fail "the LICENSE of commit $cad_commit is not the GPL-3.0 text of native/desktop/LICENSE-GPL-3.0, which the packages carry for Open CAD Studio; look at its licence before it is built"
    fi
}

# The crates that the pinned commit takes from git repositories, as
# archive-open-cad-studio-source.sh vendors them for the release page: a
# folder with one folder per crate and config.toml, which replaces those git
# repositories by the folder. The first line of config.toml names the commit
# that they were vendored for.
cad_vendor_dir=$cad_target_dir/vendor

# cad_vendor_header prints the comment at the top of config.toml in the
# vendored folder. read_cad_pin first.
cad_vendor_header() {
    printf '# Git dependencies of Open CAD Studio commit %s\n' "$cad_commit"
    cat <<'HEADER'
#
# The crates that the Cargo.lock of that commit takes from git repositories,
# as `cargo vendor --locked --versioned-dirs` writes them. In the folder
# vendor/ of the source of that commit, they build it without those
# repositories:
#
#     cargo build --release --locked --bin OpenCADStudio --config vendor/config.toml
#
# Cargo downloads the crates from crates.io as usual.

HEADER
}

# check_cad_vendor DIR fails unless DIR holds the crates vendored for the
# pinned commit, as its config.toml says. read_cad_pin first.
check_cad_vendor() {
    local dir=$1 first
    [[ -f "$dir/config.toml" ]] || fail "$dir/config.toml does not exist"
    first=$(head -n 1 "$dir/config.toml" | tr -d '\r')
    [[ "$first" == "$(cad_vendor_header | head -n 1)" ]] \
        || fail "$dir holds no crates vendored for commit $cad_commit; its config.toml begins '$first'"
}
