//! The tools of the MCP server: one table with every tool, its arguments and
//! how it is carried out, mostly by sending the command API command of the
//! same name to a running window.

use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::schema::{
    boolean, choice, integer_in, list, number_from, number_in, numbers, object, optional, ordinal,
    path, pixel, positive, positive_up_to, required, text, validate_arguments,
    whole_numbers_as_integers, xyz, Argument,
};

/// Where tool calls go: the command API of a running window, and the
/// instances that can be chosen.
pub trait Link: Send {
    /// Send one command to the chosen instance and return its JSON answer.
    fn exec(&mut self, command: &Value) -> Result<Value, String>;
    /// The running instances and which one is chosen.
    fn list_instances(&mut self) -> Value;
    /// Choose the running instance with this process ID or port.
    fn select_instance(&mut self, pid: Option<u32>, port: Option<u16>) -> Result<Value, String>;
    /// Start a new window, optionally opening files, and choose it.
    fn start_instance(&mut self, files: &[PathBuf]) -> Result<Value, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Sent as the command API command of the same name.
    Command,
    /// Sent like `Command`; answers with a `job_id` and accepts `wait_seconds`.
    Job,
    /// Sent as `screenshot`; answers with the image.
    Screenshot,
    WaitForJob,
    WaitUntilIdle,
    ListInstances,
    SelectInstance,
    StartInstance,
}

pub struct Tool {
    pub name: &'static str,
    pub kind: Kind,
    pub description: &'static str,
    pub schema: Value,
}

impl Tool {
    /// Whether the tool only reads the state of the window.
    pub fn read_only(&self) -> bool {
        matches!(
            self.name,
            "status"
                | "job"
                | "list_camera_views"
                | "list_extensions"
                | "list_instances"
                | "wait_for_job"
                | "wait_until_idle"
        )
    }

    /// The tool as `tools/list` describes it.
    pub fn listing(&self) -> Value {
        let mut title = self.name.replace('_', " ");
        if let Some(first) = title.get(..1) {
            title = first.to_ascii_uppercase() + &title[1..];
        }
        json!({
            "name": self.name,
            "title": title,
            "description": self.description,
            "inputSchema": self.schema,
            "annotations": {
                "readOnlyHint": self.read_only(),
                "openWorldHint": false,
            },
        })
    }
}

const SCENE: &str = "in scene coordinates, the units of the scans (normally metres)";
const LAYER: &str = "Zero-based layer index, as listed in status.result.clouds";
const WAIT_LIMIT: f64 = 3600.0;
const POLL_INTERVAL: Duration = Duration::from_millis(250);

fn tool(
    name: &'static str,
    kind: Kind,
    description: &'static str,
    mut arguments: Vec<Argument>,
) -> Tool {
    if kind == Kind::Job {
        arguments.push(optional(
            "wait_seconds",
            number_in(
                "Seconds to wait for the job to finish before answering, up to 3600; without it the answer comes at once with the job_id",
                0.0,
                WAIT_LIMIT,
            ),
        ));
    }
    Tool {
        name,
        kind,
        description,
        schema: object(arguments),
    }
}

/// A tool whose arguments are all optional but that needs one of them.
fn at_least_one(mut tool: Tool) -> Tool {
    tool.schema["minProperties"] = json!(1);
    tool
}

fn yaw() -> Value {
    number_in(
        "Rotation of the camera around the vertical axis in radians, from -π to π",
        -PI,
        PI,
    )
}

fn heading() -> Value {
    number_in(
        "Heading in radians, from -π to π; 0 looks along +X and π/2 along +Y",
        -PI,
        PI,
    )
}

fn walk_pitch() -> Value {
    number_in(
        "Elevation of the viewing direction in radians, from -1.55 to 1.55; positive looks up",
        -1.55,
        1.55,
    )
}

/// The choices of a section drawing, which `export_drawing` and
/// `preview_drawing` share. Each one that is left out keeps what the Section
/// drawing block in Properties has. The limits are those the core checks, no
/// others: the command and the block accept what a tool call accepts.
fn drawing_choices() -> Vec<Argument> {
    let version_keys =
        pointcloud_core::DrawingVersion::ALL.map(pointcloud_core::DrawingVersion::key);
    vec![
        optional("view", choice("Face of the section box that is drawn: plan takes the slab under the top face, seen from above; front, back, left and right take the slab behind the face at Y min, Y max, X min and X max. Another view than the block has sets fill to on for plan and off for the others, unless fill is given", &pointcloud_core::DrawingView::ALL.map(pointcloud_core::DrawingView::key))),
        optional("thickness", number_in("Depth of the slab behind the cut plane in metres, from 0.005 to 5; it is kept within the box. The block starts at 0.10", pointcloud_core::MIN_SLAB_THICKNESS, pointcloud_core::MAX_SLAB_THICKNESS)),
        optional("units", choice("Unit of the drawing: mm (the block starts with it) or m. Scene units are taken as metres", &pointcloud_core::DrawingUnits::ALL.map(pointcloud_core::DrawingUnits::key))),
        optional("origin", choice("Zero of the drawing: model keeps model X and Y in a plan and model Z as height in a vertical view; box puts the lower left corner of the view at zero, which keeps the numbers small for survey coordinates", &pointcloud_core::DrawingOrigin::ALL.map(pointcloud_core::DrawingOrigin::key))),
        optional("fill", boolean("Whether the material the cut plane goes through is drawn as filled regions with outlines")),
        optional("square", boolean("Whether edges of the filled cut are turned onto the main direction where that moves neither end more than 30 mm")),
        optional("grid", number_from("Cell of the grid the filled cut is traced from, in metres, at least 0.005; the block starts at 0.02", pointcloud_core::MIN_CUT_GRID)),
        optional("max_wall_thickness", positive_up_to("Two scanned faces at most this far apart are filled as one wall, in metres, above 0 and at most 2; gaps up to this width are closed. The block starts at 0.50", pointcloud_core::MAX_WALL_THICKNESS)),
        optional("color", choice("Colour of the points: layer (the colour of their layer) or rgb (their scanned colour)", &pointcloud_core::PointColor::ALL.map(pointcloud_core::PointColor::key))),
        optional("point_layers", choice("Layers of the points: scan (one per scan file when several are drawn) or class (one per classification)", &pointcloud_core::PointLayers::ALL.map(pointcloud_core::PointLayers::key))),
        optional("max_points", integer_in("Most points in the drawing, from 1 to 400000; the block starts at 150000. When thinning leaves more, the point spacing doubles until they fit", 1, pointcloud_core::MAX_DRAWING_POINTS as u64)),
        optional("version", choice("File version of the DXF or DWG; the block starts with r2013", &version_keys)),
    ]
}

fn view_name() -> Value {
    text(
        "Name of a saved view of the active scan; letter case is ignored",
        1,
        64,
    )
}

const POINT_FILE: &str = "Absolute destination path; its extension (.ply, .xyz, .pts, .csv, .las, .laz or .e57) selects the format. The file is replaced atomically";
const LAS_FILE: &str = "Absolute destination path ending in .las or .laz";
const OBJ_FILE: &str = "Absolute destination path ending in .obj";
const MESH_FILE: &str = "Absolute destination path; its extension selects the format: .obj (colours and normals), .ply (binary, double coordinates, colours and normals) or .stl (binary, triangles only). The file is replaced atomically";
const DRAWING_FILE: &str = "Absolute destination path; its extension selects the format: .dxf or .dwg. The file is replaced atomically";
const BCF_FILE: &str = "Absolute destination path ending in .bcf";
const RD_BOX: &str = "The area [xmin, ymin, xmax, ymax] in RD New coordinates (EPSG:28992, metres): each side longer than 0 and at most 2000";

/// Every tool, in the order `tools/list` gives them. A new command API
/// command needs one entry here; `Kind::Command` and `Kind::Job` tools send
/// their arguments unchanged with `"command": name`.
fn table() -> Vec<Tool> {
    use Kind::*;
    vec![
        tool("status", Command, "Reports the state of the window: the open layers (index, path, point counts, bounds, visibility, transform, stations), running imports and tasks with their progress, the active layer, the orbit camera (yaw and pitch in radians, zoom, pan in pixels), the viewport size in pixels, the walking camera, the section box, the Section drawing tool (drawing: its settings, a running job, the last result, whether a preview is shown), selection and measurement, saved views and annotations, display settings, whether the File view covers the model (file_view) and the status line.", vec![]),
        tool("job", Command, "Reads a background job by the job_id that an export, export_drawing, preview_drawing, select_world, pick_screen, mesh, export_mesh, merge_visible or bag3d returned: its state is running (with progress where known), complete (with its result), failed (with an error) or cancelled. The newest 32 jobs stay readable.", vec![
            required("id", text("The job_id", 1, 64)),
        ]),
        tool("wait_for_job", WaitForJob, "Waits until a background job is no longer running and returns it, polling it four times a second. Answers with timed_out: true and the running job when the time is up.", vec![
            required("id", text("The job_id", 1, 64)),
            optional("timeout_seconds", number_in("Longest wait in seconds, default 60", 0.0, WAIT_LIMIT)),
        ]),
        tool("wait_until_idle", WaitUntilIdle, "Waits until the window has no work under way: no imports, octree builds, selections, thinning, scaling, meshing, mesh export, section drawing or its preview, merging, 3D BAG download, station photos, view snapshots or point loading for the camera. Call it after open, after changing the camera before a screenshot, and before export_bcf. Answers with idle: false and what is still busy when the time is up.", vec![
            optional("timeout_seconds", number_in("Longest wait in seconds, default 60", 0.0, WAIT_LIMIT)),
        ]),
        tool("screenshot", Screenshot, "Captures the 3D viewport (the scene without ribbon and panels) as a PNG image and returns it, after waiting up to 4 seconds for the points of the current camera to load. The text part gives the width and height in pixels. Fails while the window is minimised, and while the File view or Settings covers the viewport; file_view with open false returns to the model.", vec![
            optional("path", path("Absolute path ending in .png where the image is also written")),
            optional("max_edge", integer_in("Longest edge of the image in pixels, from 16 to 8192; default 1920. A larger viewport is scaled down", 16, 8192)),
        ]),
        tool("open", Command, "Opens in the window a point cloud or mesh file (LAS, LAZ, E57, PLY, PCD, PTX, XYZ, ASC, TXT, CSV, PTS, OBJ, OFF, STL or DXF), every supported file directly inside a folder, or the scans a scan project file (.rcp) lists. Answers once the folder or project file has been read, with the accepted files, missing and already open scans and the import ids; loading continues in the background, so call wait_until_idle before using the new layers.", vec![
            required("path", path("Absolute path of a file, folder or scan project file")),
        ]),
        tool("cancel_import", Command, "Cancels a running full-stream import; no partial layer is added.", vec![
            required("id", ordinal("Import id from the answer of open or from status.result.imports")),
        ]),
        tool("remove", Command, "Removes a layer from the project.", vec![
            required("index", ordinal(LAYER)),
        ]),
        tool("set_active", Command, "Makes a layer the active one. Saved views, exports, transforms, thinning, indexing and meshing act on the active layer.", vec![
            required("index", ordinal(LAYER)),
        ]),
        tool("set_visible", Command, "Shows or hides the points of a layer.", vec![
            required("index", ordinal(LAYER)),
            required("visible", boolean("Whether the layer is shown")),
        ]),
        tool("camera", Command, "Turns the orbit camera to a standard viewing direction.", vec![
            required("preset", choice("Viewing direction", &["top", "bottom", "front", "back", "left", "right", "isometric"])),
        ]),
        tool("set_camera", Command, "Sets the orbit camera exactly. Values outside the ranges are rejected without changing the view.", vec![
            required("yaw", yaw()),
            required("pitch", number_in("Elevation of the camera in radians, from -1.56 to 1.56; positive looks down on the model from above", -1.56, 1.56)),
            required("zoom", number_in("Viewing distance relative to the whole model: 1 frames the complete model and smaller values come closer (0.1 magnifies ten times); from 0.000001 to 10000", 0.000_001, 10_000.0)),
            required("pan", pixel("Shift of the picture [x, y] in screen pixels")),
        ]),
        tool("zoom_all", Command, "Fits the complete model in the viewport at the default isometric direction, like the F key.", vec![]),
        tool("open_panorama", Command, "Stands in a scanner station of a layer and shows the photos taken there (E57 scans with station images); status.result.walk reports the view.", vec![
            required("index", ordinal(LAYER)),
            required("station", ordinal("Zero-based station number within that layer")),
        ]),
        tool("set_panorama", Command, "Turns the walking camera while it stands in a station or walks.", vec![
            required("yaw", heading()),
            required("pitch", walk_pitch()),
            required("field_of_view", number_in("Horizontal field of view in radians, from 0.35 to 2.1", 0.35, 2.1)),
        ]),
        tool("walk", Command, "Places the walking (first-person) camera at a position, looking along a heading; inside a station ball it shows that station's photos.", vec![
            required("eye", xyz("Camera position [x, y, z] in scene coordinates, the units of the scans (normally metres)")),
            required("yaw", heading()),
            required("pitch", walk_pitch()),
        ]),
        tool("close_panorama", Command, "Leaves the walking camera and returns to the orbit view.", vec![]),
        tool("list_camera_views", Command, "Lists the saved views of the active scan with their camera, section box, colour mode and annotations, and the name of the active view.", vec![]),
        tool("save_camera_view", Command, "Saves what the viewport shows of the active scan (camera, section box, colour mode) as a view and makes it the active view. A snapshot image follows shortly after; wait_until_idle waits for it.", vec![
            optional("name", text("Name of the new view, unique within the scan; without it the first free \"View N\" is used", 1, 64)),
        ]),
        tool("update_camera_view", Command, "Overwrites a saved view with what the viewport shows now, keeping its name, identifier and annotations, and makes it the active view.", vec![
            required("name", view_name()),
        ]),
        tool("rename_camera_view", Command, "Renames a saved view of the active scan.", vec![
            required("name", view_name()),
            required("new_name", text("New name, unique within the scan", 1, 64)),
        ]),
        tool("restore_camera_view", Command, "Shows a saved view again (camera, section box, colour mode) and makes it the active view with its annotations.", vec![
            required("name", view_name()),
        ]),
        tool("delete_camera_view", Command, "Deletes a saved view of the active scan with its snapshot.", vec![
            required("name", view_name()),
        ]),
        tool("add_note", Command, "Adds a note at a position to the active view; when no view is active the current view is first saved as \"View N\". Returns the annotations of the view.", vec![
            required("point", xyz("Position [x, y, z] of the note in scene coordinates, the units of the scans (normally metres)")),
            required("text", text("Text of the note", 1, 240)),
        ]),
        tool("add_line", Command, "Adds a line, drawn as an arrow from `from` to `to`, to the active view. Returns the annotations of the view.", vec![
            required("from", xyz("Start [x, y, z] in scene coordinates")),
            required("to", xyz("End [x, y, z] in scene coordinates")),
        ]),
        tool("delete_annotation", Command, "Removes an annotation from the active view.", vec![
            required("index", ordinal("Zero-based place in the active view's annotations")),
        ]),
        tool("set_annotation_tool", Command, "Chooses the note or line tool of the viewport, or leaves it; a half-placed annotation is dropped.", vec![
            optional("tool", json!({"type": ["string", "null"], "enum": ["note", "line", null], "description": "note, line, or null (or absent) to leave the tool"})),
        ]),
        tool("annotate_screen", Command, "Clicks at a viewport pixel with the active annotation tool: the exact source point there becomes the point of a note (then call submit_note) or the start or end of a line. Answers when the search has started; wait_until_idle, then status.result.views.placing shows the picked point.", vec![
            required("pointer", pixel("Viewport pixel [x, y] from the top-left corner of the viewport")),
        ]),
        tool("submit_note", Command, "Gives the note that waits for its text that text and adds it to the active view.", vec![
            required("text", text("Text of the note", 1, 240)),
        ]),
        tool("export_bcf", Command, "Writes all saved views of the active scan with their snapshots and annotations as one BCF 2.1 file. Call wait_until_idle first so the newest snapshots are included.", vec![
            required("path", path(BCF_FILE)),
        ]),
        tool("set_theme", Command, "Chooses and keeps the colour theme of the window.", vec![
            required("theme", choice("forge (Deep Forge), light (Blueprint Light), night (Night Build), blueprint (Blueprint Blue) or contrast (High Contrast); openaec is the same as night and is what status.result.theme reports for it", &["forge", "light", "night", "blueprint", "contrast", "openaec"])),
        ]),
        tool("set_language", Command, "Sets the language of the user interface and keeps it for later sessions.", vec![
            required("language", choice("auto for the language of the system when there is a translation for it, en for English, or the code of a translation such as nl for Dutch; status.result.language reports the choice", &crate::i18n::Language::keys())),
        ]),
        tool("set_color", Command, "Chooses how points are coloured.", vec![
            required("mode", choice("Colour source", &["rgb", "elevation", "intensity", "classification"])),
        ]),
        tool("set_class_visible", Command, "Shows or hides one classification code in the viewport and in exact selections.", vec![
            required("code", integer_in("Classification code, from 0 to 255 (2 is ground, 6 building)", 0, 255)),
            required("visible", boolean("Whether points of this class are shown")),
        ]),
        tool("set_point_size", Command, "Sets the size of the drawn points.", vec![
            required("size", number_in("Radius of a point on screen in pixels, from 0.1 to 20", 0.1, 20.0)),
        ]),
        tool("set_eye_dome", Command, "Switches the depth-based eye-dome shading on or off.", vec![
            required("enabled", boolean("Whether eye-dome shading is on")),
        ]),
        tool("set_eye_dome_strength", Command, "Sets the strength of the eye-dome shading.", vec![
            required("strength", number_in("Strength from 0 to 5; 1 is the default", 0.0, 5.0)),
        ]),
        tool("set_budget", Command, "Sets how many points the viewport draws at most.", vec![
            required("points", integer_in("Point budget, from 1000 to 10000000", 1_000, 10_000_000)),
        ]),
        tool("set_section", Command, "Switches on a section box that clips the view, exports and selections to an axis-aligned box inside the model bounds.", vec![
            required("min", xyz("Lowest corner [x, y, z] in scene coordinates")),
            required("max", xyz("Highest corner [x, y, z] in scene coordinates")),
        ]),
        tool("clear_section", Command, "Switches the section box off.", vec![]),
        tool("select_world", Job, "Selects every exact source point inside an inclusive box across the visible layers, honouring class filters, the section box and deleted points. Answers with a job_id; the job reports the number of points.", vec![
            required("min", xyz("Lowest corner [x, y, z] in scene coordinates")),
            required("max", xyz("Highest corner [x, y, z] in scene coordinates")),
        ]),
        tool("pick_screen", Job, "Picks the source point nearest a viewport pixel, first among the drawn points and then in the full source. Answers with a job_id; the complete job gives the point's source ordinal, XYZ, colour, intensity and classification, or zero points on a miss.", vec![
            required("pointer", pixel("Viewport pixel [x, y] from the top-left corner; status.result.viewport_size gives the size")),
            optional("radius", number_in("Search radius in pixels, from 1 to 64; default 8", 1.0, 64.0)),
        ]),
        tool("cancel_selection", Command, "Stops a running box selection or point pick; no partial selection is kept.", vec![]),
        tool("clear_selection", Command, "Clears the point selection.", vec![]),
        tool("measure", Command, "Sets a finished distance (polyline) or area (closed polygon) measurement through points and returns its values: segment lengths, length, horizontal length and height difference, or area, plan area and perimeter, in scene units.", vec![
            required("mode", choice("distance or area", &["distance", "area"])),
            required("points", list(&format!("Points [x, y, z] {SCENE}: 2 to 256 for a distance, 3 to 256 for an area"), xyz("Point [x, y, z]"), 2, 256)),
        ]),
        tool("clear_measure", Command, "Removes the measurement.", vec![]),
        tool("zoom_selection", Command, "Frames the selected points in the viewport without changing the section box; the camera follows once status.result.selection_bounds_pending is false.", vec![]),
        tool("delete_selection", Command, "Hides the selected points; the source file is not changed and undo_delete restores them.", vec![]),
        tool("undo_delete", Command, "Restores the latest deleted or thinned points.", vec![]),
        tool("redo_delete", Command, "Deletes again what undo_delete restored.", vec![]),
        tool("thin", Command, "Keeps an exact percentage of the active layer's remaining points, in the background (status.result.thin_pending); undo_delete restores them.", vec![
            required("percent", integer_in("Percentage of the points to keep, from 1 to 100", 1, 100)),
        ]),
        tool("translate", Command, "Moves the active layer by an offset.", vec![
            required("offset", xyz("Offset [dx, dy, dz] in scene units")),
        ]),
        tool("scale", Command, "Scales the active layer around the exact centroid of its remaining points; large sources run in the background (status.result.scale).", vec![
            required("factors", xyz("Scale factors [x, y, z]")),
        ]),
        tool("cancel_scale", Command, "Cancels a running scale.", vec![]),
        tool("reset_transform", Command, "Puts the active layer back at its source coordinates.", vec![]),
        tool("build_index", Command, "Starts building the disk octree of the active unindexed layer; status.result.index_progress reports the progress.", vec![]),
        tool("cancel_index", Command, "Cancels a running octree build.", vec![]),
        tool("set_auto_index", Command, "Switches the automatic indexing of large clouds on or off.", vec![
            required("enabled", boolean("Whether large clouds are indexed automatically")),
        ]),
        tool("set_surface_settings", Command, "Sets the limits of 3D surface reconstruction.", vec![
            required("max_vertices", integer_in("Most vertices, from 3 to 1000000", 3, 1_000_000)),
            required("neighbors", integer_in("Neighbours per vertex, from 3 to 32", 3, 32)),
            required("edge_factor", positive("Longest edge relative to the typical point spacing")),
        ]),
        tool("mesh", Job, "Reconstructs a terrain (2.5D) or surface (3D) mesh from the remaining points of the active layer inside the section box and class filters, and writes it as OBJ. Answers with a job_id; the complete job reports the vertices and triangles and, as a measure of quality, the open edges (edges with one triangle: rims and holes) and the connected parts (components) of the mesh.", vec![
            required("mode", choice("terrain or surface", &["terrain", "surface"])),
            required("path", path(OBJ_FILE)),
        ]),
        tool("cancel_mesh", Command, "Cancels the running mesh job.", vec![]),
        tool("export_mesh", Job, "Saves the mesh the active layer holds (a terrain mesh, a 3D surface, an opened mesh file or downloaded 3D BAG buildings; status.result.clouds[].mesh is null for a layer without one) as OBJ, PLY or STL, moved and scaled as in the scene. Answers with a job_id; the complete job reports the format, vertices and triangles. An STL file holds 32-bit floats: a mesh farther than 2,048 m from zero on an axis is written relative to a whole-metre origin, which the job reports as origin and the file header names; other programs show such a file near zero.", vec![
            required("path", path(MESH_FILE)),
        ]),
        tool("export", Job, "Exports the full active layer from its source, without deleted points. Answers with a job_id; the job reports the point count.", vec![
            required("path", path(POINT_FILE)),
        ]),
        tool("export_section", Job, "Exports the points of the active layer inside the section box, without deleted points. Answers with a job_id.", vec![
            required("path", path(POINT_FILE)),
        ]),
        tool("export_selection", Job, "Exports the selected points of the active layer. Answers with a job_id.", vec![
            required("path", path(POINT_FILE)),
        ]),
        tool("export_minus_selection", Job, "Exports the active layer without its selected and deleted points. Answers with a job_id.", vec![
            required("path", path(POINT_FILE)),
        ]),
        tool("export_drawing", Job, "Draws what the section box cuts as a 2D drawing at scale 1:1 and writes it as DXF or DWG: the points of the slab behind one face of the box, thinned to one per 5 mm, and for a plan the filled cut with its outlines, from every visible layer with its transform, without deleted points and hidden classes. Needs the section box. The choices given are also put in the Section drawing block of Properties; those left out keep what the block has. Answers with a job_id; the running job reports its stage (reading, tracing, writing), the complete job the points in the slab and drawn, the point spacing used and whether the point limit raised it, the regions and the small ones dropped, the grid cell and main direction of the filled cut, and the file size.", {
            let mut arguments = vec![required("path", path(DRAWING_FILE))];
            arguments.extend(drawing_choices());
            arguments
        }),
        tool("preview_drawing", Job, "Traces the filled cut of the section box as export_drawing would draw it and lays it over the points in the viewport, on the cut plane; nothing is written. The cut is traced whatever fill says. Answers with a job_id; the complete job reports the regions. status.result.drawing.preview_shown tells whether it is on screen: it goes away when the section box, the visible layers, a layer transform, the deleted points, the classes shown or the view, slab thickness, squaring, grid or wall thickness change.", drawing_choices()),
        tool("clear_drawing_preview", Command, "Takes the preview of the filled cut off the viewport.", vec![]),
        tool("cancel_drawing", Command, "Cancels the running section drawing or preview; the step under way ends first, and an existing file at the destination is left as it is.", vec![]),
        tool("merge_visible", Job, "Merges the visible LAS/LAZ layers, with their deletions and transforms, into one file. Answers with a job_id.", vec![
            required("path", path(LAS_FILE)),
        ]),
        tool("cancel_merge", Command, "Cancels the running merge.", vec![]),
        tool("bag3d", Job, "Downloads the buildings of the Dutch 3D BAG register inside a box in RD New coordinates of at most 2 by 2 km and about 5,000 buildings, writes them as a georeferenced OBJ mesh and opens that as a layer. Needs an internet connection. Answers with a job_id; the running job reports the page it is at, the complete job the buildings, vertices, triangles and pages. A denser area fails after the first page with the number of buildings it holds.", vec![
            required("bbox", numbers(RD_BOX, 4)),
            required("lod", choice("Level of detail of the building models", &["1.2", "1.3", "2.2"])),
            required("path", path(OBJ_FILE)),
        ]),
        tool("cancel_bag3d", Command, "Cancels the running 3D BAG download; the request under way ends first, and an existing file at the destination is left as it is.", vec![]),
        tool("list_extensions", Command, "Lists the optional features built into the application (id, name, version, description, author, category, whether it uses the internet) and whether each is enabled.", vec![]),
        tool("set_extension_enabled", Command, "Switches a built-in optional feature on or off and keeps that for later sessions. When the answer has saved false the choice could not be written and holds for this session only; save_error says why. Switching bag3d off closes its panel, stops a running download and makes the bag3d tool fail.", vec![
            required("id", choice("Id of the extension, as list_extensions gives it", &crate::extensions::ids())),
            required("enabled", boolean("Whether the extension is switched on")),
        ]),
        tool("file_view", Command, "Opens the File view over the model, on a page when one is named, or closes it and returns to the model. A screenshot needs it closed. Opening is refused while the Settings dialog is open.", vec![
            required("open", boolean("true to show the File view, false to return to the model")),
            optional("page", choice("Page to show, only with open true; without it the view opens on workspace, or keeps the page it shows", &crate::file_view::FilePage::ids())),
        ]),
        tool("list_instances", ListInstances, "Lists the running Open Pointcloud Studio windows (process ID, port, version, start time) and which one the tools drive.", vec![]),
        at_least_one(tool("select_instance", SelectInstance, "Chooses the running window that the other tools drive, by process ID or port (at least one of them) as list_instances gives them.", vec![
            optional("pid", integer_in("Process ID", 1, u64::from(u32::MAX))),
            optional("port", integer_in("Command API port", 1, 65_535)),
        ])),
        tool("start_instance", StartInstance, "Starts a new window, optionally opening files, waits until its command API answers and chooses it.", vec![
            optional("files", list("Absolute paths of files, folders or scan project files to open", path("Absolute path"), 0, 64)),
        ]),
    ]
}

pub fn tools() -> &'static [Tool] {
    static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
    TOOLS.get_or_init(table)
}

pub fn find(name: &str) -> Option<&'static Tool> {
    tools().iter().find(|tool| tool.name == name)
}

/// What a tool call returns: its content blocks, the JSON it reports, and
/// whether it failed.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub content: Vec<Value>,
    pub structured: Value,
    pub is_error: bool,
}

impl Outcome {
    fn json(value: Value, is_error: bool) -> Self {
        Self {
            content: vec![json!({"type": "text", "text": value.to_string()})],
            structured: value,
            is_error,
        }
    }

    /// An answer of the command API, which fails when `ok` is false.
    fn answer(value: Value) -> Self {
        let failed = value.get("ok") == Some(&Value::Bool(false));
        Self::json(value, failed)
    }

    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        Self {
            content: vec![json!({"type": "text", "text": message})],
            structured: json!({"ok": false, "error": message}),
            is_error: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    UnknownTool,
    InvalidArguments(String),
}

/// Carry out a tool call. Arguments that do not fit the tool's schema are
/// refused before anything is sent; `cancelled` ends waits early.
pub fn call(
    name: &str,
    arguments: Option<&Value>,
    link: &mut dyn Link,
    cancelled: &dyn Fn() -> bool,
) -> Result<Outcome, CallError> {
    let tool = find(name).ok_or(CallError::UnknownTool)?;
    let empty = Value::Object(Map::new());
    let arguments = arguments.unwrap_or(&empty);
    validate_arguments(&tool.schema, arguments).map_err(CallError::InvalidArguments)?;
    let mut arguments = arguments.clone();
    whole_numbers_as_integers(&tool.schema, &mut arguments);
    let mut arguments = arguments.as_object().cloned().unwrap_or_default();
    let outcome = match tool.kind {
        Kind::Command => send(link, tool.name, arguments),
        Kind::Job => {
            let wait = arguments
                .remove("wait_seconds")
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0);
            run_job(link, tool.name, arguments, wait, cancelled)
        }
        Kind::Screenshot => screenshot(link, arguments),
        Kind::WaitForJob => {
            let id = arguments["id"].as_str().unwrap_or_default().to_owned();
            let timeout = timeout(&arguments);
            match wait_for_job(link, &id, timeout, cancelled) {
                Ok((job, timed_out)) => {
                    job_outcome(json!({"ok": true, "job": job, "timed_out": timed_out}))
                }
                Err(error) => Outcome::error(error),
            }
        }
        Kind::WaitUntilIdle => wait_until_idle(link, timeout(&arguments), cancelled),
        Kind::ListInstances => Outcome::json(link.list_instances(), false),
        Kind::SelectInstance => {
            let pid = arguments.get("pid").and_then(Value::as_u64);
            let port = arguments.get("port").and_then(Value::as_u64);
            let pid = pid.and_then(|pid| u32::try_from(pid).ok());
            let port = port.and_then(|port| u16::try_from(port).ok());
            match link.select_instance(pid, port) {
                Ok(value) => Outcome::json(value, false),
                Err(error) => Outcome::error(error),
            }
        }
        Kind::StartInstance => {
            let files: Vec<PathBuf> = arguments
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .collect();
            if let Some(relative) = files.iter().find(|file| !file.is_absolute()) {
                return Err(CallError::InvalidArguments(format!(
                    "{} is not an absolute path",
                    relative.display()
                )));
            }
            match link.start_instance(&files) {
                Ok(value) => Outcome::json(value, false),
                Err(error) => Outcome::error(error),
            }
        }
    };
    Ok(outcome)
}

/// The command API body of a command: its name and its arguments.
pub fn command_body(name: &str, arguments: Map<String, Value>) -> Value {
    let mut body = arguments;
    body.insert("command".into(), Value::from(name));
    Value::Object(body)
}

fn send(link: &mut dyn Link, name: &str, arguments: Map<String, Value>) -> Outcome {
    match link.exec(&command_body(name, arguments)) {
        Ok(answer) => Outcome::answer(answer),
        Err(error) => Outcome::error(error),
    }
}

fn timeout(arguments: &Map<String, Value>) -> Duration {
    let seconds = arguments
        .get("timeout_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(60.0);
    Duration::from_secs_f64(seconds.clamp(0.0, WAIT_LIMIT))
}

/// A job's final answer fails when the job failed.
fn job_outcome(value: Value) -> Outcome {
    let failed = value["job"]["state"] == "failed";
    Outcome::json(value, failed)
}

fn run_job(
    link: &mut dyn Link,
    name: &str,
    arguments: Map<String, Value>,
    wait: f64,
    cancelled: &dyn Fn() -> bool,
) -> Outcome {
    let answer = match link.exec(&command_body(name, arguments)) {
        Ok(answer) => answer,
        Err(error) => return Outcome::error(error),
    };
    let id = answer["job_id"].as_str().map(str::to_owned);
    let (Some(id), true) = (id, wait > 0.0) else {
        return Outcome::answer(answer);
    };
    match wait_for_job(link, &id, Duration::from_secs_f64(wait), cancelled) {
        Ok((job, timed_out)) => {
            let mut value = answer;
            value["job"] = job;
            value["timed_out"] = Value::Bool(timed_out);
            job_outcome(value)
        }
        Err(error) => Outcome::error(error),
    }
}

/// Poll a job until it no longer runs, the time is up or the call is
/// cancelled. Returns the job and whether it still runs.
fn wait_for_job(
    link: &mut dyn Link,
    id: &str,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(Value, bool), String> {
    let deadline = Instant::now() + timeout;
    loop {
        let answer = link.exec(&json!({"command": "job", "id": id}))?;
        if answer["ok"] != true {
            return Err(answer["error"]
                .as_str()
                .unwrap_or("the job could not be read")
                .to_owned());
        }
        let job = answer["job"].clone();
        if job["state"] != "running" {
            return Ok((job, false));
        }
        if Instant::now() >= deadline || cancelled() {
            return Ok((job, true));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// What `status.result` shows to be under way.
pub fn busy(result: &Value) -> Vec<&'static str> {
    let mut busy = Vec::new();
    if result["imports"]
        .as_array()
        .is_some_and(|imports| !imports.is_empty())
    {
        busy.push("imports");
    }
    for (key, name) in [
        ("index_progress", "index"),
        ("scale", "scale"),
        ("mesh", "mesh"),
        ("merge", "merge"),
        ("bag3d", "bag3d"),
    ] {
        if result[key].is_object() {
            busy.push(name);
        }
    }
    for (key, name) in [
        ("selection_pending", "selection"),
        ("selection_bounds_pending", "selection_bounds"),
        ("thin_pending", "thin"),
        ("mesh_export_pending", "mesh_export"),
        ("detail_pending", "points"),
    ] {
        if result[key] == true {
            busy.push(name);
        }
    }
    if result["photos_loading"]
        .as_u64()
        .is_some_and(|count| count > 0)
    {
        busy.push("photos");
    }
    if result["views"]["snapshots_pending"]
        .as_u64()
        .is_some_and(|count| count > 0)
    {
        busy.push("snapshots");
    }
    if result["views"]["export_pending"] == true {
        busy.push("bcf_export");
    }
    if result["drawing"]["job"].is_object() {
        busy.push("drawing");
    }
    busy
}

/// Poll the status until nothing is under way in `QUIET_LOOKS` looks in a
/// row. They span half a second, longer than the 220 ms after a camera
/// change before the window starts reading the points for it.
fn wait_until_idle(
    link: &mut dyn Link,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Outcome {
    const QUIET_LOOKS: u32 = 3;
    let started = Instant::now();
    let mut quiet = 0;
    loop {
        let answer = match link.exec(&json!({"command": "status"})) {
            Ok(answer) if answer["ok"] == true => answer,
            Ok(answer) => return Outcome::answer(answer),
            Err(error) => return Outcome::error(error),
        };
        let busy = busy(&answer["result"]);
        quiet = if busy.is_empty() { quiet + 1 } else { 0 };
        let done = quiet >= QUIET_LOOKS;
        if done || started.elapsed() >= timeout || cancelled() {
            return Outcome::json(
                json!({
                    "ok": true,
                    "idle": busy.is_empty(),
                    "busy": busy,
                    "waited_seconds": (started.elapsed().as_secs_f64() * 10.0).round() / 10.0,
                    "status": answer["result"]["status"],
                }),
                false,
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn screenshot(link: &mut dyn Link, mut arguments: Map<String, Value>) -> Outcome {
    arguments.insert("base64".into(), Value::Bool(true));
    let mut answer = match link.exec(&command_body("screenshot", arguments)) {
        Ok(answer) => answer,
        Err(error) => return Outcome::error(error),
    };
    if answer["ok"] != true {
        return Outcome::answer(answer);
    }
    let Some(Value::String(png)) = answer
        .as_object_mut()
        .and_then(|fields| fields.remove("png_base64"))
    else {
        return Outcome::error("the window answered without an image");
    };
    Outcome {
        content: vec![
            json!({"type": "image", "data": png, "mimeType": "image/png"}),
            json!({"type": "text", "text": answer.to_string()}),
        ],
        structured: answer,
        is_error: false,
    }
}
