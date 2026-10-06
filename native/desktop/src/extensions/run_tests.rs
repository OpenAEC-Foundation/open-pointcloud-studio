//! Tests that run the programs of extensions against the local API of a
//! window, as a click on their button does.

use std::time::{Duration, Instant};

use tokio::sync::mpsc::UnboundedReceiver;

use super::tests::{example_copy, example_folder, install_confirmed, send, Bench, EXAMPLE};
use super::*;
use crate::i18n::{Language, TestLanguage};
use crate::native_api::{ApiCommand, Delivery};

/// Start the local API for a window, as the application does.
fn with_api(studio: &mut Studio) -> UnboundedReceiver<Delivery> {
    let (receiver, handle) = crate::native_api::start(Some(0)).unwrap();
    studio.api_handle = Some(handle);
    receiver
}

/// What `drive` saw of a run: how it ended and the last progress it
/// reported.
struct Driven {
    end: RunEnd,
    progress: Option<(f64, String)>,
    log: String,
}

/// Serve the requests of a run and wait for its end, as the window does.
fn drive(studio: &mut Studio, receiver: &mut UnboundedReceiver<Delivery>, id: &str) -> Driven {
    let run = &studio.extension_host.runs[id];
    let control = Arc::clone(&run.control);
    let number = run.number;
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut progress = None;
    loop {
        let ended = control.try_end();
        // Requests sent before the end are served, as the program waited
        // for their answers.
        while let Ok(delivery) = receiver.try_recv() {
            let message = match delivery.caller {
                None => Message::ApiRequest(delivery.request),
                Some(caller) => {
                    Message::Extension(ExtensionAction::Api(caller, Box::new(delivery.request)))
                }
            };
            let _ = studio.update(message);
            if let Some(reported) = studio
                .extension_host
                .runs
                .get(id)
                .and_then(|run| run.progress.clone())
            {
                progress = Some(reported);
            }
        }
        if let Some(end) = ended {
            let _ = studio.update(Message::Extension(ExtensionAction::Ended(
                id.to_owned(),
                number,
                end.clone(),
            )));
            let log = fs::read_to_string(&control.log_path).unwrap_or_default();
            return Driven { end, progress, log };
        }
        assert!(
            Instant::now() < deadline,
            "the run did not end; its log:\n{}",
            fs::read_to_string(&control.log_path).unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Install an extension whose program is a script of the shell of the
/// system: PowerShell on Windows, sh elsewhere. Its command passes `--one`
/// and its button `two words`.
fn script_extension(studio: &mut Studio, bench: &Bench, id: &str, powershell: &str, sh: &str) {
    let source = bench.directory.path().join(format!("source-{id}"));
    fs::create_dir_all(&source).unwrap();
    let (interpreter, program, script) = if cfg!(windows) {
        ("powershell", "run.ps1", powershell)
    } else {
        ("sh", "run.sh", sh)
    };
    fs::write(source.join(program), script).unwrap();
    fs::write(
        source.join("icon.svg"),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\"><circle cx=\"12\" cy=\"12\" r=\"6\" fill=\"#D97706\"/></svg>",
    )
    .unwrap();
    let manifest = json!({
        "id": id,
        "name": "Script",
        "version": "1.0.0",
        "author": "Example",
        "description": "Runs a script.",
        "min_app_version": "0.1.0",
        "command": {"interpreter": interpreter, "program": program, "args": ["--one"]},
        "uses": {"network": false, "files_outside_folder": false, "commands": ["status"]},
        "contributes": {
            "ribbon": [{"id": "go", "label": "Go", "icon": "icon.svg", "args": ["two words"]}],
        },
    });
    fs::write(
        source.join(manifest::MANIFEST),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    install_confirmed(studio, &source);
    assert!(
        studio.extension_host.find(id).is_some(),
        "{}",
        studio.status
    );
}

const ENVIRONMENT_PS1: &str = r#"$lines = @(
    "port=$env:OPS_API_PORT",
    "token=$env:OPS_API_TOKEN",
    "id=$env:OPS_EXTENSION_ID",
    "context=$env:OPS_CONTEXT",
    "url=$env:OPS_API_URL",
    "cwd=$((Get-Location).Path)",
    "args=$($args -join '|')"
)
[System.IO.File]::WriteAllLines((Join-Path (Get-Location).Path 'env.txt'), [string[]]$lines)
Write-Output 'hello from the run'
"#;

const ENVIRONMENT_SH: &str = r#"IFS='|'
printf 'port=%s\ntoken=%s\nid=%s\ncontext=%s\nurl=%s\ncwd=%s\nargs=%s\n' "$OPS_API_PORT" "$OPS_API_TOKEN" "$OPS_EXTENSION_ID" "$OPS_CONTEXT" "$OPS_API_URL" "$(pwd -P)" "$*" > env.txt
echo 'hello from the run'
"#;

#[test]
fn a_run_gets_the_api_its_context_its_arguments_and_a_log() {
    let _language = TestLanguage::hold(Language::English);
    // The window and its server end before another test starts one.
    let _server = crate::native_api::one_server_at_a_time();
    let bench = Bench::new();
    let mut studio = bench.studio();
    let mut receiver = with_api(&mut studio);
    let port = studio.api_handle.as_ref().unwrap().port;
    let id = "org.example.environment";
    script_extension(&mut studio, &bench, id, ENVIRONMENT_PS1, ENVIRONMENT_SH);

    let _ = studio.update(Message::Extension(ExtensionAction::Press(
        id.into(),
        Some("go".into()),
    )));
    assert_eq!(studio.status, "Script runs…");
    let token = studio.extension_host.runs[id].token.clone();
    let running = send(&mut studio, ApiCommand::Status)["result"]["extensions"]["running"].clone();
    assert_eq!(running[0]["id"], id);
    assert_eq!(running[0]["entry"], "go");
    assert_eq!(running[0]["stopping"], false);
    // The window draws the run in the ribbon, the page and the status bar.
    assert!(studio.extension_runs_status().is_some());
    let _ = studio.view();
    // A second start of the same extension is refused while it runs.
    assert_eq!(
        studio.start_extension(id, None).err().unwrap(),
        "Script runs already"
    );

    let driven = drive(&mut studio, &mut receiver, id);
    assert!(driven.end.succeeded(), "{:?}\n{}", driven.end, driven.log);
    assert_eq!(studio.status, "Script finished");
    assert!(studio.extension_host.runs.is_empty());
    assert!(studio.extension_runs_status().is_none());

    let folder = bench.root.join(id);
    let written = fs::read_to_string(folder.join("env.txt")).unwrap();
    let values: BTreeMap<&str, &str> = written
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    assert_eq!(values["port"], port.to_string());
    assert_eq!(values["url"], format!("http://127.0.0.1:{port}"));
    assert_eq!(values["token"], token);
    assert_eq!(values["id"], id);
    assert_eq!(values["args"], "--one|two words");
    assert_eq!(
        fs::canonicalize(values["cwd"]).unwrap(),
        fs::canonicalize(&folder).unwrap()
    );
    let context: Value = serde_json::from_slice(&fs::read(values["context"]).unwrap()).unwrap();
    assert_eq!(context["extension"]["id"], id);
    assert_eq!(context["extension"]["version"], "1.0.0");
    assert_eq!(context["entry"], "go");
    assert_eq!(context["scans"], 0);
    assert_eq!(
        Path::new(values["context"]).parent().unwrap(),
        folder.join("logs")
    );
    assert!(driven.log.contains("hello from the run"), "{}", driven.log);
    assert!(
        driven.log.contains("# Ended with exit code 0"),
        "{}",
        driven.log
    );

    // The token of the run stopped working when the run ended.
    let refused = reqwest::blocking::Client::new()
        .post(format!("http://127.0.0.1:{port}/exec"))
        .header("X-OPS-Token", token)
        .body(json!({"command": "status"}).to_string())
        .send()
        .unwrap();
    assert_eq!(refused.status(), 403);
}

#[test]
fn a_run_can_be_stopped_and_a_failure_shows_its_code_and_error() {
    let _language = TestLanguage::hold(Language::English);
    // The window and its server end before another test starts one.
    let _server = crate::native_api::one_server_at_a_time();
    let bench = Bench::new();
    let mut studio = bench.studio();
    let mut receiver = with_api(&mut studio);

    let id = "org.example.sleep";
    script_extension(
        &mut studio,
        &bench,
        id,
        "Write-Output 'sleeping'\nStart-Sleep -Seconds 120\n",
        "echo sleeping\nsleep 120\n",
    );
    let _ = studio.update(Message::Extension(ExtensionAction::Press(
        id.into(),
        Some("go".into()),
    )));
    // Wait until the program runs, then stop it with its button.
    let log = studio.extension_host.runs[id].control.log_path.clone();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("sleeping")
    {
        assert!(Instant::now() < deadline, "the script did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let started = Instant::now();
    let _ = studio.update(Message::Extension(ExtensionAction::Press(
        id.into(),
        Some("go".into()),
    )));
    assert_eq!(studio.status, "Stopping Script…");
    assert_eq!(
        send(&mut studio, ApiCommand::Status)["result"]["extensions"]["running"][0]["stopping"],
        true
    );
    let driven = drive(&mut studio, &mut receiver, id);
    assert!(driven.end.stopped, "{:?}", driven.end);
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(studio.status, "Script stopped");
    assert!(driven.log.contains("# Stopped"), "{}", driven.log);

    // Through the local API as well.
    let started = send(
        &mut studio,
        ApiCommand::RunExtension {
            id: id.into(),
            entry: None,
        },
    );
    assert_eq!(started["ok"], true, "{started}");
    assert!(PathBuf::from(started["log"].as_str().unwrap()).is_file());
    let stopping = send(&mut studio, ApiCommand::StopExtension { id: id.into() });
    assert_eq!(stopping, json!({"ok": true, "id": id, "stopping": true}));
    assert!(drive(&mut studio, &mut receiver, id).end.stopped);

    let id = "org.example.failure";
    script_extension(
        &mut studio,
        &bench,
        id,
        "Write-Output 'working'\n[Console]::Error.WriteLine('first line')\n[Console]::Error.WriteLine('something broke')\nexit 3\n",
        "echo working\necho 'first line' >&2\necho 'something broke' >&2\nexit 3\n",
    );
    let _ = studio.update(Message::Extension(ExtensionAction::Press(id.into(), None)));
    let driven = drive(&mut studio, &mut receiver, id);
    assert_eq!(driven.end.code, Some(3));
    assert!(!driven.end.succeeded());
    assert_eq!(
        studio.status,
        "Script failed with exit code 3: first line · something broke"
    );
    assert!(driven.log.contains("working"), "{}", driven.log);
    assert!(driven.log.contains("something broke"), "{}", driven.log);
    assert!(
        driven.log.contains("# Ended with exit code 3"),
        "{}",
        driven.log
    );

    // Switching an extension off stops its run.
    let id = "org.example.sleep";
    let _ = studio.update(Message::Extension(ExtensionAction::Press(id.into(), None)));
    let _ = studio.update(Message::Extension(ExtensionAction::SetEnabled(
        id.into(),
        false,
    )));
    assert!(drive(&mut studio, &mut receiver, id).end.stopped);

    // The logs of the newest runs are kept.
    let logs: Vec<_> = fs::read_dir(bench.root.join("org.example.sleep").join("logs"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".log"))
        .collect();
    assert_eq!(logs.len(), 3);
}

/// A program that a script starts and that writes a file a moment later,
/// on its own: a hidden Windows PowerShell on Windows, a subshell that
/// ignores `SIGTERM` elsewhere.
fn starts_a_program(file: &str, then_ps1: &str, then_sh: &str) -> (String, String) {
    let powershell = format!(
        "$target = Join-Path (Get-Location).Path '{file}'\n\
         Start-Process -FilePath \"$env:SystemRoot\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" -WindowStyle Hidden -ArgumentList \"-NoProfile -NonInteractive -Command `\"Start-Sleep -Seconds 4; Set-Content -LiteralPath '$target' -Value alive`\"\"\n\
         Write-Output 'started'\n\
         {then_ps1}"
    );
    let sh = format!(
        "( trap '' TERM; sleep 4; echo alive > {file} ) >/dev/null 2>&1 &\n\
         echo started\n\
         {then_sh}"
    );
    (powershell, sh)
}

/// Wait until a file is there, or until the time is up; whether it came.
fn appears(path: &Path, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if path.is_file() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    path.is_file()
}

#[test]
fn what_a_run_started_stays_when_it_ends_and_is_ended_when_it_is_stopped() {
    let _language = TestLanguage::hold(Language::English);
    // The window and its server end before another test starts one.
    let _server = crate::native_api::one_server_at_a_time();
    let bench = Bench::new();
    let mut studio = bench.studio();
    let mut receiver = with_api(&mut studio);

    // A run that ends by itself leaves what it started running, as a report
    // it opened in another program; also once nothing holds the run.
    let id = "org.example.starter";
    let (powershell, sh) = starts_a_program("survivor.txt", "exit 0", "exit 0");
    script_extension(&mut studio, &bench, id, &powershell, &sh);
    let _ = studio.update(Message::Extension(ExtensionAction::Press(id.into(), None)));
    let driven = drive(&mut studio, &mut receiver, id);
    assert!(driven.end.succeeded(), "{:?}\n{}", driven.end, driven.log);
    assert!(driven.log.contains("started"), "{}", driven.log);
    assert!(studio.extension_host.runs.is_empty());
    assert!(
        appears(
            &bench.root.join(id).join("survivor.txt"),
            Duration::from_secs(30)
        ),
        "the program the run started was ended with it"
    );

    // A stopped run ends what it started, also what outlives its own
    // process by ignoring the request to end.
    let id = "org.example.holder";
    let (powershell, sh) = starts_a_program("stopped.txt", "Start-Sleep -Seconds 120", "sleep 120");
    script_extension(&mut studio, &bench, id, &powershell, &sh);
    let _ = studio.update(Message::Extension(ExtensionAction::Press(id.into(), None)));
    let log = studio.extension_host.runs[id].control.log_path.clone();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !fs::read_to_string(&log)
        .unwrap_or_default()
        .contains("started")
    {
        assert!(Instant::now() < deadline, "the script did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = studio.update(Message::Extension(ExtensionAction::Stop(id.into())));
    let driven = drive(&mut studio, &mut receiver, id);
    assert!(driven.end.stopped, "{:?}", driven.end);
    assert_eq!(studio.status, "Script stopped");
    assert!(
        !appears(
            &bench.root.join(id).join("stopped.txt"),
            Duration::from_secs(8)
        ),
        "the program the stopped run started went on"
    );
}

#[test]
fn closing_the_window_ends_the_runs() {
    let _language = TestLanguage::hold(Language::English);
    // The window and its server end before another test starts one.
    let _server = crate::native_api::one_server_at_a_time();
    let bench = Bench::new();
    let mut studio = bench.studio();
    let _receiver = with_api(&mut studio);
    let id = "org.example.sleep";
    script_extension(
        &mut studio,
        &bench,
        id,
        "Start-Sleep -Seconds 120\n",
        "sleep 120\n",
    );
    let _ = studio.update(Message::Extension(ExtensionAction::Press(id.into(), None)));
    let control = Arc::clone(&studio.extension_host.runs[id].control);
    let _ = studio.update(Message::Exit);
    let deadline = Instant::now() + Duration::from_secs(20);
    let end = loop {
        if let Some(end) = control.try_end() {
            break end;
        }
        assert!(Instant::now() < deadline, "the run went on");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(end.stopped);
}

/// Whether an interpreter is installed and works: it runs a script that
/// prints 42 the way a run does.
fn interpreter_works(interpreter: manifest::Interpreter) -> bool {
    let directory = tempfile::tempdir().unwrap();
    let (name, script) = match interpreter {
        manifest::Interpreter::Python => ("probe.py", "print(6 * 7)\n"),
        manifest::Interpreter::PowerShell => ("probe.ps1", "Write-Output (6 * 7)\n"),
        _ => return false,
    };
    fs::write(directory.path().join(name), script).unwrap();
    let launch = manifest::Launch {
        interpreter: Some(interpreter),
        program: name.into(),
        args: Vec::new(),
    };
    let Ok((program, arguments)) = run::command_line(&launch, directory.path(), &[]) else {
        return false;
    };
    std::process::Command::new(program)
        .args(arguments)
        .current_dir(directory.path())
        .output()
        .is_ok_and(|output| {
            output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "42"
        })
}

/// Install the example, with another command when given, open a scan of
/// four points and press its button: it reads the status and shows the
/// points in the status bar.
fn the_example_counts_the_points(command: Option<Value>) {
    let _language = TestLanguage::hold(Language::English);
    // The window and its server end before another test starts one.
    let _server = crate::native_api::one_server_at_a_time();
    let bench = Bench::new();
    let mut studio = bench.studio();
    let mut receiver = with_api(&mut studio);
    let source = match command {
        Some(command) => example_copy(bench.directory.path(), "1.0.0", Some(command)),
        None => example_folder(),
    };
    install_confirmed(&mut studio, &source);
    assert_eq!(studio.status, "Installed Point count report 1.0.0");

    let scan = bench.directory.path().join("hall.xyz");
    fs::write(&scan, "0 0 0\n4 0 0\n4 3 0\n0 3 2\n").unwrap();
    let cloud = Arc::new(pointcloud_core::open(&scan, 10).unwrap());
    let _ = studio.update(Message::Loaded(Ok(cloud)));

    let _ = studio.update(Message::Extension(ExtensionAction::Press(
        EXAMPLE.into(),
        Some("count".into()),
    )));
    let driven = drive(&mut studio, &mut receiver, EXAMPLE);
    assert!(driven.end.succeeded(), "{:?}\n{}", driven.end, driven.log);
    assert_eq!(
        studio.status,
        "Point count report: 1 scan, 4 points, 0 selected"
    );
    assert_eq!(driven.progress, Some((60.0, "Counting points".to_owned())));
    assert!(
        driven.log.contains("Started from: count; active scan: "),
        "{}",
        driven.log
    );
    assert!(
        driven.log.contains("1 scan, 4 points, 0 selected"),
        "{}",
        driven.log
    );
}

#[test]
fn the_example_counts_the_points_with_powershell() {
    if !interpreter_works(manifest::Interpreter::PowerShell) {
        eprintln!("skipped: PowerShell is not installed here");
        return;
    }
    the_example_counts_the_points(
        (!cfg!(windows)).then(|| json!({"interpreter": "powershell", "program": "report.ps1"})),
    );
}

#[test]
fn the_example_counts_the_points_with_python() {
    if !interpreter_works(manifest::Interpreter::Python) {
        eprintln!("skipped: Python is not installed here");
        return;
    }
    the_example_counts_the_points(
        cfg!(windows).then(|| json!({"interpreter": "python", "program": "report.py"})),
    );
}

#[test]
fn log_names_are_dates_and_the_tail_of_errors_fits_on_a_line() {
    use std::time::{SystemTime, UNIX_EPOCH};
    assert_eq!(run::stamp(UNIX_EPOCH), "19700101-000000");
    assert_eq!(
        run::stamp(UNIX_EPOCH + Duration::from_secs(1_790_208_000 + 3_723)),
        "20260924-010203"
    );
    assert_eq!(
        run::stamp(UNIX_EPOCH + Duration::from_secs(951_782_400)),
        "20000229-000000"
    );
    assert!(run::stamp(SystemTime::now()).starts_with("20"));
    assert_eq!(
        run::tail_text(b"\n one \r\n\ntwo\nthree\nfour\n"),
        "two · three · four"
    );
    let long = "x".repeat(400);
    let tail = run::tail_text(long.as_bytes());
    assert_eq!(tail.chars().count(), 300);
    assert!(tail.starts_with('…'));
    assert_eq!(run::tail_text(b""), "");
}
