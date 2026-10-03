//! Running windows of the application: finding them through the discovery
//! files their command API writes, choosing one, starting one, and sending
//! it commands.

use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, UNIX_EPOCH};

use reqwest::blocking::Client;
use serde_json::{json, Value};

use super::tools::Link;

const API_NAME: &str = "native-rust-v1";
/// How long a running instance may take to answer `/info`.
const PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// How long a command may take; the window itself gives up after 10 seconds.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a started window may take until its command API answers.
const START_TIMEOUT: Duration = Duration::from_secs(60);

/// A running window as its discovery file and `/info` describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    pub pid: u32,
    pub port: u16,
    pub token: String,
    /// Milliseconds since 1970 when its command API started.
    pub started: u64,
    pub version: Option<String>,
    pub discovery_file: PathBuf,
}

impl Instance {
    /// The instance as tools report it, without its token.
    pub fn summary(&self, chosen: bool) -> Value {
        json!({
            "pid": self.pid,
            "port": self.port,
            "version": self.version,
            "started": self.started,
            "discovery_file": self.discovery_file,
            "selected": chosen,
        })
    }
}

pub fn http_client() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_secs(2))
        .timeout(COMMAND_TIMEOUT)
        .build()
        .map_err(|error| error.to_string())
}

/// Read one discovery file. Its start time falls back to the time the file
/// was written, for instances that do not record it.
pub fn read_discovery(path: &Path) -> Option<Instance> {
    let name = path.file_name()?.to_str()?;
    if !(name.starts_with("instance-") && name.ends_with(".json")) {
        return None;
    }
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return None;
    }
    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    if value["api"] != API_NAME {
        return None;
    }
    let written = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok())
        .unwrap_or(0);
    Some(Instance {
        pid: u32::try_from(value["pid"].as_u64()?).ok()?,
        port: u16::try_from(value["port"].as_u64()?).ok()?,
        token: value["token"].as_str()?.to_owned(),
        started: value["started"].as_u64().unwrap_or(written),
        version: None,
        discovery_file: path.to_path_buf(),
    })
}

/// The instance when its process runs and its port answers `/info` as
/// that process, with the version it reports.
pub fn probe(mut instance: Instance, client: &Client) -> Option<Instance> {
    if !process_running(instance.pid) {
        return None;
    }
    let response = client
        .get(format!("http://127.0.0.1:{}/info", instance.port))
        .timeout(PROBE_TIMEOUT)
        .send()
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let info: Value = serde_json::from_str(&response.text().ok()?).ok()?;
    if info["api"] != API_NAME || info["pid"].as_u64() != Some(u64::from(instance.pid)) {
        return None;
    }
    instance.version = info["version"].as_str().map(str::to_owned);
    Some(instance)
}

/// The live instances of a discovery directory, most recently started first.
/// Files of processes that are gone or whose port does not answer are
/// skipped and left alone.
pub fn discover(directory: &Path, client: &Client) -> Vec<Instance> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let found: Vec<Instance> = entries
        .flatten()
        .filter_map(|entry| read_discovery(&entry.path()))
        .collect();
    // A port that nothing listens on can take seconds to refuse, so the
    // instances are probed side by side.
    let mut live: Vec<Instance> = std::thread::scope(|scope| {
        let probes: Vec<_> = found
            .into_iter()
            .map(|instance| scope.spawn(move || probe(instance, client)))
            .collect();
        probes
            .into_iter()
            .filter_map(|probe| probe.join().ok().flatten())
            .collect()
    });
    live.sort_by(|a, b| b.started.cmp(&a.started).then(b.pid.cmp(&a.pid)));
    live
}

#[cfg(windows)]
fn process_running(pid: u32) -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
        fn GetLastError() -> u32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ERROR_ACCESS_DENIED: u32 = 5;
    // SAFETY: the handle is checked before use and closed once; the exit
    // code is written to a local.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            // A process of another user may exist without being readable.
            return GetLastError() == ERROR_ACCESS_DENIED;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(process, &mut code);
        CloseHandle(process);
        read != 0 && code == STILL_ACTIVE
    }
}

#[cfg(target_os = "linux")]
fn process_running(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

#[cfg(not(any(windows, target_os = "linux")))]
fn process_running(_pid: u32) -> bool {
    // The `/info` check below tells a reused or closed port apart.
    true
}

/// Why a command did not get an answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendError {
    /// Nothing answers on the port: the command was not delivered.
    Unreachable(String),
    /// The window refused the command or did not answer it in time.
    Failed(String),
}

/// Send one command to an instance's `POST /exec` and return its answer.
pub fn send(client: &Client, instance: &Instance, command: &Value) -> Result<Value, SendError> {
    let response = client
        .post(format!("http://127.0.0.1:{}/exec", instance.port))
        .header("Content-Type", "application/json")
        .header("X-OPS-Token", &instance.token)
        .body(command.to_string())
        .send()
        .map_err(|error| {
            if error.is_connect() {
                SendError::Unreachable(error.to_string())
            } else {
                SendError::Failed(error.to_string())
            }
        })?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|error| SendError::Failed(error.to_string()))?;
    let answer: Option<Value> = serde_json::from_str(&body).ok();
    if status.is_success() {
        return answer.ok_or_else(|| SendError::Failed("the window answered without JSON".into()));
    }
    let reason = answer
        .as_ref()
        .and_then(|answer| answer["error"].as_str())
        .unwrap_or(body.as_str())
        .to_owned();
    Err(SendError::Failed(format!(
        "the command API answered HTTP {}: {reason}",
        status.as_u16()
    )))
}

/// A port on the loopback interface that nothing listens on now.
pub fn free_port() -> std::io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

/// Wait until the discovery file of a started process names an instance
/// that answers, while the process still runs.
pub fn await_instance(
    directory: &Path,
    child: &mut Child,
    client: &Client,
    timeout: Duration,
) -> Result<Instance, String> {
    let pid = child.id();
    let file = directory.join(format!("instance-{pid}.json"));
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(instance) = read_discovery(&file).and_then(|found| probe(found, client)) {
            return Ok(instance);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the application exited ({status}) before its command API answered"
            ));
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "the started window (pid {pid}) did not write {} within {} seconds; it was left open, and list_instances shows it once its command API answers",
                file.display(),
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The command line that starts a window with its command API on `port`.
pub fn start_command(program: &Path, port: u16, files: &[PathBuf]) -> Command {
    let mut command = Command::new(program);
    command
        .arg("--api-port")
        .arg(port.to_string())
        .args(files)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// Start a process that outlives this one: it gets no console and its own
/// process group, so closing the client of this server leaves it open.
#[cfg(windows)]
fn spawn_detached(mut command: Command) -> std::io::Result<Child> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    keep_standard_handles_private();
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
    match command.spawn() {
        Ok(child) => Ok(child),
        // A job that does not allow breaking away refuses that flag.
        Err(_) => {
            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
            command.spawn()
        }
    }
}

/// The pipes of this server's standard streams must not be inherited by a
/// started window, or the client would not see them close when this server
/// ends.
#[cfg(windows)]
fn keep_standard_handles_private() {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    for handle in [
        std::io::stdin().as_raw_handle(),
        std::io::stdout().as_raw_handle(),
        std::io::stderr().as_raw_handle(),
    ] {
        if !handle.is_null() {
            // SAFETY: the handle belongs to this process; only its
            // inheritance flag changes.
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(unix)]
fn spawn_detached(mut command: Command) -> std::io::Result<Child> {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    command.spawn()
}

#[cfg(not(any(windows, unix)))]
fn spawn_detached(mut command: Command) -> std::io::Result<Child> {
    command.spawn()
}

/// Start a window of `program` with its command API on a free port, wait
/// for its discovery file in `directory` and return it as an instance.
pub fn start(
    program: &Path,
    directory: &Path,
    files: &[PathBuf],
    client: &Client,
) -> Result<Instance, String> {
    let port = free_port().map_err(|error| format!("no free port: {error}"))?;
    let mut child = spawn_detached(start_command(program, port, files))
        .map_err(|error| format!("could not start {}: {error}", program.display()))?;
    eprintln!(
        "open-pointcloud-studio mcp: started {} (pid {}) with its command API on port {port}",
        program.display(),
        child.id()
    );
    let result = await_instance(directory, &mut child, client, START_TIMEOUT);
    // Collect the exit status when the window closes.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    result
}

/// The link to the windows of this computer: the instance the tools drive,
/// found in the discovery directory or started when there is none.
pub struct Connector {
    directory: PathBuf,
    program: PathBuf,
    client: Client,
    current: Option<Instance>,
    /// Whether the client chose the current instance itself.
    chosen: bool,
    auto_start: bool,
}

impl Connector {
    pub fn new(directory: PathBuf, program: PathBuf, auto_start: bool) -> Result<Self, String> {
        Ok(Self {
            directory,
            program,
            client: http_client()?,
            current: None,
            chosen: false,
            auto_start,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The current instance; without one, the most recently started live
    /// instance, or a newly started window.
    fn instance(&mut self) -> Result<Instance, String> {
        if let Some(instance) = &self.current {
            return Ok(instance.clone());
        }
        let instance = match discover(&self.directory, &self.client).into_iter().next() {
            Some(instance) => instance,
            None if self.auto_start => start(&self.program, &self.directory, &[], &self.client)?,
            None => {
                return Err(format!(
                    "no running Open Pointcloud Studio window was found in {}",
                    self.directory.display()
                ))
            }
        };
        eprintln!(
            "open-pointcloud-studio mcp: using pid {} on port {}",
            instance.pid, instance.port
        );
        self.current = Some(instance.clone());
        self.chosen = false;
        Ok(instance)
    }
}

impl Link for Connector {
    fn exec(&mut self, command: &Value) -> Result<Value, String> {
        let instance = self.instance()?;
        match send(&self.client, &instance, command) {
            Ok(answer) => Ok(answer),
            Err(SendError::Unreachable(_)) if !self.chosen => {
                // The window that was picked automatically has closed; the
                // command never arrived, so it goes to the next choice.
                self.current = None;
                let instance = self.instance()?;
                send(&self.client, &instance, command).map_err(|error| match error {
                    SendError::Unreachable(reason) | SendError::Failed(reason) => reason,
                })
            }
            Err(SendError::Unreachable(reason)) => Err(format!(
                "the selected window (pid {}, port {}) does not answer: {reason}; use list_instances and select_instance to choose another",
                instance.pid, instance.port
            )),
            Err(SendError::Failed(reason)) => Err(reason),
        }
    }

    fn list_instances(&mut self) -> Value {
        let live = discover(&self.directory, &self.client);
        let current = self.current.as_ref();
        json!({
            "directory": self.directory,
            "instances": live
                .iter()
                .map(|instance| instance.summary(current.is_some_and(|chosen| chosen.pid == instance.pid && chosen.port == instance.port)))
                .collect::<Vec<_>>(),
            "selected": current.map(|instance| json!({
                "pid": instance.pid,
                "port": instance.port,
                "by_client": self.chosen,
            })),
        })
    }

    fn select_instance(&mut self, pid: Option<u32>, port: Option<u16>) -> Result<Value, String> {
        let live = discover(&self.directory, &self.client);
        let found = live.iter().find(|instance| {
            pid.is_none_or(|pid| instance.pid == pid)
                && port.is_none_or(|port| instance.port == port)
        });
        let Some(instance) = found else {
            let running: Vec<String> = live
                .iter()
                .map(|instance| format!("pid {} on port {}", instance.pid, instance.port))
                .collect();
            return Err(format!(
                "no running window matches; running: {}",
                if running.is_empty() {
                    "none".to_owned()
                } else {
                    running.join(", ")
                }
            ));
        };
        self.current = Some(instance.clone());
        self.chosen = true;
        eprintln!(
            "open-pointcloud-studio mcp: selected pid {} on port {}",
            instance.pid, instance.port
        );
        Ok(json!({"ok": true, "selected": instance.summary(true)}))
    }

    fn start_instance(&mut self, files: &[PathBuf]) -> Result<Value, String> {
        let instance = start(&self.program, &self.directory, files, &self.client)?;
        self.current = Some(instance.clone());
        self.chosen = true;
        Ok(json!({"ok": true, "started": instance.summary(true)}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::mock::mock_api;
    use std::thread;

    fn write_discovery(directory: &Path, name: &str, value: Value) -> PathBuf {
        let path = directory.join(name);
        fs::write(&path, value.to_string()).unwrap();
        path
    }

    fn closed_port() -> u16 {
        free_port().unwrap()
    }

    #[test]
    fn discovery_keeps_live_instances_newest_first() {
        let directory = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let older = mock_api(me);
        let newer = mock_api(me);
        let other_pid = mock_api(me.wrapping_add(1));
        write_discovery(
            directory.path(),
            "instance-1.json",
            json!({"pid": me, "port": older.port, "token": "a", "api": API_NAME, "started": 1_000}),
        );
        write_discovery(
            directory.path(),
            "instance-2.json",
            json!({"pid": me, "port": newer.port, "token": "b", "api": API_NAME, "started": 2_000}),
        );
        // Its port answers for another process.
        write_discovery(
            directory.path(),
            "instance-3.json",
            json!({"pid": me, "port": other_pid.port, "token": "c", "api": API_NAME, "started": 3_000}),
        );
        // Nothing listens on its port.
        write_discovery(
            directory.path(),
            "instance-4.json",
            json!({"pid": me, "port": closed_port(), "token": "d", "api": API_NAME, "started": 4_000}),
        );
        // Its process is gone.
        write_discovery(
            directory.path(),
            "instance-5.json",
            json!({"pid": u32::MAX - 7, "port": newer.port, "token": "e", "api": API_NAME, "started": 5_000}),
        );
        write_discovery(
            directory.path(),
            "instance-6.json",
            json!("not an instance"),
        );
        fs::write(directory.path().join("instance-7.json"), b"{broken").unwrap();
        write_discovery(
            directory.path(),
            "notes.json",
            json!({"pid": me, "port": newer.port, "token": "f", "api": API_NAME, "started": 9_000}),
        );
        write_discovery(
            directory.path(),
            "instance-8.json",
            json!({"pid": me, "port": newer.port, "token": "g", "api": "other", "started": 9_000}),
        );

        let client = http_client().unwrap();
        let live = discover(directory.path(), &client);
        let ports: Vec<u16> = live.iter().map(|instance| instance.port).collect();
        assert_eq!(ports, [newer.port, older.port]);
        assert_eq!(live[0].token, "b");
        assert_eq!(live[0].version.as_deref(), Some("9.9.9"));
        assert!(discover(&directory.path().join("missing"), &client).is_empty());
        // Stale files are skipped, not removed.
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 9);
    }

    #[test]
    fn a_file_without_start_time_uses_its_modification_time() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_discovery(
            directory.path(),
            "instance-42.json",
            json!({"pid": 42, "port": 1234, "token": "t", "api": API_NAME}),
        );
        let instance = read_discovery(&path).unwrap();
        assert_eq!((instance.pid, instance.port), (42, 1234));
        assert!(instance.started > 1_600_000_000_000);
    }

    #[test]
    fn the_connector_uses_the_newest_instance_and_a_chosen_one_sticks() {
        let directory = tempfile::tempdir().unwrap();
        let me = std::process::id();
        let older = mock_api(me);
        let newer = mock_api(me);
        write_discovery(
            directory.path(),
            "instance-1.json",
            json!({"pid": me, "port": older.port, "token": "old", "api": API_NAME, "started": 1}),
        );
        write_discovery(
            directory.path(),
            "instance-2.json",
            json!({"pid": me, "port": newer.port, "token": "new", "api": API_NAME, "started": 2}),
        );
        let mut connector =
            Connector::new(directory.path().into(), PathBuf::from("unused"), false).unwrap();
        assert_eq!(
            connector.exec(&json!({"command": "status"})).unwrap()["ok"],
            true
        );
        assert_eq!(
            newer.bodies.lock().unwrap().as_slice(),
            [(r#"{"command":"status"}"#.to_owned(), Some("new".to_owned()))]
        );
        assert!(older.bodies.lock().unwrap().is_empty());

        let listed = connector.list_instances();
        assert_eq!(listed["instances"].as_array().unwrap().len(), 2);
        assert_eq!(listed["instances"][0]["selected"], true);
        assert_eq!(listed["selected"]["by_client"], false);
        assert!(
            listed.to_string().find("\"new\"").is_none(),
            "tokens stay private"
        );

        let selected = connector.select_instance(None, Some(older.port)).unwrap();
        assert_eq!(selected["selected"]["port"], older.port);
        connector.exec(&json!({"command": "zoom_all"})).unwrap();
        assert_eq!(older.bodies.lock().unwrap().len(), 1);
        assert!(connector.select_instance(Some(1), None).is_err());
        assert_eq!(connector.list_instances()["selected"]["port"], older.port);
    }

    #[test]
    fn without_instances_and_auto_start_the_connector_says_so() {
        let directory = tempfile::tempdir().unwrap();
        let mut connector =
            Connector::new(directory.path().into(), PathBuf::from("unused"), false).unwrap();
        let error = connector.exec(&json!({"command": "status"})).unwrap_err();
        assert!(
            error.starts_with("no running Open Pointcloud Studio window"),
            "{error}"
        );
    }

    #[test]
    fn a_started_window_is_awaited_through_its_discovery_file() {
        let directory = tempfile::tempdir().unwrap();
        let client = http_client().unwrap();
        // A process that runs for a while stands in for the window.
        let mut child = if cfg!(windows) {
            Command::new("ping")
                .args(["-n", "30", "127.0.0.1"])
                .stdout(Stdio::null())
                .spawn()
                .unwrap()
        } else {
            Command::new("sleep").arg("30").spawn().unwrap()
        };
        let pid = child.id();
        let api = mock_api(pid);
        let file = directory.path().join(format!("instance-{pid}.json"));
        let port = api.port;
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            let value = json!({"pid": pid, "port": port, "token": "fresh", "api": API_NAME});
            fs::write(&file, value.to_string()).unwrap();
        });
        let started = Instant::now();
        let instance = await_instance(
            directory.path(),
            &mut child,
            &client,
            Duration::from_secs(20),
        )
        .unwrap();
        writer.join().unwrap();
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert_eq!((instance.pid, instance.port), (pid, api.port));
        assert_eq!(instance.token, "fresh");
        let _ = child.kill();
        let _ = child.wait();

        // A process that ends before writing its file is reported.
        let mut quick = if cfg!(windows) {
            Command::new("cmd").args(["/C", "exit 3"]).spawn().unwrap()
        } else {
            Command::new("sh").args(["-c", "exit 3"]).spawn().unwrap()
        };
        let error = await_instance(
            directory.path(),
            &mut quick,
            &client,
            Duration::from_secs(20),
        )
        .unwrap_err();
        assert!(error.contains("exited"), "{error}");
    }

    #[test]
    fn the_start_command_asks_for_the_port_and_opens_the_files() {
        let command = start_command(
            Path::new("studio"),
            47_911,
            &[PathBuf::from("/scans/a.e57"), PathBuf::from("/scans/b.laz")],
        );
        let arguments: Vec<_> = command
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            arguments,
            ["--api-port", "47911", "/scans/a.e57", "/scans/b.laz"]
        );
        let port = free_port().unwrap();
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok());
    }
}
