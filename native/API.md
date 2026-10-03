# Native command API

## Connecting

The Rust desktop app starts a local command server on `127.0.0.1`. It executes
named Rust operations in the running GUI; it does not evaluate JavaScript or
use a webview. The port and a per-process token are written to
`$XDG_CONFIG_HOME/open-pointcloud-studio-native/instances/instance-<pid>.json`
(or `~/.config/open-pointcloud-studio-native/instances/` when XDG_CONFIG_HOME
is unset; on Windows `%APPDATA%\open-pointcloud-studio-native\instances\`). The directory is mode 0700 and the discovery file mode 0600 on
Unix. Besides `port` and `token`, the file holds the process ID `pid`, the
API name `api` (`native-rust-v1`) and `started`, the time the server started
in milliseconds since 1970, so a client can tell several windows apart and
pick the newest. A fixed port can be requested with `--api-port PORT [INPUT ...]`.
[MCP.md](MCP.md) describes `--mcp`, a Model Context Protocol server that finds
windows this way and offers these commands as tools.

`GET /health` returns `{"status":"ok"}`. `GET /info` returns the process ID,
port, version, API name and start time. `POST /exec` accepts one JSON command and requires
the discovery file's token in the `X-OPS-Token` header. A request without a
valid token receives HTTP 403. The legacy `POST /eval` endpoint returns HTTP
410 because script evaluation is not part of the native application.

For example, after reading `port` and `token` from the discovery file:

```bash
curl -H 'Content-Type: application/json' -H 'X-OPS-Token: TOKEN' \
  -d '{"command":"status"}' http://127.0.0.1:PORT/exec
```

## Answers, jobs and imports

Commands use absolute file paths. They return JSON with `ok: true` or
`ok: false` and an `error`. File opening returns `accepted: true` as soon as
the GUI starts loading; poll `status` for the new layer. `open` answers after
a folder or scan project file has been read on a worker thread, with the list
of files it started loading. Exports return
`accepted: true` and a `job_id`. Query `{"command":"job","id":"JOB_ID"}`
for a durable `running`, `complete` (with point count), or `failed` result.
The newest 32 jobs remain queryable even if the GUI status line changes.
Non-LAS/LAZ imports return an `import_id`; `status.result.imports` lists active
imports with decoded finite-point counts and cancellation state. Use
`cancel_import` with that ID to stop a long import. A cancelled import never
adds a partial layer. LAS/LAZ header previews open immediately and have a null
`import_id`.

## Meshing and merging

Mesh jobs report `reading`, `reconstructing`, or `writing` with completed and
total units. `cancel_mesh` requests cancellation; a cancelled mesh leaves an
existing destination untouched. Only one mesh job runs at a time.
The complete job has `path`, `mode`, `source_points`, `vertices` and
`triangles`, and what was measured of the mesh: `open_edges`, the edges that
belong to one triangle only (the rims of the surface and of its holes), and
`components`, the parts of the mesh that share no vertex with each other. A
closed surface has no open edges. The distance between the points and the
mesh is not measured.
`merge_visible` joins all visible LAS/LAZ layers into one `.las` or `.laz` file
in a background task. It preserves original point attributes and applies each
layer's current deletions and affine transform. Sources must have matching LAS
version, point layout, coordinate grid and metadata; incompatible CRS metadata
is rejected instead of silently choosing one. Poll its job or
`status.result.merge` for processed and written point counts. `cancel_merge`
stops the task and leaves an existing destination unchanged.

## Mesh export

`export_mesh` saves the mesh the active layer holds: a terrain mesh, a 3D
surface, the faces of an opened mesh file or downloaded 3D BAG buildings.
`path` is an absolute destination whose extension chooses the format:

- `.obj`: text, with colours and normals per vertex where the mesh has them.
- `.ply`: binary little-endian, with double coordinates, colours as `uchar`
  and normals as `float` where the mesh has them.
- `.stl`: binary, triangles only, with 32-bit float coordinates.

The mesh is written as the scene shows it, with the move and scale of its
layer applied. The command answers with a `job_id`; the complete job has
`operation` (`export_mesh`), `path`, `format` (`obj`, `ply` or `stl`),
`vertices`, `triangles` and `origin`. `origin` is `null` except for an STL
file of a mesh that lies more than 2,048 m from zero on an axis: such an axis
is written relative to a whole-metre origin, so that the floats keep their
precision. `origin` is that point `[x, y, z]`, and the 80-byte header of the
file names it as `origin X Y Z m`. This application adds it again when it
opens the file; another program shows the mesh near zero.

The file is written to a temporary file first and appears under its name
when it is complete; a failed job leaves an existing destination as it was.
The command is refused for a path that is not absolute or has another
extension, for a layer without a mesh (`status.result.clouds[].mesh` is
`null` there), for the source file of the layer as destination, and while
another mesh export is open or running. `status.result.mesh_export_pending`
is true while the file is written.

## 3D BAG download

`bag3d` downloads the buildings of the Dutch 3D BAG register inside a box and
opens them as a mesh layer, as the 3D BAG panel does, which File > 3D BAG
buildings… opens. `bbox` is `[xmin, ymin, xmax, ymax]` in RD New (EPSG:28992)
with sides longer than 0 and at most 2,000 m, `lod` is `1.2`, `1.3` or `2.2`,
and `path` is an absolute `.obj` destination in a folder that exists; the
file is georeferenced in RD New + NAP and carries the attribution of the
register. The command needs an internet connection and answers with a
`job_id`. While the download runs, its job and
`status.result.bag3d` hold `page` (the pages read), `pages` (the pages the
area needs, or `null` while the service has not said), `buildings`, `lod`,
`path`, `cancel_requested` and `elapsed_seconds`; `status.result.bag3d` is
`null` when no download runs. The complete job has `buildings`, `vertices`,
`triangles`, `pages` and `path`. A page holds about fifty buildings and a
download takes at most a hundred pages, so an area with more than about 5,000
buildings fails after the first page with the number of buildings it holds;
an area whose buildings are too detailed for a mesh of one million vertices
fails after a few pages. A box outside the area of RD New and a destination
whose folder does not exist are refused before anything is asked.
`cancel_bag3d` stops the download once the request under
way has ended; the job becomes `cancelled` and an existing destination stays
as it was. Only one download runs at a time. While the extension is switched
off the command answers `extension bag3d is disabled`.

## Extensions

`list_extensions` lists the optional features built into the application in
`extensions`: for each its `id`, `name`, `version` (the version of the
application), `description`, `author`, `category`, `builtin` (always true: no
code from another source is loaded), `uses_network` and `enabled`.
`set_extension_enabled` switches one on or off and keeps that in
`extensions.json` in the configuration directory; an unknown `id` is refused.
The answer holds `id`, `enabled` and `saved`. When `saved` is false the file
could not be written: the switch holds for this session only and `save_error`
says why. Switching `bag3d` off closes its panel, stops a running download
and disables its entry in the File view.

## File view

`file_view` opens the File view over the model or closes it. With
`open: true` an optional `page` (`workspace`, `extensions` or `about`) chooses
the page; without it the view opens on `workspace`, or keeps the page it
shows. The answer holds `file_view` with `open` and `page`, as
`status.result.file_view` does; `page` is `null` while the view is closed. The
3D BAG panel is not part of the File view and is not opened by this command.
A `page` with `open: false`, an unknown page, and opening while the Settings
dialog is open are refused. The Settings dialog is not opened or closed
through this API.
`status.result.mesh_export_pending` is true from the moment the window asks
where to save a mesh from the File view or Properties, or from the moment
`export_mesh` is accepted, until that file has been written.

## Editing

For large clouds, `scale` returns `running: true`. Poll `status.result.scale`
for processed and total source points; it becomes `null` when the transform
finishes or is cancelled. `cancel_scale` stops the scan without applying the
new factors. Repeating Scale after a successful run reuses the exact centroid
until the set of remaining points changes.
`thin` accepts a keep percentage from 1 to 100 and runs in the background.
Poll `status.result.thin_pending`; after it becomes false, the active cloud's
`remaining` and `deleted` counts reflect the exact edit. `undo_delete` restores
the removed points without changing the source file.

## Index

While an uncached octree is built, `status.result.index_progress` reports the
source-read count and known total, then tree records handled, depth and leaf
count, with `settled` for the points that have reached their leaf of the
`total` points in the cloud. `fraction` is how far the current stage is, from
0 to 1, or `null` while the size of the source is unknown. Its `stage` is
`reading_source`, `building_tree` or `ready`, and
`cancelling` shows whether cancellation has been requested. The field becomes
`null` after the build finishes. `cancel_index` stops the build and discards
its temporary files without publishing a partial cache.

## Selection and picking

World-box selection also returns a job ID and uses the same query. Its limits
are inclusive source XYZ coordinates, independent of the viewport camera and
point budget. It selects across visible layers while respecting class filters,
the active section box and previously deleted points. Indexed layers search
intersecting octree leaves; unindexed layers stream their complete sources.
The job and `status.result.selected_points` report exact counts. For very large
selections the viewport draws a representative highlight sample rather than
uploading every selected point again; the native status line reports how many
highlights are shown.
`pick_screen` uses viewport-local pixel coordinates from the top-left corner;
`status.result.viewport_size` gives the current width and height. It first
checks the point records actually drawn in the active layer's current LOD;
when none covers the pointer, it searches the exact source through its octree
when available, or streams the source otherwise. The optional `radius` defaults to 8 pixels and
may be 1–64. The returned job contains the zero-based source ordinal, world
XYZ, RGB, intensity and classification for a hit; a miss completes with zero
points. It honors the section box, class filters and deleted-point mask.
When the displayed point spheres are larger than the requested tolerance,
picking also accepts their visible discs and chooses the frontmost curved
sphere surface where discs overlap. The source search remains exact even when
the viewport shows only a bounded LOD sample.
Selections retain their exact source-coordinate bounds. `zoom_selection` uses
those bounds to frame the points quickly even after a live transform; the
selection highlight follows the current transform. The command leaves the
section box unchanged and `status.result.selection_bounds_pending` reports
whether a bounds calculation is still running.
If an earlier Delete hid other source points, the cached bounds still apply
when its deletion mask has no ordinal in common with the current selection.
`cancel_selection` stops a running world-box or viewport-box scan; the job
becomes `cancelled` and no partial selection replaces the previous one. Escape
or Clear in the native UI also stops an in-progress scan. An in-progress point
pick is discarded when cancelled.

## Measuring

`measure` sets a finished distance or area measurement from scene coordinates,
as if its points had been picked in the viewport. It replaces the current
measurement, leaves the active tool unchanged and returns the measurement.
`distance` takes 2–256 points and `area` 3–256. `status.result.measure` is
`null` without a measurement. Otherwise it holds `mode`, `finished`, the
`points` and the `segments` lengths in order, where an area includes its
closing edge, followed by `length`, `horizontal_length` and
`height_difference` for a distance, or `area`, `plan_area` and `perimeter` for
an area. `length` and `horizontal_length` follow the polyline, the height
difference is the last point's Z minus the first point's, `area` is the true
3D area and `plan_area` the area seen from above. Values are unrounded scene
units. `status.result.measure_mode` is the measuring mode active in the
viewport: `distance`, `area` or `null`. Points picked there appear in
`measure` with `finished: false` until the measurement is finished.

## Views and annotations

A saved view holds the orbit camera (`yaw`, `pitch`, `zoom`, `pan`), the
walking camera in `walk` when it was saved while walking, the scene bounds and
viewport size the camera was relative to in `frame`, the section box in
`section` (`enabled`, `min`, `max` in model coordinates, also while it is
off), the `color_mode`, its `guid`, its `created` time in seconds since 1970,
`snapshot_due: true` while its snapshot is missing or older than the view,
and its `annotations`. An annotation is `{"kind":"note","point":[x,y,z],
"text":…,"guid":…,"created":…}` or `{"kind":"line","from":[x,y,z],
"to":[x,y,z]}`. The view last saved or restored is the active view:
`add_note` and `add_line` add to it, and first save the current view as
"View N" when no view is active. `status.result.views` reports the `active`
view with its annotations (or `null`), the `annotation_tool` in use in the
viewport (`note`, `line` or `null`), a half-placed annotation in `placing`
(its `kind` and picked `point`: a note that waits for its text or the start of
a line, otherwise `null`), and `snapshots_pending`: the views whose snapshot
image is still to be written. `set_annotation_tool`, `annotate_screen` and
`submit_note` drive the annotation tools as the ribbon and the viewport do:
`annotate_screen` picks the exact source point at a viewport pixel like
`pick_screen` and answers `accepted: true` when the search has started; poll
`status.result.selection_pending` until it is false and read `placing`. A snapshot is taken shortly after a view is saved or updated or
its annotations change, once the viewport has drawn the change, and only
while the viewport shows that view: the camera, the section box, the colour
mode and the scene bounds are as the view was last saved, updated or
restored. After `set_camera`, `set_section` or another change of those,
`add_note`, `add_line` and `delete_annotation` leave the snapshot as it is
and the view keeps `snapshot_due`; `restore_camera_view` shows the view again
and takes the snapshot. A snapshot taken in a viewport of another size than
the view's `frame` rewrites `pan` and `frame` for that size. Wait for `snapshots_pending` to reach 0 before
`export_bcf` when the file should carry the newest images. `export_bcf`
writes all views of the active scan synchronously and answers with the
number of `views` and of `snapshots` in the file, and with `snapshots_due`:
the views whose snapshot is missing or older than the view.

## Screenshots

`screenshot` captures the scene part of the window, without ribbon, panels and
status bar, the way view snapshots are taken. It first waits until the
viewport has read the points for the current camera, at most about 4
seconds; `status.result.detail_pending` is true while those points are still
being read, which starts about 0.2 seconds after the camera changed. The answer has the `width` and `height` of the PNG image in
pixels, its size in `bytes`, the `viewport_size` in logical pixels with the
window's `scale_factor`, `detail_pending` (true when the points were still
being read when the picture was taken), the `path` it was written to or
`null`, and `png_base64`, the image as standard base64 text, when `base64`
is true. Without a `path` the image is returned as base64; with a `path` only
when `base64` is true as well. An image whose longer edge exceeds `max_edge`
(16–8192, default 1920 pixels) is scaled down. A file that exists at `path`
is replaced. The command fails while the File view or Settings covers the
viewport (`file_view` with `open: false` returns to the model), and while the
window is minimised (`"the window is minimised;
restore it to take a screenshot"`); a view snapshot due meanwhile is taken
when the view is restored.

## Commands

| Command | JSON fields | Effect |
| --- | --- | --- |
| `status` | — | Lists clouds (each with `mesh`: `null`, or the `vertices`, `triangles`, `open_edges` and `components` of the mesh the layer holds; for a mesh read from a file the last two count vertices at the same position as one), active imports and decoded counts, selected/deleted counts, the current measurement, edited bounds and transforms, visibility, active layer, camera and viewport size, saved views for that layer and the active view with its annotations, theme, `language` (`auto`, `en` or `nl`, as chosen), section box, auto-index and 3D surface settings, index and scale progress, a running mesh, merge or 3D BAG download (`bag3d`), `mesh_export_pending`, `detail_pending` while the viewport reads points for its camera, whether the File view covers the model (`file_view`), and current status text |
| `job` | `id` | Reads an export, selection, mesh, mesh export, merge or 3D BAG download task's state and result |
| `open` | `path` | Opens a point cloud or mesh, every supported file directly inside a folder, or the scans listed by a scan project file (`.rcp`) in the running GUI. Returns `files`, the accepted paths in opening order, with `missing` (listed scans not found) and their names in `missing_names`, `already_open` (scans skipped because they are open or loading), `errors`, and `import_ids` for the full-stream readers; `import_id` is the last of those or null. Fails when nothing can be opened |
| `cancel_import` | `id` | Cancels a running full-stream import without adding a partial layer |
| `remove` | `index` | Removes a layer from the project |
| `set_active` | `index` | Chooses the active layer |
| `set_visible` | `index`, `visible` | Shows or hides a point layer |
| `camera` | `preset` | Chooses `top`, `bottom`, `front`, `back`, `left`, `right` or `isometric` |
| `set_camera` | `yaw`, `pitch`, `zoom`, `pan` | Sets an exact camera view; angles are radians, zoom is 0.000001–10000, and pan is a two-number screen-pixel array. Rejects non-finite or out-of-range values without changing the view |
| `zoom_all` | — | Fits the complete model at the default isometric orientation, matching the ribbon button and `F` shortcut |
| `open_panorama` | `index`, `station` | Stands in a scanner station of a layer and shows its photos; `status.result.walk` reports the view and whether full-resolution photos are loaded |
| `set_panorama` | `yaw`, `pitch`, `field_of_view` | Turns the walking camera; yaw within ±π, pitch within ±1.55 and a horizontal field of view from 0.35 to 2.1 radians |
| `walk` | `eye`, `yaw`, `pitch` | Places the walking camera at a position in scene coordinates, looking along the heading `yaw` and elevation `pitch`; inside a station ball it shows that station's photos |
| `close_panorama` | — | Leaves the walking camera and returns to the orbit view |
| `list_camera_views` | — | Lists the saved views of the active scan with everything they hold, and the name of the `active` view |
| `save_camera_view` | optional `name` | Saves the current view of the active scan (camera, section box, colour mode) and makes it the active view; returns its `name` and `guid`. The name must be unique within that scan and 1–64 characters long; without a name the first free "View 1", "View 2", … is used (maximum 32 views per scan) |
| `update_camera_view` | `name` | Overwrites a named view with the current view, keeping its name, identifier, time and annotations, and makes it the active view |
| `rename_camera_view` | `name`, `new_name` | Renames a view of the active scan |
| `restore_camera_view` | `name` | Restores a named view of the active scan, ignoring name case: its camera, section box and colour mode. It becomes the active view and its annotations are shown; a snapshot that is due is taken |
| `delete_camera_view` | `name` | Deletes a named view of the active scan with its snapshot, ignoring name case |
| `add_note` | `point`, `text` | Adds a note of 1–240 characters at an `[x, y, z]` scene position to the active view and returns the view's `annotations`. The snapshot is renewed when the viewport shows the view, otherwise when the view is next restored |
| `add_line` | `from`, `to` | Adds a line (drawn as an arrow from the first to the second `[x, y, z]` scene position) to the active view and returns the view's `annotations` |
| `delete_annotation` | `index` | Removes the annotation at that zero-based place in the active view's list |
| `set_annotation_tool` | `tool` | Chooses the `note` or `line` tool of the viewport, or leaves it with `null`; a half-placed annotation is dropped |
| `annotate_screen` | `pointer` | Clicks at viewport pixel `[x, y]` with the active annotation tool: the picked point becomes the point of a note that waits for its text, or the start or the end of a line |
| `submit_note` | `text` | Gives the note that waits for its text its text, adds it to the active view and returns the view's `annotations` |
| `export_bcf` | `path` | Writes all views of the active scan as one BCF 2.1 file at an absolute `.bcf` path |
| `set_theme` | `theme` | Chooses and persists `forge`, `light`, `night`, `blueprint` or `contrast`; `openaec` remains an alias for Night Build |
| `set_language` | `language` | Chooses and persists the language of the user interface: `auto` for the language of the system when there is a translation for it, `en` for English or `nl` for Dutch. Returns the `language` now chosen, as `status.result.language` reports it; an unknown value is refused and changes nothing |
| `set_color` | `mode` | Chooses `rgb`, `elevation`, `intensity` or `classification` |
| `set_class_visible` | `code`, `visible` | Shows or hides one classification code in the viewport and exact selection |
| `set_point_size` | `size` | Sets point size from 0.1 to 20 |
| `set_eye_dome` | `enabled` | Enables or disables the depth-based shading pass |
| `set_eye_dome_strength` | `strength` | Sets depth-shading strength from 0 to 5; 1 is the default |
| `set_budget` | `points` | Sets visible point budget from 1,000 to 10,000,000 |
| `set_section` | `min`, `max` | Enables an XYZ section box using two three-number arrays inside the visible model bounds |
| `clear_section` | — | Disables the section box |
| `select_world` | `min`, `max` | Selects all exact source points in an inclusive XYZ box, returning a job ID |
| `pick_screen` | `pointer`, optional `radius` | Picks a drawn source point near viewport pixel `[x, y]` when possible, then falls back to the full source; returns a job ID |
| `cancel_selection` | — | Stops a running full-resolution box selection or point-pick source scan |
| `clear_selection` | — | Clears the current point selection |
| `measure` | `mode`, `points` | Sets a finished `distance` (polyline) or `area` (closed polygon) measurement through an array of `[x, y, z]` scene coordinates and returns its computed values |
| `clear_measure` | — | Removes the current measurement |
| `zoom_selection` | — | Frames the exact selected source points in the 3D view without changing the section box; poll `selection_bounds_pending` in status until the camera updates |
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
| `mesh` | `mode`, `path` | Starts `terrain` or `surface` reconstruction to an absolute `.obj` path using undeleted points inside the active section box and visible classification filters; surface mode uses the current 3D surface settings and returns a job ID. The complete job reports the open edges and the connected parts of the mesh |
| `cancel_mesh` | — | Requests cancellation of the running mesh task |
| `export_mesh` | `path` | Saves the mesh the active layer holds to an absolute `.obj`, `.ply` or `.stl` path; the extension chooses the format. Returns a job ID |
| `merge_visible` | `path` | Merges the visible LAS/LAZ layers to an absolute `.las` or `.laz` path; returns a job ID |
| `cancel_merge` | — | Requests cancellation of the running merge task |
| `bag3d` | `bbox`, `lod`, `path` | Downloads the 3D BAG buildings inside an RD New box `[xmin, ymin, xmax, ymax]` of at most 2 by 2 km at level of detail `1.2`, `1.3` or `2.2` to an absolute `.obj` path in an existing folder and opens them as a layer; returns a job ID |
| `cancel_bag3d` | — | Requests cancellation of the running 3D BAG download |
| `list_extensions` | — | Lists the built-in optional features and whether each is enabled |
| `set_extension_enabled` | `id`, `enabled` | Switches a built-in optional feature (`bag3d`) on or off and persists that; `saved` in the answer is false, with `save_error`, when it could not be persisted |
| `file_view` | `open`, optional `page` | Opens the File view, on the page `workspace`, `extensions` or `about` when one is named, or closes it and returns to the model |
| `export` | `path` | Exports the active source, honoring deleted points |
| `export_section` | `path` | Exports only the current section of the active source, honoring deleted points |
| `export_selection` | `path` | Exports exact selected points from the active source, including points outside the preview |
| `export_minus_selection` | `path` | Exports the active source without selected or deleted points |
| `screenshot` | optional `path`, `base64`, `max_edge` | Captures the 3D viewport as a PNG image: written atomically to an absolute `.png` path, replacing a file there, and/or returned as base64 in `png_base64` |

## Exports, stored settings and the server

The destination extension selects PLY, XYZ, PTS, CSV, LAS, LAZ or E57. Export
is atomic and scans the complete source rather than the viewport sample.
`status.result.hidden_classes` lists disabled numeric classification codes.
Color mode, point size, eye-dome settings, point budget and auto-index changes
made through this API also update the native `settings.json` defaults after a
short debounce, so they remain in effect when the app restarts.
The server binds only to loopback, limits request bodies to 64 KiB, and has
no permissive browser CORS headers. After an unclean shutdown, an old
discovery file may remain until the next native launch; clients should check
`/health` and `/info` before using an entry.
