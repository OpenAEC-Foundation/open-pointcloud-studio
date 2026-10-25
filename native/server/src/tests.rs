//! The service through its routes, as a client sees it: access and origin,
//! answers that are always JSON, the limits, the policy, the tools over REST
//! and MCP, the OpenAPI document, and `docs/cloud-api.md` held to the code.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use pointcloud_core::ops::{self, CachedIndex, Convert, SectionDrawingOptions};
use pointcloud_core::{
    Bounds, DrawingView, ExportFormat, LoadError, OrientedBox, SurfaceMeshConfig,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use crate::config::Config;
use crate::error::ApiError;
use crate::http::{router_with, AppState, Seam};
use crate::payload::{decode_base64, encode_base64};
use crate::tools::{self, CallLimits, ToolOutput};

const KEY: &str = "a-test-key-of-the-service";

/// A folder for the work folders of one test, and the settings over it.
struct Service {
    work: tempfile::TempDir,
}

impl Service {
    fn new() -> Self {
        Self {
            work: tempfile::tempdir().unwrap(),
        }
    }

    fn config(&self, pairs: &[(&str, &str)]) -> Config {
        let mut map: BTreeMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        map.insert(
            "OPENAEC_API_WERKMAP".into(),
            self.work.path().display().to_string(),
        );
        if !map.contains_key("OPENAEC_API_SLEUTELS") {
            map.entry("OPENAEC_API_TOEGANG".into())
                .or_insert_with(|| "open".into());
        }
        Config::from_source(&|name| map.get(name).cloned()).unwrap()
    }

    fn open(&self) -> Arc<AppState> {
        Arc::new(AppState::new(self.config(&[])))
    }

    fn with_key(&self) -> Arc<AppState> {
        let hash = Sha256::digest(KEY.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Arc::new(AppState::new(
            self.config(&[("OPENAEC_API_SLEUTELS", &hash)]),
        ))
    }

    /// Whether the folder of the work folders holds nothing.
    fn clean(&self) -> bool {
        fs::read_dir(self.work.path()).unwrap().next().is_none()
    }
}

/// The answer of one request: status, headers and the JSON of the body
/// (`null` without one).
async fn send(router: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| panic!("not JSON ({status}): {}", String::from_utf8_lossy(&bytes)))
    };
    (status, headers, value)
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

fn post(uri: &str, body: &Value) -> Request<Body> {
    let text = body.to_string();
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, text.len())
        .body(Body::from(text))
        .unwrap()
}

fn with_key(mut request: Request<Body>, key: &str) -> Request<Body> {
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {key}").parse().unwrap(),
    );
    request
}

fn with_origin(mut request: Request<Body>, origin: &str) -> Request<Body> {
    request
        .headers_mut()
        .insert(header::ORIGIN, origin.parse().unwrap());
    request
}

fn rpc(id: Value, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn assert_triple(status: StatusCode, headers: &HeaderMap, body: &Value, code: &str) {
    assert_eq!(
        headers.get(header::CONTENT_TYPE).unwrap(),
        "application/json",
        "{status} {body}"
    );
    let object = body.as_object().unwrap_or_else(|| panic!("{body}"));
    for key in ["error_code", "melding", "remedie"] {
        assert!(object.contains_key(key), "{key} in {body}");
    }
    assert_eq!(body["error_code"], code, "{status} {body}");
}

// --- Inputs --------------------------------------------------------------

/// A 10 by 10 by 10 grid of coloured points 0.25 apart, as ASCII PLY.
fn grid() -> Vec<u8> {
    let mut text = String::from("ply\nformat ascii 1.0\nelement vertex 1000\n");
    text.push_str("property float x\nproperty float y\nproperty float z\n");
    text.push_str("property uchar red\nproperty uchar green\nproperty uchar blue\n");
    text.push_str("end_header\n");
    for x in 0..10 {
        for y in 0..10 {
            for z in 0..10 {
                let at = |step: i32| f64::from(step) * 0.25;
                text.push_str(&format!(
                    "{:.2} {:.2} {:.2} 255 0 255\n",
                    at(x),
                    at(y),
                    at(z)
                ));
            }
        }
    }
    text.into_bytes()
}

/// The walls of a room of 4 by 3 m and 2.5 m high, a point every 0.1 m.
fn room() -> Vec<u8> {
    let mut text = String::new();
    for level in 0..=25 {
        let z = f64::from(level) * 0.1;
        for step in 0..=40 {
            let x = f64::from(step) * 0.1;
            text.push_str(&format!("{x:.3} 0.000 {z:.3}\n{x:.3} 3.000 {z:.3}\n"));
        }
        for step in 1..30 {
            let y = f64::from(step) * 0.1;
            text.push_str(&format!("0.000 {y:.3} {z:.3}\n4.000 {y:.3} {z:.3}\n"));
        }
    }
    text.into_bytes()
}

/// `count` points of a sloping field, as XYZ.
fn field(count: u32) -> Vec<u8> {
    let mut text = String::new();
    for at in 0..count {
        let (x, y) = (f64::from(at % 300) * 0.1, f64::from(at / 300) * 0.1);
        text.push_str(&format!("{x:.3} {y:.3} {:.3}\n", 0.01 * x + 0.02 * y));
    }
    text.into_bytes()
}

/// A binary_compressed PCD of `count` points, compressed as literal runs.
fn compressed_pcd(count: u32) -> Vec<u8> {
    let mut planes = Vec::new();
    for axis in 0..3u32 {
        for at in 0..count {
            planes.extend((at as f32 * 0.1 + axis as f32).to_le_bytes());
        }
    }
    let mut compressed = Vec::new();
    for run in planes.chunks(32) {
        compressed.push((run.len() - 1) as u8);
        compressed.extend(run);
    }
    let mut bytes = format!(
        "VERSION 0.7\nFIELDS x y z\nSIZE 4 4 4\nTYPE F F F\nCOUNT 1 1 1\nWIDTH {count}\nHEIGHT 1\nVIEWPOINT 0 0 0 1 0 0 0\nPOINTS {count}\nDATA binary_compressed\n"
    )
    .into_bytes();
    bytes.extend((compressed.len() as u32).to_le_bytes());
    bytes.extend((planes.len() as u32).to_le_bytes());
    bytes.extend(compressed);
    bytes
}

fn input(format: &str, bytes: &[u8]) -> Value {
    json!({"format": format, "data": encode_base64(bytes)})
}

fn room_plan() -> Value {
    json!({"min": [-0.5, -0.5, 0.0], "max": [4.5, 3.5, 1.2]})
}

// --- Access and origin ---------------------------------------------------

#[tokio::test]
async fn keys_guard_the_tools_and_not_health_or_the_description() {
    let service = Service::new();
    let router = router_with(service.with_key());
    for uri in ["/v1/health", "/v1/openapi.json"] {
        let (status, _, _) = send(&router, get(uri)).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
    let (status, headers, body) = send(&router, get("/v1/tools")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(headers.get(header::WWW_AUTHENTICATE).unwrap(), "Bearer");
    assert_triple(status, &headers, &body, "TOEGANG_GEWEIGERD");
    let (status, _, _) = send(&router, with_key(get("/v1/tools"), "wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, body) = send(&router, with_key(get("/v1/tools"), KEY)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["tools"].as_array().unwrap().len(), 5);
    let (status, headers, body) = send(
        &router,
        post("/mcp", &rpc(json!(1), "tools/list", json!({}))),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_triple(status, &headers, &body, "TOEGANG_GEWEIGERD");
    let (status, _, body) = send(
        &router,
        with_key(post("/mcp", &rpc(json!(1), "tools/list", json!({}))), KEY),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["tools"].as_array().unwrap().len(), 5);
    let request = post(
        "/v1/tools/inspect",
        &json!({"input": input("ply", &grid())}),
    );
    let (status, _, _) = send(&router, request).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_foreign_origin_is_refused_on_the_tools_only() {
    let service = Service::new();
    let config = service.config(&[("OPENAEC_API_HERKOMSTEN", "https://app.example.org")]);
    let router = router_with(Arc::new(AppState::new(config)));
    let list = || post("/mcp", &rpc(json!(1), "tools/list", json!({})));
    for request in [get("/v1/tools"), list()] {
        let (status, headers, body) =
            send(&router, with_origin(request, "https://example.org")).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_triple(status, &headers, &body, "HERKOMST_GEWEIGERD");
    }
    for request in [get("/v1/tools"), list()] {
        let (status, _, _) = send(&router, with_origin(request, "https://APP.example.org")).await;
        assert_eq!(status, StatusCode::OK, "a listed origin");
    }
    for request in [get("/v1/tools"), list()] {
        let (status, _, _) = send(&router, request).await;
        assert_eq!(status, StatusCode::OK, "no origin");
    }
    for uri in ["/v1/health", "/v1/openapi.json"] {
        let (status, _, _) = send(&router, with_origin(get(uri), "https://example.org")).await;
        assert_eq!(status, StatusCode::OK, "{uri}");
    }
}

// --- Every answer that is not a success is the triple --------------------

#[tokio::test]
async fn every_failure_is_json_with_its_code() {
    let service = Service::new();
    let router = router_with(service.open());
    let request = |method: Method, uri: &str| {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    };
    let cases: Vec<(Request<Body>, StatusCode, &str, Option<&str>)> = vec![
        (get("/v2/x"), StatusCode::NOT_FOUND, "ROUTE_ONBEKEND", None),
        (get("/"), StatusCode::NOT_FOUND, "ROUTE_ONBEKEND", None),
        (
            get("/v1/tools/convert/more"),
            StatusCode::NOT_FOUND,
            "ROUTE_ONBEKEND",
            None,
        ),
        (
            get("/mcp"),
            StatusCode::METHOD_NOT_ALLOWED,
            "METHODE_NIET_TOEGESTAAN",
            Some("POST"),
        ),
        (
            request(Method::POST, "/v1/health"),
            StatusCode::METHOD_NOT_ALLOWED,
            "METHODE_NIET_TOEGESTAAN",
            Some("GET"),
        ),
        (
            get("/v1/tools/convert"),
            StatusCode::METHOD_NOT_ALLOWED,
            "METHODE_NIET_TOEGESTAAN",
            Some("POST"),
        ),
        (
            request(Method::DELETE, "/v1/tools"),
            StatusCode::METHOD_NOT_ALLOWED,
            "METHODE_NIET_TOEGESTAAN",
            Some("GET"),
        ),
        (
            request(Method::PUT, "/v1/openapi.json"),
            StatusCode::METHOD_NOT_ALLOWED,
            "METHODE_NIET_TOEGESTAAN",
            Some("GET"),
        ),
        (
            post("/v1/tools/unknown", &json!({})),
            StatusCode::NOT_FOUND,
            "TOOL_ONBEKEND",
            None,
        ),
        (
            post("/v1/tools/convert", &json!([1, 2])),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
            None,
        ),
        (
            post("/v1/tools/convert", &json!({"input": 1})),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
            None,
        ),
    ];
    for (request, expected, code, allow) in cases {
        let what = format!("{} {}", request.method(), request.uri());
        let (status, headers, body) = send(&router, request).await;
        assert_eq!(status, expected, "{what}: {body}");
        assert_triple(status, &headers, &body, code);
        if let Some(allow) = allow {
            let header = headers
                .get(header::ALLOW)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            assert!(header.contains(allow), "{what}: Allow {header}");
        }
    }
    let broken = Request::post("/v1/tools/convert")
        .body(Body::from("{\"input\": "))
        .unwrap();
    let (status, headers, body) = send(&router, broken).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_triple(status, &headers, &body, "ARGUMENT_ONGELDIG");
}

fn seam(
    run: impl Fn(
            &str,
            &serde_json::value::RawValue,
            &CallLimits,
            &tempfile::TempDir,
            ops::KeepGoing<'_>,
        ) -> Result<ToolOutput, ApiError>
        + Send
        + Sync
        + 'static,
) -> Seam {
    Arc::new(run)
}

#[tokio::test]
async fn a_panicking_tool_is_answered_with_json() {
    let service = Service::new();
    let state = AppState::with_seam(
        service.config(&[]),
        seam(|_, _, _, _, _| panic!("a test tool panics")),
    );
    let router = router_with(Arc::new(state));
    let request = post(
        "/v1/tools/inspect",
        &json!({"input": input("ply", &grid())}),
    );
    let (status, headers, body) = send(&router, request).await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_triple(status, &headers, &body, "INTERNE_FOUT");
    assert!(service.clean(), "the work folder went with the panic");
}

// --- Limits --------------------------------------------------------------

#[tokio::test]
async fn a_body_over_the_limit_is_refused_with_json() {
    let service = Service::new();
    let router = router_with(Arc::new(AppState::new(
        service.config(&[("OPENAEC_API_MAX_LICHAAM_MIB", "1")]),
    )));
    let body = vec![b' '; 1024 * 1024 + 1];
    let request = Request::post("/v1/tools/inspect")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap();
    let (status, headers, answer) = send(&router, request).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_triple(status, &headers, &answer, "LICHAAM_TE_GROOT");
    // Without a stated length it is refused while it is read.
    let streamed = Request::post("/mcp")
        .body(Body::from(vec![b' '; 1024 * 1024 + 1]))
        .unwrap();
    let (status, headers, answer) = send(&router, streamed).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_triple(status, &headers, &answer, "LICHAAM_TE_GROOT");
    // One byte less is read, and refused for what it holds.
    let body = vec![b' '; 1024 * 1024];
    let request = Request::post("/v1/tools/inspect")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .unwrap();
    let (status, _, _) = send(&router, request).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_full_waiting_room_answers_large_calls_at_once_and_lists_still() {
    let service = Service::new();
    let state = Arc::new(AppState::new(service.config(&[
        ("OPENAEC_API_GELIJKTIJDIG", "1"),
        ("OPENAEC_API_WACHTRIJ", "0"),
    ])));
    let router = router_with(Arc::clone(&state));
    let _room = Arc::clone(&state.limits.waiting_room)
        .try_acquire_owned()
        .unwrap();
    let large = json!({"input": input("xyz", &field(5000))});
    let started = Instant::now();
    let (status, headers, body) = send(&router, post("/v1/tools/inspect", &large)).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers.get(header::RETRY_AFTER).unwrap(), "10");
    assert_triple(status, &headers, &body, "DRUK");
    assert!(started.elapsed() < Duration::from_secs(5), "at once");
    let (status, _, _) = send(
        &router,
        post("/mcp", &rpc(json!(1), "tools/list", json!({}))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(
        &router,
        post("/mcp", &rpc(json!(2), "initialize", json!({}))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(&router, get("/v1/tools")).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn busy_places_answer_after_the_wait() {
    let service = Service::new();
    let mut config = service.config(&[("OPENAEC_API_GELIJKTIJDIG", "1")]);
    config.wait = Duration::from_millis(200);
    let state = Arc::new(AppState::new(config));
    let router = router_with(Arc::clone(&state));
    let _place = Arc::clone(&state.limits.running)
        .acquire_owned()
        .await
        .unwrap();
    let started = Instant::now();
    let small = json!({"input": input("xyz", &field(10))});
    let (status, headers, body) = send(&router, post("/v1/tools/inspect", &small)).await;
    assert!(started.elapsed() >= Duration::from_millis(200));
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(headers.get(header::RETRY_AFTER).unwrap(), "10");
    assert_triple(status, &headers, &body, "DRUK");
    assert!(service.clean());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_time_limit_cancels_the_work_which_keeps_its_place_until_it_stops() {
    let service = Service::new();
    let mut config = service.config(&[("OPENAEC_API_GELIJKTIJDIG", "1")]);
    config.time_limit = Duration::from_millis(100);
    let state = Arc::new(AppState::with_seam(
        config,
        seam(|_, _, _, _, keep_going| loop {
            keep_going().map_err(|_| ApiError::internal("cancelled"))?;
            std::thread::sleep(Duration::from_millis(150));
        }),
    ));
    let router = router_with(Arc::clone(&state));
    // A body above 64 KiB, which takes a place in the waiting room too.
    let large = json!({"input": input("xyz", &field(5000))});
    let room = state.limits.waiting_room.available_permits();
    let (status, headers, body) = send(&router, post("/v1/tools/inspect", &large)).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert_triple(status, &headers, &body, "TIJDSLIMIET");
    assert_eq!(
        state.limits.running.available_permits(),
        0,
        "the work still holds its place to compute"
    );
    assert_eq!(
        state.limits.waiting_room.available_permits(),
        room - 1,
        "and its place in the waiting room"
    );
    // It sees the flag at its next step and gives both places back.
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.limits.running.available_permits() == 0
        || state.limits.waiting_room.available_permits() < room
    {
        assert!(Instant::now() < deadline, "the places came back");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!state.limits.stuck());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !service.clean() {
        assert!(Instant::now() < deadline, "the work folder is removed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_that_ignores_the_cancel_turns_health_to_stuck_until_it_ends() {
    let service = Service::new();
    let mut config = service.config(&[]);
    config.time_limit = Duration::from_millis(50);
    config.grace = Duration::from_millis(100);
    let release = Arc::new(AtomicBool::new(false));
    let held = Arc::clone(&release);
    let state = Arc::new(AppState::with_seam(
        config,
        seam(move |_, _, _, _, _| {
            while !held.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(ApiError::internal("stopped"))
        }),
    ));
    let router = router_with(Arc::clone(&state));
    let small = json!({"input": input("xyz", &field(10))});
    let (status, _, _) = send(&router, post("/v1/tools/inspect", &small)).await;
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    let (status, _, _) = send(&router, get("/v1/health")).await;
    assert_eq!(status, StatusCode::OK, "within the grace period");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (status, headers, body) = send(&router, get("/v1/health")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_triple(status, &headers, &body, "WERK_VASTGELOPEN");
    assert_eq!(body["status"], "vastgelopen");
    assert_eq!(body["versie"], crate::VERSION);
    release.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (status, _, body) = send(&router, get("/v1/health")).await;
        if status == StatusCode::OK {
            assert_eq!(body["status"], "ok");
            break;
        }
        assert!(Instant::now() < deadline, "health comes back");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_goes_away_cancels_the_work() {
    let service = Service::new();
    let mut config = service.config(&[("OPENAEC_API_GELIJKTIJDIG", "1")]);
    config.time_limit = Duration::from_secs(60);
    let cancelled = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&cancelled);
    let state = Arc::new(AppState::with_seam(
        config,
        seam(move |_, _, _, _, keep_going| {
            // Bounded, so that a failing test ends rather than waits for
            // this thread.
            let give_up = Instant::now() + Duration::from_secs(10);
            while Instant::now() < give_up {
                if keep_going().is_err() {
                    seen.store(true, Ordering::Release);
                    return Err(ApiError::internal("cancelled"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(ApiError::internal("never cancelled"))
        }),
    ));
    let router = router_with(Arc::clone(&state));
    let large = json!({"input": input("xyz", &field(5000))});
    let room = state.limits.waiting_room.available_permits();
    // The client stops waiting long before the time limit, which drops the
    // request.
    let request = send(&router, post("/v1/tools/inspect", &large));
    assert!(
        tokio::time::timeout(Duration::from_millis(300), request)
            .await
            .is_err(),
        "no answer yet"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !cancelled.load(Ordering::Acquire)
        || state.limits.running.available_permits() == 0
        || state.limits.waiting_room.available_permits() < room
    {
        assert!(
            Instant::now() < deadline,
            "the work saw the cancel and gave its places back"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!state.limits.stuck());
    let deadline = Instant::now() + Duration::from_secs(5);
    while !service.clean() {
        assert!(Instant::now() < deadline, "the work folder is removed");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn work_of_a_client_that_went_away_is_stuck_only_after_its_time_limit() {
    let service = Service::new();
    let mut config = service.config(&[]);
    config.time_limit = Duration::from_millis(600);
    config.grace = Duration::from_millis(100);
    let release = Arc::new(AtomicBool::new(false));
    let held = Arc::clone(&release);
    let state = Arc::new(AppState::with_seam(
        config,
        seam(move |_, _, _, _, _| {
            // Bounded, so that a failing test ends rather than waits for
            // this thread.
            let give_up = Instant::now() + Duration::from_secs(10);
            while !held.load(Ordering::Acquire) && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(ApiError::internal("stopped"))
        }),
    ));
    let router = router_with(Arc::clone(&state));
    let started = Instant::now();
    let small = json!({"input": input("xyz", &field(10))});
    let request = send(&router, post("/v1/tools/inspect", &small));
    assert!(tokio::time::timeout(Duration::from_millis(50), request)
        .await
        .is_err());
    // Cancelled, and still running, but within its own time limit.
    tokio::time::sleep(Duration::from_millis(150)).await;
    if started.elapsed() < Duration::from_millis(650) {
        let (status, _, _) = send(&router, get("/v1/health")).await;
        assert_eq!(status, StatusCode::OK, "within the time limit of the work");
    }
    // A grace period after that, it is stuck.
    tokio::time::sleep(Duration::from_millis(900).saturating_sub(started.elapsed())).await;
    let (status, headers, body) = send(&router, get("/v1/health")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_triple(status, &headers, &body, "WERK_VASTGELOPEN");
    release.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(5);
    while send(&router, get("/v1/health")).await.0 != StatusCode::OK {
        assert!(Instant::now() < deadline, "health comes back");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// --- Policy --------------------------------------------------------------

#[tokio::test]
async fn the_policy_refuses_what_the_cloud_does_not_allow() {
    let service = Service::new();
    let router = router_with(service.open());
    let ply = input("ply", &grid());
    let cases = [
        (
            "unknown",
            json!({"input": ply}),
            StatusCode::NOT_FOUND,
            "TOOL_ONBEKEND",
        ),
        (
            "inspect",
            json!({"input": ply, "path": "/etc/passwd"}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "convert",
            json!({"input": ply, "output_format": "laz", "destination": "/tmp/x.laz"}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!({"input": ply, "colour": true}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!({"input": {"format": "ply", "data": "QUJD", "path": "x"}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!({"input": {"format": "ply", "data": "not base64!"}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!({"input": {"format": "dwg", "data": "QUJD"}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!({"input": {"format": "ply", "data": "QUJD", "name": "../etc/x.ply"}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!({"input": {"format": "ply", "data": "QUJD", "name": "a/b.ply"}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!({"input": {"format": "ply", "data": "QUJD", "name": "a..b.ply"}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "convert",
            json!({"input": ply, "output_format": "laz", "rotation": 30}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "convert",
            json!({"input": ply, "output_format": "dxf"}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "convert",
            json!({"input": ply, "output_format": "laz", "box": {"min": [1, 1, 1], "max": [0, 0, 0]}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "convert",
            json!({"input": ply, "output_format": "laz", "box": {"min": [0, 0, 0], "max": [1, 1, 1], "rotation": 4000}}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "section_drawing",
            json!({"input": ply, "box": room_plan(), "view": "top"}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "section_drawing",
            json!({"input": ply, "box": room_plan(), "thickness": 6}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "section_drawing",
            json!({"input": ply, "box": room_plan(), "output_format": "pdf"}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "surface_mesh",
            json!({"input": ply, "neighbors": 40}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "surface_mesh",
            json!({"input": ply, "max_vertices": 2}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "surface_mesh",
            json!({"input": ply, "edge_factor": 0}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
    ];
    for (tool, arguments, expected, code) in cases {
        let (status, headers, body) =
            send(&router, post(&format!("/v1/tools/{tool}"), &arguments)).await;
        assert_eq!(status, expected, "{tool} {arguments}: {body}");
        assert_triple(status, &headers, &body, code);
    }
    assert!(service.clean());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn large_bodies_are_refused_as_small_ones_with_short_messages() {
    let service = Service::new();
    let router = router_with(service.open());
    // Bodies above 64 KiB are read on a blocking thread before the queue.
    let large = input("xyz", &field(5000));
    let long = "A".repeat(200_000);
    let rest = [
        (
            "unknown",
            json!({"input": large}),
            StatusCode::NOT_FOUND,
            "TOOL_ONBEKEND",
        ),
        (
            "inspect",
            json!({"input": large, "path": "/etc/passwd"}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        // A reader of JSON quotes a string of the wrong type in full.
        (
            "inspect",
            json!({"input": long}),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
        (
            "inspect",
            json!(long),
            StatusCode::BAD_REQUEST,
            "ARGUMENT_ONGELDIG",
        ),
    ];
    for (tool, arguments, expected, code) in rest {
        let (status, headers, body) =
            send(&router, post(&format!("/v1/tools/{tool}"), &arguments)).await;
        assert_eq!(status, expected, "{tool}: {}", body["melding"]);
        assert_triple(status, &headers, &body, code);
        let melding = body["melding"].as_str().unwrap();
        assert!(melding.chars().count() < 500, "{melding}");
    }
    let call = |id: u64, arguments: Value| {
        post(
            "/mcp",
            &rpc(
                json!(id),
                "tools/call",
                json!({"name": "inspect", "arguments": arguments}),
            ),
        )
    };
    for (id, arguments) in [
        (1, json!({"input": large, "path": "/etc/passwd"})),
        (2, json!({"input": long})),
    ] {
        let (status, _, body) = send(&router, call(id, arguments)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["id"], id);
        assert_eq!(body["result"]["isError"], true);
        let triple = &body["result"]["structuredContent"];
        assert_eq!(triple["error_code"], "ARGUMENT_ONGELDIG");
        assert!(triple["melding"].as_str().unwrap().chars().count() < 500);
    }
    let broken = format!(
        "{{\"jsonrpc\": \"2.0\", \"id\": 3, \"method\": {}}}",
        json!(long)
    );
    let (status, _, body) = send(
        &router,
        Request::post("/mcp").body(Body::from(broken)).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32601);
    // A large call that passes the policy is answered as a small one.
    let (status, _, body) = send(&router, call(4, json!({"input": large}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], false, "{body}");
    assert_eq!(
        body["result"]["structuredContent"]["report"]["points"],
        5000
    );
    assert!(service.clean());
}

#[test]
fn every_example_fits_its_arguments() {
    for tool in tools::table() {
        let example = serde_json::value::to_raw_value(&tool.example()).unwrap();
        tools::arguments_fit(tool.name, &example)
            .unwrap_or_else(|error| panic!("{}: {error:?}", tool.name));
        let keys = crate::policy::top_level_keys(example.get().as_bytes()).unwrap();
        crate::policy::check(tool.name, &keys).unwrap();
    }
}

// --- REST ----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_tool_answers_its_example() {
    let service = Service::new();
    let router = router_with(service.open());
    for tool in tools::table() {
        let (status, headers, body) = send(
            &router,
            post(&format!("/v1/tools/{}", tool.name), &tool.example()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{}: {body}", tool.name);
        assert_eq!(headers.get(header::CACHE_CONTROL).unwrap(), "no-store");
        assert!(body["report"].is_object(), "{}", tool.name);
        if tool.file_result {
            let output = &body["output"];
            let data = decode_base64(output["data"].as_str().unwrap()).unwrap();
            assert_eq!(output["bytes"], data.len());
            let sha = Sha256::digest(&data)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert_eq!(output["sha256"], sha);
            let format = output["format"].as_str().unwrap();
            assert_eq!(output["file_name"], format!("example.{format}"));
            assert_eq!(output["media_type"], crate::payload::media_type(format));
        } else {
            assert!(body.get("output").is_none());
        }
    }
    assert!(service.clean());
}

/// A seam that runs the real tool and reports what the work folder held
/// when the tool returned.
fn watching(seen: Arc<std::sync::Mutex<Vec<PathBuf>>>) -> Seam {
    seam(move |name, arguments, limits, work, keep_going| {
        let result = tools::call_blocking(name, arguments, limits, work, keep_going);
        let mut files = Vec::new();
        let mut pending = vec![work.path().to_path_buf()];
        while let Some(folder) = pending.pop() {
            for entry in fs::read_dir(folder).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    pending.push(entry.path());
                } else {
                    files.push(
                        entry
                            .path()
                            .strip_prefix(work.path())
                            .unwrap()
                            .to_path_buf(),
                    );
                }
            }
        }
        *seen.lock().unwrap() = files;
        result
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_result_over_the_output_limit_is_refused_before_it_is_written() {
    let service = Service::new();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let config = service.config(&[("OPENAEC_API_MAX_UITKOMST_MIB", "1")]);
    let router = router_with(Arc::new(AppState::with_seam(
        config,
        watching(Arc::clone(&seen)),
    )));
    let arguments = json!({"input": input("xyz", &field(60_000)), "output_format": "las"});
    let (status, headers, body) = send(&router, post("/v1/tools/convert", &arguments)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_triple(status, &headers, &body, "UITKOMST_TE_GROOT");
    assert!(body["remedie"].as_str().unwrap().contains("laz"));
    assert_eq!(
        *seen.lock().unwrap(),
        [PathBuf::from("input.xyz")],
        "only the input was written"
    );
    // Cropped to a box, the same file fits.
    let cropped = json!({
        "input": input("xyz", &field(60_000)),
        "output_format": "las",
        "box": {"min": [0.0, 0.0, -1.0], "max": [29.9, 1.0, 10.0]},
    });
    let (status, _, body) = send(&router, post("/v1/tools/convert", &cropped)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["report"]["points"], 3300);
    // A box that holds too much stops writing and leaves no file either.
    let wide = json!({
        "input": input("xyz", &field(60_000)),
        "output_format": "las",
        "box": {"min": [-1.0, -1.0, -1.0], "max": [100.0, 100.0, 100.0], "rotation": 10.0},
    });
    let (status, headers, body) = send(&router, post("/v1/tools/convert", &wide)).await;
    assert_triple(status, &headers, &body, "UITKOMST_TE_GROOT");
    assert_eq!(*seen.lock().unwrap(), [PathBuf::from("input.xyz")]);
    assert!(service.clean());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_empty_slab_is_refused_with_the_hint_of_its_view() {
    let service = Service::new();
    let router = router_with(service.open());
    for (view, section) in [
        (
            "plan",
            json!({"min": [-0.5, -0.5, 10.0], "max": [4.5, 3.5, 11.0]}),
        ),
        (
            "front",
            json!({"min": [0.5, 1.0, 0.5], "max": [3.5, 2.0, 2.0]}),
        ),
    ] {
        let arguments = json!({"input": input("xyz", &room()), "box": section, "view": view});
        let (status, headers, body) =
            send(&router, post("/v1/tools/section_drawing", &arguments)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
        assert_triple(status, &headers, &body, "TOOL_MISLUKT");
        assert_eq!(
            body["melding"],
            format!("Invalid point cloud: {}", ops::EMPTY_SLAB)
        );
        let view = DrawingView::from_key(view).unwrap();
        assert_eq!(body["remedie"], ops::empty_slab_hint(view));
    }
    assert!(service.clean());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_message_names_a_folder_of_the_service() {
    let service = Service::new();
    // The message of the core for a source that changed while a drawing
    // read it names the file with its folder, as here.
    let state = AppState::with_seam(
        service.config(&[]),
        seam(|_, _, _, work, _| {
            let source = work.path().join("input.xyz");
            Err(ApiError::tool_failed(
                &LoadError::InvalidData(format!("{} changed while it was read", source.display())),
                None,
            ))
        }),
    );
    let router = router_with(Arc::new(state));
    let arguments = json!({"input": input("xyz", &room()), "box": room_plan()});
    let (status, headers, body) =
        send(&router, post("/v1/tools/section_drawing", &arguments)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_triple(status, &headers, &body, "TOOL_MISLUKT");
    assert_eq!(
        body["melding"],
        "Invalid point cloud: input.xyz changed while it was read"
    );
    let work = service.work.path().display().to_string();
    assert!(!body.to_string().contains(&work), "{body}");
    assert!(service.clean());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_work_folder_is_gone_after_a_success_and_a_failure() {
    let service = Service::new();
    let router = router_with(service.open());
    let good = json!({"input": input("ply", &grid()), "output_format": "laz"});
    let (status, _, _) = send(&router, post("/v1/tools/convert", &good)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(service.clean());
    let bad = json!({"input": input("ply", b"ply\nnot a point cloud\n"), "output_format": "laz"});
    let (status, headers, body) = send(&router, post("/v1/tools/convert", &bad)).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_triple(status, &headers, &body, "TOOL_MISLUKT");
    assert!(service.clean());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compressed_pcd_is_inspected() {
    let service = Service::new();
    let router = router_with(service.open());
    let arguments = json!({"input": input("pcd", &compressed_pcd(700))});
    let (status, _, body) = send(&router, post("/v1/tools/inspect", &arguments)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["report"]["points"], 700);
    assert_eq!(body["report"]["format"], "pcd");
    assert!(service.clean());
}

/// The bytes of a REST answer's file.
fn data_of(body: &Value) -> Vec<u8> {
    decode_base64(body["output"]["data"].as_str().unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_tools_write_what_the_operations_write() {
    let service = Service::new();
    let router = router_with(service.open());
    let files = tempfile::tempdir().unwrap();
    let (grid_file, room_file) = (
        files.path().join("input.ply"),
        files.path().join("input.xyz"),
    );
    fs::write(&grid_file, grid()).unwrap();
    fs::write(&room_file, room()).unwrap();
    let direct = |name: &str| files.path().join(name);
    let never_stop: ops::KeepGoing<'_> = &ops::never_stop;

    // inspect
    let (_, _, body) = send(
        &router,
        post(
            "/v1/tools/inspect",
            &json!({"input": input("ply", &grid())}),
        ),
    )
    .await;
    let cloud = ops::open_source(&grid_file, never_stop).unwrap();
    assert_eq!(
        body["report"],
        serde_json::to_value(ops::inspect(&cloud)).unwrap()
    );

    // convert to PLY within a turned box
    let section = json!({"min": [-1.0, -1.0, -1.0], "max": [1.1, 2.5, 2.5], "rotation": 30.0});
    let arguments = json!({"input": input("ply", &grid()), "output_format": "ply", "box": section});
    let (status, _, body) = send(&router, post("/v1/tools/convert", &arguments)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let turned = OrientedBox::new(
        Bounds {
            min: [-1.0, -1.0, -1.0],
            max: [1.1, 2.5, 2.5],
        },
        30.0,
    );
    let convert = Convert {
        format: ExportFormat::PlyBinary,
        section: Some(turned),
        max_points: None,
    };
    let count = ops::convert_file(&grid_file, &direct("turned.ply"), convert, never_stop).unwrap();
    assert_eq!(body["report"]["points"], count);
    assert_eq!(data_of(&body), fs::read(direct("turned.ply")).unwrap());

    // section_drawing as DXF
    let arguments = json!({"input": input("xyz", &room()), "box": room_plan()});
    let (status, _, body) = send(&router, post("/v1/tools/section_drawing", &arguments)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let plan = OrientedBox::from(Bounds {
        min: [-0.5, -0.5, 0.0],
        max: [4.5, 3.5, 1.2],
    });
    let drawn = ops::section_drawing_file(
        &room_file,
        "input",
        &direct("plan.dxf"),
        plan,
        &SectionDrawingOptions::default(),
        CachedIndex::Never,
        never_stop,
    )
    .unwrap();
    assert_eq!(data_of(&body), fs::read(direct("plan.dxf")).unwrap());
    assert_eq!(body["report"]["drawn_points"], drawn.stats.drawn_points);
    assert_eq!(body["report"]["view"], "plan");
    assert_eq!(body["output"]["file_name"], "input.dxf");

    // terrain_mesh
    let (status, _, body) = send(
        &router,
        post(
            "/v1/tools/terrain_mesh",
            &json!({"input": input("ply", &grid())}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let stats = ops::terrain_mesh_file(&grid_file, &direct("terrain.obj"), never_stop).unwrap();
    assert_eq!(data_of(&body), fs::read(direct("terrain.obj")).unwrap());
    assert_eq!(body["report"]["triangles"], stats.triangles);

    // surface_mesh with two options
    let arguments = json!({"input": input("xyz", &room()), "max_vertices": 2000, "neighbors": 10});
    let (status, _, body) = send(&router, post("/v1/tools/surface_mesh", &arguments)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let config = SurfaceMeshConfig {
        max_vertices: 2000,
        neighbors: 10,
        ..SurfaceMeshConfig::default()
    };
    ops::surface_mesh_file(&room_file, &direct("surface.obj"), config, never_stop).unwrap();
    assert_eq!(data_of(&body), fs::read(direct("surface.obj")).unwrap());
    assert!(service.clean());
}

// --- MCP -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_answers_the_handshake_the_list_and_the_tools() {
    let service = Service::new();
    let router = router_with(service.open());
    let ask = |message: Value| post("/mcp", &message);

    for (requested, answered) in [
        (Some("2025-06-18"), "2025-06-18"),
        (Some("2025-03-26"), "2025-03-26"),
        (Some("1999-01-01"), "2025-11-25"),
        (None, "2025-11-25"),
    ] {
        let params =
            requested.map_or_else(|| json!({}), |version| json!({"protocolVersion": version}));
        let (status, _, body) = send(&router, ask(rpc(json!(1), "initialize", params))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["protocolVersion"], answered);
        assert_eq!(
            body["result"]["serverInfo"]["name"],
            "open-pointcloud-studio-api"
        );
        assert_eq!(
            body["result"]["capabilities"],
            json!({"tools": {"listChanged": false}})
        );
        assert!(body["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("base64"));
    }

    let (_, _, body) = send(&router, ask(rpc(json!(2), "tools/list", json!({})))).await;
    let listed = body["result"]["tools"].as_array().unwrap();
    assert_eq!(listed.len(), 5);
    assert!(listed
        .iter()
        .all(|tool| tool["annotations"]["readOnlyHint"] == true
            && tool["annotations"]["openWorldHint"] == false));
    let (_, _, rest) = send(&router, get("/v1/tools")).await;
    assert_eq!(body["result"], rest);

    let (_, _, body) = send(&router, ask(rpc(json!(3), "ping", json!({})))).await;
    assert_eq!(body["result"], json!({}));

    // inspect: the report as text and as structuredContent.
    let call = |id: u64, name: &str, arguments: Value| {
        ask(rpc(
            json!(id),
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        ))
    };
    let (_, _, body) = send(
        &router,
        call(4, "inspect", json!({"input": input("ply", &grid())})),
    )
    .await;
    let result = &body["result"];
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["report"]["points"], 1000);
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);

    // convert: the blob is the REST data, structuredContent the REST answer
    // without it.
    let arguments = json!({"input": input("ply", &grid()), "output_format": "laz"});
    let (_, _, rest) = send(&router, post("/v1/tools/convert", &arguments)).await;
    let (_, _, body) = send(&router, call(5, "convert", arguments)).await;
    let result = &body["result"];
    assert_eq!(body["id"], 5);
    assert_eq!(result["content"][1]["type"], "resource");
    assert_eq!(
        result["content"][1]["resource"]["blob"],
        rest["output"]["data"]
    );
    assert_eq!(result["content"][1]["resource"]["uri"], "result:input.laz");
    assert_eq!(
        result["content"][1]["resource"]["mimeType"],
        "application/octet-stream"
    );
    let mut without_data = rest.clone();
    without_data["output"]
        .as_object_mut()
        .unwrap()
        .remove("data");
    assert_eq!(result["structuredContent"], without_data);

    // An older protocol gets no structuredContent.
    let mut older = call(6, "inspect", json!({"input": input("ply", &grid())}));
    older
        .headers_mut()
        .insert("mcp-protocol-version", "2025-03-26".parse().unwrap());
    let (_, _, body) = send(&router, older).await;
    assert!(body["result"].get("structuredContent").is_none());
    assert_eq!(body["result"]["isError"], false);

    // Refusals of a tool are results the client can read.
    let (status, _, body) = send(&router, call(7, "load_project", json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"]["isError"], true);
    assert_eq!(
        body["result"]["structuredContent"]["error_code"],
        "TOOL_ONBEKEND"
    );
    assert!(body["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .starts_with("Error: "));
    let (_, _, body) = send(
        &router,
        call(
            8,
            "inspect",
            json!({"input": input("ply", &grid()), "path": "/etc/passwd"}),
        ),
    )
    .await;
    assert_eq!(
        body["result"]["structuredContent"]["error_code"],
        "ARGUMENT_ONGELDIG"
    );
    let (_, _, body) = send(
        &router,
        call(9, "inspect", json!({"input": input("ply", b"no cloud")})),
    )
    .await;
    assert_eq!(
        body["result"]["structuredContent"]["error_code"],
        "TOOL_MISLUKT"
    );
    assert!(service.clean());
}

#[tokio::test]
async fn mcp_protocol_errors_carry_the_triple() {
    let service = Service::new();
    let router = router_with(service.open());
    let raw = |text: &str| {
        Request::post("/mcp")
            .body(Body::from(text.to_owned()))
            .unwrap()
    };

    let (status, _, body) = send(
        &router,
        post(
            "/mcp",
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        ),
    )
    .await;
    assert_eq!((status, body), (StatusCode::ACCEPTED, Value::Null));

    let (status, _, body) = send(&router, raw("{\"jsonrpc\": ")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], -32700);
    assert_eq!(body["error"]["data"]["error_code"], "LICHAAM_ONLEESBAAR");
    assert!(body["error"]["data"].get("remedie").is_some());

    let (status, _, body) = send(
        &router,
        raw("[{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"ping\"}]"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], -32600);
    assert_eq!(body["error"]["data"]["error_code"], "ARGUMENT_ONGELDIG");

    let (status, _, body) = send(&router, raw("{\"id\": 1, \"method\": \"ping\"}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], -32600);

    let (status, _, body) = send(
        &router,
        post("/mcp", &rpc(json!("x"), "resources/list", json!({}))),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], "x");
    assert_eq!(body["error"]["code"], -32601);
    assert_eq!(body["error"]["data"]["error_code"], "ROUTE_ONBEKEND");

    let (status, _, body) = send(
        &router,
        post(
            "/mcp",
            &rpc(json!(2), "tools/call", json!({"arguments": {}})),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["error"]["code"], -32602);
    assert_eq!(body["error"]["data"]["error_code"], "ARGUMENT_ONGELDIG");

    // An id of null is an id, not a notification.
    let (status, _, body) = send(&router, post("/mcp", &rpc(Value::Null, "ping", json!({})))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["result"], json!({}));
}

// --- OpenAPI -------------------------------------------------------------

#[tokio::test]
async fn the_description_has_a_path_per_tool_with_its_schema() {
    let service = Service::new();
    let router = router_with(service.open());
    let (status, _, document) = send(&router, get("/v1/openapi.json")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(document["openapi"], "3.1.0");
    assert_eq!(document["info"]["version"], crate::VERSION);
    let paths = document["paths"].as_object().unwrap();
    let names: Vec<String> = paths
        .keys()
        .map(|path| path.trim_start_matches("/v1/tools/").to_owned())
        .collect();
    let tools: Vec<&str> = tools::table().iter().map(|tool| tool.name).collect();
    assert_eq!(
        names.iter().collect::<BTreeSet<_>>(),
        tools
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>()
            .iter()
            .collect::<BTreeSet<_>>()
    );
    for tool in tools::table() {
        let operation = &paths[&format!("/v1/tools/{}", tool.name)]["post"];
        let content = &operation["requestBody"]["content"]["application/json"];
        assert_eq!(content["schema"], tool.input_schema(), "{}", tool.name);
        assert_eq!(content["example"], tool.example(), "{}", tool.name);
        assert_eq!(
            operation["security"],
            json!([{"sleutel": []}]),
            "{}",
            tool.name
        );
        assert_eq!(operation["operationId"], tool.name);
        for status in [
            "400", "401", "403", "404", "405", "413", "422", "500", "503", "504",
        ] {
            assert_eq!(
                operation["responses"][status]["$ref"],
                "#/components/responses/Fout"
            );
        }
        let result = operation["responses"]["200"]["content"]["application/json"]["schema"]["$ref"]
            .as_str()
            .unwrap();
        let schema = result.trim_start_matches("#/components/schemas/");
        assert!(
            document["components"]["schemas"][schema].is_object(),
            "{schema}"
        );
    }
    assert_eq!(
        document["components"]["securitySchemes"]["sleutel"],
        json!({"type": "http", "scheme": "bearer"})
    );
}

// --- docs/cloud-api.md ---------------------------------------------------

const DOCUMENT: &str = include_str!("../../../docs/cloud-api.md");

/// The rows of the first table under `heading`, as cells without their
/// backticks, without the header and the rule.
fn table_under(heading: &str) -> Vec<Vec<String>> {
    let start = DOCUMENT
        .lines()
        .position(|line| line.trim() == heading)
        .unwrap_or_else(|| panic!("{heading} is in docs/cloud-api.md"));
    DOCUMENT
        .lines()
        .skip(start + 1)
        .skip_while(|line| !line.starts_with('|'))
        .take_while(|line| line.starts_with('|'))
        .skip(2)
        .map(|line| {
            line.trim_matches('|')
                .split('|')
                .map(|cell| cell.trim().replace('`', ""))
                .collect()
        })
        .collect()
}

#[test]
fn the_document_lists_the_tools_and_the_errors_of_the_code() {
    let documented: Vec<String> = table_under("## Tools")
        .into_iter()
        .map(|row| row[0].clone())
        .collect();
    let tools: Vec<&str> = tools::table().iter().map(|tool| tool.name).collect();
    assert_eq!(documented, tools);
    let mapping: Vec<String> =
        table_under("## Cloud tools beside the desktop and the command line")
            .into_iter()
            .map(|row| row[0].clone())
            .collect();
    assert_eq!(mapping, tools);

    let errors: BTreeSet<(u16, String)> = table_under("## Errors")
        .into_iter()
        .flat_map(|row| {
            let status: u16 = row[0].parse().unwrap_or_else(|_| panic!("{row:?}"));
            row[1]
                .split('/')
                .map(|code| (status, code.trim().to_owned()))
                .collect::<Vec<_>>()
        })
        .collect();
    let codes: BTreeSet<(u16, String)> = crate::error::CODES
        .iter()
        .map(|(status, code)| (*status, (*code).to_owned()))
        .collect();
    assert_eq!(errors, codes);

    for anchor in [
        "## Extensions to the shared contract",
        "## Limits",
        "## What is not in the cloud",
    ] {
        assert!(
            DOCUMENT.lines().any(|line| line.trim() == anchor),
            "{anchor}"
        );
    }
    // Every limit variable is documented with its default.
    for variable in [
        "OPENAEC_API_SLEUTELS",
        "OPENAEC_API_TOEGANG",
        "OPENAEC_MCP_HTTP",
        "OPENAEC_MCP_LOG",
        "OPENAEC_API_LOGVORM",
        "OPENAEC_API_MAX_LICHAAM_MIB",
        "OPENAEC_API_MAX_UITKOMST_MIB",
        "OPENAEC_API_GELIJKTIJDIG",
        "OPENAEC_API_WACHTRIJ",
        "OPENAEC_API_WACHTEN_S",
        "OPENAEC_API_TIJDSLIMIET_S",
        "OPENAEC_API_WERKMAP",
        "OPENAEC_API_HERKOMSTEN",
        "OPENAEC_API_GEHEUGEN_MIB",
    ] {
        assert!(DOCUMENT.contains(&format!("`{variable}`")), "{variable}");
    }
}

#[test]
fn every_error_the_code_makes_is_in_the_list_of_codes() {
    let made = [
        ApiError::argument("x"),
        ApiError::unreadable("x"),
        ApiError::no_access(),
        ApiError::origin(),
        ApiError::unknown_tool("x"),
        ApiError::unknown_route(),
        ApiError::wrong_method(),
        ApiError::too_large(1),
        ApiError::output_too_large(1),
        ApiError::tool_failed(&LoadError::Cancelled, None),
        ApiError::internal("x"),
        ApiError::busy(),
        ApiError::timeout(1),
    ];
    for error in made {
        assert!(
            crate::error::CODES.contains(&(error.status.as_u16(), error.code)),
            "{error:?}"
        );
    }
    assert!(crate::error::CODES.contains(&(503, "WERK_VASTGELOPEN")));
}
