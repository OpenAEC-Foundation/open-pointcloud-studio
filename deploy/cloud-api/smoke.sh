#!/usr/bin/env bash
# Smoke test of a running cloud API: health and OpenAPI, the access check,
# each tool on a small file, the refusals and MCP. One line per check; when
# GITHUB_STEP_SUMMARY is set, also a row of a table in the job summary.
#
#   smoke.sh BASE_URL KEY [--version V] [--limit-mib N] [--load]
#
# BASE_URL is where the service answers, such as http://127.0.0.1:8080.
# KEY is a bearer key of the service; - for a service in open mode, which
# leaves out the check that a call without a key is refused.
#   --version V     the health route has to report version V
#   --limit-mib N   the service has a body limit of N MiB
#                   (OPENAEC_API_MAX_LICHAAM_MIB): N + 1 MiB is refused
#   --load          two section drawings of the largest test file at once,
#                   then health
# SMOKE_CONNECT_TO, when set, goes to curl as --connect-to, such as
# api.example.org:443:my-balancer.elb.amazonaws.com:443: the service is then
# reached under the name of its certificate before its DNS record exists.
#
# Needs bash, curl and python3. Exits 1 when a check fails.
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
usage="usage: smoke.sh BASE_URL KEY [--version V] [--limit-mib N] [--load]"
[[ $# -ge 2 ]] || { echo "$usage" >&2; exit 2; }
base=${1%/}
key=$2
shift 2
version=
limit=
load=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --version) version=${2:?$usage}; shift 2 ;;
        --limit-mib) limit=${2:?$usage}; shift 2 ;;
        --load) load=true; shift ;;
        *) echo "$usage" >&2; exit 2 ;;
    esac
done

TOOLS='["inspect", "convert", "section_drawing", "terrain_mesh", "surface_mesh"]'

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
python3 -I "$here/make-test-files.py" "$work/files" || exit 1
files=$work/files

curl_options=(-sS --max-time 330)
[[ -z "${SMOKE_CONNECT_TO:-}" ]] || curl_options+=(--connect-to "$SMOKE_CONNECT_TO")
auth=()
[[ "$key" == - ]] || auth=(-H "Authorization: Bearer $key")

failures=0
checks=0
if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
    {
        echo "### Smoke test of the cloud API"
        echo
        echo "\`$base\`"
        echo
        echo "| Check | Result |"
        echo "| --- | --- |"
    } >> "$GITHUB_STEP_SUMMARY"
fi

# report RESULT TEXT: one line, and a row of the summary.
report() {
    checks=$((checks + 1))
    if [[ "$1" == ok ]]; then
        echo "ok    $2"
    else
        echo "WRONG $2"
        failures=$((failures + 1))
    fi
    if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then
        echo "| ${2//|/\\|} | $1 |" >> "$GITHUB_STEP_SUMMARY"
    fi
}

# call NAME METHOD PATH [BODY_FILE] [CURL_ARGUMENTS...] leaves the answer in
# $work/NAME.json, its headers in $work/NAME.head and the status in $status.
call() {
    local name=$1 method=$2 path=$3 body=${4:-}
    shift 3
    [[ $# -eq 0 ]] || shift
    local arguments=("${curl_options[@]}" -o "$work/$name.json" -D "$work/$name.head"
        -w '%{http_code}' -X "$method" "$base$path")
    if [[ -n "$body" ]]; then
        arguments+=(-H 'Content-Type: application/json' --data-binary "@$body")
    fi
    status=$(curl "${arguments[@]}" "$@" 2> "$work/$name.curl") || status="none ($(head -c 200 "$work/$name.curl"))"
}

# query NAME EXPRESSION prints a Python expression over the JSON answer d of
# NAME (strings as they are, the rest as JSON), or nothing when the answer
# is not JSON or the expression fails.
query() {
    python3 -I -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as file:
        d = json.load(file)
    value = eval(sys.argv[2], {"d": d, "TOOLS": json.loads(sys.argv[3])})
except Exception:
    sys.exit(0)
sys.stdout.write(value if isinstance(value, str) else json.dumps(value))
' "$work/$1.json" "$2" "$TOOLS"
}

# header NAME FIELD prints the value of a header of the answer of NAME.
header() {
    tr -d '\r' < "$work/$1.head" | awk -v field="$(tr '[:upper:]' '[:lower:]' <<< "$2")" '
        { name = tolower($0); sub(/:.*/, "", name) }
        name == field { sub(/^[^:]*:[ \t]*/, ""); print; exit }'
}

# file_of NAME DESTINATION writes the file that the answer of NAME carries
# and checks its bytes and SHA-256 against the answer; it fails otherwise.
file_of() {
    python3 -I -c '
import base64, hashlib, json, sys
with open(sys.argv[1], encoding="utf-8") as file:
    output = json.load(file)["output"]
data = base64.b64decode(output["data"], validate=True)
with open(sys.argv[2], "wb") as file:
    file.write(data)
if len(data) != output["bytes"] or hashlib.sha256(data).hexdigest() != output["sha256"]:
    sys.exit("the file does not have the bytes and SHA-256 that the answer gives")
' "$work/$1.json" "$2" 2> /dev/null
}

# expect_error NAME STATUS CODE: the answer of NAME has STATUS and is the
# JSON triple of the contract with error_code CODE.
expect_error() {
    [[ "$status" == "$2" ]] \
        && [[ "$(query "$1" 'd["error_code"]')" == "$3" ]] \
        && [[ "$(query "$1" 'sorted(d) == ["error_code", "melding", "remedie"]')" == true ]]
}

verdict() { if "$@"; then echo ok; else echo wrong; fi; }

# 1. Health.
call health GET /v1/health
health_ok() {
    [[ "$status" == 200 && "$(query health 'd["status"]')" == ok ]] \
        && [[ -z "$version" || "$(query health 'd["versie"]')" == "$version" ]]
}
report "$(verdict health_ok)" "GET /v1/health: 200, status ok, version $(query health 'd.get("versie")')${version:+ (expected $version)} (status $status)"

# 2. OpenAPI.
call openapi GET /v1/openapi.json
openapi_ok() {
    [[ "$status" == 200 && "$(query openapi 'd["openapi"]')" == 3.1.0 ]] \
        && [[ "$(query openapi 'sorted(d["paths"]) == sorted("/v1/tools/" + name for name in TOOLS)')" == true ]]
}
report "$(verdict openapi_ok)" "GET /v1/openapi.json: OpenAPI $(query openapi 'd.get("openapi")') with a path for each of the five tools (status $status)"

# 3. Access, and the list of tools.
if [[ "$key" == - ]]; then
    report ok "GET /v1/tools without a key: not checked, the service is open"
else
    call no-key GET /v1/tools
    no_key_ok() {
        expect_error no-key 401 TOEGANG_GEWEIGERD && [[ "$(header no-key WWW-Authenticate)" == Bearer* ]]
    }
    report "$(verdict no_key_ok)" "GET /v1/tools without a key: 401 TOEGANG_GEWEIGERD with WWW-Authenticate (status $status)"
fi
call tools GET /v1/tools "" "${auth[@]}"
tools_ok() {
    [[ "$status" == 200 && "$(query tools '[tool["name"] for tool in d["tools"]] == TOOLS')" == true ]]
}
report "$(verdict tools_ok)" "GET /v1/tools: $(query tools '" ".join(tool["name"] for tool in d["tools"])') (status $status)"

# 4. inspect.
call inspect POST /v1/tools/inspect "$files/inspect.json" "${auth[@]}"
inspect_ok() { [[ "$status" == 200 && "$(query inspect 'd["report"]["points"]')" == 1000 ]]; }
report "$(verdict inspect_ok)" "inspect of grid.ply: $(query inspect 'd["report"]["points"]') points of 1000 (status $status)"

# 5. convert to LAZ, and inspect of the LAZ.
call convert-laz POST /v1/tools/convert "$files/convert-laz.json" "${auth[@]}"
laz_ok() {
    [[ "$status" == 200 && "$(query convert-laz 'd["output"]["file_name"]')" == grid.laz ]] \
        && file_of convert-laz "$work/grid.laz" \
        && python3 -I "$here/make-test-files.py" --inspect-answer "$work/convert-laz.json" "$work/inspect-laz-body.json" \
        && call inspect-laz POST /v1/tools/inspect "$work/inspect-laz-body.json" "${auth[@]}" \
        && [[ "$status" == 200 && "$(query inspect-laz 'd["report"]["points"]')" == 1000 ]]
}
report "$(verdict laz_ok)" "convert to LAZ: $(query convert-laz 'd["output"]["file_name"]'), $(query convert-laz 'd["output"]["bytes"]') bytes, SHA-256 as given, inspect of it 1000 points"

# 6. convert with a box.
call convert-box POST /v1/tools/convert "$files/convert-box.json" "${auth[@]}"
box_ok() { [[ "$status" == 200 && "$(query convert-box 'd["report"]["points"]')" == 500 ]]; }
report "$(verdict box_ok)" "convert with a box up to x 1.1: $(query convert-box 'd["report"]["points"]') points of 500 (status $status)"

# 7. inspect of a compressed PCD, whose reader writes to the temp folder.
call inspect-pcd POST /v1/tools/inspect "$files/inspect-pcd.json" "${auth[@]}"
pcd_ok() { [[ "$status" == 200 && "$(query inspect-pcd 'd["report"]["points"]')" == 1000 ]]; }
report "$(verdict pcd_ok)" "inspect of a binary_compressed PCD: $(query inspect-pcd 'd["report"]["points"]') points of 1000 (status $status)"

# 8. A plan drawing of the room.
call drawing POST /v1/tools/section_drawing "$files/drawing.json" "${auth[@]}"
drawing_ok() {
    [[ "$status" == 200 && "$(query drawing 'd["report"]["drawn_points"] > 0')" == true ]] \
        && file_of drawing "$work/room.dxf" \
        && grep -q '^SECTION' <(tr -d '\r ' < "$work/room.dxf") \
        && grep -q '^ENTITIES' <(tr -d '\r ' < "$work/room.dxf")
}
report "$(verdict drawing_ok)" "section_drawing plan of room.xyz: DXF with SECTION and ENTITIES, $(query drawing 'd["report"]["drawn_points"]') points drawn (status $status)"

# 9. Meshes.
for mesh in terrain surface; do
    call "$mesh" POST "/v1/tools/${mesh}_mesh" "$files/$mesh.json" "${auth[@]}"
    mesh_ok() {
        [[ "$status" == 200 ]] && file_of "$mesh" "$work/$mesh.obj" \
            && grep -q '^v ' "$work/$mesh.obj" && grep -q '^f ' "$work/$mesh.obj"
    }
    report "$(verdict mesh_ok)" "${mesh}_mesh: OBJ with v and f lines, $(query "$mesh" 'd["report"]["vertices"]') vertices, $(query "$mesh" 'd["report"]["triangles"]') triangles (status $status)"
done

# 10. Refusals.
call path POST /v1/tools/inspect "$files/path.json" "${auth[@]}"
report "$(verdict expect_error path 400 ARGUMENT_ONGELDIG)" "a path argument: 400 ARGUMENT_ONGELDIG (status $status)"
call unknown-tool POST /v1/tools/unknown "$files/inspect.json" "${auth[@]}"
report "$(verdict expect_error unknown-tool 404 TOOL_ONBEKEND)" "POST /v1/tools/unknown: 404 TOOL_ONBEKEND (status $status)"
call unknown-route GET /v2/x
report "$(verdict expect_error unknown-route 404 ROUTE_ONBEKEND)" "GET /v2/x: 404 ROUTE_ONBEKEND as JSON (status $status)"
call wrong-method GET /v1/tools/convert "" "${auth[@]}"
wrong_method_ok() { expect_error wrong-method 405 METHODE_NIET_TOEGESTAAN && [[ "$(header wrong-method Allow)" == *POST* ]]; }
report "$(verdict wrong_method_ok)" "GET /v1/tools/convert: 405 METHODE_NIET_TOEGESTAAN as JSON, Allow $(header wrong-method Allow) (status $status)"

# 11. Origin.
call foreign-origin GET /v1/tools "" "${auth[@]}" -H 'Origin: https://example.org'
report "$(verdict expect_error foreign-origin 403 HERKOMST_GEWEIGERD)" "GET /v1/tools from Origin https://example.org: 403 HERKOMST_GEWEIGERD (status $status)"
call origin-health GET /v1/health "" -H 'Origin: https://example.org'
report "$(verdict test "$status" = 200)" "GET /v1/health from Origin https://example.org: 200 (status $status)"

# 12. The body limit.
if [[ -n "$limit" ]]; then
    python3 -I "$here/make-test-files.py" --large-body "$limit" "$work/large-body.json"
    call too-large POST /v1/tools/inspect "$work/large-body.json" "${auth[@]}"
    report "$(verdict expect_error too-large 413 LICHAAM_TE_GROOT)" "a body of $limit MiB and a byte: 413 LICHAAM_TE_GROOT (status $status)"
fi

# 13. MCP.
call mcp-initialize POST /mcp "$files/mcp-initialize.json" "${auth[@]}"
initialize_ok() {
    [[ "$status" == 200 && -n "$(query mcp-initialize 'd["result"]["protocolVersion"]')" ]] \
        && [[ "$(query mcp-initialize 'd["result"]["serverInfo"]["name"]')" == open-pointcloud-studio-api ]]
}
report "$(verdict initialize_ok)" "MCP initialize: protocol $(query mcp-initialize 'd["result"]["protocolVersion"]'), $(query mcp-initialize 'd["result"]["serverInfo"]["name"]') $(query mcp-initialize 'd["result"]["serverInfo"]["version"]') (status $status)"
call mcp-initialized POST /mcp "$files/mcp-initialized.json" "${auth[@]}"
report "$(verdict test "$status" = 202)" "MCP notification: 202 (status $status)"
call mcp-list POST /mcp "$files/mcp-list.json" "${auth[@]}"
list_ok() { [[ "$status" == 200 && "$(query mcp-list '[tool["name"] for tool in d["result"]["tools"]] == TOOLS')" == true ]]; }
report "$(verdict list_ok)" "MCP tools/list: $(query mcp-list 'len(d["result"]["tools"])') tools (status $status)"
call mcp-get GET /mcp "" "${auth[@]}"
report "$(verdict expect_error mcp-get 405 METHODE_NIET_TOEGESTAAN)" "GET /mcp: 405 METHODE_NIET_TOEGESTAAN as JSON (status $status)"

# The load step: two drawings of the largest file at once, then health.
if $load; then
    call load-1 POST /v1/tools/section_drawing "$files/load.json" "${auth[@]}" &
    first=$!
    call load-2 POST /v1/tools/section_drawing "$files/load.json" "${auth[@]}"
    wait "$first"
    load_ok() {
        [[ "$(query load-1 'd["report"]["drawn_points"] > 0')" == true ]] \
            && [[ "$(query load-2 'd["report"]["drawn_points"] > 0')" == true ]]
    }
    report "$(verdict load_ok)" "two section drawings of hall.xyz at once: both drawn"
    call load-health GET /v1/health
    report "$(verdict test "$status" = 200)" "GET /v1/health after the load: 200 (status $status)"
fi

echo "$((checks - failures)) of $checks checks passed"
[[ "$failures" -eq 0 ]]
