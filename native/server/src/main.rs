//! open-pointcloud-studio-api, the headless HTTP and MCP service of Open
//! Pointcloud Studio.
//!
//! ```text
//! open-pointcloud-studio-api [--http ADDRESS]
//! open-pointcloud-studio-api --health-check [--http ADDRESS]
//! open-pointcloud-studio-api --version | -V
//! open-pointcloud-studio-api --help | -h
//! ```
//!
//! A setting that stops the start ends the process with exit code 2 and a
//! line on standard error that names the variable.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use pointcloud_server::config::{self, Access, Config};
use pointcloud_server::http::{self, AppState};
use pointcloud_server::{NAME, VERSION};

const USAGE: &str = "Usage:
  open-pointcloud-studio-api [--http ADDRESS]                 serve the cloud API (default: OPENAEC_MCP_HTTP, else 127.0.0.1:8080)
  open-pointcloud-studio-api --health-check [--http ADDRESS]  ask GET /v1/health of the local service; exit 0 on 200
  open-pointcloud-studio-api --version | -V
  open-pointcloud-studio-api --help | -h

Access: OPENAEC_API_SLEUTELS (SHA-256 of each key) or, for local use only,
OPENAEC_API_TOEGANG=open. The other settings are in docs/cloud-api.md.";

/// The address to listen on: `--http` wins over `OPENAEC_MCP_HTTP`.
fn address(flag: Option<String>) -> String {
    flag.or_else(|| {
        std::env::var("OPENAEC_MCP_HTTP")
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
    .unwrap_or_else(|| "127.0.0.1:8080".to_owned())
}

/// A refusal at start: the line on standard error and exit code 2.
fn refuse(line: &str) -> ExitCode {
    eprintln!("{NAME}: {line}");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let mut arguments = Vec::new();
    for argument in std::env::args_os().skip(1) {
        match argument.into_string() {
            Ok(argument) => arguments.push(argument),
            Err(_) => return refuse("arguments must be text"),
        }
    }
    let mut http_address = None;
    let mut health_check = false;
    let mut rest = arguments.into_iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--version" | "-V" => {
                println!("{NAME} {VERSION}");
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--health-check" => health_check = true,
            "--http" => match rest.next() {
                Some(value) => http_address = Some(value),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let address = address(http_address);
    if health_check {
        return check_health(&address);
    }
    serve(&address)
}

/// `GET /v1/health` on 127.0.0.1 at the port of `address`, within 3 s.
fn check_health(address: &str) -> ExitCode {
    let Some(port) = address
        .rsplit(':')
        .next()
        .and_then(|port| port.parse::<u16>().ok())
    else {
        return refuse(&format!(
            "--http needs an address with a port, not {address}"
        ));
    };
    let timeout = Duration::from_secs(3);
    let ask = || -> std::io::Result<String> {
        let target = SocketAddr::from(([127, 0, 0, 1], port));
        let mut stream = TcpStream::connect_timeout(&target, timeout)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.write_all(
            b"GET /v1/health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        )?;
        let mut answer = Vec::new();
        stream.take(64 * 1024).read_to_end(&mut answer)?;
        Ok(String::from_utf8_lossy(&answer).into_owned())
    };
    match ask() {
        Ok(answer) if answer.starts_with("HTTP/1.1 200") => ExitCode::SUCCESS,
        Ok(answer) => {
            eprintln!(
                "{NAME}: health check answered {}",
                answer.lines().next().unwrap_or("nothing")
            );
            ExitCode::FAILURE
        }
        Err(error) => {
            eprintln!("{NAME}: health check failed: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Tracing to standard error, filtered by `OPENAEC_MCP_LOG`, as text or as
/// one JSON object per line (`OPENAEC_API_LOGVORM=json`).
fn start_tracing() -> Result<(), String> {
    let filter = std::env::var("OPENAEC_MCP_LOG").unwrap_or_else(|_| "info".to_owned());
    let filter = tracing_subscriber::EnvFilter::try_new(&filter)
        .map_err(|error| format!("OPENAEC_MCP_LOG is not a tracing filter: {error}"))?;
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr);
    match std::env::var("OPENAEC_API_LOGVORM").ok().as_deref() {
        None | Some("") | Some("text") => builder.init(),
        Some("json") => builder.json().init(),
        Some(_) => return Err("OPENAEC_API_LOGVORM must be text or json.".to_owned()),
    }
    Ok(())
}

fn serve(address: &str) -> ExitCode {
    if let Err(line) = start_tracing() {
        return refuse(&line);
    }
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(line) => return refuse(&line),
    };
    // The work folders of the calls, and the folder the core writes its
    // scratch files to.
    let temp = std::env::temp_dir();
    for (name, folder) in [("OPENAEC_API_WERKMAP", &config.work_dir), ("TMPDIR", &temp)] {
        if let Err(error) = http::check_folder(folder) {
            return refuse(&format!(
                "{name}: the service cannot write in {}: {error}",
                folder.display()
            ));
        }
    }
    let worst = match (config.memory_mib, config::cgroup_memory_mib()) {
        (None, Some(limit)) => {
            match config.check_memory(limit, "the memory limit of the container") {
                Ok(worst) => worst,
                Err(line) => return refuse(&line),
            }
        }
        _ => config.worst_case_mib(),
    };
    // Every call stands on its own: no cache of an earlier one is read.
    pointcloud_core::set_disk_caches(false);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{NAME}: no runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let open = config.access == Access::Open;
    let result = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(address).await?;
        tracing::info!(
            address = %listener.local_addr()?,
            version = VERSION,
            worst_case_mib = worst,
            "listening"
        );
        if open {
            tracing::warn!(
                "OPENAEC_API_TOEGANG=open: the routes answer without a key. For local use only."
            );
        }
        let router = http::router_with(Arc::new(AppState::new(config)));
        axum::serve(listener, router)
            .with_graceful_shutdown(stopped())
            .await
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{NAME}: cannot serve on {address}: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Ctrl-C, or SIGTERM where there are signals.
async fn stopped() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {}
        () = terminate => {}
    }
    tracing::info!("stopping");
}
