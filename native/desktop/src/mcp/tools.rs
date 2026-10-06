//! The tools of the MCP server: one table with every tool, its arguments and
//! how it is carried out, mostly by sending the command API command of the
//! same name to a running window.

use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::schema::{
    boolean, choice, integer_in, list, number, number_from, number_in, number_or_null, numbers,
    object, optional, ordinal, path, pixel, positive, positive_up_to, required, text,
    validate_arguments, whole_numbers_as_integers, xyz, xyz_or_null, Argument,
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
                | "list_drawings"
                | "list_faces"
                | "list_photos"
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

/// The settings of a closed mesh, which `mesh` with the mode `closed` and
/// `set_closed_mesh_settings` share. Each one that is left out keeps what
/// the Closed mesh block in Properties has. The limits are those the core
/// checks.
fn closed_mesh_settings() -> Vec<Argument> {
    vec![
        optional("voxel", number_or_null("Edge of a voxel in metres, from 0.005 to 0.5, or null for automatic: 0.02 for a region up to 20 m long, 0.03 up to 60 m and 0.05 beyond. Detail under about two voxels is lost; choose the voxel at least twice the spacing of the points. The block starts with automatic", 0.005, 0.5)),
        optional("max_hole", number_in("Gaps in the points up to this wide are closed, in metres, from 0 (none) to 3.2 and never more than 32 voxels; wider ones, such as door and window openings, stay open. The block starts at 0.25", 0.0, pointcloud_core::MAX_CLOSED_MESH_HOLE)),
        optional("simplify_mm", number_or_null("How far simplification may move the surface, in millimetres, from 0 (no simplification) to 1000, or null for automatic: 0.15 voxel, 3 mm at voxels of 0.02 m. The block starts with automatic", 0.0, 1000.0)),
        optional("sample_percent", number_in("Share of the source points the surface is fitted to, in percent, from 0.01 to 100: the same points on every run, spread over the region. A smaller share is faster but can lose sparse detail, and one that leaves no point in the region fails the job. The block starts at 100, every point", pointcloud_core::MIN_CLOSED_MESH_SAMPLE_PERCENT, 100.0)),
        optional("sides", choice("Which side a face looks at: automatic takes the scanner station that measured it where the scan knows its stations and the centre of the region elsewhere; centre turns every face to the centre of the region (the section box cut back to the points), which suits a room in its box; upward turns every face up, for data measured from above. The block starts with automatic", &["automatic", "centre", "upward"])),
        optional("layers", choice("Which layers give their points: active (the active layer, as the block starts) or visible (every layer whose points are shown and that reaches the section box when it is on, without layers of 3D BAG buildings). The mesh goes to the active layer either way", &["active", "visible"])),
    ]
}

/// The settings of a face detection, which `detect_faces` and
/// `set_face_settings` share. Each one that is left out keeps what the
/// Detect faces block in Properties has. The limits are those the block
/// states; the core asks for values above zero only.
fn face_settings() -> Vec<Argument> {
    use crate::faces::{
        MAX_ANGLE, MAX_FACE_AREA, MAX_TOLERANCE, MIN_ANGLE, MIN_FACE_AREA, MIN_TOLERANCE,
    };
    vec![
        optional("distance_tolerance", number_in("How far a point may lie from the plane of its face, in metres, from 0.001 to 0.5; about three times the noise of the scan or more. The block starts at 0.02 (20 mm) and shows millimetres", MIN_TOLERANCE, MAX_TOLERANCE)),
        optional("angle_tolerance", number_in("How far the surface at a point may be turned from its face, in degrees, from 1 to 45. The block starts at 10", MIN_ANGLE, MAX_ANGLE)),
        optional("min_area", number_in("Smaller faces are not reported, in square metres, from 0.01 to 10000. The block starts at 0.25", MIN_FACE_AREA, MAX_FACE_AREA)),
        optional("cylinders", boolean("Whether round columns and pipes are looked for among the points no flat face took. The block starts with true")),
        optional("layers", choice("Which layers give their points: active (the active layer, as the block starts) or visible (every layer whose points are shown and that reaches the section box when it is on, without layers of 3D BAG buildings; the active layer has to be one of them). The faces are kept with the active layer either way", &["active", "visible"])),
        optional("color", choice("How the faces are coloured in the scene: face (one colour per face, by its class, as the block starts) or deviation (the distance of the scan to each face: blue behind it, near white on it, red in front of it, full colour at the distance tolerance)", &["face", "deviation"])),
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
const MESH_JOB_FILE: &str = "Absolute destination path. Required for terrain and surface, where it ends in .obj. For closed it may be left out: the mesh then only becomes the mesh of the active layer; with a path ending in .obj, .ply, .stl, .dxf, .dwg or .ifc in a folder that exists it is also written there, as the scene shows it";
const MESH_FILE: &str = "Absolute destination path; its extension selects the format: .obj (colours and normals), .ply (binary, double coordinates, colours and normals), .stl (binary, triangles only), .dxf or .dwg (editable MESH entities on layer OPS-MESH, metres; no ACIS solids) or .ifc (IFC4 building element proxy with a triangulated face set). The file is replaced atomically";
const FACES_FILE: &str = "Absolute destination path in a folder that exists; its extension selects the format: .json (every plane and cylinder with its parameters, outline, residuals and the edges between faces), .obj (the faces as triangles, one group per face), .dxf or .dwg (3D polyface meshes: flat faces on layers OPS-PLANES-FLOOR, -CEILING, -WALL and -SLOPED showing their outline and openings, cylinders on OPS-CYLINDERS with their axes as lines on OPS-CYLINDER-AXES; metres, scene coordinates; no ACIS solids) or .ifc (IFC4: every face an IfcBuildingElementProxy, a flat face as a polygonal face set with its openings, a cylinder seen from outside as an extruded circle along its axis, with property set OPS_ScanGeometry). The file is replaced atomically";
const DRAWING_FILE: &str = "Absolute destination path; its extension selects the format: .dxf or .dwg. The file is replaced atomically";
const BCF_FILE: &str = "Absolute destination path ending in .bcf";
const RD_BOX: &str = "The area [xmin, ymin, xmax, ymax] in RD New coordinates (EPSG:28992, metres): each side longer than 0 and at most 2000";

/// Every tool, in the order `tools/list` gives them. A new command API
/// command needs one entry here; `Kind::Command` and `Kind::Job` tools send
/// their arguments unchanged with `"command": name`.
fn table() -> Vec<Tool> {
    use Kind::*;
    vec![
        tool("status", Command, "Reports the state of the window: the open layers (index, path, point counts, bounds, visibility, transform, stations), running imports and tasks with their progress, the active layer, the orbit camera (yaw and pitch in radians, zoom, pan in pixels), the viewport size in pixels, the walking camera, the section box, the Section drawing tool (drawing: its settings, a running job, the last result, whether a preview is shown), the Closed mesh tool (closed_mesh: its settings, a running job, the last result), the Detect faces tool (faces: its settings, a running job, the last job, export_pending and result, the faces of the active layer in figures; clouds[].faces has those figures per layer, or null), the photos of the files and the one that is entered (photos), selection and measurement, saved views and annotations, display settings, whether the File view covers the model (file_view), the Mesh to Plans wizard (mesh_to_plans: whether it is shown as card or strip, its step and the status of every step) and the status line.", vec![]),
        tool("job", Command, "Reads a background job by the job_id that an export, export_drawing, preview_drawing, select_world, pick_screen, mesh, export_mesh, detect_faces, export_faces, colour_from_photos, merge_visible or bag3d returned, or that status.result.mesh_to_plans.job names: its state is running (with progress where known), complete (with its result), failed (with an error) or cancelled. The newest 32 jobs stay readable.", vec![
            required("id", text("The job_id", 1, 64)),
        ]),
        tool("wait_for_job", WaitForJob, "Waits until a background job is no longer running and returns it, polling it four times a second. Answers with timed_out: true and the running job when the time is up.", vec![
            required("id", text("The job_id", 1, 64)),
            optional("timeout_seconds", number_in("Longest wait in seconds, default 60", 0.0, WAIT_LIMIT)),
        ]),
        tool("wait_until_idle", WaitUntilIdle, "Waits until the window has no work under way: no imports, octree builds, selections, thinning, scaling, meshing, mesh export, face detection, faces export, section drawing or its preview, a drawing file being read, steps of Mesh to Plans, colouring points from photos, merging, 3D BAG download, station photos, the photos of a file being listed or decoded, view snapshots, the fill of the cut of a mesh by the section box or point loading for the camera. Call it after open, after changing the camera before a screenshot, and before export_bcf. Answers with idle: false and what is still busy when the time is up.", vec![
            optional("timeout_seconds", number_in("Longest wait in seconds, default 60", 0.0, WAIT_LIMIT)),
        ]),
        tool("screenshot", Screenshot, "Captures the 3D viewport (the scene without ribbon and panels) as a PNG image and returns it, after waiting up to 4 seconds for the points of the current camera to load and the fill of the cut of a mesh to be made; while the Drawing view is shown it captures the drawing instead, and the answer says which in view (model or drawing). The text part gives the width and height in pixels. Fails while the window is minimised, and while the File view, Settings or the card of the Mesh to Plans wizard covers the viewport; file_view with open false returns to the model, mesh_to_plans_view with minimized true leaves the wizard as a strip that is not captured.", vec![
            optional("path", path("Absolute path ending in .png where the image is also written")),
            optional("max_edge", integer_in("Longest edge of the image in pixels, from 16 to 8192; default 1920. A larger viewport is scaled down", 16, 8192)),
            optional("window", boolean("true for the whole window as the application draws it, with ribbon, panels and status bar, also while the File view, Settings or the Mesh to Plans wizard covers the model; view is then window")),
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
            optional("orbit_point", xyz_or_null("Point [x, y, z] in scene coordinates that the camera turns about from now on, or null to turn about the centre of the model again; left out keeps the current orbit point (status.result.camera.orbit_point)")),
        ]),
        tool("orbit", Command, "Turns the orbit camera by these angles, as a left drag in the scene does: about the orbit point while it is in view, which keeps its place on screen, otherwise about the centre of the model.", vec![
            required("yaw", number_in("Turn about the vertical in radians, from -2π to 2π", -2.0 * PI, 2.0 * PI)),
            required("pitch", number_in("Change of elevation in radians, from -π to π; the elevation stays within ±1.56", -PI, PI)),
        ]),
        tool("pick_orbit_point", Command, "Makes the drawn point nearest to the camera within 8 pixels of a viewport pixel the orbit point, as a double click in the scene does; with no point there the camera turns about the centre of the model again. Answers with orbit_point, the point or null.", vec![
            required("pointer", pixel("Viewport pixel [x, y] from the top-left corner")),
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
        tool("close_panorama", Command, "Leaves the walking camera and returns to the orbit view. While a photo is entered it leaves the photo and puts the camera back where it was before the first photo was entered, into the station panorama it stood in then.", vec![]),
        tool("list_photos", Command, "Lists the photos of a layer that stand on their own, apart from the photos of scanner stations: panoramas and photos taken along a path (E57). Each has its index, kind (pinhole, spherical or cylindrical), name, width and height in pixels, position and viewing direction in scene coordinates, and station when the file names one; the answer also gives the coordinate_system the file states. The photos are in the order of the file, which is the order of the path.", vec![
            optional("layer", ordinal("Zero-based layer index; without it the active layer when it has photos, else the first layer with photos")),
        ]),
        tool("enter_photo", Command, "Stands where a photo was taken and lays the photo over the points: a panorama is looked around like a station panorama, a pinhole photo is first seen from its own camera with its field of view. Measuring and picking work on the points under the photo. status.result.photos.view reports the photo, with shown true once it is decoded and failed true when it cannot be; close_panorama returns.", vec![
            required("index", ordinal("Zero-based photo number, as list_photos gives it")),
            optional("layer", ordinal("Zero-based layer index; without it the active layer when it has photos, else the first layer with photos")),
        ]),
        tool("photo_blend", Command, "Sets how much of the entered photo covers the points, as the Photo slider does: 0 shows the points only, 1 the photo only. It stays for later photos.", vec![
            required("value", number_in("From 0 to 1", 0.0, 1.0)),
        ]),
        tool("next_photo", Command, "Enters the next photo along the path of the entered photo, as Page Down does; fails at the last photo.", vec![]),
        tool("previous_photo", Command, "Enters the previous photo along the path of the entered photo, as Page Up does; fails at the first photo.", vec![]),
        tool("colour_from_photos", Job, "Gives the points of a layer the colours its photos see them with (E57 panoramas, photos along a path and the photos of scanner stations): the remaining points inside the section box and class filters, or all of them without a section box. A photo colours a point when no point of the layer lies in front of it, within max_distance of the photo; with blend (the default) every photo that sees a point adds to its colour, weighted strongly to the nearest, otherwise the nearest alone, a pinhole photo preferring its middle over its edges. The colours replace those of the file in the viewport and in exports, as one edit that undo_delete takes back (Undo keeps at most 1 GiB of earlier photo colours and lets go of the oldest edits beyond that); color_mode becomes RGB. Answers with a job_id; the running job reports its stage (loading, reading, photos), part and parts, completed and total; the complete job reports points, coloured, unseen and unseen_share, photos (in reach), photos_used, photos_failed, seconds with times per stage, and compared: against the colours the points had, per channel R G B, mean_difference (photo minus stored), mean_abs_difference and median_abs_difference, or null when they had none.", vec![
            optional("layer", ordinal("Zero-based layer index; without it the active layer when it has photos, else the first layer with photos")),
            optional("max_distance", number_in("Largest distance from a photo to a point it colours, in metres, from 0.5 to 500; default 20", 0.5, 500.0)),
            optional("blend", boolean("Blend every photo that sees a point, weighted to the nearest (default true), or take the nearest alone")),
        ]),
        tool("cancel_colour_from_photos", Command, "Cancels the running colouring from photos; the colours stay as they were.", vec![]),
        tool("clear_photo_colours", Command, "Takes the photo colours of a layer away, as Remove photo colours in Properties does; undo_delete brings them back.", vec![
            optional("layer", ordinal("Zero-based layer index; without it the active layer")),
        ]),
        tool("list_camera_views", Command, "Lists the saved views of the active scan with their camera, section box, colour mode and annotations, and the name of the active view.", vec![]),
        tool("save_camera_view", Command, "Saves what the 3D viewport shows of the active scan (camera, the section box while it is on, colour mode) as a view and makes it the active view, showing the 3D scene when a drawing or the File view was in front. A snapshot image follows shortly after; wait_until_idle waits for it.", vec![
            optional("name", text("Name of the new view, unique within the scan; without it the first free \"View N\" is used", 1, 64)),
        ]),
        tool("update_camera_view", Command, "Overwrites a saved view with what the 3D viewport shows now, keeping its name, identifier and annotations, and makes it the active view, showing the 3D scene when a drawing or the File view was in front.", vec![
            required("name", view_name()),
        ]),
        tool("rename_camera_view", Command, "Renames a saved view of the active scan.", vec![
            required("name", view_name()),
            required("new_name", text("New name, unique within the scan", 1, 64)),
        ]),
        tool("restore_camera_view", Command, "Shows a saved view again (camera, its section box or the box switched off when it has none, colour mode) in the 3D scene and makes it the active view with its annotations.", vec![
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
        tool("set_section", Command, "Switches on a section box that clips the view, exports, selections and drawings. Without rotation it is an axis-aligned box inside the model bounds; with rotation it is the box min..max turned that many degrees about the vertical through its centre, so that it can follow walls at an angle to the axes. status.result.section reports min, max and rotation.", vec![
            required("min", xyz("Lowest corner [x, y, z] in scene coordinates, before the turn")),
            required("max", xyz("Highest corner [x, y, z] in scene coordinates, before the turn")),
            optional("rotation", number_in("Degrees counter-clockwise seen from above, about the vertical through the centre of the box; 0 or absent for a box along the axes", -3_600.0, 3_600.0)),
        ]),
        tool("clear_section", Command, "Switches the section box off.", vec![]),
        at_least_one(tool("set_section_fill", Command, "Chooses how the cut of a mesh by the section box is filled: where a wall, floor or ceiling has two opposite faces no farther apart than max_thickness, the material between them is closed with a cap of one colour on the faces of the box. A single surface, such as a facade scanned from one side, a loose sheet or the open edge of a mesh, gets no cap. What is left out is kept; the setting is kept for later sessions and status.result.section_fill reports it.", vec![
            optional("fill_cut", boolean("Whether the cut is filled; on by default")),
            optional("color", text("Colour of the caps as #rrggbb; #585858 by default", 6, 64)),
            optional("max_thickness", number_in("Maximum wall thickness: the largest distance between two opposite faces that is filled, in metres, from 0.01 to 2; 0.5 by default", 0.01, 2.0)),
        ])),
        tool("align_section_to_walls", Command, "Turns the section box along the main direction of the walls inside it, found in the middle half of its height; the box turns at most 45 degrees and keeps its size and centre. Answers when the search has started; wait_until_idle, then status.result.section.rotation holds the turn and status.result.status says what was found.", vec![]),
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
        tool("undo_delete", Command, "Takes back the latest edit: restores the latest deleted or thinned points, or gives a layer back the colours it had before its latest colouring from photos or removal of its photo colours.", vec![]),
        tool("redo_delete", Command, "Does again what undo_delete took back.", vec![]),
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
        tool("build_index", Command, "Starts building the disk octree of the active unindexed layer, or queues it ahead of the automatic builds while as many builds run as the computer takes at once; status.result.index lists the running and waiting builds.", vec![]),
        tool("cancel_index", Command, "Cancels every running octree build and empties the queue.", vec![]),
        tool("set_auto_index", Command, "Switches the automatic indexing of large clouds on or off.", vec![
            required("enabled", boolean("Whether large clouds are indexed automatically")),
        ]),
        tool("set_surface_settings", Command, "Sets the settings of the 3D surface in Properties, which mesh with the mode surface uses. A field that is left out keeps its value; the fields are checked together with the others, and when one is refused none changes. Answers with the settings.", vec![
            optional("max_vertices", integer_in("Most vertices, from 3 to 1000000. Properties starts at 50000", 3, 1_000_000)),
            optional("neighbors", integer_in("Neighbours per vertex, from 3 to 32. Properties starts at 12", 3, 32)),
            optional("edge_factor", positive("Longest edge relative to the typical point spacing. Properties starts at 4")),
            optional("mesh_size", number_from("Width of a voxel in the units of the scan within which one point is kept before the vertices are thinned, so that no two vertices lie much closer together, from 0. Properties starts at 0, which leaves the spacing to max_vertices", 0.0)),
        ]),
        tool("mesh", Job, "Makes a mesh from the remaining points inside the section box and class filters and gives it to the active layer, where it takes the place of the mesh that layer had. terrain (2.5D, from the lowest points seen from above) and surface (3D, from a sample; not watertight) take the active layer and write an OBJ file. closed makes a surface without overlaps that is closed wherever the scan has points or a gap narrower than max_hole: it takes the active layer or every visible one, needs an index for a layer of more than 5,000,000 points, and stops when the result would exceed 4,000,000 vertices or 8,000,000 triangles; put the section box around one room or a few. Answers with a job_id. The complete job reports the vertices and triangles, the open edges (edges with one triangle: rims and holes) and the connected parts (components); for closed also the mean, 95th percentile and largest distance between points and mesh in metres (deviation_mean, deviation_p95, deviation_max), the voxel used, where the sides came from (sides: stations, mixed or fallback), the surface elements without a station and those whose side nothing told (surfels_without_station, surfels_undecided), and advice when those figures call for it.", {
            let mut arguments = vec![
                required("mode", choice("terrain, surface or closed", &["terrain", "surface", "closed"])),
                optional("path", path(MESH_JOB_FILE)),
            ];
            arguments.extend(closed_mesh_settings());
            arguments
        }),
        tool("set_closed_mesh_settings", Command, "Sets the settings of the Closed mesh block in Properties, which mesh with the mode closed and the Start button of the block use. The fields given are checked together: when one is refused, none changes. Answers with the settings as status.result.closed_mesh.settings reports them.", closed_mesh_settings()),
        tool("cancel_mesh", Command, "Cancels the running mesh job; a cancelled job leaves the mesh of the layer and an existing file as they were.", vec![]),
        tool("export_mesh", Job, "Saves the mesh the active layer holds (a terrain mesh, a 3D surface, a closed mesh, an opened mesh file or downloaded 3D BAG buildings; status.result.clouds[].mesh is null for a layer without one) as OBJ, PLY, STL, DXF, DWG or IFC, moved and scaled as in the scene. Answers with a job_id; the complete job reports the format, vertices and triangles. An STL file holds 32-bit floats: a mesh farther than 2,048 m from zero on an axis is written relative to a whole-metre origin, which the job reports as origin and the file header names; other programs show such a file near zero.", vec![
            required("path", path(MESH_FILE)),
        ]),
        tool("set_face_settings", Command, "Sets the settings of the Detect faces block in Properties, which detect_faces and the Start button of the block use, and the colouring of the faces that are shown. The fields given are checked together: when one is refused, none changes. Answers with the settings as status.result.faces.settings reports them.", face_settings()),
        tool("detect_faces", Job, "Finds the flat faces (floors, ceilings, walls and sloped planes) and the round columns and pipes in the remaining points inside the section box and class filters, of the active layer or of every visible one, and keeps them with the active layer as a layer of its own beside its mesh; faces that layer had are replaced. Put the section box around one room or a few: a region too large for 1,500,000 working voxels of 30 mm is searched with larger voxels, and narrow faces and faces close together are lost. Answers with a job_id; the running job reports its stage (stations, loading, reading, segmenting, measuring, outlining, meshing), the complete job the faces per type (floors, ceilings, walls, sloped, cylinders), the edges, the voxel used and whether it was coarse, the points read and on a face, and the seconds. The faces follow a later translate or scale of the layer; they are marked stale (status.result.clouds[].faces.stale: points, index, scan or moved) when points of a layer that took part are deleted, restored or thinned, when the index such a layer was read through is replaced, when one is removed, or when one of several layers that took part is translated or scaled.", face_settings()),
        tool("cancel_detect_faces", Command, "Cancels the running face detection; the step under way ends first, and faces the layer had are left as they were.", vec![]),
        tool("list_faces", Command, "Lists the faces detected in the active layer, in scene coordinates and metres, planes first and then cylinders, each largest first: for a plane its id, class (floor, ceiling, wall or sloped, by the direction of its normal alone), normal, a point, the offset d of normal . x = d, area, covered_area and coverage, and residual (points, inliers, rms, mean, mean_abs, p95, max); for a cylinder its axis_start, axis_end, radius, diameter, length, arc_degrees and residual. Also the settings and voxel of the detection, the selected face and whether the faces are stale.", vec![
            optional("boundaries", boolean("Whether every plane comes with its outline (boundary: per connected part an outer ring and the rings of its openings, as [x, y, z] corners) and the answer with the edges between faces; default false")),
        ]),
        tool("select_face", Command, "Highlights one face of the active layer in the viewport and in the list of the block, by its id from list_faces, or takes the highlight off. Answers with that face, outline included.", vec![
            optional("id", json!({"type": ["integer", "null"], "minimum": 1, "description": "Number of the face; null or absent for none"})),
        ]),
        tool("export_faces", Job, "Saves the faces detected in the active layer in scene coordinates, as they stand after a move or scale of the layer: as JSON with the parameters of every plane and cylinder, the outlines, the residuals, the edges and the settings (the file names only the file name of the scan, not its folder), as OBJ with one group per face, as DXF or DWG with 3D polyface meshes per layer, or as IFC4 with a building element proxy per face. Answers with a job_id; the complete job reports the format, the planes, cylinders and edges written, the file size and whether the faces were stale.", vec![
            required("path", path(FACES_FILE)),
        ]),
        tool("clear_faces", Command, "Removes the faces detected in the active layer; answers with cleared false when it had none.", vec![]),
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
        tool("drawing_view", Command, "Shows the Drawing view in the main area in place of the 3D scene, or the 3D scene again. The view shows the drawing that preview_drawing or export_drawing made last, or the file open_drawing read. Answers with drawing_view as status.result.drawing_view reports it: shown, the source, units, counts, layers with their visibility, extents and the camera. A screenshot while it is shown captures the drawing.", vec![
            required("show", boolean("true to show the drawing, false to return to the 3D scene")),
        ]),
        tool("open_drawing", Job, "Reads a DXF or DWG file into the Drawing view and shows it: points, lines, polylines with their arcs, circles, arcs, ellipses, solid fills with holes, 2D solids, texts and block references. Other entities are counted by type in skipped, 3D content such as meshes in skipped_3d; neither is shown. A file without units is read as millimetres. Answers with a job_id; the complete job reports the units, layers, points, polylines, fills, texts, inserts and what was skipped.", vec![
            required("path", path("Absolute path of an existing .dxf or .dwg file")),
        ]),
        tool("drawing_zoom_extents", Command, "Zooms the Drawing view so that the whole drawing fits; answers with the camera (center in drawing units, pixels_per_unit). Fails without a drawing.", vec![]),
        tool("set_drawing_layer", Command, "Shows or hides a layer of the drawing in the Drawing view, by its name as status.result.drawing_view.drawing.layers lists it (any case), or every layer with *.", vec![
            required("layer", text("Layer name, or * for all layers", 1, 255)),
            required("visible", boolean("Whether the layer is shown")),
        ]),
        tool("create_drawing", Job, "Makes a plan, an elevation or a section as Create 2D plan / elevation / section in the Project Browser does, from every visible layer with the other settings of the Section drawing block, and shows it in the Drawing view. It is listed under VIEWS, and how it was made (the box, the face, the slab, the settings and the scans) is kept, so that show_drawing makes it again in a later session. Answers with a job_id; the complete job reports its name, guid and kind and the regions of the filled cut.", vec![
            required("kind", choice("What to make", &crate::sheet_dialog::SheetKind::ALL.map(crate::sheet_dialog::SheetKind::key))),
            optional("basis", text("model for the whole 3D model (the default), section_box for the section box while it is on, or the name of a saved view of the active scan with a section box", 1, 64)),
            optional("side", choice("The side an elevation or a section looks at: front looks along +Y, back along -Y, left along +X and right along -X", &["front", "back", "left", "right"])),
            optional("height", number("Height of the cut of a plan made from the model, in scene units; by default 1.20 above the floor of the model")),
            optional("position", number("Where a section made from the model cuts, along the axis it looks along, in scene units; by default the middle of the model")),
            optional("thickness", positive_up_to("Depth of the slab behind the cut of a plan or a section in metres, 0.005 to 5; 0.10 by default", 5.0)),
            optional("name", text("Name of the drawing; without it the drawing is named after its kind and what it was made from", 1, 96)),
        ]),
        tool("list_drawings", Command, "Lists the drawings of create_drawing made from an open scan, with how each was made and whether it is made and shown in this session, and the previews, exports and opened files of this session.", vec![]),
        tool("show_drawing", Command, "Shows a drawing of create_drawing in the Drawing view by its name, any case. A drawing that is not made in this session yet is made again from how it was made, from its scans, which must be open; the answer then has accepted and a job_id to wait for. The name 3D model shows the 3D model, as a click on its row does: the active view lets go and its annotations are hidden.", vec![
            required("name", text("Name of the drawing as list_drawings gives it", 1, 96)),
        ]),
        tool("delete_drawing", Command, "Deletes a drawing of create_drawing by its name, any case, with how it was made.", vec![
            required("name", text("Name of the drawing as list_drawings gives it", 1, 96)),
        ]),
        tool("set_browser_group", Command, "Opens or collapses a group of the Project Browser: scans, classes, views or bcf, a kind under views (3d, plans, elevations, sections, files), or the scans of one folder as folder: followed by the path of the folder. Collapsing changes nothing that is loaded or shown; the window keeps the choice. status.result.project_browser reports the groups, what VIEWS lists and, as shown, the row that is highlighted.", vec![
            required("group", text("The group", 1, 4096)),
            required("open", boolean("true to open the group, false to collapse it")),
        ]),
        tool("set_sheet_crop", Command, "Sets the crop region of a drawing of create_drawing, as dragging its handles in the Drawing view or the Crop region section of Properties does, and makes the drawing again under the same name; only the faces of its box in the plane of the drawing move. Give rect alone, or any of the figures. Answers with accepted, a job_id and the new crop, or with changed: false.", vec![
            optional("name", text("Name of the drawing as list_drawings gives it; without it the drawing shown in the Drawing view", 1, 96)),
            optional("rect", list("The crop region as [[left, bottom], [right, top]] in the units and coordinates of the drawing, as list_drawings gives it under crop.rect", numbers("A corner [u, v]", 2), 2, 2)),
            optional("width", positive("Width of the crop region in metres, at least 0.10, about its centre")),
            optional("height", positive("Height of the crop region in metres, at least 0.10, about its centre")),
            optional("center", numbers("The centre: the model X and Y for a plan; for an elevation or a section its place along the box and its height", 2)),
            optional("rotation", number("The turn of the box of a plan in degrees, counter-clockwise seen from above")),
            optional("cut", number("A plan: the height of the cut. An elevation or a section: where the cut lies along the direction it looks, measured along the box")),
            optional("depth", positive("The view depth behind the cut in metres: the slab of a plan or a section (0.005 to 5), the depth of the box of an elevation")),
        ]),
        tool("drag_crop_handle", Command, "Drags a handle of the crop region of the drawing the Drawing view shows to a point of the drawing, as the pointer does: the side or the two sides it moves go there, with the size in whole centimetres and at least 0.10 m. With release false the handle is held and the region is drawn as during a drag, with its size; let go (the default), the drawing is made again in place, with a job_id.", vec![
            required("handle", choice("The handle", &["left", "right", "bottom", "top", "bottom_left", "bottom_right", "top_left", "top_right"])),
            required("to", numbers("The point [u, v] of the drawing, in its units and coordinates, as list_drawings gives crop.rect", 2)),
            optional("release", boolean("false to hold the handle there without letting go; true by default")),
        ]),
        tool("duplicate_view", Command, "Duplicates a row of VIEWS in the Project Browser: a saved view of the active scan, a drawing of create_drawing (with the drawing as it is made, without computing it again) or the 3D model (the current 3D view, with the section box while it is on, saved as a view). The copy is named with \" (2)\" or the next free number after the name, listed right below the original, shown, and changes on its own. Answers with its kind, name and guid; a drawing that is not made yet is made, with a job_id, and when it cannot be made now no copy is kept.", vec![
            required("name", text("Name of the view or drawing, any case; 3D model for the default 3D view", 1, 96)),
            optional("kind", choice("What the name is; without it a saved view of that name, else a drawing, else the 3D model", &["model", "view", "drawing"])),
        ]),
        tool("rotate_crop", Command, "Turns what the keys R and then O turn: the crop region of a plan of create_drawing, counter-clockwise on the sheet (its box turns as far about the vertical through the centre of the region and the plan is made again upright in it, with a job_id), or in the 3D view the section box about its centre. With apply false the turn starts as RO starts it, shown at the angle given, and waits for Enter, a click, Escape or this command with apply true and no degrees. status.result.turning reports a turn under way.", vec![
            optional("name", text("Name of a plan as list_drawings gives it; without it the plan shown in the Drawing view, or the section box in the 3D view", 1, 96)),
            optional("degrees", number_in("The turn in degrees, counter-clockwise", -3600.0, 3600.0)),
            optional("apply", boolean("false to start the turn and show it without applying it; true by default")),
        ]),
        tool("open_in_cad_viewer", Command, "Opens a DXF or DWG file in the CAD viewer: the Open CAD Studio that comes with the application, else the program chosen in Settings, else an installed Open CAD Studio, started read-only and without waiting for it; without any of them the file goes to the program the system has for it. Without a path it opens the last file that a drawing, faces or mesh export wrote. Answers with the path, the viewer program (null for the system program) and read_only. status.result.cad_viewer tells which viewer was found.", vec![
            optional("path", path("Absolute path of an existing .dxf or .dwg file; without it the last exported one")),
        ]),
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
        tool("file_view", Command, "Opens the File view over the model, on a page when one is named, or closes it and returns to the model. A screenshot needs it closed. Opening is refused while the Settings dialog or the card of the Mesh to Plans wizard is open.", vec![
            required("open", boolean("true to show the File view, false to return to the model")),
            optional("page", choice("Page to show, only with open true; without it the view opens on workspace, or keeps the page it shows", &crate::file_view::FilePage::ids())),
        ]),
        tool("mesh_to_plans_view", Command, "Shows the Mesh to Plans wizard, the card that makes plans, sections, elevations, a site plan and a model of a building step by step, or takes it away. With open true it is shown as its card over the window, on a step when one is named, and with minimized true as a strip above the scene instead: the card covers the model, so a screenshot is refused while it is shown, and the strip covers nothing. Opening closes the File view and is refused while the Settings dialog is open. Answers with mesh_to_plans as status.result.mesh_to_plans reports it: open, minimized, the step shown, whether Next may leave it (next_ready, with next_reason when not) and the id, number, name and status of every step.", vec![
            required("open", boolean("true to show the wizard, false to take it away; what its steps hold stays")),
            optional("step", choice("Step to show, only with open true; without it the wizard keeps the step it shows", &crate::mesh_to_plans::WizardStep::ids())),
            optional("minimized", boolean("Only with open true: true shows the wizard as the strip above the scene, false or absent as the card")),
        ]),
        tool("mesh_to_plans_action", Command, "Does what a button of the Mesh to Plans wizard does on the step it shows, whether the wizard is shown or not: run starts a job for that step and run_all one for every step not yet confirmed or skipped (answering with its job_id for job and wait_for_job), confirm accepts a step that waits for confirmation (for step 0, prepare, this is Confirm levels), skip skips an optional step, cancel stops the running job, back and next go to the step before or after it, and resume opens the project in folder (or the project file folder names) on the first step that is not done, as Resume in the Project Browser does. Step 0 reads the shown scans: it finds the box around the building, the main direction of its walls, the footprint and the levels with their heights to the millimetre, and writes the project file and survey/profile.csv and survey/top.png in the project folder. Refused when the button would be disabled, with the reason. Answers with mesh_to_plans as status.result.mesh_to_plans reports it, including the project file and, after step 0, prepare with the frame and the levels above P.", vec![
            required("action", choice("What to do", &["run", "run_all", "confirm", "skip", "cancel", "back", "next", "resume"])),
            optional("folder", path("Absolute path of the folder of a new project, before it was first written (by default Documents/OPS Mesh to Plans/<name of the first scan>); for resume the folder or file of the project to open")),
        ]),
        tool("mesh_to_plans_level", Command, "Does what the page of step 0 (prepare) of the Mesh to Plans wizard does with a level, once step 0 has run. The level named by its id (as status.result.mesh_to_plans.prepare.levels lists them: 00 for the floor that is P, 01 and up above it, -01 and down below it, R for the roof) is selected; then it gets name, cut_height and floor_above_p when they are given, and then action is done: select (nothing more), show (Show in model: the card becomes the strip and the section box takes that storey; mesh_to_plans_view with open true puts the box back), set_peil (that whole floor becomes P and the others are numbered from it), merge (with the level above it), remove, or add (a level 3 m above the selected or highest floor; level may be left out). Confirmed levels can only be selected and shown. Answers with mesh_to_plans as status.result.mesh_to_plans reports it.", vec![
            optional("level", text("Id of the level, as prepare.levels lists it; required except for add", 1, 64)),
            optional("action", choice("What to do after the changes; select by default", &crate::mesh_to_plans::LEVEL_ACTIONS)),
            optional("name", text("New name of the level", 1, 60)),
            optional("cut_height", number_in("Height of the cut of its plan above its floor, in metres", 0.3, 3.0)),
            optional("floor_above_p", number_in("New height of its floor above P, in metres; ceiling and slab move along", -1000.0, 1000.0)),
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
        ("section_align_pending", "section_align"),
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
    if ["listing", "decoding"].into_iter().any(|key| {
        result["photos"][key]
            .as_u64()
            .is_some_and(|count| count > 0)
    }) {
        busy.push("file_photos");
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
    if !result["drawing_view"]["reading"].is_null() {
        busy.push("drawing_view");
    }
    if result["closed_mesh"]["job"].is_object() {
        busy.push("closed_mesh");
    }
    if result["faces"]["job"].is_object() {
        busy.push("faces");
    }
    if result["faces"]["export_pending"] == true {
        busy.push("faces_export");
    }
    if result["section_fill"]["pending"] == true {
        busy.push("section_caps");
    }
    if result["mesh_to_plans"]["job"].is_object() {
        busy.push("mesh_to_plans");
    }
    if result["colour_from_photos"]["job"].is_object() {
        busy.push("colour_from_photos");
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
