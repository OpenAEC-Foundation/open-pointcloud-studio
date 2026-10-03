//! A stand-in for a window's command API in tests: `/info` reports a
//! process ID, and `/exec` records each body with its token and answers
//! through a function of the command.

use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{json, Value};
use tiny_http::{Header, Response, Server};

/// Each `/exec` body as it arrived, with its `X-OPS-Token` header.
pub type Recorded = Arc<Mutex<Vec<(String, Option<String>)>>>;

pub struct MockApi {
    pub port: u16,
    pub bodies: Recorded,
}

impl MockApi {
    pub fn bodies(&self) -> Vec<String> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .map(|(body, _)| body.clone())
            .collect()
    }
}

/// A command API that answers every command with `{"ok": true}`.
pub fn mock_api(pid: u32) -> MockApi {
    answering(pid, |_| json!({"ok": true}))
}

pub fn answering(pid: u32, answer: impl Fn(&Value) -> Value + Send + 'static) -> MockApi {
    let server = Server::http("127.0.0.1:0").unwrap();
    let port = server.server_addr().to_ip().unwrap().port();
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&bodies);
    thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let content_type = Header::from_bytes(b"Content-Type", b"application/json").unwrap();
            let reply = if request.url() == "/info" {
                json!({"pid": pid, "port": port, "version": "9.9.9", "api": "native-rust-v1"})
            } else {
                let mut body = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
                let token = request
                    .headers()
                    .iter()
                    .find(|header| header.field.equiv("X-OPS-Token"))
                    .map(|header| header.value.to_string());
                let command = serde_json::from_str(&body).unwrap_or(Value::Null);
                recorded.lock().unwrap().push((body, token));
                answer(&command)
            };
            let _ =
                request.respond(Response::from_string(reply.to_string()).with_header(content_type));
        }
    });
    MockApi { port, bodies }
}
