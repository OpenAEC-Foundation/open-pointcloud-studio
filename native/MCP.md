# MCP server

`open-pointcloud-studio --mcp` runs a [Model Context Protocol](https://modelcontextprotocol.io)
server on standard input and output. Its tools drive a running window of the
application through the local [command API](API.md): they open scans, move
the camera, take screenshots of the 3D view, measure, select, save views,
export, draw sections, mesh and detect faces, just as the commands of that
API do.

The server speaks JSON-RPC 2.0 with one message per line. Standard output
carries nothing but those messages; diagnostics go to standard error. The
process keeps its console streams and opens no window itself.

## Starting it from a client

A client starts the server as a child process. Most clients take a JSON
configuration like this one; the name of the outer key differs between
clients:

```json
{
  "mcpServers": {
    "open-pointcloud-studio": {
      "command": "/usr/bin/open-pointcloud-studio",
      "args": ["--mcp"]
    }
  }
}
```

`command` is the full path of the executable, which depends on the system
and on how the application was installed:

| Installed from | `command` |
| --- | --- |
| Windows installer, current user | `C:\Users\NAME\AppData\Local\Programs\Open Pointcloud Studio\open-pointcloud-studio.exe`, with the name of the account for `NAME`, unless another folder was chosen |
| Windows installer, all users | `C:\Program Files\Open Pointcloud Studio\open-pointcloud-studio.exe`, unless another folder was chosen |
| macOS disk image | `/Applications/Open Pointcloud Studio.app/Contents/MacOS/open-pointcloud-studio` |
| Linux package (`.deb`) | `/usr/bin/open-pointcloud-studio` |
| AppImage | The full path of the AppImage file itself |
| zip or `.tar.gz` | The full path of `open-pointcloud-studio` (`.exe` on Windows) in the unpacked folder |

Write the path out: most clients do not fill in `%LOCALAPPDATA%` or `~`. In
JSON every backslash of a Windows path is written twice:
`"C:\\Program Files\\Open Pointcloud Studio\\open-pointcloud-studio.exe"`.

A development build works the same way:
`cargo run -p open-pointcloud-studio-native -- --mcp` from `native/`, or the
built `target/release/open-pointcloud-studio --mcp`.

The server supports the protocol versions `2025-11-25`, `2025-06-18`,
`2025-03-26` and `2024-11-05`. `initialize` answers with the version the
client asks for when it is one of these, and otherwise with `2025-11-25`. It
offers `tools` (`tools/list`, `tools/call`) and answers `ping`. Batches are
accepted. `notifications/cancelled` ends a running wait, and the cancelled
call gets no answer. Unknown methods get error -32601, malformed messages
-32700 or -32600, and unknown tools or `arguments` that are not an object
-32602. Arguments that do not fit a tool's input schema get -32602 as well,
except when `2025-11-25` was agreed: then they are a tool result with
`isError: true`, so the caller can correct them. A whole number written with a
fraction, such as `6.0`, counts as an integer. A command that the window
refuses, or a window that does not answer, is a tool result with
`isError: true` and the reason as text.

Closing the server's standard input ends the session: a running wait stops,
calls still queued are dropped without an answer, and the server exits.

## Which window it drives

Every window writes a discovery file with its process ID, port, token and
start time when its command API starts; see [API.md](API.md). The server
reads those files from the same configuration directory as the window
(`XDG_CONFIG_HOME` is honoured, so a client that sets it for both finds its
own windows only). A file counts when its process still runs and its port
answers `GET /info` as that process; other files are skipped and left alone.

The first tool call that needs a window uses the most recently started live
one. When none runs, the server starts the application itself with
`--api-port` on a free port, as a detached process without console, and waits
up to a minute for its discovery file. That window stays open when the client
or the server ends. A server that runs from a mounted AppImage starts the
window through that AppImage file, because the executable inside it is only
there while the server runs. A server started with `--appimage-extract-and-run`
starts the unpacked executable instead.

`list_instances` lists the live windows and which one is in use;
`select_instance` chooses one by process ID or port, and `start_instance`
starts a new window, optionally opening files, and chooses it. A window that
was picked automatically and has since closed is replaced by the next choice;
a window chosen with `select_instance` or `start_instance` stays chosen, and
calls report an error when it is gone.

## Tools

Positions are scene coordinates in the units of the scans, normally metres.
Angles are radians. Screen positions are viewport pixels from the top-left
corner of the 3D view; `status` reports its size. Most tools send the API
command of the same name with the same arguments, and return the window's
JSON answer as text (and as `structuredContent` from protocol version
`2025-06-18`).

Exports, section drawings and their previews, selections, picks, meshes, mesh
exports, face detections, faces exports, merges and 3D BAG downloads answer
at once with a `job_id`.
They accept `wait_seconds` to wait for the job before answering, and
`wait_for_job` waits for a job by its ID; a job that failed makes the result
an error. `wait_until_idle` waits until imports, octree builds, background
edits, meshing (a closed mesh included), mesh export, a face detection, a
faces export, a section drawing or its preview, a drawing file being read, steps of Mesh to Plans, merging, a 3D BAG download,
point loading for the camera, the fill of the cut of a mesh by the section
box and view snapshots have finished; call it after
`open`, before `screenshot` when the camera changed, and before `export_bcf`.
`screenshot` itself waits up to 4 seconds for the points of the camera and the
fill of the cut, and so does the snapshot of a view. `screenshot` returns MCP image content (`type: "image"`,
`mimeType: "image/png"`, base64 data) followed by a text part with the image
size. While the Drawing view is shown, `screenshot` captures the drawing
instead of the 3D viewport. While the File view, Settings or the card of the Mesh to Plans wizard covers the model, `screenshot` answers
with an error; `file_view` with `open: false` returns to the model, and `mesh_to_plans_view` with `minimized: true` leaves the wizard as a strip above the scene that is not captured. While the
window is minimised there is no picture to take: `screenshot`
then answers with an error, and the snapshot of a view saved meanwhile
follows when that view is restored.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `status` | — | State of the window: layers (each with its mesh and its detected faces), imports and tasks, camera, viewport size, section box, the Section drawing, Closed mesh and Detect faces tools, the Mesh to Plans wizard, selection, measurement, views, settings |
| `job` | `id` | Reads a background job once |
| `wait_for_job` | `id`, optional `timeout_seconds` (default 60) | Waits until the job no longer runs |
| `wait_until_idle` | optional `timeout_seconds` (default 60) | Waits until no work is under way; reports what is still busy |
| `screenshot` | optional `path` (`.png`), `max_edge` (16–8192, default 1920) | Image of the 3D viewport, or of the drawing while the Drawing view is shown |
| `open` | `path` | Opens a file, every supported file in a folder, or a scan project file |
| `cancel_import` | `id` | Cancels a full-stream import |
| `remove` | `index` | Removes a layer |
| `set_active` | `index` | Chooses the active layer |
| `set_visible` | `index`, `visible` | Shows or hides a layer |
| `camera` | `preset` | `top`, `bottom`, `front`, `back`, `left`, `right` or `isometric` |
| `set_camera` | `yaw`, `pitch` (radians), `zoom` (1 frames the model, smaller is closer), `pan` (`[x, y]` pixels), optional `orbit_point` (`[x, y, z]` or `null`) | Sets the orbit camera exactly, and the point it turns about |
| `orbit` | `yaw`, `pitch` (radians) | Turns the orbit camera as a left drag does, about the orbit point |
| `pick_orbit_point` | `pointer` (`[x, y]` pixels) | Makes the drawn point at a viewport pixel the orbit point, as a double click does |
| `zoom_all` | — | Fits the whole model |
| `open_panorama` | `index`, `station` | Stands in a scanner station and shows its photos |
| `set_panorama` | `yaw`, `pitch`, `field_of_view` (radians) | Turns the walking camera |
| `walk` | `eye` (`[x, y, z]`), `yaw`, `pitch` | Places the walking camera |
| `close_panorama` | — | Returns to the orbit view |
| `list_camera_views` | — | Saved views of the active scan |
| `save_camera_view` | optional `name` | Saves the current view, with the section box while it is on, and makes it active |
| `update_camera_view` | `name` | Overwrites a view with the current view |
| `rename_camera_view` | `name`, `new_name` | Renames a view |
| `restore_camera_view` | `name` | Shows a saved view: its camera and its section box, or the box off when it has none |
| `delete_camera_view` | `name` | Deletes a view |
| `add_note` | `point`, `text` | Adds a note to the active view |
| `add_line` | `from`, `to` | Adds a line to the active view |
| `delete_annotation` | `index` | Removes an annotation of the active view |
| `set_annotation_tool` | optional `tool` (`note`, `line` or null) | Chooses or leaves the annotation tool |
| `annotate_screen` | `pointer` | Clicks with the annotation tool at a viewport pixel |
| `submit_note` | `text` | Gives the waiting note its text |
| `export_bcf` | `path` (`.bcf`) | Writes the views of the active scan as BCF |
| `set_theme` | `theme` | `forge`, `light`, `night`, `blueprint` or `contrast`; `openaec`, as `status` reports Night Build, is the same as `night` |
| `set_language` | `language` | `auto` (the language of the system), `en` or `nl`; kept for later sessions |
| `set_color` | `mode` | `rgb`, `elevation`, `intensity` or `classification` |
| `set_class_visible` | `code` (0–255), `visible` | Shows or hides a classification code |
| `set_point_size` | `size` (0.1–20 pixels) | Point size |
| `set_eye_dome` | `enabled` | Eye-dome shading on or off |
| `set_eye_dome_strength` | `strength` (0–5) | Eye-dome strength |
| `set_budget` | `points` (1,000–10,000,000) | Point budget of the viewport |
| `set_section` | `min`, `max`, optional `rotation` | Switches on a section box; `rotation` turns it that many degrees about the vertical through its centre |
| `clear_section` | — | Switches the section box off |
| `set_section_fill` | at least one of `fill_cut`, `color` (`#rrggbb`), `max_thickness` (maximum wall thickness, 0.01–2 m) | Fills the cut of a mesh by the section box between two opposite faces; a single surface gets no fill |
| `align_section_to_walls` | — | Turns the section box along the walls inside it |
| `select_world` | `min`, `max`, optional `wait_seconds` | Selects the exact points in a box; job |
| `pick_screen` | `pointer`, optional `radius` (1–64 pixels), `wait_seconds` | Picks the source point at a viewport pixel; job |
| `cancel_selection` | — | Stops a running selection or pick |
| `clear_selection` | — | Clears the selection |
| `measure` | `mode` (`distance` or `area`), `points` | Sets a measurement and returns its values |
| `clear_measure` | — | Removes the measurement |
| `zoom_selection` | — | Frames the selected points |
| `delete_selection` | — | Hides the selected points |
| `undo_delete` | — | Restores the latest deleted points |
| `redo_delete` | — | Deletes them again |
| `thin` | `percent` (1–100) | Keeps a percentage of the active layer's points |
| `translate` | `offset` | Moves the active layer |
| `scale` | `factors` | Scales the active layer around its centroid |
| `cancel_scale` | — | Cancels a running scale |
| `reset_transform` | — | Undoes move and scale of the active layer |
| `build_index` | — | Builds the octree of the active layer |
| `cancel_index` | — | Cancels the octree build |
| `set_auto_index` | `enabled` | Automatic indexing of large clouds |
| `set_surface_settings` | optional `max_vertices` (3–1,000,000), `neighbors` (3–32), `edge_factor` (above 0), `mesh_size` (0 or more, in the units of the scan) | Sets the settings of the 3D surface; a field left out keeps its value, and when one is refused none changes |
| `mesh` | `mode` (`terrain`, `surface` or `closed`), `path` (`.obj`; for `closed` optional, and `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc`), for `closed` optional `voxel` (0.005–0.5 m or null for automatic), `max_hole` (0–3.2 m), `simplify_mm` (0–1000 or null for automatic), `sample_percent` (0.01–100), `sides` (`automatic`, `centre` or `upward`), `layers` (`active` or `visible`), optional `wait_seconds` | Meshes the active layer, or for `closed` the active layer or every visible layer that reaches the section box (layers of 3D BAG buildings stay out), inside the section box; the job reports vertices, triangles, open edges and connected parts, and for `closed` the distance between points and mesh, where the sides came from and advice. Settings left out keep what the Closed mesh block has |
| `set_closed_mesh_settings` | optional `voxel`, `max_hole`, `simplify_mm`, `sample_percent`, `sides`, `layers`, as for `mesh` | Sets the settings of the Closed mesh block; refused as a whole when one value is |
| `cancel_mesh` | — | Cancels the mesh job |
| `export_mesh` | `path` (`.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc`), optional `wait_seconds` | Saves the mesh of the active layer in the format of the extension; job |
| `set_face_settings` | optional `distance_tolerance` (0.001–0.5 m), `angle_tolerance` (1–45 degrees), `min_area` (0.01–10000 m²), `cylinders`, `layers` (`active` or `visible`), `color` (`face` or `deviation`) | Sets the settings of the Detect faces block and the colouring of the faces that are shown; refused as a whole when one value is |
| `detect_faces` | the settings of `set_face_settings`, all optional, and `wait_seconds` | Finds the flat faces (floors, ceilings, walls, sloped planes) and the round columns and pipes of the active layer, or of every visible layer that reaches the section box, inside the section box, and keeps them with the active layer beside its mesh; job. The job reports the faces per type, the edges, the voxel used and the points on a face. Settings left out keep what the block has |
| `cancel_detect_faces` | — | Cancels the running face detection |
| `list_faces` | optional `boundaries` | The faces of the active layer in scene coordinates: class, plane or axis, area, residuals; with `boundaries` also the outlines and the edges |
| `select_face` | optional `id` (a number from `list_faces`, or null) | Highlights a face in the viewport and the block, or takes the highlight off |
| `export_faces` | `path` (`.json`, `.obj`, `.dxf`, `.dwg` or `.ifc`), optional `wait_seconds` | Saves the faces of the active layer in the format of the extension; job |
| `clear_faces` | — | Removes the faces of the active layer |
| `export` | `path`, optional `wait_seconds` | Exports the active layer; job |
| `export_section` | `path`, optional `wait_seconds` | Exports the section box; job |
| `export_selection` | `path`, optional `wait_seconds` | Exports the selected points; job |
| `export_minus_selection` | `path`, optional `wait_seconds` | Exports all but the selected points; job |
| `export_drawing` | `path` (`.dxf` or `.dwg`), optional `view` (`plan`, `front`, `back`, `left` or `right`), `thickness` (0.005–5 m), `units` (`mm` or `m`), `origin` (`model` or `box`), `fill`, `square`, `grid` (at least 0.005 m), `max_wall_thickness` (above 0, at most 2 m), `color` (`layer` or `rgb`), `point_layers` (`scan` or `class`), `max_points` (1–400,000), `version` (`r2004`, `r2010`, `r2013` or `r2018`), `wait_seconds` | Draws the slab behind one face of the section box as a 2D drawing in DXF or DWG; job. Choices left out keep what the Section drawing block has |
| `preview_drawing` | the choices of `export_drawing` without `path`, optional `wait_seconds` | Traces the filled cut of that slab and lays it over the points in the viewport; job |
| `clear_drawing_preview` | — | Takes the preview of the filled cut off the viewport |
| `drawing_view` | `show` | Shows the Drawing view in place of the 3D scene, or the 3D scene again |
| `open_drawing` | `path` (`.dxf` or `.dwg`), optional `wait_seconds` | Reads a DXF or DWG file into the Drawing view and shows it; job |
| `drawing_zoom_extents` | — | Fits the whole drawing in the Drawing view |
| `set_drawing_layer` | `layer` (name or `*`), `visible` | Shows or hides a layer of the drawing in the Drawing view |
| `create_drawing` | `kind` (`plan`, `elevation` or `section`), optional `basis` (`model`, `section_box` or the name of a saved view with a section box), `side` (`front`, `back`, `left` or `right`), `height`, `position`, `thickness` (0.005–5 m), `name`, `wait_seconds` | Makes a plan, an elevation or a section as Create 2D plan / elevation / section does, shows it and keeps how it was made; job |
| `list_drawings` | — | The drawings of `create_drawing` made from an open scan with how each was made, and the previews, exports and files of this session |
| `show_drawing` | `name` | Shows a drawing of `create_drawing`; one not made in this session yet is made again from its scans, with a job to wait for |
| `delete_drawing` | `name` | Forgets a drawing of `create_drawing` |
| `set_browser_group` | `group` (`scans`, `classes`, `views`, `bcf`, `3d`, `plans`, `elevations`, `sections`, `files` or `folder:` and a path), `open` | Opens or collapses a group of the Project Browser |
| `open_in_cad_viewer` | optional `path` (`.dxf` or `.dwg`) | Opens a DXF or DWG file, by default the last one exported, in Open CAD Studio or the program chosen in Settings, read-only; without a viewer in the system program |
| `cancel_drawing` | — | Cancels the running section drawing or preview |
| `merge_visible` | `path` (`.las` or `.laz`), optional `wait_seconds` | Merges the visible LAS/LAZ layers; job |
| `cancel_merge` | — | Cancels the merge |
| `bag3d` | `bbox` (`[xmin, ymin, xmax, ymax]` in RD New, at most 2 by 2 km), `lod` (`1.2`, `1.3` or `2.2`), `path` (`.obj`), optional `wait_seconds` | Downloads the 3D BAG buildings of an area and opens them as a layer; job |
| `cancel_bag3d` | — | Cancels the 3D BAG download |
| `list_extensions` | — | Built-in optional features and whether each is enabled |
| `set_extension_enabled` | `id` (`bag3d`), `enabled` | Switches a built-in optional feature on or off; kept for later sessions, unless the answer has `saved: false` with `save_error` |
| `file_view` | `open`, optional `page` (`new`, `open`, `import`, `export`, `workspace`, `extensions` or `about`) | Opens the File view, on a page, or returns to the model |
| `mesh_to_plans_view` | `open`, optional `step` (`prepare`, `mesh`, `views`, `walls`, `openings`, `rooms`, `sheet`, `site` or `result`) and `minimized` | Shows the Mesh to Plans wizard as its card or as a strip above the scene, on a step, or takes it away |
| `mesh_to_plans_action` | `action` (`run`, `run_all`, `confirm`, `skip`, `cancel`, `back`, `next` or `resume`), optional `folder` (absolute: for a new project, or the project to resume) | Does what a button of the Mesh to Plans wizard does on the step it shows, or opens a saved project; `run` and `run_all` answer with a `job_id` |
| `mesh_to_plans_level` | optional `level` (its id), `action` (`select`, `show`, `set_peil`, `add`, `merge` or `remove`), `name`, `cut_height` (0.3 to 3 m) and `floor_above_p` | Selects, renames, moves, shows in the model, makes P, adds, merges or removes a level of step 0 of the wizard |
| `list_instances` | — | Running windows and the one in use |
| `select_instance` | `pid` or `port`, at least one | Chooses the window to drive |
| `start_instance` | optional `files` | Starts a new window and chooses it |

`tools/list` gives each tool a description and a JSON Schema of its
arguments with types, ranges, enumerations and units. The tools are listed in
one table in [`desktop/src/mcp/tools.rs`](desktop/src/mcp/tools.rs); a new
API command needs one entry there and one line here.

## Example session

Each line below is one message; `>` is sent by the client and `<` by the
server (shortened).

```text
> {"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"script","version":"1"}}}
< {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"open-pointcloud-studio",...}}}
> {"jsonrpc":"2.0","method":"notifications/initialized"}
> {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open","arguments":{"path":"/scans/site.e57"}}}
< {"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"{\"accepted\":true,...}"}],"isError":false,...}}
> {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"wait_until_idle","arguments":{"timeout_seconds":120}}}
> {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"camera","arguments":{"preset":"top"}}}
> {"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"screenshot","arguments":{"max_edge":1024}}}
< {"jsonrpc":"2.0","id":5,"result":{"content":[{"type":"image","data":"iVBORw0KGgo...","mimeType":"image/png"},{"type":"text","text":"{\"width\":1024,...}"}],"isError":false,...}}
```
