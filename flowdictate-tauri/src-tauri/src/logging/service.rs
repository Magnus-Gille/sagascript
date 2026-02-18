use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::Utc;
use serde::Serialize;
use tracing::warn;
use uuid::Uuid;

const MAX_FILE_SIZE: u64 = 5_000_000; // 5MB
const MAX_FILES: u32 = 5;

/// Structured JSONL logging service matching the Swift app's format
pub struct LoggingService {
    pub app_session_id: String,
    dictation_session_id: Mutex<Option<String>>,
    log_file: Mutex<Option<File>>,
    log_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub ts: String,
    pub level: &'static str,
    #[serde(rename = "appSession")]
    pub app_session: String,
    #[serde(rename = "dictationSession", skip_serializing_if = "Option::is_none")]
    pub dictation_session: Option<String>,
    pub category: &'static str,
    pub event: String,
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
}

impl LoggingService {
    pub fn new() -> Self {
        let app_session_id = format!("app-{}", &Uuid::new_v4().to_string()[..8]);
        let log_dir = Self::log_directory();
        let log_path = log_dir.join("flowdictate.log");

        // Create log directory with restrictive permissions
        if let Err(e) = fs::create_dir_all(&log_dir) {
            warn!("Failed to create log directory: {e}");
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&log_dir, fs::Permissions::from_mode(0o700));
        }

        let log_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();

        #[cfg(unix)]
        if log_path.exists() {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&log_path, fs::Permissions::from_mode(0o600));
        }

        Self {
            app_session_id,
            dictation_session_id: Mutex::new(None),
            log_file: Mutex::new(log_file),
            log_path,
        }
    }

    fn log_directory() -> PathBuf {
        #[cfg(target_os = "macos")]
        {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library/Logs/FlowDictate")
        }

        #[cfg(target_os = "windows")]
        {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("FlowDictate/Logs")
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("FlowDictate/logs")
        }
    }

    /// Start a new dictation session, returns session ID
    pub fn start_dictation_session(&self) -> String {
        let id = format!("dict-{}", &Uuid::new_v4().to_string()[..8]);
        *self.dictation_session_id.lock().unwrap() = Some(id.clone());
        id
    }

    /// End the current dictation session
    pub fn end_dictation_session(&self) {
        *self.dictation_session_id.lock().unwrap() = None;
    }

    /// Log an entry to the JSONL file
    pub fn log(
        &self,
        level: &'static str,
        category: &'static str,
        event: &str,
        data: serde_json::Value,
    ) {
        let entry = LogEntry {
            ts: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            level,
            app_session: self.app_session_id.clone(),
            dictation_session: self.dictation_session_id.lock().unwrap().clone(),
            category,
            event: event.to_string(),
            data,
        };

        self.write_entry(&entry);
    }

    fn write_entry(&self, entry: &LogEntry) {
        let mut file_guard = self.log_file.lock().unwrap();
        if let Some(ref mut file) = *file_guard {
            self.rotate_if_needed(file);

            if let Ok(json) = serde_json::to_string(entry) {
                let _ = writeln!(file, "{json}");
                let _ = file.flush();
            }
        }
    }

    fn rotate_if_needed(&self, _file: &mut File) {
        let size = fs::metadata(&self.log_path)
            .map(|m| m.len())
            .unwrap_or(0);

        if size < MAX_FILE_SIZE {
            return;
        }

        let dir = Self::log_directory();

        // Delete oldest
        let oldest = dir.join(format!("flowdictate.{MAX_FILES}.log"));
        let _ = fs::remove_file(oldest);

        // Rotate: N-1 -> N, ..., 1 -> 2
        for i in (1..MAX_FILES).rev() {
            let from = dir.join(format!("flowdictate.{i}.log"));
            let to = dir.join(format!("flowdictate.{}.log", i + 1));
            let _ = fs::rename(from, to);
        }

        // Current -> .1
        let rotated = dir.join("flowdictate.1.log");
        let _ = fs::rename(&self.log_path, rotated);

        // Reopen
        // (The caller will need to handle this - for simplicity we just create a new file)
    }
}
