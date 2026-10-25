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
# Asked for fields, it answers with GH_IS_DRAFT and the names in GH_ASSETS.
# Asked for the list of releases, it prints the tags in GH_RELEASES, or
# fails when GH_FAIL is set.
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
echo "$*" >> "$GH_LOG"
if [[ "$1 $2" == "release view" ]]; then
    case "$*" in
        *"--json isDraft"*) echo "${GH_IS_DRAFT:-true}" ;;
        *"--json assets"*) printf '%s\n' ${GH_ASSETS:-} ;;
        *) [[ "$GH_HAS_DRAFT" == "true" ]] ;;
    esac
elif [[ "$1" == api ]]; then
    [[ -z "${GH_FAIL:-}" ]] || exit 1
    printf '%s\n' ${GH_RELEASES:-}
fi
EOF
chmod +x "$work/bin/gh"
: > "$work/notes.md"

# draft_call HAS_DRAFT prints the one call that writes the release.
draft_call() {
    : > "$work/gh.log"
    PATH="$work/bin:$PATH" GH_LOG="$work/gh.log" GH_HAS_DRAFT=$1 \
        GITHUB_REPOSITORY=example/repository GITHUB_SHA=0123abc \
        bash "$packaging_dir/draft-release.sh" v9.9.9 "$work/notes.md" >/dev/null || true
    grep -v '^release view ' "$work/gh.log" || true
}

call=$(draft_call false)
if [[ "$call" == "release create v9.9.9 "*"--target 0123abc"* && "$call" == *--draft* && "$call" != *--prerelease* ]]; then
    passed "a new draft is created at the commit of the run"
else
    wrong "a new draft is not created at the commit of the run: gh $call"
fi

# Publishing a draft puts the tag where the draft points, so a draft from an
# earlier run has to be moved to the commit these packages were built from.
call=$(draft_call true)
if [[ "$call" == "release edit v9.9.9 "*"--target 0123abc"* ]]; then
    passed "a draft from an earlier run is moved to the commit of the run"
else
    wrong "a draft from an earlier run keeps its old commit: gh $call"
fi

# ---- prune-release-assets.sh ---------------------------------------------

# A draft from an earlier run holds the files of this release and the source
# archives of Open CAD Studio for a commit pinned before; only those go.
# prune_calls IS_DRAFT prints the files it was asked to remove.
prune_calls() {
    : > "$work/gh.log"
    PATH="$work/bin:$PATH" GH_LOG="$work/gh.log" GH_IS_DRAFT=$1 GITHUB_REPOSITORY=example/repository \
        GH_ASSETS="$(bash "$packaging_dir/expected-assets.sh" 9.9.9 | sed 'p; s/$/.sha256/')
open-cad-studio-source_00000000.tar.gz open-cad-studio-source_00000000.tar.gz.sha256" \
        bash "$packaging_dir/prune-release-assets.sh" v9.9.9 9.9.9 > /dev/null 2>&1 || true
    sed -n 's/^release delete-asset v9.9.9 \([^ ]*\) .*--yes$/\1/p' "$work/gh.log"
}
removed=$(prune_calls true)
if [[ "$removed" == "open-cad-studio-source_00000000.tar.gz"$'\n'"open-cad-studio-source_00000000.tar.gz.sha256" ]]; then
    passed "files that an earlier run left on the draft and this release does not hold are removed"
else
    wrong "prune-release-assets.sh removed '$(echo $removed)' instead of the source archives of an older commit"
fi
removed=$(prune_calls false)
if [[ -z "$removed" ]]; then
    passed "nothing is removed from a published release"
else
    wrong "prune-release-assets.sh removed '$(echo $removed)' from a published release"
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

signed_notes=$(WINDOWS_SIGNED=true MACOS_SIGNED=true bash "$packaging_dir/release-notes.sh" "$number")
if grep -qF 'Developer ID signed, notarized by Apple and stapled' <<< "$signed_notes" \
    && ! grep -qF 'The application is not signed with a paid developer certificate' <<< "$signed_notes" \
    && grep -qF 'notarized by Apple' <<< "$signed_notes"; then
    passed "the release notes describe notarized macOS packages only when the build reports them"
else
    wrong "the release notes misreport Developer ID signing"
fi

# The image of the cloud API: a tag run pushes it before the notes and gives
# its digest, a draft run has none and says it comes with the release.
digest=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
image_notes=$(API_IMAGE_DIGEST=$digest bash "$packaging_dir/release-notes.sh" "$number")
verify_image="gh attestation verify oci://ghcr.io/openaec-foundation/open-pointcloud-studio-api@$digest --repo OpenAEC-Foundation/open-pointcloud-studio"
if [[ "$(grep -cF -- "$verify_image" <<< "$image_notes")" -eq 2 ]] \
    && grep -qF "ghcr.io/openaec-foundation/open-pointcloud-studio-api:$number" <<< "$image_notes" \
    && ! grep -qF 'published with the release' <<< "$image_notes"; then
    passed "the notes of a tag run give the digest of the image and how to verify its attestation, in both languages"
else
    wrong "the notes of a tag run do not give the digest of the image and its attestation command in both languages"
fi
if grep -qF 'It is published with the release' <<< "$notes" && grep -qF 'met de release gepubliceerd' <<< "$notes" \
    && ! grep -qF 'oci://' <<< "$notes"; then
    passed "the notes of a draft run say that the image is published with the release"
else
    wrong "the notes of a draft run name a digest or do not say that the image comes with the release"
fi
if ! API_IMAGE_DIGEST=sha256:123 bash "$packaging_dir/release-notes.sh" "$number" > /dev/null 2>&1; then
    passed "release-notes.sh refuses a digest that is not sha256 and 64 hex digits"
else
    wrong "release-notes.sh writes notes with a malformed digest"
fi

# SHA256SUMS is always there; the OpenPGP signatures only with the key.
if [[ "$(grep -cF 'sha256sum --ignore-missing -c SHA256SUMS' <<< "$notes")" -eq 2 ]] \
    && ! grep -qF 'gpg --verify' <<< "$notes"; then
    passed "the notes say how to check SHA256SUMS in both languages, and name no signature without the key"
else
    wrong "the notes do not say how to check SHA256SUMS in both languages, or name a signature without the key"
fi
fingerprint=0123456789ABCDEF0123456789ABCDEF01234567
gpg_notes=$(LINUX_GPG_SIGNED=true LINUX_GPG_FINGERPRINT="0123 4567 89ab cdef 0123 4567 89ab cdef 0123 4567" \
    bash "$packaging_dir/release-notes.sh" "$number")
if [[ "$(grep -cF 'gpg --verify SHA256SUMS.asc SHA256SUMS' <<< "$gpg_notes")" -eq 2 ]] \
    && [[ "$(grep -cF "gpg --keyserver hkps://keys.openpgp.org --recv-keys $fingerprint" <<< "$gpg_notes")" -eq 2 ]] \
    && grep -qF "gpg --verify FILE.AppImage.asc FILE.AppImage" <<< "$gpg_notes"; then
    passed "the notes of a signed release give the fingerprint and how to verify the signatures, in both languages"
else
    wrong "the notes of a signed release do not give the fingerprint and the gpg commands in both languages"
fi
if ! LINUX_GPG_SIGNED=true bash "$packaging_dir/release-notes.sh" "$number" > /dev/null 2>&1; then
    passed "release-notes.sh refuses signed notes without the fingerprint of the key"
else
    wrong "release-notes.sh writes signed notes without a fingerprint"
fi

# The signatures of the C library, as sdk.yml reports them.
sdk_notes=$(SDK_WINDOWS_SIGNED=true SDK_MACOS_SIGNED=true bash "$packaging_dir/release-notes.sh" "$number")
if grep -qF 'The C library for Windows carries a valid code signature; the one for macOS is signed with a developer certificate and notarized.' <<< "$sdk_notes" \
    && grep -qF 'De C-bibliotheek voor Windows heeft een geldige code-handtekening; die voor macOS is met een ontwikkelaarscertificaat' <<< "$sdk_notes" \
    && grep -qF 'The C library for Windows is not code-signed in this release; the one for macOS is signed ad hoc and not notarized.' <<< "$notes" \
    && grep -qF 'De C-bibliotheek voor Windows is in deze release niet ondertekend; die voor macOS is alleen ad hoc' <<< "$notes"; then
    passed "the notes say whether the C libraries are signed, as the build reports them"
else
    wrong "the notes misreport the signatures of the C libraries"
fi

# ---- signature-assets.sh and prune-release-assets.sh with signatures -----

if [[ "$(bash "$packaging_dir/signature-assets.sh" 9.9.9)" == SHA256SUMS ]] \
    && [[ "$(KEEP_SIGNATURES=true bash "$packaging_dir/signature-assets.sh" 9.9.9)" == "SHA256SUMS"$'\n'"SHA256SUMS.asc"$'\n'"${BINARY_NAME}_9.9.9_amd64.AppImage.asc"$'\n'"${BINARY_NAME}_9.9.9_arm64.AppImage.asc" ]]; then
    passed "signature-assets.sh lists SHA256SUMS always, and the OpenPGP signatures when they are kept"
else
    wrong "signature-assets.sh lists '$(echo $(bash "$packaging_dir/signature-assets.sh" 9.9.9))' and '$(echo $(KEEP_SIGNATURES=true bash "$packaging_dir/signature-assets.sh" 9.9.9))'"
fi

# A draft with the files of this release, SHA256SUMS and the signatures of
# an earlier run that signed: prune_signed KEEP prints what it removes.
prune_signed() {
    : > "$work/gh.log"
    PATH="$work/bin:$PATH" GH_LOG="$work/gh.log" GH_IS_DRAFT=true GITHUB_REPOSITORY=example/repository \
        KEEP_SIGNATURES=$1 \
        GH_ASSETS="$(bash "$packaging_dir/expected-assets.sh" 9.9.9 | sed 'p; s/$/.sha256/')
$(KEEP_SIGNATURES=true bash "$packaging_dir/signature-assets.sh" 9.9.9)" \
        bash "$packaging_dir/prune-release-assets.sh" v9.9.9 9.9.9 > /dev/null 2>&1 || true
    sed -n 's/^release delete-asset v9.9.9 \([^ ]*\) .*--yes$/\1/p' "$work/gh.log"
}
removed_kept=$(prune_signed true)
removed_unsigned=$(prune_signed false)
if [[ -z "$removed_kept" ]] \
    && [[ "$removed_unsigned" == "SHA256SUMS.asc"$'\n'"${BINARY_NAME}_9.9.9_amd64.AppImage.asc"$'\n'"${BINARY_NAME}_9.9.9_arm64.AppImage.asc" ]]; then
    passed "the draft keeps SHA256SUMS always and the signatures only while the release is signed"
else
    wrong "prune-release-assets.sh removed '$(echo $removed_kept)' from a signed draft and '$(echo $removed_unsigned)' from an unsigned one"
fi

# ---- sign-linux.sh -----------------------------------------------------------

signed_assets=$work/signed-assets
mkdir -p "$signed_assets"
echo "checksums" > "$signed_assets/SHA256SUMS"
for arch in amd64 arm64; do
    echo "an AppImage" > "$signed_assets/${BINARY_NAME}_9.9.9_$arch.AppImage"
done
signing_output="$work/gpg-output"
: > "$signing_output"
if output=$(env -u OPENAEC_GPG_PRIVATE_KEY -u OPENAEC_GPG_PASSPHRASE -u OPENAEC_GPG_FINGERPRINT \
        GITHUB_OUTPUT="$signing_output" bash "$packaging_dir/sign-linux.sh" "$signed_assets" 9.9.9 2>&1) \
    && grep -qxF 'gpg_signed=false' "$signing_output" && grep -qF 'not configured' <<< "$output" \
    && ! ls "$signed_assets"/*.asc > /dev/null 2>&1; then
    passed "sign-linux.sh without the key signs nothing, says so and reports gpg_signed=false"
else
    wrong "sign-linux.sh without the key does not end quietly with gpg_signed=false:"
    sed 's/^/        /' <<< "$output"
fi

# A throwaway key in a GnuPG home of its own, with a short path for the
# socket of the agent.
if command -v gpg > /dev/null 2>&1; then
    gnupg=$work/gnupg
    mkdir -p "$gnupg"
    chmod 700 "$gnupg"
    test_gpg() { gpg --homedir "$gnupg" --batch --no-tty --pinentry-mode loopback --passphrase test-passphrase "$@"; }
    if test_gpg --quick-generate-key "Test signer <test@example.org>" ed25519 sign never > /dev/null 2>&1; then
        test_fingerprint=$(test_gpg --with-colons --list-secret-keys 2> /dev/null | awk -F: '$1 == "fpr" { print $10; exit }')
        test_key=$(test_gpg --armor --export-secret-keys "$test_fingerprint" 2> /dev/null)
        : > "$signing_output"
        if output=$(OPENAEC_GPG_PRIVATE_KEY=$test_key OPENAEC_GPG_PASSPHRASE=test-passphrase \
                OPENAEC_GPG_FINGERPRINT=$test_fingerprint GITHUB_OUTPUT="$signing_output" \
                bash "$packaging_dir/sign-linux.sh" "$signed_assets" 9.9.9 2>&1) \
            && grep -qxF 'gpg_signed=true' "$signing_output" \
            && test_gpg --verify "$signed_assets/SHA256SUMS.asc" "$signed_assets/SHA256SUMS" > /dev/null 2>&1 \
            && test_gpg --verify "$signed_assets/${BINARY_NAME}_9.9.9_amd64.AppImage.asc" \
                "$signed_assets/${BINARY_NAME}_9.9.9_amd64.AppImage" > /dev/null 2>&1 \
            && test_gpg --verify "$signed_assets/${BINARY_NAME}_9.9.9_arm64.AppImage.asc" \
                "$signed_assets/${BINARY_NAME}_9.9.9_arm64.AppImage" > /dev/null 2>&1 \
            && grep -q 'BEGIN PGP SIGNATURE' "$signed_assets/SHA256SUMS.asc"; then
            passed "sign-linux.sh signs SHA256SUMS and both AppImages with the key, armoured and verified"
        else
            wrong "sign-linux.sh does not sign SHA256SUMS and the AppImages with a throwaway key:"
            sed 's/^/        /' <<< "$output"
        fi
        rm -f "$signed_assets"/*.asc
        if output=$(OPENAEC_GPG_PRIVATE_KEY=$test_key OPENAEC_GPG_PASSPHRASE=test-passphrase \
                OPENAEC_GPG_FINGERPRINT=0123456789ABCDEF0123456789ABCDEF01234567 GITHUB_OUTPUT="$signing_output" \
                bash "$packaging_dir/sign-linux.sh" "$signed_assets" 9.9.9 2>&1); then
            wrong "sign-linux.sh signs with a key whose fingerprint is not the one asked for"
        elif grep -qF 'is not the one of OPENAEC_GPG_FINGERPRINT' <<< "$output" \
            && ! ls "$signed_assets"/*.asc > /dev/null 2>&1; then
            passed "sign-linux.sh refuses a key with another fingerprint and signs nothing"
        else
            wrong "sign-linux.sh refuses a key with another fingerprint for another reason, or signs:"
            sed 's/^/        /' <<< "$output"
        fi
    else
        echo "notice: gpg cannot make a key here; the signing tests of sign-linux.sh are skipped"
    fi
    gpgconf --homedir "$gnupg" --kill all > /dev/null 2>&1 || true
else
    echo "notice: gpg is not installed; the signing tests of sign-linux.sh are skipped"
fi

signing_output="$work/macos-signing-output"
if GITHUB_OUTPUT="$signing_output" bash "$packaging_dir/macos/prepare-signing.sh" > /dev/null \
    && grep -qFx 'macos_signed=false' "$signing_output"; then
    passed "the macOS job reports ad hoc signing without Apple credentials"
else
    wrong "the macOS job does not report its ad hoc signing status"
fi
if APPLE_SIGNING_IDENTITY='Developer ID Application: test' \
    GITHUB_OUTPUT="$signing_output" \
    bash "$packaging_dir/macos/prepare-signing.sh" > /dev/null 2>&1; then
    wrong "the macOS job accepted incomplete Apple credentials"
else
    passed "the macOS job refuses a partial Apple credential set"
fi

# ---- set-version.sh and version.sh ---------------------------------------

# A repository of its own with these scripts, a workspace like that of
# native/ with the line ends of a checkout on Windows, and an origin to push
# tags to. The member ui has a version of its own, and las is a crate from
# crates.io that has the version of the workspace by chance.
version_repo=$work/version-repo
mkdir -p "$version_repo/native/core" "$version_repo/native/desktop" "$version_repo/native/ui"
cp -R "$packaging_dir" "$version_repo/native/packaging"
crlf() { printf '%s\r\n' "$@"; }
# workspace_files NUMBER writes native/Cargo.toml with NUMBER, and
# lock_file NUMBER native/Cargo.lock with NUMBER for core and desktop.
workspace_files() {
    crlf '[workspace]' 'members = ["core", "desktop", "ui"]' 'resolver = "2"' '' \
        '[workspace.package]' "version = \"$1\"" 'edition = "2021"' '' \
        '[workspace.metadata.example]' 'version = "1.0.0"'
}
lock_file() {
    crlf '# This file is automatically @generated by Cargo.' 'version = 4' '' \
        '[[package]]' 'name = "las"' 'version = "0.9.5"' \
        'source = "registry+https://github.com/rust-lang/crates.io-index"' '' \
        '[[package]]' 'name = "open-pointcloud-studio-native"' "version = \"$1\"" \
        'dependencies = [' ' "las",' ' "pointcloud-core",' ']' '' \
        '[[package]]' 'name = "openaec-ui"' 'version = "0.1.0"' '' \
        '[[package]]' 'name = "pointcloud-core"' "version = \"$1\""
}
workspace_files 0.9.5 > "$version_repo/native/Cargo.toml"
lock_file 0.9.5 > "$version_repo/native/Cargo.lock"
crlf '[package]' 'name = "pointcloud-core"' 'version.workspace = true' > "$version_repo/native/core/Cargo.toml"
crlf '[package]' 'name = "open-pointcloud-studio-native"' 'version.workspace = true' '' \
    '[dependencies]' 'pointcloud-core = { path = "../core" }' > "$version_repo/native/desktop/Cargo.toml"
crlf '[package]' 'name = "openaec-ui"' 'version = "0.1.0"' > "$version_repo/native/ui/Cargo.toml"
crlf '# Changelog' '' '<!-- A section starts with "## YYYY.W.N - YYYY-MM-DD". -->' '' \
    '## Unreleased' '' '- Something changed.' '' '## 0.9.5 - 2026-10-13' '' '- Something before.' \
    > "$version_repo/CHANGELOG.md"
git -C "$version_repo" init -q
git -C "$version_repo" config core.autocrlf false
git -C "$version_repo" add -A
git -C "$version_repo" -c user.name=test -c user.email=test@example.org commit -q -m "a workspace"
git init -q --bare "$work/version-origin.git"
git -C "$version_repo" remote add origin "$work/version-origin.git"

# versions ARGUMENTS... runs set-version.sh of that repository; GH_RELEASES
# holds the releases on GitHub, drafts included.
versions() {
    env PATH="$work/bin:$PATH" GH_LOG="$work/gh.log" GITHUB_REPOSITORY=example/repository \
        GH_RELEASES="${GH_RELEASES:-}" GH_FAIL="${GH_FAIL:-}" \
        bash "$version_repo/native/packaging/set-version.sh" "$@"
}
# expect_next WHAT ANSWER ARGUMENTS... expects next to print ANSWER.
expect_next() {
    local what=$1 answer=$2 output
    shift 2
    if output=$(versions next "$@" 2>&1) && [[ "$output" == "$answer" ]]; then
        passed "set-version.sh next gives $answer $what"
    else
        wrong "set-version.sh next gives '$output' instead of $answer $what"
    fi
}

expect_next "on a Saturday of week 43 without tags" 2026.43.0 --date 2026-10-24
# The week-year of the ISO week, not the year of the calendar: the first days
# of a year can be in the last week of the year before, and the last days in
# the first week of the next.
expect_next "on 1 January 2027, in week 53 of 2026" 2026.53.0 --date 2027-01-01 --offline
expect_next "on 29 December 2025, in week 1 of 2026" 2026.1.0 --date 2025-12-29 --offline
expect_next "on 4 January 2027, in week 1 of 2027" 2027.1.0 --date 2027-01-04 --offline
expect_next "on 5 January 2026, without a leading zero" 2026.2.0 --date 2026-01-05 --offline
expect_next "for today in UTC" "$(date -u +%G).$((10#$(date -u +%V))).0" --offline

# The highest N of the week counts, wherever it is, and a gap is not filled:
# a tag here, an annotated tag only on origin, a draft only on GitHub.
git -C "$version_repo" tag v2026.43.0
git -C "$version_repo" tag v2026.4.7
git -C "$version_repo" tag v0.9.5
git -C "$version_repo" -c user.name=test -c user.email=test@example.org -c tag.gpgSign=false \
    tag -a -m "a release" v2026.43.2
git -C "$version_repo" push -q origin v2026.43.2
git -C "$version_repo" tag -d v2026.43.2 > /dev/null
expect_next "with v2026.43.0 here and v2026.43.2 on origin" 2026.43.3 --date 2026-10-24
expect_next "with --offline, which counts v2026.43.0 here only" 2026.43.1 --date 2026-10-24 --offline
expect_next "in week 4, with v2026.4.7 here" 2026.4.8 --date 2026-01-19 --offline
GH_RELEASES="v2026.44.0 v0.9.5" expect_next "with a draft v2026.44.0" 2026.44.1 --date 2026-10-26
if GH_FAIL=1 versions next --date 2026-10-24 > /dev/null 2>&1; then
    wrong "set-version.sh next gives a number when the releases on GitHub cannot be listed"
else
    passed "set-version.sh next refuses to guess when the releases on GitHub cannot be listed"
fi
if versions next --date 2026-02-30 --offline > /dev/null 2>&1; then
    wrong "set-version.sh next takes 2026-02-30 for a date"
else
    passed "set-version.sh next refuses a day that does not exist"
fi

# Until the first release numbered by week, the workspace has 0.9.5, which
# check accepts as long as no version is asked for.
if output=$(versions check 2>&1) && grep -qxF 'native/Cargo.lock pointcloud-core: 0.9.5' <<< "$output" \
    && output=$(bash "$version_repo/native/packaging/version.sh" 2> /dev/null) \
    && grep -qxF 'number=0.9.5' <<< "$output"; then
    passed "check and version.sh accept 0.9.5, the last version numbered the old way"
else
    wrong "check or version.sh refuses the workspace on 0.9.5:"
    sed 's/^/        /' <<< "$output"
fi
accepted=
for number in 0.9.6 2026.03.0 2026.0.0 2026.54.0 2026.43.100 2026.43.01 0999.1.0 2026.43.0-rc1 2026.43; do
    workspace_files "$number" > "$version_repo/native/Cargo.toml"
    lock_file "$number" > "$version_repo/native/Cargo.lock"
    if versions check > /dev/null 2>&1; then
        accepted="$accepted $number"
    fi
done
for number in 0.9.5 v0.9.6 2026.03.0 v2026.43.0-rc1; do
    output=$(versions check "$number" 2>&1) || true
    grep -qF "'$number' is not YYYY.W.N" <<< "$output" || accepted="$accepted (asked for) $number"
done
for number in 2026.1.0 2026.53.99; do
    workspace_files "$number" > "$version_repo/native/Cargo.toml"
    lock_file "$number" > "$version_repo/native/Cargo.lock"
    versions check "v$number" > /dev/null 2>&1 || accepted="$accepted (refused) $number"
done
if [[ -z "$accepted" ]]; then
    passed "check accepts YYYY.W.N alone, with a week from 1 to 53 and N from 0 to 99"
else
    wrong "check does not hold versions to YYYY.W.N:$accepted"
fi
workspace_files 0.9.5 > "$version_repo/native/Cargo.toml"
lock_file 0.9.5 > "$version_repo/native/Cargo.lock"

# set writes nothing when it refuses.
snapshot() { cat "$version_repo/native/Cargo.toml" "$version_repo/native/Cargo.lock" "$version_repo/CHANGELOG.md" | cksum; }
before=$(snapshot)
refused=
versions set 2026.03.0 > /dev/null 2>&1 && refused="$refused 2026.03.0"
output=$(versions set 2026.43.0 2>&1) || true
grep -qF 'not above v2026.43.0' <<< "$output" || refused="$refused 2026.43.0"
versions set 2026.42.5 > /dev/null 2>&1 && refused="$refused 2026.42.5"
if [[ -z "$refused" && "$(snapshot)" == "$before" ]]; then
    passed "set-version.sh set refuses a version that is not YYYY.W.N or not above the highest tag, and writes nothing"
else
    wrong "set-version.sh set does not refuse:$refused, or writes the files"
fi

# set next writes the version that next gives into native/Cargo.toml and the
# entries of the members that take it in native/Cargo.lock, and names the
# section of the changelog; nothing else changes, line ends included.
if output=$(versions set next --date 2026-10-24 --changelog 2026-10-24 2>&1); then
    changes=
    cmp -s <(workspace_files 2026.43.3) "$version_repo/native/Cargo.toml" || changes="$changes native/Cargo.toml"
    cmp -s <(lock_file 2026.43.3) "$version_repo/native/Cargo.lock" || changes="$changes native/Cargo.lock"
    cmp -s <(crlf '# Changelog' '' '<!-- A section starts with "## YYYY.W.N - YYYY-MM-DD". -->' '' \
        '## Unreleased' '' '## 2026.43.3 - 2026-10-24' '' '- Something changed.' '' \
        '## 0.9.5 - 2026-10-13' '' '- Something before.') "$version_repo/CHANGELOG.md" \
        || changes="$changes CHANGELOG.md"
    if [[ -z "$changes" ]]; then
        passed "set-version.sh set next writes 2026.43.3 for the workspace, its two members and the changelog"
    else
        wrong "set-version.sh set next did not write as expected:$changes"
    fi
    if bash "$version_repo/native/packaging/release-notes.sh" --check 2026.43.3 > /dev/null 2>&1; then
        passed "the release notes are cut from the section that set-version.sh named"
    else
        wrong "release-notes.sh finds no section for 2026.43.3 after set-version.sh"
    fi
else
    wrong "set-version.sh set next failed:"
    sed 's/^/        /' <<< "$output"
fi
if output=$(bash "$version_repo/native/packaging/version.sh" v2026.43.3 2> /dev/null) \
    && grep -qxF 'number=2026.43.3' <<< "$output" && grep -qxF 'tag=v2026.43.3' <<< "$output" \
    && ! bash "$version_repo/native/packaging/version.sh" v2026.43.4 > /dev/null 2>&1; then
    passed "version.sh accepts the tag of the version set and refuses another"
else
    wrong "version.sh does not hold the tag to the version set:"
    sed 's/^/        /' <<< "$output"
fi

# set_refuses REASON ARGUMENTS... expects set to fail with REASON and to
# leave the files as they are.
set_refuses() {
    local reason=$1 before output
    shift
    before=$(snapshot)
    output=$(versions set "$@" 2>&1) && return 1
    grep -qF "$reason" <<< "$output" && [[ "$(snapshot)" == "$before" ]]
}

# A changelog whose section Unreleased says nothing stops set before it
# writes anything.
if set_refuses "has no '- ' item" 2026.43.4 --changelog 2026-10-25; then
    passed "set-version.sh set refuses a release whose changelog section is empty, and writes nothing"
else
    wrong "set-version.sh set writes a release whose changelog section is empty"
fi

# A lock file in which one member still has the old version fails the
# check, as cargo --locked would.
awk '/^name = "pointcloud-core"/ { core = 1 } core && /^version/ { sub(/2026\.43\.3/, "0.9.5"); core = 0 } { print }' \
    <(lock_file 2026.43.3) > "$version_repo/native/Cargo.lock"
if output=$(versions check 2>&1); then
    wrong "check accepts a Cargo.lock with pointcloud-core on 0.9.5"
elif grep -qF "version '0.9.5' for pointcloud-core" <<< "$output" \
    && ! bash "$version_repo/native/packaging/version.sh" > /dev/null 2>&1; then
    passed "check and version.sh refuse a Cargo.lock in which a member has another version"
else
    wrong "check refuses a Cargo.lock with pointcloud-core on 0.9.5 for another reason:"
    sed 's/^/        /' <<< "$output"
fi

# Nor is a changelog without the section Unreleased, or with a section for
# the version already, given a heading.
workspace_files 2026.43.3 > "$version_repo/native/Cargo.toml"
lock_file 2026.43.3 > "$version_repo/native/Cargo.lock"
refused=
crlf '# Changelog' '' '## 2026.43.3 - 2026-10-24' '' '- Something changed.' > "$version_repo/CHANGELOG.md"
set_refuses "has no section '## Unreleased'" 2026.43.4 --changelog 2026-10-25 || refused="$refused (no Unreleased)"
crlf '# Changelog' '' '## Unreleased' '' '- Something new.' '' '## 2026.43.4 - 2026-10-20' '' '- Something changed.' \
    > "$version_repo/CHANGELOG.md"
set_refuses "has a section for 2026.43.4 already" 2026.43.4 --changelog 2026-10-25 || refused="$refused (section taken)"
if [[ -z "$refused" ]]; then
    passed "set-version.sh set refuses a changelog without Unreleased or with the section already, and writes nothing"
else
    wrong "set-version.sh set does not refuse the changelog:$refused"
fi

# With the line ends of a checkout on Linux or macOS, the files keep theirs.
workspace_files 2026.43.3 | tr -d '\r' > "$version_repo/native/Cargo.toml"
lock_file 2026.43.3 | tr -d '\r' > "$version_repo/native/Cargo.lock"
printf '%s\n' '# Changelog' '' '## Unreleased' '' '- Something new.' '' '## 2026.43.3 - 2026-10-24' \
    > "$version_repo/CHANGELOG.md"
changes=
if output=$(versions set 2026.43.4 --changelog 2026-10-25 2>&1); then
    cmp -s <(workspace_files 2026.43.4 | tr -d '\r') "$version_repo/native/Cargo.toml" || changes="$changes native/Cargo.toml"
    cmp -s <(lock_file 2026.43.4 | tr -d '\r') "$version_repo/native/Cargo.lock" || changes="$changes native/Cargo.lock"
    cmp -s <(printf '%s\n' '# Changelog' '' '## Unreleased' '' '## 2026.43.4 - 2026-10-25' '' '- Something new.' '' \
        '## 2026.43.3 - 2026-10-24') "$version_repo/CHANGELOG.md" || changes="$changes CHANGELOG.md"
else
    changes=" (set failed: $output)"
fi
if [[ -z "$changes" ]]; then
    passed "set-version.sh set writes files with LF line ends as it does those with CRLF"
else
    wrong "set-version.sh set did not write the files with LF line ends as expected:$changes"
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
# where Cargo would. For the source archives it also vendors, lists and
# resolves the packages of the Cargo.lock in its folder as Cargo does, with
# the licences that CARGO_LICENCES gives as NAME|LICENCE lines.
cat > "$work/bin/cargo" <<'EOF'
#!/usr/bin/env bash
echo "$PWD|$*" >> "${CARGO_LOG:-/dev/null}"
# NAME VERSION SOURCE for each package of Cargo.lock; SOURCE is - for its own.
packages() {
    tr -d '\r"' < Cargo.lock | awk '
        /^\[\[package\]\]/ { if (name != "") print name, version, (source == "" ? "-" : source); name = version = source = "" }
        /^name = / { name = $3 }
        /^version = / { version = $3 }
        /^source = / { source = $3 }
        END { if (name != "") print name, version, (source == "" ? "-" : source) }'
}
licence() { sed -n "s/^$1|//p" "$CARGO_LICENCES"; }
last=${!#}
config=
previous=
for argument in "$@"; do
    [[ "$previous" != --config ]] || config=$argument
    previous=$argument
done
case $1 in
build)
    while [[ $# -gt 0 ]]; do
        [[ "$1" != --target-dir ]] || folder=$2
        shift
    done
    mkdir -p "$folder/release"
    echo program > "$folder/release/OpenCADStudio"
    echo program > "$folder/release/OpenCADStudio.exe"
    ;;
vendor)
    echo "To use vendored sources, add this to your .cargo/config.toml for this project:"
    echo
    echo '[source.crates-io]'
    echo 'replace-with = "vendored-sources"'
    packages | while read -r name version source; do
        [[ "$source" != - ]] || continue
        mkdir -p "$last/$name-$version"
        printf '[package]\nname = "%s"\nversion = "%s"\nlicense = "%s"\n' \
            "$name" "$version" "$(licence "$name")" > "$last/$name-$version/Cargo.toml"
        echo '{"files":{},"package":null}' > "$last/$name-$version/.cargo-checksum.json"
        echo "licence of $name" > "$last/$name-$version/LICENSE"
    done
    packages | awk '$3 ~ /^git\+/ { sub(/#.*/, "", $3); print $3 }' | sort -u | while read -r key; do
        key=$(sed 's|%2F|/|g' <<< "$key")
        url=${key#git+}
        query=${url#*\?}
        printf '\n[source."%s"]\ngit = "%s"\n%s = "%s"\nreplace-with = "vendored-sources"\n' \
            "$key" "${url%%\?*}" "${query%%=*}" "${query#*=}"
    done
    printf '\n[source.vendored-sources]\ndirectory = "%s"\n' "$last"
    ;;
tree)
    echo "upstream v0.1.0 ($PWD)|"
    packages | while read -r name version source; do
        case $source in
            git+*) source=${source#git+}; echo "$name v$version (${source%%#*}#$(cut -c1-8 <<< "${source##*#}"))|$(licence "$name")" ;;
            registry+*) echo "$name v$version|$(licence "$name")" ;;
        esac
    done
    # Cargo marks a crate it printed before with (*), in colour on a build
    # machine that asks for colours.
    packages | awk '$3 ~ /^git\+/ { print; exit }' | while read -r name version source; do
        source=${source#git+}
        printf '%s v%s (%s#%s)|%s \033[33m\033[2m(*)\033[39m\033[22m\n' "$name" "$version" \
            "${source%%#*}" "$(cut -c1-8 <<< "${source##*#}")" "$(licence "$name")"
    done
    ;;
metadata)
    # Resolves only when the folder that --config names holds every package
    # of a git repository.
    missing=$(packages | awk '$3 ~ /^git\+/ { print $1 "-" $2 }' | while read -r package; do
        [[ -f "$(dirname "$config")/$package/Cargo.toml" ]] || echo "$package"
    done)
    [[ -n "$config" && -z "$missing" ]] || { echo "error: no matching package $missing" >&2; exit 101; }
    echo '{}'
    ;;
esac
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

# --export writes the pinned commit alone into a bare repository, as the job
# that checks the pin hands it to the jobs that build; a checkout elsewhere
# fetches it from there, also by a relative path.
if build_cad --export "$work/exported.git" > /dev/null 2> "$work/build.log" \
    && [[ "$(git -C "$work/exported.git" rev-parse refs/heads/pinned)" == "$good" ]] \
    && (cd "$work" && PATH="$work/bin:$PATH" OCS_PIN_FILE="$work/ocs.pin" OCS_FETCH_FROM=exported.git \
        OCS_TARGET_DIR="$work/elsewhere" bash "$packaging_dir/build-open-cad-studio.sh" --fetch) \
        > /dev/null 2> "$work/build.log" \
    && [[ "$(git -C "$work/elsewhere/source" rev-parse HEAD)" == "$good" ]]; then
    passed "a checkout fetches the pinned commit from the repository that --export wrote"
else
    wrong "the pinned commit cannot be fetched from the repository that --export wrote:"
    sed 's/^/        /' "$work/build.log"
fi

# OCS_VENDOR_DIR puts crates vendored for the commit in place of their git
# repositories; a folder vendored for another commit is refused before Cargo
# runs.
mkdir -p "$work/vendored"
printf '# Git dependencies of Open CAD Studio commit %s\n' "$good" > "$work/vendored/config.toml"
export OCS_VENDOR_DIR=$work/vendored
if build_cad -j 1 > /dev/null 2> "$work/build.log" \
    && [[ "$(cat "$work/cargo.log")" == *"--config "*/vendored/config.toml" -j 1" ]]; then
    passed "build-open-cad-studio.sh builds with the crates vendored for the commit"
else
    wrong "build-open-cad-studio.sh does not build with the crates vendored for the commit: $(cat "$work/cargo.log")"
    sed 's/^/        /' "$work/build.log"
fi
printf '# Git dependencies of Open CAD Studio commit %s\n' "$good_hash" > "$work/vendored/config.toml"
refused_build "crates vendored for another commit" "no crates vendored for commit $good"
unset OCS_VENDOR_DIR

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
pinned_vendor=$(sed -n 's/^vendor=//p' <<< "$pinned")

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
        && grep -qF "$pinned_vendor" "$notice" && grep -qF "releases/tag/v9.9.9" "$notice"; then
        passed "the notice of Open CAD Studio names the pinned commit and where its source is"
    else
        wrong "the notice of Open CAD Studio does not name $pinned_commit, $pinned_archive, $pinned_vendor and the release page:"
        sed 's/^/        /' "$notice"
    fi
    if grep -qF "is in $CAD_BINARY_NAME-LICENSE.txt." "$notice"; then
        passed "the notice of Open CAD Studio in an archive points at the licence text beside it"
    else
        wrong "the notice of Open CAD Studio in an archive does not point at $CAD_BINARY_NAME-LICENSE.txt"
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

# For Windows, an Open CAD Studio that loads the C++ runtime of MinGW is
# refused before anything is packed; one built with MSVC is packed with 7z,
# here a stand-in that lists what the archive would hold.
cat > "$work/bin/7z" <<'EOF'
#!/usr/bin/env bash
find "$4" -type f | sort > "$3"
EOF
chmod +x "$work/bin/7z"
cp "$work/built/$CAD_BINARY_NAME" "$work/built/msvc.exe"
{ cat "$work/built/$CAD_BINARY_NAME"; echo "# loads libstdc++-6.dll"; } > "$work/built/gnu.exe"
chmod +x "$work/built/gnu.exe"
windows_package=$work/packages/${BINARY_NAME}_9.9.9_windows-x64
if PATH="$work/bin:$PATH" bash "$packaging_dir/build-archive.sh" "$work/built/$BINARY_NAME" "$work/built/gnu.exe" \
    9.9.9 windows-x64 "$work/packages" > "$work/archive.log" 2>&1; then
    wrong "build-archive.sh packs for Windows an Open CAD Studio that loads the C++ runtime of MinGW"
elif ! grep -qF 'libstdc++-6.dll of MinGW' "$work/archive.log" || [[ -e "$windows_package" ]]; then
    wrong "build-archive.sh refuses an Open CAD Studio that loads the C++ runtime of MinGW only after it began, or without saying why:"
    sed 's/^/        /' "$work/archive.log"
else
    passed "build-archive.sh refuses for Windows an Open CAD Studio that loads the C++ runtime of MinGW"
fi
if PATH="$work/bin:$PATH" bash "$packaging_dir/build-archive.sh" "$work/built/$BINARY_NAME" "$work/built/msvc.exe" \
    9.9.9 windows-x64 "$work/packages" > "$work/archive.log" 2>&1 \
    && grep -qF "/$CAD_BINARY_NAME.exe" "$windows_package.zip" \
    && grep -qF "/$CAD_BINARY_NAME-NOTICE.txt" "$windows_package.zip"; then
    passed "build-archive.sh packs for Windows an Open CAD Studio built with MSVC as $CAD_BINARY_NAME.exe"
else
    wrong "build-archive.sh does not pack for Windows an Open CAD Studio built with MSVC:"
    sed 's/^/        /' "$work/archive.log"
fi

# An installed or unpacked Windows package is refused for the same reason,
# also where the C++ runtime of MinGW is on the search path.
mkdir -p "$work/mingw"
cp "$work/built/$BINARY_NAME" "$work/mingw/"
cp "$work/built/gnu.exe" "$work/mingw/$CAD_BINARY_NAME.exe"
if output=$(bash "$packaging_dir/check-open-cad-studio.sh" "$work/mingw/$BINARY_NAME" 2>&1); then
    wrong "check-open-cad-studio.sh accepts an Open CAD Studio that loads the C++ runtime of MinGW"
elif grep -qF 'libstdc++-6.dll of MinGW' <<< "$output"; then
    passed "check-open-cad-studio.sh refuses an Open CAD Studio that loads the C++ runtime of MinGW"
else
    wrong "check-open-cad-studio.sh refuses an Open CAD Studio that loads the C++ runtime of MinGW for another reason:"
    sed 's/^/        /' <<< "$output"
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
    # The .deb carries no OpenCADStudio-LICENSE.txt, so its notice must not
    # point there.
    notice=$work/root/usr/share/doc/$BINARY_NAME/$CAD_BINARY_NAME-NOTICE.txt
    if grep -qF "is in /usr/share/common-licenses/GPL-3." "$notice" \
        && ! grep -qF "$CAD_BINARY_NAME-LICENSE.txt" "$notice"; then
        passed "the notice of Open CAD Studio in the .deb points at the GPL text of the system"
    else
        wrong "the notice of Open CAD Studio in the .deb points at a licence text the package does not carry:"
        sed 's/^/        /' "$notice"
    fi
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

# The commit to publish has, besides the two crates from git repositories,
# two from crates.io: one under permissive licences and one under a copyleft
# licence only, which the notices have to name as well. Its .gitattributes
# would leave a file out of an archive.
cat >> "$upstream/Cargo.lock" <<'LOCK'

[[package]]
name = "plain"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "shared"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
LOCK
printf 'src/main.rs export-ignore\n' > "$upstream/.gitattributes"
kept=$(commit_upstream "a file that git archive would leave out")
short=${kept:0:8}
pin_upstream "$kept"
printf '%s\n' 'followed|MIT' 'shortened|MPL-2.0' 'plain|MIT OR Apache-2.0' 'shared|LGPL-2.1-or-later' \
    > "$work/licences"
# Notices that name what the Cargo.lock above holds.
cat > "$work/cad-notice.txt" <<'NOTICE'
    https://example.org/followed
    commit 1111111111111111111111111111111111111111
        followed 0.1.0 (MIT)

    https://example.org/short
    commit abc1234000000000000000000000000000000000
        shortened 0.1.0 (MPL-2.0)

        shared 2.0.0 (LGPL-2.1-or-later)
NOTICE
echo "It links shortened and shared." > "$work/notice.txt"

# archive_source [VARIABLE=VALUE...] runs the script on the commit above,
# with the notices above unless the variables say otherwise.
archive_source() {
    : > "$work/cargo.log"
    env PATH="$work/bin:$PATH" CARGO_LOG="$work/cargo.log" CARGO_LICENCES="$work/licences" \
        OCS_PIN_FILE="$work/ocs.pin" OCS_FETCH_FROM="$upstream" OCS_TARGET_DIR="$work/ocs-target" \
        OCS_NOTICE_TEMPLATE="$work/cad-notice.txt" OCS_NOTICE="$work/notice.txt" "$@" \
        bash "$packaging_dir/archive-open-cad-studio-source.sh" "$work/source"
}
source_archive=$work/source/open-cad-studio-source_$short.tar.gz
vendor_archive=$work/source/open-cad-studio-vendor_$short.tar.gz
if output=$(archive_source 2> "$work/source.log"); then
    if [[ "$output" == "$source_archive"$'\n'"$vendor_archive" ]]; then
        passed "archive-open-cad-studio-source.sh writes the source and the vendored crates of the pinned commit"
    else
        wrong "archive-open-cad-studio-source.sh printed '$output'"
    fi
    expected=$(git -C "$upstream" ls-tree -r --name-only "$kept" | sed "s|^|open-cad-studio-$short/|" | sort)
    archived=$(tar -tzf "$source_archive" | grep -v '/$' | sort)
    if [[ "$archived" == "$expected" ]]; then
        passed "the source archive holds every file of the pinned commit"
    else
        wrong "the source archive does not hold the files of the pinned commit:"
        diff <(echo "$expected") <(echo "$archived") | sed 's/^/        /'
    fi
    # Only the crates from git repositories, in the folder vendor/ of the
    # source, with the configuration that puts them in place of those.
    vendored=$(tar -tzf "$vendor_archive" | grep -v '/$' | sort)
    expected=$(printf "open-cad-studio-$short/vendor/%s\n" config.toml \
        followed-0.1.0/.cargo-checksum.json followed-0.1.0/Cargo.toml followed-0.1.0/LICENSE \
        shortened-0.1.0/.cargo-checksum.json shortened-0.1.0/Cargo.toml shortened-0.1.0/LICENSE | sort)
    if [[ "$vendored" == "$expected" ]]; then
        passed "the vendored crates are those from git repositories, in the folder vendor/ of the source"
    else
        wrong "the archive of vendored crates does not hold the crates from git repositories alone:"
        diff <(echo "$expected") <(echo "$vendored") | sed 's/^/        /'
    fi
    config=$(tar -xzOf "$vendor_archive" "open-cad-studio-$short/vendor/config.toml")
    if [[ "$(head -n 1 <<< "$config")" == "# Git dependencies of Open CAD Studio commit $kept" ]] \
        && grep -qxF 'directory = "vendor"' <<< "$config" \
        && grep -qxF 'branch = "feature/one"' <<< "$config" \
        && grep -qxF 'git = "https://example.org/short.git"' <<< "$config" \
        && ! grep -qF 'crates-io' <<< "$config"; then
        passed "vendor/config.toml names the commit and puts the folder in place of the git repositories only"
    else
        wrong "vendor/config.toml does not name the commit or does not replace the git repositories alone:"
        sed 's/^/        /' <<< "$config"
    fi
    if (cd "$work/source" && if command -v sha256sum >/dev/null 2>&1; then sha256sum -c --quiet ./*.sha256; else shasum -a 256 -c --quiet ./*.sha256; fi) > /dev/null 2>&1 \
        && [[ -f "$source_archive.sha256" && -f "$vendor_archive.sha256" ]]; then
        passed "both archives have a .sha256 beside them"
    else
        wrong "the archives have no fitting .sha256 beside them"
    fi
    if grep -q '|metadata .*--config vendor/config.toml' "$work/cargo.log"; then
        passed "Cargo resolves the unpacked archives with the vendored crates"
    else
        wrong "the unpacked archives are not resolved with the vendored crates: $(cat "$work/cargo.log")"
    fi
else
    wrong "archive-open-cad-studio-source.sh failed:"
    sed 's/^/        /' "$work/source.log"
fi

# The crates vendored for the commit are used again; those of another
# commit are not.
cp "$vendor_archive" "$work/vendor-before.tar.gz"
if archive_source > /dev/null 2> "$work/source.log" && ! grep -q '|vendor ' "$work/cargo.log" \
    && cmp -s "$vendor_archive" "$work/vendor-before.tar.gz"; then
    passed "the crates vendored before for the commit are used again, for the same archive"
else
    wrong "the crates vendored before for the commit are vendored again, or give another archive:"
    sed 's/^/        /' "$work/source.log" "$work/cargo.log"
fi
sed -i.orig "1s/$kept/$good_hash/" "$work/ocs-target/vendor/config.toml"
rm -f "$work/ocs-target/vendor/config.toml.orig"
if archive_source > /dev/null 2> "$work/source.log" && grep -q '|vendor ' "$work/cargo.log"; then
    passed "crates vendored for another commit are vendored again"
else
    wrong "crates vendored for another commit are used:"
    sed 's/^/        /' "$work/source.log"
fi

# Notices that do not name a crate are refused, and so is a vendored folder
# that misses one.
grep -v 'shared 2.0.0' "$work/cad-notice.txt" > "$work/cad-notice-short.txt"
if archive_source OCS_NOTICE_TEMPLATE="$work/cad-notice-short.txt" > /dev/null 2> "$work/source.log"; then
    wrong "archive-open-cad-studio-source.sh accepts a notice that does not name a crate under a copyleft licence"
elif grep -qF 'shared 2.0.0 (LGPL-2.1-or-later)' "$work/source.log"; then
    passed "archive-open-cad-studio-source.sh refuses a notice that does not name a crate under a copyleft licence"
else
    wrong "archive-open-cad-studio-source.sh refuses a notice without a crate for another reason:"
    sed 's/^/        /' "$work/source.log"
fi
grep -v 'commit 1111' "$work/cad-notice.txt" > "$work/cad-notice-short.txt"
if archive_source OCS_NOTICE_TEMPLATE="$work/cad-notice-short.txt" > /dev/null 2> "$work/source.log"; then
    wrong "archive-open-cad-studio-source.sh accepts a notice that does not name the commit of a git repository"
elif grep -qF 'commit 1111111111111111111111111111111111111111' "$work/source.log"; then
    passed "archive-open-cad-studio-source.sh refuses a notice that does not name the commit of a git repository"
else
    wrong "archive-open-cad-studio-source.sh refuses a notice without a commit for another reason:"
    sed 's/^/        /' "$work/source.log"
fi
echo "It links shared." > "$work/notice-short.txt"
if archive_source OCS_NOTICE="$work/notice-short.txt" > /dev/null 2> "$work/source.log"; then
    wrong "archive-open-cad-studio-source.sh accepts a NOTICE.txt that does not name a crate under a copyleft licence"
elif grep -qF 'copyleft licence that Open CAD Studio links: shortened' "$work/source.log"; then
    passed "archive-open-cad-studio-source.sh refuses a NOTICE.txt that does not name a crate under a copyleft licence"
else
    wrong "archive-open-cad-studio-source.sh refuses a NOTICE.txt for another reason:"
    sed 's/^/        /' "$work/source.log"
fi
rm -rf "$work/ocs-target/vendor/shortened-0.1.0"
if archive_source > /dev/null 2> "$work/source.log"; then
    wrong "archive-open-cad-studio-source.sh accepts a vendored folder without a crate"
elif grep -qF 'remove it, and it is vendored again' "$work/source.log"; then
    passed "archive-open-cad-studio-source.sh refuses a vendored folder without a crate"
else
    wrong "archive-open-cad-studio-source.sh refuses a vendored folder without a crate for another reason:"
    sed 's/^/        /' "$work/source.log"
fi

# The release has both as its last files, under names that the download
# buttons of the website, which look for the endings of the packages, never
# offer.
source_names=$(bash "$packaging_dir/expected-assets.sh" 9.9.9 | tail -n 2)
if [[ "$source_names" == "$pinned_archive"$'\n'"$pinned_vendor" \
    && "$pinned_archive" == open-cad-studio-source_*.tar.gz && "$pinned_vendor" == open-cad-studio-vendor_*.tar.gz \
    && "$source_names" != *linux* && "$source_names" != *macos* && "$source_names" != *windows* ]]; then
    passed "the release carries the source of Open CAD Studio as $(echo $source_names)"
else
    wrong "the release does not carry the source of Open CAD Studio as $pinned_archive and $pinned_vendor, but '$(echo $source_names)'"
fi

if [[ "$failures" -ne 0 ]]; then
    fail "$failures of the tests failed"
fi
echo "all tests of the package scripts passed"
