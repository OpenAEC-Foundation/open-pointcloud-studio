//! MCP over `POST /mcp`: one JSON-RPC message per request, answered as
//! JSON (Streamable HTTP without streams), without sessions.
//!
//! The tools are those of the REST routes and do the same work. A file
//! result travels as an embedded resource whose blob is its base64, and
//! `structuredContent` is the REST answer without `output.data`, so that
//! the file is not sent twice.

use std::sync::Arc;

use axum::body::Bytes;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::de::IgnoredAny;
use serde::{Deserialize, Deserializer};
use serde_json::value::RawValue;
use serde_json::{json, Value};

use crate::error::{short, ApiError};
use crate::http::{read_body, run_tool, AppState, ToolName};
use crate::limits::Place;
use crate::{payload, policy, tools};

/// The protocol versions the service speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
/// The first version whose tool results carry `structuredContent`.
const STRUCTURED_SINCE: &str = "2025-06-18";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// The version `requested` when the service speaks it, else the newest.
pub fn negotiate(requested: Option<&str>) -> &'static str {
    PROTOCOL_VERSIONS
        .into_iter()
        .find(|version| Some(*version) == requested)
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

/// Whether tool results carry `structuredContent`: from 2025-06-18 on, by
/// the `MCP-Protocol-Version` header a client sends after the handshake,
/// and when it sends none.
fn structured(headers: &HeaderMap) -> bool {
    headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok())
        .is_none_or(|version| version >= STRUCTURED_SINCE)
}

/// The id of a message: absent for a notification, any JSON value else.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default, deserialize_with = "present")]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(borrow, default)]
    params: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct CallParams<'a> {
    name: String,
    #[serde(borrow, default)]
    arguments: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct InitializeParams {
    #[serde(default, rename = "protocolVersion")]
    protocol_version: Option<String>,
}

fn json_answer(status: StatusCode, body: Vec<u8>) -> Response {
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

/// A JSON-RPC error with the triple of the contract as its `data`.
fn rpc_error(status: StatusCode, id: &Value, code: i64, error: &ApiError) -> Response {
    let body = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": error.melding, "data": error.json()},
    });
    json_answer(status, serde_json::to_vec(&body).unwrap_or_default())
}

fn rpc_result(id: &Value, result: Value) -> Response {
    let body = json!({"jsonrpc": "2.0", "id": id, "result": result});
    json_answer(
        StatusCode::OK,
        serde_json::to_vec(&body).unwrap_or_default(),
    )
}

/// A tool that failed, as a result the client can read: `isError`, the
/// message as text, and the triple as `structuredContent`.
fn tool_error(id: &Value, error: &ApiError, structured: bool) -> Response {
    let mut result = json!({
        "content": [{"type": "text", "text": format!("Error: {}", error.melding)}],
        "isError": true,
    });
    if structured {
        result["structuredContent"] = error.json();
    }
    rpc_result(id, result)
}

fn instructions(state: &AppState) -> String {
    let mib = 1024 * 1024;
    format!(
        "These tools work on point-cloud files that travel in the request. Each tool takes the file as input.data, base64 (RFC 4648 with padding, no line breaks), with input.format its extension; a file result comes back as an embedded resource whose blob is its base64. No tool reads or writes a path on the server, and nothing persists between calls. Boxes are in the units of the scan. A request may hold {} MiB, a result {} MiB before base64, and a call may take {} s.",
        state.config.max_body / mib,
        state.config.max_output / mib as u64,
        state.config.time_limit.as_secs()
    )
}

/// What one message asks for, once its body has been read.
enum Next {
    /// The whole answer: a protocol error, a notification, the handshake, a
    /// list, a ping, or a call that the policy refused.
    Answer(Response),
    /// A `tools/call` that the policy lets through, with its arguments as
    /// they lie in the body.
    Call {
        id: Value,
        name: String,
        arguments: Bytes,
    },
}

/// Answer one message of `POST /mcp`. Its body is read on a blocking thread
/// when it is large; only a call that passes the policy goes on to the
/// limits and the work.
pub async fn handle(
    state: Arc<AppState>,
    headers: &HeaderMap,
    body: Bytes,
    place: Place,
) -> Response {
    let structured = structured(headers);
    let reader = Arc::clone(&state);
    let next = match read_body(&body, move |body| read(&reader, body, structured)).await {
        Ok(next) => next,
        Err(error) => return error.into_response(),
    };
    match next {
        Next::Answer(response) => response,
        Next::Call {
            id,
            name,
            arguments,
        } => {
            let mut response = call(&state, &id, &name, arguments, structured, place).await;
            response.extensions_mut().insert(ToolName(name));
            response
        }
    }
}

/// Read one message: the answer when it needs no work, else the call.
fn read(state: &AppState, body: &Bytes, structured: bool) -> Next {
    let null = Value::Null;
    if body.iter().find(|byte| !byte.is_ascii_whitespace()) == Some(&b'[') {
        let error =
            ApiError::argument("Batches are not supported: send one JSON-RPC message per request.");
        return Next::Answer(rpc_error(
            StatusCode::BAD_REQUEST,
            &null,
            INVALID_REQUEST,
            &error,
        ));
    }
    let envelope: Envelope<'_> = match serde_json::from_slice(body) {
        Ok(envelope) => envelope,
        Err(error) => {
            return Next::Answer(if serde_json::from_slice::<IgnoredAny>(body).is_err() {
                let error = ApiError::unreadable(format!("Parse error: {}", short(&error)));
                rpc_error(StatusCode::BAD_REQUEST, &null, PARSE_ERROR, &error)
            } else {
                let error = ApiError::argument(format!("Invalid Request: {}", short(&error)));
                rpc_error(StatusCode::BAD_REQUEST, &null, INVALID_REQUEST, &error)
            });
        }
    };
    let id = envelope.id.clone().unwrap_or(Value::Null);
    let method = match (&envelope.jsonrpc, &envelope.method) {
        (Some(version), Some(method)) if version == "2.0" => method.clone(),
        _ => {
            let error = ApiError::argument(
                "Invalid Request: a JSON-RPC 2.0 message needs \"jsonrpc\": \"2.0\" and a method.",
            );
            return Next::Answer(rpc_error(
                StatusCode::BAD_REQUEST,
                &id,
                INVALID_REQUEST,
                &error,
            ));
        }
    };
    // A notification is taken without an answer, whatever it says.
    if envelope.id.is_none() {
        return Next::Answer(StatusCode::ACCEPTED.into_response());
    }
    Next::Answer(match method.as_str() {
        "initialize" => {
            let requested = envelope
                .params
                .and_then(|params| serde_json::from_str::<InitializeParams>(params.get()).ok())
                .and_then(|params| params.protocol_version);
            rpc_result(
                &id,
                json!({
                    "protocolVersion": negotiate(requested.as_deref()),
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": crate::NAME, "version": crate::VERSION},
                    "instructions": instructions(state),
                }),
            )
        }
        "ping" => rpc_result(&id, json!({})),
        "tools/list" => rpc_result(&id, tools::listing()),
        "tools/call" => {
            let Some(params) = envelope
                .params
                .and_then(|params| serde_json::from_str::<CallParams<'_>>(params.get()).ok())
            else {
                let error = ApiError::argument("tools/call needs params with the name of a tool.");
                return Next::Answer(rpc_error(StatusCode::OK, &id, INVALID_PARAMS, &error));
            };
            let name = params.name.clone();
            let arguments = match params.arguments {
                Some(arguments) => {
                    // The arguments as they lie in the body, without a copy.
                    let start = arguments.get().as_ptr() as usize - body.as_ptr() as usize;
                    body.slice(start..start + arguments.get().len())
                }
                None => Bytes::from_static(b"{}"),
            };
            drop(envelope);
            let checked =
                policy::top_level_keys(&arguments).and_then(|keys| policy::check(&name, &keys));
            if let Err(error) = checked {
                let mut response = tool_error(&id, &error, structured);
                response.extensions_mut().insert(ToolName(name));
                return Next::Answer(response);
            }
            return Next::Call {
                id,
                name,
                arguments,
            };
        }
        other => {
            let shown: String = other.chars().take(64).collect();
            let error = ApiError::new(
                StatusCode::NOT_FOUND,
                "ROUTE_ONBEKEND",
                format!("Method not found: {shown}"),
                Some("The service answers initialize, ping, tools/list and tools/call."),
            );
            rpc_error(StatusCode::OK, &id, METHOD_NOT_FOUND, &error)
        }
    })
}

/// `tools/call` that passed the policy: the same work as the REST route,
/// within the same limits. A refusal of the limits is an HTTP error, as on
/// the REST route; a failure of the tool is a result with `isError`.
async fn call(
    state: &Arc<AppState>,
    id: &Value,
    name: &str,
    arguments: Bytes,
    structured: bool,
    place: Place,
) -> Response {
    let worker = Arc::clone(state);
    let (tool, answer_id) = (name.to_owned(), id.clone());
    let place = place.take();
    let outcome = state
        .limits
        .run(move |keep_going| {
            let _place = place;
            run_tool(&worker, &tool, &arguments, keep_going, |output| {
                payload::mcp_answer(&answer_id, output, structured)
            })
        })
        .await;
    match outcome {
        Ok(Ok(body)) => json_answer(StatusCode::OK, body),
        Ok(Err(error)) => tool_error(id, &error, structured),
        Err(error) => error.into_response(),
    }
}
