#!/usr/bin/env bash
set -euo pipefail
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"
cat > "$work/bin/xcrun" <<'MOCK'
#!/usr/bin/env bash
[[ $1 == stapler && $2 == validate && $3 == 'a file.dmg' ]] || exit 99
n=$(cat "$MOCK_STATE" 2>/dev/null || echo 0)
n=$((n + 1)); echo "$n" > "$MOCK_STATE"
if [[ $MOCK_ERROR == 68 ]]; then
    if [[ $n -le $MOCK_FAILURES ]]; then
        echo 'NSURLErrorDomain Code=-1005 at https://api.apple-cloudkit.com/database/lookup'
        exit 68
    fi
elif [[ $MOCK_ERROR == 65 ]]; then
    echo 'The ticket does not validate'; exit 65
elif [[ $MOCK_ERROR == other68 ]]; then
    echo 'An invalid ticket, not a network error'; exit 68
fi
echo 'The validate action worked!'
MOCK
cat > "$work/bin/sleep" <<'MOCK'
#!/usr/bin/env bash
echo "$1" >> "$MOCK_STATE.sleeps"
MOCK
chmod +x "$work/bin/"*
export PATH="$work/bin:$PATH" MOCK_STATE="$work/state"
check() {
    export MOCK_ERROR=$1 MOCK_FAILURES=$2
    rm -f "$MOCK_STATE" "$MOCK_STATE.sleeps"
    if bash "$script_dir/validate-ticket.sh" 'a file.dmg' > "$work/output"; then code=0; else code=$?; fi
    [[ $code == "$3" && $(cat "$MOCK_STATE") == "$4" ]] || {
        cat "$work/output"; echo "wrong outcome: $code, $(cat "$MOCK_STATE")" >&2; exit 1;
    }
    echo "ok: $MOCK_ERROR, failures=$MOCK_FAILURES, exit=$code, calls=$4"
}
check 0 0 0 1
check 68 1 0 2
check 68 4 0 5
check 68 99 68 5
check 65 99 65 1
check other68 99 68 1
