#!/usr/bin/env bash
# Builds Open CAD Studio, the CAD program every package carries beside the
# application, from the one commit of its repository that
# open-cad-studio.pin names.
#
#   build-open-cad-studio.sh [CARGO_OPTIONS...]
#   build-open-cad-studio.sh --fetch            only fetch and check the source
#   build-open-cad-studio.sh --pin              only read the pin file and print it
#   build-open-cad-studio.sh --export FOLDER    write a bare repository with
#                                               only the pinned commit
#
# The source is not part of this repository. The pinned commit is fetched
# into native/target/open-cad-studio/source, a git repository of its own, and
# nothing is built unless the tree of that commit is the tree the pin file
# names (see fetch_cad_source in open-cad-studio-source.sh). It is built as
# it is, without a change: its Cargo.lock names the full hash of one commit
# for every git dependency, which check_cad_source checks, and
# `cargo build --locked` builds those commits and nothing newer.
# OCS_FETCH_FROM fetches from another place than the pinned URL, such as a
# local clone or the folder that --export wrote; the tree is checked all the
# same. OCS_VENDOR_DIR names a folder of crates that
# archive-open-cad-studio-source.sh vendored for the pinned commit, such as
# the vendor/ of its release file: Cargo then builds those in place of their
# git repositories, which it does not fetch from. A folder vendored for
# another commit is refused.
#
# The options go to `cargo build`, for example `-j 4` or
# `--target aarch64-apple-darwin`. Open CAD Studio is a Cargo workspace of its
# own, with other versions of iced and wgpu than the application, so it has
# its own target folder: the program is written to
# native/target/open-cad-studio[/TARGET]/release/OpenCADStudio[.exe], and its
# path is printed last. A development build of the application finds it
# there without a --target. Cargo runs in the source folder, so that the
# .cargo/config.toml of Open CAD Studio applies: it gives the program its
# stack size on Windows. Run again with nothing changed, Cargo has nothing to
# do; only another commit in the pin file is fetched and built again.
#
# --pin prints url=, commit=, tree=, date=, archive= and vendor= lines (the
# names of the release files with the source and with the crates from git
# repositories), as the Packages workflow reads them. With --export, the
# job of the workflow that checked the commit hands it to the jobs that
# build it, so that they fetch nothing from its repository.
#
# Built with the GNU toolchain for Windows, the program links the C++ runtime
# of MinGW, libstdc++-6.dll, for a mesh library written in C++. That library
# and the ones it needs in turn are copied from the folder of g++ to beside
# the program, so that it also starts where MinGW is not on the search path,
# as from a development build of the application. The packages build it with
# the MSVC toolchain, which needs none of this.
#
# Runs with bash 3.2 (macOS) and with the bash of Git for Windows.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

read_cad_pin
case "${1:-}" in
    --pin)
        printf 'url=%s\ncommit=%s\ntree=%s\ndate=%s\narchive=%s\nvendor=%s\n' \
            "$cad_url" "$cad_commit" "$cad_tree" "$cad_date" "$(cad_source_archive_name)" \
            "$(cad_vendor_archive_name)"
        exit 0
        ;;
    --fetch)
        fetch_cad_source
        check_cad_source
        echo "$cad_source_dir"
        exit 0
        ;;
    --export)
        [[ $# -eq 2 ]] || fail "usage: build-open-cad-studio.sh --export FOLDER"
        fetch_cad_source
        rm -rf "$2"
        git init -q --bare "$2"
        git -C "$2" fetch -q --depth 1 "$(cd "$cad_source_dir" && pwd)" "$cad_commit:refs/heads/pinned" \
            || fail "commit $cad_commit could not be put into $2"
        echo "$2"
        exit 0
        ;;
esac

# The crates from git repositories that archive-open-cad-studio-source.sh
# vendored for this commit, in place of those repositories.
vendor_config=()
if [[ -n "${OCS_VENDOR_DIR:-}" ]]; then
    [[ -d "$OCS_VENDOR_DIR" ]] || fail "OCS_VENDOR_DIR $OCS_VENDOR_DIR is no folder"
    vendor_dir=$(cd "$OCS_VENDOR_DIR" && pwd)
    check_cad_vendor "$vendor_dir"
    vendor_config=(--config "$vendor_dir/config.toml")
fi

target=
previous=
for argument in "$@"; do
    case "$argument" in
        --target=*) target=${argument#--target=} ;;
    esac
    [[ "$previous" != --target ]] || target=$argument
    previous=$argument
done
mkdir -p "$cad_target_dir"
target_dir=$(cd "$cad_target_dir" && pwd)
binary=$target_dir/${target:+$target/}release/$CAD_BINARY_NAME
if [[ "$target" == *windows* || ( -z "$target" && ( "$OSTYPE" == msys* || "$OSTYPE" == cygwin* ) ) ]]; then
    binary=$binary.exe
fi

triple=${target:-$(rustc -vV | sed -n 's/^host: //p' | tr -d '\r')}

# copy_mingw_runtime PROGRAM copies the libraries of MinGW that PROGRAM needs,
# and those that they need, from the folder of g++ to beside PROGRAM.
copy_mingw_runtime() {
    local program=$1 folder compiler bin file name
    [[ "$triple" == *-windows-gnu* ]] || return 0
    folder=$(dirname "$program")
    compiler=$(command -v g++ || true)
    if [[ -z "$compiler" ]] || ! command -v objdump >/dev/null 2>&1; then
        echo "g++ or objdump was not found: $program starts only where the C++ runtime of MinGW is on the search path" >&2
        return 0
    fi
    bin=$(dirname "$compiler")
    local queue=("$program") seen=" "
    while [[ ${#queue[@]} -gt 0 ]]; do
        file=${queue[0]}
        queue=("${queue[@]:1}")
        while IFS= read -r name; do
            [[ "$seen" != *" $name "* && -f "$bin/$name" ]] || continue
            seen="$seen$name "
            if ! cmp -s "$bin/$name" "$folder/$name"; then
                cp "$bin/$name" "$folder/$name"
                echo "copied $name of $bin beside $(basename "$program")" >&2
            fi
            queue+=("$bin/$name")
        done < <(objdump -p "$file" | sed -n 's/^[[:space:]]*DLL Name: //p' | tr -d '\r')
    done
}

fetch_cad_source
check_cad_source

# rustc overflows its default stack on the largest crate of the program in a
# release build; the .cargo/config.toml of Open CAD Studio sets the same.
export RUST_MIN_STACK=${RUST_MIN_STACK:-67108864}

cd "$cad_source_dir"
cargo build --release --locked --bin "$CAD_BINARY_NAME" --target-dir "$target_dir" \
    ${vendor_config[@]+"${vendor_config[@]}"} "$@" >&2

[[ -f "$binary" ]] || fail "cargo finished, but $binary does not exist"
copy_mingw_runtime "$binary"
echo "$binary"
