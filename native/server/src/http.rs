//! The routes of the service and the layers around them.
//!
//! `/v1/health` and `/v1/openapi.json` are public. `/v1/tools`,
//! `/v1/tools/{name}` and `/mcp` check the `Origin` first, then the key, and
//! then take a place in the waiting room when their body may be large. Every
//! answer that is not a success is the JSON triple, also for an unknown
//! route, a wrong method, a body that is too large and a panic.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, Extensions, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pointcloud_core::ops::KeepGoing;
use serde_json::value::RawValue;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tower_http::catch_panic::CatchPanicLayer;

use crate::config::{Access, Config};
use crate::error::{short, ApiError, Scrub};
use crate::limits::{Limits, Place, SMALL_REQUEST};
use crate::tools::{CallLimits, ToolOutput};
use crate::{mcp, openapi, payload, policy, tools};

/// What runs a tool: `tools::call_blocking`, or a stand-in of a test.
pub type Seam = Arc<
    dyn Fn(&str, &RawValue, &CallLimits, &TempDir, KeepGoing<'_>) -> Result<ToolOutput, ApiError>
        + Send
        + Sync,
>;

/// What every route shares.
pub struct AppState {
    pub config: Config,
    pub limits: Limits,
    pub(crate) seam: Seam,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self::with_seam(config, Arc::new(tools::call_blocking))
    }

    /// The state with another runner of the tools.
    pub fn with_seam(config: Config, seam: Seam) -> Self {
        Self {
            limits: Limits::new(&config),
            config,
            seam,
        }
    }

    fn call_limits(&self) -> CallLimits {
        CallLimits {
            max_output: self.config.max_output,
        }
    }
}

/// The tool a request called, for the log line.
#[derive(Debug, Clone)]
pub(crate) struct ToolName(pub(crate) String);

/// The first eight hex digits of the hash of the key a request used, for
/// the log line; never the key.
#[derive(Debug, Clone)]
struct KeyPrefix(String);

fn json_body(status: StatusCode, body: Vec<u8>) -> Response {
    (
        status,
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        )],
        body,
    )
        .into_response()
}

/// What `read` finds in `body`: at once for a small body, else on a
/// blocking thread, so that a pass over a large body does not hold up a
/// thread that answers other requests.
pub(crate) async fn read_body<T: Send + 'static>(
    body: &Bytes,
    read: impl FnOnce(&Bytes) -> T + Send + 'static,
) -> Result<T, ApiError> {
    if body.len() as u64 <= SMALL_REQUEST {
        return Ok(read(body));
    }
    let body = body.clone();
    tokio::task::spawn_blocking(move || read(&body))
        .await
        .map_err(|_| ApiError::internal("The service failed while it read the request."))
}

/// A body that could not be read, as the triple.
fn body_error(state: &AppState, rejection: BytesRejection) -> Response {
    if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::too_large(state.config.max_body).into_response()
    } else {
        ApiError::unreadable(format!("The body could not be read: {rejection}")).into_response()
    }
}

/// Run tool `name` on `arguments` in a fresh work folder and build its
/// answer with `answer`. The folder is removed when this returns, after a
/// success and after a failure, and no message names it or the service's
/// other folders.
pub(crate) fn run_tool(
    state: &AppState,
    name: &str,
    arguments: &[u8],
    keep_going: KeepGoing<'_>,
    answer: impl FnOnce(&ToolOutput) -> Result<Vec<u8>, ApiError>,
) -> Result<Vec<u8>, ApiError> {
    let work = tempfile::Builder::new()
        .prefix("call-")
        .tempdir_in(&state.config.work_dir)
        .map_err(|_| ApiError::internal("No work folder could be made."))?;
    let temp = std::env::temp_dir();
    let scrub = Scrub::new(&[work.path(), &state.config.work_dir, &temp]);
    let result = (|| {
        let arguments: &RawValue = serde_json::from_slice(arguments).map_err(|error| {
            ApiError::argument(format!(
                "The arguments must be one JSON object: {}",
                short(&error)
            ))
        })?;
        let output = (state.seam)(name, arguments, &state.call_limits(), &work, keep_going)?;
        answer(&output)
    })();
    drop(work);
    result.map_err(|error| error.scrubbed(&scrub))
}

async fn health(State(state): State<Arc<AppState>>) -> Response {
    let mut body = json!({
        "status": "ok",
        "versie": crate::VERSION,
        "protocol": crate::PROTOCOL,
    });
    if state.limits.stuck() {
        body["status"] = Value::from("vastgelopen");
        body["error_code"] = Value::from("WERK_VASTGELOPEN");
        body["melding"] =
            Value::from("Work that was cancelled at its time limit is still running.");
        body["remedie"] = Value::from("Replace this instance of the service.");
        return (StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response();
    }
    Json(body).into_response()
}

async fn openapi_document() -> Json<Value> {
    Json(openapi::document())
}

async fn list_tools() -> Json<Value> {
    Json(tools::listing())
}

/// `POST /v1/tools/{name}`: the body holds the arguments of the tool.
async fn call_tool(
    State(state): State<Arc<AppState>>,
    uri: Uri,
    extensions: Extensions,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let name = uri
        .path()
        .strip_prefix("/v1/tools/")
        .unwrap_or_default()
        .to_owned();
    let place = extensions.get::<Place>().cloned().unwrap_or_default();
    let mut response = call_tool_named(&state, &name, body, place).await;
    response.extensions_mut().insert(ToolName(name));
    response
}

async fn call_tool_named(
    state: &Arc<AppState>,
    name: &str,
    body: Result<Bytes, BytesRejection>,
    place: Place,
) -> Response {
    let body = match body {
        Ok(body) if body.is_empty() => Bytes::from_static(b"{}"),
        Ok(body) => body,
        Err(rejection) => return body_error(state, rejection),
    };
    let tool = name.to_owned();
    let checked = read_body(&body, move |body| {
        policy::top_level_keys(body)
            .and_then(|keys| policy::check(&tool, &keys))
            .map(drop)
    })
    .await;
    if let Err(error) = checked.and_then(|checked| checked) {
        return error.into_response();
    }
    let worker = Arc::clone(state);
    let tool = name.to_owned();
    let place = place.take();
    let outcome = state
        .limits
        .run(move |keep_going| {
            let _place = place;
            run_tool(&worker, &tool, &body, keep_going, payload::rest_answer)
        })
        .await;
    match outcome {
        Ok(Ok(answer)) => json_body(StatusCode::OK, answer),
        Ok(Err(error)) | Err(error) => error.into_response(),
    }
}

async fn mcp_message(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    extensions: Extensions,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    let place = extensions.get::<Place>().cloned().unwrap_or_default();
    match body {
        Ok(body) => mcp::handle(state, &headers, body, place).await,
        Err(rejection) => body_error(&state, rejection),
    }
}

async fn unknown_route() -> Response {
    ApiError::unknown_route().into_response()
}

async fn wrong_method() -> Response {
    ApiError::wrong_method().into_response()
}

/// Refuse an `Origin` that is not listed; a request without one passes.
async fn check_origin(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let listed = origin
            .to_str()
            .map(str::to_ascii_lowercase)
            .is_ok_and(|origin| state.config.origins.contains(&origin));
        if !listed {
            return ApiError::origin().into_response();
        }
    }
    next.run(request).await
}

/// The first eight hex digits of a hash.
fn hash_prefix(hash: &[u8; 32]) -> String {
    hash[..4].iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Refuse a request without a valid key, unless access is open.
async fn check_access(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Response {
    let prefix = match &state.config.access {
        Access::Open => None,
        Access::Keys(hashes) => {
            let key = request
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    let (scheme, key) = value.split_at_checked(7)?;
                    scheme.eq_ignore_ascii_case("bearer ").then(|| key.trim())
                })
                .filter(|key| !key.is_empty());
            let hash: Option<[u8; 32]> = key.map(|key| Sha256::digest(key.as_bytes()).into());
            match hash.filter(|hash| hashes.contains(hash)) {
                Some(hash) => Some(hash_prefix(&hash)),
                None => return ApiError::no_access().into_response(),
            }
        }
    };
    let mut response = next.run(request).await;
    if let Some(prefix) = prefix {
        response.extensions_mut().insert(KeyPrefix(prefix));
    }
    response
}

/// Take a place in the waiting room for a body that may be large, before
/// the body is read. A call takes it along into its work, which gives it
/// back when it ends; else it is given back with the answer.
async fn admission(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let length = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    // A list is asked for without a body.
    let length = match (request.method(), length) {
        (&Method::GET, None) => Some(0),
        (_, length) => length,
    };
    match state.limits.admit(length) {
        Ok(place) => {
            let place = Place::new(place);
            request.extensions_mut().insert(place.clone());
            let response = next.run(request).await;
            drop(place);
            response
        }
        Err(error) => error.into_response(),
    }
}

/// Answers of the tool routes are never stored by a cache on the way.
async fn no_store(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// One line per request: method, route, tool, status, duration, body bytes
/// and the prefix of the key's hash; never a body or a path of an upload.
async fn log_request(request: Request, next: Next) -> Response {
    let started = Instant::now();
    let method = request.method().clone();
    let route: String = request.uri().path().chars().take(100).collect();
    let body_bytes = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    let response = next.run(request).await;
    let tool = response
        .extensions()
        .get::<ToolName>()
        .map(|tool| tool.0.chars().take(64).collect::<String>());
    let key = response
        .extensions()
        .get::<KeyPrefix>()
        .map(|key| key.0.clone());
    tracing::info!(
        method = %method,
        route = %route,
        tool = tool.as_deref().unwrap_or(""),
        status = response.status().as_u16(),
        millis = started.elapsed().as_millis() as u64,
        body_bytes = body_bytes.unwrap_or(0),
        key = key.as_deref().unwrap_or(""),
        "request"
    );
    response
}

fn panic_answer(_: Box<dyn std::any::Any + Send + 'static>) -> Response {
    ApiError::internal("The service failed while it answered.").into_response()
}

/// The routes with the default runner of the tools.
pub fn router(config: Config) -> Router {
    router_with(Arc::new(AppState::new(config)))
}

/// The routes over `state`.
pub fn router_with(state: Arc<AppState>) -> Router {
    let max_body = state.config.max_body;
    let protected = Router::new()
        .route("/v1/tools", get(list_tools))
        .route("/v1/tools/{name}", post(call_tool))
        .route("/mcp", post(mcp_message))
        // The last layer runs first: the origin, then the key, then the
        // waiting room.
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            admission,
        ))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            check_access,
        ))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            check_origin,
        ))
        .route_layer(middleware::from_fn(no_store));
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/openapi.json", get(openapi_document))
        .merge(protected)
        .fallback(unknown_route)
        .method_not_allowed_fallback(wrong_method)
        .layer(DefaultBodyLimit::max(max_body))
        .layer(CatchPanicLayer::custom(panic_answer))
        .layer(middleware::from_fn(log_request))
        .with_state(state)
}

/// Whether the service can write in `folder`: it is made when missing, and
/// a file is written and removed in it.
pub fn check_folder(folder: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(folder)?;
    let probe = tempfile::Builder::new()
        .prefix(".write-check-")
        .tempfile_in(folder)?;
    std::io::Write::write_all(&mut probe.as_file(), b"ok")?;
    probe.close()
}
