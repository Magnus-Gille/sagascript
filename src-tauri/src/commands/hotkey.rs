//! Shortcut configuration: changing profiles and modes, retrying a failed
//! registration, and reporting the registration health.

use sagascript_core::settings::{validate_hotkey, HotkeyMode, HotkeyProfile, Settings};
use tauri::State;
use tracing::{error, info};

use super::{lock_controller, try_persist_and_sync, SharedController};
use crate::command_error::CommandError;
use crate::events::EmitEvent;
use crate::hotkey::configuration::HotkeyChange;
use crate::hotkey::{HotkeyHealth, HotkeyStatus, OperationalHotkey};

#[tauri::command]
pub async fn get_active_hotkey_profile(
    controller: State<'_, SharedController>,
) -> Result<Option<HotkeyProfile>, String> {
    Ok(lock_controller(&controller)
        .active_hotkey_profile()
        .cloned())
}

#[tauri::command]
pub async fn set_hotkey_mode(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    mode: HotkeyMode,
) -> Result<(), CommandError> {
    apply_hotkey_change(app, controller, health, HotkeyChange::Mode(mode)).map(|_| ())
}

#[tauri::command]
pub async fn set_hotkey(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    shortcut: String,
) -> Result<(), CommandError> {
    validate_hotkey(&shortcut).map_err(CommandError::HotkeyInvalid)?;
    let mut profiles = lock_controller(&controller)
        .settings()
        .resolved_hotkey_profiles();
    let profile_index = profiles
        .iter()
        .position(|profile| profile.id == "default")
        .unwrap_or(0);
    profiles[profile_index].set_primary_shortcut(shortcut);
    set_hotkey_profiles(app, controller, health, profiles)
        .await
        .map(|_| ())
}

#[tauri::command]
pub async fn set_hotkey_profiles(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    profiles: Vec<HotkeyProfile>,
) -> Result<Vec<String>, CommandError> {
    apply_hotkey_change(app, controller, health, HotkeyChange::Profiles(profiles))
}

pub(crate) struct HotkeyConfigurationLease<'a>(&'a SharedController);

impl Drop for HotkeyConfigurationLease<'_> {
    fn drop(&mut self) {
        lock_controller(self.0).end_hotkey_configuration_change();
    }
}

pub(crate) fn acquire_hotkey_configuration(
    controller: &SharedController,
) -> Result<HotkeyConfigurationLease<'_>, CommandError> {
    if !lock_controller(controller).begin_hotkey_configuration_change() {
        return Err(CommandError::Busy(
            "Finish or cancel the active dictation before changing shortcuts".into(),
        ));
    }
    Ok(HotkeyConfigurationLease(controller))
}

/// The persistence half of a hotkey change, run once the new shortcuts are
/// registered: the file and the controller, nothing else. A refusal or a failed
/// write leaves both as they were.
fn persist_hotkey_change(
    controller: &SharedController,
    update: &HotkeyChange,
    shortcuts: &[String],
) -> Result<Settings, CommandError> {
    try_persist_and_sync(controller, |settings| {
        update.apply_registered(settings, shortcuts)
    })
}

/// What a failed hotkey change does about the shortcuts registered before it.
#[derive(Debug, PartialEq, Eq)]
enum Rollback<'a> {
    /// Register these again.
    Restore(&'a [String]),
    /// Nothing was registered, so there is nothing to put back.
    NothingActive,
    /// The previous state is unknown. A change is refused before it touches the
    /// OS in that state, so a rollback does not start from here; if one ever
    /// did, nothing is registered again and nothing is claimed about what is.
    StateUnknown,
}

fn rollback_for(previous: &OperationalHotkey) -> Rollback<'_> {
    match previous {
        OperationalHotkey::Registered(shortcuts) => Rollback::Restore(shortcuts),
        OperationalHotkey::Inactive => Rollback::NothingActive,
        OperationalHotkey::Unknown => Rollback::StateUnknown,
    }
}

fn apply_hotkey_change(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    update: HotkeyChange,
) -> Result<Vec<String>, CommandError> {
    let _transition = health.transition_guard();
    let _recording_lease = acquire_hotkey_configuration(&controller)?;
    let old_settings = lock_controller(&controller).settings().clone();
    let (candidate, notices) = update
        .prepare_with_notices(&sagascript_core::settings::store::load())
        .map_err(CommandError::HotkeyInvalid)?;
    let new_shortcuts = candidate.resolved_shortcuts();
    let new_primary = candidate.hotkey.clone();
    let old_shortcut = old_settings.hotkey.clone();
    let old_operational = health.operational_hotkey();
    if old_operational == OperationalHotkey::Unknown {
        return Err(CommandError::HotkeyChangeFailed(
            "Hotkey registration state is unknown after an earlier OS error; restart Sagascript before changing profiles"
                .to_string(),
        ));
    }

    let old_shortcuts = old_settings.resolved_shortcuts();
    if new_shortcuts == old_shortcuts
        && old_operational.matches(&new_shortcuts)
        && !health.is_failed()
    {
        let persisted = persist_hotkey_change(&controller, &update, &new_shortcuts)
            .map_err(|error| CommandError::HotkeyChangeFailed(error.to_string()))?;
        let change = health.record(&new_primary, None, old_operational);
        if change.changed {
            let _ = app.emit_event(
                crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                &change.status,
            );
        }
        crate::update_profiles_menu(&app, &persisted.resolved_hotkey_profiles());
        return Ok(notices);
    }

    if let OperationalHotkey::Registered(old_shortcuts) = &old_operational {
        if let Err(error) = crate::hotkey::unregister_shortcuts(&app, old_shortcuts) {
            error!("Failed to unregister operational hotkeys: {error}");
            let change = health.record(
                &old_shortcut,
                Some(format!(
                    "failed to unregister active hotkeys: {error}; operational state is unknown"
                )),
                OperationalHotkey::Unknown,
            );
            if change.changed {
                let _ = app.emit_event(
                    crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                    &change.status,
                );
            }
            return Err(CommandError::HotkeyChangeFailed(format!(
                "Failed to unregister active hotkeys: {error}"
            )));
        }
    }

    if let Err(error) = crate::hotkey::register_shortcuts(&app, &new_shortcuts) {
        if let Err(cleanup_error) = crate::hotkey::unregister_shortcuts(&app, &new_shortcuts) {
            let change = health.record(
                &old_shortcut,
                Some(format!("new registration failed: {error}; partial registration cleanup failed: {cleanup_error}; operational state is unknown")),
                OperationalHotkey::Unknown,
            );
            if change.changed {
                let _ = app.emit_event(
                    crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                    &change.status,
                );
            }
            return Err(CommandError::HotkeyChangeFailed(format!(
                "Failed to register hotkey profiles: {error}; cleanup failed: {cleanup_error}"
            )));
        }
        let change = match rollback_for(&old_operational) {
            Rollback::Restore(old_shortcuts) => match crate::hotkey::register_shortcuts(&app, old_shortcuts) {
                Ok(()) => health.record(&old_shortcut, None, old_operational.clone()),
                Err(rollback_error) => {
                    health.record(
                        &old_shortcut,
                        Some(format!("new registration failed: {error}; restoring previous hotkeys also failed: {rollback_error}")),
                        OperationalHotkey::Inactive,
                    )
                }
            },
            Rollback::NothingActive => health.record(
                &old_shortcut,
                Some("no previous hotkey was active".to_string()),
                OperationalHotkey::Inactive,
            ),
            Rollback::StateUnknown => health.record(
                &old_shortcut,
                Some(format!("new registration failed: {error}; the previous hotkey state was unknown")),
                OperationalHotkey::Unknown,
            ),
        };
        if change.changed {
            let _ = app.emit_event(
                crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                &change.status,
            );
        }
        return Err(CommandError::HotkeyChangeFailed(format!(
            "Failed to register hotkey profiles: {error}"
        )));
    }

    health.record(
        &old_shortcut,
        Some("hotkey profile change is pending persistence".to_string()),
        OperationalHotkey::registered_many(&new_shortcuts),
    );

    let persisted = match persist_hotkey_change(&controller, &update, &new_shortcuts) {
        Ok(settings) => settings,
        Err(save_error) => {
            let unregister_error = crate::hotkey::unregister_shortcuts(&app, &new_shortcuts).err();
            let rollback_error = if unregister_error.is_none() {
                match rollback_for(&old_operational) {
                    Rollback::Restore(old_shortcuts) => {
                        crate::hotkey::register_shortcuts(&app, old_shortcuts).err()
                    }
                    Rollback::NothingActive | Rollback::StateUnknown => None,
                }
            } else {
                None
            };
            let unknown = unregister_error.is_some();
            let restored_operational = if unknown {
                OperationalHotkey::Unknown
            } else if rollback_error.is_some() {
                OperationalHotkey::Inactive
            } else {
                old_operational.clone()
            };
            let health_error = unknown
                .then(|| {
                    "failed to unregister unpersisted hotkeys; operational state is unknown"
                        .to_string()
                })
                .or_else(|| {
                    rollback_error
                        .as_ref()
                        .map(|error| format!("failed to restore previous hotkeys: {error}"))
                });
            let change = health.record(&old_shortcut, health_error, restored_operational);
            if change.changed {
                let _ = app.emit_event(
                    crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                    &change.status,
                );
            }
            return Err(CommandError::HotkeyChangeFailed(format!(
                "Failed to persist hotkey profiles: {save_error}"
            )));
        }
    };

    info!("Hotkey set to: {}", persisted.hotkey);

    let change = health.record(
        &new_primary,
        None,
        OperationalHotkey::registered_many(&new_shortcuts),
    );
    if change.changed {
        let _ = app.emit_event(
            crate::events::event::HOTKEY_REGISTRATION_CHANGED,
            &change.status,
        );
    }

    crate::update_profiles_menu(&app, &persisted.resolved_hotkey_profiles());

    info!(
        "Hotkey profiles changed: {} registered",
        new_shortcuts.len()
    );
    Ok(notices)
}

/// Retry the shortcuts currently persisted on disk.
///
/// This is used after macOS grants Accessibility while the app is already
/// running. Reading the file again matters for CLI-driven changes: the file
/// can contain a requested bare F-key while the controller deliberately keeps
/// the previous operational shortcut after registration failed closed.
///
/// If the AppKit event monitor itself was never installed (setup failure),
/// reinstall it here on the macOS main thread before re-registering, so a
/// retry can recover without an app restart. Install and registration
/// failures still surface through the normal health path below.
#[tauri::command]
pub async fn retry_hotkey_registration(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
) -> Result<(), CommandError> {
    #[cfg(target_os = "macos")]
    if !crate::hotkey::bare_function_key_monitor_installed() {
        let app_for_install = app.clone();
        let (installed_tx, installed_rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let result = crate::hotkey::install_bare_function_key_monitor(&app_for_install);
            let _ = installed_tx.send(result);
        })
        .map_err(|error| error.to_string())?;
        // The main thread runs the install promptly; a disconnected channel
        // means the app is shutting down, so fall through and let the
        // registration attempt below report the monitor as unavailable.
        if let Ok(Err(error)) = installed_rx.recv() {
            tracing::warn!("F13-F24 event monitor reinstall failed: {error}");
        }
    }
    let profiles = sagascript_core::settings::store::load().resolved_hotkey_profiles();
    set_hotkey_profiles(app, controller, health, profiles)
        .await
        .map(|_| ())
}

/// Current hotkey registration health — whether the last registration
/// attempt (at startup, from this command, or from the settings-file
/// watcher's hot-reload) actually succeeded. Reads the process-wide flag
/// rather than querying the global-shortcut plugin's `is_registered()`,
/// which only tells you *a* shortcut is bound, not whether *our* most recent
/// attempt to bind it succeeded.
#[tauri::command]
pub async fn hotkey_status(health: State<'_, HotkeyHealth>) -> Result<HotkeyStatus, String> {
    Ok(health.status())
}
