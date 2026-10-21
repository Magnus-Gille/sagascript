//! Shortcut configuration: changing profiles and modes, retrying a failed
//! registration, and reporting the registration health.

use sagascript_core::settings::{validate_hotkey, HotkeyMode, HotkeyProfile, Settings};
use std::fmt::Display;
use tauri::State;
use tracing::{error, info};

#[cfg(any(target_os = "macos", test))]
use super::main_thread::MainThreadReply;
use super::{lock_controller, try_persist_and_replace, SharedController};
use crate::command_error::CommandError;
use crate::events::EmitEvent;
use crate::hotkey::configuration::HotkeyChange;
use crate::hotkey::transition::{self, RollbackOutcome, TransitionFailureCause};
use crate::hotkey::{HotkeyHealth, HotkeyStatus, NativeRegistrar, OperationalHotkey};

#[tauri::command]
pub async fn get_active_hotkey_profile(
    controller: State<'_, SharedController>,
) -> Result<Option<HotkeyProfile>, CommandError> {
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
/// The one setter that replaces the controller's whole copy by the file's,
/// because it is the caller that has just registered the file's shortcuts with
/// the OS: the controller adopting them is what makes them the operational
/// ones. No other setter may install shortcuts in the controller; each mirrors
/// the fields it writes (`persist_tests.rs` has the rule of every one).
pub(super) fn persist_hotkey_change(
    controller: &SharedController,
    update: &HotkeyChange,
    shortcuts: &[String],
) -> Result<Settings, CommandError> {
    try_persist_and_replace(controller, |settings| {
        update.apply_registered(settings, shortcuts)
    })
    .map(|persisted| persisted.current)
}

/// The settings `update` would produce from `stored`, with the notices about
/// profile models it reset, or the refusal.
///
/// A refusal rejects with the code of its [`SettingsError`]
/// (`dictionary_blocks_language_change`, `profile_invalid`, `model_incompatible`,
/// …), so the Settings window can tell a blocked language change from a shortcut
/// that cannot be used (ADR 0006).
fn prepare_candidate(
    update: &HotkeyChange,
    stored: &Settings,
) -> Result<(Settings, Vec<String>), CommandError> {
    update
        .prepare_with_notices(stored)
        .map_err(CommandError::Settings)
}

fn healthy_shortcuts_are_identical(
    new: &[String],
    old: &[String],
    operational: &OperationalHotkey,
    registration_failed: bool,
) -> bool {
    new == old && operational.matches(new) && !registration_failed
}

fn command_registration_failure_diagnostics(
    cause: TransitionFailureCause,
) -> (Option<String>, String) {
    match cause {
        TransitionFailureCause::PreviousUnknown => {
            let message = "Hotkey registration state is unknown after an earlier OS error; restart Sagascript before changing profiles".to_string();
            (Some(message.clone()), message)
        }
        TransitionFailureCause::UnregisterOld { error } => (
            Some(format!(
                "failed to unregister active hotkeys: {error}; operational state is unknown"
            )),
            format!("Failed to unregister active hotkeys: {error}"),
        ),
        TransitionFailureCause::RegisterNew { error, rollback } => match rollback {
            RollbackOutcome::CleanupFailed(cleanup_error) => (
                Some(format!("new registration failed: {error}; partial registration cleanup failed: {cleanup_error}; operational state is unknown")),
                format!("Failed to register hotkey profiles: {error}; cleanup failed: {cleanup_error}"),
            ),
            RollbackOutcome::RestoreFailed(restore_error) => (
                Some(format!("new registration failed: {error}; restoring previous hotkeys also failed: {restore_error}")),
                format!("Failed to register hotkey profiles: {error}"),
            ),
            RollbackOutcome::Restored(OperationalHotkey::Inactive) => (
                Some("no previous hotkey was active".to_string()),
                format!("Failed to register hotkey profiles: {error}"),
            ),
            RollbackOutcome::Restored(OperationalHotkey::Registered(_)) => {
                (None, format!("Failed to register hotkey profiles: {error}"))
            }
            RollbackOutcome::Restored(OperationalHotkey::Unknown) => (
                Some(format!("new registration failed: {error}; the previous hotkey state was unknown")),
                format!("Failed to register hotkey profiles: {error}"),
            ),
        },
    }
}

fn persistence_rollback_health_error(rollback: &RollbackOutcome) -> Option<String> {
    match rollback {
        RollbackOutcome::CleanupFailed(_) => Some(
            "failed to unregister unpersisted hotkeys; operational state is unknown".to_string(),
        ),
        RollbackOutcome::RestoreFailed(error) => {
            Some(format!("failed to restore previous hotkeys: {error}"))
        }
        RollbackOutcome::Restored(_) => None,
    }
}

struct HotkeyChangeContext<'a> {
    old_primary: &'a str,
    new_primary: &'a str,
    old_operational: &'a OperationalHotkey,
    new_shortcuts: &'a [String],
}

/// Finish a command hotkey change after its healthy-identical fast path.
/// Registration and persistence are injected so the sequence can be exercised
/// without a Tauri app or real settings I/O.
fn commit_registered_hotkey_change<T, E: Display>(
    context: HotkeyChangeContext<'_>,
    health: &HotkeyHealth,
    registrar: &mut impl transition::Registrar,
    persist: impl FnOnce() -> Result<T, E>,
    mut emit: impl FnMut(crate::hotkey::HotkeyStatusChange),
) -> Result<T, CommandError> {
    let operational =
        match transition::apply(context.old_operational, context.new_shortcuts, registrar) {
            Ok(operational) => operational,
            Err(failure) => {
                if let TransitionFailureCause::UnregisterOld { error } = &failure.cause {
                    error!("Failed to unregister operational hotkeys: {error}");
                }
                let (health_error, message) =
                    command_registration_failure_diagnostics(failure.cause);
                let change = health.record(context.old_primary, health_error, failure.operational);
                if change.changed {
                    emit(change);
                }
                return Err(CommandError::HotkeyChangeFailed(message));
            }
        };

    // Preserve the existing pending record before persistence. It deliberately
    // does not emit an event; only the final success or rollback does.
    health.record(
        context.old_primary,
        Some("hotkey profile change is pending persistence".to_string()),
        OperationalHotkey::registered_many(context.new_shortcuts),
    );

    let persisted = match persist() {
        Ok(persisted) => persisted,
        Err(save_error) => {
            let rollback =
                transition::rollback(context.old_operational, context.new_shortcuts, registrar);
            let health_error = persistence_rollback_health_error(&rollback);
            let change = health.record(context.old_primary, health_error, rollback.operational());
            if change.changed {
                emit(change);
            }
            return Err(CommandError::HotkeyChangeFailed(format!(
                "Failed to persist hotkey profiles: {save_error}"
            )));
        }
    };

    let change = health.record(context.new_primary, None, operational);
    if change.changed {
        emit(change);
    }
    Ok(persisted)
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
    let (candidate, notices) = prepare_candidate(&update, &sagascript_core::machine::load())?;
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
    if healthy_shortcuts_are_identical(
        &new_shortcuts,
        &old_shortcuts,
        &old_operational,
        health.is_failed(),
    ) {
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

    let mut registrar = NativeRegistrar::new(&app);
    let persisted = commit_registered_hotkey_change(
        HotkeyChangeContext {
            old_primary: &old_shortcut,
            new_primary: &new_primary,
            old_operational: &old_operational,
            new_shortcuts: &new_shortcuts,
        },
        &health,
        &mut registrar,
        || persist_hotkey_change(&controller, &update, &new_shortcuts),
        |change| {
            let _ = app.emit_event(
                crate::events::event::HOTKEY_REGISTRATION_CHANGED,
                &change.status,
            );
        },
    )?;

    info!("Hotkey set to: {}", persisted.hotkey);

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
            .map_err(|error| super::main_thread::MainThreadUnreachable(error.to_string()))
        })
        .await?;
        after_monitor_install(reply)?;
    }
    let profiles = sagascript_core::machine::load().resolved_hotkey_profiles();
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
pub async fn hotkey_status(health: State<'_, HotkeyHealth>) -> Result<HotkeyStatus, CommandError> {
    Ok(health.status())
}

#[cfg(test)]
#[path = "hotkey_tests.rs"]
mod tests;
