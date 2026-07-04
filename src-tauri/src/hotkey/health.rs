use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;

/// Snapshot of hotkey registration health, exposed to the frontend via the
/// `hotkey_status` command and the `hotkey-registration-changed` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HotkeyStatus {
    pub ok: bool,
    pub error: Option<String>,
    pub shortcut: String,
}

/// Result of recording a registration attempt: the resulting status, plus
/// whether the status actually changed (ok/failed flag, shortcut, or error
/// detail). Callers use `changed` to decide whether to emit
/// `hotkey-registration-changed` — a repeated identical failure (e.g. the
/// settings watcher firing again on the same broken shortcut) must not spam
/// listeners, but a different shortcut or error while still failed must.
#[derive(Debug, Clone, PartialEq)]
pub struct HotkeyStatusChange {
    pub status: HotkeyStatus,
    pub changed: bool,
}

/// Process-wide hotkey registration health.
///
/// Deliberately NOT folded into `AppController`/`HotkeyService`: routing
/// registration outcomes through the controller mutex — which is already
/// locked from the global-shortcut handler, the settings-watcher thread, and
/// the `set_hotkey` command — would risk lock-ordering deadlocks for no real
/// benefit. This is a small independent piece of shared state (an
/// `AtomicBool` plus a couple of `Mutex<String>`s for the error/shortcut
/// detail) instead. It is set on registration failure and cleared on
/// registration success at all three registration sites: app startup,
/// `commands::set_hotkey`, and the settings-file watcher's hot-reload path.
pub struct HotkeyHealth {
    failed: AtomicBool,
    error: Mutex<Option<String>>,
    shortcut: Mutex<String>,
}

impl HotkeyHealth {
    pub fn new(initial_shortcut: &str) -> Self {
        Self {
            failed: AtomicBool::new(false),
            error: Mutex::new(None),
            shortcut: Mutex::new(initial_shortcut.to_string()),
        }
    }

    /// True if the most recent registration attempt failed. Sticky until the
    /// next successful registration clears it.
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    pub fn status(&self) -> HotkeyStatus {
        HotkeyStatus {
            ok: !self.is_failed(),
            error: self.error.lock().unwrap().clone(),
            shortcut: self.shortcut.lock().unwrap().clone(),
        }
    }

    /// Record the outcome of a registration attempt for `shortcut`.
    /// `error = None` means the registration succeeded (clears the flag);
    /// `Some(msg)` means it failed (sets the flag). Returns the resulting
    /// status plus whether any part of the status actually changed.
    pub fn record(&self, shortcut: &str, error: Option<String>) -> HotkeyStatusChange {
        let prev = self.status();
        let new_failed = error.is_some();
        self.failed.store(new_failed, Ordering::SeqCst);
        *self.error.lock().unwrap() = error.clone();
        *self.shortcut.lock().unwrap() = shortcut.to_string();
        let status = HotkeyStatus {
            ok: !new_failed,
            error,
            shortcut: shortcut.to_string(),
        };
        HotkeyStatusChange {
            changed: status != prev,
            status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_not_failed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        assert!(!h.is_failed());
        let s = h.status();
        assert!(s.ok);
        assert!(s.error.is_none());
        assert_eq!(s.shortcut, "Control+Shift+Space");
    }

    #[test]
    fn record_failure_sets_flag_and_reports_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let change = h.record("Control+Shift+Space", Some("already registered".to_string()));
        assert!(change.changed);
        assert!(!change.status.ok);
        assert_eq!(change.status.error.as_deref(), Some("already registered"));
        assert!(h.is_failed());
    }

    #[test]
    fn record_same_failure_twice_is_not_changed_second_time() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let _ = h.record("Control+Shift+Space", Some("busy".to_string()));
        let second = h.record("Control+Shift+Space", Some("busy".to_string()));
        assert!(!second.changed);
        assert!(!second.status.ok);
    }

    #[test]
    fn record_success_after_failure_clears_flag_and_reports_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let _ = h.record("Control+Shift+Space", Some("busy".to_string()));
        assert!(h.is_failed());
        let change = h.record("Control+Shift+Space", None);
        assert!(change.changed);
        assert!(change.status.ok);
        assert!(change.status.error.is_none());
        assert!(!h.is_failed());
    }

    #[test]
    fn record_success_after_success_is_not_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let change = h.record("Control+Shift+Space", None);
        assert!(!change.changed);
        assert!(change.status.ok);
    }

    #[test]
    fn status_reflects_latest_shortcut() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        h.record("Alt+D", None);
        assert_eq!(h.status().shortcut, "Alt+D");
    }

    #[test]
    fn failure_then_different_failure_reports_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let _ = h.record("Control+Shift+Space", Some("busy".to_string()));
        let second = h.record("Alt+D", Some("also busy".to_string()));
        // The ok/failed flag didn't flip, but the shortcut/error detail did —
        // listeners must hear about it or the UI shows a stale failure.
        assert!(second.changed);
        assert_eq!(second.status.shortcut, "Alt+D");
        assert_eq!(second.status.error.as_deref(), Some("also busy"));
        assert_eq!(h.status().shortcut, "Alt+D");
    }
}
