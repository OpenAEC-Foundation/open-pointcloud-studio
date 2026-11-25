#!/usr/bin/env bash
# Public releases require every platform signature. Drafts may be inspected
# without organisation credentials, but explicitly record each missing proof.
set -euo pipefail
problems=0
for field in WINDOWS_SIGNED MACOS_SIGNED SDK_WINDOWS_SIGNED SDK_MACOS_SIGNED LINUX_GPG_SIGNED; do
    value=${!field:-false}
    if [[ "$value" != true ]]; then
        problems=$((problems + 1))
        if [[ ${DRAFT_ONLY:-false} == true ]]; then
            echo "::warning::$field is not true; this draft must not be published"
        else
            echo "::error::$field is not true; refusing to publish unsigned release files"
        fi
    fi
done
[[ $problems -eq 0 || ${DRAFT_ONLY:-false} == true ]]
