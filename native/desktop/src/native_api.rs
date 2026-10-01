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
    Job { id: String },
    Open { path: PathBuf },
    Remove { index: usize },
    SetActive { index: usize },
    SetVisible { index: usize, visible: bool },
    Camera { preset: String },
    SetColor { mode: String },
    SetClassVisible { code: u8, visible: bool },
    SetPointSize { size: f32 },
    SetEyeDome { enabled: bool },
    SetEyeDomeStrength { strength: f32 },
    SetBudget { points: u32 },
    SetSection { min: [f64; 3], max: [f64; 3] },
    ClearSection,
    SelectWorld { min: [f64; 3], max: [f64; 3] },
    ClearSelection,
    DeleteSelection,
    UndoDelete,
    RedoDelete,
    Translate { offset: [f64; 3] },
    Scale { factors: [f64; 3] },
    CancelScale,
    BuildIndex,
    CancelIndex,
    SetAutoIndex { enabled: bool },
    ResetTransform,
    Mesh { mode: String, path: PathBuf },
    CancelMesh,
    Export { path: PathBuf },
    ExportSection { path: PathBuf },
    ExportSelection { path: PathBuf },
    ExportMinusSelection { path: PathBuf },
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

fn discovery_directory() -> PathBuf {
    if let Some(root) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(root).join("open-pointcloud-studio-native/instances");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config/open-pointcloud-studio-native/instances");
    }
    std::env::temp_dir().join("open-pointcloud-studio-native/instances")
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

fn write_discovery(port: u16, token: &str) -> Result<PathBuf, String> {
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
        &json!({"pid": std::process::id(), "port": port, "token": token, "api": "native-rust-v1"}),
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
                "api": "native-rust-v1"
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
    let discovery_path = write_discovery(port, &token)?;
    let (sender, receiver) = unbounded_channel();
    thread::spawn(move || {
        for request in server.incoming_requests() {
            handle_request(request, port, &token, &sender);
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
