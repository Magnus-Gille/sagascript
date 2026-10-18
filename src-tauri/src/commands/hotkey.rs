//! Shortcut configuration: changing profiles and modes, retrying a failed
//! registration, and reporting the registration health.

use sagascript_core::settings::{validate_hotkey, HotkeyMode, HotkeyProfile, Settings};
use tauri::State;
use tracing::{error, info};

#[cfg(any(target_os = "macos", test))]
use super::main_thread::MainThreadReply;
use super::{lock_controller, try_persist_and_replace, SharedController};
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
    let profiles =
        profiles_with_default_shortcut(lock_controller(&controller).settings(), shortcut)?;
    set_hotkey_profiles(app, controller, health, profiles)
        .await
        .map(|_| ())
}

/// The profile list `set_hotkey` applies: `shortcut` becomes the primary
/// shortcut of the default profile (the one called `default`, else the first),
/// the profile `config set hotkey` changes too (#373).
pub(super) fn profiles_with_default_shortcut(
    settings: &Settings,
    shortcut: String,
) -> Result<Vec<HotkeyProfile>, CommandError> {
    validate_hotkey(&shortcut).map_err(|error| CommandError::HotkeyInvalid(error.to_string()))?;
    let mut profiles = settings.resolved_hotkey_profiles();
    let profile_index = HotkeyProfile::default_index(&profiles);
    profiles[profile_index].set_primary_shortcut(shortcut);
    Ok(profiles)
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
///
/// The one setter that must replace the controller's whole copy by the file's,
/// because it is the caller that has just registered the file's shortcuts with
/// the OS: the controller adopting them is what makes them the operational
/// ones. No other setter may install shortcuts in the controller.
pub(super) fn persist_hotkey_change(
    controller: &SharedController,
    update: &HotkeyChange,
    shortcuts: &[String],
) -> Result<Settings, CommandError> {
    try_persist_and_replace(controller, |settings| {
        update.apply_registered(settings, shortcuts)
    })
    .map(|(_, written)| written)
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

/// How long [`retry_hotkey_registration`] waits for the main thread to
/// reinstall the AppKit event monitor. The install is immediate; the bound only
/// keeps a main thread that is stuck from parking a worker.
#[cfg(target_os = "macos")]
const MONITOR_INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// What `retry_hotkey_registration` does with the main thread's answer to the
/// monitor reinstall.
///
/// - Installed, or the install failed: go on. A failed install is logged, and
///   the registration that follows reports the monitor as unavailable through
///   the normal health path.
/// - The main thread dropped the answer ([`MainThreadReply::Disconnected`]: the
///   app is shutting down, or the closure panicked): go on as well; registering
///   reports what it finds, and there is nothing to wait for.
/// - No answer in time: the install stays queued on the main thread and runs
///   when the thread is free (it is idempotent). Registering now would fail
///   with "restart Sagascript" for a monitor that is about to exist and would
///   record that as the registration health, so the call ends here with a
///   message to try again; the retry finds the monitor installed. The error is
///   `busy` (retryable), not `hotkey_change_failed`: nothing was attempted, so
///   the registration health has no diagnostic for it, and the settings window
///   shows this message itself (by code, never by its text).
#[cfg(any(target_os = "macos", test))]
fn after_monitor_install(reply: MainThreadReply<Result<(), String>>) -> Result<(), CommandError> {
    match reply {
        MainThreadReply::Done(Err(error)) => {
            tracing::warn!("F13-F24 event monitor reinstall failed: {error}");
            Ok(())
        }
        MainThreadReply::Done(Ok(())) | MainThreadReply::Disconnected => Ok(()),
        MainThreadReply::TimedOut => {
            tracing::warn!("F13-F24 event monitor reinstall was not confirmed in time");
            Err(CommandError::Busy(
                "The macOS F13-F24 event monitor is still starting; try again in a moment"
                    .to_string(),
            ))
        }
    }
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
        // The main thread runs the install promptly. Wait for it off the
        // worker and not for ever (ADR 0003): a busy main thread, or one that
        // is gone because the app is shutting down, must not park a Tokio
        // worker. `after_monitor_install` says what each answer means.
        let reply = super::main_thread::await_main_thread(MONITOR_INSTALL_TIMEOUT, |reply| {
            app.run_on_main_thread(move || {
                let result = crate::hotkey::install_bare_function_key_monitor(&app_for_install);
                let _ = reply.send(result);
            })
            .map_err(|error| error.to_string())
        })
        .await?;
        after_monitor_install(reply)?;
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

#[cfg(test)]
#[path = "hotkey_tests.rs"]
mod tests;
