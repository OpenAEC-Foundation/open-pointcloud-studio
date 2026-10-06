//! Running the program of an extension: a separate process in the folder of
//! the extension, given the port and a token of the local API and a file
//! with the context, whose output goes to a log file per run. A process that
//! fails or crashes ends its run and nothing else.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::manifest::{Interpreter, Launch, LOGS};

/// The newest runs whose logs are kept per extension.
const KEPT_RUNS: usize = 20;
/// The most of its output a run writes to its log.
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
/// The end of the error output kept for the status bar.
const TAIL_BYTES: usize = 4096;
/// How long a stopped process on Unix gets to end before it is killed.
const GRACE: Duration = Duration::from_millis(1500);

/// The names of the variables a run gets.
pub const PORT_VARIABLE: &str = "OPS_API_PORT";
pub const TOKEN_VARIABLE: &str = "OPS_API_TOKEN";
pub const ID_VARIABLE: &str = "OPS_EXTENSION_ID";
pub const CONTEXT_VARIABLE: &str = "OPS_CONTEXT";
pub const URL_VARIABLE: &str = "OPS_API_URL";

/// What starts a run.
pub struct RunRequest<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub folder: &'a Path,
    pub launch: &'a Launch,
    /// What the button or tile adds after the arguments of the command.
    pub extra_args: &'a [String],
    pub port: u16,
    pub token: &'a str,
    pub context: &'a Value,
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunEnd {
    /// The exit code; `None` when a signal ended the process.
    pub code: Option<i32>,
    /// Whether it ended because it was stopped.
    pub stopped: bool,
    /// The last lines of its error output.
    pub stderr_tail: String,
}

impl RunEnd {
    pub fn succeeded(&self) -> bool {
        !self.stopped && self.code == Some(0)
    }
}

/// A file found on the search path of the system.
fn on_path(names: &[&str]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for directory in std::env::split_paths(&path) {
        for name in names {
            let candidate = directory.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// The program that runs a script of an interpreter, with the arguments
/// before the script.
pub fn interpreter_command(interpreter: Interpreter) -> Result<(PathBuf, Vec<OsString>), String> {
    let missing = || {
        format!(
            "{} is not installed on this computer, or not on its search path",
            interpreter.key()
        )
    };
    let words = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    match interpreter {
        Interpreter::Python => if cfg!(windows) {
            if let Some(launcher) = on_path(&["py.exe"]) {
                return Ok((launcher, words(&["-3"])));
            }
            on_path(&["python.exe", "python3.exe"]).map(|python| (python, Vec::new()))
        } else {
            on_path(&["python3", "python"]).map(|python| (python, Vec::new()))
        }
        .ok_or_else(missing),
        Interpreter::PowerShell => {
            let flags = ["-NoLogo", "-NoProfile", "-NonInteractive"];
            if cfg!(windows) {
                // Windows PowerShell comes with every Windows; it is looked
                // for in its own place before the search path.
                let builtin = std::env::var_os("SystemRoot")
                    .map(PathBuf::from)
                    .map(|root| root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
                    .filter(|path| path.is_file());
                let program = builtin
                    .or_else(|| on_path(&["powershell.exe", "pwsh.exe"]))
                    .ok_or_else(missing)?;
                let mut arguments = words(&flags);
                arguments.extend(words(&["-ExecutionPolicy", "Bypass", "-File"]));
                Ok((program, arguments))
            } else {
                let program = on_path(&["pwsh"]).ok_or_else(missing)?;
                let mut arguments = words(&flags);
                arguments.push("-File".into());
                Ok((program, arguments))
            }
        }
        Interpreter::Node => on_path(if cfg!(windows) {
            &["node.exe"]
        } else {
            &["node"]
        })
        .map(|node| (node, Vec::new()))
        .ok_or_else(missing),
        Interpreter::Sh => {
            let shell = PathBuf::from("/bin/sh");
            if shell.is_file() {
                Ok((shell, Vec::new()))
            } else {
                Err(missing())
            }
        }
    }
}

/// The program and the arguments a launch starts in a folder.
pub fn command_line(
    launch: &Launch,
    folder: &Path,
    extra: &[String],
) -> Result<(PathBuf, Vec<OsString>), String> {
    let program = launch
        .program
        .split('/')
        .fold(folder.to_path_buf(), |path, part| path.join(part));
    let (program, mut arguments) = match launch.interpreter {
        None => (program, Vec::new()),
        Some(interpreter) => {
            let (interpreter, mut arguments) = interpreter_command(interpreter)?;
            arguments.push(program.into_os_string());
            (interpreter, arguments)
        }
    };
    arguments.extend(launch.args.iter().map(OsString::from));
    arguments.extend(extra.iter().map(OsString::from));
    Ok((program, arguments))
}

/// `YYYYMMDD-HHMMSS` in UTC, for the names of the log files.
pub fn stamp(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    // The civil date of a day count, after Howard Hinnant.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Keep the logs of the newest runs only, to make room for one more.
fn prune_logs(logs: &Path) {
    let Ok(entries) = fs::read_dir(logs) else {
        return;
    };
    let mut runs: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| {
            name.strip_prefix("run-")
                .and_then(|rest| rest.strip_suffix(".log"))
                .map(str::to_owned)
        })
        .collect();
    runs.sort();
    let surplus = runs.len().saturating_sub(KEPT_RUNS - 1);
    for run in &runs[..surplus] {
        let _ = fs::remove_file(logs.join(format!("run-{run}.log")));
        let _ = fs::remove_file(logs.join(format!("run-{run}.context.json")));
    }
}

/// The log of a run, written by the threads that read its output.
struct Log {
    file: Option<File>,
    written: u64,
    cut: bool,
}

impl Log {
    fn write(&mut self, bytes: &[u8]) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let room = MAX_LOG_BYTES.saturating_sub(self.written);
        let taken = bytes.len().min(usize::try_from(room).unwrap_or(usize::MAX));
        if file.write_all(&bytes[..taken]).is_err() {
            self.file = None;
            return;
        }
        self.written += taken as u64;
        if taken < bytes.len() && !self.cut {
            self.cut = true;
            let _ = writeln!(
                file,
                "\n# The output went on; the log keeps its first {} MiB.",
                MAX_LOG_BYTES / (1024 * 1024)
            );
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read one output stream of the process to its end into the log, keeping
/// the end of it when it is the error output.
fn pump(
    mut stream: impl Read + Send + 'static,
    log: Arc<Mutex<Log>>,
    tail: Option<Arc<Mutex<VecDeque<u8>>>>,
) -> Receiver<()> {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    lock(&log).write(&buffer[..read]);
                    if let Some(tail) = &tail {
                        let mut tail = lock(tail);
                        tail.extend(&buffer[..read]);
                        let surplus = tail.len().saturating_sub(TAIL_BYTES);
                        tail.drain(..surplus);
                    }
                }
            }
        }
        let _ = done.send(());
    });
    finished
}

/// The last lines of the error output, on one line for the status bar.
pub fn tail_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(3);
    let joined = lines[start..].join(" · ");
    if joined.chars().count() > 300 {
        let kept: String = joined.chars().rev().take(299).collect();
        format!("…{}", kept.chars().rev().collect::<String>())
    } else {
        joined
    }
}

/// A Windows job that holds the process and whatever it starts, so that
/// stopping the run, and closing the window, ends all of them.
#[cfg(windows)]
mod job {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimits {
        per_process_user_time: i64,
        per_job_user_time: i64,
        limit_flags: u32,
        minimum_working_set: usize,
        maximum_working_set: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimits {
        basic: BasicLimits,
        io: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory: usize,
        peak_job_memory: usize,
    }

    const EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(
            job: Handle,
            class: i32,
            information: *mut c_void,
            length: u32,
        ) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub struct Job(Handle);

    // The handle is only used through the calls above, which may come from
    // any thread.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        /// A job for a child that has just started; `None` when the system
        /// refuses one, and the child is then stopped by itself.
        pub fn holding(child: &std::process::Child) -> Option<Self> {
            // SAFETY: plain calls with a structure of the size the call
            // expects; the handles are checked before they are used.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if handle.is_null() {
                    return None;
                }
                let job = Self(handle);
                let mut limits = ExtendedLimits::default();
                limits.basic.limit_flags = KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    handle,
                    EXTENDED_LIMIT_INFORMATION,
                    (&mut limits as *mut ExtendedLimits).cast(),
                    std::mem::size_of::<ExtendedLimits>() as u32,
                );
                if set == 0 || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0 {
                    return None;
                }
                Some(job)
            }
        }

        pub fn terminate(&self) {
            // SAFETY: the handle is a job this value owns.
            unsafe {
                TerminateJobObject(self.0, 1);
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is a job this value owns; closing it ends
            // the processes that are still in it.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(unix)]
extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}

/// Send a signal to the process group of a run, which its process leads.
#[cfg(unix)]
fn signal_group(pid: u32, signal: i32) {
    if let Ok(pid) = i32::try_from(pid) {
        // SAFETY: a plain system call; a group that is gone is no error.
        unsafe {
            kill(-pid, signal);
        }
    }
}

/// A run under way: its process, the threads that read its output and the
/// request to stop it.
pub struct RunControl {
    child: Mutex<Child>,
    #[cfg(windows)]
    job: Option<job::Job>,
    stop: AtomicBool,
    stop_requested: Mutex<Option<Instant>>,
    killed: AtomicBool,
    /// Set, under the lock of the child, once its exit was collected: its
    /// process id may then belong to another process.
    exited: AtomicBool,
    tail: Arc<Mutex<VecDeque<u8>>>,
    log: Arc<Mutex<Log>>,
    pumps: Mutex<Vec<Receiver<()>>>,
    end: Mutex<Option<RunEnd>>,
    pub log_path: PathBuf,
    pub context_path: PathBuf,
    pub pid: u32,
}

/// Start the program of an extension. The context is written to a file
/// beside the log of the run first.
pub fn start(request: &RunRequest<'_>) -> Result<Arc<RunControl>, String> {
    let (program, arguments) = command_line(request.launch, request.folder, request.extra_args)?;
    let logs = request.folder.join(LOGS);
    fs::create_dir_all(&logs).map_err(|error| format!("{}: {error}", logs.display()))?;
    prune_logs(&logs);
    let mut name = stamp(SystemTime::now());
    let mut number = 1;
    while logs.join(format!("run-{name}.log")).exists() {
        number += 1;
        name = format!("{}-{number}", stamp(SystemTime::now()));
    }
    let log_path = logs.join(format!("run-{name}.log"));
    let context_path = logs.join(format!("run-{name}.context.json"));
    let context = serde_json::to_vec_pretty(request.context).map_err(|error| error.to_string())?;
    fs::write(&context_path, context)
        .map_err(|error| format!("{}: {error}", context_path.display()))?;
    let mut log_file =
        File::create(&log_path).map_err(|error| format!("{}: {error}", log_path.display()))?;
    let shown: Vec<String> = std::iter::once(program.to_string_lossy().into_owned())
        .chain(
            arguments
                .iter()
                .map(|argument| argument.to_string_lossy().into_owned()),
        )
        .collect();
    let _ = writeln!(
        log_file,
        "# {} {} in Open Pointcloud Studio {}, started {} UTC\n# {}",
        request.name,
        request.version,
        env!("CARGO_PKG_VERSION"),
        stamp(SystemTime::now()),
        shown.join(" ")
    );

    let mut command = Command::new(&program);
    command
        .args(&arguments)
        .current_dir(request.folder)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env(PORT_VARIABLE, request.port.to_string())
        .env(TOKEN_VARIABLE, request.token)
        .env(ID_VARIABLE, request.id)
        .env(CONTEXT_VARIABLE, &context_path)
        .env(URL_VARIABLE, format!("http://127.0.0.1:{}", request.port))
        // Output in the order it is written, and in UTF-8.
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for a program without a window of its own.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so that stopping it ends what it started.
        command.process_group(0);
    }
    let mut child = command.spawn().map_err(|error| {
        let _ = writeln!(log_file, "# Could not start: {error}");
        format!("{} could not start: {error}", program.display())
    })?;
    #[cfg(windows)]
    let job = job::Job::holding(&child);
    let log = Arc::new(Mutex::new(Log {
        file: Some(log_file),
        written: 0,
        cut: false,
    }));
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let mut pumps = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        pumps.push(pump(stdout, Arc::clone(&log), None));
    }
    if let Some(stderr) = child.stderr.take() {
        pumps.push(pump(stderr, Arc::clone(&log), Some(Arc::clone(&tail))));
    }
    let pid = child.id();
    Ok(Arc::new(RunControl {
        child: Mutex::new(child),
        #[cfg(windows)]
        job,
        stop: AtomicBool::new(false),
        stop_requested: Mutex::new(None),
        killed: AtomicBool::new(false),
        exited: AtomicBool::new(false),
        tail,
        log,
        pumps: Mutex::new(pumps),
        end: Mutex::new(None),
        log_path,
        context_path,
        pid,
    }))
}

impl RunControl {
    /// Ask the run to stop: on Windows its process and what it started end
    /// at once; on Unix they get a moment to end before they are killed.
    pub fn stop(&self) {
        if self.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        *lock(&self.stop_requested) = Some(Instant::now());
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                job.terminate();
            }
            self.kill();
        }
        #[cfg(unix)]
        {
            let _child = lock(&self.child);
            if !self.exited.load(Ordering::SeqCst) {
                signal_group(self.pid, 15);
            }
        }
    }

    /// End the process and what it started at once.
    pub fn kill(&self) {
        if self.killed.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut child = lock(&self.child);
        if self.exited.load(Ordering::SeqCst) {
            return;
        }
        #[cfg(unix)]
        signal_group(self.pid, 9);
        let _ = child.kill();
    }

    pub fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// How the run ended, once it has; a stopped run that does not end is
    /// killed after a moment.
    pub fn try_end(&self) -> Option<RunEnd> {
        // One caller at a time finishes the run.
        let mut finished = lock(&self.end);
        if let Some(end) = finished.clone() {
            return Some(end);
        }
        let collected = {
            let mut child = lock(&self.child);
            let collected = child.try_wait();
            if matches!(collected, Ok(Some(_))) {
                self.exited.store(true, Ordering::SeqCst);
            }
            collected
        };
        let status = match collected {
            Ok(Some(status)) => status,
            Ok(None) => {
                let overdue = lock(&self.stop_requested)
                    .is_some_and(|requested| requested.elapsed() >= GRACE);
                if overdue {
                    self.kill();
                }
                return None;
            }
            Err(_) => {
                self.kill();
                return None;
            }
        };
        // The output is read to its end, unless something the process
        // started keeps it open.
        let deadline = Instant::now() + Duration::from_secs(2);
        for pump in lock(&self.pumps).drain(..) {
            let left = deadline.saturating_duration_since(Instant::now());
            let _ = pump.recv_timeout(left);
        }
        let stopped = self.stopping();
        let tail: Vec<u8> = lock(&self.tail).iter().copied().collect();
        let end = RunEnd {
            code: status.code(),
            stopped,
            stderr_tail: tail_text(&tail),
        };
        {
            let mut log = lock(&self.log);
            let line = match (stopped, end.code) {
                (true, _) => "\n# Stopped\n".to_owned(),
                (false, Some(code)) => format!("\n# Ended with exit code {code}\n"),
                (false, None) => "\n# Ended by a signal\n".to_owned(),
            };
            if let Some(file) = log.file.as_mut() {
                let _ = file.write_all(line.as_bytes());
            }
            log.file = None;
        }
        *finished = Some(end.clone());
        Some(end)
    }

    /// Wait until the run has ended.
    pub fn wait(&self) -> RunEnd {
        loop {
            if let Some(end) = self.try_end() {
                return end;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}
