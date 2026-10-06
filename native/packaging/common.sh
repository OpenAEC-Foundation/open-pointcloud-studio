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

# Open CAD Studio, the CAD program every package carries beside the
# application. It keeps the name of its upstream binary, which the
# application looks for. Its source is not part of this repository:
# open-cad-studio-source.sh reads the pin file that names the one commit of
# its repository that is built, and fetches and checks that commit.
CAD_BINARY_NAME=OpenCADStudio

# The notices that travel with every package: NOTICE.txt of the application
# and the template of that of Open CAD Studio. The OCS_ variables move them
# for the tests of the scripts.
notice_file=${OCS_NOTICE:-$packaging_dir/NOTICE.txt}
cad_notice_template=${OCS_NOTICE_TEMPLATE:-$packaging_dir/$CAD_BINARY_NAME-NOTICE.txt.in}

fail() {
    echo "$(basename "$0"): $*" >&2
    exit 1
}

. "$packaging_dir/open-cad-studio-source.sh"

# The name of the release file that holds the source of the pinned commit.
cad_source_archive_name() {
    read_cad_pin
    echo "open-cad-studio-source_${cad_commit:0:8}.tar.gz"
}

# The name of the release file that holds the crates that the pinned commit
# takes from git repositories (archive-open-cad-studio-source.sh).
cad_vendor_archive_name() {
    read_cad_pin
    echo "open-cad-studio-vendor_${cad_commit:0:8}.tar.gz"
}

# mingw_runtime_of PROGRAM prints, one per line, the libraries of the C++
# runtime of MinGW that the Windows program PROGRAM loads by name. A build of
# Open CAD Studio with the GNU toolchain loads libstdc++-6.dll, and through it
# the other two; build-open-cad-studio.sh puts them beside it for a
# development build. No package carries them, nor their licences, so a
# package takes a build with the MSVC toolchain, which loads none of them.
mingw_runtime_of() {
    local library
    for library in libstdc++-6.dll libgcc_s_seh-1.dll libwinpthread-1.dll; do
        if LC_ALL=C grep -aqF "$library" "$1"; then
            echo "$library"
        fi
    done
}

# refuse_mingw_runtime PROGRAM fails when PROGRAM loads a library of the C++
# runtime of MinGW (see mingw_runtime_of).
refuse_mingw_runtime() {
    local libraries
    libraries=$(mingw_runtime_of "$1" | tr '\n' ' ')
    [[ -z "$libraries" ]] \
        || fail "$1 loads ${libraries% } of MinGW, which no package carries: it was built with the GNU toolchain; build it with build-open-cad-studio.sh --target x86_64-pc-windows-msvc, as the Packages workflow does"
}

# The licences travel with every copy of the binary, and those of Open CAD
# Studio with it: copy_licences DESTINATION NUMBER, where NUMBER is the
# version of the package.
copy_licences() {
    local destination=$1 number=$2
    mkdir -p "$destination"
    cp "$repo_dir/LICENSE.md" "$destination/LICENSE-LGPL-3.0.md"
    cp "$native_dir/desktop/LICENSE-GPL-3.0" "$destination/LICENSE-GPL-3.0.txt"
    cp "$native_dir"/assets/fonts/*-OFL.txt "$destination/"
    cp "$notice_file" "$destination/NOTICE.txt"
    copy_cad_licence "$destination"
    copy_cad_notice "$destination" "$number"
}

# The licence text of Open CAD Studio. It is the GPL-3.0 text, the same as
# the LICENSE of the pinned commit: check_cad_source refuses to build a
# commit whose LICENSE says otherwise.
copy_cad_licence() {
    local destination=$1
    mkdir -p "$destination"
    cp "$native_dir/desktop/LICENSE-GPL-3.0" "$destination/$CAD_BINARY_NAME-LICENSE.txt"
}

# The notice of Open CAD Studio, which names the pinned commit, the crates it
# takes from git repositories and the release files with their source:
# copy_cad_notice DESTINATION NUMBER [LICENCE_TEXT].
# LICENCE_TEXT is where the notice says the licence text is, by default
# OpenCADStudio-LICENSE.txt beside it, which copy_cad_licence writes.
copy_cad_notice() {
    local destination=$1 number=$2 licence_text=${3:-$CAD_BINARY_NAME-LICENSE.txt}
    [[ -n "$number" ]] || fail "copy_cad_notice needs the version of the package"
    read_cad_pin
    mkdir -p "$destination"
    fill_template "$cad_notice_template" "$destination/$CAD_BINARY_NAME-NOTICE.txt" \
        "URL=${cad_url%.git}" "COMMIT=$cad_commit" "DATE=$cad_date" "VERSION=$number" \
        "ARCHIVE=$(cad_source_archive_name)" "VENDOR_ARCHIVE=$(cad_vendor_archive_name)" \
        "LICENCE_TEXT=$licence_text"
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
