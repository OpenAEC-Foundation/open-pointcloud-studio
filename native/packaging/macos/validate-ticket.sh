#!/usr/bin/env bash
# Retry only Apple's known CloudKit connection-loss failure. An invalid
# ticket/signature, any other error, or exhausted retries still fails closed.
set -euo pipefail
[[ $# == 1 ]] || { echo 'usage: validate-ticket.sh FILE' >&2; exit 2; }
for attempt in 1 2 3 4 5; do
    if output=$(xcrun stapler validate "$1" 2>&1); then
        printf '%s\n' "$output"
        exit 0
    else
        code=$?
    fi
    printf '%s\n' "$output"
    if [[ $code != 68 || "$output" != *'Code=-1005'* || "$output" != *'api.apple-cloudkit.com'* ]]; then
        exit "$code"
    fi
    [[ $attempt != 5 ]] || exit "$code"
    echo "CloudKit connection lost; validation attempt $attempt failed, retrying."
    sleep "$((attempt * 5))"
done
