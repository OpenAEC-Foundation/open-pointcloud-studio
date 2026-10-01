# Native command API

The Rust desktop app starts a local command server on `127.0.0.1`. It executes
named Rust operations in the running GUI; it does not evaluate JavaScript or
use a webview. The port and a per-process token are written to
`$XDG_CONFIG_HOME/open-pointcloud-studio-native/instances/instance-<pid>.json`
(or `~/.config/open-pointcloud-studio-native/instances/` when XDG_CONFIG_HOME
is unset). The directory is mode 0700 and the discovery file mode 0600 on
Unix. A fixed port can be requested with `--api-port PORT [INPUT ...]`.

`GET /health` returns `{"status":"ok"}`. `GET /info` returns the process ID,
port, version and API name. `POST /exec` accepts one JSON command and requires
the discovery file's token in the `X-OPS-Token` header. A request without a
valid token receives HTTP 403. The legacy `POST /eval` endpoint returns HTTP
410 because script evaluation is not part of the native application.

For example, after reading `port` and `token` from the discovery file:

```bash
curl -H 'Content-Type: application/json' -H 'X-OPS-Token: TOKEN' \
  -d '{"command":"status"}' http://127.0.0.1:PORT/exec
```

Commands use absolute file paths. They return JSON with `ok: true` or
`ok: false` and an `error`. File opening returns `accepted: true` as soon as
the GUI starts loading; poll `status` for the new layer. Exports return
`accepted: true` and a `job_id`. Query `{"command":"job","id":"JOB_ID"}`
for a durable `running`, `complete` (with point count), or `failed` result.
The newest 32 jobs remain queryable even if the GUI status line changes.
World-box selection also returns a job ID and uses the same query. Its limits
are inclusive source XYZ coordinates, independent of the viewport camera and
point budget. It selects across visible layers while respecting class filters,
the active section box and previously deleted points. Indexed layers search
intersecting octree leaves; unindexed layers stream their complete sources.

| Command | JSON fields | Effect |
| --- | --- | --- |
| `status` | — | Lists clouds, point counts, selected/deleted counts, visibility, active layer, camera, section box and current status text |
| `job` | `id` | Reads an export or selection task's state and result |
| `open` | `path` | Opens a point cloud or mesh in the running GUI |
| `remove` | `index` | Removes a layer from the project |
| `set_active` | `index` | Chooses the active layer |
| `set_visible` | `index`, `visible` | Shows or hides a point layer |
| `camera` | `preset` | Chooses `top`, `bottom`, `front`, `back`, `left`, `right` or `isometric` |
| `set_color` | `mode` | Chooses `rgb`, `elevation`, `intensity` or `classification` |
| `set_class_visible` | `code`, `visible` | Shows or hides one classification code in the viewport and exact selection |
| `set_point_size` | `size` | Sets point size from 1 to 8 |
| `set_budget` | `points` | Sets visible point budget from 1,000 to 2,000,000 |
| `set_section` | `min`, `max` | Enables an XYZ section box using two three-number arrays inside the visible model bounds |
| `clear_section` | — | Disables the section box |
| `select_world` | `min`, `max` | Selects all exact source points in an inclusive XYZ box, returning a job ID |
| `clear_selection` | — | Clears the current point selection |
| `delete_selection` | — | Hides selected points in the open view; may first queue an octree build for LAZ |
| `undo_delete` | — | Restores the latest deletion batch |
| `redo_delete` | — | Reapplies the latest undone deletion batch |
| `export` | `path` | Exports the active source, honoring deleted points |
| `export_section` | `path` | Exports only the current section of the active source, honoring deleted points |

The destination extension selects PLY, XYZ, PTS, CSV, LAS, LAZ or E57. Export
is atomic and scans the complete source rather than the viewport sample.
`status.result.hidden_classes` lists disabled numeric classification codes.
The View ribbon's broad Ground, Vegetation, Buildings and Other groups also
apply; a point must pass both its group and individual class switch.
The server binds only to loopback, limits request bodies to 64 KiB, and has
no permissive browser CORS headers. After an unclean shutdown, an old
discovery file may remain until the next native launch; clients should check
`/health` and `/info` before using an entry.
