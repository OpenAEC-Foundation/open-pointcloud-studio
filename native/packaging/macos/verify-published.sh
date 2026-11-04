#!/usr/bin/env bash
# Verify already-downloaded macOS files after the caller checked SHA256SUMS.
# No signing credentials: validate the actual Developer ID signatures, ticket,
# quarantine/Gatekeeper assessment, native application and C SDK.
set -euo pipefail
[[ $# == 3 ]] || { echo 'usage: verify-published.sh ASSETS NUMBER ABSOLUTE_RECORD' >&2; exit 2; }
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
assets=$(cd "$1" && pwd)
NUMBER=$2
record=$3
[[ "$NUMBER" =~ ^[1-9][0-9]{3}\.([1-9]|[1-4][0-9]|5[0-3])\.(0|[1-9][0-9]?)$ ]] || exit 2
[[ "$record" == /* ]] || { echo 'record path must be absolute' >&2; exit 2; }
: "${MACOS_SIGNED:?expected application signing state is required}"
: "${SDK_MACOS_SIGNED:?expected SDK signing state is required}"
for state in "$MACOS_SIGNED" "$SDK_MACOS_SIGNED"; do
    [[ "$state" == true || "$state" == false ]] || exit 2
done
PART=${PART:-macos-$(uname -m)}
[[ "$PART" == "macos-$(uname -m)" ]] || { echo 'runner architecture does not match record' >&2; exit 1; }
mkdir -p "$(dirname "$record")"
if [[ ! -s "$record" ]]; then
    printf '## %s\n\nRunner image %s %s, %s.\n\n' "$PART" "${ImageOS:-unknown}" "${ImageVersion:-}" "$(uname -sm)" > "$record"
fi
identities="$(dirname "$record")/identities-$PART.txt"
: > "$identities"
scratch=$(mktemp -d "${TMPDIR:-/tmp}/ops-release-verify.XXXXXX")
mounted=false
cleanup() {
    if [[ "$mounted" == true ]]; then hdiutil detach "$scratch/image" > /dev/null 2>&1 || true; fi
    rm -rf "$scratch"
}
trap cleanup EXIT
failures=()
# shown COMMAND...: runs it, and puts it with its output and exit
# code into the record.
shown() {
  local output code
  output=$("$@" 2>&1) && code=0 || code=$?
  { echo "\`$*\` (exit $code)"; echo; echo '```'; echo "$output"; echo '```'; echo; } >> "$record"
  echo "$output"
  return "$code"
}
dmg="$assets/open-pointcloud-studio_${NUMBER}_macos-universal.dmg"
# Marked as a browser marks a download, so that the system assesses
# it as it would assess a download.
xattr -w com.apple.quarantine "0081;$(printf %x "$(date +%s)");verify-release;$(uuidgen)" "$dmg"
{ echo "### Assessment and notarization"; echo; } >> "$record"
accepted=true
shown spctl -a -t open --context context:primary-signature -v "$dmg" > /dev/null || accepted=false
shown bash "$script_dir/validate-ticket.sh" "$dmg" > /dev/null || accepted=false
hdiutil attach -nobrowse -readonly -mountpoint "$scratch/image" "$dmg" > /dev/null
mounted=true
app="$scratch/image/Open Pointcloud Studio.app"
shown spctl --assess --type execute -vv "$app" > /dev/null || accepted=false
if [[ "$MACOS_SIGNED" == true && "$accepted" != true ]]; then
  failures+=("the release says the app is notarized, and spctl or the ticket does not agree")
fi
{ echo "### Signatures"; echo; } >> "$record"
mkdir -p "$scratch/unpacked"
tar -xzf "$assets/open-pointcloud-studio_${NUMBER}_macos-universal.tar.gz" -C "$scratch/unpacked"
tar -xzf "$assets/open-pointcloud-studio-sdk_${NUMBER}_macos-universal-dylib.tar.gz" -C "$scratch/unpacked"
sdk="$scratch/unpacked/open-pointcloud-studio-sdk_${NUMBER}_macos-universal-dylib"
for file in "$app" "$app/Contents/MacOS/open-pointcloud-studio" "$app/Contents/MacOS/OpenCADStudio" \
    "$scratch/unpacked/open-pointcloud-studio_${NUMBER}_macos-universal/open-pointcloud-studio" \
    "$sdk/lib/libops_pointcloud.dylib"; do
  shown codesign --verify --deep --strict "$file" > /dev/null || failures+=("invalid code signature: $file")
  details=$(shown codesign -dvv --verbose=4 "$file") || failures+=("codesign cannot read the signature of $file")
  expected=$MACOS_SIGNED
  [[ "$file" != *.dylib ]] || expected=$SDK_MACOS_SIGNED
  if [[ "$expected" == true ]]; then
    grep -q '^Authority=Developer ID Application' <<< "$details" \
      || failures+=("$(basename "$file") is not signed with the developer certificate")
    echo "Team ID of the signature of $(basename "$file"): $(sed -n 's/^TeamIdentifier=//p' <<< "$details")" >> "$identities"
  fi
done
for program in "$app/Contents/MacOS/open-pointcloud-studio" \
    "$scratch/unpacked/open-pointcloud-studio_${NUMBER}_macos-universal/open-pointcloud-studio"; do
    version=$(shown "$program" --version) || failures+=("$program did not start")
    [[ "$version" == "open-pointcloud-studio $NUMBER" ]] || failures+=("unexpected version: $version")
done
hdiutil detach "$scratch/image" > /dev/null
mounted=false
{ echo "### The C library on $(uname -m)"; echo; } >> "$record"
cc -std=c11 -Wall -Wextra -Werror -I"$sdk/include" "$sdk/examples/inspect.c" \
  -L"$sdk/lib" -lops_pointcloud -Wl,-rpath,"$sdk/lib" -o "$scratch/inspect"
mkdir -p "$scratch/run"
inspection=$(shown "$scratch/inspect" "$scratch/run") || failures+=("the example of the C library failed")
[[ "$inspection" == *"ops_pointcloud $NUMBER, ABI"* ]] || failures+=("unexpected SDK version")
if [[ ${#failures[@]} -gt 0 ]]; then
  { echo "**Failed:**"; printf -- '- %s\n' "${failures[@]}"; } >> "$record"
  printf '::error::%s\n' "${failures[@]}"
  exit 1
fi
