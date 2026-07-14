use std::sync::{Mutex, MutexGuard};

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

/// What this process knows about the shortcut currently registered with the OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationalHotkey {
    Inactive,
    Registered(String),
    Unknown,
}

impl OperationalHotkey {
    pub fn registered(shortcut: &str) -> Self {
        Self::Registered(shortcut.to_string())
    }
}

/// Process-wide hotkey registration health.
///
/// Deliberately NOT folded into `AppController`/`HotkeyService`: routing
/// registration outcomes through the controller mutex — which is already
/// locked from the global-shortcut handler, the settings-watcher thread, and
/// the `set_hotkey` command — would risk lock-ordering deadlocks for no real
/// benefit. Requested, operational, and error state share one mutex so status
/// readers cannot observe a torn transition. A second mutex serializes the OS
/// unregister/register transaction across GUI and settings-watcher callers.
pub struct HotkeyHealth {
    state: Mutex<HotkeyState>,
    transition: Mutex<()>,
}

struct HotkeyState {
    error: Option<String>,
    shortcut: String,
    operational_hotkey: OperationalHotkey,
}

impl HotkeyHealth {
    pub fn new(initial_shortcut: &str) -> Self {
        Self {
            state: Mutex::new(HotkeyState {
                error: None,
                shortcut: initial_shortcut.to_string(),
                operational_hotkey: OperationalHotkey::Inactive,
            }),
            transition: Mutex::new(()),
        }
    }

    /// Serialize process-global unregister/register transactions from the GUI
    /// command and the external-settings watcher.
    pub fn transition_guard(&self) -> MutexGuard<'_, ()> {
        self.transition.lock().unwrap()
    }

    /// The shortcut currently registered with the OS, if any. This can differ
    /// from the saved/requested shortcut after a failed hot-reload falls back
    /// to the previous binding.
    pub fn operational_hotkey(&self) -> OperationalHotkey {
        self.state.lock().unwrap().operational_hotkey.clone()
    }

    /// True if the most recent registration attempt failed. Sticky until the
    /// next successful registration clears it.
    pub fn is_failed(&self) -> bool {
        self.state.lock().unwrap().error.is_some()
    }

    pub fn status(&self) -> HotkeyStatus {
        let state = self.state.lock().unwrap();
        HotkeyStatus {
            ok: state.error.is_none(),
            error: state.error.clone(),
            shortcut: state.shortcut.clone(),
        }
    }

    /// Atomically record the requested shortcut, any error, and the shortcut
    /// known to be registered with the OS.
    /// `error = None` means the registration succeeded (clears the flag);
    /// `Some(msg)` means it failed (sets the flag). Returns the resulting
    /// status plus whether any part of the status actually changed.
    pub fn record(
        &self,
        shortcut: &str,
        error: Option<String>,
        operational_hotkey: OperationalHotkey,
    ) -> HotkeyStatusChange {
        let mut state = self.state.lock().unwrap();
        let prev = HotkeyStatus {
            ok: state.error.is_none(),
            error: state.error.clone(),
            shortcut: state.shortcut.clone(),
        };
        state.error = error.clone();
        state.shortcut = shortcut.to_string();
        state.operational_hotkey = operational_hotkey;
        let status = HotkeyStatus {
            ok: error.is_none(),
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
        assert_eq!(h.operational_hotkey(), OperationalHotkey::Inactive);
    }

    #[test]
    fn record_failure_sets_flag_and_reports_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let change = h.record(
            "Control+Shift+Space",
            Some("already registered".to_string()),
            OperationalHotkey::Inactive,
        );
        assert!(change.changed);
        assert!(!change.status.ok);
        assert_eq!(change.status.error.as_deref(), Some("already registered"));
        assert!(h.is_failed());
    }

    #[test]
    fn record_same_failure_twice_is_not_changed_second_time() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let _ = h.record(
            "Control+Shift+Space",
            Some("busy".to_string()),
            OperationalHotkey::Inactive,
        );
        let second = h.record(
            "Control+Shift+Space",
            Some("busy".to_string()),
            OperationalHotkey::Inactive,
        );
        assert!(!second.changed);
        assert!(!second.status.ok);
    }

    #[test]
    fn record_success_after_failure_clears_flag_and_reports_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let _ = h.record(
            "Control+Shift+Space",
            Some("busy".to_string()),
            OperationalHotkey::Inactive,
        );
        assert!(h.is_failed());
        let change = h.record(
            "Control+Shift+Space",
            None,
            OperationalHotkey::registered("Control+Shift+Space"),
        );
        assert!(change.changed);
        assert!(change.status.ok);
        assert!(change.status.error.is_none());
        assert!(!h.is_failed());
    }

    #[test]
    fn record_success_after_success_is_not_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let change = h.record(
            "Control+Shift+Space",
            None,
            OperationalHotkey::Inactive,
        );
        assert!(!change.changed);
        assert!(change.status.ok);
    }

    #[test]
    fn status_reflects_latest_shortcut() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        h.record("Alt+D", None, OperationalHotkey::registered("Alt+D"));
        assert_eq!(h.status().shortcut, "Alt+D");
    }

    #[test]
    fn failure_then_different_failure_reports_changed() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let _ = h.record(
            "Control+Shift+Space",
            Some("busy".to_string()),
            OperationalHotkey::Inactive,
        );
        let second = h.record(
            "Alt+D",
            Some("also busy".to_string()),
            OperationalHotkey::Inactive,
        );
        // The ok/failed flag didn't flip, but the shortcut/error detail did —
        // listeners must hear about it or the UI shows a stale failure.
        assert!(second.changed);
        assert_eq!(second.status.shortcut, "Alt+D");
        assert_eq!(second.status.error.as_deref(), Some("also busy"));
        assert_eq!(h.status().shortcut, "Alt+D");
    }

    #[test]
    fn operational_shortcut_is_tracked_separately_from_requested_health() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        h.record(
            "Control+ScrollLock",
            Some("unsupported".to_string()),
            OperationalHotkey::registered("Control+Shift+Space"),
        );

        assert_eq!(h.status().shortcut, "Control+ScrollLock");
        assert_eq!(
            h.operational_hotkey(),
            OperationalHotkey::registered("Control+Shift+Space")
        );

        h.record(
            "Shift+F6",
            None,
            OperationalHotkey::registered("Shift+F6"),
        );
        assert_eq!(
            h.operational_hotkey(),
            OperationalHotkey::registered("Shift+F6")
        );
        assert!(h.status().ok);
    }

    #[test]
    fn unknown_operational_state_is_not_treated_as_inactive() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        h.record(
            "Control+Shift+Space",
            Some("unregister failed; restart required".to_string()),
            OperationalHotkey::Unknown,
        );

        assert_eq!(h.operational_hotkey(), OperationalHotkey::Unknown);
        assert_ne!(h.operational_hotkey(), OperationalHotkey::Inactive);
        assert!(h.is_failed());
    }
}
