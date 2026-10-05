//! Local JSON command bridge for the native GUI. No script evaluation or webview.

use std::fs;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
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
    },
    ClearSection,
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
    SetSurfaceSettings {
        max_vertices: usize,
        neighbors: usize,
        edge_factor: f64,
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
    FileView {
        open: bool,
        /// The page to show, with `open: true`; without it the File view
        /// opens on its first page or keeps the page it shows.
        #[serde(default)]
        page: Option<String>,
    },
    Screenshot {
        /// Absolute `.png` path to write the image to.
        path: Option<PathBuf>,
        /// Whether the answer carries the PNG as base64; without a path it does.
        base64: Option<bool>,
        /// Longest edge of the image in pixels.
        max_edge: Option<u32>,
    },
}

#[derive(Clone, Debug)]
pub struct ApiRequest {
    pub command: ApiCommand,
    pub reply: Sender<Value>,
}

pub struct ApiHandle {
    pub port: u16,
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
    let content_type =
        Header::from_bytes(b"Content-Type", b"application/json").expect("valid JSON content type");
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
    sender: &UnboundedSender<ApiRequest>,
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
            let authorized = request.headers().iter().any(|header| {
                header.field.to_string().eq_ignore_ascii_case("X-OPS-Token")
                    && header.value.as_str() == token
            });
            if !authorized {
                respond(request, 403, json!({"error": "invalid API token"}));
                return;
            }
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
            let command = match serde_json::from_slice::<ApiCommand>(&body) {
                Ok(command) => command,
                Err(error) => {
                    respond(request, 400, json!({"error": error.to_string()}));
                    return;
                }
            };
            let (reply, receiver) = mpsc::channel();
            if sender.send(ApiRequest { command, reply }).is_err() {
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

pub fn start(
    requested_port: Option<u16>,
) -> Result<(UnboundedReceiver<ApiRequest>, ApiHandle), String> {
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
    thread::spawn(move || {
        for request in server.incoming_requests() {
            handle_request(request, port, started, &token, &sender);
        }
    });
    Ok((
        receiver,
        ApiHandle {
            port,
            discovery_path,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_authenticates_and_delivers_a_typed_command() {
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
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let request = loop {
            match receiver.try_recv() {
                Ok(request) => break request,
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
}
