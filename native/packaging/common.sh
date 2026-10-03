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

fail() {
    echo "$(basename "$0"): $*" >&2
    exit 1
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
