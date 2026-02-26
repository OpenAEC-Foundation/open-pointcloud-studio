use serde::{Deserialize, Serialize};
use std::fs;
use std::process::Command;

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveResult {
    success: bool,
    message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoadResult {
    success: bool,
    data: Option<String>,
    message: String,
}

/// Save data to a file
#[tauri::command]
pub fn save_file(path: String, data: String) -> SaveResult {
    match fs::write(&path, &data) {
        Ok(_) => SaveResult {
            success: true,
            message: format!("File saved to {}", path),
        },
        Err(e) => SaveResult {
            success: false,
            message: format!("Failed to save file: {}", e),
        },
    }
}

/// Load data from a file
#[tauri::command]
pub fn load_file(path: String) -> LoadResult {
    match fs::read_to_string(&path) {
        Ok(content) => LoadResult {
            success: true,
            data: Some(content),
            message: "File loaded successfully".to_string(),
        },
        Err(e) => LoadResult {
            success: false,
            data: None,
            message: format!("Failed to load file: {}", e),
        },
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ShellResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

/// Execute a shell command (git, claude, or other allowed commands)
#[tauri::command]
pub async fn execute_shell(program: String, args: Vec<String>) -> ShellResult {
    let allowed_programs = ["git", "claude"];
    let program_name = program.to_lowercase();

    if !allowed_programs.iter().any(|&p| program_name == p || program_name.ends_with(&format!("\\{}", p)) || program_name.ends_with(&format!("/{}", p))) {
        return ShellResult {
            success: false,
            stdout: String::new(),
            stderr: format!("Program '{}' is not allowed. Allowed: git, claude", program),
            code: -1,
        };
    }

    let result = tauri::async_runtime::spawn_blocking(move || {
        Command::new(&program).args(&args).output()
    }).await;

    match result {
        Ok(Ok(output)) => ShellResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            code: output.status.code().unwrap_or(-1),
        },
        Ok(Err(e)) => ShellResult {
            success: false,
            stdout: String::new(),
            stderr: format!("Failed to execute command: {}", e),
            code: -1,
        },
        Err(e) => ShellResult {
            success: false,
            stdout: String::new(),
            stderr: format!("Task failed: {}", e),
            code: -1,
        },
    }
}
