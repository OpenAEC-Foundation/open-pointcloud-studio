#!/usr/bin/env bash
# Tests of the packaging of the C library. They need no built library and no
# network: stand-in libraries, a stand-in Cargo and a registry of a few
# made-up crates take their place, so the tests also run on a developer
# machine.
#
#   test-sdk-packaging.sh
#
# - the names of the SDK archives against the download patterns of the
#   foundation's website;
# - the layout of each archive that build-sdk-archive.sh writes;
# - third-party-notices.py on the made-up crates: what it lists, what it
#   refuses, and that its output does not change between runs;
# - the functions that check-exports.sh reads from the header.
#
# Runs with the bash of Git for Windows as well.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
. "$here/../../packaging/common.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

failures=0
passed() { echo "ok    $*"; }
wrong() {
    echo "WRONG $*"
    failures=$((failures + 1))
}

# A path as a program of the system reads it: Python on Windows does not
# know the paths of the bash of Git for Windows.
native_path() {
    if command -v cygpath > /dev/null 2>&1; then
        cygpath -m "$1"
    else
        printf '%s\n' "$1"
    fi
}

python=
for candidate in python3 python; do
    if "$candidate" -c 'import sys; sys.exit(sys.version_info < (3, 8))' > /dev/null 2>&1; then
        python=$candidate
        break
    fi
done
[[ -n "$python" ]] || fail "no Python 3.8 or newer found"

# ---- the names against the website ---------------------------------------

# The patterns by which shared/downloads.js of the foundation's website
# (repository OpenAEC-Foundation/website, as of 24 October 2026) gives a
# release file to a download button, with the key of each. The first match
# in name order wins, and a name that matches nothing is offered by no
# button. They are matched without regard to case, as the script does.
website_patterns=(
    'windows=(_x64-setup|-windows-setup)\.exe$'
    'windows-msi=\.msi$'
    'windows-zip=(-windows|_windows-x64)\.zip$'
    'macos=\.dmg$'
    'macos-tar=(-macos|_macos-universal)\.tar\.gz$'
    'linux-appimage=\.appimage$'
    'linux-deb=_amd64\.deb$'
    'linux-rpm=\.rpm$'
    'linux-tar=(-linux|_linux-amd64)\.tar\.gz$'
    'android=\.apk$'
    'linux-arm64-appimage=[_.-](arm64|aarch64)\.appimage$'
    'linux-arm64-deb=[_.-](arm64|aarch64)\.deb$'
    'linux-arm64-tar=[_-]linux-(arm64|aarch64)\.tar\.gz$'
    'plugin-windows=-windows-x86_64\.dll$'
    'plugin-linux=-linux-x86_64\.so$'
    'plugin-macos=-macos-aarch64\.dylib$'
    'plugin-manifest=^plugin\.toml$'
)

# website_keys NAME prints the keys whose pattern matches NAME.
website_keys() {
    local entry keys=()
    shopt -s nocasematch
    for entry in "${website_patterns[@]}"; do
        if [[ "$1" =~ ${entry#*=} ]]; then
            keys+=("${entry%%=*}")
        fi
    done
    shopt -u nocasematch
    echo "${keys[*]:-}"
}

assets=$(bash "$packaging_dir/expected-assets.sh" 9.9.9)
sdk_names=$(grep -F -- "-sdk_" <<< "$assets" || true)
expected_sdk="${BINARY_NAME}-sdk_9.9.9_windows-x64-dll.zip
${BINARY_NAME}-sdk_9.9.9_macos-universal-dylib.tar.gz
${BINARY_NAME}-sdk_9.9.9_linux-amd64-so.tar.gz
${BINARY_NAME}-sdk_9.9.9_linux-arm64-so.tar.gz"
if [[ "$sdk_names" == "$expected_sdk" ]]; then
    passed "the release holds the four SDK archives"
else
    wrong "expected-assets.sh lists '$(echo $sdk_names)' as SDK archives"
fi

matched=
for name in $sdk_names; do
    keys=$(website_keys "$name")
    [[ -z "$keys" ]] || matched="$matched $name ($keys)"
done
if [[ -z "$matched" ]]; then
    passed "no download button of the website offers an SDK archive"
else
    wrong "the website would offer SDK archives:$matched"
fi

# The copied patterns still find the files of the application, so the test
# above is not silent because they match nothing.
controls=(
    "${BINARY_NAME}_9.9.9_x64-setup.exe=windows"
    "${BINARY_NAME}_9.9.9_windows-x64.zip=windows-zip"
    "${BINARY_NAME}_9.9.9_macos-universal.dmg=macos"
    "${BINARY_NAME}_9.9.9_macos-universal.tar.gz=macos-tar"
    "${BINARY_NAME}_9.9.9_linux-amd64.tar.gz=linux-tar"
    "${BINARY_NAME}_9.9.9_linux-arm64.tar.gz=linux-arm64-tar"
)
missed=
for control in "${controls[@]}"; do
    name=${control%%=*}
    grep -qxF "$name" <<< "$assets" || missed="$missed $name (not a release file)"
    [[ " $(website_keys "$name") " == *" ${control#*=} "* ]] || missed="$missed $name"
done
if [[ -z "$missed" ]]; then
    passed "the copied patterns find the files of the application"
else
    wrong "the copied patterns do not find:$missed"
fi

# ---- a registry of made-up crates and a stand-in Cargo -------------------

registry="$work/cargo-home/registry/src/index.example-0000"
mkdir -p "$registry/alpha-1.0.0" "$registry/beta-2.1.0" "$registry/gamma-0.3.0" "$registry/delta-1.0.0"
echo "alpha MIT text" > "$registry/alpha-1.0.0/LICENSE-MIT"
printf 'alpha Apache text\r\n' > "$registry/alpha-1.0.0/license-apache"
echo "not a licence" > "$registry/alpha-1.0.0/README.md"
echo "gamma copying text" > "$registry/gamma-0.3.0/COPYING"
echo "delta notice text" > "$registry/delta-1.0.0/NOTICE"

# The stand-in answers `cargo metadata` and `cargo tree` from files in
# FAKE_CARGO_DIR, one tree per target, and writes down what it was asked.
cat > "$work/fake-cargo.py" <<'EOF'
import os, sys
folder = os.environ["FAKE_CARGO_DIR"]
arguments = sys.argv[1:]
with open(os.path.join(folder, "calls.log"), "a", encoding="utf-8") as log:
    log.write(" ".join(arguments) + "\n")
if arguments[0] == "metadata":
    name = "metadata.json"
else:
    name = "tree-%s.txt" % arguments[arguments.index("--target") + 1]
try:
    with open(os.path.join(folder, name), encoding="utf-8") as answer:
        sys.stdout.write(answer.read())
except FileNotFoundError:
    sys.stderr.write("error: no %s\n" % name)
    sys.exit(101)
EOF

# fake_cargo DIR TARGET LINE... writes the tree of TARGET into DIR.
fake_tree() {
    local folder=$1 target=$2
    shift 2
    mkdir -p "$folder"
    echo '{"packages": [{"name": "pointcloud-sdk", "version": "9.9.9"}, {"name": "member", "version": "1.2.3"}]}' \
        > "$folder/metadata.json"
    printf '%s\n' "$@" > "$folder/tree-$target.txt"
}

# notices DIR OUTPUT TARGET... runs the notices script with the stand-in.
notices() {
    local folder=$1 output=$2
    shift 2
    local arguments=() target
    for target in "$@"; do
        arguments+=(--target "$target")
    done
    CARGO_HOME=$(native_path "$work/cargo-home") FAKE_CARGO_DIR=$(native_path "$folder") \
        OPS_NOTICES_CARGO="$python -I '$(native_path "$work/fake-cargo.py")'" \
        "$python" -I "$native_dir/tools/third-party-notices.py" --manifest-path "$native_dir/Cargo.toml" \
        --package pointcloud-sdk "${arguments[@]}" --output "$output"
}

ordinary_tree=(
    'pointcloud-sdk v9.9.9 (/somewhere/native/sdk)|LGPL-3.0-or-later|'
    'member v1.2.3 (/somewhere/native/member)|LGPL-3.0-or-later|'
    'beta v2.1.0|MIT OR Apache-2.0|https://example.org/beta'
    'alpha v1.0.0|MIT/Apache-2.0|https://example.org/alpha'
    'gamma v0.3.0 (proc-macro)|Unlicense OR MIT|'
    'alpha v1.0.0|MIT/Apache-2.0|https://example.org/alpha (*)'
)
for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-msvc x86_64-apple-darwin; do
    fake_tree "$work/tree" "$target" "${ordinary_tree[@]}"
done
fake_tree "$work/tree" aarch64-apple-darwin "${ordinary_tree[@]}" \
    'delta v1.0.0|Apache-2.0 WITH LLVM-exception|https://example.org/delta'

# ---- third-party-notices.py ----------------------------------------------

if notices "$work/tree" "$work/linux.txt" x86_64-unknown-linux-gnu > /dev/null 2>"$work/error.txt"; then
    listed=$(sed -n '/^=\{78\}$/{n;p;}' "$work/linux.txt" | tr '\n' ' ')
    if [[ "$listed" == "alpha 1.0.0 beta 2.1.0 gamma 0.3.0 " ]]; then
        passed "the notices list the crates once, sorted, without the members of the workspace"
    else
        wrong "the notices list '$listed'"
    fi
    alpha=$(sed -n '/^alpha 1.0.0$/,/^=\{78\}$/p' "$work/linux.txt")
    if grep -qxF 'alpha MIT text' <<< "$alpha" && grep -qxF 'alpha Apache text' <<< "$alpha" \
        && grep -qxF 'LICENSE-MIT' <<< "$alpha" && ! grep -qF 'not a licence' <<< "$alpha" \
        && grep -qxF 'Repository: https://example.org/alpha' <<< "$alpha"; then
        passed "the notices carry the licence files of a crate and nothing else of it"
    else
        wrong "the notices of alpha are:"
        sed 's/^/        /' <<< "$alpha"
    fi
    if grep -qxF 'Standard text: https://spdx.org/licenses/MIT.html' "$work/linux.txt" \
        && grep -qxF 'Standard text: https://spdx.org/licenses/Apache-2.0.html' "$work/linux.txt"; then
        passed "a crate without licence files is given the standard text of each licence it names"
    else
        wrong "the notices of a crate without licence files name no standard texts"
    fi
    if grep -q $'\r' "$work/linux.txt"; then
        wrong "the notices hold carriage returns"
    else
        passed "the notices have one kind of line ending"
    fi
    calls=$(grep '^tree ' "$work/tree/calls.log" | head -n 1)
    if [[ "$calls" == *"-p pointcloud-sdk"* && "$calls" == *"-e normal"* && "$calls" == *"--locked"* \
        && "$calls" == *"--target x86_64-unknown-linux-gnu"* && "$calls" == *"--prefix none"* ]]; then
        passed "the notices ask Cargo for the normal dependencies of the package for the target, locked"
    else
        wrong "the notices asked Cargo: $calls"
    fi
else
    wrong "the notices of ordinary crates were refused:"
    sed 's/^/        /' "$work/error.txt"
fi

notices "$work/tree" "$work/again.txt" x86_64-unknown-linux-gnu > /dev/null 2>&1 || true
if cmp -s "$work/linux.txt" "$work/again.txt"; then
    passed "the notices are the same on every run"
else
    wrong "two runs of the notices differ"
fi

if notices "$work/tree" "$work/macos.txt" x86_64-apple-darwin aarch64-apple-darwin > /dev/null 2>&1 \
    && grep -qxF 'delta 1.0.0' "$work/macos.txt" && grep -qxF 'delta notice text' "$work/macos.txt" \
    && grep -qxF 'alpha 1.0.0' "$work/macos.txt" \
    && grep -qF 'Built for: aarch64-apple-darwin, x86_64-apple-darwin' "$work/macos.txt"; then
    passed "the notices of two targets hold the crates of both"
else
    wrong "the notices of two targets miss the crates of one"
fi

# refused NAME DESCRIPTION LINE: the notices of a tree with LINE must fail
# and name the crate NAME.
refused() {
    local name=$1 description=$2 line=$3
    local folder="$work/refused-$name"
    fake_tree "$folder" x86_64-unknown-linux-gnu "${ordinary_tree[@]}" "$line"
    if notices "$folder" "$folder/notices.txt" x86_64-unknown-linux-gnu > /dev/null 2> "$folder/error.txt"; then
        wrong "the notices accept $description"
    elif ! grep -qF "$name" "$folder/error.txt"; then
        wrong "the refusal of $description does not name $name: $(cat "$folder/error.txt")"
    elif [[ -e "$folder/notices.txt" ]]; then
        wrong "the notices were written although $description was refused"
    else
        passed "the notices refuse $description"
    fi
}
mkdir -p "$registry/epsilon-1.0.0" "$registry/zeta-1.0.0" "$registry/eta-1.0.0" "$registry/iota-1.0.0"
refused epsilon "a licence nobody has looked at" 'epsilon v1.0.0|GPL-3.0-only|'
refused zeta "a licence nobody has looked at, also when it is common" 'zeta v1.0.0|Unlicense|'
refused eta "a licence expression that cannot be read" 'eta v1.0.0|MIT OR|'
refused theta "a crate whose source was not downloaded" 'theta v1.0.0|MIT|'
refused iota "a crate from a git repository" 'iota v1.0.0 (https://example.org/iota#0123abc)|MIT|'
refused kappa "a crate without a licence" 'kappa v1.0.0||'

# An expression is accepted when the crate can be used under licences that
# have been looked at.
accepted=
for expression in 'MIT' 'Unlicense OR MIT' '(MIT OR Apache-2.0) AND Unicode-3.0' \
    'Apache-2.0 WITH LLVM-exception OR MIT' 'MIT/Apache-2.0' '(Apache-2.0 OR MIT) AND BSD-3-Clause'; do
    folder="$work/accepted-$(printf '%s' "$expression" | tr -c 'A-Za-z0-9' '_')"
    mkdir -p "$registry/lambda-1.0.0"
    fake_tree "$folder" x86_64-unknown-linux-gnu "lambda v1.0.0|$expression|"
    notices "$folder" "$folder/notices.txt" x86_64-unknown-linux-gnu > /dev/null 2>&1 \
        || accepted="$accepted '$expression'"
done
for expression in 'GPL-3.0-only AND MIT' 'MIT AND (Unlicense OR GPL-2.0-only)' 'MIT WITH LLVM-exception2'; do
    folder="$work/refused-$(printf '%s' "$expression" | tr -c 'A-Za-z0-9' '_')"
    fake_tree "$folder" x86_64-unknown-linux-gnu "lambda v1.0.0|$expression|"
    ! notices "$folder" "$folder/notices.txt" x86_64-unknown-linux-gnu > /dev/null 2>&1 \
        || accepted="$accepted (wrongly) '$expression'"
done
if [[ -z "$accepted" ]]; then
    passed "licence expressions are read with AND, OR, WITH, parentheses and the old slash"
else
    wrong "licence expressions misread:$accepted"
fi

# ---- build-sdk-archive.sh ------------------------------------------------

lib_dir="$work/lib"
mkdir -p "$lib_dir"
for file in libops_pointcloud.so libops_pointcloud.dylib ops_pointcloud.dll ops_pointcloud.dll.lib; do
    echo "stand-in $file" > "$lib_dir/$file"
done

# archive_files ARCHIVE prints the files in it, without folders, sorted.
archive_files() {
    case "$1" in
        *.zip)
            "$python" -I -c 'import sys, zipfile; print("\n".join(zipfile.ZipFile(sys.argv[1]).namelist()))' \
                "$(native_path "$1")" | tr -d '\r'
            ;;
        *) tar -tzf "$1" ;;
    esac | grep -v '/$' | sort
}

common_files="examples/inspect.c
include/ops_pointcloud.h
LICENSE-GPL-3.0.txt
LICENSE-LGPL-3.0.md
README.md
THIRD-PARTY-NOTICES.txt"
for case in "windows-x64-dll:zip:bin/ops_pointcloud.dll lib/ops_pointcloud.dll.lib:x86_64-pc-windows-msvc" \
    "linux-amd64-so:tar.gz:lib/libops_pointcloud.so lib/libops_pointcloud.so.1:x86_64-unknown-linux-gnu" \
    "linux-arm64-so:tar.gz:lib/libops_pointcloud.so lib/libops_pointcloud.so.1:aarch64-unknown-linux-gnu" \
    "macos-universal-dylib:tar.gz:lib/libops_pointcloud.dylib:aarch64-apple-darwin, x86_64-apple-darwin"; do
    IFS=: read -r target extension libraries triples <<< "$case"
    package="${BINARY_NAME}-sdk_9.9.9_${target}"
    out="$work/out-$target"
    if ! CARGO_HOME=$(native_path "$work/cargo-home") FAKE_CARGO_DIR=$(native_path "$work/tree") \
        OPS_NOTICES_CARGO="$python -I '$(native_path "$work/fake-cargo.py")'" \
        bash "$here/build-sdk-archive.sh" "$lib_dir" 9.9.9 "$target" "$out" > "$work/archive.log" 2>&1; then
        wrong "build-sdk-archive.sh failed for $target:"
        sed 's/^/        /' "$work/archive.log"
        continue
    fi
    archive="$out/$package.$extension"
    expected=$( (printf '%s\n' "$common_files"; printf '%s\n' $libraries) | sed "s|^|$package/|" | sort)
    if [[ -f "$archive" && "$(archive_files "$archive")" == "$expected" ]]; then
        passed "the $target archive holds the library, header, example, README, licences and notices"
    else
        wrong "the $target archive holds:"
        archive_files "$archive" 2>&1 | sed 's/^/        /'
    fi
    if (cd "$out" && sha256sum -c "$package.$extension.sha256" > /dev/null 2>&1); then
        passed "the $target archive has a .sha256 that checks"
    else
        wrong "the $target archive has no .sha256 that checks"
    fi
    if grep -qF "Built for: $triples" "$out/$package/THIRD-PARTY-NOTICES.txt" \
        && cmp -s "$out/$package/README.md" "$native_dir/sdk/README.md" \
        && cmp -s "$out/$package/include/ops_pointcloud.h" "$native_dir/sdk/include/ops_pointcloud.h"; then
        passed "the $target archive has the notices of its targets and the README and header of the repository"
    else
        wrong "the $target archive has other notices, README or header"
    fi
done

# On Linux the name without the version is a link to the library, as the
# linker and the soname want it; Git for Windows copies a link instead.
case "$(uname -s)" in
    MINGW* | MSYS* | CYGWIN*) ;;
    *)
        listing=$(tar -tvzf "$work/out-linux-amd64-so/${BINARY_NAME}-sdk_9.9.9_linux-amd64-so.tar.gz")
        if grep -q 'lib/libops_pointcloud\.so -> libops_pointcloud\.so\.1$' <<< "$listing"; then
            passed "libops_pointcloud.so is a link to libops_pointcloud.so.1"
        else
            wrong "libops_pointcloud.so is no link to libops_pointcloud.so.1"
        fi
        ;;
esac

if bash "$here/build-sdk-archive.sh" "$work/empty" 9.9.9 linux-amd64-so "$work/out-empty" > /dev/null 2>&1; then
    wrong "build-sdk-archive.sh packs a folder without a library"
else
    passed "build-sdk-archive.sh refuses a folder without a library"
fi
if bash "$here/build-sdk-archive.sh" "$lib_dir" 9.9.9 linux-amd64 "$work/out-wrong" > /dev/null 2>&1; then
    wrong "build-sdk-archive.sh packs for a target with an application name"
else
    passed "build-sdk-archive.sh refuses a target it does not know"
fi

# ---- check-exports.sh ----------------------------------------------------

declared=$(bash "$here/check-exports.sh" --declared)
if [[ $(wc -l <<< "$declared") -eq 15 && "$declared" == *ops_cloud_open* && "$declared" == *ops_index_read* \
    && "$declared" == *ops_version* ]]; then
    passed "check-exports.sh reads the fifteen functions of the header"
else
    wrong "check-exports.sh reads from the header: $(echo $declared)"
fi

if ((failures > 0)); then
    echo "$failures test(s) of the SDK packaging failed"
    exit 1
fi
echo "all tests of the SDK packaging passed"
