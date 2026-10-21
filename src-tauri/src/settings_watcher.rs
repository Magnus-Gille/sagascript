//! Hot reload of settings and personal dictionaries edited outside the app
//! (for example by `sagascript config set`).
//!
//! The watcher runs on its own thread, never on the native UI or audio thread.
//! Each change is handled inside `catch_unwind` (ADR 0005), so one panic is
//! counted in the hotkey health and the watcher keeps serving later changes.

use std::time::Duration;

use tauri::Manager;
use tracing::{error, info};

use crate::commands::{self, lock_controller, SharedController};
use crate::events::{self, EmitEvent, UiState};
use crate::hotkey;
use crate::hotkey::transition::{self, RollbackOutcome, TransitionFailure, TransitionFailureCause};
use crate::hotkey::{NativeRegistrar, OperationalHotkey};
use crate::launch::load_settings_with_permission_gate;
use crate::tray::update_profiles_menu;

#[derive(Debug)]
enum SettingsHotkeyTransitionError {
    Invalid(String),
    Registration(TransitionFailure),
}

/// Validation stays ahead of all OS work. Keeping this caller seam separate
/// lets the watcher retain its settings restoration and diagnostic contract.
fn transition_settings_hotkeys(
    previous: &OperationalHotkey,
    new: &[String],
    validation_error: Option<String>,
    registrar: &mut impl hotkey::transition::Registrar,
) -> Result<OperationalHotkey, SettingsHotkeyTransitionError> {
    if let Some(error) = validation_error {
        return Err(SettingsHotkeyTransitionError::Invalid(error));
    }
    transition::apply(previous, new, registrar).map_err(SettingsHotkeyTransitionError::Registration)
}

fn watcher_registration_diagnostic(cause: TransitionFailureCause) -> String {
    match cause {
        TransitionFailureCause::PreviousUnknown => "failed to unregister previous hotkeys: registration state is unknown after an earlier OS error; restart Sagascript".to_string(),
        TransitionFailureCause::UnregisterOld { error } => {
            format!("failed to unregister previous hotkeys: {error}")
        }
        TransitionFailureCause::RegisterNew { error, rollback } => match rollback {
            RollbackOutcome::CleanupFailed(cleanup_error) => format!(
                "failed to register new hotkey profiles: {error}; partial-registration cleanup failed: {cleanup_error}"
            ),
            RollbackOutcome::RestoreFailed(_) => format!(
                "failed to register new hotkey profiles: {error}; previous profiles could not be restored"
            ),
            RollbackOutcome::Restored(_) => format!(
                "failed to register new hotkey profiles: {error}; previous profiles restored"
            ),
        },
    }
}

/// Restore the settings fields that participate in shortcut configuration.
/// Other incoming settings continue to hot-reload after a shortcut failure.
fn restore_previous_hotkey_settings(
    settings: &mut sagascript_core::settings::Settings,
    old: &sagascript_core::settings::Settings,
) {
    settings.hotkey = old.hotkey.clone();
    settings.language = old.language;
    settings.profile_models = old.profile_models.clone();
    settings.hotkey_profiles = old.hotkey_profiles.clone();
    settings.hotkey_mode = old.hotkey_mode;
}

/// Whether a filesystem event may reflect creation or replacement of a user
/// configuration file.
fn configuration_event_may_affect(
    event: &notify::Event,
    settings_path: &std::path::Path,
    global_glossary_path: &std::path::Path,
    profile_glossary_dir: &std::path::Path,
) -> bool {
    let created_or_modified = matches!(
        event.kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_)
    );
    let glossary_changed =
        created_or_modified || matches!(event.kind, notify::EventKind::Remove(_));

    event.paths.iter().any(|path| {
        (created_or_modified && path == settings_path)
            || (glossary_changed
                && (path == global_glossary_path
                    || (path.parent() == Some(profile_glossary_dir)
                        && path.extension().and_then(|extension| extension.to_str())
                            == Some("txt"))))
    })
}

/// Feed every watcher event to `handle`, isolating each in `catch_unwind`
/// (#359): a panic while hot-reloading one change is logged and counted in
/// `health`, and the watcher keeps serving later changes instead of dying
/// silently with its thread.
fn run_settings_watcher_loop<E>(
    events: impl IntoIterator<Item = E>,
    health: &hotkey::health::SettingsWatcherHealth,
    mut handle: impl FnMut(E),
) {
    health.set_running(true);
    for event in events {
        // AssertUnwindSafe: the state a reload shares with the rest of the app
        // sits behind poison-tolerant locks (ADR 0005), its guards and leases
        // release while unwinding, and every event re-reads settings from
        // disk, so the next event never builds on a half-applied reload.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(event)));
        if let Err(payload) = outcome {
            let recovered_panics = health.record_recovered_panic();
            error!(
                recovered_panics,
                panic = sagascript_core::sync::panic_message(&*payload),
                "Settings watcher recovered from a panic while hot-reloading a change; still watching"
            );
        }
    }
    health.set_running(false);
}

/// Watch settings and personal dictionaries for external changes and hot-reload
/// them into the running app. Handles hotkey re-registration and emits a
/// settings-changed event to the frontend.
pub(crate) fn start_settings_watcher(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    use notify::PollWatcher;
    #[cfg(not(target_os = "macos"))]
    use notify::RecommendedWatcher;
    use notify::{Config, RecursiveMode, Watcher};
    use std::sync::mpsc;

    let settings_path = sagascript_core::settings::store::settings_path();
    let global_glossary_path = sagascript_core::settings::store::global_glossary_path();
    let profile_glossary_dir = sagascript_core::settings::store::profile_glossary_dir();
    let watch_dir = match settings_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            error!("Cannot determine settings directory for file watcher");
            return;
        }
    };
    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel();

        // notify's optional macOS kqueue backend can panic after our atomic
        // settings-file replacement. Polling this tiny directory avoids that
        // stale-file-descriptor path. Comparing contents is required because
        // several rapid saves can share the same whole-second mtime and size.
        // One-second polling keeps this background safety net inexpensive;
        // CLI-driven settings hot reload is not latency-sensitive.
        #[cfg(target_os = "macos")]
        let watcher_result = PollWatcher::new(
            tx,
            Config::default()
                .with_poll_interval(Duration::from_secs(1))
                .with_compare_contents(true),
        );
        #[cfg(not(target_os = "macos"))]
        let watcher_result = RecommendedWatcher::new(tx, Config::default());

        let mut watcher = match watcher_result {
            Ok(w) => w,
            Err(e) => {
                error!("Failed to create settings file watcher: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::Recursive) {
            error!("Failed to watch settings directory: {e}");
            return;
        }

        info!("Settings file watcher started on {}", watch_dir.display());

        let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
        run_settings_watcher_loop(rx, health.settings_watcher(), |event| {
            let event = match event {
                Ok(e) => e,
                Err(e) => {
                    error!("File watcher error: {e}");
                    return;
                }
            };

            if !configuration_event_may_affect(
                &event,
                &settings_path,
                &global_glossary_path,
                &profile_glossary_dir,
            ) {
                return;
            }

            // Small delay to let atomic rename complete
            std::thread::sleep(Duration::from_millis(50));

            let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
            let ctrl: tauri::State<'_, SharedController> = app.state();
            // Keep a live recording's bindings/configuration intact. This is
            // the dedicated settings-watcher thread, never the native UI or
            // audio thread. Re-read disk after idle so queued writes coalesce.
            let _recording_lease = loop {
                match commands::acquire_hotkey_configuration(&ctrl) {
                    Ok(lease) => break lease,
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            };
            let _transition = health.transition_guard();
            let mut new_settings = load_settings_with_permission_gate();
            let old_settings = {
                let c = lock_controller(&ctrl);
                c.settings().clone()
            };

            let old_shortcuts = old_settings.resolved_shortcuts();
            let new_shortcuts = new_settings.resolved_shortcuts();
            let validation_error = new_settings
                .validate_shortcut_configuration()
                .err()
                .map(|error| error.to_string());
            if new_shortcuts != old_shortcuts || validation_error.is_some() {
                info!("Settings watcher: hotkey profiles changed");
                // A panic between unregister and register must not leave
                // health claiming the old shortcuts are still registered.
                let _registration_change = health.registration_change();
                let old_operational = health.operational_hotkey();
                let mut registrar = NativeRegistrar::new(&app);
                let result = transition_settings_hotkeys(
                    &old_operational,
                    &new_shortcuts,
                    validation_error,
                    &mut registrar,
                );
                let change = match result {
                    Ok(operational) => health.record(&new_settings.hotkey, None, operational),
                    Err(SettingsHotkeyTransitionError::Invalid(error)) => {
                        restore_previous_hotkey_settings(&mut new_settings, &old_settings);
                        health.record(
                            &old_settings.hotkey,
                            Some(format!("{error}; previous hotkey profiles remain active")),
                            old_operational,
                        )
                    }
                    Err(SettingsHotkeyTransitionError::Registration(failure)) => {
                        restore_previous_hotkey_settings(&mut new_settings, &old_settings);
                        let health_error = watcher_registration_diagnostic(failure.cause);
                        health.record(
                            &old_settings.hotkey,
                            Some(health_error),
                            failure.operational,
                        )
                    }
                };
                if change.changed {
                    let _ =
                        app.emit_event(events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
                }
            }

            // Update controller with all new settings
            let profiles = new_settings.resolved_hotkey_profiles();
            let warm_after_change =
                sagascript_core::transcription::pianissimo_backend::profile_change_needs_warm(
                    &old_settings,
                    &new_settings,
                );
            let settings_for_unload = new_settings.clone();
            {
                let mut c = lock_controller(&ctrl);
                c.update_settings(new_settings);
            }
            if warm_after_change {
                sagascript_core::transcription::pianissimo_backend::warm_in_background(
                    "profile_change",
                    sagascript_core::machine::load,
                );
            }
            sagascript_core::transcription::pianissimo_backend::unload_if_unused(
                &settings_for_unload,
            );
            update_profiles_menu(&app, &profiles);

            // Notify frontend so UI reflects external changes
            let _ = app.emit_event(events::event::STATE_CHANGED, UiState::SettingsReloaded);

            info!("Settings hot-reloaded from disk");
        });
    });
}

#[cfg(test)]
#[path = "settings_watcher_tests.rs"]
mod tests;
