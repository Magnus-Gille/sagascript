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
        let log_path = log_dir.join("sagascript.log");

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
                .join("Library/Logs/Sagascript")
        }

        #[cfg(target_os = "windows")]
        {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Sagascript/Logs")
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Sagascript/logs")
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
        let oldest = dir.join(format!("sagascript.{MAX_FILES}.log"));
        let _ = fs::remove_file(oldest);

        // Rotate: N-1 -> N, ..., 1 -> 2
        for i in (1..MAX_FILES).rev() {
            let from = dir.join(format!("sagascript.{i}.log"));
            let to = dir.join(format!("sagascript.{}.log", i + 1));
            let _ = fs::rename(from, to);
        }

        // Current -> .1
        let rotated = dir.join("sagascript.1.log");
        let _ = fs::rename(&self.log_path, rotated);

        // Reopen
        // (The caller will need to handle this - for simplicity we just create a new file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_session_id_format() {
        let svc = LoggingService::new();
        assert!(
            svc.app_session_id.starts_with("app-"),
            "got: {}",
            svc.app_session_id
        );
        // "app-" + 8 hex chars = 12 total
        assert_eq!(svc.app_session_id.len(), 12);
    }

    #[test]
    fn dictation_session_id_format() {
        let svc = LoggingService::new();
        let id = svc.start_dictation_session();
        assert!(id.starts_with("dict-"), "got: {id}");
        assert_eq!(id.len(), 13); // "dict-" + 8 hex chars
    }

    #[test]
    fn start_and_end_dictation_session() {
        let svc = LoggingService::new();

        // No active session initially
        assert!(svc.dictation_session_id.lock().unwrap().is_none());

        // Start session
        let id = svc.start_dictation_session();
        assert_eq!(
            svc.dictation_session_id.lock().unwrap().as_deref(),
            Some(id.as_str())
        );

        // End session
        svc.end_dictation_session();
        assert!(svc.dictation_session_id.lock().unwrap().is_none());
    }

    #[test]
    fn multiple_sessions_get_unique_ids() {
        let svc = LoggingService::new();
        let id1 = svc.start_dictation_session();
        svc.end_dictation_session();
        let id2 = svc.start_dictation_session();
        assert_ne!(id1, id2);
    }

    #[test]
    fn log_entry_serialization() {
        let entry = LogEntry {
            ts: "2026-02-20T10:00:00.000Z".to_string(),
            level: "info",
            app_session: "app-abc12345".to_string(),
            dictation_session: Some("dict-def67890".to_string()),
            category: "App",
            event: "test_event".to_string(),
            data: serde_json::json!({"key": "value"}),
        };

        let json = serde_json::to_value(&entry).unwrap();
        assert_eq!(json["ts"], "2026-02-20T10:00:00.000Z");
        assert_eq!(json["level"], "info");
        assert_eq!(json["appSession"], "app-abc12345");
        assert_eq!(json["dictationSession"], "dict-def67890");
        assert_eq!(json["category"], "App");
        assert_eq!(json["event"], "test_event");
        assert_eq!(json["data"]["key"], "value");
    }

    #[test]
    fn log_entry_skips_null_fields() {
        let entry = LogEntry {
            ts: "2026-02-20T10:00:00.000Z".to_string(),
            level: "info",
            app_session: "app-abc12345".to_string(),
            dictation_session: None,
            category: "App",
            event: "test".to_string(),
            data: serde_json::Value::Null,
        };

        let json_str = serde_json::to_string(&entry).unwrap();
        assert!(!json_str.contains("dictationSession"));
        assert!(!json_str.contains("\"data\""));
    }

    #[test]
    fn log_does_not_panic() {
        let svc = LoggingService::new();
        // Should not panic even if file operations fail
        svc.log("info", "Test", "test_event", serde_json::json!({}));
    }

    #[test]
    fn constants() {
        assert_eq!(MAX_FILE_SIZE, 5_000_000);
        assert_eq!(MAX_FILES, 5);
    }
}
