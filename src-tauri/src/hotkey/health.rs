use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};

use sagascript_core::sync::lock_or_recover;
use serde::Serialize;

// Recovered as-is after a panic (ADR 0005): `record` assigns the three fields
// with no panic point between them, and `transition` guards no data. The
// settings watcher holds both while re-registering shortcuts.
const STATE_LOCK: &str = "hotkey_health.state";
const TRANSITION_LOCK: &str = "hotkey_health.transition";

/// Snapshot of hotkey registration health, exposed to the frontend via the
/// `hotkey_status` command and the `hotkey-registration-changed` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HotkeyStatus {
    pub ok: bool,
    pub error: Option<String>,
    pub shortcut: String,
    pub shortcuts: Vec<String>,
    /// The settings watcher re-registers shortcuts after external changes;
    /// its health is reported here (#359). Not part of `ok`.
    pub settings_watcher: SettingsWatcherStatus,
}

/// Health of the settings hot-reload watcher thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SettingsWatcherStatus {
    /// The watcher's event loop is running.
    pub running: bool,
    /// Panics caught while hot-reloading a change; the watcher kept running.
    pub recovered_panics: u32,
}

/// Lock-free health counters written by the settings watcher thread.
#[derive(Default)]
pub struct SettingsWatcherHealth {
    running: AtomicBool,
    recovered_panics: AtomicU32,
}

impl SettingsWatcherHealth {
    pub fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::SeqCst);
    }

    /// Count a caught panic; returns the new total.
    pub fn record_recovered_panic(&self) -> u32 {
        self.recovered_panics
            .fetch_add(1, Ordering::SeqCst)
            .saturating_add(1)
    }

    pub fn status(&self) -> SettingsWatcherStatus {
        SettingsWatcherStatus {
            running: self.running.load(Ordering::SeqCst),
            recovered_panics: self.recovered_panics.load(Ordering::SeqCst),
        }
    }
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
    Registered(Vec<String>),
    Unknown,
}

impl OperationalHotkey {
    #[cfg(test)]
    pub fn registered(shortcut: &str) -> Self {
        Self::Registered(vec![shortcut.to_string()])
    }

    pub fn registered_many(shortcuts: &[String]) -> Self {
        Self::Registered(shortcuts.to_vec())
    }

    pub fn matches(&self, shortcuts: &[String]) -> bool {
        matches!(self, Self::Registered(registered) if registered == shortcuts)
    }
}

/// Process-wide hotkey registration health.
///
/// Deliberately NOT folded into `AppController`: routing
/// registration outcomes through the controller mutex — which is already
/// locked from the global-shortcut handler, the settings-watcher thread, and
/// the `set_hotkey` command — would risk lock-ordering deadlocks for no real
/// benefit. Requested, operational, and error state share one mutex so status
/// readers cannot observe a torn transition. A second mutex serializes the OS
/// unregister/register transaction across GUI and settings-watcher callers.
pub struct HotkeyHealth {
    state: Mutex<HotkeyState>,
    transition: Mutex<()>,
    settings_watcher: SettingsWatcherHealth,
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
            settings_watcher: SettingsWatcherHealth::default(),
        }
    }

    pub fn settings_watcher(&self) -> &SettingsWatcherHealth {
        &self.settings_watcher
    }

    /// Guard for an unregister/register sequence. If the thread unwinds from
    /// a panic before the guard is dropped normally, the OS registration is
    /// recorded as unknown (#359), as after an unregister error.
    pub fn registration_change(&self) -> RegistrationChange<'_> {
        RegistrationChange { health: self }
    }

    /// Serialize process-global unregister/register transactions from the GUI
    /// command and the external-settings watcher.
    pub fn transition_guard(&self) -> MutexGuard<'_, ()> {
        lock_or_recover(&self.transition, TRANSITION_LOCK)
    }

    /// The shortcut currently registered with the OS, if any. This can differ
    /// from the saved/requested shortcut after a failed hot-reload falls back
    /// to the previous binding.
    pub fn operational_hotkey(&self) -> OperationalHotkey {
        lock_or_recover(&self.state, STATE_LOCK)
            .operational_hotkey
            .clone()
    }

    /// True if the most recent registration attempt failed. Sticky until the
    /// next successful registration clears it.
    pub fn is_failed(&self) -> bool {
        lock_or_recover(&self.state, STATE_LOCK).error.is_some()
    }

    pub fn status(&self) -> HotkeyStatus {
        let state = lock_or_recover(&self.state, STATE_LOCK);
        HotkeyStatus {
            ok: state.error.is_none(),
            error: state.error.clone(),
            shortcut: state.shortcut.clone(),
            shortcuts: requested_shortcuts(&state.shortcut, &state.operational_hotkey),
            settings_watcher: self.settings_watcher.status(),
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
        let mut state = lock_or_recover(&self.state, STATE_LOCK);
        // One snapshot for both sides: watcher health never counts as a
        // registration change.
        let settings_watcher = self.settings_watcher.status();
        let prev = HotkeyStatus {
            ok: state.error.is_none(),
            error: state.error.clone(),
            shortcut: state.shortcut.clone(),
            shortcuts: requested_shortcuts(&state.shortcut, &state.operational_hotkey),
            settings_watcher,
        };
        state.error = error.clone();
        state.shortcut = shortcut.to_string();
        state.operational_hotkey = operational_hotkey.clone();
        let status = HotkeyStatus {
            ok: error.is_none(),
            error,
            shortcut: shortcut.to_string(),
            shortcuts: requested_shortcuts(shortcut, &operational_hotkey),
            settings_watcher,
        };
        HotkeyStatusChange {
            changed: status != prev,
            status,
        }
    }
}

/// See [`HotkeyHealth::registration_change`].
pub struct RegistrationChange<'a> {
    health: &'a HotkeyHealth,
}

impl Drop for RegistrationChange<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        // Runs while unwinding: only poison-tolerant locking, no panics.
        let shortcut = lock_or_recover(&self.health.state, STATE_LOCK)
            .shortcut
            .clone();
        self.health.record(
            &shortcut,
            Some(
                "A hotkey change was interrupted by an internal error; restart Sagascript to re-register shortcuts"
                    .to_string(),
            ),
            OperationalHotkey::Unknown,
        );
    }
}

fn requested_shortcuts(primary: &str, operational: &OperationalHotkey) -> Vec<String> {
    match operational {
        OperationalHotkey::Registered(shortcuts) => shortcuts.clone(),
        OperationalHotkey::Inactive | OperationalHotkey::Unknown => vec![primary.to_string()],
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
        let change = h.record("Control+Shift+Space", None, OperationalHotkey::Inactive);
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
    fn status_reports_every_registered_profile_shortcut() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let shortcuts = vec![
            "Control+Shift+Space".to_string(),
            "Option+Space".to_string(),
        ];
        h.record(
            &shortcuts[0],
            None,
            OperationalHotkey::registered_many(&shortcuts),
        );

        assert_eq!(h.status().shortcuts, shortcuts);
        assert_eq!(
            h.operational_hotkey(),
            OperationalHotkey::Registered(shortcuts)
        );
    }

    #[test]
    fn operational_match_requires_the_complete_registered_shortcut_set() {
        let shortcuts = vec!["F13".to_string(), "Control+Space".to_string()];
        let operational = OperationalHotkey::registered_many(&shortcuts);

        assert!(operational.matches(&shortcuts));
        assert!(!operational.matches(&["F13".to_string()]));
        assert!(!OperationalHotkey::Inactive.matches(&shortcuts));
        assert!(!OperationalHotkey::Unknown.matches(&shortcuts));
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

        h.record("Shift+F6", None, OperationalHotkey::registered("Shift+F6"));
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

    #[test]
    fn status_reports_settings_watcher_health_without_flagging_a_change() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        let first = h.record("Control+Shift+Space", None, OperationalHotkey::Inactive);
        assert_eq!(
            first.status.settings_watcher,
            SettingsWatcherStatus {
                running: false,
                recovered_panics: 0
            }
        );

        h.settings_watcher().set_running(true);
        assert_eq!(h.settings_watcher().record_recovered_panic(), 1);
        let watcher = h.status().settings_watcher;
        assert!(watcher.running);
        assert_eq!(watcher.recovered_panics, 1);
        let again = h.record("Control+Shift+Space", None, OperationalHotkey::Inactive);
        assert!(
            !again.changed,
            "watcher health is not a registration change"
        );
        assert_eq!(again.status.settings_watcher, watcher);
    }

    #[test]
    fn registration_change_marks_registration_unknown_only_when_unwinding() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        h.record(
            "Control+Shift+Space",
            None,
            OperationalHotkey::registered("Control+Shift+Space"),
        );

        drop(h.registration_change());
        assert!(h.status().ok, "a completed change leaves health alone");

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _change = h.registration_change();
            panic!("injected panic between unregister and register (#359)");
        }));
        assert!(unwound.is_err());
        assert_eq!(h.operational_hotkey(), OperationalHotkey::Unknown);
        assert!(h.is_failed());
        assert_eq!(h.status().shortcut, "Control+Shift+Space");
    }

    /// The settings watcher holds `transition_guard()` while it re-registers
    /// shortcuts; a panic there must not break every later hotkey change.
    #[test]
    fn health_keeps_working_after_a_panic_poisoned_its_locks() {
        let h = HotkeyHealth::new("Control+Shift+Space");
        std::thread::scope(|scope| {
            let panicked = scope
                .spawn(|| {
                    let _transition = h.transition_guard();
                    let _state = h.state.lock().unwrap();
                    panic!("injected panic during a hotkey transition (#359)");
                })
                .join();
            assert!(panicked.is_err());
        });
        assert!(h.transition.is_poisoned() && h.state.is_poisoned());

        drop(h.transition_guard());
        let change = h.record("Alt+D", None, OperationalHotkey::registered("Alt+D"));
        assert!(change.status.ok);
        assert_eq!(h.status().shortcut, "Alt+D");
    }
}
