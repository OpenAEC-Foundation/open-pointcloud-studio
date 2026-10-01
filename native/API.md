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
Mesh jobs report `reading`, `reconstructing`, or `writing` with completed and
total units. `cancel_mesh` requests cancellation; a cancelled mesh leaves an
existing destination untouched. Only one mesh job runs at a time.
`merge_visible` joins all visible LAS/LAZ layers into one `.las` or `.laz` file
in a background task. It preserves original point attributes and applies each
layer's current deletions and affine transform. Sources must have matching LAS
version, point layout, coordinate grid and metadata; incompatible CRS metadata
is rejected instead of silently choosing one. Poll its job or
`status.result.merge` for processed and written point counts. `cancel_merge`
stops the task and leaves an existing destination unchanged.
For large clouds, `scale` returns `running: true`. Poll `status.result.scale`
for processed and total source points; it becomes `null` when the transform
finishes or is cancelled. `cancel_scale` stops the scan without applying the
new factors. Repeating Scale after a successful run reuses the exact centroid
until the set of remaining points changes.
`thin` accepts a keep percentage from 1 to 100 and runs in the background.
Poll `status.result.thin_pending`; after it becomes false, the active cloud's
`remaining` and `deleted` counts reflect the exact edit. `undo_delete` restores
the removed points without changing the source file.
While an uncached octree is built, `status.result.index_progress` reports the
source-read count and known total, then tree records handled, depth and leaf
count. Its `stage` is `reading_source`, `building_tree` or `ready`, and
`cancelling` shows whether cancellation has been requested. The field becomes
`null` after the build finishes. `cancel_index` stops the build and discards
its temporary files without publishing a partial cache.
World-box selection also returns a job ID and uses the same query. Its limits
are inclusive source XYZ coordinates, independent of the viewport camera and
point budget. It selects across visible layers while respecting class filters,
the active section box and previously deleted points. Indexed layers search
intersecting octree leaves; unindexed layers stream their complete sources.
The job and `status.result.selected_points` report exact counts. For very large
selections the viewport draws a representative highlight sample rather than
uploading every selected point again; the native status line reports how many
highlights are shown.
`cancel_selection` stops a running world-box or viewport-box scan; the job
becomes `cancelled` and no partial selection replaces the previous one. Escape
or Clear in the native UI also stops an in-progress scan. An in-progress point
pick is discarded when cancelled.

| Command | JSON fields | Effect |
| --- | --- | --- |
| `status` | — | Lists clouds, point counts, selected/deleted counts, edited bounds and transforms, visibility, active layer, camera, section box, auto-index and 3D surface settings, index and scale progress, and current status text |
| `job` | `id` | Reads an export, selection, mesh or merge task's state and result |
| `open` | `path` | Opens a point cloud or mesh in the running GUI |
| `remove` | `index` | Removes a layer from the project |
| `set_active` | `index` | Chooses the active layer |
| `set_visible` | `index`, `visible` | Shows or hides a point layer |
| `camera` | `preset` | Chooses `top`, `bottom`, `front`, `back`, `left`, `right` or `isometric` |
| `set_color` | `mode` | Chooses `rgb`, `elevation`, `intensity` or `classification` |
| `set_class_visible` | `code`, `visible` | Shows or hides one classification code in the viewport and exact selection |
| `set_point_size` | `size` | Sets point size from 0.1 to 20 |
| `set_eye_dome` | `enabled` | Enables or disables the depth-based shading pass |
| `set_eye_dome_strength` | `strength` | Sets depth-shading strength from 0 to 5; 1 is the default |
| `set_budget` | `points` | Sets visible point budget from 1,000 to 2,000,000 |
| `set_section` | `min`, `max` | Enables an XYZ section box using two three-number arrays inside the visible model bounds |
| `clear_section` | — | Disables the section box |
| `select_world` | `min`, `max` | Selects all exact source points in an inclusive XYZ box, returning a job ID |
| `cancel_selection` | — | Stops a running full-resolution box selection or discards an in-progress point pick |
| `clear_selection` | — | Clears the current point selection |
| `delete_selection` | — | Hides selected points in the open view; may first queue an octree build for LAZ |
| `undo_delete` | — | Restores the latest deletion batch |
| `redo_delete` | — | Reapplies the latest undone deletion batch |
| `thin` | `percent` | Keeps an exact percentage of the active cloud's remaining points, with Undo support |
| `translate` | `offset` | Applies three finite XYZ offsets to the active cloud view |
| `scale` | `factors` | Scales the active view around the exact centroid of remaining points; large sources stream from the disk octree in the background |
| `cancel_scale` | — | Cancels a running centroid calculation without changing the source |
| `build_index` | — | Starts an octree build for the active unindexed cloud |
| `cancel_index` | — | Cancels a running octree build without publishing a partial index |
| `set_auto_index` | `enabled` | Enables or disables automatic indexing of large clouds |
| `set_surface_settings` | `max_vertices`, `neighbors`, `edge_factor` | Sets the native GUI's 3D surface reconstruction limits atomically: 3–1,000,000 vertices, 3–32 neighbors and a finite positive edge factor |
| `reset_transform` | — | Restores the active cloud's source coordinates |
| `mesh` | `mode`, `path` | Starts `terrain` or `surface` reconstruction to an absolute `.obj` path; surface mode uses the current 3D surface settings and returns a job ID |
| `cancel_mesh` | — | Requests cancellation of the running mesh task |
| `merge_visible` | `path` | Merges the visible LAS/LAZ layers to an absolute `.las` or `.laz` path; returns a job ID |
| `cancel_merge` | — | Requests cancellation of the running merge task |
| `export` | `path` | Exports the active source, honoring deleted points |
| `export_section` | `path` | Exports only the current section of the active source, honoring deleted points |
| `export_selection` | `path` | Exports exact selected points from the active source, including points outside the preview |
| `export_minus_selection` | `path` | Exports the active source without selected or deleted points |

The destination extension selects PLY, XYZ, PTS, CSV, LAS, LAZ or E57. Export
is atomic and scans the complete source rather than the viewport sample.
`status.result.hidden_classes` lists disabled numeric classification codes.
Color mode, point size, eye-dome settings, point budget and auto-index changes
made through this API also update the native `settings.json` defaults after a
short debounce, so they remain in effect when the app restarts.
The View ribbon's broad Ground, Vegetation, Buildings and Other groups also
apply; a point must pass both its group and individual class switch.
The server binds only to loopback, limits request bodies to 64 KiB, and has
no permissive browser CORS headers. After an unclean shutdown, an old
discovery file may remain until the next native launch; clients should check
`/health` and `/info` before using an entry.
