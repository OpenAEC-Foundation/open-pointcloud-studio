//! Opt-in Streamable HTTP transport for the existing desktop MCP tools.
//! No tool or point-cloud operation is implemented a second time here.

use std::collections::HashMap;
use std::io::{self, Read};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use super::{answer, error, instances, lock, tools::Link, Shared, INVALID_PARAMS};

const MAX_BODY: u64 = 1024 * 1024;
const MAX_SESSIONS: usize = 16;
const MAX_CALLS: usize = 8;
const SESSION_IDLE: Duration = Duration::from_secs(30 * 60);

struct Session {
    shared: Shared,
    link: Mutex<Box<dyn Link>>,
    used: Mutex<Instant>,
}

impl Session {
    fn new(link: Box<dyn Link>) -> Self {
        Self {
            shared: Shared {
                writer: Mutex::new(Box::new(io::sink())),
                protocol: Mutex::new(None),
                cancelled: Mutex::new(Default::default()),
                closed: false.into(),
                ended: false.into(),
            },
            link: Mutex::new(link),
            used: Mutex::new(Instant::now()),
        }
    }

    fn dispatch(&self, mut message: Value) -> Option<Value> {
        *lock(&self.used) = Instant::now();
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        if method == "tools/call" {
            let name = message["params"]["name"].as_str().unwrap_or_default();
            let Some(name) = name.strip_prefix("pointcloud_") else {
                return Some(error(
                    message["id"].clone(),
                    INVALID_PARAMS,
                    "HTTP desktop tool names start with pointcloud_",
                ));
            };
            let name = match name {
                "stand" => "status",
                "opslaan" => "export",
                name => name,
            };
            message["params"]["name"] = Value::from(name.to_owned());
            self.shared.forget(&super::id_key(&message["id"]));
            return answer(message, &self.shared, Some(lock(&self.link).as_mut()));
        }
        let mut reply = answer(message, &self.shared, None)?;
        if method == "tools/list" {
            if let Some(tools) = reply["result"]["tools"].as_array_mut() {
                for tool in tools.iter_mut() {
                    tool["name"] = Value::from(format!("pointcloud_{}", tool["name"].as_str()?));
                }
                for (source, alias) in [
                    ("pointcloud_status", "pointcloud_stand"),
                    ("pointcloud_export", "pointcloud_opslaan"),
                ] {
                    if let Some(mut tool) =
                        tools.iter().find(|tool| tool["name"] == source).cloned()
                    {
                        tool["name"] = Value::from(alias);
                        tools.push(tool);
                    }
                }
            }
        }
        Some(reply)
    }
}

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request
        .headers()
        .iter()
        .find(|header| header.field.equiv(name))
        .map(|header| header.value.as_str())
}

fn accepted(origin: bool, authorization: Option<&str>, secret: &str) -> bool {
    if origin {
        return false;
    }
    let Some(given) = authorization.and_then(|value| value.strip_prefix("Bearer ")) else {
        return false;
    };
    // Compare all bytes of a same-length key without an early mismatch exit.
    given.len() == secret.len()
        && given
            .bytes()
            .zip(secret.bytes())
            .fold(0, |difference, (a, b)| difference | (a ^ b))
            == 0
}

fn respond(request: Request, status: u16, body: Value, session: Option<&str>) {
    let mut response = Response::from_string(if status == 202 {
        String::new()
    } else {
        body.to_string()
    })
    .with_status_code(status)
    .with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
    if let Some(id) = session {
        response.add_header(Header::from_bytes("Mcp-Session-Id", id).unwrap());
    }
    let _ = request.respond(response);
}

struct State {
    secret: String,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    calls: AtomicUsize,
    client: reqwest::blocking::Client,
}

fn handle(mut request: Request, state: &Arc<State>) {
    if !accepted(
        header(&request, "Origin").is_some(),
        header(&request, "Authorization"),
        &state.secret,
    ) {
        let status = if header(&request, "Origin").is_some() {
            403
        } else {
            401
        };
        respond(request, status, json!({"error": "access denied"}), None);
        return;
    }
    if request.url() == "/gezond" && request.method() == &Method::Get {
        let running = instances::discover(&crate::native_api::discovery_directory(), &state.client)
            .iter()
            .any(|instance| {
                state
                    .client
                    .get(format!("http://127.0.0.1:{}/gui-health", instance.port))
                    .timeout(std::time::Duration::from_secs(3))
                    .send()
                    .is_ok_and(|reply| reply.status().is_success())
            });
        respond(
            request,
            if running { 200 } else { 503 },
            json!({"ok": running}),
            None,
        );
        return;
    }
    if request.url() != "/mcp" {
        respond(request, 404, json!({"error": "unknown route"}), None);
        return;
    }
    if header(&request, "MCP-Protocol-Version")
        .is_some_and(|version| !super::PROTOCOL_VERSIONS.contains(&version))
    {
        respond(
            request,
            400,
            json!({"error": "unsupported MCP protocol version"}),
            None,
        );
        return;
    }
    let id = header(&request, "Mcp-Session-Id").map(str::to_owned);
    if request.method() == &Method::Delete {
        let removed = id.as_ref().and_then(|id| lock(&state.sessions).remove(id));
        if let Some(session) = removed {
            session.shared.ended.store(true, Ordering::Relaxed);
            respond(request, 200, json!({}), None);
        } else {
            respond(request, 404, json!({"error": "unknown session"}), None);
        }
        return;
    }
    if request.method() != &Method::Post {
        // This server replies with JSON and does not offer an SSE GET stream.
        respond(request, 405, json!({"error": "use POST"}), None);
        return;
    }
    if !header(&request, "Content-Type").is_some_and(|value| {
        value.split(';').next().unwrap_or_default().trim() == "application/json"
    }) {
        respond(request, 415, json!({"error": "use application/json"}), None);
        return;
    }
    let mut bytes = Vec::new();
    if request
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        respond(request, 400, json!({"error": "cannot read request"}), None);
        return;
    }
    if bytes.len() as u64 > MAX_BODY {
        respond(request, 413, json!({"error": "request too large"}), None);
        return;
    }
    let message: Value = match serde_json::from_slice(&bytes) {
        Ok(message) => message,
        Err(_) => {
            respond(request, 400, json!({"error": "invalid JSON"}), None);
            return;
        }
    };
    if !message.is_object() {
        respond(
            request,
            400,
            json!({"error": "one JSON-RPC message is required"}),
            None,
        );
        return;
    }
    if message["method"] == "initialize" {
        let mut sessions = lock(&state.sessions);
        sessions.retain(|_, session| {
            let keep = lock(&session.used).elapsed() < SESSION_IDLE;
            if !keep {
                session.shared.ended.store(true, Ordering::Relaxed);
            }
            keep
        });
        if sessions.len() >= MAX_SESSIONS {
            respond(
                request,
                503,
                json!({"error": "session limit reached"}),
                None,
            );
            return;
        }
        let connector = instances::Connector::new(
            crate::native_api::discovery_directory(),
            match std::env::current_exe() {
                Ok(path) => path,
                Err(_) => {
                    respond(
                        request,
                        500,
                        json!({"error":"cannot locate application"}),
                        None,
                    );
                    return;
                }
            },
            false,
        );
        let connector = match connector {
            Ok(link) => link,
            Err(_) => {
                respond(
                    request,
                    500,
                    json!({"error":"cannot create desktop connection"}),
                    None,
                );
                return;
            }
        };
        let session = Arc::new(Session::new(Box::new(connector)));
        let id = uuid::Uuid::new_v4().to_string();
        let mut reply = session.dispatch(message).unwrap_or(Value::Null);
        if reply.get("result").is_some() {
            reply["result"]["instructions"] = Value::from(
                "These are the desktop application's existing tools with pointcloud_ prefixes. Start the native application in the same configuration directory before calling tools. pointcloud_stand aliases status and pointcloud_opslaan aliases export; file paths refer to the container's mounted filesystem. Use pointcloud_wait_until_idle and pointcloud_wait_for_job for background work."
            );
        }
        if reply.get("error").is_none() {
            sessions.insert(id.clone(), session);
        }
        respond(request, 200, reply, Some(&id));
        return;
    }
    let Some(id) = id else {
        respond(
            request,
            400,
            json!({"error": "Mcp-Session-Id is required"}),
            None,
        );
        return;
    };
    let Some(session) = lock(&state.sessions).get(&id).cloned() else {
        respond(request, 404, json!({"error": "unknown session"}), None);
        return;
    };
    if message["method"] != "tools/call" {
        let reply = session.dispatch(message);
        respond(
            request,
            if reply.is_some() { 200 } else { 202 },
            reply.unwrap_or(Value::Null),
            Some(&id),
        );
        return;
    }
    if state.calls.fetch_add(1, Ordering::Relaxed) >= MAX_CALLS {
        state.calls.fetch_sub(1, Ordering::Relaxed);
        respond(
            request,
            503,
            json!({"error": "tool call limit reached"}),
            Some(&id),
        );
        return;
    }
    let state = Arc::clone(state);
    thread::spawn(move || {
        let reply = session.dispatch(message);
        respond(request, 200, reply.unwrap_or(Value::Null), Some(&id));
        state.calls.fetch_sub(1, Ordering::Relaxed);
    });
}

pub(crate) fn run() -> i32 {
    let configured = || -> Result<_, String> {
        let port: u16 = std::env::var("OPENAEC_MCP_POORT")
            .map_err(|_| "OPENAEC_MCP_POORT is required")?
            .parse()
            .map_err(|_| "OPENAEC_MCP_POORT must be a port number")?;
        if port == 0 {
            return Err("OPENAEC_MCP_POORT must not be zero".into());
        }
        let address = std::env::var("OPENAEC_MCP_ADRES")
            .ok()
            .map(|value| value.parse::<IpAddr>())
            .transpose()
            .map_err(|_| "OPENAEC_MCP_ADRES must be an IP address")?
            .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
        let secret =
            std::env::var("OPENAEC_MCP_GEHEIM").map_err(|_| "OPENAEC_MCP_GEHEIM is required")?;
        if secret.len() < 32 || !secret.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(
                "OPENAEC_MCP_GEHEIM must contain at least 32 printable non-space ASCII characters"
                    .into(),
            );
        }
        let client = instances::http_client()?;
        let server = Server::http((address, port)).map_err(|error| error.to_string())?;
        Ok((
            server,
            State {
                secret,
                sessions: Mutex::new(HashMap::new()),
                calls: AtomicUsize::new(0),
                client,
            },
        ))
    };
    match configured() {
        Ok((server, state)) => {
            eprintln!(
                "Desktop MCP HTTP listening on {} (authenticated; no browser Origin allowed)",
                server.server_addr()
            );
            let state = Arc::new(state);
            for request in server.incoming_requests() {
                handle(request, &state);
            }
            0
        }
        Err(error) => {
            eprintln!("Desktop MCP HTTP: {error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recording;
    impl Link for Recording {
        fn exec(&mut self, command: &Value) -> Result<Value, String> {
            Ok(json!({"ok":true,"command":command}))
        }
        fn list_instances(&mut self) -> Value {
            json!([])
        }
        fn select_instance(&mut self, _: Option<u32>, _: Option<u16>) -> Result<Value, String> {
            Ok(json!({}))
        }
        fn start_instance(&mut self, _: &[std::path::PathBuf]) -> Result<Value, String> {
            Err("not used".into())
        }
    }

    #[test]
    fn bearer_key_is_required_and_browser_origins_are_refused() {
        assert!(accepted(false, Some("Bearer secret"), "secret"));
        assert!(!accepted(true, Some("Bearer secret"), "secret"));
        assert!(!accepted(false, None, "secret"));
        assert!(!accepted(false, Some("Bearer secreu"), "secret"));
        assert!(!accepted(false, Some("Bearer secret-extra"), "secret"));
    }

    #[test]
    fn http_lists_the_stdio_tools_with_prefixes_and_routes_aliases_to_the_same_code() {
        let session = Session::new(Box::new(Recording));
        session.dispatch(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}));
        let reply = session
            .dispatch(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .unwrap();
        let listed = reply["result"]["tools"].as_array().unwrap();
        assert_eq!(listed.len(), super::super::tools::tools().len() + 2);
        for tool in super::super::tools::tools() {
            assert!(listed
                .iter()
                .any(|entry| entry["name"] == format!("pointcloud_{}", tool.name)
                    && entry["inputSchema"] == tool.schema));
        }
        let reply = session.dispatch(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"pointcloud_stand","arguments":{}}})).unwrap();
        assert_eq!(
            reply["result"]["structuredContent"]["command"]["command"],
            "status"
        );
        assert!(session
            .dispatch(
                json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"status"}})
            )
            .unwrap()
            .get("error")
            .is_some());
    }
}
