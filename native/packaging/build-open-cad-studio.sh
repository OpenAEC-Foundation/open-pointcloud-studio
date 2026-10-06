#!/usr/bin/env bash
# Builds Open CAD Studio, the CAD program that shows exported drawings, from
# the one commit of its repository that open-cad-studio.pin names.
#
#   build-open-cad-studio.sh [CARGO_OPTIONS...]
#   build-open-cad-studio.sh --fetch    only fetch and check the source
#   build-open-cad-studio.sh --pin      only read the pin file and print it
#
# The source is not part of this repository. The pinned commit is fetched
# into native/target/open-cad-studio/source, a git repository of its own, and
# nothing is built unless the tree of that commit is the tree the pin file
# names (see fetch_cad_source in common.sh). It is built as it is, without a
# change: its Cargo.lock names the full hash of one commit for every git
# dependency, which check_cad_source checks, and `cargo build --locked`
# builds those commits and nothing newer. OCS_FETCH_FROM fetches from another
# place than the pinned URL, such as a local clone; the tree is checked all
# the same.
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
# --pin prints url=, commit=, tree= and date= lines.
#
# Built with the GNU toolchain for Windows, the program links the C++ runtime
# of MinGW, libstdc++-6.dll, for a mesh library written in C++. That library
# and the ones it needs in turn are copied from the folder of g++ to beside
# the program, so that it also starts where MinGW is not on the search path,
# as from a development build of the application. Built with the MSVC
# toolchain, it needs none of this.
#
# Runs with bash 3.2 (macOS) and with the bash of Git for Windows.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

read_cad_pin
case "${1:-}" in
    --pin)
        printf 'url=%s\ncommit=%s\ntree=%s\ndate=%s\n' "$cad_url" "$cad_commit" "$cad_tree" "$cad_date"
        exit 0
        ;;
    --fetch)
        fetch_cad_source
        check_cad_source
        echo "$cad_source_dir"
        exit 0
        ;;
esac

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
cargo build --release --locked --bin "$CAD_BINARY_NAME" --target-dir "$target_dir" "$@" >&2

[[ -f "$binary" ]] || fail "cargo finished, but $binary does not exist"
copy_mingw_runtime "$binary"
echo "$binary"
