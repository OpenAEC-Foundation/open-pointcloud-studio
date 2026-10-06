# Shared by the package scripts: where things are in the repository, the
# names every package uses, and the files every package carries.
#
# Sourced, not run. Written for the bash 3.2 of macOS as well.

packaging_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
native_dir=$(cd "$packaging_dir/.." && pwd)
repo_dir=$(cd "$native_dir/.." && pwd)

# The application id is the name of the desktop entry and its icon on Linux
# and the bundle identifier on macOS. The window announces the same id, which
# is how a desktop shell ties a window to its launcher.
APP_ID=org.openaec.OpenPointcloudStudio
APP_NAME="Open Pointcloud Studio"
BINARY_NAME=open-pointcloud-studio

# Open CAD Studio, the CAD program that shows exported drawings. It keeps
# the name of its upstream binary, which the application looks for. Its
# source is not part of this repository: open-cad-studio.pin names the one
# commit of its repository that is built, and fetch_cad_source fetches it
# into a folder of the target folder. The OCS_ variables move these places
# for the tests of the scripts.
CAD_BINARY_NAME=OpenCADStudio
cad_pin_file=${OCS_PIN_FILE:-$packaging_dir/open-cad-studio.pin}
cad_target_dir=${OCS_TARGET_DIR:-$native_dir/target/open-cad-studio}
cad_source_dir=${OCS_SOURCE_DIR:-$cad_target_dir/source}

fail() {
    echo "$(basename "$0"): $*" >&2
    exit 1
}

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
# URL, or from OCS_FETCH_FROM, a local clone or a mirror, when that is set;
# a checkout that is already at the commit is used without fetching. Files
# that differ from the commit are restored and files it does not have are
# removed, so a build never takes a change made by hand. A checkout that has
# the commit is not checked out again: the build script of the program
# watches .git/HEAD, and Cargo would compile its largest crate again.
fetch_cad_source() {
    local dir=$cad_source_dir from=${OCS_FETCH_FROM:-$cad_url} head tree date changed
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
# GPL-3.0 text of native/desktop/LICENSE-GPL-3.0.
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
        fail "the LICENSE of commit $cad_commit is not the GPL-3.0 text of native/desktop/LICENSE-GPL-3.0; look at its licence before it is built"
    fi
}

# The licences travel with every copy of the binary.
copy_licences() {
    local destination=$1
    mkdir -p "$destination"
    cp "$repo_dir/LICENSE.md" "$destination/LICENSE-LGPL-3.0.md"
    cp "$native_dir/desktop/LICENSE-GPL-3.0" "$destination/LICENSE-GPL-3.0.txt"
    cp "$native_dir"/assets/fonts/*-OFL.txt "$destination/"
    cp "$packaging_dir/NOTICE.txt" "$destination/NOTICE.txt"
}

# Write FILE.sha256 beside FILE, naming the file without its folder so that
# the check works wherever both are downloaded to.
write_sha256() {
    local file=$1
    local folder name
    folder=$(dirname "$file")
    name=$(basename "$file")
    (
        cd "$folder"
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum "$name" > "$name.sha256"
        else
            shasum -a 256 "$name" > "$name.sha256"
        fi
    )
}

# Replace the @NAME@ placeholders of a template: fill_template IN OUT NAME=value ...
fill_template() {
    local input=$1 output=$2
    shift 2
    local expression=() pair
    for pair in "$@"; do
        # '|' separates, so a value may hold slashes; none of ours holds '|'.
        expression+=(-e "s|@${pair%%=*}@|${pair#*=}|g")
    done
    sed "${expression[@]}" "$input" > "$output"
    if grep -n '@[A-Z_][A-Z_]*@' "$output" >&2; then
        fail "placeholders left in $output"
    fi
}
