//! A Model Context Protocol server on standard input and output: JSON-RPC
//! 2.0 messages, one per line, whose tools drive a running window of the
//! application through its local command API.

mod instances;
#[cfg(test)]
mod mock;
mod schema;
mod tools;

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard, PoisonError};
use std::thread;

use serde_json::{json, Value};

// The tests of the window hold what it reports against what a wait looks at.
#[cfg(test)]
pub(crate) use tools::busy;
use tools::{CallError, Link, Outcome};

/// Protocol versions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// The first protocol version whose tool results carry `structuredContent`.
const STRUCTURED_SINCE: &str = "2025-06-18";
/// The first protocol version that reports arguments which do not fit a
/// tool's schema as a tool result with `isError`, so the caller can correct
/// them; older versions get a JSON-RPC error.
const INVALID_ARGUMENTS_AS_RESULT_SINCE: &str = "2025-11-25";
/// Longest message read; a longer line is answered with a parse error.
const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

const INSTRUCTIONS: &str = "These tools drive a running Open Pointcloud Studio window, a point-cloud viewer and editor, through its local command API. The first tool call uses the most recently started window, or starts one when none runs; list_instances and select_instance choose another. Positions are scene coordinates in the units of the scans (normally metres), angles are radians and screen positions are viewport pixels from the top-left corner. Call wait_until_idle after open and before screenshot or export_bcf. Exports, selections, picks, meshes, face detections and merges answer with a job_id: pass wait_seconds or call wait_for_job. screenshot returns the 3D viewport as an image, or the drawing while drawing_view shows it.";

type Failure = (i64, String);

/// Run the server on standard input and output until the input ends, and
/// return the exit code of the process.
pub fn run() -> i32 {
    let program = match window_program(
        std::env::var_os("APPIMAGE"),
        std::env::var_os("APPDIR"),
        is_mount_point,
        std::env::current_exe(),
    ) {
        Ok(program) => program,
        Err(error) => {
            eprintln!("open-pointcloud-studio mcp: cannot find this program: {error}");
            return 1;
        }
    };
    let directory = crate::native_api::discovery_directory();
    let link = match instances::Connector::new(directory, program, true) {
        Ok(link) => link,
        Err(error) => {
            eprintln!("open-pointcloud-studio mcp: {error}");
            return 1;
        }
    };
    eprintln!(
        "open-pointcloud-studio mcp {}: serving on standard input and output; windows are found in {}",
        env!("CARGO_PKG_VERSION"),
        link.directory().display()
    );
    let stdin = io::stdin();
    match serve(stdin.lock(), Box::new(io::stdout()), Box::new(link)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("open-pointcloud-studio mcp: {error}");
            1
        }
    }
}

/// The program that starts a new window: this executable, or the AppImage it
/// was started from. Inside a mounted AppImage the executable lies in a mount
/// that goes away when this server ends, which would end the windows it
/// started and promises to leave open; the AppImage file itself stays.
///
/// `appimage` and `appdir` are the variables the AppImage runtime sets. Every
/// program an AppImage starts inherits them, so they count only when this
/// executable lies in the folder they announce. That folder must also be a
/// mount: an AppImage that was unpacked instead runs the application as a
/// child of the process that is started, so the window would not be
/// recognised by its process id, and where mounting is not possible the file
/// does not start at all without the option that unpacks it. The unpacked
/// executable starts a window directly.
fn window_program(
    appimage: Option<OsString>,
    appdir: Option<OsString>,
    is_mount: impl Fn(&Path) -> bool,
    executable: io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    let executable = executable?;
    let appimage = appimage.map(PathBuf::from);
    let appdir = appdir.map(PathBuf::from);
    Ok(match (appimage, appdir) {
        (Some(appimage), Some(appdir))
            if lies_in(&executable, &appdir) && is_mount(&appdir) && appimage.is_file() =>
        {
            appimage
        }
        _ => executable,
    })
}

/// Whether a resolved path lies in a folder, which may be named through a
/// link.
fn lies_in(path: &Path, folder: &Path) -> bool {
    // An empty path is the start of every path.
    !folder.as_os_str().is_empty()
        && (path.starts_with(folder)
            || std::fs::canonicalize(folder).is_ok_and(|resolved| path.starts_with(resolved)))
}

/// Whether a folder is where a file system is mounted: it then lies on
/// another device than the folder that holds it.
#[cfg(unix)]
fn is_mount_point(folder: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (folder.metadata(), folder.parent().map(Path::metadata)) {
        (Ok(own), Some(Ok(parent))) => own.dev() != parent.dev(),
        _ => false,
    }
}

/// AppImages exist on Linux only.
#[cfg(not(unix))]
fn is_mount_point(_: &Path) -> bool {
    false
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the reading thread and the thread that carries out tool calls share.
struct Shared {
    writer: Mutex<Box<dyn Write + Send>>,
    /// The protocol version agreed in `initialize`.
    protocol: Mutex<Option<&'static str>>,
    /// Requests the client has cancelled, by their id as JSON text.
    cancelled: Mutex<HashSet<String>>,
    /// Set when a message could not be written: the client has gone.
    closed: AtomicBool,
    /// Set when the input ended and work under way is to stop.
    ended: AtomicBool,
}

impl Shared {
    /// Write one message as one line. Serialized JSON holds no line breaks.
    fn write(&self, message: &Value) {
        let mut line = message.to_string().into_bytes();
        line.push(b'\n');
        let mut writer = lock(&self.writer);
        if writer
            .write_all(&line)
            .and_then(|()| writer.flush())
            .is_err()
        {
            self.closed.store(true, Ordering::Relaxed);
        }
    }

    fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    fn is_cancelled(&self, key: &str) -> bool {
        self.has_ended() || lock(&self.cancelled).contains(key)
    }

    fn forget(&self, key: &str) {
        lock(&self.cancelled).remove(key);
    }

    fn structured(&self) -> bool {
        lock(&self.protocol).is_none_or(|version| version >= STRUCTURED_SINCE)
    }

    fn invalid_arguments_as_result(&self) -> bool {
        lock(&self.protocol).is_some_and(|version| version >= INVALID_ARGUMENTS_AS_RESULT_SINCE)
    }
}

/// Tool calls, carried out one after another in the order they arrived.
enum Work {
    Call(Value),
    /// A batch that holds at least one tool call; it is answered as a whole.
    Batch(Vec<Value>),
}

/// Serve JSON-RPC messages from `reader` until it ends. Initialization,
/// pings, tool lists and notifications are answered at once; tool calls run
/// in order on another thread, so a long wait can be cancelled. The end of
/// the input shuts the server down: a running wait stops and queued calls
/// are dropped unanswered.
pub fn serve<R: BufRead>(
    reader: R,
    writer: Box<dyn Write + Send>,
    link: Box<dyn Link>,
) -> io::Result<()> {
    serve_with_limit(reader, writer, link, MAX_MESSAGE_BYTES, true)
}

/// `serve` with a longest message of `limit` bytes. Without
/// `cancel_at_end`, queued calls are carried out and answered before it
/// returns.
fn serve_with_limit<R: BufRead>(
    mut reader: R,
    writer: Box<dyn Write + Send>,
    mut link: Box<dyn Link>,
    limit: usize,
    cancel_at_end: bool,
) -> io::Result<()> {
    let shared = Arc::new(Shared {
        writer: Mutex::new(writer),
        protocol: Mutex::new(None),
        cancelled: Mutex::new(HashSet::new()),
        closed: AtomicBool::new(false),
        ended: AtomicBool::new(false),
    });
    let (sender, receiver) = mpsc::channel::<Work>();
    let worker = {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            for work in receiver {
                carry_out(work, link.as_mut(), &shared);
            }
        })
    };
    let result = loop {
        if shared.closed.load(Ordering::Relaxed) {
            break Ok(());
        }
        match read_line(&mut reader, limit) {
            Ok(Line::End) => break Ok(()),
            Ok(Line::TooLong) => shared.write(&error(
                Value::Null,
                PARSE_ERROR,
                &format!("Parse error: the message is longer than {limit} bytes"),
            )),
            Ok(Line::Message(bytes)) => receive(&bytes, &shared, &sender),
            Err(error) => break Err(error),
        }
    };
    if cancel_at_end || shared.closed.load(Ordering::Relaxed) {
        shared.ended.store(true, Ordering::Relaxed);
    }
    drop(sender);
    if worker.join().is_err() {
        eprintln!("open-pointcloud-studio mcp: the tool thread stopped unexpectedly");
    }
    result
}

enum Line {
    Message(Vec<u8>),
    TooLong,
    End,
}

/// Read up to the next line feed. A line longer than `limit` is skipped.
fn read_line<R: BufRead>(reader: &mut R, limit: usize) -> io::Result<Line> {
    let mut line = Vec::new();
    let mut too_long = false;
    loop {
        let available = match reader.fill_buf() {
            Ok(available) => available,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        if available.is_empty() {
            return Ok(if too_long {
                Line::TooLong
            } else if line.is_empty() {
                Line::End
            } else {
                Line::Message(line)
            });
        }
        let end = available.iter().position(|byte| *byte == b'\n');
        let part = &available[..end.unwrap_or(available.len())];
        if !too_long {
            if line.len() + part.len() > limit {
                too_long = true;
                line = Vec::new();
            } else {
                line.extend_from_slice(part);
            }
        }
        let used = end.map_or(available.len(), |end| end + 1);
        reader.consume(used);
        if end.is_some() {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Message(line)
            });
        }
    }
}

/// A message as JSON-RPC sees it.
enum Envelope<'a> {
    Request {
        id: &'a Value,
        method: &'a str,
        params: Option<&'a Value>,
    },
    Notification {
        method: &'a str,
        params: Option<&'a Value>,
    },
    /// An answer to a request; this server sends none, so it is dropped.
    Response,
    Invalid {
        id: Value,
        reason: &'static str,
    },
}

fn envelope(message: &Value) -> Envelope<'_> {
    let Some(fields) = message.as_object() else {
        return Envelope::Invalid {
            id: Value::Null,
            reason: "a message must be a JSON object",
        };
    };
    let id = fields.get("id");
    let valid_id = id.filter(|id| id.is_string() || id.is_number());
    let answer_id = valid_id.cloned().unwrap_or(Value::Null);
    if fields.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Envelope::Invalid {
            id: answer_id,
            reason: "jsonrpc must be \"2.0\"",
        };
    }
    let Some(method) = fields.get("method") else {
        if fields.contains_key("result") || fields.contains_key("error") {
            return Envelope::Response;
        }
        return Envelope::Invalid {
            id: answer_id,
            reason: "a request needs a method",
        };
    };
    let Some(method) = method.as_str() else {
        return Envelope::Invalid {
            id: answer_id,
            reason: "method must be a string",
        };
    };
    let params = fields.get("params");
    if params.is_some_and(|params| !params.is_object() && !params.is_array()) {
        return Envelope::Invalid {
            id: answer_id,
            reason: "params must be an object or an array",
        };
    }
    match (id, valid_id) {
        (None, _) => Envelope::Notification { method, params },
        (Some(_), Some(id)) => Envelope::Request { id, method, params },
        (Some(_), None) => Envelope::Invalid {
            id: Value::Null,
            reason: "id must be a string or a number",
        },
    }
}

fn is_tool_call(message: &Value) -> bool {
    matches!(
        envelope(message),
        Envelope::Request {
            method: "tools/call",
            ..
        }
    )
}

fn id_key(id: &Value) -> String {
    id.to_string()
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Handle one line of input on the reading thread.
fn receive(bytes: &[u8], shared: &Shared, work: &mpsc::Sender<Work>) {
    let text = bytes.trim_ascii();
    if text.is_empty() {
        return;
    }
    let message: Value = match serde_json::from_slice(text) {
        Ok(message) => message,
        Err(problem) => {
            shared.write(&error(
                Value::Null,
                PARSE_ERROR,
                &format!("Parse error: {problem}"),
            ));
            return;
        }
    };
    match message {
        Value::Array(messages) if messages.is_empty() => shared.write(&error(
            Value::Null,
            INVALID_REQUEST,
            "Invalid Request: an empty batch",
        )),
        Value::Array(messages) => {
            if messages.iter().any(is_tool_call) {
                for message in messages.iter().filter(|message| is_tool_call(message)) {
                    shared.forget(&id_key(&message["id"]));
                }
                let _ = work.send(Work::Batch(messages));
            } else {
                let answers: Vec<Value> = messages
                    .into_iter()
                    .filter_map(|message| answer(message, shared, None))
                    .collect();
                if !answers.is_empty() {
                    shared.write(&Value::Array(answers));
                }
            }
        }
        message if is_tool_call(&message) => {
            // An id the client used before starts without its old cancellation.
            shared.forget(&id_key(&message["id"]));
            let _ = work.send(Work::Call(message));
        }
        message => {
            if let Some(reply) = answer(message, shared, None) {
                shared.write(&reply);
            }
        }
    }
}

/// Carry out queued tool calls on the tool thread. A cancelled call gets
/// no answer, and nothing more is carried out once the input has ended.
fn carry_out(work: Work, link: &mut dyn Link, shared: &Shared) {
    match work {
        Work::Call(message) => {
            let key = id_key(&message["id"]);
            if !shared.is_cancelled(&key) {
                let reply = answer(message, shared, Some(link));
                if let (Some(reply), false) = (reply, shared.is_cancelled(&key)) {
                    shared.write(&reply);
                }
            }
            shared.forget(&key);
        }
        Work::Batch(messages) => {
            let mut replies = Vec::new();
            for message in messages {
                if shared.has_ended() {
                    return;
                }
                replies.extend(answer(message, shared, Some(&mut *link)));
            }
            if !replies.is_empty() && !shared.has_ended() {
                shared.write(&Value::Array(replies));
            }
        }
    }
}

/// The answer to one message, if it gets one. Tool calls need the link.
fn answer(message: Value, shared: &Shared, link: Option<&mut dyn Link>) -> Option<Value> {
    match envelope(&message) {
        Envelope::Response => None,
        Envelope::Invalid { id, reason } => Some(error(
            id,
            INVALID_REQUEST,
            &format!("Invalid Request: {reason}"),
        )),
        Envelope::Notification { method, params } => {
            if method == "notifications/cancelled" {
                if let Some(id) = params.and_then(|params| params.get("requestId")) {
                    lock(&shared.cancelled).insert(id_key(id));
                }
            }
            None
        }
        Envelope::Request { id, method, params } => {
            let result = match (method, link) {
                ("initialize", _) => initialize(params, shared),
                ("ping", _) => Ok(json!({})),
                ("tools/list", _) => list_tools(params),
                ("tools/call", Some(link)) => call_tool(params, link, shared, &id_key(id)),
                ("tools/call", None) => Err((
                    INTERNAL_ERROR,
                    "tool calls are carried out in order on the tool thread".to_owned(),
                )),
                _ => Err((METHOD_NOT_FOUND, format!("Method not found: {method}"))),
            };
            Some(match result {
                Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
                Err((code, message)) => error(id.clone(), code, &message),
            })
        }
    }
}

/// The version to speak: the client's when this server knows it, otherwise
/// the newest this server knows.
pub fn negotiate(requested: &str) -> &'static str {
    PROTOCOL_VERSIONS
        .iter()
        .copied()
        .find(|version| *version == requested)
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

fn initialize(params: Option<&Value>, shared: &Shared) -> Result<Value, Failure> {
    let requested = params
        .and_then(|params| params.get("protocolVersion"))
        .and_then(Value::as_str)
        .ok_or_else(|| {
            (
                INVALID_PARAMS,
                "initialize needs the protocolVersion of the client".to_owned(),
            )
        })?;
    let version = negotiate(requested);
    *lock(&shared.protocol) = Some(version);
    eprintln!("open-pointcloud-studio mcp: speaking protocol version {version}");
    Ok(json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "open-pointcloud-studio",
            "title": "Open Pointcloud Studio",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    }))
}

fn list_tools(params: Option<&Value>) -> Result<Value, Failure> {
    if params
        .and_then(|params| params.get("cursor"))
        .is_some_and(|cursor| !cursor.is_null())
    {
        return Err((
            INVALID_PARAMS,
            "unknown cursor: the list of tools has one page".to_owned(),
        ));
    }
    Ok(json!({
        "tools": tools::tools().iter().map(tools::Tool::listing).collect::<Vec<_>>(),
    }))
}

fn call_tool(
    params: Option<&Value>,
    link: &mut dyn Link,
    shared: &Shared,
    key: &str,
) -> Result<Value, Failure> {
    let Some(name) = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str)
    else {
        return Err((
            INVALID_PARAMS,
            "tools/call needs the name of a tool".to_owned(),
        ));
    };
    let arguments = params
        .and_then(|params| params.get("arguments"))
        .filter(|arguments| !arguments.is_null());
    if arguments.is_some_and(|arguments| !arguments.is_object()) {
        return Err((
            INVALID_PARAMS,
            format!("Invalid arguments for {name}: arguments must be an object"),
        ));
    }
    let cancelled = || shared.is_cancelled(key);
    let called = panic::catch_unwind(AssertUnwindSafe(|| {
        tools::call(name, arguments, link, &cancelled)
    }))
    .map_err(|_| {
        (
            INTERNAL_ERROR,
            format!("the tool {name} failed unexpectedly"),
        )
    })?;
    match called {
        Ok(outcome) => Ok(tool_result(outcome, shared.structured())),
        Err(CallError::UnknownTool) => Err((INVALID_PARAMS, format!("Unknown tool: {name}"))),
        Err(CallError::InvalidArguments(reason)) => {
            let message = format!("Invalid arguments for {name}: {reason}");
            if shared.invalid_arguments_as_result() {
                Ok(tool_result(Outcome::error(message), shared.structured()))
            } else {
                Err((INVALID_PARAMS, message))
            }
        }
    }
}

fn tool_result(outcome: Outcome, structured: bool) -> Value {
    let mut result = json!({"content": outcome.content, "isError": outcome.is_error});
    if structured && outcome.structured.is_object() {
        result["structuredContent"] = outcome.structured;
    }
    result
}

#[cfg(test)]
mod tests;
