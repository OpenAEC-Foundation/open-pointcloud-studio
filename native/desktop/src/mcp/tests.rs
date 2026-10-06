use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use super::instances::Connector;
use super::mock;
use super::tools::{self, Kind, Link};
use super::*;

/// Output of a session, kept for the test while the server writes it.
#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);

impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

type Answer = Box<dyn FnMut(&Value) -> Result<Value, String> + Send>;

/// A link that records the commands it is sent and answers through a
/// function of each command.
struct FakeLink {
    commands: Arc<Mutex<Vec<Value>>>,
    answer: Answer,
}

impl FakeLink {
    fn new(answer: impl FnMut(&Value) -> Result<Value, String> + Send + 'static) -> Self {
        Self {
            commands: Arc::default(),
            answer: Box::new(answer),
        }
    }

    fn ok() -> Self {
        Self::new(|_| Ok(json!({"ok": true})))
    }
}

impl Link for FakeLink {
    fn exec(&mut self, command: &Value) -> Result<Value, String> {
        self.commands.lock().unwrap().push(command.clone());
        (self.answer)(command)
    }

    fn list_instances(&mut self) -> Value {
        json!({"instances": [{"pid": 7, "port": 47_910}]})
    }

    fn select_instance(&mut self, pid: Option<u32>, port: Option<u16>) -> Result<Value, String> {
        Ok(json!({"ok": true, "pid": pid, "port": port}))
    }

    fn start_instance(&mut self, files: &[PathBuf]) -> Result<Value, String> {
        Ok(json!({"ok": true, "files": files}))
    }
}

fn request(id: u64, method: &str, params: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string()
}

fn call(id: u64, name: &str, arguments: Value) -> String {
    request(
        id,
        "tools/call",
        json!({"name": name, "arguments": arguments}),
    )
}

fn initialize(id: u64, version: &str) -> String {
    request(
        id,
        "initialize",
        json!({"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
    )
}

/// Feed lines to a server and return every line it wrote, parsed. The
/// calls are all answered before the session ends.
fn session_with_limit(input: &str, link: Box<dyn Link>, limit: usize) -> Vec<Value> {
    ended_session(input, link, limit, false)
}

fn ended_session(
    input: &str,
    link: Box<dyn Link>,
    limit: usize,
    cancel_at_end: bool,
) -> Vec<Value> {
    let output = Output::default();
    serve_with_limit(
        Cursor::new(input.as_bytes().to_vec()),
        Box::new(output.clone()),
        link,
        limit,
        cancel_at_end,
    )
    .unwrap();
    let bytes = output.0.lock().unwrap().clone();
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.is_empty() || text.ends_with('\n'),
        "every message ends its line"
    );
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn session(lines: &[String], link: impl Link + 'static) -> Vec<Value> {
    session_with_limit(
        &(lines.join("\n") + "\n"),
        Box::new(link),
        MAX_MESSAGE_BYTES,
    )
}

fn by_id(answers: &[Value], id: u64) -> Value {
    answers
        .iter()
        .find(|answer| answer["id"] == id)
        .unwrap_or_else(|| panic!("no answer to {id} in {answers:?}"))
        .clone()
}

fn text_of(result: &Value, place: usize) -> Value {
    serde_json::from_str(result["content"][place]["text"].as_str().unwrap()).unwrap()
}

#[test]
fn initialize_answers_with_the_clients_version_when_known() {
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            initialize(2, "2024-11-05"),
            initialize(3, "2099-01-01"),
            request(4, "initialize", json!({"capabilities": {}})),
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
            request(5, "ping", json!({})),
        ],
        FakeLink::ok(),
    );
    assert_eq!(answers.len(), 5, "the notification gets no answer");
    let first = by_id(&answers, 1);
    assert_eq!(first["jsonrpc"], "2.0");
    assert_eq!(first["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(
        first["result"]["serverInfo"]["name"],
        "open-pointcloud-studio"
    );
    assert_eq!(
        first["result"]["serverInfo"]["version"],
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(
        first["result"]["capabilities"]["tools"],
        json!({"listChanged": false})
    );
    assert_eq!(
        by_id(&answers, 2)["result"]["protocolVersion"],
        "2024-11-05"
    );
    assert_eq!(
        by_id(&answers, 3)["result"]["protocolVersion"],
        PROTOCOL_VERSIONS[0]
    );
    assert_eq!(by_id(&answers, 4)["error"]["code"], INVALID_PARAMS);
    assert_eq!(by_id(&answers, 5)["result"], json!({}));
    for version in PROTOCOL_VERSIONS {
        assert_eq!(negotiate(version), version);
    }
}

/// Arguments that fit a schema, built from its limits.
fn sample(schema: &Value) -> Value {
    if let Some(first) = schema["enum"].as_array().and_then(|values| values.first()) {
        return first.clone();
    }
    let kind = match &schema["type"] {
        Value::Array(kinds) => kinds[0].as_str().unwrap(),
        kind => kind.as_str().unwrap(),
    };
    match kind {
        "object" => {
            let mut fields = Map::new();
            for (name, property) in schema["properties"].as_object().unwrap() {
                fields.insert(name.clone(), sample(property));
            }
            Value::Object(fields)
        }
        "array" => {
            let count = schema["minItems"].as_u64().unwrap_or(1).max(1);
            Value::Array((0..count).map(|_| sample(&schema["items"])).collect())
        }
        "number" => json!(schema["minimum"].as_f64().unwrap_or(1.0)),
        "integer" => json!(schema["minimum"].as_u64().unwrap_or(0)),
        "boolean" => json!(true),
        "string" => {
            let length = schema["minLength"].as_u64().unwrap_or(1).max(1) as usize;
            json!(std::env::temp_dir()
                .join("x".repeat(length))
                .to_string_lossy()
                .into_owned())
        }
        other => panic!("unexpected type {other}"),
    }
}

#[test]
fn tools_list_gives_every_tool_a_closed_object_schema() {
    let answers = session(
        &[
            request(1, "tools/list", json!({})),
            request(2, "tools/list", json!({"cursor": "next"})),
        ],
        FakeLink::ok(),
    );
    let listed = by_id(&answers, 1)["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(by_id(&answers, 2)["error"]["code"], INVALID_PARAMS);
    assert!(listed.len() >= 75, "{} tools", listed.len());
    let mut names = std::collections::HashSet::new();
    for tool in &listed {
        let name = tool["name"].as_str().unwrap();
        assert!(names.insert(name), "{name} is listed once");
        assert!(
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "{name} is snake_case"
        );
        assert!(tool["description"].as_str().unwrap().len() > 20, "{name}");
        let input = &tool["inputSchema"];
        assert_eq!(input["type"], "object", "{name}");
        assert_eq!(input["additionalProperties"], false, "{name}");
        let properties = input["properties"].as_object().unwrap();
        for required in input["required"].as_array().into_iter().flatten() {
            assert!(
                properties.contains_key(required.as_str().unwrap()),
                "{name}"
            );
        }
        for (argument, property) in properties {
            assert!(
                property["type"].is_string() || property["type"].is_array(),
                "{name}.{argument}"
            );
            assert!(property["description"].is_string(), "{name}.{argument}");
        }
        // The schema accepts arguments built from it, and those arguments
        // are a command the command API understands.
        let arguments = sample(input);
        schema::validate_arguments(input, &arguments).unwrap();
        let tool = tools::find(name).unwrap();
        if matches!(tool.kind, Kind::Command | Kind::Job | Kind::Screenshot) {
            let mut fields = arguments.as_object().unwrap().clone();
            fields.remove("wait_seconds");
            let body = tools::command_body(name, fields);
            serde_json::from_value::<crate::native_api::ApiCommand>(body.clone())
                .unwrap_or_else(|error| panic!("{body} is no API command: {error}"));
        }
    }
    for name in [
        "status",
        "screenshot",
        "wait_for_job",
        "list_instances",
        "select_instance",
    ] {
        assert!(names.contains(name), "{name}");
    }
}

#[test]
fn tool_calls_go_to_the_command_of_the_same_name() {
    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "camera" => json!({"ok": true, "view": "TOP"}),
            "export" => json!({"ok": false, "error": "no active cloud"}),
            "zoom_all" => return Err("the window does not answer".into()),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "camera", json!({"preset": "top"})),
            call(3, "export", json!({"path": "/scans/out.laz"})),
            call(4, "zoom_all", json!({})),
            call(5, "clear_section", Value::Null),
            request(6, "tools/call", json!({"name": "status"})),
            call(7, "select_instance", json!({"port": 47_911})),
            call(
                8,
                "start_instance",
                json!({"files": [std::env::temp_dir().join("a.e57")]}),
            ),
        ],
        link,
    );
    let camera = by_id(&answers, 2)["result"].clone();
    assert_eq!(camera["isError"], false);
    assert_eq!(text_of(&camera, 0), json!({"ok": true, "view": "TOP"}));
    assert_eq!(camera["structuredContent"]["view"], "TOP");
    let export = by_id(&answers, 3)["result"].clone();
    assert_eq!(export["isError"], true, "a refused command is a tool error");
    assert_eq!(text_of(&export, 0)["error"], "no active cloud");
    let unreachable = by_id(&answers, 4)["result"].clone();
    assert_eq!(unreachable["isError"], true);
    assert_eq!(
        unreachable["content"][0]["text"],
        "the window does not answer"
    );
    assert_eq!(by_id(&answers, 5)["result"]["isError"], false);
    assert_eq!(by_id(&answers, 6)["result"]["isError"], false);
    assert_eq!(
        by_id(&answers, 7)["result"]["structuredContent"]["port"],
        47_911
    );
    assert_eq!(by_id(&answers, 8)["result"]["isError"], false);
    assert_eq!(
        commands.lock().unwrap().as_slice(),
        [
            json!({"command": "camera", "preset": "top"}),
            json!({"command": "export", "path": "/scans/out.laz"}),
            json!({"command": "zoom_all"}),
            json!({"command": "clear_section"}),
            json!({"command": "status"}),
        ]
    );
}

#[test]
fn older_protocol_versions_get_no_structured_content() {
    let answers = session(
        &[initialize(1, "2025-03-26"), call(2, "status", json!({}))],
        FakeLink::ok(),
    );
    let result = by_id(&answers, 2)["result"].clone();
    assert!(result.get("structuredContent").is_none());
    assert_eq!(text_of(&result, 0), json!({"ok": true}));
}

#[test]
fn malformed_messages_get_json_rpc_errors() {
    let answers = session(
        &[
            "{not json".to_owned(),
            "[]".to_owned(),
            "42".to_owned(),
            json!({"jsonrpc": "1.0", "id": 1, "method": "ping"}).to_string(),
            json!({"jsonrpc": "2.0", "id": 2}).to_string(),
            json!({"jsonrpc": "2.0", "id": 3, "method": 5}).to_string(),
            json!({"jsonrpc": "2.0", "id": {"nested": true}, "method": "ping"}).to_string(),
            json!({"jsonrpc": "2.0", "id": 4, "method": "ping", "params": "text"}).to_string(),
            request(5, "resources/list", json!({})),
            call(6, "no_such_tool", json!({})),
            call(
                7,
                "set_camera",
                json!({"yaw": 0.1, "pitch": 0.1, "zoom": 1}),
            ),
            call(8, "set_point_size", json!({"size": 50})),
            call(9, "camera", json!({"preset": "sideways"})),
            call(10, "status", json!({"verbose": true})),
            call(11, "status", json!([1])),
            request(12, "tools/call", json!({"arguments": {}})),
            call(13, "select_instance", json!({})),
            json!({"jsonrpc": "2.0", "method": "notifications/unknown"}).to_string(),
            json!({"jsonrpc": "2.0", "id": 99, "result": {}}).to_string(),
        ],
        FakeLink::ok(),
    );
    let codes: Vec<(Value, i64)> = answers
        .iter()
        .map(|answer| {
            (
                answer["id"].clone(),
                answer["error"]["code"].as_i64().unwrap(),
            )
        })
        .collect();
    let expected = [
        (Value::Null, PARSE_ERROR),
        (Value::Null, INVALID_REQUEST),
        (Value::Null, INVALID_REQUEST),
        (json!(1), INVALID_REQUEST),
        (json!(2), INVALID_REQUEST),
        (json!(3), INVALID_REQUEST),
        (Value::Null, INVALID_REQUEST),
        (json!(4), INVALID_REQUEST),
        (json!(5), METHOD_NOT_FOUND),
    ];
    assert_eq!(&codes[..expected.len()], expected);
    for id in 6..=13 {
        assert_eq!(by_id(&answers, id)["error"]["code"], INVALID_PARAMS, "{id}");
    }
    assert_eq!(
        answers.len(),
        17,
        "notifications and responses get no answer"
    );
    assert_eq!(
        by_id(&answers, 6)["error"]["message"],
        "Unknown tool: no_such_tool"
    );
    assert_eq!(
        by_id(&answers, 7)["error"]["message"],
        "Invalid arguments for set_camera: missing required argument pan"
    );
    assert_eq!(
        by_id(&answers, 8)["error"]["message"],
        "Invalid arguments for set_point_size: size must be at most 20"
    );
}

#[test]
fn from_2025_11_25_arguments_that_do_not_fit_are_a_tool_error() {
    let link = FakeLink::ok();
    let commands = Arc::clone(&link.commands);
    let answers = session(
        &[
            initialize(1, "2025-11-25"),
            call(2, "set_point_size", json!({"size": 50})),
            call(3, "select_instance", json!({})),
            call(4, "no_such_tool", json!({})),
            call(5, "status", json!([1])),
            call(
                6,
                "set_class_visible",
                json!({"code": 6.0, "visible": true}),
            ),
        ],
        link,
    );
    let refused = by_id(&answers, 2)["result"].clone();
    assert_eq!(refused["isError"], true);
    assert_eq!(
        refused["content"][0]["text"],
        "Invalid arguments for set_point_size: size must be at most 20"
    );
    let unnamed = by_id(&answers, 3)["result"].clone();
    assert_eq!(unnamed["isError"], true);
    assert_eq!(
        unnamed["content"][0]["text"],
        "Invalid arguments for select_instance: give at least 1 of the arguments pid, port"
    );
    // An unknown tool and arguments that are no object stay protocol errors.
    assert_eq!(by_id(&answers, 4)["error"]["code"], INVALID_PARAMS);
    assert_eq!(by_id(&answers, 5)["error"]["code"], INVALID_PARAMS);
    // A whole number written with a fraction is an integer.
    assert_eq!(by_id(&answers, 6)["result"]["isError"], false);
    assert_eq!(
        commands.lock().unwrap().as_slice(),
        [json!({"command": "set_class_visible", "code": 6, "visible": true})]
    );
}

#[test]
fn the_end_of_the_input_stops_waits_and_drops_queued_calls() {
    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "job" => json!({"ok": true, "job": {"state": "running"}}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let input = [
        initialize(1, "2025-06-18"),
        call(
            2,
            "wait_for_job",
            json!({"id": "forever", "timeout_seconds": 60}),
        ),
        call(3, "zoom_all", json!({})),
    ]
    .join("\n")
        + "\n";
    let started = Instant::now();
    let answers = ended_session(&input, Box::new(link), MAX_MESSAGE_BYTES, true);
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(answers.len(), 1, "only initialize is answered: {answers:?}");
    assert_eq!(
        by_id(&answers, 1)["result"]["protocolVersion"],
        "2025-06-18"
    );
    assert!(commands
        .lock()
        .unwrap()
        .iter()
        .all(|command| command["command"] == "job"));
}

#[test]
fn a_batch_is_answered_as_a_batch() {
    let batch = json!([
        {"jsonrpc": "2.0", "id": 1, "method": "ping"},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "status"}},
    ]);
    let pings = json!([
        {"jsonrpc": "2.0", "id": 3, "method": "ping"},
        {"jsonrpc": "2.0", "id": 4, "method": "nothing"},
    ]);
    let answers = session(&[batch.to_string(), pings.to_string()], FakeLink::ok());
    assert_eq!(answers.len(), 2);
    let mut ids: Vec<Vec<Value>> = answers
        .iter()
        .map(|answer| {
            answer
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].clone())
                .collect()
        })
        .collect();
    ids.sort_by_key(|ids| ids[0].as_u64());
    assert_eq!(ids, [vec![json!(1), json!(2)], vec![json!(3), json!(4)]]);
}

#[test]
fn large_messages_keep_one_message_per_line() {
    let padding = "x".repeat(3 * 1024 * 1024);
    let big_request = request(1, "ping", json!({"_meta": {"padding": padding}}));
    let png = "A".repeat(4 * 1024 * 1024);
    let image = png.clone();
    let link = FakeLink::new(move |_| {
        Ok(json!({"ok": true, "width": 2, "height": 1, "png_base64": image}))
    });
    let commands = Arc::clone(&link.commands);
    let input = format!(
        "{big_request}\r\n\r\n{}\n{}",
        call(2, "screenshot", json!({"max_edge": 640})),
        request(3, "ping", json!({}))
    );
    let answers = session_with_limit(&input, Box::new(link), MAX_MESSAGE_BYTES);
    assert_eq!(answers.len(), 3);
    assert_eq!(by_id(&answers, 1)["result"], json!({}));
    assert_eq!(by_id(&answers, 3)["result"], json!({}));
    let shot = by_id(&answers, 2)["result"].clone();
    assert_eq!(shot["isError"], false);
    assert_eq!(shot["content"][0]["type"], "image");
    assert_eq!(shot["content"][0]["mimeType"], "image/png");
    assert_eq!(
        shot["content"][0]["data"].as_str().unwrap().len(),
        png.len()
    );
    let details = text_of(&shot, 1);
    assert_eq!(details["width"], 2);
    assert!(details.get("png_base64").is_none());
    assert_eq!(
        commands.lock().unwrap().as_slice(),
        [json!({"command": "screenshot", "max_edge": 640, "base64": true})]
    );

    // A line over the limit is refused and the next one is still read.
    let input = format!(
        "{}\n{}\n",
        request(4, "ping", json!({"p": "y".repeat(500)})),
        request(5, "ping", json!({}))
    );
    let answers = session_with_limit(&input, Box::new(FakeLink::ok()), 200);
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0]["error"]["code"], PARSE_ERROR);
    assert_eq!(by_id(&answers, 5)["result"], json!({}));
}

#[test]
fn jobs_are_waited_for_and_waits_can_be_cancelled() {
    let polls = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&polls);
    let link = FakeLink::new(move |command| {
        Ok(match command["command"].as_str().unwrap() {
            "mesh" => json!({"ok": true, "accepted": true, "job_id": "j-1"}),
            "job" if command["id"] == "j-1" => {
                let mut count = counted.lock().unwrap();
                *count += 1;
                if *count < 3 {
                    json!({"ok": true, "job": {"state": "running", "stage": "reading"}})
                } else {
                    json!({"ok": true, "job": {"state": "complete", "path": "/m.obj"}})
                }
            }
            "job" if command["id"] == "j-2" => {
                json!({"ok": true, "job": {"state": "failed", "error": "disk full"}})
            }
            "job" => json!({"ok": true, "job": {"state": "running"}}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let started = Instant::now();
    let answers = session(
        &[
            call(1, "mesh", json!({"mode": "terrain", "path": "/m.obj", "wait_seconds": 30})),
            call(2, "wait_for_job", json!({"id": "j-2"})),
            call(3, "wait_for_job", json!({"id": "forever", "timeout_seconds": 60})),
            json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 3}}).to_string(),
            call(4, "wait_for_job", json!({"id": "forever", "timeout_seconds": 0.3})),
            request(5, "ping", json!({})),
        ],
        link,
    );
    assert!(started.elapsed() < Duration::from_secs(20));
    let mesh = by_id(&answers, 1)["result"].clone();
    assert_eq!(mesh["isError"], false);
    let mesh = text_of(&mesh, 0);
    assert_eq!(mesh["job_id"], "j-1");
    assert_eq!(mesh["job"]["state"], "complete");
    assert_eq!(mesh["timed_out"], false);
    let failed = by_id(&answers, 2)["result"].clone();
    assert_eq!(failed["isError"], true, "a failed job fails the call");
    assert!(
        answers.iter().all(|answer| answer["id"] != 3),
        "a cancelled call is not answered"
    );
    let timed_out = text_of(&by_id(&answers, 4)["result"], 0);
    assert_eq!(timed_out["timed_out"], true);
    assert_eq!(timed_out["job"]["state"], "running");
    let sent = commands.lock().unwrap().clone();
    assert_eq!(
        sent[0],
        json!({"command": "mesh", "mode": "terrain", "path": "/m.obj"})
    );
    assert_eq!(
        sent[1..4],
        vec![json!({"command": "job", "id": "j-1"}); 3][..]
    );
}

#[test]
fn wait_until_idle_follows_the_status() {
    let looks = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&looks);
    let link = FakeLink::new(move |_| {
        let mut count = counted.lock().unwrap();
        *count += 1;
        let busy = *count <= 2;
        Ok(json!({"ok": true, "result": {
            "imports": if busy { json!([{"id": 1}]) } else { json!([]) },
            "index_progress": null,
            "detail_pending": busy,
            "views": {"snapshots_pending": 0, "export_pending": false},
            "status": "Ready",
        }}))
    });
    let answers = session(
        &[call(1, "wait_until_idle", json!({"timeout_seconds": 20}))],
        link,
    );
    let idle = text_of(&by_id(&answers, 1)["result"], 0);
    assert_eq!(idle["idle"], true);
    assert_eq!(idle["busy"], json!([]));
    assert_eq!(
        *looks.lock().unwrap(),
        5,
        "two busy looks and three quiet ones"
    );
    assert_eq!(
        tools::busy(
            &json!({"imports": [1], "mesh": {}, "thin_pending": true, "photos_loading": 2, "views": {"snapshots_pending": 1}})
        ),
        ["imports", "mesh", "thin", "photos", "snapshots"]
    );
    // A download of buildings and the writing of a mesh file are work too.
    assert_eq!(
        tools::busy(&json!({"bag3d": {"page": 1}, "mesh_export_pending": true})),
        ["bag3d", "mesh_export"]
    );
    assert!(tools::busy(&json!({"bag3d": null, "mesh_export_pending": false})).is_empty());
}

#[test]
fn building_download_is_a_job_with_a_box_a_detail_level_and_a_destination() {
    let tool = tools::find("bag3d").unwrap();
    assert_eq!(tool.kind, Kind::Job);
    assert!(!tool.read_only());
    assert_eq!(tool.schema["required"], json!(["bbox", "lod", "path"]));
    assert!(tool.schema["properties"]["wait_seconds"].is_object());
    assert_eq!(
        tool.schema["properties"]["lod"]["enum"],
        json!(["1.2", "1.3", "2.2"])
    );
    let area = json!([121000, 487000, 121100, 487100]);
    let arguments = json!({"bbox": area, "lod": "2.2", "path": "/b.obj"});
    schema::validate_arguments(&tool.schema, &arguments).unwrap();
    for refused in [
        json!({"bbox": [121000, 487000, 121100], "lod": "2.2", "path": "/b.obj"}),
        json!({"bbox": area, "lod": "3", "path": "/b.obj"}),
        json!({"bbox": area, "lod": "2.2"}),
    ] {
        assert!(
            schema::validate_arguments(&tool.schema, &refused).is_err(),
            "{refused}"
        );
    }
    assert_eq!(tools::find("cancel_bag3d").unwrap().kind, Kind::Command);

    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "bag3d" => json!({"ok": true, "accepted": true, "job_id": "b-1"}),
            "job" => json!({"ok": true, "job": {"state": "complete", "buildings": 44}}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let mut waiting = arguments.clone();
    waiting["wait_seconds"] = json!(5);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "bag3d", waiting),
            call(3, "cancel_bag3d", json!({})),
        ],
        link,
    );
    let download = text_of(&by_id(&answers, 2)["result"], 0);
    assert_eq!(download["job"]["buildings"], 44);
    assert_eq!(download["timed_out"], false);
    assert_eq!(
        *commands.lock().unwrap(),
        [
            json!({"command": "bag3d", "bbox": area, "lod": "2.2", "path": "/b.obj"}),
            json!({"command": "job", "id": "b-1"}),
            json!({"command": "cancel_bag3d"}),
        ]
    );
}

#[test]
fn mesh_export_is_a_job_whose_destination_names_the_format() {
    let tool = tools::find("export_mesh").unwrap();
    assert_eq!(tool.kind, Kind::Job);
    assert!(!tool.read_only());
    assert_eq!(tool.schema["required"], json!(["path"]));
    assert!(tool.schema["properties"]["wait_seconds"].is_object());
    // The argument names every format the core writes.
    let destination = tool.schema["properties"]["path"]["description"]
        .as_str()
        .unwrap();
    for format in [
        pointcloud_core::MeshFormat::Obj,
        pointcloud_core::MeshFormat::Ply,
        pointcloud_core::MeshFormat::Stl,
    ] {
        assert!(
            destination.contains(&format!(".{}", format.extension())),
            "{format:?}"
        );
    }
    for refused in [
        json!({}),
        json!({"path": ""}),
        json!({"path": "/m.ply", "format": "ply"}),
    ] {
        assert!(
            schema::validate_arguments(&tool.schema, &refused).is_err(),
            "{refused}"
        );
    }
    // The job tool tells a caller that this command answers with a job.
    assert!(tools::find("job")
        .unwrap()
        .description
        .contains("export_mesh"));

    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "export_mesh" => json!({"ok": true, "accepted": true, "job_id": "x-1"}),
            "job" => json!({"ok": true, "job": {
                "state": "complete", "format": "stl", "triangles": 12, "origin": [207000, 474000, 0],
            }}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(
                2,
                "export_mesh",
                json!({"path": "/m.stl", "wait_seconds": 5}),
            ),
        ],
        link,
    );
    let export = text_of(&by_id(&answers, 2)["result"], 0);
    assert_eq!(export["job"]["format"], "stl");
    assert_eq!(export["job"]["origin"], json!([207000, 474000, 0]));
    assert_eq!(export["timed_out"], false);
    assert_eq!(
        *commands.lock().unwrap(),
        [
            json!({"command": "export_mesh", "path": "/m.stl"}),
            json!({"command": "job", "id": "x-1"}),
        ]
    );
}

#[test]
fn closed_mesh_is_a_mode_of_the_mesh_job_with_settings_of_its_own() {
    let mesh = tools::find("mesh").unwrap();
    let set = tools::find("set_closed_mesh_settings").unwrap();
    assert_eq!((mesh.kind, set.kind), (Kind::Job, Kind::Command));
    assert!(!mesh.read_only() && !set.read_only());
    // A closed mesh needs no file, so only the mode is required.
    assert_eq!(mesh.schema["required"], json!(["mode"]));
    assert!(set.schema.get("required").is_none());
    let fields = &mesh.schema["properties"];
    assert_eq!(
        fields["mode"]["enum"],
        json!(["terrain", "surface", "closed"])
    );
    let destination = fields["path"]["description"].as_str().unwrap();
    for extension in [".obj", ".ply", ".stl"] {
        assert!(destination.contains(extension), "{extension}");
    }
    // The settings are those of the core, with its limits, and both tools
    // take the same ones.
    assert_eq!(
        fields["sides"]["enum"],
        json!(["automatic", "centre", "upward"])
    );
    assert_eq!(fields["layers"]["enum"], json!(["active", "visible"]));
    assert_eq!(fields["voxel"]["type"], json!(["number", "null"]));
    assert_eq!(fields["voxel"]["minimum"], 0.005);
    assert_eq!(fields["voxel"]["maximum"], 0.5);
    assert_eq!(fields["max_hole"]["minimum"], 0.0);
    assert_eq!(
        fields["max_hole"]["maximum"],
        pointcloud_core::MAX_CLOSED_MESH_HOLE
    );
    assert_eq!(fields["simplify_mm"]["type"], json!(["number", "null"]));
    assert_eq!(fields["sample_percent"]["type"], "number");
    assert_eq!(
        fields["sample_percent"]["minimum"],
        pointcloud_core::MIN_CLOSED_MESH_SAMPLE_PERCENT
    );
    assert_eq!(fields["sample_percent"]["maximum"], 100.0);
    let mut shared = fields.as_object().unwrap().clone();
    for other in ["mode", "path", "wait_seconds"] {
        assert!(shared.remove(other).is_some(), "{other}");
    }
    assert_eq!(set.schema["properties"], Value::Object(shared));
    // The description names the limits a mesh has in the core.
    let limits = format!(
        "{} vertices or {} triangles",
        pointcloud_core::MAX_MESH_VERTICES,
        pointcloud_core::MAX_MESH_TRIANGLES
    );
    assert!(
        mesh.description.replace(',', "").contains(&limits),
        "{limits}"
    );

    let arguments = json!({
        "mode": "closed", "voxel": null, "max_hole": 0.3, "simplify_mm": 0,
        "sample_percent": 10, "sides": "centre", "layers": "visible",
    });
    schema::validate_arguments(&mesh.schema, &arguments).unwrap();
    schema::validate_arguments(&set.schema, &json!({})).unwrap();
    schema::validate_arguments(&set.schema, &json!({"simplify_mm": null})).unwrap();
    for refused in [
        json!({}),
        json!({"mode": "solid"}),
        json!({"mode": "closed", "voxel": 0.004}),
        json!({"mode": "closed", "voxel": 0.6}),
        json!({"mode": "closed", "voxel": "auto"}),
        json!({"mode": "closed", "max_hole": 3.3}),
        json!({"mode": "closed", "max_hole": null}),
        json!({"mode": "closed", "simplify_mm": -1}),
        json!({"mode": "closed", "sample_percent": 0}),
        json!({"mode": "closed", "sample_percent": 100.5}),
        json!({"mode": "closed", "sample_percent": null}),
        json!({"mode": "closed", "sides": "inward"}),
        json!({"mode": "closed", "layers": "all"}),
        json!({"mode": "closed", "selection_only": true}),
    ] {
        assert!(
            schema::validate_arguments(&mesh.schema, &refused).is_err(),
            "{refused}"
        );
    }
    assert!(schema::validate_arguments(&set.schema, &json!({"mode": "closed"})).is_err());

    // A closed mesh that is being made is work under way; its last result
    // is not.
    assert_eq!(
        tools::busy(&json!({"closed_mesh": {"job": {"stage": "planning"}, "last": null}})),
        ["closed_mesh"]
    );
    assert!(tools::busy(&json!({"closed_mesh": {
        "job": null, "last": {"state": "complete"},
    }}))
    .is_empty());

    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "mesh" => json!({"ok": true, "accepted": true, "job_id": "c-1"}),
            "job" => json!({"ok": true, "job": {
                "state": "complete", "mode": "closed", "open_edges": 0, "deviation_p95": 0.004,
            }}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let mut waiting = arguments.clone();
    waiting["wait_seconds"] = json!(5);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "set_closed_mesh_settings", json!({"voxel": 0.03})),
            call(3, "mesh", waiting),
            call(4, "cancel_mesh", json!({})),
        ],
        link,
    );
    let made = text_of(&by_id(&answers, 3)["result"], 0);
    assert_eq!(made["job"]["open_edges"], 0);
    assert_eq!(made["job"]["deviation_p95"], 0.004);
    assert_eq!(made["timed_out"], false);
    // A null goes out as a null: the window takes it for automatic.
    assert_eq!(
        *commands.lock().unwrap(),
        [
            json!({"command": "set_closed_mesh_settings", "voxel": 0.03}),
            json!({
                "command": "mesh", "mode": "closed", "voxel": null, "max_hole": 0.3,
                "simplify_mm": 0, "sample_percent": 10, "sides": "centre", "layers": "visible",
            }),
            json!({"command": "job", "id": "c-1"}),
            json!({"command": "cancel_mesh"}),
        ]
    );
    // The window reads that command as the tool sent it.
    let read: crate::native_api::ApiCommand = serde_json::from_value(json!({
        "command": "mesh", "mode": "closed", "voxel": null, "simplify_mm": 0, "sample_percent": 10,
    }))
    .unwrap();
    let crate::native_api::ApiCommand::Mesh {
        mode,
        path,
        options,
    } = read
    else {
        panic!("not a mesh command");
    };
    assert_eq!((mode.as_str(), path), ("closed", None));
    assert_eq!(options.voxel, Some(None), "null is automatic");
    assert_eq!(options.simplify_mm, Some(Some(0.0)));
    assert_eq!(options.sample_percent, Some(10.0));
    assert_eq!(options.max_hole, None, "left out keeps the block's value");
}

#[test]
fn surface_settings_are_optional_and_include_the_mesh_size() {
    let set = tools::find("set_surface_settings").unwrap();
    assert_eq!(set.kind, Kind::Command);
    // A field that is left out keeps its value in the window.
    assert!(set.schema.get("required").is_none());
    let fields = &set.schema["properties"];
    let mut names: Vec<&str> = fields
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["edge_factor", "max_vertices", "mesh_size", "neighbors"]
    );
    assert_eq!(fields["mesh_size"]["minimum"], 0.0);
    for accepted in [
        json!({}),
        json!({"mesh_size": 0.05}),
        json!({"max_vertices": 20000, "neighbors": 8, "edge_factor": 3.5, "mesh_size": 0}),
    ] {
        schema::validate_arguments(&set.schema, &accepted).unwrap();
    }
    for refused in [
        json!({"max_vertices": 2}),
        json!({"neighbors": 33}),
        json!({"edge_factor": 0}),
        json!({"mesh_size": -0.1}),
        json!({"sample_percent": 10}),
    ] {
        assert!(
            schema::validate_arguments(&set.schema, &refused).is_err(),
            "{refused}"
        );
    }
    // The window reads a command with one field as that field alone.
    let read: crate::native_api::ApiCommand = serde_json::from_value(json!({
        "command": "set_surface_settings", "mesh_size": 0.05,
    }))
    .unwrap();
    let crate::native_api::ApiCommand::SetSurfaceSettings {
        max_vertices,
        neighbors,
        edge_factor,
        mesh_size,
    } = read
    else {
        panic!("not a surface settings command");
    };
    assert_eq!((max_vertices, neighbors, edge_factor), (None, None, None));
    assert_eq!(mesh_size, Some(0.05));
}

#[test]
fn section_drawing_is_a_job_with_a_preview_and_a_cancel() {
    let export = tools::find("export_drawing").unwrap();
    let preview = tools::find("preview_drawing").unwrap();
    assert_eq!((export.kind, preview.kind), (Kind::Job, Kind::Job));
    assert!(!export.read_only() && !preview.read_only());
    assert_eq!(export.schema["required"], json!(["path"]));
    assert!(preview.schema.get("required").is_none());
    for tool in [export, preview] {
        assert!(tool.schema["properties"]["wait_seconds"].is_object());
    }
    // The destination names both formats the core writes.
    let destination = export.schema["properties"]["path"]["description"]
        .as_str()
        .unwrap();
    for format in pointcloud_core::DrawingFormat::ALL {
        assert!(
            destination.contains(&format!(".{}", format.extension())),
            "{format:?}"
        );
    }
    // The choices are those of the core, with its limits.
    let choices = &export.schema["properties"];
    assert_eq!(
        choices["view"]["enum"],
        json!(["plan", "front", "back", "left", "right"])
    );
    assert_eq!(choices["units"]["enum"], json!(["mm", "m"]));
    assert_eq!(choices["origin"]["enum"], json!(["model", "box"]));
    assert_eq!(choices["color"]["enum"], json!(["layer", "rgb"]));
    assert_eq!(choices["point_layers"]["enum"], json!(["scan", "class"]));
    assert_eq!(
        choices["version"]["enum"],
        json!(["r2004", "r2010", "r2013", "r2018"])
    );
    assert_eq!(
        choices["max_points"]["maximum"],
        pointcloud_core::MAX_DRAWING_POINTS
    );
    assert_eq!(
        choices["thickness"]["maximum"],
        pointcloud_core::MAX_SLAB_THICKNESS
    );
    assert_eq!(
        choices["max_wall_thickness"]["maximum"],
        pointcloud_core::MAX_WALL_THICKNESS
    );
    // No limit of its own: the largest wall is above zero and the grid has
    // no largest cell, as the command and the Properties block have it.
    assert_eq!(choices["max_wall_thickness"]["exclusiveMinimum"], 0);
    assert!(choices["max_wall_thickness"].get("minimum").is_none());
    assert_eq!(choices["grid"]["minimum"], pointcloud_core::MIN_CUT_GRID);
    assert!(choices["grid"].get("maximum").is_none());
    // A preview takes the same choices, without a destination.
    let mut without_path = choices.as_object().unwrap().clone();
    without_path.remove("path");
    assert_eq!(preview.schema["properties"], Value::Object(without_path));

    let arguments = json!({
        "path": "/d/plan.dwg", "view": "front", "thickness": 0.25, "units": "m",
        "fill": true, "square": false, "max_points": 20000.0, "version": "r2018",
    });
    schema::validate_arguments(&export.schema, &arguments).unwrap();
    schema::validate_arguments(&preview.schema, &json!({})).unwrap();
    // A wall thinner than the smallest wall the block starts with, and a
    // coarse grid.
    let thin_wall = json!({"path": "/d/plan.dxf", "max_wall_thickness": 0.03, "grid": 5.0});
    schema::validate_arguments(&export.schema, &thin_wall).unwrap();
    for refused in [
        json!({}),
        json!({"path": "/d/plan.dxf", "view": "top"}),
        json!({"path": "/d/plan.dxf", "thickness": -0.1}),
        json!({"path": "/d/plan.dxf", "thickness": 6}),
        json!({"path": "/d/plan.dxf", "max_wall_thickness": 0}),
        json!({"path": "/d/plan.dxf", "max_wall_thickness": 2.5}),
        json!({"path": "/d/plan.dxf", "grid": 0.004}),
        json!({"path": "/d/plan.dxf", "max_points": 400_001}),
        json!({"path": "/d/plan.dxf", "format": "dwg"}),
    ] {
        assert!(
            schema::validate_arguments(&export.schema, &refused).is_err(),
            "{refused}"
        );
    }
    assert!(schema::validate_arguments(&preview.schema, &json!({"path": "/d/plan.dxf"})).is_err());
    for name in ["clear_drawing_preview", "cancel_drawing"] {
        let tool = tools::find(name).unwrap();
        assert_eq!(tool.kind, Kind::Command);
        assert!(tool.schema["properties"].as_object().unwrap().is_empty());
    }
    // The job tool tells a caller that these commands answer with a job.
    let job = tools::find("job").unwrap().description;
    assert!(job.contains("export_drawing") && job.contains("preview_drawing"));

    // A drawing that is being made is work under way; its last result and
    // a preview on screen are not.
    assert_eq!(
        tools::busy(&json!({"drawing": {"job": {"stage": "reading"}, "last": null}})),
        ["drawing"]
    );
    assert!(tools::busy(&json!({"drawing": {
        "job": null, "last": {"state": "complete"}, "preview_shown": true,
    }}))
    .is_empty());

    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "export_drawing" => json!({"ok": true, "accepted": true, "job_id": "d-1"}),
            "preview_drawing" => json!({"ok": true, "accepted": true, "job_id": "d-2"}),
            "job" => json!({"ok": true, "job": {"state": "complete", "regions": 3}}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let mut waiting = arguments.clone();
    waiting["wait_seconds"] = json!(5);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "export_drawing", waiting),
            call(3, "preview_drawing", json!({"view": "plan"})),
            call(4, "clear_drawing_preview", json!({})),
            call(5, "cancel_drawing", json!({})),
        ],
        link,
    );
    let export = text_of(&by_id(&answers, 2)["result"], 0);
    assert_eq!(export["job"]["regions"], 3);
    assert_eq!(export["timed_out"], false);
    assert_eq!(
        text_of(&by_id(&answers, 3)["result"], 0)["job_id"],
        "d-2",
        "without wait_seconds the answer comes at once"
    );
    // The whole number of points goes out as an integer, as the command
    // reads it.
    assert_eq!(
        *commands.lock().unwrap(),
        [
            json!({
                "command": "export_drawing", "path": "/d/plan.dwg", "view": "front",
                "thickness": 0.25, "units": "m", "fill": true, "square": false,
                "max_points": 20000, "version": "r2018",
            }),
            json!({"command": "job", "id": "d-1"}),
            json!({"command": "preview_drawing", "view": "plan"}),
            json!({"command": "clear_drawing_preview"}),
            json!({"command": "cancel_drawing"}),
        ]
    );
}

#[test]
fn face_detection_is_a_job_with_a_list_a_highlight_an_export_and_a_clear() {
    let detect = tools::find("detect_faces").unwrap();
    let set = tools::find("set_face_settings").unwrap();
    let export = tools::find("export_faces").unwrap();
    assert_eq!((detect.kind, export.kind), (Kind::Job, Kind::Job));
    assert_eq!(set.kind, Kind::Command);
    for tool in [detect, export] {
        assert!(tool.schema["properties"]["wait_seconds"].is_object());
        assert!(!tool.read_only());
    }
    // Every setting may be left out, and has the limits the block states.
    assert!(detect.schema.get("required").is_none());
    let settings = &set.schema["properties"];
    use crate::faces::{
        MAX_ANGLE, MAX_FACE_AREA, MAX_TOLERANCE, MIN_ANGLE, MIN_FACE_AREA, MIN_TOLERANCE,
    };
    for (name, minimum, maximum) in [
        ("distance_tolerance", MIN_TOLERANCE, MAX_TOLERANCE),
        ("angle_tolerance", MIN_ANGLE, MAX_ANGLE),
        ("min_area", MIN_FACE_AREA, MAX_FACE_AREA),
    ] {
        assert_eq!(settings[name]["minimum"], minimum, "{name}");
        assert_eq!(settings[name]["maximum"], maximum, "{name}");
    }
    assert_eq!(settings["layers"]["enum"], json!(["active", "visible"]));
    assert_eq!(settings["color"]["enum"], json!(["face", "deviation"]));
    assert_eq!(settings["cylinders"]["type"], "boolean");
    // A detection takes the settings, and a wait.
    let mut with_wait = settings.as_object().unwrap().clone();
    with_wait.insert(
        "wait_seconds".into(),
        detect.schema["properties"]["wait_seconds"].clone(),
    );
    assert_eq!(detect.schema["properties"], Value::Object(with_wait));
    // The angle the core takes is the angle the tool takes.
    let core = |angle: f64| {
        pointcloud_core::surfaces::SurfaceDetectConfig {
            angle_tolerance_deg: angle,
            ..Default::default()
        }
        .validate()
        .is_ok()
    };
    assert!(core(MIN_ANGLE) && core(MAX_ANGLE));
    assert!(!core(MIN_ANGLE - 0.1) && !core(MAX_ANGLE + 0.1));

    let arguments = json!({
        "distance_tolerance": 0.015, "angle_tolerance": 8, "min_area": 0.5,
        "cylinders": false, "layers": "visible", "color": "deviation",
    });
    schema::validate_arguments(&detect.schema, &arguments).unwrap();
    schema::validate_arguments(&detect.schema, &json!({})).unwrap();
    for refused in [
        json!({"distance_tolerance": 0}),
        json!({"distance_tolerance": 20}),
        json!({"angle_tolerance": 60}),
        json!({"min_area": 0.001}),
        json!({"layers": "all"}),
        json!({"color": "residual"}),
        json!({"voxel": 0.01}),
    ] {
        assert!(
            schema::validate_arguments(&set.schema, &refused).is_err(),
            "{refused}"
        );
    }
    // The destination names both formats; the list only reads.
    assert_eq!(export.schema["required"], json!(["path"]));
    let destination = export.schema["properties"]["path"]["description"]
        .as_str()
        .unwrap();
    assert!(destination.contains(".json") && destination.contains(".obj"));
    let list = tools::find("list_faces").unwrap();
    assert!(list.read_only());
    schema::validate_arguments(&list.schema, &json!({})).unwrap();
    schema::validate_arguments(&list.schema, &json!({"boundaries": true})).unwrap();
    let select = tools::find("select_face").unwrap();
    for accepted in [json!({}), json!({"id": 3}), json!({"id": null})] {
        schema::validate_arguments(&select.schema, &accepted).unwrap();
    }
    for refused in [json!({"id": 0}), json!({"id": "wall"}), json!({"id": 1.5})] {
        assert!(
            schema::validate_arguments(&select.schema, &refused).is_err(),
            "{refused}"
        );
    }
    for name in ["cancel_detect_faces", "clear_faces"] {
        let tool = tools::find(name).unwrap();
        assert_eq!(tool.kind, Kind::Command);
        assert!(tool.schema["properties"].as_object().unwrap().is_empty());
    }
    // The job tool tells a caller that these commands answer with a job.
    let job = tools::find("job").unwrap().description;
    assert!(job.contains("detect_faces") && job.contains("export_faces"));

    // A detection under way and a faces file being written are work; the
    // last result and the faces a layer keeps are not.
    assert_eq!(
        tools::busy(&json!({"faces": {"job": {"stage": "reading"}, "export_pending": false}})),
        ["faces"]
    );
    assert_eq!(
        tools::busy(&json!({"faces": {"job": null, "export_pending": true}})),
        ["faces_export"]
    );
    // So is the fill of the cut while the 3D view makes it.
    assert_eq!(
        tools::busy(&json!({"section_fill": {"fill_cut": true, "pending": true}})),
        ["section_caps"]
    );
    assert!(tools::busy(&json!({"section_fill": {"fill_cut": true, "pending": false}})).is_empty());
    assert!(tools::busy(&json!({"faces": {
        "job": null, "last": {"state": "complete"}, "export_pending": false,
        "result": {"count": 7},
    }}))
    .is_empty());

    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "detect_faces" => json!({"ok": true, "accepted": true, "job_id": "f-1"}),
            "export_faces" => json!({"ok": true, "accepted": true, "job_id": "f-2"}),
            "job" => json!({"ok": true, "job": {"state": "complete", "count": 7}}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let mut waiting = arguments.clone();
    waiting["wait_seconds"] = json!(5);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "set_face_settings", json!({"min_area": 1})),
            call(3, "detect_faces", waiting),
            call(4, "list_faces", json!({"boundaries": true})),
            call(5, "select_face", json!({"id": 2.0})),
            call(6, "export_faces", json!({"path": "/f/faces.json"})),
            call(7, "cancel_detect_faces", json!({})),
            call(8, "clear_faces", json!({})),
        ],
        link,
    );
    let detected = text_of(&by_id(&answers, 3)["result"], 0);
    assert_eq!(detected["job"]["count"], 7);
    assert_eq!(detected["timed_out"], false);
    assert_eq!(
        text_of(&by_id(&answers, 6)["result"], 0)["job_id"],
        "f-2",
        "without wait_seconds the answer comes at once"
    );
    // The number of a face goes out as an integer, as the command reads it.
    assert_eq!(
        *commands.lock().unwrap(),
        [
            json!({"command": "set_face_settings", "min_area": 1}),
            json!({
                "command": "detect_faces", "distance_tolerance": 0.015, "angle_tolerance": 8,
                "min_area": 0.5, "cylinders": false, "layers": "visible", "color": "deviation",
            }),
            json!({"command": "job", "id": "f-1"}),
            json!({"command": "list_faces", "boundaries": true}),
            json!({"command": "select_face", "id": 2}),
            json!({"command": "export_faces", "path": "/f/faces.json"}),
            json!({"command": "cancel_detect_faces"}),
            json!({"command": "clear_faces"}),
        ]
    );
}

#[test]
fn colouring_from_photos_is_a_job_that_can_be_cancelled_and_cleared() {
    let colour = tools::find("colour_from_photos").unwrap();
    assert_eq!(colour.kind, Kind::Job);
    assert!(!colour.read_only());
    for accepted in [
        json!({}),
        json!({"layer": 1, "max_distance": 12.5, "blend": false, "wait_seconds": 30}),
    ] {
        schema::validate_arguments(&colour.schema, &accepted).unwrap();
    }
    for refused in [
        json!({"max_distance": 0.1}),
        json!({"max_distance": 501}),
        json!({"blend": "yes"}),
        json!({"layer": -1}),
    ] {
        assert!(
            schema::validate_arguments(&colour.schema, &refused).is_err(),
            "{refused}"
        );
    }
    assert_eq!(
        tools::find("cancel_colour_from_photos").unwrap().kind,
        Kind::Command
    );
    let clear = tools::find("clear_photo_colours").unwrap();
    schema::validate_arguments(&clear.schema, &json!({"layer": 0})).unwrap();
    assert!(tools::find("job")
        .unwrap()
        .description
        .contains("colour_from_photos"));
    // A colouring under way is work; how the last one ended is not.
    assert_eq!(
        tools::busy(&json!({"colour_from_photos": {"job": {"stage": "photos"}, "last": null}})),
        ["colour_from_photos"]
    );
    assert!(tools::busy(&json!({"colour_from_photos": {
        "job": null, "last": {"state": "complete"},
    }}))
    .is_empty());

    let link = FakeLink::new(|command| {
        Ok(match command["command"].as_str().unwrap() {
            "colour_from_photos" => json!({"ok": true, "accepted": true, "job_id": "c-1"}),
            "job" => json!({"ok": true, "job": {"state": "complete", "coloured": 9}}),
            _ => json!({"ok": true}),
        })
    });
    let commands = Arc::clone(&link.commands);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(
                2,
                "colour_from_photos",
                json!({"max_distance": 8, "wait_seconds": 5}),
            ),
            call(3, "cancel_colour_from_photos", json!({})),
            call(4, "clear_photo_colours", json!({"layer": 0})),
        ],
        link,
    );
    assert_eq!(
        text_of(&by_id(&answers, 2)["result"], 0)["job"]["coloured"],
        9
    );
    assert_eq!(
        *commands.lock().unwrap(),
        [
            json!({"command": "colour_from_photos", "max_distance": 8}),
            json!({"command": "job", "id": "c-1"}),
            json!({"command": "cancel_colour_from_photos"}),
            json!({"command": "clear_photo_colours", "layer": 0}),
        ]
    );
}

#[test]
fn extension_and_file_view_tools_offer_what_the_window_knows() {
    let list = tools::find("list_extensions").unwrap();
    assert_eq!(list.kind, Kind::Command);
    assert!(list.read_only());
    let switch = tools::find("set_extension_enabled").unwrap();
    assert!(!switch.read_only());
    assert_eq!(switch.schema["required"], json!(["id", "enabled"]));
    assert_eq!(
        switch.schema["properties"]["id"]["enum"],
        json!(crate::extensions::ids())
    );
    schema::validate_arguments(&switch.schema, &json!({"id": "bag3d", "enabled": false})).unwrap();
    assert!(
        schema::validate_arguments(&switch.schema, &json!({"id": "other", "enabled": false}))
            .is_err()
    );

    let view = tools::find("file_view").unwrap();
    assert_eq!(view.kind, Kind::Command);
    assert_eq!(view.schema["required"], json!(["open"]));
    assert_eq!(
        view.schema["properties"]["page"]["enum"],
        json!(crate::file_view::FilePage::ids())
    );
    schema::validate_arguments(&view.schema, &json!({"open": false})).unwrap();
    schema::validate_arguments(&view.schema, &json!({"open": true, "page": "extensions"})).unwrap();
    assert!(schema::validate_arguments(&view.schema, &json!({"page": "about"})).is_err());
    assert!(
        schema::validate_arguments(&view.schema, &json!({"open": true, "page": "settings"}))
            .is_err()
    );

    let link = FakeLink::ok();
    let commands = Arc::clone(&link.commands);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "file_view", json!({"open": true, "page": "extensions"})),
            call(
                3,
                "set_extension_enabled",
                json!({"id": "bag3d", "enabled": false}),
            ),
            call(4, "list_extensions", json!({})),
            call(5, "file_view", json!({"open": false})),
        ],
        link,
    );
    for id in 2..=5 {
        assert_eq!(by_id(&answers, id)["result"]["isError"], false, "{id}");
    }
    assert_eq!(
        *commands.lock().unwrap(),
        [
            json!({"command": "file_view", "open": true, "page": "extensions"}),
            json!({"command": "set_extension_enabled", "id": "bag3d", "enabled": false}),
            json!({"command": "list_extensions"}),
            json!({"command": "file_view", "open": false}),
        ]
    );
}

#[test]
fn language_tool_offers_the_languages_of_the_table_and_sends_the_choice() {
    let tool = tools::find("set_language").unwrap();
    assert_eq!(tool.kind, Kind::Command);
    assert!(!tool.read_only());
    assert_eq!(tool.schema["required"], json!(["language"]));
    assert_eq!(
        tool.schema["properties"]["language"]["enum"],
        json!(crate::i18n::Language::keys())
    );
    schema::validate_arguments(&tool.schema, &json!({"language": "nl"})).unwrap();
    assert!(schema::validate_arguments(&tool.schema, &json!({"language": "xx"})).is_err());

    let link = FakeLink::new(|command| Ok(json!({"ok": true, "language": command["language"]})));
    let commands = Arc::clone(&link.commands);
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "set_language", json!({"language": "nl"})),
        ],
        link,
    );
    assert_eq!(by_id(&answers, 2)["result"]["isError"], false);
    assert_eq!(
        *commands.lock().unwrap(),
        [json!({"command": "set_language", "language": "nl"})]
    );
}

#[test]
fn new_windows_start_from_the_appimage_when_there_is_one() {
    let directory = tempfile::tempdir().unwrap();
    let appimage = directory.path().join("Open-Pointcloud-Studio.AppImage");
    std::fs::write(&appimage, b"image").unwrap();
    let executable = PathBuf::from("/tmp/.mount_abc/usr/bin/open-pointcloud-studio");
    let program = |appimage: Option<&Path>, appdir: Option<&str>, mounted: bool| {
        window_program(
            appimage.map(Into::into),
            appdir.map(Into::into),
            |_| mounted,
            Ok(executable.clone()),
        )
        .unwrap()
    };

    assert_eq!(program(None, None, false), executable);
    assert_eq!(
        program(Some(&appimage), Some("/tmp/.mount_abc"), true),
        appimage
    );
    // A client that itself runs from an AppImage hands its variables on to
    // an installed server: they name an existing file that is another
    // program, and a folder this executable does not lie in.
    for inherited in [
        None,
        Some("/tmp/.mount_other"),
        Some("/tmp/.mount_ab"),
        Some(""),
    ] {
        assert_eq!(program(Some(&appimage), inherited, true), executable);
    }
    // An unpacked AppImage is a plain folder: the file would start the
    // application as a child of the process this server waits for.
    assert_eq!(
        program(Some(&appimage), Some("/tmp/.mount_abc"), false),
        executable
    );
    // The variable may name a file that is gone, or nothing.
    let gone = directory.path().join("gone.AppImage");
    for stale in [gone.as_path(), Path::new("")] {
        assert_eq!(
            program(Some(stale), Some("/tmp/.mount_abc"), true),
            executable
        );
    }
    // The failure to find this program is reported, whatever the variables say.
    for appimage in [None, Some(appimage.clone().into())] {
        let missing = window_program(
            appimage,
            Some("/tmp/.mount_abc".into()),
            |_| true,
            Err(io::Error::other("no executable")),
        );
        assert!(missing.is_err());
    }
}

#[cfg(unix)]
#[test]
fn the_folder_of_an_appimage_is_recognised_through_a_link_and_as_a_mount() {
    let directory = tempfile::tempdir().unwrap();
    let appimage = directory.path().join("Open-Pointcloud-Studio.AppImage");
    std::fs::write(&appimage, b"image").unwrap();
    let folder = directory.path().join("mount");
    std::fs::create_dir_all(folder.join("usr/bin")).unwrap();
    let link = directory.path().join("link");
    std::os::unix::fs::symlink(&folder, &link).unwrap();
    // The path of the running executable is resolved; the announced folder
    // need not be.
    let executable = std::fs::canonicalize(&folder)
        .unwrap()
        .join("usr/bin/open-pointcloud-studio");
    assert_eq!(
        window_program(
            Some(appimage.clone().into()),
            Some(link.into()),
            |_| true,
            Ok(executable),
        )
        .unwrap(),
        appimage
    );

    // A folder made in another folder lies on the same device as it.
    assert!(!is_mount_point(&folder));
    assert!(!is_mount_point(&directory.path().join("missing")));
    assert!(!is_mount_point(Path::new("/")));
    #[cfg(target_os = "linux")]
    assert!(is_mount_point(Path::new("/proc")));
}

/// The command names in the first column of a Markdown table.
fn table_names(document: &str) -> Vec<&str> {
    document
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|rest| rest.split_once('`'))
        .map(|(name, _)| name)
        .collect()
}

#[test]
fn every_api_command_has_a_tool_and_every_tool_is_documented() {
    let commands = table_names(include_str!("../../../API.md"));
    assert!(commands.len() >= 70);
    for command in commands {
        let tool = tools::find(command).unwrap_or_else(|| panic!("no tool for {command}"));
        assert!(matches!(
            tool.kind,
            Kind::Command | Kind::Job | Kind::Screenshot
        ));
    }
    let documented = table_names(include_str!("../../../MCP.md"));
    let listed: Vec<&str> = tools::tools().iter().map(|tool| tool.name).collect();
    assert_eq!(documented, listed, "MCP.md lists the tools in table order");
}

fn write_discovery(directory: &Path, pid: u32, port: u16, token: &str, started: u64) {
    std::fs::write(
        directory.join(format!("instance-{port}.json")),
        json!({"pid": pid, "port": port, "token": token, "api": "native-rust-v1", "started": started})
            .to_string(),
    )
    .unwrap();
}

/// The server against a stand-in command API on a real port: the bodies it
/// sends over HTTP are exactly the commands of the API.
#[test]
fn end_to_end_against_a_mock_command_api() {
    let directory = tempfile::tempdir().unwrap();
    let me = std::process::id();
    let job_looks = Arc::new(Mutex::new(0));
    let counted = Arc::clone(&job_looks);
    let api = mock::answering(me, move |command| match command["command"].as_str() {
        Some("screenshot") => {
            json!({"ok": true, "width": 1, "height": 1, "bytes": 3, "png_base64": "iVBORw=="})
        }
        Some("export") => json!({"ok": true, "accepted": true, "job_id": "e-1"}),
        Some("job") => {
            let mut looks = counted.lock().unwrap();
            *looks += 1;
            let state = if *looks < 2 { "running" } else { "complete" };
            json!({"ok": true, "job": {"state": state, "points": 12}})
        }
        _ => json!({"ok": true}),
    });
    let other = mock::mock_api(me);
    write_discovery(directory.path(), me, api.port, "secret-a", 10);
    // A newer window that the client does not choose.
    write_discovery(directory.path(), me, other.port, "secret-b", 20);
    let link = Connector::new(directory.path().into(), PathBuf::from("unused"), false).unwrap();
    let answers = session(
        &[
            initialize(1, "2025-06-18"),
            call(2, "list_instances", json!({})),
            call(3, "select_instance", json!({"port": api.port})),
            call(
                4,
                "set_camera",
                json!({"yaw": 0.5, "pitch": 0.25, "zoom": 0.1, "pan": [10, -20]}),
            ),
            call(5, "screenshot", json!({"max_edge": 800})),
            call(
                6,
                "export",
                json!({"path": "/scans/out.laz", "wait_seconds": 10}),
            ),
            call(7, "save_camera_view", json!({"name": "Front door"})),
            call(8, "set_annotation_tool", json!({"tool": null})),
        ],
        link,
    );
    let listed = text_of(&by_id(&answers, 2)["result"], 0);
    let ports: Vec<Value> = listed["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|instance| instance["port"].clone())
        .collect();
    assert_eq!(ports, [json!(other.port), json!(api.port)], "newest first");
    assert_eq!(by_id(&answers, 3)["result"]["isError"], false);
    for id in 4..=8 {
        assert_eq!(by_id(&answers, id)["result"]["isError"], false, "{id}");
    }
    let shot = by_id(&answers, 5)["result"].clone();
    assert_eq!(
        shot["content"][0],
        json!({"type": "image", "data": "iVBORw==", "mimeType": "image/png"})
    );
    let export = text_of(&by_id(&answers, 6)["result"], 0);
    assert_eq!(export["job"], json!({"state": "complete", "points": 12}));

    assert_eq!(
        api.bodies(),
        [
            r#"{"command":"set_camera","pan":[10,-20],"pitch":0.25,"yaw":0.5,"zoom":0.1}"#,
            r#"{"base64":true,"command":"screenshot","max_edge":800}"#,
            r#"{"command":"export","path":"/scans/out.laz"}"#,
            r#"{"command":"job","id":"e-1"}"#,
            r#"{"command":"job","id":"e-1"}"#,
            r#"{"command":"save_camera_view","name":"Front door"}"#,
            r#"{"command":"set_annotation_tool","tool":null}"#,
        ]
    );
    assert!(api
        .bodies
        .lock()
        .unwrap()
        .iter()
        .all(|(_, token)| token.as_deref() == Some("secret-a")));
    assert!(other.bodies().is_empty(), "the other window is not driven");
}
