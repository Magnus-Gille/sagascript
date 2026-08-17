// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

// Core (audio/transcription/settings/error) lives in the sagascript-core
// crate; the CLI (clap definitions + subcommand dispatch) in sagascript-cli.
// This crate is the desktop shell: Tauri GUI + the desktop integrations
// (auto-paste, tray, hotkey, overlay) + CLI-first dispatch in main().

// File-logging service used by the desktop app only (the CLI logs via
// tracing_subscriber to stderr).
mod logging;

mod app_controller;
mod commands;
mod events;
mod hotkey;
mod overlay;
mod paste;
mod platform;
mod updates;

use tracing_subscriber::EnvFilter;

use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// Grace after a timeout-triggered abort for the blocking inference to unwind and
/// release the warm-state lock before we log it as still stuck.
const ABORT_GRACE_SECS: u64 = 5;

use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tracing::{error, info, warn};

use app_controller::{AppController, HotkeyDownResult, StopRecordingOutcome};
use commands::{SharedController, SharedWhisper};
#[cfg(test)]
use sagascript_core::settings::validate_hotkey;
use sagascript_core::settings::Language;
use sagascript_core::transcription::WhisperBackend;

/// Minimum recording duration before we allow stop (300ms)
const MIN_RECORDING_MS: u64 = 300;
const SAFE_FALLBACK_HOTKEY: &str = "Control+Shift+Space";

/// Shared tray status menu item for updating from anywhere
type SharedStatusItem = Mutex<Option<MenuItem<tauri::Wry>>>;

#[derive(Clone)]
struct LanguageMenuItems {
    auto: CheckMenuItem<tauri::Wry>,
    swedish: CheckMenuItem<tauri::Wry>,
    english: CheckMenuItem<tauri::Wry>,
    norwegian: CheckMenuItem<tauri::Wry>,
}

type SharedLanguageMenuItems = Mutex<Option<LanguageMenuItems>>;

fn language_from_menu_id(id: &str) -> Option<Language> {
    match id {
        "language_auto" => Some(Language::Auto),
        "language_swedish" => Some(Language::Swedish),
        "language_english" => Some(Language::English),
        "language_norwegian" => Some(Language::Norwegian),
        _ => None,
    }
}

pub(crate) fn update_language_menu(app: &tauri::AppHandle, language: Language) {
    let items = {
        let state: tauri::State<'_, SharedLanguageMenuItems> = app.state();
        let items = state.lock().unwrap().clone();
        items
    };
    let Some(items) = items else {
        return;
    };

    for (item, selected) in [
        (&items.auto, language == Language::Auto),
        (&items.swedish, language == Language::Swedish),
        (&items.english, language == Language::English),
        (&items.norwegian, language == Language::Norwegian),
    ] {
        if let Err(error) = item.set_checked(selected) {
            error!("Failed to update tray language menu: {error}");
        }
    }
}

#[derive(Clone)]
struct UpdateMenuItems {
    status: MenuItem<tauri::Wry>,
    check: MenuItem<tauri::Wry>,
}

struct UpdateMenuState {
    items: Option<UpdateMenuItems>,
    checking: bool,
}

type SharedUpdateMenuState = Mutex<UpdateMenuState>;

const UPDATE_CHECK_ACTION: &str = "Check for Updates…";

fn update_status_text(result: &updates::UpdateCheck) -> String {
    match result {
        updates::UpdateCheck::Available { version } => {
            format!("Update Available — v{version}")
        }
        updates::UpdateCheck::UpToDate => "Sagascript is up to date".to_string(),
    }
}

fn check_for_updates(app: tauri::AppHandle) {
    let items = {
        let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
        let mut state = state.lock().unwrap();
        if state.checking {
            return;
        }
        let Some(items) = state.items.clone() else {
            return;
        };
        state.checking = true;
        items
    };

    if let Err(error) = items.status.set_text("Checking for updates…") {
        error!("Failed to update tray update status: {error}");
    }
    if let Err(error) = items.check.set_enabled(false) {
        error!("Failed to disable update action: {error}");
    }

    tauri::async_runtime::spawn(async move {
        let result = updates::check_for_update(env!("CARGO_PKG_VERSION")).await;
        let (status_text, check_error) = match result {
            Ok(result) => (update_status_text(&result), None),
            Err(error) => ("Update check failed — try again".to_string(), Some(error)),
        };
        if let Some(error) = check_error {
            warn!("Update check failed: {error}");
        }

        let items = {
            let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
            let mut state = state.lock().unwrap();
            state.checking = false;
            state.items.clone()
        };
        let Some(items) = items else {
            return;
        };
        if let Err(error) = items.status.set_text(status_text) {
            error!("Failed to set tray update status: {error}");
        }
        if let Err(error) = items.check.set_enabled(true) {
            error!("Failed to re-enable update action: {error}");
        }
    });
}

#[cfg(any(target_os = "macos", test))]
fn auto_paste_permitted(requested: bool, accessibility_trusted: bool) -> bool {
    !requested || accessibility_trusted
}

/// Select the shortcut that is safe to register at startup. Invalid persisted
/// values remain visible in Settings, but never become active; the default
/// stays operational so an upgrade cannot strand dictation entirely.
#[cfg(test)]
fn startup_hotkey_candidate(requested: &str) -> (String, Option<String>) {
    match validate_hotkey(requested) {
        Ok(()) => (requested.to_string(), None),
        Err(error) => {
            let fallback = sagascript_core::settings::Settings::default().hotkey;
            debug_assert!(validate_hotkey(&fallback).is_ok());
            (fallback, Some(error))
        }
    }
}

/// Treat macOS TCC approval as runtime authorization, never as a preference
/// that can be inherited from another bundle identity or forced by the CLI.
fn load_settings_with_permission_gate() -> sagascript_core::settings::Settings {
    let settings = sagascript_core::settings::store::load();

    #[cfg(target_os = "macos")]
    {
        let mut settings = settings;
        if !auto_paste_permitted(
            settings.auto_paste,
            crate::platform::macos::is_accessibility_trusted(),
        ) {
            warn!("Auto-paste was enabled without Accessibility permission; disabling it");
            match sagascript_core::settings::store::update(|latest| latest.auto_paste = false) {
                Ok(persisted) => settings = persisted,
                Err(error) => {
                    error!("Failed to persist permission-gated auto-paste setting: {error}");
                    settings.auto_paste = false;
                }
            }
        }
        settings
    }

    #[cfg(not(target_os = "macos"))]
    settings
}

#[derive(Debug, PartialEq, Eq)]
enum GuiInstanceLockError {
    AlreadyRunning,
    Unavailable(String),
}

/// Process-lifetime OS lock for the bare GUI launch path. The lock file may
/// remain on disk, but the kernel-owned lock is released automatically on
/// clean exit or crash, so stale files cannot strand a later launch.
#[derive(Debug)]
struct GuiInstanceGuard {
    _file: File,
}

fn acquire_gui_instance_guard() -> Result<GuiInstanceGuard, GuiInstanceLockError> {
    let app_data_dir = sagascript_core::settings::store::app_data_dir();
    std::fs::create_dir_all(&app_data_dir).map_err(|error| {
        GuiInstanceLockError::Unavailable(format!(
            "failed to create app data directory {}: {error}",
            app_data_dir.display()
        ))
    })?;
    acquire_gui_instance_guard_at(&app_data_dir.join("gui-instance.lock"))
}

fn is_gui_instance_lock_contention(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }

    // LockFileEx reports contention as ERROR_LOCK_VIOLATION. Rust does not
    // currently normalize that code to WouldBlock on Windows.
    #[cfg(windows)]
    if error.raw_os_error() == Some(33) {
        return true;
    }

    false
}

fn acquire_gui_instance_guard_at(path: &Path) -> Result<GuiInstanceGuard, GuiInstanceLockError> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let file = options.open(path).map_err(|error| {
        GuiInstanceLockError::Unavailable(format!(
            "failed to open GUI instance lock {}: {error}",
            path.display()
        ))
    })?;

    match file.try_lock_exclusive() {
        Ok(()) => Ok(GuiInstanceGuard { _file: file }),
        Err(error) if is_gui_instance_lock_contention(&error) => {
            Err(GuiInstanceLockError::AlreadyRunning)
        }
        Err(error) => Err(GuiInstanceLockError::Unavailable(format!(
            "failed to lock GUI instance file {}: {error}",
            path.display()
        ))),
    }
}

fn main() {
    // CLI mode: if a subcommand is given, run CLI and exit. The desktop
    // binary is a full CLI (CLI-first design) — the GUI only launches on a
    // bare invocation.
    if let Some(parsed) = sagascript_cli::try_parse() {
        // CLI mode uses warn-level logging to keep stdout clean
        let configured_filter = std::env::var("RUST_LOG").ok();
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::new(sagascript_cli::effective_log_filter(
                configured_filter.as_deref(),
                "warn",
            )))
            .with_writer(std::io::stderr)
            .init();
        sagascript_cli::run(parsed);
        return;
    }

    // GUI mode: initialize tracing (console logging)
    let configured_filter = std::env::var("RUST_LOG").ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(sagascript_cli::effective_log_filter(
            configured_filter.as_deref(),
            "info",
        )))
        .init();

    info!("Sagascript starting...");

    // CLI subcommands returned above, so this lock covers GUI processes only.
    // Acquire it before TCC checks, state construction, or desktop services.
    let _gui_instance_guard = match acquire_gui_instance_guard() {
        Ok(guard) => guard,
        Err(GuiInstanceLockError::AlreadyRunning) => {
            info!("Another Sagascript GUI instance is already running; exiting");
            return;
        }
        Err(GuiInstanceLockError::Unavailable(error)) => {
            error!("Cannot establish single-instance GUI ownership: {error}");
            return;
        }
    };

    let settings = load_settings_with_permission_gate();
    info!("Loaded settings: language={:?}, model={:?}, hotkey={}", settings.language, settings.whisper_model, settings.hotkey);
    let initial_hotkey = settings.hotkey.clone();
    let controller = Mutex::new(AppController::new(settings));
    let whisper: SharedWhisper = Arc::new(WhisperBackend::new());
    // Process-wide hotkey registration health (see hotkey::health for why this
    // is deliberately independent of the AppController mutex). Assumed healthy
    // until the first real registration attempt in `.setup()` below proves
    // otherwise — there's no observable window in between since that attempt
    // runs synchronously before the event loop starts.
    let hotkey_health = hotkey::HotkeyHealth::new(&initial_hotkey);

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    let ctrl: tauri::State<'_, SharedController> = app.state();

                    match event.state {
                        ShortcutState::Pressed => {
                            info!("Hotkey pressed: {shortcut}");
                            let (result, active_profile) = {
                                let mut c = ctrl.lock().unwrap();
                                let profile = c
                                    .settings()
                                    .hotkey_profile_for_shortcut(&shortcut.to_string())
                                    .or_else(|| {
                                        (shortcut.to_string() == SAFE_FALLBACK_HOTKEY).then(|| {
                                            c.settings().resolved_hotkey_profiles()[0].clone()
                                        })
                                    });
                                let result = match profile {
                                    Some(profile) => match c.handle_hotkey_down_for_profile(profile) {
                                        Ok(r) => r,
                                        Err(e) => {
                                            error!("Hotkey down error: {e}");
                                            HotkeyDownResult::NoOp
                                        }
                                    },
                                    None => {
                                        warn!("Ignoring unconfigured shortcut event: {shortcut}");
                                        HotkeyDownResult::NoOp
                                    }
                                };
                                (result, c.active_hotkey_profile().cloned())
                            };
                            match result {
                                HotkeyDownResult::StartedRecording => {
                                    let show_overlay = {
                                        let c = ctrl.lock().unwrap();
                                        c.settings().show_overlay
                                    };
                                    let _ = app.emit(events::event::STATE_CHANGED, "recording");
                                    if let Some(profile) = active_profile {
                                        let _ = app.emit(events::event::ACTIVE_HOTKEY_PROFILE_CHANGED, profile);
                                    }
                                    update_tray_status(app, "recording");
                                    if show_overlay {
                                        overlay::show(app);
                                    }
                                }
                                HotkeyDownResult::StopRecording => {
                                    stop_recording_and_transcribe(app, &ctrl);
                                }
                                HotkeyDownResult::NoOp => {}
                            }
                        }
                        ShortcutState::Released => {
                            info!("Hotkey released: {shortcut}");
                            handle_hotkey_release(app, &ctrl, &shortcut.to_string());
                        }
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .manage(controller)
        .manage(whisper)
        .manage(hotkey_health)
        .manage(Mutex::new(None::<MenuItem<tauri::Wry>>) as SharedStatusItem)
        .manage(Mutex::new(None::<LanguageMenuItems>) as SharedLanguageMenuItems)
        .manage(Mutex::new(UpdateMenuState {
            items: None,
            checking: false,
        }) as SharedUpdateMenuState)
        .setup(|app| {
            // Hide from dock on macOS (tray-only app)
            #[cfg(target_os = "macos")]
            platform::macos::set_activation_policy_accessory();

            // Read every configured dictation profile and register its shortcut.
            let profiles = {
                let ctrl: tauri::State<'_, SharedController> = app.state();
                let c = ctrl.lock().unwrap();
                c.settings().resolved_hotkey_profiles()
            };
            let requested_primary = profiles
                .iter()
                .find(|profile| profile.id == "default")
                .unwrap_or(&profiles[0])
                .shortcut
                .clone();

            // Register global shortcut. Failure here (combo already claimed by
            // Spotlight/Raycast/etc.) used to be log-only: the app would look
            // fine (tray shows "Idle") while being completely unable to
            // dictate. Recorded in the process-wide health flag so the tray
            // and Settings UI can surface it.
            let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
            let validation_error = sagascript_core::settings::Settings::validate_hotkey_profiles(&profiles).err();
            let registration_shortcuts: Vec<String> = if validation_error.is_some() {
                vec![SAFE_FALLBACK_HOTKEY.to_string()]
            } else {
                profiles.iter().map(|profile| profile.shortcut.clone()).collect()
            };
            if let Some(error) = &validation_error {
                warn!(
                    "Refusing invalid saved hotkey profiles: {error}; trying safe fallback '{SAFE_FALLBACK_HOTKEY}'"
                );
            }
            let registration_error = app
                .global_shortcut()
                .register_multiple(registration_shortcuts.iter().map(String::as_str))
                .err()
                .map(|error| error.to_string());
            let cleanup_error = registration_error.as_ref().and_then(|_| {
                app.global_shortcut()
                    .unregister_multiple(registration_shortcuts.iter().map(String::as_str))
                    .err()
                    .map(|error| error.to_string())
            });
            if let Some(error) = &registration_error {
                error!("Failed to register hotkey profiles: {error}");
            } else {
                info!("Registered {} hotkey profile(s)", registration_shortcuts.len());
            }
            let (health_error, operational_hotkey) = match (validation_error, registration_error, cleanup_error) {
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
                (_, None, Some(_)) => unreachable!("cleanup only runs after registration failure"),
            };
            let change = health.record(&requested_primary, health_error, operational_hotkey);
            if change.changed {
                let _ = app.emit(events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
            }

            // Build tray menu
            let quit = MenuItem::with_id(app, "quit", "Quit Sagascript", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Open Sagascript...", true, None::<&str>)?;
            let transcribe_file_item =
                MenuItem::with_id(app, "transcribe_file", "Transcribe File...", true, None::<&str>)?;
            let status =
                MenuItem::with_id(app, "status", "Sagascript - Idle", false, None::<&str>)?;
            let language = {
                let ctrl: tauri::State<'_, SharedController> = app.state();
                let language = ctrl.lock().unwrap().language();
                language
            };
            let language_auto = CheckMenuItem::with_id(
                app,
                "language_auto",
                "Auto-detect",
                true,
                language == Language::Auto,
                None::<&str>,
            )?;
            let language_swedish = CheckMenuItem::with_id(
                app,
                "language_swedish",
                "Swedish",
                true,
                language == Language::Swedish,
                None::<&str>,
            )?;
            let language_english = CheckMenuItem::with_id(
                app,
                "language_english",
                "English",
                true,
                language == Language::English,
                None::<&str>,
            )?;
            let language_norwegian = CheckMenuItem::with_id(
                app,
                "language_norwegian",
                "Norwegian",
                true,
                language == Language::Norwegian,
                None::<&str>,
            )?;
            let language_menu = Submenu::with_items(
                app,
                "Language",
                true,
                &[
                    &language_auto,
                    &language_swedish,
                    &language_english,
                    &language_norwegian,
                ],
            )?;
            let update_status = MenuItem::with_id(
                app,
                "update_status",
                "Updates: not checked",
                false,
                None::<&str>,
            )?;
            let check_for_updates_item = MenuItem::with_id(
                app,
                "check_for_updates",
                UPDATE_CHECK_ACTION,
                true,
                None::<&str>,
            )?;

            // Store status item so we can update it after transcription
            {
                let status_state: tauri::State<'_, SharedStatusItem> = app.state();
                *status_state.lock().unwrap() = Some(status.clone());
            }
            {
                let language_state: tauri::State<'_, SharedLanguageMenuItems> = app.state();
                *language_state.lock().unwrap() = Some(LanguageMenuItems {
                    auto: language_auto,
                    swedish: language_swedish,
                    english: language_english,
                    norwegian: language_norwegian,
                });
            }
            {
                let update_state: tauri::State<'_, SharedUpdateMenuState> = app.state();
                update_state.lock().unwrap().items = Some(UpdateMenuItems {
                    status: update_status.clone(),
                    check: check_for_updates_item.clone(),
                });
            }

            let menu = Menu::with_items(
                app,
                &[
                    &status,
                    &language_menu,
                    &update_status,
                    &check_for_updates_item,
                    &settings_item,
                    &transcribe_file_item,
                    &quit,
                ],
            )?;

            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

            let _tray = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .tooltip("Sagascript")
                .icon(tray_icon)
                .icon_as_template(true)
                .on_menu_event(move |app, event| {
                    if let Some(language) = language_from_menu_id(event.id().as_ref()) {
                        if let Err(error) = commands::set_language_for_app(app, language) {
                            error!("Failed to change language from tray menu: {error}");
                        }
                        return;
                    }

                    match event.id().as_ref() {
                    "quit" => {
                        info!("Quit requested");
                        app.exit(0);
                    }
                    "settings" => {
                        open_settings_window(app, None);
                    }
                    "transcribe_file" => {
                        open_settings_window(app, Some("transcribe"));
                    }
                    "check_for_updates" => {
                        check_for_updates(app.clone());
                    }
                    _ => {}
                }
                })
                .build(app)?;

            info!("Tray icon created");

            // Render the initial tray state through the same path as later
            // updates — the hotkey-health flag was recorded above, before the
            // tray existed, and no state transition may ever come to refresh
            // a static "Idle" label if the hotkey is dead.
            update_tray_status(app.handle(), "idle");

            // Migrate settings store from FlowDictate
            {
                let app_dir = app.path().app_data_dir().ok();
                if let Some(dir) = app_dir {
                    let legacy = dir.join("flowdictate-settings.json");
                    let new_path = dir.join("sagascript-settings.json");
                    migrate_legacy_settings(&legacy, &new_path);
                }
            }

            // Watch settings file for external changes (e.g. `sagascript config set`)
            start_settings_watcher(app.handle().clone());

            // A bare GUI launch is an explicit request to see Sagascript. The
            // app has no configured Tauri windows, so completed-onboarding
            // launches must open Settings here too instead of leaving the user
            // with only a menu-bar icon.
            {
                let settings = sagascript_core::settings::store::load();
                let initial_tab = initial_main_window_tab(settings.has_completed_onboarding);
                if initial_tab == Some("onboarding") {
                    info!("First launch detected, opening onboarding");
                }
                open_settings_window(app.handle(), initial_tab);
            }

            // Preload + warm the whisper model in the background so the first
            // dictation of the session doesn't pay model-load and Metal/CoreML
            // kernel-compile latency. Best-effort: if the model isn't downloaded
            // yet (fresh install) we just skip and load lazily on first use.
            {
                let whisper: tauri::State<'_, SharedWhisper> = app.state();
                let whisper = whisper.inner().clone();
                let (model, language, vad_enabled) = {
                    let ctrl: tauri::State<'_, SharedController> = app.state();
                    let c = ctrl.lock().unwrap();
                    (
                        c.settings().effective_model(),
                        c.language(),
                        c.settings().vad_enabled,
                    )
                };
                std::thread::spawn(move || {
                    if let Err(e) = whisper.ensure_model(model) {
                        warn!("Model preload skipped: {e}");
                        return;
                    }
                    if let Err(e) = whisper.warmup(language) {
                        warn!("Model warmup failed: {e}");
                    } else {
                        info!("Model preloaded and warmed: {}", model.display_name());
                    }
                });

                // Startup is verification-only: model repair/download remains
                // tied to an explicit GUI enable action or CLI transcription.
                if vad_enabled {
                    let vad_path = sagascript_core::transcription::model::vad_model_path();
                    if vad_path.exists() {
                        if let Err(e) =
                            sagascript_core::transcription::model::verify_vad_model(&vad_path)
                        {
                            warn!(
                                "VAD model startup verification failed; re-enable VAD or use the CLI to repair it: {e}"
                            );
                        }
                    } else {
                        warn!(
                            "VAD is enabled but its model is missing; re-enable VAD or use the CLI to download it"
                        );
                    }
                }

                // Model and accelerator assets are downloaded only from an
                // explicit model-selection/download action. Startup never
                // performs a silent CoreML network backfill.
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            // Hide settings window instead of closing it (prevents app exit)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                info!("Window hidden (not closed)");
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_settings,
            commands::get_active_hotkey_profile,
            commands::get_last_transcription,
            commands::get_last_error,
            commands::is_model_ready,
            commands::get_loaded_model,
            commands::set_language,
            commands::set_whisper_model,
            commands::set_auto_select_model,
            commands::set_hotkey_mode,
            commands::set_hotkey,
            commands::set_hotkey_profiles,
            commands::hotkey_status,
            commands::start_recording,
            commands::stop_and_transcribe,
            commands::cancel_recording,
            commands::is_model_downloaded,
            commands::get_model_info,
            commands::download_model,
            commands::set_auto_paste,
            commands::set_show_overlay,
            commands::set_initial_prompt,
            commands::set_beam_size,
            commands::set_temperature_fallback,
            commands::set_vad_enabled,
            commands::get_build_info,
            commands::transcribe_file,
            commands::get_supported_formats,
            commands::check_accessibility_permission,
            commands::request_accessibility_permission,
            commands::microphone_status,
            commands::request_microphone_access,
            commands::open_microphone_settings,
            commands::get_platform,
            commands::set_onboarding_completed,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Sagascript")
        .run(|_app_handle, event| {
            match event {
                // Prevent app from exiting when all windows are closed (tray-only app),
                // but allow explicit exit requests (e.g. from tray "Quit" menu)
                tauri::RunEvent::ExitRequested {
                    api, code: None, ..
                } => api.prevent_exit(),
                // Finder/Spotlight sends Reopen when the already-running app is
                // launched again. A miniaturized NSWindow may still count as
                // "visible", so always run the full reveal path.
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen { .. } => {
                    info!("Application reopen requested");
                    open_settings_window(_app_handle, None);
                }
                _ => {}
            }
        });
}

/// Pure state -> (tooltip, title, menu_text) mapping for the tray, extracted
/// so the "hotkey unavailable" sticky-warning behavior can be unit tested
/// without a running Tauri app. `hotkey_failed` must win over every normal
/// state (idle/recording/transcribing/loading_model): once the hotkey is
/// known to be unregistered, no ordinary state transition is allowed to
/// silently paper back over "Idle" — that's the whole point of making the
/// warning sticky.
fn tray_label(state: &str, hotkey_failed: bool) -> (&'static str, &'static str, &'static str) {
    if hotkey_failed {
        return (
            "Sagascript - Hotkey unavailable",
            "\u{26A0}",
            "Hotkey unavailable",
        );
    }
    match state {
        "recording" => ("Sagascript - Recording...", "Rec", "Recording..."),
        "loading_model" => ("Sagascript - Loading model...", "Loading...", "Loading model..."),
        "transcribing" => ("Sagascript - Transcribing...", "...", "Transcribing..."),
        _ => ("Sagascript", "", "Idle"),
    }
}

/// Update the tray tooltip, title, and status menu item to reflect current
/// state. Consults the process-wide hotkey health flag on every call so a
/// broken hotkey registration renders as a sticky "Hotkey unavailable"
/// warning that later state changes (recording -> idle, model-preload status,
/// etc.) cannot silently overwrite.
fn update_tray_status(app: &tauri::AppHandle, state: &str) {
    let hotkey_failed = app.state::<hotkey::HotkeyHealth>().is_failed();
    let (tooltip, title, menu_text) = tray_label(state, hotkey_failed);

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip));
        let _ = tray.set_title(Some(title));
    }

    set_status_menu_text(app, &format!("Sagascript - {menu_text}"));
}

/// Migrate the legacy FlowDictate settings file to the new Sagascript path, if
/// present and no Sagascript settings file already exists. Rename failure used
/// to be silently swallowed (`let _ = std::fs::rename(...)`), which would
/// silently reset an upgrading user to defaults with no diagnostic trail.
fn migrate_legacy_settings(legacy: &std::path::Path, new_path: &std::path::Path) {
    if !legacy.exists() || new_path.exists() {
        return;
    }

    info!("Migrating settings store from FlowDictate");
    match std::fs::rename(legacy, new_path) {
        Ok(()) => info!(
            "Migrated settings store from FlowDictate ({}) to Sagascript ({})",
            legacy.display(),
            new_path.display()
        ),
        Err(e) => warn!(
            "Failed to migrate FlowDictate settings from {} to {}: {e} — \
             the upgrading user will fall back to default settings",
            legacy.display(),
            new_path.display()
        ),
    }
}

/// Truncate transcription text for tray display, cutting on a char boundary.
fn truncate_for_tray(text: &str) -> String {
    if text.len() > 60 {
        let cut = text
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= 57)
            .last()
            .unwrap_or(0);
        format!("{}...", &text[..cut])
    } else {
        text.to_string()
    }
}

/// Update the tray status menu item and tooltip to show the last transcription.
///
/// Guarded the same way as [`update_tray_status`]: if the hotkey is currently
/// unregistered, showing "Last: <transcription>" would silently overwrite the
/// sticky "Hotkey unavailable" warning (the exact trap called out in review —
/// this call always follows an `update_tray_status(_, "idle")` in the success
/// path). Delegate to `update_tray_status` in that case instead of duplicating
/// the warning text here.
fn update_tray_last_result(app: &tauri::AppHandle, text: &str) {
    if app.state::<hotkey::HotkeyHealth>().is_failed() {
        update_tray_status(app, "idle");
        return;
    }

    let display = truncate_for_tray(text);

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(&format!("Sagascript\nLast: {display}")));
    }

    set_status_menu_text(app, &format!("\u{2713} {display}"));
}

/// Helper to update the status menu item text
fn set_status_menu_text(app: &tauri::AppHandle, text: &str) {
    let guard = app.state::<SharedStatusItem>().lock().unwrap().clone();
    if let Some(item) = guard {
        let _ = item.set_text(text);
    }
}

fn initial_main_window_tab(has_completed_onboarding: bool) -> Option<&'static str> {
    if has_completed_onboarding {
        None
    } else {
        Some("onboarding")
    }
}

trait MainWindowVisibility {
    fn unminimize(&self) -> Result<(), String>;
    fn show(&self) -> Result<(), String>;
    fn set_focus(&self) -> Result<(), String>;
}

impl MainWindowVisibility for tauri::WebviewWindow {
    fn unminimize(&self) -> Result<(), String> {
        tauri::WebviewWindow::unminimize(self).map_err(|error| error.to_string())
    }

    fn show(&self) -> Result<(), String> {
        tauri::WebviewWindow::show(self).map_err(|error| error.to_string())
    }

    fn set_focus(&self) -> Result<(), String> {
        tauri::WebviewWindow::set_focus(self).map_err(|error| error.to_string())
    }
}

fn reveal_existing_main_window(window: &impl MainWindowVisibility) -> Result<(), String> {
    window
        .unminimize()
        .map_err(|error| format!("failed to restore main window: {error}"))?;
    window
        .show()
        .map_err(|error| format!("failed to show main window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed to focus main window: {error}"))
}

/// Open or focus the main window, optionally navigating to a specific tab.
/// Errors are surfaced in the application log instead of being silently lost.
fn open_settings_window(app: &tauri::AppHandle, tab: Option<&str>) {
    info!("Opening main window (tab: {:?})", tab);

    if let Err(error) = try_open_settings_window(app, tab) {
        error!("Failed to open main window: {error}");
    }
}

fn try_open_settings_window(app: &tauri::AppHandle, tab: Option<&str>) -> Result<(), String> {
    // Build a URL with optional query parameter
    let url = match tab {
        Some("onboarding") => "index.html?onboarding=true".to_string(),
        Some(t) => format!("index.html?tab={t}"),
        None => "index.html".to_string(),
    };

    if let Some(window) = app.get_webview_window("settings") {
        // If switching tab on existing window, emit an event
        if let Some(t) = tab {
            if let Err(error) = window.emit("navigate_tab", t) {
                warn!("Failed to navigate main window to '{t}': {error}");
            }
        }
        reveal_existing_main_window(&window)
    } else {
        // Cap default height to 80% of screen so it fits on small displays (e.g. 768p laptops)
        let default_height = if let Ok(Some(monitor)) = app.primary_monitor() {
            let logical_h = monitor.size().height as f64 / monitor.scale_factor();
            (logical_h * 0.8).min(660.0)
        } else {
            660.0
        };

        let window = tauri::WebviewWindowBuilder::new(
            app,
            "settings",
            tauri::WebviewUrl::App(url.into()),
        )
        .title("Sagascript")
        .inner_size(500.0, default_height)
        .min_inner_size(500.0, 400.0)
        .resizable(true)
        .center()
        .focused(true)
        .build()
        .map_err(|error| format!("failed to create main window: {error}"))?;

        reveal_existing_main_window(&window)
    }
}

/// Handle hotkey release: stop recording for push-to-talk mode
fn handle_hotkey_release(
    app: &tauri::AppHandle,
    ctrl: &tauri::State<'_, SharedController>,
    shortcut: &str,
) {
    let should_stop = {
        let c = ctrl.lock().unwrap();
        c.should_stop_profile_on_key_up(shortcut)
    };

    if !should_stop {
        return;
    }

    stop_recording_and_transcribe(app, ctrl);
}

/// Run a UI closure on the macOS main thread. NSStatusItem / NSWindow (tray,
/// overlay) APIs must not be touched from a worker thread; best-effort — logs
/// if the dispatch itself fails.
fn dispatch_to_main<F>(app: &tauri::AppHandle, f: F)
where
    F: FnOnce(&tauri::AppHandle) + Send + 'static,
{
    let app_for_closure = app.clone();
    if let Err(e) = app.run_on_main_thread(move || f(&app_for_closure)) {
        error!("Failed to dispatch UI work to main thread: {e}");
    }
}

/// Stop recording, enforce minimum duration, and spawn transcription.
/// Shared by both push-to-talk (on key-up) and toggle (on second key-down).
fn stop_recording_and_transcribe(
    app: &tauri::AppHandle,
    ctrl: &tauri::State<'_, SharedController>,
) {
    // Compute how long we still need to hold to satisfy the minimum recording
    // duration — but do NOT block the global-shortcut (UI) thread waiting for it
    // (finding 2): a std::thread::sleep here freezes UI redraw and stalls
    // subsequent hotkey events. The delay is offloaded to an async task below.
    let elapsed = {
        let c = ctrl.lock().unwrap();
        c.recording_elapsed()
    };
    let min = Duration::from_millis(MIN_RECORDING_MS);
    let remaining = if elapsed < min {
        let rem = min - elapsed;
        info!(
            "Recording too short ({:.0}ms), deferring stop by {:.0}ms off the UI thread...",
            elapsed.as_millis(),
            rem.as_millis()
        );
        Some(rem)
    } else {
        None
    };

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Min-duration top-up delay, offloaded so the UI thread stays responsive.
        if let Some(rem) = remaining {
            tokio::time::sleep(rem).await;
        }

        // Stop recording (single lock acquisition, re-acquired here since the
        // controller State can't be moved into the task). Guarded so a stop that
        // races an already-stopped session is a no-op, and a capture/resample
        // failure surfaces as a real error (findings 3 & 4).
        let outcome = {
            let ctrl: tauri::State<'_, SharedController> = app_handle.state();
            let mut c = ctrl.lock().unwrap();
            c.stop_recording_guarded()
        };
        let audio = match outcome {
            StopRecordingOutcome::NotRecording => return,
            StopRecordingOutcome::Failed(msg) => {
                error!("Recording stop failed: {msg}");
                dispatch_to_main(&app_handle, |app| {
                    overlay::hide(app);
                    update_tray_status(app, "idle");
                });
                let _ = app_handle.emit(events::event::ERROR, msg);
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                return;
            }
            StopRecordingOutcome::Stopped(audio) => audio,
        };

        // Hide overlay + show the transcribing state — re-dispatched to the main
        // thread now that this runs on a worker.
        dispatch_to_main(&app_handle, |app| {
            overlay::hide(app);
            update_tray_status(app, "transcribing");
        });
        let _ = app_handle.emit(events::event::STATE_CHANGED, "transcribing");

        if audio.is_empty() {
            {
                let ctrl: tauri::State<'_, SharedController> = app_handle.state();
                ctrl.lock().unwrap().on_transcription_error("No audio captured");
            }
            dispatch_to_main(&app_handle, |app| update_tray_status(app, "idle"));
            let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
            return;
        }

        // Transcribe (timeout/cancellation logic is owned by a separate work
        // package — left unchanged). Runs in this same task, which is already
        // off the UI thread.
        let ctrl: tauri::State<'_, SharedController> = app_handle.state();
        let whisper: tauri::State<'_, SharedWhisper> = app_handle.state();

        // Extract what we need for transcription (lock briefly)
        let (language, effective_model, opts) = {
            let c = ctrl.lock().unwrap();
            (
                c.language(),
                c.settings().effective_model_for(c.language()),
                commands::build_transcribe_options(c.settings()),
            )
        };

        info!("Transcribing with model: {}", effective_model.display_name());

        // Show model loading status in tray
        if whisper.needs_reload(effective_model) {
            let _ = app_handle.emit(events::event::STATE_CHANGED, "loading_model");
            dispatch_to_main(&app_handle, |app| update_tray_status(app, "loading_model"));
        }

        // Ensure model is loaded
        let result = if let Err(e) = whisper.ensure_model(effective_model) {
            Err(e)
        } else {
            // Run blocking transcription on a separate thread with a timeout. On
            // timeout we trigger a REAL abort (whisper-rs abort callback wired in
            // WhisperBackend): request_abort() flips the flag whisper.cpp checks
            // between compute steps, so the blocking task returns and releases the
            // warm state instead of running to completion and wedging the pipeline.
            let whisper_ref = whisper.inner().clone();
            let mut fut = tokio::task::spawn_blocking(move || {
                whisper_ref.transcribe_sync_with_options(&audio, language, &opts, |_| {})
            });

            let timeout = Duration::from_secs(TRANSCRIPTION_TIMEOUT_SECS);
            match tokio::time::timeout(timeout, &mut fut).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => Err(sagascript_core::error::DictationError::TranscriptionFailed(
                    format!("Task join error: {e}"),
                )),
                Err(_) => {
                    warn!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s — requesting abort");
                    whisper.request_abort();
                    // Brief grace for the aborted inference to unwind; log which
                    // outcome occurred so a genuine hang is visible.
                    match tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut fut).await
                    {
                        Ok(_) => info!("Aborted transcription task exited — warm-state lock released"),
                        Err(_) => error!(
                            "Transcription task still running {ABORT_GRACE_SECS}s after abort — \
                             warm state may stay locked until it unwinds; further transcriptions \
                             will report ModelBusy rather than block forever"
                        ),
                    }
                    Err(sagascript_core::error::DictationError::TranscriptionFailed(
                        format!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s (inference aborted)"),
                    ))
                }
            }
        };

        match result {
            Ok(text) => {
                info!("Transcription complete: {} chars", text.len());

                // Check if auto-paste is enabled (lock briefly)
                let should_paste = {
                    let c = ctrl.lock().unwrap();
                    c.settings().auto_paste
                };

                if should_paste {
                    // Auto-paste MUST run on the main thread — enigo's macOS TIS APIs
                    // crash (SIGABRT) if called from a tokio worker thread.
                    let text_for_paste = text.clone();
                    if let Err(e) = app_handle.run_on_main_thread(move || {
                        info!("Running auto-paste on main thread...");
                        let paste_svc = crate::paste::PasteService::new();
                        match paste_svc.paste(&text_for_paste) {
                            Ok(()) => info!("Auto-paste completed successfully"),
                            Err(e) => error!("Auto-paste failed: {e}"),
                        }
                    }) {
                        error!("Failed to dispatch paste to main thread: {e}");
                    }
                }

                let mut c = ctrl.lock().unwrap();
                c.on_transcription_success(&text);
                drop(c);

                let _ = app_handle.emit(events::event::TRANSCRIPTION_RESULT, &text);
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                let text_for_tray = text.clone();
                dispatch_to_main(&app_handle, move |app| {
                    update_tray_status(app, "idle");
                    update_tray_last_result(app, &text_for_tray);
                });
                info!("Transcription flow complete, app should remain running");
            }
            Err(e) => {
                error!("Transcription failed: {e}");
                let mut c = ctrl.lock().unwrap();
                c.on_transcription_error(&e.to_string());
                drop(c);
                let _ = app_handle.emit(events::event::ERROR, e.to_string());
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                dispatch_to_main(&app_handle, |app| update_tray_status(app, "idle"));
                info!("Error flow complete, app should remain running");
            }
        }
    });
}

/// Whether a filesystem event may reflect creation or replacement of the settings file.
fn settings_event_may_affect(
    event: &notify::Event,
    settings_path: &std::path::Path,
) -> bool {
    let relevant_kind = matches!(
        event.kind,
        notify::EventKind::Create(_) | notify::EventKind::Modify(_)
    );

    relevant_kind
        && event
            .paths
            .iter()
            .any(|path| path == settings_path)
}

/// Watch the settings file for external changes and hot-reload into the running app.
/// Handles hotkey re-registration and emits a settings-changed event to the frontend.
fn start_settings_watcher(app: tauri::AppHandle) {
    use notify::{Config, RecursiveMode, Watcher};
    #[cfg(not(target_os = "macos"))]
    use notify::RecommendedWatcher;
    #[cfg(target_os = "macos")]
    use notify::PollWatcher;
    use std::sync::mpsc;

    let settings_path = sagascript_core::settings::store::settings_path();
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

        if let Err(e) = watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
            error!("Failed to watch settings directory: {e}");
            return;
        }

        info!("Settings file watcher started on {}", watch_dir.display());

        for event in rx {
            let event = match event {
                Ok(e) => e,
                Err(e) => {
                    error!("File watcher error: {e}");
                    continue;
                }
            };

            if !settings_event_may_affect(&event, &settings_path) {
                continue;
            }

            // Small delay to let atomic rename complete
            std::thread::sleep(Duration::from_millis(50));

            let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
            let _transition = health.transition_guard();
            let mut new_settings = load_settings_with_permission_gate();
            let ctrl: tauri::State<'_, SharedController> = app.state();
            let old_settings = {
                let c = ctrl.lock().unwrap();
                c.settings().clone()
            };

            let old_profiles = old_settings.resolved_hotkey_profiles();
            let new_profiles = new_settings.resolved_hotkey_profiles();
            let old_profile_shortcuts: Vec<&str> = old_profiles.iter().map(|profile| profile.shortcut.as_str()).collect();
            let new_profile_shortcuts: Vec<&str> = new_profiles.iter().map(|profile| profile.shortcut.as_str()).collect();
            if new_profile_shortcuts != old_profile_shortcuts {
                info!("Settings watcher: hotkey profiles changed");
                let old_operational = health.operational_hotkey();
                let new_shortcuts: Vec<String> = new_profiles.iter().map(|profile| profile.shortcut.clone()).collect();
                let validation_error = sagascript_core::settings::Settings::validate_hotkey_profiles(&new_profiles).err();
                let unregister_error = if validation_error.is_none() {
                    match &old_operational {
                        hotkey::OperationalHotkey::Registered(shortcuts) => app
                            .global_shortcut()
                            .unregister_multiple(shortcuts.iter().map(String::as_str))
                            .err()
                            .map(|error| error.to_string()),
                        hotkey::OperationalHotkey::Inactive => None,
                        hotkey::OperationalHotkey::Unknown => Some(
                            "registration state is unknown after an earlier OS error; restart Sagascript".to_string(),
                        ),
                    }
                } else {
                    None
                };

                let change = if let Some(error) = validation_error {
                    new_settings.hotkey = old_settings.hotkey.clone();
                    new_settings.language = old_settings.language;
                    new_settings.hotkey_profiles = old_settings.hotkey_profiles.clone();
                    health.record(&old_settings.hotkey, Some(format!("{error}; previous hotkey profiles remain active")), old_operational)
                } else if let Some(error) = unregister_error {
                    new_settings.hotkey = old_settings.hotkey.clone();
                    new_settings.language = old_settings.language;
                    new_settings.hotkey_profiles = old_settings.hotkey_profiles.clone();
                    health.record(&old_settings.hotkey, Some(format!("failed to unregister previous hotkeys: {error}")), hotkey::OperationalHotkey::Unknown)
                } else {
                    match app.global_shortcut().register_multiple(new_shortcuts.iter().map(String::as_str)) {
                        Ok(()) => health.record(&new_settings.hotkey, None, hotkey::OperationalHotkey::registered_many(&new_shortcuts)),
                        Err(error) => {
                            new_settings.hotkey = old_settings.hotkey.clone();
                            new_settings.language = old_settings.language;
                            new_settings.hotkey_profiles = old_settings.hotkey_profiles.clone();
                            match app.global_shortcut().unregister_multiple(new_shortcuts.iter().map(String::as_str)) {
                                Err(cleanup_error) => health.record(
                                    &old_settings.hotkey,
                                    Some(format!("failed to register new hotkey profiles: {error}; partial-registration cleanup failed: {cleanup_error}")),
                                    hotkey::OperationalHotkey::Unknown,
                                ),
                                Ok(()) => {
                                    let restored = match &old_operational {
                                        hotkey::OperationalHotkey::Registered(shortcuts) => app.global_shortcut().register_multiple(shortcuts.iter().map(String::as_str)).is_ok(),
                                        hotkey::OperationalHotkey::Inactive => true,
                                        hotkey::OperationalHotkey::Unknown => false,
                                    };
                                    health.record(
                                        &old_settings.hotkey,
                                        Some(format!("failed to register new hotkey profiles: {error}; previous profiles {}", if restored { "restored" } else { "could not be restored" })),
                                        if restored { old_operational } else { hotkey::OperationalHotkey::Inactive },
                                    )
                                }
                            }
                        }
                    }
                };
                if change.changed {
                    let _ = app.emit(events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
                }
            }

            // Update controller with all new settings
            let selected_language = new_settings.language;
            {
                let mut c = ctrl.lock().unwrap();
                c.update_settings(new_settings);
            }
            update_language_menu(&app, selected_language);

            // Notify frontend so UI reflects external changes
            let _ = app.emit(events::event::STATE_CHANGED, "settings_reloaded");

            info!("Settings hot-reloaded from disk");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gui_instance_lock_rejects_a_concurrent_owner_and_recovers_after_drop() {
        let dir = std::env::temp_dir().join(format!(
            "sagascript-instance-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("gui-instance.lock");

        let first = acquire_gui_instance_guard_at(&path).unwrap();
        assert_eq!(
            acquire_gui_instance_guard_at(&path).unwrap_err(),
            GuiInstanceLockError::AlreadyRunning
        );

        drop(first);
        acquire_gui_instance_guard_at(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn language_menu_ids_resolve_to_persisted_language_values() {
        assert_eq!(language_from_menu_id("language_auto"), Some(Language::Auto));
        assert_eq!(
            language_from_menu_id("language_swedish"),
            Some(Language::Swedish)
        );
        assert_eq!(
            language_from_menu_id("language_english"),
            Some(Language::English)
        );
        assert_eq!(
            language_from_menu_id("language_norwegian"),
            Some(Language::Norwegian)
        );
        assert_eq!(language_from_menu_id("other"), None);
    }

    #[test]
    fn update_status_describes_available_and_current_releases() {
        assert_eq!(
            update_status_text(&updates::UpdateCheck::Available {
                version: semver::Version::new(1, 2, 3)
            }),
            "Update Available — v1.2.3"
        );
        assert_eq!(
            update_status_text(&updates::UpdateCheck::UpToDate),
            "Sagascript is up to date"
        );
    }

    #[derive(Default)]
    struct MockMainWindow {
        operations: std::cell::RefCell<Vec<&'static str>>,
        fail_at: Option<&'static str>,
    }

    impl MainWindowVisibility for MockMainWindow {
        fn unminimize(&self) -> Result<(), String> {
            self.operations.borrow_mut().push("unminimize");
            if self.fail_at == Some("unminimize") {
                Err("restore failed".to_string())
            } else {
                Ok(())
            }
        }

        fn show(&self) -> Result<(), String> {
            self.operations.borrow_mut().push("show");
            if self.fail_at == Some("show") {
                Err("show failed".to_string())
            } else {
                Ok(())
            }
        }

        fn set_focus(&self) -> Result<(), String> {
            self.operations.borrow_mut().push("set_focus");
            if self.fail_at == Some("set_focus") {
                Err("focus failed".to_string())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn main_window_reveal_restores_before_showing_and_focusing() {
        let window = MockMainWindow::default();

        reveal_existing_main_window(&window).unwrap();

        assert_eq!(
            *window.operations.borrow(),
            ["unminimize", "show", "set_focus"]
        );
    }

    #[test]
    fn main_window_reveal_reports_the_failed_step_and_stops() {
        let window = MockMainWindow {
            fail_at: Some("show"),
            ..Default::default()
        };

        let error = reveal_existing_main_window(&window).unwrap_err();

        assert!(error.contains("show main window"));
        assert!(error.contains("show failed"));
        assert_eq!(*window.operations.borrow(), ["unminimize", "show"]);
    }

    #[test]
    fn completed_onboarding_starts_on_the_settings_view() {
        assert_eq!(initial_main_window_tab(true), None);
    }

    #[test]
    fn incomplete_onboarding_starts_on_the_onboarding_view() {
        assert_eq!(initial_main_window_tab(false), Some("onboarding"));
    }

    #[cfg(target_os = "macos")]
    fn wait_for_settings_event(
        rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
        settings_path: &std::path::Path,
    ) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(Ok(event)) if settings_event_may_affect(&event, settings_path) => {
                    return true;
                }
                Ok(_) => continue,
                Err(_) => return false,
            }
        }
        false
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_settings_watcher_survives_repeated_atomic_replacements() {
        use notify::{Config, PollWatcher, RecursiveMode, Watcher};

        let dir = std::env::temp_dir().join(format!(
            "sagascript-settings-watcher-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let settings_path = dir.join("sagascript-settings.json");
        std::fs::write(&settings_path, b"{}\n").unwrap();

        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = PollWatcher::new(
            tx,
            Config::default()
                .with_poll_interval(Duration::from_millis(100))
                .with_compare_contents(true),
        )
        .unwrap();
        watcher
            .watch(&dir, RecursiveMode::NonRecursive)
            .unwrap();

        for value in [1, 2, 3, 4, 5] {
            let temporary = dir.join(format!("settings-{value}.tmp"));
            std::fs::write(&temporary, format!("{{\"value\":{value}}}\n")).unwrap();
            std::fs::rename(&temporary, &settings_path).unwrap();
            assert!(
                wait_for_settings_event(&rx, &settings_path),
                "watcher stopped reporting the settings path after atomic replacement {value}"
            );
        }

        drop(watcher);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn settings_event_filter_accepts_target_create_and_modify_events() {
        use notify::event::{CreateKind, ModifyKind};
        use notify::{Event, EventKind};

        let watch_dir = std::path::Path::new("/tmp/sagascript");
        let settings_path = watch_dir.join("sagascript-settings.json");

        for kind in [
            EventKind::Create(CreateKind::Any),
            EventKind::Modify(ModifyKind::Any),
        ] {
            let target_event = Event::new(kind).add_path(settings_path.clone());
            assert!(settings_event_may_affect(&target_event, &settings_path));
        }
    }

    #[test]
    fn settings_event_filter_rejects_unrelated_and_non_mutating_events() {
        use notify::event::{AccessKind, ModifyKind, RemoveKind};
        use notify::{Event, EventKind};

        let watch_dir = std::path::Path::new("/tmp/sagascript");
        let settings_path = watch_dir.join("sagascript-settings.json");
        let unrelated = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(watch_dir.join("settings.tmp"));
        assert!(!settings_event_may_affect(&unrelated, &settings_path));

        let remove = Event::new(EventKind::Remove(RemoveKind::Any))
            .add_path(settings_path.clone());
        assert!(!settings_event_may_affect(&remove, &settings_path));

        let access = Event::new(EventKind::Access(AccessKind::Any))
            .add_path(settings_path.clone());
        assert!(!settings_event_may_affect(&access, &settings_path));

        let other = Event::new(EventKind::Other).add_path(settings_path.clone());
        assert!(!settings_event_may_affect(&other, &settings_path));
    }

    #[test]
    fn auto_paste_requires_runtime_accessibility_approval() {
        assert!(auto_paste_permitted(false, false));
        assert!(auto_paste_permitted(false, true));
        assert!(!auto_paste_permitted(true, false));
        assert!(auto_paste_permitted(true, true));
    }

    #[test]
    fn startup_keeps_a_valid_hotkey() {
        let requested = "Option+Space";
        let (candidate, error) = startup_hotkey_candidate(requested);

        assert_eq!(candidate, requested);
        assert!(error.is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_replaces_reserved_hotkey_with_safe_operational_fallback() {
        let (candidate, error) = startup_hotkey_candidate("Super+Q");

        assert_eq!(candidate, sagascript_core::settings::Settings::default().hotkey);
        assert!(error
            .as_deref()
            .is_some_and(|message| message.contains("reserved for Quit on macOS")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn startup_replaces_saved_cut_hotkey_with_safe_operational_fallback() {
        let (candidate, error) = startup_hotkey_candidate("Super+X");

        assert_eq!(candidate, sagascript_core::settings::Settings::default().hotkey);
        assert!(error
            .as_deref()
            .is_some_and(|message| message.contains("reserved for Cut on macOS")));
    }

    // -- tray_label --

    #[test]
    fn tray_label_idle_not_failed() {
        assert_eq!(tray_label("idle", false), ("Sagascript", "", "Idle"));
    }

    #[test]
    fn tray_label_recording_not_failed() {
        assert_eq!(
            tray_label("recording", false),
            ("Sagascript - Recording...", "Rec", "Recording...")
        );
    }

    #[test]
    fn tray_label_hotkey_failed_is_distinct_from_idle() {
        let failed = tray_label("idle", true);
        assert_ne!(failed, tray_label("idle", false));
        assert_eq!(failed.2, "Hotkey unavailable");
    }

    #[test]
    fn tray_label_hotkey_failed_wins_over_recording() {
        // The sticky warning must win over a normal state transition into
        // "recording" — a hotkey that isn't registered can't actually be
        // driving a recording state the user trusts.
        assert_eq!(tray_label("recording", true), tray_label("idle", true));
    }

    #[test]
    fn tray_label_hotkey_failed_wins_over_transcribing() {
        assert_eq!(tray_label("transcribing", true), tray_label("idle", true));
    }

    #[test]
    fn tray_label_hotkey_failed_wins_over_loading_model() {
        assert_eq!(tray_label("loading_model", true), tray_label("idle", true));
    }

    #[test]
    fn truncate_for_tray_empty_string_unchanged() {
        assert_eq!(truncate_for_tray(""), "");
    }

    #[test]
    fn truncate_for_tray_ascii_at_threshold_unchanged() {
        let text = "a".repeat(60);
        assert_eq!(truncate_for_tray(&text), text);
    }

    #[test]
    fn truncate_for_tray_ascii_over_threshold_truncated() {
        let text = "a".repeat(61);
        let display = truncate_for_tray(&text);
        assert_eq!(display, format!("{}...", "a".repeat(57)));
    }

    #[test]
    fn truncate_for_tray_multibyte_straddle_does_not_panic() {
        // "å" is 2 bytes in UTF-8; placed so its second byte falls exactly at
        // byte offset 57 — the fixed byte-slice cutoff used before the fix.
        let text = format!("{}å{}", "a".repeat(56), "b".repeat(10));
        let display = truncate_for_tray(&text);
        assert!(std::str::from_utf8(display.as_bytes()).is_ok());
    }

    #[test]
    fn truncate_for_tray_all_multibyte_does_not_panic() {
        // "🎉" is 4 bytes in UTF-8; 16 repeats = 64 bytes, so byte offset 57
        // never lands on a char boundary.
        let text = "🎉".repeat(16);
        let display = truncate_for_tray(&text);
        assert!(std::str::from_utf8(display.as_bytes()).is_ok());
    }

    fn migrate_test_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("sagascript-migrate-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn migrate_legacy_settings_renames_when_new_absent() {
        let dir = migrate_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("flowdictate-settings.json");
        let new_path = dir.join("sagascript-settings.json");
        std::fs::write(&legacy, r#"{"language":"sv"}"#).unwrap();

        migrate_legacy_settings(&legacy, &new_path);

        assert!(!legacy.exists(), "legacy file should be renamed away");
        assert!(new_path.exists(), "new path should now hold the migrated settings");
        assert_eq!(std::fs::read_to_string(&new_path).unwrap(), r#"{"language":"sv"}"#);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_legacy_settings_noop_when_new_already_exists() {
        let dir = migrate_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("flowdictate-settings.json");
        let new_path = dir.join("sagascript-settings.json");
        std::fs::write(&legacy, "legacy-content").unwrap();
        std::fs::write(&new_path, "already-migrated-content").unwrap();

        migrate_legacy_settings(&legacy, &new_path);

        // Neither file should be touched — a Sagascript settings file already exists.
        assert!(legacy.exists());
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "legacy-content");
        assert_eq!(std::fs::read_to_string(&new_path).unwrap(), "already-migrated-content");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_legacy_settings_noop_when_legacy_absent() {
        let dir = migrate_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("flowdictate-settings.json");
        let new_path = dir.join("sagascript-settings.json");

        // Should not panic when there's nothing to migrate.
        migrate_legacy_settings(&legacy, &new_path);

        assert!(!new_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Regression test for the swallowed-rename bug: a failed rename (here,
    /// forced by a destination whose parent directory doesn't exist) must not
    /// panic, and the legacy file must be left in place rather than lost —
    /// the old `let _ = std::fs::rename(...)` code silently discarded the
    /// error either way, but this at least confirms the failure path doesn't
    /// destroy data.
    #[test]
    fn migrate_legacy_settings_does_not_panic_on_rename_failure() {
        let dir = migrate_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("flowdictate-settings.json");
        std::fs::write(&legacy, "legacy-content").unwrap();
        // Destination's parent directory doesn't exist -> rename fails.
        let new_path = dir.join("nonexistent-subdir").join("sagascript-settings.json");

        migrate_legacy_settings(&legacy, &new_path);

        assert!(legacy.exists(), "legacy file must survive a failed rename");
        assert!(!new_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
