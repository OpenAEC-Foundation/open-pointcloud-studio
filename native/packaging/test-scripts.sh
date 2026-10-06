#!/usr/bin/env bash
# Tests of the package scripts themselves. They need no built binary and no
# packaging tool: the programs a script calls are replaced by stand-ins that
# are put first on the PATH, so the tests also run on a developer machine.
#
#   test-scripts.sh
#
# Runs with the bash of Git for Windows as well.
set -euo pipefail

. "$(dirname "${BASH_SOURCE[0]}")/common.sh"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"

failures=0
passed() { echo "ok    $*"; }
wrong() {
    echo "WRONG $*"
    failures=$((failures + 1))
}

# ---- linux/check-file-types.sh -------------------------------------------

# Answers the two questions the script asks. Like the real program it writes
# the list of applications line by line, with work in between: a reader that
# stops at the first line makes the next write fail.
cat > "$work/bin/gio" <<'EOF'
#!/usr/bin/env bash
case $1 in
info)
    case ${!#} in
    *.e57 | */unnamed-e57) type=model/e57 ;;
    *.las) type=application/vnd.las ;;
    *.laz) type=application/vnd.laszip ;;
    *.ply) type=application/x-ply ;;
    *.pcd) type=application/x-pcd ;;
    *.ptx) type=application/x-ptx ;;
    *.pts) type=application/x-pts ;;
    *.rcp) type=application/x-rcp ;;
    *) type=application/octet-stream ;;
    esac
    echo "attributes:"
    echo "  standard::content-type: $type"
    ;;
mime)
    echo "Default application for “$2”: $GIO_OFFERS"
    sleep 0.2
    echo "Registered applications:"
    echo "	$GIO_OFFERS"
    echo "Recommended applications:"
    echo "	$GIO_OFFERS"
    ;;
esac
EOF
chmod +x "$work/bin/gio"

# What smoke-test.sh leaves with SMOKE_KEEP_DIR; only the names matter here.
mkdir -p "$work/smoke/scans" "$work/smoke/out"
: > "$work/smoke/scans/grid.ply"
for extension in e57 las laz pts; do
    : > "$work/smoke/out/grid.$extension"
done

if output=$(PATH="$work/bin:$PATH" GIO_OFFERS="$APP_ID.desktop" \
    bash "$packaging_dir/linux/check-file-types.sh" "$work/smoke" 2>&1); then
    passed "check-file-types.sh accepts a system that offers the application"
else
    wrong "check-file-types.sh refuses a system that offers the application:"
    sed 's/^/        /' <<< "$output"
fi

if output=$(PATH="$work/bin:$PATH" GIO_OFFERS="org.example.Other.desktop" \
    bash "$packaging_dir/linux/check-file-types.sh" "$work/smoke" 2>&1); then
    wrong "check-file-types.sh accepts a system that offers another application"
else
    passed "check-file-types.sh refuses a system that offers another application"
fi

# ---- draft-release.sh ----------------------------------------------------

# Writes down what it is asked; GH_HAS_DRAFT says whether the release exists.
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$GH_LOG"
if [[ "$1 $2" == "release view" ]]; then
    [[ "$GH_HAS_DRAFT" == "true" ]]
fi
EOF
chmod +x "$work/bin/gh"
: > "$work/notes.md"

# draft_call HAS_DRAFT PRERELEASE prints the one call that writes the release.
draft_call() {
    : > "$work/gh.log"
    PATH="$work/bin:$PATH" GH_LOG="$work/gh.log" GH_HAS_DRAFT=$1 \
        GITHUB_REPOSITORY=example/repository GITHUB_SHA=0123abc \
        bash "$packaging_dir/draft-release.sh" v9.9.9 "$work/notes.md" "$2" >/dev/null || true
    grep -v '^release view ' "$work/gh.log" || true
}

call=$(draft_call false false)
if [[ "$call" == "release create v9.9.9 "*"--target 0123abc"* && "$call" == *--draft* && "$call" != *--prerelease* ]]; then
    passed "a new draft is created at the commit of the run"
else
    wrong "a new draft is not created at the commit of the run: gh $call"
fi

# Publishing a draft puts the tag where the draft points, so a draft from an
# earlier run has to be moved to the commit these packages were built from.
call=$(draft_call true false)
if [[ "$call" == "release edit v9.9.9 "*"--target 0123abc"* ]]; then
    passed "a draft from an earlier run is moved to the commit of the run"
else
    wrong "a draft from an earlier run keeps its old commit: gh $call"
fi

call=$(draft_call false true)
if [[ "$call" == *--prerelease* ]]; then
    passed "a pre-release is marked as one"
else
    wrong "a pre-release is not marked as one: gh $call"
fi

# ---- release-notes.sh ----------------------------------------------------

# Any released version will do; the footer is the same for all.
number=$(awk '/^## v?[0-9]/ { sub(/\r$/, ""); sub(/^v/, "", $2); print $2; exit }' "$repo_dir/CHANGELOG.md")
notes=$(bash "$packaging_dir/release-notes.sh" "$number")

# A downloaded file is not executable, so the notes must not name a command
# that starts the AppImage without saying how to make it so.
if grep -qF "chmod +x ${BINARY_NAME}_${number}_amd64.AppImage" <<< "$notes"; then
    passed "the release notes say how to make the AppImage executable"
else
    wrong "the release notes start the AppImage without making it executable"
fi

# The Linux packages are attested by the release workflow; the downloads
# part of the notes must say how to check one, in both languages, against
# the release workflow. Only that part is searched: the changelog section
# above it may quote the command as well.
downloads=${notes#*## Downloads}
signer="--signer-workflow OpenAEC-Foundation/open-pointcloud-studio/.github/workflows/release.yml"
if grep -qF -- "gh attestation verify FILE --repo OpenAEC-Foundation/open-pointcloud-studio $signer" <<< "$downloads"     && grep -qF -- "gh attestation verify BESTAND --repo OpenAEC-Foundation/open-pointcloud-studio $signer" <<< "$downloads"; then
    passed "the release notes say how to verify the attestation of a Linux package"
else
    wrong "the release notes do not say how to verify the attestation of a Linux package"
fi

missing=
for name in $(bash "$packaging_dir/expected-assets.sh" "$number"); do
    grep -qF "$name" <<< "$notes" || missing="$missing $name"
done
if [[ -z "$missing" ]]; then
    passed "the release notes name every file of the release"
else
    wrong "the release notes do not name:$missing"
fi

# ---- open-cad-studio.pin and build-open-cad-studio.sh --------------------

# The pin file of the repository holds only a URL, two hashes and a date.
if output=$(bash "$packaging_dir/build-open-cad-studio.sh" --pin 2>&1) \
    && grep -qE '^url=https://[^ ]+$' <<< "$output" \
    && grep -qE '^commit=[0-9a-f]{40}$' <<< "$output" \
    && grep -qE '^tree=[0-9a-f]{40}$' <<< "$output" \
    && grep -qE '^date=[0-9]{4}-[0-9]{2}-[0-9]{2}$' <<< "$output" \
    && [[ "$(grep -c . "$packaging_dir/open-cad-studio.pin")" -eq 4 ]]; then
    passed "the pin file names the repository, commit, tree and date of Open CAD Studio and nothing else"
else
    wrong "the pin file of Open CAD Studio is not read as four lines url, commit, tree and date:"
    sed 's/^/        /' <<< "$output"
fi

# A pin file that is not exactly that is refused.
good_hash=0123456789abcdef0123456789abcdef01234567
refused=0
for pin in \
    "url=https://example.org/a.git|commit=$good_hash|date=2026-10-01" \
    "url=http://example.org/a.git|commit=$good_hash|tree=$good_hash|date=2026-10-01" \
    "url=https://example.org/a.git|commit=0123abc|tree=$good_hash|date=2026-10-01" \
    "url=https://example.org/a.git|commit=$good_hash|tree=$good_hash|date=1 October" \
    "url=https://example.org/a.git|commit=$good_hash|tree=$good_hash|date=2026-10-01|branch=main" \
    "url=https://example.org/a.git|commit=$good_hash|tree=$good_hash|date=2026-10-01|commit=$good_hash" \
    "url=https://example.org/a.git|commit=$good_hash|tree=$good_hash|date=2026-10-01|a comment"; do
    tr '|' '\n' <<< "$pin" > "$work/bad.pin"
    if OCS_PIN_FILE="$work/bad.pin" bash "$packaging_dir/build-open-cad-studio.sh" --pin > /dev/null 2>&1; then
        wrong "build-open-cad-studio.sh reads the pin file '$pin'"
        refused=1
    fi
done
[[ "$refused" -ne 0 ]] || passed "build-open-cad-studio.sh refuses a pin file with a missing, malformed or other line"

# A repository in place of the upstream one. Its Cargo.lock follows a branch
# and names a rev by a shortened hash, both with the full hash of a commit.
upstream=$work/upstream
mkdir -p "$upstream/src"
cp "$native_dir/desktop/LICENSE-GPL-3.0" "$upstream/LICENSE"
printf 'fn main() {}\n' > "$upstream/src/main.rs"
cat > "$upstream/Cargo.lock" <<'EOF'
[[package]]
name = "followed"
version = "0.1.0"
source = "git+https://example.org/followed.git?branch=feature%2Fone#1111111111111111111111111111111111111111"

[[package]]
name = "shortened"
version = "0.1.0"
source = "git+https://example.org/short.git?rev=abc1234#abc1234000000000000000000000000000000000"
EOF
git -C "$upstream" init -q
git -C "$upstream" config core.autocrlf false
# commit_upstream MESSAGE commits what is there and prints the hash.
commit_upstream() {
    git -C "$upstream" add -A
    GIT_COMMITTER_DATE=2026-10-01T12:00:00Z GIT_AUTHOR_DATE=2026-10-01T12:00:00Z \
        git -C "$upstream" -c user.name=test -c user.email=test@example.org commit -q -m "$1"
    git -C "$upstream" rev-parse HEAD
}
# pin_upstream COMMIT [TREE [DATE]] writes the pin file for COMMIT.
pin_upstream() {
    printf 'url=https://example.org/upstream.git\ncommit=%s\ntree=%s\ndate=%s\n' "$1" \
        "${2:-$(git -C "$upstream" rev-parse "$1^{tree}")}" "${3:-2026-10-01}" > "$work/ocs.pin"
}
good=$(commit_upstream "a tree to build")

# Writes down where and how it is asked to build, and leaves a program
# where Cargo would.
cat > "$work/bin/cargo" <<'EOF'
#!/usr/bin/env bash
echo "$PWD|$*" >> "$CARGO_LOG"
while [[ $# -gt 0 ]]; do
    [[ "$1" != --target-dir ]] || folder=$2
    shift
done
mkdir -p "$folder/release"
echo program > "$folder/release/OpenCADStudio"
echo program > "$folder/release/OpenCADStudio.exe"
EOF
printf '#!/usr/bin/env bash\necho "host: x86_64-unknown-linux-gnu"\n' > "$work/bin/rustc"
chmod +x "$work/bin/cargo" "$work/bin/rustc"

# build_cad ARGUMENTS... runs the script on the repository above.
build_cad() {
    : > "$work/cargo.log"
    PATH="$work/bin:$PATH" CARGO_LOG="$work/cargo.log" OCS_PIN_FILE="$work/ocs.pin" \
        OCS_FETCH_FROM="$upstream" OCS_TARGET_DIR="$work/ocs-target" \
        bash "$packaging_dir/build-open-cad-studio.sh" "$@"
}
source_dir=$work/ocs-target/source

pin_upstream "$good"
if output=$(build_cad -j 1 2> "$work/build.log"); then
    if [[ "$output" == */ocs-target/release/OpenCADStudio* && -f "$output" ]] \
        && [[ "$(git -C "$source_dir" rev-parse HEAD)" == "$good" ]]; then
        passed "build-open-cad-studio.sh fetches the pinned commit and prints the program"
    else
        wrong "build-open-cad-studio.sh printed '$output' and left $(git -C "$source_dir" rev-parse HEAD 2>&1)"
    fi
    call=$(cat "$work/cargo.log")
    if [[ "$call" == "$(cd "$source_dir" && pwd)|build --release --locked --bin OpenCADStudio --target-dir "*"/ocs-target -j 1" ]]; then
        passed "Cargo builds the pinned source with --locked into the target folder of Open CAD Studio"
    else
        wrong "Cargo was not asked to build the pinned source with --locked: $call"
    fi
else
    wrong "build-open-cad-studio.sh failed on a pinned commit:"
    sed 's/^/        /' "$work/build.log"
fi

# What is changed in the checkout by hand is not built.
echo "fn changed() {}" >> "$source_dir/src/main.rs"
echo "extra" > "$source_dir/src/extra.rs"
if build_cad > /dev/null 2> "$work/build.log" \
    && [[ -z "$(git -C "$source_dir" status --porcelain)" && ! -e "$source_dir/src/extra.rs" ]] \
    && ! grep -q '^fetching' "$work/build.log"; then
    passed "a checkout at the pinned commit is restored, not fetched again"
else
    wrong "a checkout with changes is built as it is, or fetched again:"
    sed 's/^/        /' "$work/build.log"
fi

# refused_build WHAT PATTERN expects the build to fail before Cargo runs, with
# PATTERN in its message.
refused_build() {
    if build_cad > /dev/null 2> "$work/build.log"; then
        wrong "build-open-cad-studio.sh builds $1"
    elif [[ -s "$work/cargo.log" ]] || ! grep -q "$2" "$work/build.log"; then
        wrong "build-open-cad-studio.sh does not refuse $1 before Cargo runs with '$2':"
        sed 's/^/        /' "$work/build.log"
    else
        passed "build-open-cad-studio.sh refuses $1"
    fi
}

pin_upstream "$good" "$good_hash"
refused_build "a commit whose tree is not the pinned tree" "names the tree $good_hash"
pin_upstream "$good" "" 2026-10-02
refused_build "a commit of another date than the pinned one" "names 2026-10-02"

sed -i.orig 's/#1111111111111111111111111111111111111111//' "$upstream/Cargo.lock"
rm -f "$upstream/Cargo.lock.orig"
pin_upstream "$(commit_upstream "a git dependency without a commit")"
refused_build "a Cargo.lock with a git dependency without a commit" "names no commit"
git -C "$upstream" checkout -q "$good" -- Cargo.lock

sed -i.orig 's/#abc1234000/#fff1234000/' "$upstream/Cargo.lock"
rm -f "$upstream/Cargo.lock.orig"
pin_upstream "$(commit_upstream "a shortened hash that resolves to another commit")"
refused_build "a shortened hash that resolves to another commit" "resolves rev abc1234"
git -C "$upstream" checkout -q "$good" -- Cargo.lock

echo "Another licence" > "$upstream/LICENSE"
pin_upstream "$(commit_upstream "another licence")"
refused_build "a commit with another licence than GPL-3.0" "is not the GPL-3.0 text"
git -C "$upstream" checkout -q "$good" -- LICENSE

pin_upstream "$good"
if output=$(build_cad --fetch 2> "$work/build.log") && [[ "$output" == "$source_dir" && ! -s "$work/cargo.log" ]]; then
    passed "build-open-cad-studio.sh --fetch checks the source and builds nothing"
else
    wrong "build-open-cad-studio.sh --fetch printed '$output':"
    sed 's/^/        /' "$work/build.log"
fi

# ---- Open CAD Studio in the packages --------------------------------------

# Stand-ins for the two programs: the application, and an Open CAD Studio
# that reports a version and converts a drawing by copying it.
mkdir -p "$work/built"
printf '#!/usr/bin/env bash\necho application\n' > "$work/built/$BINARY_NAME"
cat > "$work/built/$CAD_BINARY_NAME" <<EOF
#!/usr/bin/env bash
case \$1 in
--version) echo "$CAD_BINARY_NAME 2026.40" ;;
--export) cp "\$2" "\$3" ;;
*) exit 1 ;;
esac
EOF
chmod +x "$work/built/$BINARY_NAME" "$work/built/$CAD_BINARY_NAME"
pinned=$(bash "$packaging_dir/build-open-cad-studio.sh" --pin)
pinned_commit=$(sed -n 's/^commit=//p' <<< "$pinned")
pinned_archive=$(sed -n 's/^archive=//p' <<< "$pinned")

# The archive of Linux and macOS is packed with tar, which every system has.
if bash "$packaging_dir/build-archive.sh" "$work/built/$BINARY_NAME" "$work/built/$CAD_BINARY_NAME" \
    9.9.9 linux-amd64 "$work/packages" > "$work/archive.log" 2>&1; then
    package=$work/packages/${BINARY_NAME}_9.9.9_linux-amd64
    listing=$(tar -tvzf "$package.tar.gz")
    if grep -qE "^-rwxr-xr-x .* ${BINARY_NAME}_9.9.9_linux-amd64/$CAD_BINARY_NAME\$" <<< "$listing" \
        && grep -qF "/$CAD_BINARY_NAME-LICENSE.txt" <<< "$listing" \
        && grep -qF "/$CAD_BINARY_NAME-NOTICE.txt" <<< "$listing"; then
        passed "the archive carries Open CAD Studio, executable, with its licence and notice"
    else
        wrong "the archive lacks Open CAD Studio, its executable bit, its licence or its notice:"
        sed 's/^/        /' <<< "$listing"
    fi
    notice=$package/$CAD_BINARY_NAME-NOTICE.txt
    if grep -qF "$pinned_commit" "$notice" && grep -qF "$pinned_archive" "$notice" \
        && grep -qF "releases/tag/v9.9.9" "$notice"; then
        passed "the notice of Open CAD Studio names the pinned commit and where its source is"
    else
        wrong "the notice of Open CAD Studio does not name $pinned_commit, $pinned_archive and the release page:"
        sed 's/^/        /' "$notice"
    fi
    if cmp -s "$package/$CAD_BINARY_NAME-LICENSE.txt" "$native_dir/desktop/LICENSE-GPL-3.0"; then
        passed "the licence of Open CAD Studio is the GPL-3.0 text"
    else
        wrong "the licence of Open CAD Studio in the archive is not the GPL-3.0 text"
    fi
    if output=$(bash "$packaging_dir/check-open-cad-studio.sh" "$package/$BINARY_NAME" 2>&1); then
        passed "check-open-cad-studio.sh accepts Open CAD Studio beside the application"
    else
        wrong "check-open-cad-studio.sh refuses Open CAD Studio beside the application:"
        sed 's/^/        /' <<< "$output"
    fi
else
    wrong "build-archive.sh failed:"
    sed 's/^/        /' "$work/archive.log"
fi

if bash "$packaging_dir/build-archive.sh" "$work/built/$BINARY_NAME" "$work/built/missing" \
    9.9.9 linux-amd64 "$work/packages" > /dev/null 2>&1; then
    wrong "build-archive.sh packs an archive without Open CAD Studio"
else
    passed "build-archive.sh refuses to pack an archive without Open CAD Studio"
fi

# The .deb and the AppImage keep it out of the search path, where the
# application looks for it too.
mkdir -p "$work/icons/hicolor/scalable/apps"
for size in 32 48 64 128 256 512; do
    mkdir -p "$work/icons/hicolor/${size}x${size}/apps"
    : > "$work/icons/hicolor/${size}x${size}/apps/$APP_ID.png"
done
: > "$work/icons/hicolor/scalable/apps/$APP_ID.svg"
if bash "$packaging_dir/linux/stage-tree.sh" "$work/root" "$work/built/$BINARY_NAME" \
    "$work/built/$CAD_BINARY_NAME" "$work/icons" 9.9.9 2026-10-05 > "$work/stage.log" 2>&1 \
    && [[ -x "$work/root/usr/lib/$BINARY_NAME/$CAD_BINARY_NAME" ]] \
    && grep -qF "$pinned_commit" "$work/root/usr/share/doc/$BINARY_NAME/$CAD_BINARY_NAME-NOTICE.txt"; then
    passed "stage-tree.sh puts Open CAD Studio in usr/lib/$BINARY_NAME with its notice"
    if output=$(bash "$packaging_dir/check-open-cad-studio.sh" "$work/root/usr/bin/$BINARY_NAME" 2>&1); then
        passed "check-open-cad-studio.sh accepts Open CAD Studio in ../lib/$BINARY_NAME"
    else
        wrong "check-open-cad-studio.sh refuses Open CAD Studio in ../lib/$BINARY_NAME:"
        sed 's/^/        /' <<< "$output"
    fi
else
    wrong "stage-tree.sh does not put Open CAD Studio in usr/lib/$BINARY_NAME with its notice:"
    sed 's/^/        /' "$work/stage.log"
fi

# A program that does not answer as Open CAD Studio does is refused.
mkdir -p "$work/other"
cp "$work/built/$BINARY_NAME" "$work/other/"
printf '#!/usr/bin/env bash\necho something else\n' > "$work/other/$CAD_BINARY_NAME"
chmod +x "$work/other/$CAD_BINARY_NAME"
if bash "$packaging_dir/check-open-cad-studio.sh" "$work/other/$BINARY_NAME" > /dev/null 2>&1; then
    wrong "check-open-cad-studio.sh accepts a program that is not Open CAD Studio"
else
    passed "check-open-cad-studio.sh refuses a program that is not Open CAD Studio"
fi

# ---- archive-open-cad-studio-source.sh -----------------------------------

# The source goes on the release page whole, also what the .gitattributes of
# the commit would leave out of an archive.
printf 'src/main.rs export-ignore\n' > "$upstream/.gitattributes"
kept=$(commit_upstream "a file that git archive would leave out")
short=${kept:0:8}
pin_upstream "$kept"
if output=$(PATH="$work/bin:$PATH" OCS_PIN_FILE="$work/ocs.pin" OCS_FETCH_FROM="$upstream" \
    OCS_TARGET_DIR="$work/ocs-target" \
    bash "$packaging_dir/archive-open-cad-studio-source.sh" "$work/source" 2> "$work/source.log"); then
    expected=$(git -C "$upstream" ls-tree -r --name-only "$kept" | sed "s|^|open-cad-studio-$short/|" | sort)
    archived=$(tar -tzf "$output" | grep -v '/$' | sort)
    if [[ "$output" == "$work/source/open-cad-studio-source_$short.tar.gz" && "$archived" == "$expected" ]]; then
        passed "the source archive holds every file of the pinned commit"
    else
        wrong "the source archive $output does not hold the files of the pinned commit:"
        diff <(echo "$expected") <(echo "$archived") | sed 's/^/        /'
    fi
    if (cd "$work/source" && if command -v sha256sum >/dev/null 2>&1; then sha256sum -c --quiet ./*.sha256; else shasum -a 256 -c --quiet ./*.sha256; fi) > /dev/null 2>&1; then
        passed "the source archive has a .sha256 beside it"
    else
        wrong "the source archive has no fitting .sha256 beside it"
    fi
else
    wrong "archive-open-cad-studio-source.sh failed:"
    sed 's/^/        /' "$work/source.log"
fi

# The release has it as its last file, under a name that the download
# buttons of the website, which look for the endings of the packages, never
# offer.
source_name=$(bash "$packaging_dir/expected-assets.sh" 9.9.9 | tail -n 1)
if [[ "$source_name" == "$pinned_archive" && "$source_name" == open-cad-studio-source_*.tar.gz \
    && "$source_name" != *linux* && "$source_name" != *macos* && "$source_name" != *windows* ]]; then
    passed "the release carries the source of Open CAD Studio as $source_name"
else
    wrong "the release does not carry the source of Open CAD Studio as $pinned_archive, but '$source_name'"
fi

if [[ "$failures" -ne 0 ]]; then
    fail "$failures of the tests failed"
fi
echo "all tests of the package scripts passed"
