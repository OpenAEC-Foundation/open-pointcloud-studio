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

The cloud API has an MCP server of its own, over HTTP and without a window;
see [The cloud API over HTTP](#the-cloud-api-over-http).

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
refuses is a tool result with `isError: true` whose text is the window's
JSON answer with `error`; a window that cannot be reached or does not answer
gives `isError: true` with the reason as plain text.

A command that the window does not answer within 10 seconds gets HTTP 504
from the command API, and the tool result says so. That does not mean the
command was not carried out: it waits in the window's queue and is carried
out in its turn. Call `status` before calling a tool that changes something
again, or it may be done twice.

Tool calls are carried out one after another, in the order they arrive; a
call that waits (`wait_seconds`, `wait_for_job`, `wait_until_idle`) delays
the calls after it until it ends or is cancelled. Prefer short waits and
repeat them, within the client's own request timeout. Batches are carried
out in the same way, and `initialize`, `ping` and `tools/list` are answered
at once.

Closing the server's standard input ends the session: a running wait stops,
calls still queued are dropped without an answer, and the server exits.

## Which window it drives

Every window writes a discovery file with its process ID, port, token and
start time when its command API starts; see [API.md](API.md). The server
reads those files from the same configuration directory as the window
(`XDG_CONFIG_HOME` is honoured, so a client that sets it for both finds its
own windows only). A file counts when its process still runs and its port
answers `GET /info` as that process; other files are skipped and left alone.
The server waits up to 12 seconds for that answer, so that a window of an
older version, which answered one request at a time, is not taken for a
closed one while it carries out a slow command.

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
calls report an error when it is gone. When the window in use runs another
version than the server, the server says so on standard error: the window
refuses a tool whose command it does not know, with HTTP 400 and
`unknown variant`. `GET /info` lists the commands a window knows.

## Tools

Positions are scene coordinates in the units of the scans, normally metres.
Angles are radians. Screen positions are viewport pixels from the top-left
corner of the 3D view; `status` reports its size. Most tools send the API
command of the same name with the same arguments, and return the window's
JSON answer as text (and as `structuredContent` from protocol version
`2025-06-18`).

The annotations of each tool in `tools/list` say what it does to its
surroundings. `readOnlyHint` is true for the tools that only read: `status`,
`job`, `context`, the `list_` tools and the waits. Of the others,
`destructiveHint` is true for those that may take away or overwrite work or
files (see [Changes and undo](#changes-and-undo)): removing a layer,
deleting or thinning points, resetting the transform of a layer, giving or
taking away photo colours, a new mesh or face detection, clearing faces,
overwriting or deleting views, deleting drawings, sheets, viewports and
annotations, the exports and downloads that replace a file at their path,
and installing, running and stopping extensions. `texture_mesh` writes new
files only, and `screenshot` is not counted, although with `path` it
replaces a file there. `idempotentHint` is true for
`camera` and the `set_` and `show_` tools. `openWorldHint` is true for
`bag3d`, which downloads over the internet, and for `run_extension` and
`open_in_cad_viewer`, which start other programs.

Exports, section drawings and their previews, selections, picks, meshes, mesh
exports, face detections, faces exports, colourings from photos, merges,
3D BAG downloads and the dialogs of `choose_path` answer at once with a `job_id`.
They accept `wait_seconds` to wait for the job before answering, and
`wait_for_job` waits for a job by its ID; a job that failed makes the result
an error. `wait_until_idle` waits until imports, octree builds, background
edits, meshing (a closed mesh included), mesh export, texture export, a face detection, a
faces export, a section drawing or its preview, a drawing file being read, a sheet being written as PDF, a drawing being written as DXF or DWG, steps of Pointcloud to Drawing, a colouring from photos, merging, a 3D BAG download,
point loading for the camera, the fill of the cut of a mesh by the section
box, the listing and decoding of photos and view snapshots have finished; call it after
`open`, before `screenshot` when the camera changed, and before `export_bcf`.
`screenshot` itself waits up to 4 seconds for the points of the camera, the
fill of the cut and the photo that is entered, and so does the snapshot of a view. `screenshot` returns MCP image content (`type: "image"`,
`mimeType: "image/png"`, base64 data) followed by a text part with the image
size. While the Drawing view is shown, `screenshot` captures the drawing
instead of the 3D viewport. While the File view, Settings, the card of the Pointcloud to Drawing wizard or the card of Mesh Pointcloud covers the model, `screenshot` answers
with an error; `file_view` with `open: false` returns to the model, `mesh_to_plans_view` with `minimized: true` leaves the wizard as a strip above the scene that is not captured, and `mesh_wizard` with `open: false` takes the card of Mesh Pointcloud away. While the
window is minimised there is no picture to take: `screenshot`
then answers with an error, and the snapshot of a view saved meanwhile
follows when that view is restored.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `status` | — | State of the window: layers (each with its mesh, its detected faces and its points with photo colours), imports and tasks, camera, viewport size, section box, the Section drawing, Closed mesh, Detect faces and Colour from photos tools, the Pointcloud to Drawing wizard, the card of Mesh Pointcloud, selection, measurement, views, photos, settings |
| `job` | `id` | Reads a background job once |
| `wait_for_job` | `id`, optional `timeout_seconds` (default 60) | Waits until the job no longer runs |
| `wait_until_idle` | optional `timeout_seconds` (default 60) | Waits until no work is under way; reports what is still busy |
| `screenshot` | optional `path` (`.png`), `max_edge` (16–8192, default 1920), `window` | Image of the 3D viewport, or of the drawing while the Drawing view is shown, or with `window` of the whole window |
| `open` | `path` | Opens a file, every supported file in a folder, or a scan project file |
| `cancel_import` | `id` | Cancels a full-stream import |
| `remove` | `index` | Removes a layer, and empties the undo history of every layer |
| `set_active` | `index` | Chooses the active layer |
| `set_visible` | `index`, `visible` | Shows or hides a layer |
| `camera` | `preset` | `top`, `bottom`, `front`, `back`, `left`, `right` or `isometric` |
| `set_camera` | `yaw`, `pitch` (radians), `zoom` (1 frames the model, smaller is closer), `pan` (`[x, y]` pixels), optional `orbit_point` (`[x, y, z]` or `null`) | Sets the orbit camera exactly, and the point it turns about |
| `orbit` | `yaw`, `pitch` (radians) | Turns the orbit camera as a left drag does, about the orbit point |
| `pick_orbit_point` | `pointer` (`[x, y]` pixels) | Makes the drawn point at a viewport pixel the orbit point, as a double click does |
| `zoom_all` | — | Fits the whole model |
| `fit_scan_poses` | — | Frames the visible scanner stations and points together and shows the markers |
| `set_scan_poses_visible` | `visible` | Shows or hides the scanner station markers |
| `open_panorama` | `index`, `station` | Stands in a scanner station and shows its photos |
| `set_panorama` | `yaw`, `pitch`, `field_of_view` (radians) | Turns the walking camera |
| `walk` | `eye` (`[x, y, z]`), `yaw`, `pitch` | Places the walking camera |
| `close_panorama` | — | Returns to the orbit view; after `enter_photo`, to the camera before the first photo, into the station panorama it stood in then |
| `list_photos` | optional `layer` | The photos of a layer apart from its station photos (panoramas and photos along a path): index, kind, size, position, viewing direction; and the file's coordinate system |
| `enter_photo` | `index`, optional `layer` | Stands where a photo was taken and lays it over the points |
| `photo_blend` | `value` (0–1) | How much of the photo covers the points: 0 the points only, 1 the photo only |
| `set_photo_compare` | `enabled` | Shows the entered photo beside a live 3D pane with its own camera controls; measuring and picking remain active in the photo pane |
| `set_photo_split` | `ratio`, optional `vertical` | Sets the 3D pane's share of the width or height from 0.25 to 0.75; `vertical: true` stacks the panes, and the GUI divider can also be dragged |
| `next_photo` | — | The next photo along the path, as Page Down |
| `previous_photo` | — | The previous photo along the path, as Page Up |
| `colour_from_photos` | optional `layer`, `max_distance` (0.5–500 m, default 20), `blend` (default true) and `wait_seconds` | Gives the points of a layer the colours its photos see them with, the photos of its file and of its stations: the remaining points in the section box and class filters, or all; a photo colours a point that no point of the layer hides from it, within `max_distance`, the nearest first or, with `blend`, weighted to the nearest; job. The colours take the place of those of the file in the viewport and in exports, as an edit `undo_delete` takes back; Undo keeps at most 1 GiB of earlier photo colours and lets go of the oldest edits beyond that. The job reports the points, those coloured and the share no photo sees, the photos in reach and used, the times, and against the colours the points had the mean and median difference per channel |
| `cancel_colour_from_photos` | — | Cancels the running colouring; the colours stay as they were |
| `clear_photo_colours` | optional `layer` | Takes the photo colours of a layer (default the active one) away; `undo_delete` brings them back |
| `list_camera_views` | — | Saved views of the active scan |
| `save_camera_view` | optional `name` | Saves the current 3D view, with the section box while it is on, and makes it active; shows the 3D scene when a drawing was shown |
| `update_camera_view` | `name` | Overwrites a view with the current 3D view; shows the 3D scene when a drawing was shown |
| `rename_camera_view` | `name`, `new_name` | Renames a view |
| `restore_camera_view` | `name` | Shows a saved view: its camera and its section box, or the box off when it has none |
| `delete_camera_view` | `name` | Deletes a view |
| `add_note` | `point`, `text` | Adds a note to the active view |
| `add_line` | `from`, `to` | Adds a line to the active view |
| `delete_annotation` | `index`, or `id` with optional `drawing` or `sheet` | Removes an annotation of the active view by its place, or of a drawing or a sheet by its id |
| `set_annotation_tool` | optional `tool` (`note`, `line` or null) | Chooses or leaves the annotation tool |
| `annotate_screen` | `pointer` | Clicks with the annotation tool at a viewport pixel |
| `submit_note` | `text` | Gives the waiting note its text |
| `export_bcf` | `path` (`.bcf`) | Writes the views of the active scan as BCF |
| `set_theme` | `theme` | `light` (Light), `forge` (Forge (dark)), `openaec` (OpenAEC (dark)), `blueprint` (Blueprint) or `contrast` (High contrast); `night` is the same as `openaec`, which is what `status` reports |
| `set_language` | `language` | `auto` (the language of the system), `en` or `nl`; kept for later sessions |
| `set_ribbon_tab` | `tab` | `start` shows the single ribbon; legacy `view` opens the view-cube controls |
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
| `undo_delete` | — | Takes back the latest edit: restores the latest deleted or thinned points, or the colours a layer had before photo colours were given or removed |
| `redo_delete` | — | Does that edit again |
| `thin` | `percent` (1–100) | Keeps a percentage of the active layer's points |
| `translate` | `offset` | Moves the active layer |
| `scale` | `factors` | Scales the active layer around its centroid |
| `cancel_scale` | — | Cancels a running scale |
| `reset_transform` | — | Undoes move and scale of the active layer |
| `build_index` | — | Builds the octree of the active layer, or queues it while every place is taken |
| `cancel_index` | — | Cancels the octree builds and empties the queue |
| `set_auto_index` | `enabled` | Automatic indexing of large clouds |
| `set_surface_settings` | optional `max_vertices` (3–1,000,000), `neighbors` (3–32), `edge_factor` (above 0), `mesh_size` (0 or more, in the units of the scan) | Sets the settings of the 3D surface, its options in Mesh Pointcloud; a field left out keeps its value, and when one is refused none changes |
| `mesh` | `mode` (`terrain`, `surface` or `closed`), `path` (`.obj`; for `closed` optional, and `.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc`), for `closed` optional `voxel` (0.005–0.5 m or null for automatic), `max_hole` (0–3.2 m), `simplify_mm` (0–1000 or null for automatic), `sample_percent` (0.01–100), `sides` (`automatic`, `centre` or `upward`), `layers` (`active` or `visible`), optional `wait_seconds` | Meshes the active layer, or for `closed` the active layer or every visible layer that reaches the section box (layers of 3D BAG buildings stay out), inside the section box; the job reports vertices, triangles, open edges and connected parts, and for `closed` the distance between points and mesh, where the sides came from and advice. Settings left out keep what the options of a closed mesh in Mesh Pointcloud have |
| `set_closed_mesh_settings` | optional `voxel`, `max_hole`, `simplify_mm`, `sample_percent`, `sides`, `layers`, as for `mesh` | Sets the options of a closed mesh in Mesh Pointcloud; refused as a whole when one value is |
| `cancel_mesh` | — | Cancels the mesh job |
| `export_mesh` | `path` (`.obj`, `.ply`, `.stl`, `.dxf`, `.dwg` or `.ifc`), optional `wait_seconds` | Saves the mesh of the active layer in the format of the extension; job |
| `texture_mesh` | new absolute `.obj` `path`, optional `texel_size` (metres), `max_resolution` (256–4096 pixels), `wait_seconds` | Writes the active mesh's stored vertex colours to UV atlas PNG pages with an OBJ and MTL; job. Colours applied to points from E57 photos transfer to newly reconstructed surface meshes. Original E57 photo pixels are not projected directly onto texels |
| `cancel_texture_mesh` | — | Cancels the running texture export and removes its staged files |
| `set_face_settings` | optional `distance_tolerance` (0.001–0.5 m), `angle_tolerance` (1–45 degrees), `min_area` (0.01–10000 m²), `cylinders`, `layers` (`active` or `visible`), `color` (`face` or `deviation`) | Sets the options of the faces in Mesh Pointcloud and the colouring of the faces that are shown; refused as a whole when one value is |
| `detect_faces` | the settings of `set_face_settings`, all optional, and `wait_seconds` | Finds the flat faces (floors, ceilings, walls, sloped planes) and the round columns and pipes of the active layer, or of every visible layer that reaches the section box, inside the section box, and keeps them with the active layer beside its mesh; job. The job reports the faces per type, the edges, the voxel used and the points on a face. Settings left out keep what the options have |
| `cancel_detect_faces` | — | Cancels the running face detection |
| `list_faces` | optional `boundaries` | The faces of the active layer in scene coordinates: class, plane or axis, area, residuals; with `boundaries` also the outlines and the edges |
| `select_face` | optional `id` (a number from `list_faces`, or null) | Highlights a face in the viewport and in the list in Properties, or takes the highlight off |
| `export_faces` | `path` (`.json`, `.obj`, `.dxf`, `.dwg` or `.ifc`), optional `wait_seconds` | Saves the faces of the active layer in the format of the extension; job |
| `clear_faces` | — | Removes the faces of the active layer |
| `export` | `path`, optional `wait_seconds` | Exports the active layer; job |
| `export_section` | `path`, optional `wait_seconds` | Exports the section box; job |
| `export_selection` | `path`, optional `wait_seconds` | Exports the selected points; job |
| `export_minus_selection` | `path`, optional `wait_seconds` | Exports all but the selected points; job |
| `export_drawing` | `path` (`.dxf` or `.dwg`), optional `view` (`plan`, `front`, `back`, `left` or `right`), `thickness` (0.005–5 m), `units` (`mm` or `m`), `origin` (`model` or `box`), `fill`, `square`, `straight_lines` (editable straight polylines on `OPS-STRAIGHT-LINES` beside the cut outlines), `line_tolerance` (0–1 m, default 0.01), `line_min_length` (0–10 m, default 0.10), `grid` (at least 0.005 m), `max_wall_thickness` (above 0, at most 2 m), `color` (`layer` or `rgb`), `point_layers` (`scan` or `class`), `max_points` (1–400,000), `version` (`r2004`, `r2010`, `r2013` or `r2018`), `wait_seconds` | Draws the slab behind one face of the section box as a 2D drawing in DXF or DWG; job. Choices left out keep what the Section drawing block has |
| `preview_drawing` | the choices of `export_drawing` without `path`, optional `wait_seconds` | Traces the filled cut of that slab and lays it over the points in the viewport; job |
| `clear_drawing_preview` | — | Takes the preview of the filled cut off the viewport |
| `drawing_view` | `show` | Shows the Drawing view in place of the 3D scene, or the 3D scene again |
| `set_section_drawing_panel` | `open` | Opens or closes the Section drawing controls in Properties; opening needs a section box |
| `open_drawing` | `path` (`.dxf` or `.dwg`), optional `wait_seconds` | Reads a DXF or DWG file into the Drawing view and shows it; job |
| `drawing_zoom_extents` | — | Fits the whole drawing in the Drawing view |
| `set_drawing_layer` | `layer` (name or `*`), `visible` | Shows or hides a layer of the drawing in the Drawing view |
| `create_drawing` | `kind` (`plan`, `elevation` or `section`), optional `basis` (`model`, `section_box` or the name of a saved view with a section box), `side` (`front`, `back`, `left` or `right`), `height`, `position`, `thickness` (0.005–5 m), `sample_percent` (the points used, 0.1–100; 10 by default), `name`, `straight_lines`, `line_tolerance`, `line_min_length` (as for `export_drawing`), `square` (default true), `wait_seconds` | Makes a plan, an elevation or a section as Create 2D plan / elevation / section does, shows it and keeps how it was made; job |
| `list_drawings` | — | The drawings of `create_drawing` made from an open scan with how each was made, and the previews, exports and files of this session |
| `show_drawing` | `name` | Shows a drawing of `create_drawing`; one not made in this session yet is made again from its scans, with a job to wait for. `3D model` shows the 3D model |
| `delete_drawing` | `name` | Forgets a drawing of `create_drawing` |
| `set_browser_group` | `group` (`scans`, `classes`, `views`, `sheets`, `bcf`, `3d`, `plans`, `elevations`, `sections`, `files` or `folder:` and a path), `open` | Opens or collapses a group of the Project Browser |
| `list_tabs` | — | The tabs above the main area in their order, the 3D model first, with the active one; a tab of a sheet has the kind `sheet`, and each tab says whether it is `locked` |
| `show_tab` | `name` or `index` as `list_tabs` gives them | Shows an open tab as a click on it does; a drawing not made in this session yet is made, with a job to wait for. Refused while Settings, the dialog of Create 2D or the card of the Pointcloud to Drawing wizard is open |
| `close_tab` | `name` or `index` as `list_tabs` gives them | Closes a tab as its × does; its view or drawing stays, and the tab of the 3D model does not close |
| `set_sheet_crop` | optional `name`, `rect` (`[[left, bottom], [right, top]]` in drawing units), `width`, `height`, `center`, `rotation`, `cut`, `depth`, `sample_percent` (0.1–100) | Sets the crop region of a drawing of `create_drawing` and makes it again in place, from the points it read before when its cut, depth and points used stay |
| `select_crop_region` | `selected` | Selects the crop region of the drawing shown, as a click on its outline does, so that Properties shows its figures at the top and its handles can be dragged; `false` deselects it, as Escape does |
| `drag_crop_handle` | `handle` (`left`, `right`, `bottom`, `top`, `bottom_left`, `bottom_right`, `top_left` or `top_right`), `to` (`[u, v]` in drawing units), optional `release` | Drags a handle of the crop region of the drawing shown, as the pointer does |
| `duplicate_view` | `name`, optional `kind` (`model`, `view` or `drawing`) | Duplicates the 3D model, a saved view or a drawing under VIEWS, right below it, and shows the copy |
| `rotate_crop` | optional `name`, `degrees`, `apply` | Turns the crop region of a plan, or the section box in the 3D view, as R and then O do |
| `list_sheets` | — | Lists the sheets of SHEETS with their paper, title block and the views placed on them, each with what it shows |
| `create_sheet` | optional `number`, `name`, `paper` (`a4` to `a0`), `orientation`, `project`, `date`, `drawn_by` | Makes a sheet, by default A3 lying, and shows it |
| `update_sheet` | optional `sheet`, `number`, `name`, `paper`, `orientation`, `project`, `date`, `drawn_by` | Changes a sheet and its title block |
| `duplicate_sheet` | optional `sheet` | Duplicates a sheet with its views and shows the copy |
| `delete_sheet` | optional `sheet` | Deletes a sheet; its views and drawings stay |
| `show_sheet` | optional `sheet` | Shows a sheet in its tab and makes the drawings on it that are not made yet |
| `place_view` | optional `sheet`, `name`, optional `kind`, `at`, `scale` | Places a saved 3D view or a drawing on a sheet; a drawing at 1:100 unless given |
| `update_viewport` | optional `sheet`, `viewport`, optional `at`, `size`, `scale`, `title` | Moves a viewport, sets the scale of a drawing, the size of a 3D view or the title |
| `remove_viewport` | optional `sheet`, `viewport` | Takes a viewport off a sheet |
| `export_sheet_pdf` | optional `sheet`, `path` (`.pdf`), optional `wait_seconds` | Writes a sheet as a PDF of the size of its paper; a job |
| `lock_view` | optional `name`, `kind` (`view`, `drawing` or `viewport`), `sheet`, `viewport` | Locks a saved 3D view, a drawing or a viewport on a sheet |
| `unlock_view` | optional `name`, `kind`, `sheet`, `viewport` | Unlocks it again |
| `annotate_drawing` | optional `name`, `kind` (`text`, `dimension`, `leader` or `line`), optional `at`, `from`, `to`, `offset`, `text`, `height`, `snap` | Places a text, a dimension, a leader or a line on a drawing; points snap to its lines |
| `annotate_sheet` | optional `sheet`, `kind` (`text` or `line`), optional `at`, `from`, `to`, `text`, `height` | Places a text or a line on the paper of a sheet |
| `export_drawing_file` | optional `name`, `path` (`.dxf` or `.dwg`), optional `wait_seconds` | Writes a drawing with its annotations as DXF or DWG, with real dimensions; a job |
| `open_in_cad_viewer` | optional `path` (`.dxf` or `.dwg`) | Opens a DXF or DWG file, by default the last one exported, read-only in the Open CAD Studio that comes with the application, else in the program chosen in Settings or an installed Open CAD Studio; without a viewer in the system program |
| `cancel_drawing` | — | Cancels the running section drawing or preview |
| `merge_visible` | `path` (`.las` or `.laz`), optional `wait_seconds` | Merges the visible LAS/LAZ layers; job |
| `cancel_merge` | — | Cancels the merge |
| `bag3d` | `bbox` (`[xmin, ymin, xmax, ymax]` in RD New, at most 2 by 2 km), `lod` (`1.2`, `1.3` or `2.2`), `path` (`.obj`), optional `wait_seconds` | Downloads the 3D BAG buildings of an area and opens them as a layer; job |
| `cancel_bag3d` | — | Cancels the 3D BAG download |
| `list_extensions` | — | The built-in optional features and the installed extensions, with what each declares, whether it is enabled and its run under way; `problems` lists installed extensions that could not be read |
| `set_extension_enabled` | `id` (`bag3d` or the id of an installed extension), `enabled` | Switches an extension on or off; kept for later sessions, unless the answer has `saved: false` with `save_error`. Switching an installed one off stops its run |
| `install_extension` | `path` (a folder, its `extension.json` or a `.zip`) | Checks an extension and shows the user what it declares in the window, asking to confirm the install; only the user can confirm |
| `run_extension` | `id`, optional `entry` (a button or tile of it) | Starts an installed extension as its button does; answers with the run and its log |
| `stop_extension` | `id` | Stops the run of an extension and what it started |
| `show_message` | `text` (1–300 characters) | Shows a message in the status bar; one from a run starts with the name of the extension |
| `report_progress` | `percent` (0–100), optional `text` | Shows how far a task is: beside the name of a run in the status bar, or in the status line |
| `choose_path` | `mode` (`open`, `save` or `folder`), optional `title`, `filters` (`[{"name", "extensions"}]`), `file_name`, `directory`, `wait_seconds` | Asks the user for a path with a dialog of the window; job, complete with the `path` or cancelled |
| `context` | — | What the window shows: the active scan, the selection, the section box, the tab shown |
| `file_view` | `open`, optional `page` (`new`, `open`, `import`, `export`, `workspace`, `extensions` or `about`) | Opens the File view, on a page, or returns to the model |
| `mesh_wizard` | `open`, optional `step` (`method`, `options` or `run`) and `method` (`closed`, `terrain`, `surface` or `faces`) | Shows the card of Mesh Pointcloud, on a step and with a method, or takes it away; a job of the card goes on, and with `open: true` the card opens on the Run step of a job that runs |
| `mesh_to_plans_view` | `open`, optional `step` (`prepare`, `mesh`, `views`, `walls`, `openings`, `rooms`, `sheet`, `site` or `result`) and `minimized` | Shows the Pointcloud to Drawing wizard as its card or as a strip above the scene, on a step, or takes it away |
| `mesh_to_plans_action` | `action` (`run`, `run_all`, `confirm`, `skip`, `cancel`, `back`, `next` or `resume`), optional `folder` (absolute: for a new project, or the project to resume) | Does what a button of the Pointcloud to Drawing wizard does on the step it shows, or opens a saved project; `run` and `run_all` answer with a `job_id` |
| `mesh_to_plans_level` | optional `level` (its id), `action` (`select`, `show`, `set_peil`, `add`, `merge` or `remove`), `name`, `cut_height` (0.3 to 3 m) and `floor_above_p` | Selects, renames, moves, shows in the model, makes P, adds, merges or removes a level of step 0 of the wizard |
| `list_instances` | — | Running windows and the one in use |
| `select_instance` | `pid` or `port`, at least one | Chooses the window to drive |
| `start_instance` | optional `files` | Starts a new window and chooses it |

`tools/list` gives each tool a description and a JSON Schema of its
arguments with types, ranges, enumerations and units. The tools are listed in
one table in [`desktop/src/mcp/tools.rs`](desktop/src/mcp/tools.rs); a new
API command needs one entry there and one line here.

## Changes and undo

`undo_delete` takes back the latest edit and `redo_delete` does it again.
Four tools make such an edit: `delete_selection`, `thin`,
`colour_from_photos` and `clear_photo_colours`. At most eight edits are
kept, of all layers together, and a new edit ends what `redo_delete` could
do again. Earlier photo colours are kept up to about 1 GiB; beyond that a
colouring lets go of the oldest edits. `remove` of any layer empties this
history for every layer.

`translate` and `scale` are taken back with `reset_transform`, which puts
the active layer back at the coordinates of its source; `reset_transform`
itself is not taken back.

Nothing else is taken back by Undo. A new mesh or a new face detection takes
the place of what the layer had, and what `clear_faces`,
`delete_camera_view`, `update_camera_view`, `delete_drawing`,
`delete_sheet`, `remove_viewport` and `delete_annotation` take away or
overwrite does not come back. The edits change the scans in the window, not
their files; an export replaces the file at its destination, whatever file
that is.

## The cloud API over HTTP

The cloud API, `open-pointcloud-studio-api`
([docs/cloud-api.md](../docs/cloud-api.md)), serves MCP at `POST /mcp`, on
the same port as its REST routes. It differs from the server above:

- **Transport.** Streamable HTTP: one JSON-RPC message per request,
  answered as JSON. A notification gets 202, a batch is refused, and
  `GET /mcp` gets 405. The protocol versions are `2025-11-25`, `2025-06-18`
  and `2025-03-26`.
- **No state and no window.** The service keeps no session and nothing
  between calls. A call carries its file in base64 in `input`, and a result
  file comes back as an embedded resource of the tool result.
- **Keys.** Every request needs `Authorization: Bearer <key>`; the service
  holds the SHA-256 of each key in `OPENAEC_API_SLEUTELS`. Without a valid
  key it answers 401 `TOEGANG_GEWEIGERD`, and a request with an `Origin` it
  does not list gets 403 `HERKOMST_GEWEIGERD`.
- **Five tools**, all read-only and not open-world: `inspect`, `convert`,
  `section_drawing`, `terrain_mesh` and `surface_mesh`. They take no path,
  and nothing they do stays on the server, so there is nothing to undo.
  Their names differ from the tools above on purpose: those act on the
  active layer, the section box and the filters of a window and write to a
  path.

A client that speaks Streamable HTTP is configured with the address and the
key:

```json
{"type": "http", "url": "https://HOST/mcp", "headers": {"Authorization": "Bearer <key>"}}
```

[MCP client](../docs/cloud-api.md#mcp-client) in docs/cloud-api.md says how
the service answers, and [Limits](../docs/cloud-api.md#limits) how large a
file may be.

## Checking a server from outside

`packaging/mcp-client-check.py` checks either server as an MCP client
outside the project does, with the reference Python SDK of the protocol,
pinned with hashes in `packaging/mcp-client-requirements.txt`. From the root
of the repository:

```bash
python3 -m venv /tmp/mcp
/tmp/mcp/bin/pip install --require-hashes -r native/packaging/mcp-client-requirements.txt
/tmp/mcp/bin/python -I native/packaging/mcp-client-check.py --stdio /usr/bin/open-pointcloud-studio --version VERSION
/tmp/mcp/bin/python -I native/packaging/mcp-client-check.py --http http://127.0.0.1:8080/mcp --key KEY --version VERSION
```

`--stdio` starts the server of an installed application with configuration
and cache folders of its own. The server starts a window, so it needs a
display; a virtual one will do. It checks the handshake, that the server
offers tools and no resources, the annotations of `status`, `thin` and
`bag3d`, that every input schema refuses unknown arguments, `thin` 50 with
`undo_delete`, `redo_delete` and `undo_delete` again on a grid of 1000
points, a tool error for a missing file, and the token of the command API,
and it records the count of the tools. `--http` checks the refusal without
the key, the refusal of a foreign `Origin`, the five read-only tools,
`inspect`, `convert` to LAZ as an embedded resource, and the refusal of a
path.

Each writes a record in Markdown, and exits with 1 when a check fails. The
Packages workflow runs `--stdio` against the installed `.deb` on Linux, in
the job `verify-linux`, and the Container workflow runs `--http` against the
image. [docs/ecosystem.md](../docs/ecosystem.md#mcp-verification-record)
records what has run.

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
