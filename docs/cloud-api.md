# Cloud API

`open-pointcloud-studio-api` is the headless service of Open Pointcloud Studio: point-cloud files in the request, and the same operations the application and its command line run on them. It speaks the HTTP contract of the OpenAEC SuperCloud (REST, MCP and OpenAPI on one port) and holds no state between calls.

There is no second calculation path. The service, the `--export`, `--section`, `--drawing`, `--mesh`, `--surface` and `--scans` modes of the command line and the C library all call the functions of `pointcloud_core::ops`. Tests hold the files the service writes to the files those modes write, byte for byte (E57 by its points, because every E57 file has identifiers of its own).

The service is LGPL-3.0-or-later, as the core it serves.

## Tools

| Tool | What it does | Result |
| --- | --- | --- |
| `inspect` | The number of points, the bounds, whether the points carry colour, intensity and class, and the scanner stations with their positions and axes | `report` only |
| `convert` | Converts to LAS, LAZ, E57, binary PLY, XYZ, PTS or CSV; with `box`, only the points inside a box, turned or not | the file, `report.points` |
| `section_drawing` | Draws the slab behind one face of a box as a 2D DXF or DWG drawing: `plan`, `front`, `back`, `left` or `right` | the drawing, the counts of the drawing in `report` |
| `terrain_mesh` | A 2.5D terrain mesh from the lowest points seen from above | an OBJ file, `report.source_points`, `vertices`, `triangles` |
| `surface_mesh` | A 3D surface mesh through a sample of the points; not watertight | an OBJ file, the same `report` |

The input schemas are in `GET /v1/tools` and in `GET /v1/openapi.json`, each with a valid example.

Every tool takes the file as `input`:

```json
"input": {"format": "laz", "data": "<base64>", "name": "scan.laz"}
```

- `format` is the extension of the file: `las`, `laz`, `e57`, `ply`, `pcd`, `ptx`, `pts`, `xyz`, `asc`, `txt` or `csv`.
- `data` is the file in base64 (RFC 4648 section 4, with padding, without line breaks).
- `name` is optional and is a file name, never a path: its stem names the result (`scan.dxf`) and the point layer of a drawing. Without it the stem is `input`.

A box is `{"min": [x, y, z], "max": [x, y, z], "rotation": degrees}` in the units of the scan; `rotation` is optional and turns the box counter-clockwise, seen from above, about the vertical through its centre.

A tool with a file answers:

```json
{
  "output": {"format": "laz", "media_type": "application/octet-stream", "file_name": "scan.laz",
             "bytes": 123456, "sha256": "<hex>", "data": "<base64>"},
  "report": {"points": 1000}
}
```

and `inspect` answers `{"report": {...}}`. The media types are `application/octet-stream` for LAS, LAZ, E57 and PLY, `text/plain; charset=utf-8` for XYZ and PTS, `text/csv`, `image/vnd.dxf`, `image/vnd.dwg` and `model/obj`.

Files travel as base64 up to the body limit, so the tools suit programs and pipelines, not a language model that would have to write out a scan.

## Endpoints

| Route | Key | Origin check | Answer |
| --- | --- | --- | --- |
| `GET /v1/health` | no | no | `{"status": "ok", "versie", "protocol"}`; 503 `WERK_VASTGELOPEN` while cancelled work does not stop |
| `GET /v1/openapi.json` | no | no | OpenAPI 3.1, built from the tools |
| `GET /v1/tools` | yes | yes | `{"tools": [...]}`, the same list as MCP `tools/list` |
| `POST /v1/tools/{name}` | yes | yes | the result of the tool; the body is the JSON of its arguments |
| `POST /mcp` | yes | yes | one JSON-RPC message (MCP Streamable HTTP, answered as JSON); a notification gets 202 |

A wrong method gets 405 with `Allow`, and any other route 404. The answers of the tool routes carry `Cache-Control: no-store`.

## Errors

Every answer that is not a success is JSON with `error_code`, `melding` and `remedie` (`null` when nothing can be done). `melding` and `remedie` are English, as the rest of this product's API.

| HTTP | `error_code` | When |
| --- | --- | --- |
| 400 | `ARGUMENT_ONGELDIG` | not JSON or not an object; an unknown or forbidden argument; a wrong type or range; bad base64; an unknown format; a `name` that is not a file name |
| 400 | `LICHAAM_ONLEESBAAR` | the body could not be read, other than being too large |
| 401 | `TOEGANG_GEWEIGERD` | no key or a wrong key; with `WWW-Authenticate: Bearer` |
| 403 | `HERKOMST_GEWEIGERD` | an `Origin` that is not listed in `OPENAEC_API_HERKOMSTEN` |
| 404 | `TOOL_ONBEKEND` / `ROUTE_ONBEKEND` | no such tool / no such route |
| 405 | `METHODE_NIET_TOEGESTAAN` | a method the route does not take, with `Allow` |
| 413 | `LICHAAM_TE_GROOT` | a body above the limit |
| 422 | `TOOL_MISLUKT` | the core refused the work or failed in it; a slab without points gets the hint of its view as `remedie` |
| 422 | `UITKOMST_TE_GROOT` | a result above the output limit, known before the work where its size is known, else after it |
| 500 | `INTERNE_FOUT` | a panic, or a failure of the service itself |
| 503 | `DRUK` | no place to compute: the waiting room is full, or the wait for a place ran out; with `Retry-After: 10` |
| 503 | `WERK_VASTGELOPEN` | health only: cancelled work still runs 120 s after its time limit |
| 504 | `TIJDSLIMIET` | the time limit passed; the work is cancelled |

No `melding` names a folder of the service: the work folder of a call, `OPENAEC_API_WERKMAP` and the system temp folder are taken out, so `/work/call-x/input.las changed while it was read` becomes `input.las changed while it was read`.

Over MCP, a tool that fails answers a result with `isError: true`, the text `Error: <melding>` and the triple as `structuredContent`. The protocol errors −32700 (parse error, `LICHAAM_ONLEESBAAR`), −32600 (invalid request or a batch, `ARGUMENT_ONGELDIG`), −32601 (unknown method, `ROUTE_ONBEKEND`) and −32602 (invalid params, `ARGUMENT_ONGELDIG`) carry the triple as `error.data`. A full waiting room, an expired wait, the time limit and a panic are answered as on the REST routes, with their HTTP status.

A load balancer in front of the service answers some errors itself, such as its own 502, 503 and 504 when no healthy instance is there or an instance does not answer in time. Those pages are not JSON. The deployment in `deploy/aws` answers paths outside the API with a fixed JSON 404 of its own.

## What is not in the cloud

| Tool or mode | Why |
| --- | --- |
| closed mesh, face detection, mesh conversion | they run in the desktop application today; they follow when their one-file path is in `pointcloud_core::ops` |
| thinning | the arithmetic has two copies; one function in the core comes first |
| merge | it takes several inputs per request |
| photos, texture mesh | many result files; needs a result type that is an archive |
| survey | a large pipeline of the application |
| `--index` | the cloud keeps no state between calls; the C library builds indexes |
| 3D BAG | it fetches a third-party service, and the cloud only works on data in the request |
| the window tools of `--mcp` | they drive a desktop window |

Raw bodies, an object store for large files and asynchronous jobs need a change of the shared contract first.

## Cloud tools beside the desktop and the command line

| Cloud tool | Desktop MCP tool | Command line | Operation |
| --- | --- | --- | --- |
| `inspect` | (`status` reports layers, not files) | `--scans` | `open_source`, `inspect` |
| `convert` | `export`, `export_section` with `set_section` | `--export`, `--section` | `open_source`, `convert` with a point limit from the output limit |
| `section_drawing` | `export_drawing` | `--drawing` | `section_drawing_file`, without a cached index |
| `terrain_mesh` | `mesh` with mode `terrain` | `--mesh` | `terrain_mesh_file` |
| `surface_mesh` | `mesh` with mode `surface`, `set_surface_settings` | `--surface` | `surface_mesh_file` |

Where the meaning is the same, the arguments have the names, types and limits of the desktop tools: `rotation` (of `set_section`, here inside `box`), `view`, `thickness`, `units` and `fill` (of `export_drawing`), and `max_vertices`, `neighbors`, `edge_factor` and `mesh_size` (of the surface settings). A test holds them equal. The tool names differ on purpose: the desktop tools act on the window's active layer, section box and class filters and write to a path; the cloud tools take a file in the request.

## Keys

The service does not start without access control.

1. Give each client a long random key.
2. Put the SHA-256 of each key, comma-separated, in `OPENAEC_API_SLEUTELS`:

   ```bash
   printf %s "<key>" | sha256sum
   ```

3. The client sends `Authorization: Bearer <key>`.

A value that is not 64 hex digits stops the start; the message gives its place and its length, never the value. The log line of a request names the first eight hex digits of the hash of its key, never the key.

`OPENAEC_API_TOEGANG=open` drops the key check, for local use only. It is refused together with `OPENAEC_API_SLEUTELS`, and in a managed container runtime: when `ECS_CONTAINER_METADATA_URI_V4`, `ECS_CONTAINER_METADATA_URI` or `AWS_EXECUTION_ENV` is set, the service does not start open.

## Running locally

```bash
cd native
OPENAEC_API_TOEGANG=open cargo run -p open-pointcloud-studio-api -- --http 127.0.0.1:8080
```

Or the container (`deploy/cloud-api/Dockerfile`), published on the loopback address only:

```bash
docker build -f deploy/cloud-api/Dockerfile -t open-pointcloud-studio-api .
docker run --rm -p 127.0.0.1:8080:8080 -e OPENAEC_API_TOEGANG=open open-pointcloud-studio-api
```

In the image the port is set only through `OPENAEC_MCP_HTTP` (`0.0.0.0:8080`), so that the health check of the container, `open-pointcloud-studio-api --health-check`, finds it. `--health-check [--http ADDRESS]` asks `GET /v1/health` of `127.0.0.1` at that port within 3 s and ends with 0 on 200.

```text
open-pointcloud-studio-api [--http ADDRESS]
open-pointcloud-studio-api --health-check [--http ADDRESS]
open-pointcloud-studio-api --version | -V
open-pointcloud-studio-api --help | -h
```

## Examples

Build the request with `jq`, send it with `curl`, and take the file out of the answer:

```bash
jq -n --rawfile data <(base64 -w0 scan.laz) \
  '{input: {format: "laz", data: $data, name: "scan.laz"}, output_format: "ply",
    box: {min: [0, 0, 0], max: [10, 10, 3], rotation: 30}}' > request.json
curl -s -X POST http://127.0.0.1:8080/v1/tools/convert \
  -H "Authorization: Bearer $KEY" -H "content-type: application/json" \
  --data-binary @request.json > answer.json
jq -r .output.data answer.json | base64 -d > scan.ply
jq .report answer.json
```

A plan drawing of a storey:

```bash
jq -n --rawfile data <(base64 -w0 scan.laz) \
  '{input: {format: "laz", data: $data, name: "scan.laz"},
    box: {min: [0, 0, 0], max: [20, 15, 1.2]}, view: "plan", output_format: "dxf"}' \
  | curl -s -X POST http://127.0.0.1:8080/v1/tools/section_drawing \
      -H "Authorization: Bearer $KEY" -H "content-type: application/json" --data-binary @- \
  | jq -r .output.data | base64 -d > storey.dxf
```

## MCP client

A client that speaks Streamable HTTP connects to `/mcp` with the key as a header:

```json
{"type": "http", "url": "https://HOST/mcp", "headers": {"Authorization": "Bearer <key>"}}
```

`initialize` answers the protocol version the client asks for when the service speaks it (`2025-11-25`, `2025-06-18`, `2025-03-26`) and the newest otherwise, with `capabilities: {tools: {listChanged: false}}` and instructions on base64 and the limits. The service keeps no session. Tool results carry `structuredContent` unless the client sends an `MCP-Protocol-Version` before `2025-06-18`. A file result is an embedded resource `{"uri": "result:<file_name>", "mimeType", "blob": "<base64>"}`; `structuredContent` is the REST answer without `output.data`.

## Limits

| Variable | Default | Meaning |
| --- | --- | --- |
| `OPENAEC_API_SLEUTELS` | none | Comma-separated SHA-256 hex of the keys. Required unless open. |
| `OPENAEC_API_TOEGANG` | none | `open`: no key check, for local use only. Refused with keys and in a managed runtime. |
| `OPENAEC_MCP_HTTP` | `127.0.0.1:8080` | The address to listen on; `--http` wins. The image sets `0.0.0.0:8080`. |
| `OPENAEC_MCP_LOG` | `info` | The filter of the log. |
| `OPENAEC_API_LOGVORM` | `text` | `json`: one JSON object per line (the image). |
| `OPENAEC_API_MAX_LICHAAM_MIB` | `64` | The largest body, 1 to 512 MiB. |
| `OPENAEC_API_MAX_UITKOMST_MIB` | `256` | The largest result file before base64, 1 to 1024 MiB. |
| `OPENAEC_API_GELIJKTIJDIG` | `2` | Calls computing at once, 1 to 64. |
| `OPENAEC_API_WACHTRIJ` | `4` | Large requests (a body above 64 KiB) that may wait for a place, 0 to 256. |
| `OPENAEC_API_WACHTEN_S` | `30` | How long a call waits for a place before 503, 0 to 600 s. |
| `OPENAEC_API_TIJDSLIMIET_S` | `300` | How long a call may compute before 504, 1 to 3600 s. |
| `OPENAEC_API_WERKMAP` | the system temp folder | Where the work folder of each call is made (the image: `/work`). |
| `OPENAEC_API_HERKOMSTEN` | empty | Comma-separated `Origin` values allowed on the protected routes, such as `https://app.example.org`. |
| `OPENAEC_API_GEHEUGEN_MIB` | none | The memory the service may use, in MiB. Without it, the limit of the container's control group when it is finite. |

A value out of range stops the start with exit code 2 and a line on standard error that names the variable.

**Admission.** A request whose `Content-Length` is at most 64 KiB passes at once: lists, the handshake, a ping, small calls. Any other request, also one without `Content-Length`, takes a place in a waiting room of `GELIJKTIJDIG + WACHTRIJ` places before its body is read, or gets 503 `DRUK` at once when the room is full. The place is held until the work of the call ends, also when it was cancelled at its time limit, so that the bodies held stay within the worst case below. A call then waits up to `WACHTEN_S` for one of the `GELIJKTIJDIG` places to compute. Lists and the handshake never wait.

**Expansion.** LAZ holds about 3 to 4 bytes a point; LAS takes 20 to 34, binary PLY about 27, text 30 to 60. A 48 MiB LAZ file (64 MiB as base64) holds 12 to 16 million points, which do not fit 256 MiB as LAS, PLY or text. The service works out an upper bound of the bytes a point takes in the format asked for: a whole conversion that cannot fit is refused with 422 `UITKOMST_TE_GROOT` before anything is written, and a conversion within a box stops writing at the limit. Crop with `box`, or choose `laz`. Drawings and meshes are bounded by the core (at most 400,000 points in a drawing, 1,000,000 vertices in a surface) and are checked against the limit after the work.

**Memory.** A running call holds about its body, its result as base64 (1.34 times the result) and its working set, which is at most 1.2 GiB for the largest section drawing; a waiting call holds its body. The worst case is `GELIJKTIJDIG × (body + 1.34 × result + 1.2 GiB) + WACHTRIJ × body`; with the defaults 2 × (64 + 344 + 1229) MiB + 4 × 64 MiB, about 3.5 GiB. The service logs it at start and does not start when it exceeds `OPENAEC_API_GEHEUGEN_MIB` or the limit of the container. The task of the deployment in `deploy/aws` has 8 GiB.

**Storage.** Nothing persists. Each call has one work folder under `OPENAEC_API_WERKMAP`, holding `input.<format>` and `out/<stem>.<extension>`, and it is removed when the work ends, also after a 504. The disk caches of the core are off. The core writes the scratch files of compressed PCD to the system temp folder, so the image sets `TMPDIR=/work/tmp`; at start the service checks that it can write in both folders and names the folder when it cannot. Logs hold the method, the route, the tool, the status, the duration, the size of the body and the prefix of the key's hash, never a body or a path of an upload.

**Cancellation.** At the time limit the call is answered with 504 and its work is cancelled: the operations of the core ask whether to go on every 65,536 points while a file is read, at each stage of a mesh, at each stage of a drawing and while its entities are written. The work keeps its place to compute until it has stopped. A conversion cannot be stopped in this version; it is bounded by one read of the upload. Work that still runs 120 s after its time limit turns `GET /v1/health` to 503 `WERK_VASTGELOPEN`, so that the orchestrator replaces the instance; it turns back to 200 when the work ends.

**Two lines of defence.** The first line refuses an unknown tool (404) and the arguments `path`, `project_path`, `project`, `pad`, `output_path`, `destination` and `directory` (400) before the arguments are typed or anything is queued; no schema holds them. The second line holds where the first would miss something: the arguments of the tools hold no field that is a path; only the service makes paths, inside the fresh work folder of a call and from fixed names (`input.<format>`, `out/<stem>.<extension>`); and with its disk caches off the core reads nothing that lies under its cache folder.

**Origin.** On the protected routes a request with an `Origin` that is not listed gets 403, against DNS rebinding of a service that runs open on a workstation. Requests without `Origin`, which programs send, pass. Health and the OpenAPI document stay public.

## Extensions to the shared contract

What this service adds to the SuperCloud contract, each with its reason. They are proposed for the shared contract only with the owner's go.

- **Codes** `HERKOMST_GEWEIGERD` (the origin check), `UITKOMST_TE_GROOT` (a result above the output limit), `ROUTE_ONBEKEND` and `METHODE_NIET_TOEGESTAAN` (every answer is JSON, also for a wrong route or method), `WERK_VASTGELOPEN` (health of an instance whose work does not stop).
- **Ten variables** besides the keys and open access: `OPENAEC_API_LOGVORM`, `OPENAEC_API_MAX_LICHAAM_MIB`, `OPENAEC_API_MAX_UITKOMST_MIB`, `OPENAEC_API_GELIJKTIJDIG`, `OPENAEC_API_WACHTRIJ`, `OPENAEC_API_WACHTEN_S`, `OPENAEC_API_TIJDSLIMIET_S`, `OPENAEC_API_WERKMAP`, `OPENAEC_API_HERKOMSTEN` and `OPENAEC_API_GEHEUGEN_MIB`: point-cloud files are large, so the limits and the memory are set per deployment.
- **The waiting room**: a large body is read only when there is room for it, so that memory stays within the worst case and lists never wait.
- **English `melding` and `remedie`** with the shared Dutch codes, as the rest of this product's API.
- **`structuredContent` without the file**: over MCP the file travels once, as the blob of an embedded resource.
- **The `Origin` check** on the protected routes.
- **Health 503 on stuck work**, so that an orchestrator replaces an instance whose cancelled work does not stop.
- **The triple as JSON-RPC `error.data`**, so that a protocol error carries the same codes as an HTTP error.

## Deployment notes

The service runs as a Linux container on Amazon ECS with AWS Fargate behind an Application Load Balancer; the CloudFormation template, its parameters and the steps of the owner are in `deploy/aws`. The keys come from AWS Secrets Manager into `OPENAEC_API_SLEUTELS`; open access is refused there. The load balancer's own error pages are not JSON (see Errors).
