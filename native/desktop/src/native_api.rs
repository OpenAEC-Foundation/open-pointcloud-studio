//! Local JSON command bridge for the native GUI. No script evaluation or webview.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const MAX_BODY_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ApiCommand {
    Status,
    Job {
        id: String,
    },
    Open {
        path: PathBuf,
    },
    CancelImport {
        id: u64,
    },
    Remove {
        index: usize,
    },
    SetActive {
        index: usize,
    },
    SetVisible {
        index: usize,
        visible: bool,
    },
    Camera {
        preset: String,
    },
    SetCamera {
        yaw: f32,
        pitch: f32,
        zoom: f32,
        pan: [f32; 2],
        /// Left out keeps the orbit point; `null` turns about the centre of
        /// the scene again.
        #[serde(default, deserialize_with = "given_point")]
        orbit_point: Option<Option<[f64; 3]>>,
    },
    PickOrbitPoint {
        pointer: [f32; 2],
    },
    Orbit {
        yaw: f32,
        pitch: f32,
    },
    OpenPanorama {
        index: usize,
        station: usize,
    },
    SetPanorama {
        yaw: f32,
        pitch: f32,
        field_of_view: f32,
    },
    Walk {
        eye: [f64; 3],
        yaw: f32,
        pitch: f32,
    },
    ClosePanorama,
    /// The photos of a layer that are not those of its stations.
    ListPhotos {
        /// The layer; without it the active layer when it has photos, else
        /// the first layer with photos.
        #[serde(default)]
        layer: Option<usize>,
    },
    EnterPhoto {
        /// Zero-based place of the photo in `list_photos`.
        index: usize,
        #[serde(default)]
        layer: Option<usize>,
    },
    PhotoBlend {
        /// 0 shows the points only, 1 the photo only.
        value: f32,
    },
    NextPhoto,
    PreviousPhoto,
    ZoomAll,
    ListCameraViews,
    SaveCameraView {
        /// Empty or absent for the next free "View N".
        #[serde(default)]
        name: String,
    },
    UpdateCameraView {
        name: String,
    },
    RenameCameraView {
        name: String,
        new_name: String,
    },
    RestoreCameraView {
        name: String,
    },
    DeleteCameraView {
        name: String,
    },
    AddNote {
        point: [f64; 3],
        text: String,
    },
    AddLine {
        from: [f64; 3],
        to: [f64; 3],
    },
    DeleteAnnotation {
        index: usize,
    },
    SetAnnotationTool {
        /// `note`, `line`, or null to leave the tool.
        tool: Option<String>,
    },
    AnnotateScreen {
        pointer: [f32; 2],
    },
    SubmitNote {
        text: String,
    },
    ExportBcf {
        path: PathBuf,
    },
    SetTheme {
        theme: String,
    },
    SetLanguage {
        /// `auto` for the language of the system, `en`, or the code of a
        /// translation such as `nl`.
        language: String,
    },
    SetColor {
        mode: String,
    },
    SetClassVisible {
        code: u8,
        visible: bool,
    },
    SetPointSize {
        size: f32,
    },
    SetEyeDome {
        enabled: bool,
    },
    SetEyeDomeStrength {
        strength: f32,
    },
    SetBudget {
        points: u32,
    },
    SetSection {
        min: [f64; 3],
        max: [f64; 3],
        /// Degrees counter-clockwise from above about the vertical through
        /// the centre of the box; absent for an axis-aligned box.
        rotation: Option<f64>,
    },
    ClearSection,
    /// How the cut of a mesh by the section box is filled; what is left out
    /// is kept.
    SetSectionFill {
        fill_cut: Option<bool>,
        /// `#rrggbb`.
        color: Option<String>,
        /// Metres, from 0.01 to 2.
        max_thickness: Option<f64>,
    },
    /// Turn the section box along the walls inside it.
    AlignSectionToWalls,
    SelectWorld {
        min: [f64; 3],
        max: [f64; 3],
    },
    PickScreen {
        pointer: [f32; 2],
        radius: Option<f32>,
    },
    CancelSelection,
    ClearSelection,
    Measure {
        mode: String,
        points: Vec<[f64; 3]>,
    },
    ClearMeasure,
    ZoomSelection,
    DeleteSelection,
    UndoDelete,
    RedoDelete,
    Thin {
        percent: u8,
    },
    Translate {
        offset: [f64; 3],
    },
    Scale {
        factors: [f64; 3],
    },
    CancelScale,
    BuildIndex,
    CancelIndex,
    SetAutoIndex {
        enabled: bool,
    },
    /// The settings of the 3D surface; a field that is left out keeps what
    /// Properties has.
    SetSurfaceSettings {
        max_vertices: Option<usize>,
        neighbors: Option<usize>,
        edge_factor: Option<f64>,
        mesh_size: Option<f64>,
    },
    ResetTransform,
    Mesh {
        /// `terrain`, `surface` or `closed`.
        mode: String,
        /// Absolute destination: `.obj` for a terrain mesh or a 3D surface.
        /// A closed mesh needs none; with one (`.obj`, `.ply` or `.stl`) it
        /// is also written there.
        #[serde(default)]
        path: Option<PathBuf>,
        /// Settings of a closed mesh; refused with the other modes.
        #[serde(flatten)]
        options: crate::closed_mesh::ClosedMeshOptions,
    },
    SetClosedMeshSettings {
        #[serde(flatten)]
        options: crate::closed_mesh::ClosedMeshOptions,
    },
    CancelMesh,
    ExportMesh {
        /// Absolute destination; its extension (`.obj`, `.ply` or `.stl`)
        /// chooses the format.
        path: PathBuf,
    },
    SetFaceSettings {
        #[serde(flatten)]
        options: crate::faces::FaceOptions,
    },
    DetectFaces {
        /// Settings of the Detect faces block; those left out keep what the
        /// block has.
        #[serde(flatten)]
        options: crate::faces::FaceOptions,
    },
    CancelDetectFaces,
    /// Colour the points of a layer from its photos.
    ColourFromPhotos {
        /// The layer and the settings of the Colour from photos block;
        /// settings left out keep what the block has.
        #[serde(flatten)]
        options: crate::photo_colours::ColourOptions,
    },
    CancelColourFromPhotos,
    /// Take the photo colours of a layer away, as Remove photo colours does.
    ClearPhotoColours {
        /// The layer; without it the active layer.
        #[serde(default)]
        layer: Option<usize>,
    },
    ListFaces {
        /// Whether every face comes with its outline and the answer with the
        /// edges between the faces.
        #[serde(default)]
        boundaries: bool,
    },
    SelectFace {
        /// The number of the face to highlight; null or absent for none.
        #[serde(default)]
        id: Option<u32>,
    },
    ExportFaces {
        /// Absolute destination; its extension (`.json` or `.obj`) chooses
        /// the format.
        path: PathBuf,
    },
    ClearFaces,
    Export {
        path: PathBuf,
    },
    ExportSection {
        path: PathBuf,
    },
    ExportSelection {
        path: PathBuf,
    },
    ExportMinusSelection {
        path: PathBuf,
    },
    ExportDrawing {
        /// Absolute destination; its extension (`.dxf` or `.dwg`) chooses
        /// the format.
        path: PathBuf,
        #[serde(flatten)]
        options: crate::drawing::DrawingOptions,
    },
    PreviewDrawing {
        #[serde(flatten)]
        options: crate::drawing::DrawingOptions,
    },
    ClearDrawingPreview,
    CancelDrawing,
    /// Show the Drawing view in the main area, or the 3D scene again.
    DrawingView {
        show: bool,
    },
    /// Read a DXF or DWG file into the Drawing view and show it.
    OpenDrawing {
        path: PathBuf,
    },
    DrawingZoomExtents,
    SetDrawingLayer {
        /// The name of a layer of the drawing, any case, or `*` for all.
        layer: String,
        visible: bool,
    },
    /// Make a plan, an elevation or a section as Create 2D plan /
    /// elevation / section does.
    CreateDrawing {
        #[serde(flatten)]
        options: CreateDrawingOptions,
    },
    /// The drawings of Create 2D made from an open scan, and the previews,
    /// exports and files of this session.
    ListDrawings,
    /// Show a drawing of Create 2D by its name, made again when it is not
    /// made yet in this session.
    ShowDrawing {
        name: String,
    },
    DeleteDrawing {
        name: String,
    },
    /// Open or collapse a group of the Project Browser.
    SetBrowserGroup {
        group: String,
        open: bool,
    },
    /// The tabs above the main area, in their order, and the active one.
    ListTabs,
    /// Show an open tab, by its place as `list_tabs` gives it or by its
    /// name, as a click on it does.
    ShowTab {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        index: Option<usize>,
    },
    /// Close an open tab as its × does; its view or drawing stays.
    CloseTab {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        index: Option<usize>,
    },
    /// Set the crop region of a drawing of Create 2D, which is made again.
    SetSheetCrop {
        #[serde(flatten)]
        options: crate::drawing_crop::SheetCropOptions,
    },
    /// Lock a saved view, a drawing or a viewport on a sheet, or unlock it.
    LockView {
        #[serde(flatten)]
        options: crate::locks::LockOptions,
    },
    UnlockView {
        #[serde(flatten)]
        options: crate::locks::LockOptions,
    },
    /// The sheets of SHEETS with the views placed on them.
    ListSheets,
    CreateSheet {
        #[serde(flatten)]
        options: crate::layouts::SheetOptions,
    },
    UpdateSheet {
        #[serde(flatten)]
        options: crate::layouts::SheetOptions,
    },
    DuplicateSheet {
        #[serde(default)]
        sheet: Option<String>,
    },
    DeleteSheet {
        #[serde(default)]
        sheet: Option<String>,
    },
    ShowSheet {
        #[serde(default)]
        sheet: Option<String>,
    },
    /// Place a saved view or a drawing on a sheet.
    PlaceView {
        #[serde(flatten)]
        options: crate::layouts::PlaceOptions,
    },
    UpdateViewport {
        #[serde(flatten)]
        options: crate::layouts::ViewportOptions,
    },
    RemoveViewport {
        #[serde(default)]
        sheet: Option<String>,
        viewport: crate::layouts::ViewportRef,
    },
    /// Write a sheet as a PDF.
    ExportSheetPdf {
        #[serde(default)]
        sheet: Option<String>,
        path: PathBuf,
    },
    /// Duplicate a row of VIEWS: the 3D model, a saved view or a drawing.
    DuplicateView {
        name: String,
        /// `model`, `view` or `drawing`; without it a saved view of that
        /// name, else a drawing, else the 3D model.
        #[serde(default)]
        kind: Option<String>,
    },
    /// Select or deselect the crop region of the drawing shown, as a click
    /// on its outline or Escape does.
    SelectCropRegion {
        selected: bool,
    },
    /// Drag a handle of the crop region of the drawing shown, as the
    /// pointer does.
    DragCropHandle {
        handle: String,
        to: [f64; 2],
        #[serde(default = "applied")]
        release: bool,
    },
    /// Turn what RO turns: the crop region of a plan, or the section box.
    RotateCrop {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        degrees: Option<f64>,
        #[serde(default = "applied")]
        apply: bool,
    },
    OpenInCadViewer {
        /// Absolute `.dxf` or `.dwg` file; without it the last one a drawing,
        /// faces or mesh export wrote.
        #[serde(default)]
        path: Option<PathBuf>,
    },
    MergeVisible {
        path: PathBuf,
    },
    CancelMerge,
    Bag3d {
        /// The area as `[xmin, ymin, xmax, ymax]` in RD New (EPSG:28992).
        bbox: [f64; 4],
        /// `1.2`, `1.3` or `2.2`.
        lod: String,
        path: PathBuf,
    },
    CancelBag3d,
    ListExtensions,
    SetExtensionEnabled {
        id: String,
        enabled: bool,
    },
    /// Copy and check an extension from a folder or a `.zip` archive and
    /// ask the user in the window to confirm its install.
    InstallExtension {
        path: PathBuf,
    },
    /// Start an installed extension, as a click on its button does.
    RunExtension {
        id: String,
        /// The id of a ribbon button or a File view tile of the extension,
        /// whose arguments are added; without it none are.
        #[serde(default)]
        entry: Option<String>,
    },
    StopExtension {
        id: String,
    },
    /// Show a message in the status bar.
    ShowMessage {
        text: String,
    },
    /// Show how far a task is in the status bar.
    ReportProgress {
        percent: f64,
        #[serde(default)]
        text: Option<String>,
    },
    /// Ask the user for a path with a dialog of the window.
    ChoosePath {
        #[serde(flatten)]
        options: PathRequest,
    },
    /// What the window shows: the active scan, the selection, the section
    /// box and the view or drawing shown.
    Context,
    FileView {
        open: bool,
        /// The page to show, with `open: true`; without it the File view
        /// opens on its first page or keeps the page it shows.
        #[serde(default)]
        page: Option<String>,
    },
    /// Show the card of Mesh Pointcloud, with a method and on a step, or
    /// take it away.
    MeshWizard {
        open: bool,
        /// `method`, `options` or `run`, with `open: true`; without it the
        /// card shows the step it showed last, or the Run step of a job
        /// that runs.
        #[serde(default)]
        step: Option<String>,
        /// `closed`, `terrain`, `surface` or `faces`, with `open: true`.
        #[serde(default)]
        method: Option<String>,
    },
    MeshToPlansView {
        open: bool,
        /// The step to show, with `open: true`; without it the wizard keeps
        /// the step it shows.
        #[serde(default)]
        step: Option<String>,
        /// With `open: true`, whether the wizard is shown as the strip
        /// above the scene instead of as the card; without it the card.
        #[serde(default)]
        minimized: Option<bool>,
    },
    MeshToPlansAction {
        /// `run`, `run_all`, `confirm`, `skip`, `cancel`, `back` or `next`:
        /// what the buttons of the card do, on the step it shows.
        action: String,
        /// The folder of a new project, before step 0 first ran.
        #[serde(default)]
        folder: Option<PathBuf>,
    },
    MeshToPlansLevel {
        /// The id of a level of step 0, as `status` lists it; not for `add`.
        #[serde(default)]
        level: Option<String>,
        /// `select` (the default), `show`, `set_peil`, `add`, `merge` or
        /// `remove`.
        #[serde(default)]
        action: Option<String>,
        #[serde(default)]
        name: Option<String>,
        /// The cut of its plan above its floor, in metres.
        #[serde(default)]
        cut_height: Option<f64>,
        /// Where its floor goes, in metres above P.
        #[serde(default)]
        floor_above_p: Option<f64>,
    },
    Screenshot {
        /// Absolute `.png` path to write the image to.
        path: Option<PathBuf>,
        /// Whether the answer carries the PNG as base64; without a path it does.
        base64: Option<bool>,
        /// Longest edge of the image in pixels.
        max_edge: Option<u32>,
        /// The whole window, with ribbon, panels and status bar, instead of
        /// the scene.
        #[serde(default)]
        window: bool,
    },
}

/// The dialog `choose_path` shows.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PathRequest {
    /// `open` for an existing file, `save` for a file to write, or
    /// `folder`.
    pub mode: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub filters: Vec<PathFilter>,
    /// The name a save dialog proposes.
    #[serde(default)]
    pub file_name: Option<String>,
    /// The folder the dialog starts in.
    #[serde(default)]
    pub directory: Option<PathBuf>,
}

/// A kind of file a dialog of `choose_path` offers.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathFilter {
    pub name: String,
    /// Extensions without the dot, such as `csv`.
    pub extensions: Vec<String>,
}

/// A turn of `rotate_crop` is applied unless asked otherwise.
fn applied() -> bool {
    true
}

/// The choices of `create_drawing`, those of the Create 2D dialog. Each one
/// left out keeps what the dialog starts with.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CreateDrawingOptions {
    /// `plan`, `elevation` or `section`.
    pub kind: String,
    /// `model`, `section_box` or the name of a saved view of the active scan
    /// with a section box.
    #[serde(default)]
    pub basis: Option<String>,
    /// `front`, `back`, `left` or `right`, for an elevation or a section.
    #[serde(default)]
    pub side: Option<String>,
    /// The height of the cut of a plan made from the model.
    #[serde(default)]
    pub height: Option<f64>,
    /// Where a section made from the model cuts, along the axis it looks.
    #[serde(default)]
    pub position: Option<f64>,
    #[serde(default)]
    pub thickness: Option<f64>,
    /// The points used, in percent: 0.1 to 100.
    #[serde(default)]
    pub sample_percent: Option<f64>,
    /// The name of the drawing; without one it is named as the dialog
    /// names it.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ApiRequest {
    pub command: ApiCommand,
    pub reply: Sender<Value>,
}

/// A request as the server hands it to the window, with the extension whose
/// token sent it; `None` for the token of the discovery file.
pub struct Delivery {
    pub request: ApiRequest,
    pub caller: Option<String>,
}

/// The commands an extension may always send with the token of its run:
/// they act on that run, or on nothing but the status bar and a dialog.
pub const EXTENSION_COMMANDS: [&str; 5] = [
    "show_message",
    "report_progress",
    "context",
    "choose_path",
    "job",
];

/// What the token of a run of an extension may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub extension: String,
    /// The commands it declared; `None` when it declared every command.
    pub commands: Option<BTreeSet<String>>,
}

impl Grant {
    pub fn allows(&self, command: &str) -> bool {
        EXTENSION_COMMANDS.contains(&command)
            || self
                .commands
                .as_ref()
                .is_none_or(|commands| commands.contains(command))
    }
}

/// The tokens of the runs of extensions, besides the token of the
/// discovery file: the window adds one when a run starts and takes it away
/// when the run ends.
#[derive(Clone, Debug, Default)]
pub struct Grants(Arc<Mutex<HashMap<String, Grant>>>);

impl Grants {
    pub fn insert(&self, token: String, grant: Grant) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(token, grant);
    }

    pub fn remove(&self, token: &str) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(token);
    }

    fn get(&self, token: &str) -> Option<Grant> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(token)
            .cloned()
    }
}

pub struct ApiHandle {
    pub port: u16,
    pub grants: Grants,
    discovery_path: PathBuf,
}

impl Drop for ApiHandle {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.discovery_path);
    }
}

/// A field that tells "left out" from `null`.
fn given_point<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Option<[f64; 3]>>, D::Error> {
    Option::<[f64; 3]>::deserialize(deserializer).map(Some)
}

pub fn discovery_directory() -> PathBuf {
    crate::preferences::config_directory()
        .unwrap_or_else(|| std::env::temp_dir().join("open-pointcloud-studio-native"))
        .join("instances")
}

fn remove_stale_instances(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("instance-") && name.ends_with(".json"))
        {
            continue;
        }
        let port = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| value["port"].as_u64())
            .and_then(|port| u16::try_from(port).ok());
        let alive = port.is_some_and(|port| {
            let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok()
        });
        if !alive {
            let _ = fs::remove_file(path);
        }
    }
}

/// Milliseconds since 1970, the start time a discovery file records.
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn write_discovery(port: u16, token: &str, started: u64) -> Result<PathBuf, String> {
    let directory = discovery_directory();
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    remove_stale_instances(&directory);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
    }
    let path = directory.join(format!("instance-{}.json", std::process::id()));
    let mut temporary =
        tempfile::NamedTempFile::new_in(&directory).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|error| error.to_string())?;
    }
    serde_json::to_writer(
        &mut temporary,
        &json!({
            "pid": std::process::id(),
            "port": port,
            "token": token,
            "api": "native-rust-v1",
            "started": started,
        }),
    )
    .map_err(|error| error.to_string())?;
    temporary
        .persist(&path)
        .map_err(|error| error.to_string())?;
    Ok(path)
}

fn respond(request: Request, status: u16, body: Value) {
    // The charset is named, so that every client reads paths and texts
    // outside ASCII as they are.
    let content_type = Header::from_bytes(b"Content-Type", b"application/json; charset=utf-8")
        .expect("valid JSON content type");
    let response = Response::from_string(body.to_string())
        .with_status_code(status)
        .with_header(content_type);
    let _ = request.respond(response);
}

fn handle_request(
    mut request: Request,
    port: u16,
    started: u64,
    token: &str,
    grants: &Grants,
    sender: &UnboundedSender<Delivery>,
) {
    match (request.method(), request.url()) {
        (&Method::Get, "/health") => respond(request, 200, json!({"status": "ok"})),
        (&Method::Get, "/info") => respond(
            request,
            200,
            json!({
                "pid": std::process::id(),
                "port": port,
                "version": env!("CARGO_PKG_VERSION"),
                "api": "native-rust-v1",
                "started": started,
            }),
        ),
        (&Method::Post, "/eval") => respond(
            request,
            410,
            json!({"error": "JavaScript evaluation is unavailable; use typed /exec commands"}),
        ),
        (&Method::Post, "/exec") => {
            let given = request
                .headers()
                .iter()
                .find(|header| header.field.to_string().eq_ignore_ascii_case("X-OPS-Token"))
                .map(|header| header.value.as_str().to_owned());
            // The token of the discovery file may send every command; that
            // of a run of an extension the commands the extension declared.
            let grant = match given {
                Some(given) if given == token => Some(None),
                Some(given) => grants.get(&given).map(Some),
                None => None,
            };
            let Some(grant) = grant else {
                respond(request, 403, json!({"error": "invalid API token"}));
                return;
            };
            let mut body = Vec::new();
            let read = request
                .as_reader()
                .take(MAX_BODY_BYTES + 1)
                .read_to_end(&mut body);
            if let Err(error) = read {
                respond(request, 400, json!({"error": error.to_string()}));
                return;
            }
            if body.len() as u64 > MAX_BODY_BYTES {
                respond(request, 413, json!({"error": "request body is too large"}));
                return;
            }
            if let Some(grant) = &grant {
                let name = serde_json::from_slice::<Value>(&body)
                    .ok()
                    .and_then(|value| value["command"].as_str().map(str::to_owned))
                    .unwrap_or_default();
                if !grant.allows(&name) {
                    let error = format!(
                        "extension {} does not declare the command {name} in uses.commands of its extension.json",
                        grant.extension
                    );
                    respond(request, 403, json!({ "error": error }));
                    return;
                }
            }
            let command = match serde_json::from_slice::<ApiCommand>(&body) {
                Ok(command) => command,
                Err(error) => {
                    respond(request, 400, json!({"error": error.to_string()}));
                    return;
                }
            };
            let (reply, receiver) = mpsc::channel();
            let delivery = Delivery {
                request: ApiRequest { command, reply },
                caller: grant.map(|grant| grant.extension),
            };
            if sender.send(delivery).is_err() {
                respond(request, 503, json!({"error": "native GUI is unavailable"}));
                return;
            }
            match receiver.recv_timeout(Duration::from_secs(10)) {
                Ok(body) => respond(request, 200, body),
                Err(_) => respond(request, 504, json!({"error": "native GUI did not respond"})),
            }
        }
        _ => respond(request, 404, json!({"error": "unknown endpoint"})),
    }
}

/// Tests that start a server hold this while it runs: a process has one
/// server, and its discovery file is named after the process.
#[cfg(test)]
pub(crate) fn one_server_at_a_time() -> std::sync::MutexGuard<'static, ()> {
    static TURN: Mutex<()> = Mutex::new(());
    TURN.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn start(
    requested_port: Option<u16>,
) -> Result<(UnboundedReceiver<Delivery>, ApiHandle), String> {
    let listener = TcpListener::bind(("127.0.0.1", requested_port.unwrap_or(0)))
        .map_err(|error| error.to_string())?;
    let port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let server = Server::from_listener(listener, None).map_err(|error| error.to_string())?;
    let token = uuid::Uuid::new_v4().to_string();
    let started = unix_millis();
    let discovery_path = write_discovery(port, &token, started)?;
    let (sender, receiver) = unbounded_channel();
    let grants = Grants::default();
    let served = grants.clone();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            handle_request(request, port, started, &token, &served, &sender);
        }
    });
    Ok((
        receiver,
        ApiHandle {
            port,
            grants,
            discovery_path,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_authenticates_and_delivers_a_typed_command() {
        let _turn = one_server_at_a_time();
        let (mut receiver, handle) = start(Some(0)).unwrap();
        let url = format!("http://127.0.0.1:{}", handle.port);
        let client = reqwest::blocking::Client::new();
        assert_eq!(
            serde_json::from_str::<Value>(
                &client
                    .get(format!("{url}/health"))
                    .send()
                    .unwrap()
                    .text()
                    .unwrap()
            )
            .unwrap()["status"],
            "ok"
        );
        assert_eq!(
            client
                .post(format!("{url}/exec"))
                .body(json!({"command": "status"}).to_string())
                .send()
                .unwrap()
                .status(),
            403
        );
        assert_eq!(
            client
                .post(format!("{url}/eval"))
                .body("return 1")
                .send()
                .unwrap()
                .status(),
            410
        );
        let discovery: Value =
            serde_json::from_slice(&fs::read(&handle.discovery_path).unwrap()).unwrap();
        let info: Value = serde_json::from_str(
            &client
                .get(format!("{url}/info"))
                .send()
                .unwrap()
                .text()
                .unwrap(),
        )
        .unwrap();
        assert!(discovery["started"]
            .as_u64()
            .is_some_and(|started| started > 0));
        assert_eq!(info["started"], discovery["started"]);
        assert_eq!(info["pid"], discovery["pid"]);
        let token = discovery["token"].as_str().unwrap().to_owned();
        let send = thread::spawn(move || {
            client
                .post(format!("{url}/exec"))
                .header("X-OPS-Token", token)
                .body(json!({"command": "camera", "preset": "top"}).to_string())
                .send()
                .unwrap()
                .text()
                .map(|body| serde_json::from_str::<Value>(&body).unwrap())
                .unwrap()
        });
        // A machine busy with other tests may take a while to connect.
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let request = loop {
            match receiver.try_recv() {
                Ok(delivery) => {
                    assert_eq!(delivery.caller, None);
                    break delivery.request;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                    if std::time::Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("no API request arrived: {error}"),
            }
        };
        assert!(matches!(request.command, ApiCommand::Camera { preset } if preset == "top"));
        request
            .reply
            .send(json!({"ok": true, "view": "TOP"}))
            .unwrap();
        assert_eq!(send.join().unwrap()["view"], "TOP");
        let discovery_path = handle.discovery_path.clone();
        drop(handle);
        assert!(!discovery_path.exists());
    }

    /// The next request the server hands to the window.
    fn delivered(receiver: &mut UnboundedReceiver<Delivery>) -> Delivery {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        loop {
            match receiver.try_recv() {
                Ok(delivery) => return delivery,
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
                    if std::time::Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("no API request arrived: {error}"),
            }
        }
    }

    #[test]
    fn the_token_of_a_run_sends_what_its_extension_declares() {
        let _turn = one_server_at_a_time();
        let (mut receiver, handle) = start(Some(0)).unwrap();
        let url = format!("http://127.0.0.1:{}/exec", handle.port);
        handle.grants.insert(
            "run-token".into(),
            Grant {
                extension: "org.example.tool".into(),
                commands: Some(BTreeSet::from(["status".to_owned()])),
            },
        );
        let post = move |token: &str, body: Value| {
            reqwest::blocking::Client::new()
                .post(&url)
                .header("X-OPS-Token", token)
                .body(body.to_string())
                .send()
                .unwrap()
        };

        // A command the extension did not declare is refused before the
        // window sees it, and so is a token the server does not know.
        let refused = post("run-token", json!({"command": "export", "path": "/x.las"}));
        assert_eq!(refused.status(), 403);
        assert_eq!(
            refused.headers()["content-type"],
            "application/json; charset=utf-8"
        );
        assert_eq!(
            serde_json::from_str::<Value>(&refused.text().unwrap()).unwrap()["error"],
            "extension org.example.tool does not declare the command export in uses.commands of its extension.json"
        );
        assert_eq!(post("guess", json!({"command": "status"})).status(), 403);
        assert!(receiver.try_recv().is_err());

        // A declared command, and one every run may send, reach the window
        // on behalf of the extension.
        let post = std::sync::Arc::new(post);
        for body in [
            json!({"command": "status"}),
            json!({"command": "show_message", "text": "hi"}),
        ] {
            let sender = {
                let post = std::sync::Arc::clone(&post);
                thread::spawn(move || post("run-token", body).status())
            };
            let delivery = delivered(&mut receiver);
            assert_eq!(delivery.caller.as_deref(), Some("org.example.tool"));
            assert!(matches!(
                delivery.request.command,
                ApiCommand::Status | ApiCommand::ShowMessage { .. }
            ));
            delivery.request.reply.send(json!({"ok": true})).unwrap();
            assert_eq!(sender.join().unwrap(), 200);
        }

        // The token stops working when the run ends.
        handle.grants.remove("run-token");
        assert_eq!(
            post("run-token", json!({"command": "status"})).status(),
            403
        );
        let every = Grant {
            extension: "org.example.all".into(),
            commands: None,
        };
        assert!(every.allows("export"));
        assert!(Grant {
            commands: Some(BTreeSet::new()),
            ..every.clone()
        }
        .allows("job"));
    }
}
