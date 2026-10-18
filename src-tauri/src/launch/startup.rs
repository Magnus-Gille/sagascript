use std::sync::{Arc, Mutex};

use sagascript_core::transcription::WhisperBackend;
use tauri::Manager;
use tracing::{error, info, warn};

use super::mode::{
    initial_window_request, should_install_update_at_launch, GuiLaunchMode, InitialWindowRequest,
};
use super::settings::{load_settings_with_permission_gate, migrate_legacy_settings_at_startup};
use super::window::open_settings_window;
use crate::app_controller::AppController;
use crate::commands::{lock_controller, SharedController, SharedWhisper};
use crate::dictation::warm_models_at_startup;
use crate::events::{self, EmitEvent};
#[cfg(target_os = "macos")]
use crate::platform;
use crate::settings_watcher::start_settings_watcher;
use crate::tray::{create_tray, manage_tray_state};
use crate::updater::{check_for_updates_and_install, manage_update_menu_state};
use crate::SAFE_FALLBACK_HOTKEY;
use crate::{
    focus_diag, hotkey, meeting_jobs, meeting_media, overlay, plain_file_jobs, update_activity,
};

/// The Tauri `setup` hook. The order of these steps is the startup order.
pub(crate) fn setup_app(
    app: &mut tauri::App,
    gui_launch_mode: GuiLaunchMode,
) -> Result<(), Box<dyn std::error::Error>> {
    // `tauri-plugin-single-instance` is initialized before Tauri calls
    // setup. Keep backend construction here so a secondary process is
    // rejected before it loads settings, opens audio resources, or
    // creates a Whisper backend.
    manage_app_state(app);

    focus_diag::init(app.handle());

    // Hide from dock on macOS (tray-only app)
    #[cfg(target_os = "macos")]
    {
        focus_diag::log("startup_activation_policy_before", serde_json::json!({}));
        platform::macos::set_activation_policy_accessory();
        focus_diag::log(
            "startup_activation_policy_set_accessory",
            serde_json::json!({}),
        );
        if let Err(error) = hotkey::install_bare_function_key_monitor(app.handle()) {
            error!("Failed to install F13-F24 event monitor: {error}");
        }
    }

    register_startup_hotkeys(app);

    create_tray(app)?;

    // Migrate settings store from FlowDictate
    migrate_legacy_settings_at_startup(app);

    // Watch settings file for external changes (e.g. `sagascript config set`)
    start_settings_watcher(app.handle().clone());

    // Only the login-item marker stays headless after onboarding.
    // A normal Finder/Spotlight launch is deliberate and opens
    // Settings; hotkey recording/transcription never enters this path.
    open_initial_window(app, gui_launch_mode);

    // Build the hidden overlay now, while Sagascript is still frontmost,
    // so the first dictation never creates a window (#291).
    if overlay::should_precreate_at_startup(
        sagascript_core::machine::load().show_overlay,
        !cfg!(target_os = "linux"),
    ) {
        overlay::precreate_hidden(app.handle());
    }

    {
        let windows: Vec<serde_json::Value> = app
            .webview_windows()
            .into_iter()
            .map(|(label, window)| {
                serde_json::json!({
                    "label": label,
                    "visible": window.is_visible().ok(),
                    "focused": window.is_focused().ok(),
                })
            })
            .collect();
        focus_diag::log("startup_windows", serde_json::json!({ "windows": windows }));
    }

    if should_install_update_at_launch(gui_launch_mode) {
        check_for_updates_and_install(app.handle().clone());
    }

    // Preload + warm the bounded set of models selected by the hotkey
    // profiles. The primary profile is restored as active after warmup,
    // while one distinct secondary stays resident for instant bilingual
    // switching. Missing models are never downloaded implicitly.
    warm_models_at_startup(app);

    Ok(())
}

/// Build the controller, the Whisper backend and the other shared state, and
/// register it with Tauri. Runs before anything can reach the controller.
fn manage_app_state(app: &tauri::App) {
    let settings = load_settings_with_permission_gate();
    let default_profile = settings.default_profile();
    info!(
        "Loaded settings: default_profile={}, language={:?}, model={:?}, hotkey={}",
        default_profile.id,
        default_profile.language,
        settings.dictation_model_for_profile(&default_profile.id),
        settings.hotkey
    );
    let initial_hotkey = settings.hotkey.clone();
    let update_activity = Arc::new(update_activity::UpdateActivity::default());
    let mut app_controller = AppController::new(settings);
    app_controller.set_update_activity(update_activity.clone());
    let controller: SharedController = Mutex::new(app_controller);
    let whisper: SharedWhisper = Arc::new(WhisperBackend::new());
    app.manage(update_activity);
    app.manage(update_activity::UpdatePreparation::default());
    app.manage(controller);
    app.manage(whisper);
    app.manage(plain_file_jobs::SharedPlainFileJobs::default());
    app.manage(Arc::new(meeting_jobs::MeetingJobs::default()));
    app.manage(meeting_media::SharedMeetingAudio::default());
    // Process-wide hotkey registration health (see hotkey::health for
    // why this is deliberately independent of the AppController
    // mutex). Assumed healthy until the synchronous registration
    // attempt below proves otherwise.
    app.manage(hotkey::HotkeyHealth::new(&initial_hotkey));
    manage_tray_state(app);
    manage_update_menu_state(app);
}

/// Register the shortcut of every configured profile and record the outcome in
/// the process-wide hotkey health.
fn register_startup_hotkeys(app: &tauri::App) {
    // Read every configured dictation profile and register its shortcut.
    let requested_settings = {
        let ctrl: tauri::State<'_, SharedController> = app.state();
        let c = lock_controller(&ctrl);
        c.settings().clone()
    };
    let requested_primary = requested_settings.default_profile().shortcut;

    // Register global shortcut. Failure here (combo already claimed by
    // Spotlight/Raycast/etc.) used to be log-only: the app would look
    // fine (tray shows "Idle") while being completely unable to
    // dictate. Recorded in the process-wide health flag so the tray
    // and Settings UI can surface it.
    let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
    let validation_error = requested_settings
        .validate_shortcut_configuration()
        .err()
        .map(|error| error.to_string());
    let registration_shortcuts: Vec<String> = if validation_error.is_some() {
        vec![SAFE_FALLBACK_HOTKEY.to_string()]
    } else {
        requested_settings.resolved_shortcuts()
    };
    if let Some(error) = &validation_error {
        warn!(
            "Refusing invalid saved hotkey profiles: {error}; trying safe fallback '{SAFE_FALLBACK_HOTKEY}'"
        );
    }
    let registration_error = hotkey::register_shortcuts(app.handle(), &registration_shortcuts)
        .err()
        .map(|error| error.to_string());
    let cleanup_error = registration_error.as_ref().and_then(|_| {
        hotkey::unregister_shortcuts(app.handle(), &registration_shortcuts)
            .err()
            .map(|error| error.to_string())
    });
    if let Some(error) = &registration_error {
        error!("Failed to register hotkey profiles: {error}");
    } else {
        info!(
            "Registered {} hotkey profile(s)",
            registration_shortcuts.len()
        );
    }
    let (health_error, operational_hotkey) = startup_hotkey_health(
        validation_error,
        registration_error,
        cleanup_error,
        registration_shortcuts,
    );
    let change = health.record(&requested_primary, health_error, operational_hotkey);
    if change.changed {
        let _ = app.emit_event(events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
    }
}

/// What the hotkey health records after the startup registration: the error to
/// show, if any, and what the OS now holds. `validation_error` is the saved
/// profiles' validation failure (the safe fallback was tried instead),
/// `registration_error` the failed registration and `cleanup_error` the failed
/// cleanup of a partial registration.
fn startup_hotkey_health(
    validation_error: Option<String>,
    registration_error: Option<String>,
    cleanup_error: Option<String>,
    registration_shortcuts: Vec<String>,
) -> (Option<String>, hotkey::OperationalHotkey) {
    match (validation_error, registration_error, cleanup_error) {
        (validation, Some(registration), Some(cleanup)) => (
            Some(format!("{}registration failed: {registration}; partial-registration cleanup failed: {cleanup}", validation.map(|error| format!("{error}; ")).unwrap_or_default())),
            hotkey::OperationalHotkey::Unknown,
        ),
        (None, None, None) => (
            None,
            hotkey::OperationalHotkey::registered_many(&registration_shortcuts),
        ),
        (Some(validation), None, None) => (
            Some(format!(
                "{validation}; using safe fallback '{SAFE_FALLBACK_HOTKEY}'"
            )),
            hotkey::OperationalHotkey::registered_many(&registration_shortcuts),
        ),
        (None, Some(registration), None) => {
            (Some(registration), hotkey::OperationalHotkey::Inactive)
        }
        (Some(validation), Some(registration), None) => (
            Some(format!(
                "{validation}; fallback '{SAFE_FALLBACK_HOTKEY}' also failed: {registration}"
            )),
            hotkey::OperationalHotkey::Inactive,
        ),
        // Cleanup runs only after a registration failure, so no run gets here;
        // if one did, the OS state is unknown, not registered.
        (validation, None, Some(cleanup)) => (
            Some(format!("{}partial-registration cleanup failed without a registration error: {cleanup}", validation.map(|error| format!("{error}; ")).unwrap_or_default())),
            hotkey::OperationalHotkey::Unknown,
        ),
    }
}

fn open_initial_window(app: &tauri::App, gui_launch_mode: GuiLaunchMode) {
    let settings = sagascript_core::machine::load();
    match initial_window_request(settings.has_completed_onboarding, gui_launch_mode) {
        InitialWindowRequest::Hidden => {
            info!("Background launch complete; Settings remains hidden");
        }
        InitialWindowRequest::Settings => {
            info!("Foreground GUI launch requested");
            open_settings_window(app.handle(), None, "startup_foreground_launch");
        }
        InitialWindowRequest::Onboarding => {
            info!("First launch detected, opening onboarding");
            open_settings_window(app.handle(), Some("onboarding"), "startup_onboarding");
        }
    }
}

#[cfg(test)]
#[path = "startup_tests.rs"]
mod tests;
