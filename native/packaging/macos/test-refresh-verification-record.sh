#!/usr/bin/env bash
set -euo pipefail
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export SOURCE_COMMIT=322c955d00ac1325afaed9e0ceba986eca6dd8a8 ORIGINAL_RUN=41555995130
export GITHUB_REPOSITORY=OpenAEC-Foundation/open-pointcloud-studio GITHUB_SERVER_URL=https://github.com
export CORRECTION_RUN=https://github.com/OpenAEC-Foundation/open-pointcloud-studio/actions/runs/123
export CORRECTION_SHA=322c955d00ac1325afaed9e0ceba986eca6dd8a8
cat > "$work/old" <<'EOF'
# Verification of Open Pointcloud Studio v2026.45.1
- Commit: 322c955d00ac1325afaed9e0ceba986eca6dd8a8
- Result: failure
## windows
WINDOWS_UNCHANGED
## macos-arm64
ARM_UNCHANGED
## macos-x86_64
**Failed:**
Apple network failure
## linux
LINUX_UNCHANGED
## Identities
IDENTITIES_UNCHANGED
EOF
printf '## macos-x86_64\nAll strict checks passed\n' > "$work/new"
run() { bash "$script_dir/refresh-verification-record.sh" v2026.45.1 "$work/old" "$work/new" "$work/result"; }
run
for text in WINDOWS_UNCHANGED ARM_UNCHANGED LINUX_UNCHANGED IDENTITIES_UNCHANGED 'All strict checks passed' '- Result: success'; do
    grep -qF -- "$text" "$work/result"
done
echo 'ok: replaces only Intel evidence and preserves all other platform records'
printf '\n**Failed:**\n' >> "$work/new"
if run; then echo 'accepted a failed replacement' >&2; exit 1; fi
echo 'ok: refuses failed replacement'
printf '## macos-x86_64\nPassed\n' > "$work/new"
sed 's/LINUX_UNCHANGED/**Failed:**/' "$work/old" > "$work/failed"
cp "$work/failed" "$work/old"
if run; then echo 'erased a failure in another platform' >&2; exit 1; fi
echo 'ok: cannot erase a failure in another platform'
