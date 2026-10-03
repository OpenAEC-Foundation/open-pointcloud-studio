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
    assert!(listed.len() >= 69, "{} tools", listed.len());
    let mut names = std::collections::HashSet::new();
    for tool in &listed {
        let name = tool["name"].as_str().unwrap();
        assert!(names.insert(name), "{name} is listed once");
        assert!(
            name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
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
    assert!(commands.len() >= 60);
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
