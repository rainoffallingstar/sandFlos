use chrono::Local;
use serde::Serialize;
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

const DIAGNOSTICS_LOG_FILE: &str = "diagnostics.log";

static DIAGNOSTICS_PATH: OnceLock<PathBuf> = OnceLock::new();
static DIAGNOSTICS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsInfo {
    pub path: String,
    pub recent: String,
}

fn write_lock() -> &'static Mutex<()> {
    DIAGNOSTICS_WRITE_LOCK.get_or_init(|| Mutex::new(()))
}

fn diagnostics_path() -> Option<&'static PathBuf> {
    DIAGNOSTICS_PATH.get()
}

fn sanitize_line(value: &str) -> String {
    value
        .replace('\r', "\\r")
        .replace('\n', "\\n")
        .replace('\t', " ")
}

pub fn init() {
    if diagnostics_path().is_some() {
        return;
    }
    let dir = crate::state::app_data_dir();
    let file_path = dir.join(DIAGNOSTICS_LOG_FILE);
    let _ = DIAGNOSTICS_PATH.set(file_path);
}

pub fn log(level: &str, scope: &str, message: impl AsRef<str>) {
    let Some(path) = diagnostics_path() else {
        return;
    };
    let Ok(_guard) = write_lock().lock() else {
        return;
    };
    let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    let line = format!(
        "[{timestamp}] [{level}] [{scope}] {}\n",
        sanitize_line(message.as_ref())
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn info(scope: &str, message: impl AsRef<str>) {
    log("INFO", scope, message);
}

pub fn warn(scope: &str, message: impl AsRef<str>) {
    log("WARN", scope, message);
}

pub fn error(scope: &str, message: impl AsRef<str>) {
    log("ERROR", scope, message);
}

pub fn path_string() -> Option<String> {
    diagnostics_path().map(|path| path.to_string_lossy().to_string())
}

pub fn recent_lines(max_lines: usize) -> String {
    let Some(path) = diagnostics_path() else {
        return String::new();
    };
    let Ok(mut file) = OpenOptions::new().read(true).open(path) else {
        return String::new();
    };
    let mut content = String::new();
    if file.read_to_string(&mut content).is_err() {
        return String::new();
    }
    let mut lines = content.lines().collect::<Vec<_>>();
    if lines.len() > max_lines {
        lines = lines.split_off(lines.len() - max_lines);
    }
    lines.join("\n")
}

pub fn app_diagnostics() -> Result<DiagnosticsInfo, String> {
    let path = path_string().ok_or_else(|| "诊断日志尚未初始化".to_string())?;
    Ok(DiagnosticsInfo {
        path,
        recent: recent_lines(200),
    })
}
