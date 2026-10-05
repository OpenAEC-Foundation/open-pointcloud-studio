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

if [[ "$failures" -ne 0 ]]; then
    fail "$failures of the tests failed"
fi
echo "all tests of the package scripts passed"
