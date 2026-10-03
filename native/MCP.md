# MCP server

`open-pointcloud-studio --mcp` runs a [Model Context Protocol](https://modelcontextprotocol.io)
server on standard input and output. Its tools drive a running window of the
application through the local [command API](API.md): they open scans, move
the camera, take screenshots of the 3D view, measure, select, save views and
export, just as the commands of that API do.

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
      "command": "C:\\Program Files\\Open Pointcloud Studio\\open-pointcloud-studio.exe",
      "args": ["--mcp"]
    }
  }
}
```

On Linux and macOS, `command` is the path of the `open-pointcloud-studio`
binary. A development build works the same way:
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

Exports, selections, picks, meshes, merges and 3D BAG downloads answer at
once with a `job_id`.
They accept `wait_seconds` to wait for the job before answering, and
`wait_for_job` waits for a job by its ID; a job that failed makes the result
an error. `wait_until_idle` waits until imports, octree builds, background
edits, meshing, mesh export, merging, a 3D BAG download, point loading for the
camera and view snapshots have finished; call it
after `open`, before `screenshot` when the camera changed, and before
`export_bcf`. `screenshot` returns MCP image content (`type: "image"`,
`mimeType: "image/png"`, base64 data) followed by a text part with the image
size. While the File view or Settings covers the model, `screenshot` answers
with an error; `file_view` with `open: false` returns to the model. While the
window is minimised there is no picture to take: `screenshot`
then answers with an error, and the snapshot of a view saved meanwhile
follows when that view is restored.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `status` | — | State of the window: layers, imports and tasks, camera, viewport size, section box, selection, measurement, views, settings |
| `job` | `id` | Reads a background job once |
| `wait_for_job` | `id`, optional `timeout_seconds` (default 60) | Waits until the job no longer runs |
| `wait_until_idle` | optional `timeout_seconds` (default 60) | Waits until no work is under way; reports what is still busy |
| `screenshot` | optional `path` (`.png`), `max_edge` (16–8192, default 1920) | Image of the 3D viewport |
| `open` | `path` | Opens a file, every supported file in a folder, or a scan project file |
| `cancel_import` | `id` | Cancels a full-stream import |
| `remove` | `index` | Removes a layer |
| `set_active` | `index` | Chooses the active layer |
| `set_visible` | `index`, `visible` | Shows or hides a layer |
| `camera` | `preset` | `top`, `bottom`, `front`, `back`, `left`, `right` or `isometric` |
| `set_camera` | `yaw`, `pitch` (radians), `zoom` (1 frames the model, smaller is closer), `pan` (`[x, y]` pixels) | Sets the orbit camera exactly |
| `zoom_all` | — | Fits the whole model |
| `open_panorama` | `index`, `station` | Stands in a scanner station and shows its photos |
| `set_panorama` | `yaw`, `pitch`, `field_of_view` (radians) | Turns the walking camera |
| `walk` | `eye` (`[x, y, z]`), `yaw`, `pitch` | Places the walking camera |
| `close_panorama` | — | Returns to the orbit view |
| `list_camera_views` | — | Saved views of the active scan |
| `save_camera_view` | optional `name` | Saves the current view and makes it active |
| `update_camera_view` | `name` | Overwrites a view with the current view |
| `rename_camera_view` | `name`, `new_name` | Renames a view |
| `restore_camera_view` | `name` | Shows a saved view |
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
| `set_section` | `min`, `max` | Switches on a section box |
| `clear_section` | — | Switches the section box off |
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
| `set_surface_settings` | `max_vertices`, `neighbors`, `edge_factor` | Limits of 3D surface reconstruction |
| `mesh` | `mode` (`terrain` or `surface`), `path` (`.obj`), optional `wait_seconds` | Meshes the active layer; job |
| `cancel_mesh` | — | Cancels the mesh job |
| `export` | `path`, optional `wait_seconds` | Exports the active layer; job |
| `export_section` | `path`, optional `wait_seconds` | Exports the section box; job |
| `export_selection` | `path`, optional `wait_seconds` | Exports the selected points; job |
| `export_minus_selection` | `path`, optional `wait_seconds` | Exports all but the selected points; job |
| `merge_visible` | `path` (`.las` or `.laz`), optional `wait_seconds` | Merges the visible LAS/LAZ layers; job |
| `cancel_merge` | — | Cancels the merge |
| `bag3d` | `bbox` (`[xmin, ymin, xmax, ymax]` in RD New, at most 2 by 2 km), `lod` (`1.2`, `1.3` or `2.2`), `path` (`.obj`), optional `wait_seconds` | Downloads the 3D BAG buildings of an area and opens them as a layer; job |
| `cancel_bag3d` | — | Cancels the 3D BAG download |
| `list_extensions` | — | Built-in optional features and whether each is enabled |
| `set_extension_enabled` | `id` (`bag3d`), `enabled` | Switches a built-in optional feature on or off; kept for later sessions, unless the answer has `saved: false` with `save_error` |
| `file_view` | `open`, optional `page` (`workspace`, `extensions` or `about`) | Opens the File view, on a page, or returns to the model |
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
