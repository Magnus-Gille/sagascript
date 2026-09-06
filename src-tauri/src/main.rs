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
#[path = "paste/completion.rs"]
mod paste_completion;
mod platform;
mod updates;

use tracing_subscriber::EnvFilter;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// Grace after a timeout-triggered abort for the blocking inference to unwind and
/// release the warm-state lock before we log it as still stuck.
const ABORT_GRACE_SECS: u64 = 5;

/// Maximum time to wait for the native paste callback to report its result.
/// macOS paste stays on its mandatory main thread; other platforms use a
/// blocking worker. A lost callback must not leave dictation stuck processing.
const PASTE_COMPLETION_TIMEOUT_MS: u64 = paste_completion::COMPLETION_TIMEOUT_MS;

use tauri::{
    menu::{Menu, MenuItem, Submenu},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::ShortcutState;
use tracing::{error, info, warn};

use app_controller::AppState;
use app_controller::{AppController, HotkeyDownResult, StopRecordingOutcome};
use commands::{SharedController, SharedWhisper};
use sagascript_core::settings::HotkeyProfile;
#[cfg(test)]
use sagascript_core::settings::{validate_hotkey, Language};
use sagascript_core::transcription::{
    WARM_MODEL_CACHE_BUDGET_MB, WARM_MODEL_CACHE_MAX_MODELS, WhisperBackend,
};

/// Minimum recording duration before we allow stop (300ms)
const MIN_RECORDING_MS: u64 = 300;
const SAFE_FALLBACK_HOTKEY: &str = "Control+Shift+Space";
const BUILD_IDENTITY: &str = concat!(
    "Version ",
    env!("CARGO_PKG_VERSION"),
    " · Build ",
    env!("GIT_HASH"),
    " · ",
    env!("BUILD_DATE"),
);
#[cfg(target_os = "macos")]
const TRAY_AUTOSAVE_NAME: &str = "ai.gille.sagascript.main";
#[cfg(target_os = "macos")]
const DEFAULT_TRAY_PREFERRED_POSITION: f64 = 340.0;

/// Shared tray status menu item for updating from anywhere
type SharedStatusItem = Mutex<Option<MenuItem<tauri::Wry>>>;

struct ProfileMenuState {
    submenu: Submenu<tauri::Wry>,
    items: Vec<MenuItem<tauri::Wry>>,
    selected_profile_id: Option<String>,
}

type SharedProfileMenuState = Mutex<Option<ProfileMenuState>>;

fn format_menu_shortcut(shortcut: &str) -> String {
    let parts: Vec<(&str, String)> = shortcut
        .split('+')
        .map(|part| {
            let original = part.trim();
            let normalized = match original.to_ascii_lowercase().as_str() {
                "commandorcontrol" | "commandorctrl" | "cmdorctrl" | "cmdorcontrol" => {
                    if cfg!(target_os = "macos") { "command" } else { "control" }
                }
                _ => original,
            };
            (original, normalized.to_ascii_lowercase())
        })
        .collect();
    let mut label = String::new();
    for (aliases, symbol) in [
        (&["control", "ctrl"][..], "⌃"),
        (&["alt", "option"][..], "⌥"),
        (&["shift"][..], "⇧"),
        (&["super", "command", "cmd"][..], "⌘"),
    ] {
        if parts
            .iter()
            .any(|(_, normalized)| aliases.contains(&normalized.as_str()))
        {
            label.push_str(symbol);
        }
    }
    for (original, normalized) in parts {
        if matches!(
            normalized.as_str(),
            "control" | "ctrl" | "alt" | "option" | "shift" | "super" | "command" | "cmd"
        ) {
            continue;
        }
        label.push_str(match normalized.as_str() {
            "space" => "Space",
            "arrowup" | "up" => "↑",
            "arrowdown" | "down" => "↓",
            "arrowleft" | "left" => "←",
            "arrowright" | "right" => "→",
            _ => original,
        });
    }
    label
}

fn profile_menu_label(profile: &HotkeyProfile, selected: bool) -> String {
    format!(
        "{}{} — {} · {}",
        if selected { "✓ " } else { "" },
        profile.name,
        profile.language.display_name(),
        format_menu_shortcut(&profile.shortcut)
    )
}

fn create_profile_menu_items(
    app: &impl tauri::Manager<tauri::Wry>,
    profiles: &[HotkeyProfile],
    selected_profile_id: Option<&str>,
) -> tauri::Result<Vec<MenuItem<tauri::Wry>>> {
    let mut items = Vec::with_capacity(profiles.len() + 1);
    for profile in profiles {
        items.push(MenuItem::with_id(
            app,
            format!("profile_shortcut_{}", profile.id),
            profile_menu_label(profile, selected_profile_id == Some(profile.id.as_str())),
            false,
            None::<&str>,
        )?);
    }
    items.push(MenuItem::with_id(
        app,
        "manage_profiles",
        "Edit Profiles…",
        true,
        None::<&str>,
    )?);
    Ok(items)
}

pub(crate) fn update_profiles_menu(app: &tauri::AppHandle, profiles: &[HotkeyProfile]) {
    let state: tauri::State<'_, SharedProfileMenuState> = app.state();
    let mut state = state.lock().unwrap();
    let Some(state) = state.as_mut() else {
        return;
    };

    let selected_profile_id = state
        .selected_profile_id
        .as_deref()
        .filter(|selected| profiles.iter().any(|profile| profile.id == *selected))
        .map(str::to_string)
        .or_else(|| profiles.first().map(|profile| profile.id.clone()));

    for item in state.items.drain(..) {
        if let Err(error) = state.submenu.remove(&item) {
            error!("Failed to remove stale tray profile item: {error}");
        }
    }
    match create_profile_menu_items(app, profiles, selected_profile_id.as_deref()) {
        Ok(items) => {
            for item in &items {
                if let Err(error) = state.submenu.append(item) {
                    error!("Failed to add tray profile item: {error}");
                }
            }
            state.items = items;
            state.selected_profile_id = selected_profile_id;
        }
        Err(error) => error!("Failed to rebuild tray profiles menu: {error}"),
    }
}

fn select_profile_menu(app: &tauri::AppHandle, profile: &HotkeyProfile) {
    let profiles = {
        let controller: tauri::State<'_, SharedController> = app.state();
        let profiles = controller.lock().unwrap().settings().resolved_hotkey_profiles();
        profiles
    };
    {
        let state: tauri::State<'_, SharedProfileMenuState> = app.state();
        let mut state = state.lock().unwrap();
        if let Some(state) = state.as_mut() {
            state.selected_profile_id = Some(profile.id.clone());
        }
    }
    update_profiles_menu(app, &profiles);
}

#[derive(Clone)]
struct UpdateMenuItems {
    status: MenuItem<tauri::Wry>,
    check: MenuItem<tauri::Wry>,
}

struct UpdateMenuState {
    items: Option<UpdateMenuItems>,
    checking: bool,
    available_version: Option<semver::Version>,
}

type SharedUpdateMenuState = Mutex<UpdateMenuState>;

const UPDATE_CHECK_ACTION: &str = "Check for Updates…";

fn update_status_text(result: &updates::UpdateCheck) -> String {
    match result {
        updates::UpdateCheck::Available { version } => {
            format!("Update available — v{version}")
        }
        updates::UpdateCheck::UpToDate => "Sagascript is up to date".to_string(),
    }
}

fn update_action_text(result: &updates::UpdateCheck) -> String {
    match result {
        updates::UpdateCheck::Available { version } => {
            format!("Download Sagascript v{version}…")
        }
        updates::UpdateCheck::UpToDate => "Check Again…".to_string(),
    }
}

fn stable_release_url(version: &semver::Version) -> String {
    format!("https://github.com/Magnus-Gille/sagascript/releases/tag/v{version}")
}

fn open_update_release(version: &semver::Version) -> Result<(), String> {
    let url = stable_release_url(version);

    #[cfg(target_os = "macos")]
    let mut command = std::process::Command::new("open");
    #[cfg(target_os = "linux")]
    let mut command = std::process::Command::new("xdg-open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = std::process::Command::new("cmd");
        command.args(["/C", "start", ""]);
        command
    };

    command
        .arg(&url)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to open {url}: {error}"))
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
        let (status_text, action_text, available_version, check_error) = match result {
            Ok(result) => {
                let available_version = match &result {
                    updates::UpdateCheck::Available { version } => Some(version.clone()),
                    updates::UpdateCheck::UpToDate => None,
                };
                (
                    update_status_text(&result),
                    update_action_text(&result),
                    available_version,
                    None,
                )
            }
            Err(error) => (
                "Couldn't check for updates".to_string(),
                "Try Again…".to_string(),
                None,
                Some(error),
            ),
        };
        if let Some(error) = check_error {
            warn!("Update check failed: {error}");
        }

        let items = {
            let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
            let mut state = state.lock().unwrap();
            state.checking = false;
            state.available_version = available_version;
            state.items.clone()
        };
        let Some(items) = items else {
            return;
        };
        if let Err(error) = items.status.set_text(status_text) {
            error!("Failed to set tray update status: {error}");
        }
        if let Err(error) = items.check.set_text(action_text) {
            error!("Failed to set tray update action: {error}");
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

fn handle_hotkey_event(app: &tauri::AppHandle, shortcut: &str, state: hotkey::BareHotkeyState) {
    let ctrl: tauri::State<'_, SharedController> = app.state();

    match state {
        hotkey::BareHotkeyState::Pressed => {
            info!("Hotkey pressed: {shortcut}");
            let (result, active_profile) = {
                let mut c = ctrl.lock().unwrap();
                let profile = c
                    .settings()
                    .hotkey_profile_for_shortcut(shortcut)
                    .or_else(|| {
                        (shortcut == SAFE_FALLBACK_HOTKEY)
                            .then(|| c.settings().resolved_hotkey_profiles()[0].clone())
                    });
                let result = match profile {
                    Some(profile) => match c.handle_hotkey_down_for_profile(profile) {
                        Ok(result) => result,
                        Err(error) => {
                            error!("Hotkey down error: {error}");
                            let _ = app.emit(events::event::ERROR, error.to_string());
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
                        select_profile_menu(app, &profile);
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
        hotkey::BareHotkeyState::Released => {
            info!("Hotkey released: {shortcut}");
            handle_hotkey_release(app, &ctrl, shortcut);
        }
    }
}
fn main() {
    let gui_launch_mode = gui_launch_mode(std::env::args_os());

    // CLI mode: if a subcommand is given, run CLI and exit. The desktop
    // binary is a full CLI (CLI-first design) — the GUI only launches on a
    // bare invocation. The private GUI-open marker is consumed here rather
    // than passed to clap so `sagascript open` can explicitly reveal Settings.
    if gui_launch_mode == GuiLaunchMode::Standard {
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

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !second_instance_requests_settings(&args) {
                info!("Background second-instance launch ignored");
                return;
            }

            // The callback is delivered by the single-instance transport,
            // which is not guaranteed to be the Tauri main thread (notably on
            // macOS). Queue all state/UI work onto the main thread. This also
            // makes the Windows WM_COPYDATA callback return promptly instead
            // of keeping the secondary process blocked on a window operation.
            let app = app.clone();
            dispatch_to_main(&app, move |app| {
                let should_reveal = app
                    .try_state::<SharedController>()
                    .map(|ctrl| should_reveal_for_reopen(ctrl.lock().unwrap().state()))
                    .unwrap_or(true);
                if should_reveal {
                    info!("Second-instance launch requested Settings");
                    open_settings_window(app, None);
                } else {
                    info!("Ignoring second-instance launch while dictation is active");
                }
            });
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    let state = match event.state {
                        ShortcutState::Pressed => hotkey::BareHotkeyState::Pressed,
                        ShortcutState::Released => hotkey::BareHotkeyState::Released,
                    };
                    handle_hotkey_event(app, &shortcut.to_string(), state);
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![sagascript_cli::open::GUI_BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // `tauri-plugin-single-instance` is initialized before Tauri calls
            // setup. Keep backend construction here so a secondary process is
            // rejected before it loads settings, opens audio resources, or
            // creates a Whisper backend.
            let settings = load_settings_with_permission_gate();
            info!("Loaded settings: language={:?}, model={:?}, hotkey={}", settings.language, settings.whisper_model, settings.hotkey);
            let initial_hotkey = settings.hotkey.clone();
            let controller: SharedController = Mutex::new(AppController::new(settings));
            let whisper: SharedWhisper = Arc::new(WhisperBackend::new());
            app.manage(controller);
            app.manage(whisper);
            // Process-wide hotkey registration health (see hotkey::health for
            // why this is deliberately independent of the AppController
            // mutex). Assumed healthy until the synchronous registration
            // attempt below proves otherwise.
            app.manage(hotkey::HotkeyHealth::new(&initial_hotkey));
            let status_item: SharedStatusItem = Mutex::new(None);
            let profile_menu: SharedProfileMenuState = Mutex::new(None);
            let update_menu: SharedUpdateMenuState = Mutex::new(UpdateMenuState {
                items: None,
                checking: false,
                available_version: None,
            });
            app.manage(status_item);
            app.manage(profile_menu);
            app.manage(update_menu);

            // Hide from dock on macOS (tray-only app)
            #[cfg(target_os = "macos")]
            {
                platform::macos::set_activation_policy_accessory();
                if let Err(error) = hotkey::install_bare_function_key_monitor(app.handle()) {
                    error!("Failed to install F13-F24 event monitor: {error}");
                }
            }

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
            let profiles = {
                let ctrl: tauri::State<'_, SharedController> = app.state();
                let profiles = ctrl.lock().unwrap().settings().resolved_hotkey_profiles();
                profiles
            };
            let selected_profile_id = profiles.first().map(|profile| profile.id.clone());
            let profile_items =
                create_profile_menu_items(app, &profiles, selected_profile_id.as_deref())?;
            let profiles_menu = Submenu::new(app, "Profiles", true)?;
            for item in &profile_items {
                profiles_menu.append(item)?;
            }
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
            let build_info_item =
                MenuItem::with_id(app, "build_info", BUILD_IDENTITY, false, None::<&str>)?;

            // Store status item so we can update it after transcription
            {
                let status_state: tauri::State<'_, SharedStatusItem> = app.state();
                *status_state.lock().unwrap() = Some(status.clone());
            }
            {
                let profile_state: tauri::State<'_, SharedProfileMenuState> = app.state();
                *profile_state.lock().unwrap() = Some(ProfileMenuState {
                    submenu: profiles_menu.clone(),
                    items: profile_items,
                    selected_profile_id,
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
                    &profiles_menu,
                    &update_status,
                    &check_for_updates_item,
                    &build_info_item,
                    &settings_item,
                    &transcribe_file_item,
                    &quit,
                ],
            )?;

            #[cfg(target_os = "macos")]
            seed_macos_tray_preferred_position();

            let tray_builder = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .tooltip("Sagascript")
                // macOS 26 can register an image-backed status item but paint
                // it blank. Compact native text stays visible: S while idle,
                // then a state marker while recording or transcribing.
                .title("S");
            #[cfg(target_os = "windows")]
            let tray_builder = match app.default_window_icon() {
                Some(icon) => tray_builder.icon(icon.clone()),
                None => return Err("Windows tray icon is missing from the app bundle".into()),
            };
            let tray = tray_builder
                .on_menu_event(move |app, event| {
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
                    "manage_profiles" => {
                        open_settings_window(app, Some("dictate"));
                    }
                    "check_for_updates" => {
                        let available_version = {
                            let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
                            let version = state.lock().unwrap().available_version.clone();
                            version
                        };
                        if let Some(version) = available_version {
                            if let Err(error) = open_update_release(&version) {
                                error!("Failed to open update release: {error}");
                            } else {
                                let items = {
                                    let state: tauri::State<'_, SharedUpdateMenuState> = app.state();
                                    let mut state = state.lock().unwrap();
                                    state.available_version = None;
                                    state.items.clone()
                                };
                                if let Some(items) = items {
                                    if let Err(error) = items.check.set_text("Check Again…") {
                                        error!("Failed to reset update action after opening release: {error}");
                                    }
                                }
                            }
                        } else {
                            check_for_updates(app.clone());
                        }
                    }
                    _ => {}
                }
                })
                .build(app)?;

            #[cfg(target_os = "macos")]
            configure_macos_tray_identity(&tray)?;
            tray.set_visible(true)?;

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
                    let new_path = sagascript_core::settings::store::settings_path();
                    migrate_legacy_settings_unless_overridden(
                        &legacy,
                        &new_path,
                        sagascript_core::settings::store::settings_path_is_overridden(),
                    );
                }
            }

            // Watch settings file for external changes (e.g. `sagascript config set`)
            start_settings_watcher(app.handle().clone());

            // Only the login-item marker stays headless after onboarding.
            // A normal Finder/Spotlight launch is deliberate and opens
            // Settings; hotkey recording/transcription never enters this path.
            {
                let settings = sagascript_core::settings::store::load();
                match initial_window_request(settings.has_completed_onboarding, gui_launch_mode) {
                    InitialWindowRequest::Hidden => {
                        info!("Background launch complete; Settings remains hidden");
                    }
                    InitialWindowRequest::Settings => {
                        info!("Foreground GUI launch requested");
                        open_settings_window(app.handle(), None);
                    }
                    InitialWindowRequest::Onboarding => {
                        info!("First launch detected, opening onboarding");
                        open_settings_window(app.handle(), Some("onboarding"));
                    }
                }
            }

            // Preload + warm the bounded set of models selected by the hotkey
            // profiles. The primary profile is restored as active after warmup,
            // while one distinct secondary stays resident for instant bilingual
            // switching. Missing models are never downloaded implicitly.
            {
                let whisper: tauri::State<'_, SharedWhisper> = app.state();
                let whisper = whisper.inner().clone();
                let (warm_plan, vad_enabled) = {
                    let ctrl: tauri::State<'_, SharedController> = app.state();
                    let c = ctrl.lock().unwrap();
                    (
                        c.settings().warm_model_plan(
                            WARM_MODEL_CACHE_MAX_MODELS,
                            WARM_MODEL_CACHE_BUDGET_MB,
                        ),
                        c.settings().vad_enabled,
                    )
                };
                std::thread::spawn(move || {
                    let Some(&(primary_model, primary_language)) = warm_plan.first() else {
                        return;
                    };

                    if let Err(e) = whisper.ensure_model(primary_model) {
                        warn!("Primary model preload skipped: {e}");
                        return;
                    }
                    if let Err(e) = whisper.warmup_model(primary_model, primary_language) {
                        warn!("Primary model warmup failed: {e}");
                    } else {
                        info!(
                            "Primary model preloaded and warmed: {}",
                            primary_model.display_name()
                        );
                    }

                    for &(model, language) in warm_plan.iter().skip(1) {
                        if let Err(e) = whisper.ensure_model(model) {
                            warn!("Secondary model preload skipped: {e}");
                            continue;
                        }
                        if let Err(e) = whisper.warmup_model(model, language) {
                            warn!("Secondary model warmup failed: {e}");
                        } else {
                            info!(
                                "Secondary model preloaded and warmed: {}",
                                model.display_name()
                            );
                        }
                    }

                    if whisper.loaded_model() != Some(primary_model) {
                        if let Err(e) = whisper.ensure_model(primary_model) {
                            warn!("Could not restore primary model after warmup: {e}");
                        }
                    }

                    info!(
                        "Warm resident models ready: {:?}",
                        whisper.resident_models()
                    );
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
            commands::retry_hotkey_registration,
            commands::hotkey_status,
            commands::start_recording,
            commands::start_training_recording,
            commands::stop_and_transcribe,
            commands::stop_and_transcribe_training,
            commands::transcribe_training_file,
            commands::cancel_recording,
            commands::is_model_downloaded,
            commands::get_model_info,
            commands::get_effective_model_info,
            commands::download_model,
            commands::set_auto_paste,
            commands::set_show_overlay,
            commands::set_initial_prompt,
            commands::suggest_training_glossary,
            commands::apply_training_glossary,
            commands::set_beam_size,
            commands::set_temperature_fallback,
            commands::set_vad_enabled,
            commands::get_build_info,
            commands::transcribe_file,
            commands::get_supported_formats,
            commands::check_accessibility_permission,
            commands::request_accessibility_permission,
            commands::open_accessibility_settings,
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
                    let state = {
                        let ctrl: tauri::State<'_, SharedController> = _app_handle.state();
                        let state = ctrl.lock().unwrap().state();
                        state
                    };
                    if should_reveal_for_reopen(state) {
                        info!("Application reopen requested");
                        open_settings_window(_app_handle, None);
                    } else {
                        info!("Ignoring application reopen while dictation is {state:?}");
                    }
                }
                _ => {}
            }
        });
}

/// Match the key AppKit uses to persist a named status item's menu-bar slot.
#[cfg(target_os = "macos")]
fn tray_preferred_position_key(autosave_name: &str) -> String {
    format!("NSStatusItem Preferred Position {autosave_name}")
}

#[cfg(target_os = "macos")]
fn initial_tray_preferred_position(has_saved_position: bool) -> Option<f64> {
    (!has_saved_position).then_some(DEFAULT_TRAY_PREFERRED_POSITION)
}

/// Seed a usable first position on crowded, notched menu bars. AppKit normally
/// puts a brand-new status item at the far-left edge of the status area, where
/// it may report `isVisible = true` while sitting behind the camera cutout.
/// Once the user moves the item, AppKit owns this value and we never overwrite
/// it.
#[cfg(target_os = "macos")]
fn seed_macos_tray_preferred_position() {
    use objc2_foundation::{NSString, NSUserDefaults};

    let defaults = NSUserDefaults::standardUserDefaults();
    let key_string = tray_preferred_position_key(TRAY_AUTOSAVE_NAME);
    let key = NSString::from_str(&key_string);
    let Some(position) =
        initial_tray_preferred_position(defaults.objectForKey(&key).is_some())
    else {
        return;
    };

    defaults.setDouble_forKey(position, &key);
    info!(position, "Seeded initial macOS tray position");
}

/// Give AppKit a stable identity for the status item before making it visible.
/// Without an autosave name macOS 26 places every fresh item in the default
/// leftmost slot, which can sit behind a MacBook camera cutout.
#[cfg(target_os = "macos")]
fn configure_macos_tray_identity(tray: &tauri::tray::TrayIcon) -> tauri::Result<()> {
    let configured = tray.with_inner_tray_icon(|inner| {
        let Some(status_item) = inner.ns_status_item() else {
            return false;
        };

        let autosave_name = objc2_foundation::NSString::from_str(TRAY_AUTOSAVE_NAME);
        // Tauri constructs the native item visible. Toggle it inside the same
        // main-thread callback so the stable name is in place before AppKit
        // performs the visible placement pass.
        status_item.setVisible(false);
        status_item.setAutosaveName(Some(&autosave_name));
        status_item.setVisible(true);

        status_item.autosaveName().to_string() == TRAY_AUTOSAVE_NAME
    })?;

    if configured {
        Ok(())
    } else {
        Err(std::io::Error::other("failed to assign the macOS tray autosave name").into())
    }
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
        return ("Sagascript - Hotkey unavailable", "!", "Hotkey unavailable");
    }
    match state {
        "recording" => ("Sagascript - Recording...", "●", "Recording..."),
        "loading_model" => ("Sagascript - Loading model...", "…", "Loading model..."),
        "transcribing" => ("Sagascript - Transcribing...", "…", "Transcribing..."),
        _ => ("Sagascript", "S", "Idle"),
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

fn migrate_legacy_settings_unless_overridden(
    legacy: &std::path::Path,
    new_path: &std::path::Path,
    settings_path_is_overridden: bool,
) {
    if settings_path_is_overridden {
        return;
    }
    migrate_legacy_settings(legacy, new_path);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitialWindowRequest {
    Hidden,
    Settings,
    Onboarding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GuiLaunchMode {
    Standard,
    ShowSettings,
    Background,
}

fn gui_launch_mode(args: impl IntoIterator<Item = std::ffi::OsString>) -> GuiLaunchMode {
    let mut args = args.into_iter();
    let _program = args.next();
    match (args.next(), args.next()) {
        (Some(argument), None)
            if argument == std::ffi::OsStr::new(sagascript_cli::open::GUI_OPEN_ARG) =>
        {
            GuiLaunchMode::ShowSettings
        }
        (Some(argument), None)
            if argument == std::ffi::OsStr::new(sagascript_cli::open::GUI_BACKGROUND_ARG) =>
        {
            GuiLaunchMode::Background
        }
        _ => GuiLaunchMode::Standard,
    }
}

fn second_instance_requests_settings(args: &[String]) -> bool {
    // `tauri-plugin-single-instance` forwards the complete argv on Windows,
    // including argv[0], but transports differ across platforms and versions.
    // Treat the private background marker as a capability to stay hidden and
    // let every other relaunch (including a bare Start-menu launch) reveal
    // Settings. Looking for the marker anywhere is robust to either argv
    // shape and to a future plugin adding metadata arguments.
    !args
        .iter()
        .any(|argument| argument == sagascript_cli::open::GUI_BACKGROUND_ARG)
}

fn initial_window_request(
    has_completed_onboarding: bool,
    launch_mode: GuiLaunchMode,
) -> InitialWindowRequest {
    if !has_completed_onboarding {
        InitialWindowRequest::Onboarding
    } else if launch_mode == GuiLaunchMode::Background {
        InitialWindowRequest::Hidden
    } else {
        InitialWindowRequest::Settings
    }
}

fn should_reveal_for_reopen(state: AppState) -> bool {
    !state.is_busy()
}

trait MainWindowVisibility {
    fn activate_app(&self);
    fn reset_to_normal_level(&self) -> Result<(), String>;
    fn unminimize(&self) -> Result<(), String>;
    fn show(&self) -> Result<(), String>;
    fn set_focus(&self) -> Result<(), String>;
}

impl MainWindowVisibility for tauri::WebviewWindow {
    fn activate_app(&self) {
        #[cfg(target_os = "macos")]
        if let Err(error) = self
            .app_handle()
            .run_on_main_thread(platform::macos::activate_app)
        {
            warn!("Failed to schedule foreground application activation: {error}");
        }
    }

    fn reset_to_normal_level(&self) -> Result<(), String> {
        tauri::WebviewWindow::set_always_on_top(self, false).map_err(|error| error.to_string())
    }

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
    if let Err(error) = window.reset_to_normal_level() {
        warn!("Failed to restore normal main-window level: {error}");
    }
    window
        .unminimize()
        .map_err(|error| format!("failed to restore main window: {error}"))?;
    window
        .show()
        .map_err(|error| format!("failed to show main window: {error}"))?;
    window
        .set_focus()
        .map_err(|error| format!("failed to focus main window: {error}"))?;
    // Queue activation after presentation. During initial `.setup()` a direct
    // AppKit activation happens before Tauri's event loop is ready and macOS
    // leaves the onboarding window behind the previously active application.
    window.activate_app();
    Ok(())
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
        .always_on_top(false)
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

/// Run a UI closure on Tauri's main thread. Native tray/window APIs must not be
/// touched from a transport or worker thread; best-effort — logs if dispatch
/// itself fails.
fn dispatch_to_main<F>(app: &tauri::AppHandle, f: F)
where
    F: FnOnce(&tauri::AppHandle) + Send + 'static,
{
    let app_for_closure = app.clone();
    if let Err(e) = app.run_on_main_thread(move || f(&app_for_closure)) {
        error!("Failed to dispatch UI work to main thread: {e}");
    }
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn elapsed_ms_if_success<T, E>(result: &Result<T, E>, start: Instant) -> Option<u64> {
    result.as_ref().ok().map(|_| elapsed_ms(start))
}

/// Stop recording, enforce minimum duration, and spawn transcription.
/// Shared by both push-to-talk (on key-up) and toggle (on second key-down).
fn stop_recording_and_transcribe(
    app: &tauri::AppHandle,
    ctrl: &tauri::State<'_, SharedController>,
) {
    let key_up_at = Instant::now();

    // Compute how long we still need to hold to satisfy the minimum recording
    // duration — but do NOT block the global-shortcut (UI) thread waiting for it
    // (finding 2): a std::thread::sleep here freezes UI redraw and stalls
    // subsequent hotkey events. The delay is offloaded to an async task below.
    let elapsed = {
        let mut c = ctrl.lock().unwrap();
        c.mark_release();
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
        let key_up_to_capture_stopped_ms = elapsed_ms(key_up_at);

        // Keep the recording indicator visible through processing, so key-up
        // never leaves the user without feedback while inference/paste runs.
        // Show the transcribing state — re-dispatched to the main
        // thread now that this runs on a worker.
        dispatch_to_main(&app_handle, |app| {
            update_tray_status(app, "transcribing");
        });
        let _ = app_handle.emit(events::event::STATE_CHANGED, "transcribing");

        if audio.is_empty() {
            {
                let ctrl: tauri::State<'_, SharedController> = app_handle.state();
                let mut c = ctrl.lock().unwrap();
                c.log_dictation_performance(serde_json::json!({
                    "outcome": "no_speech",
                    "keyUpToCaptureStoppedMs": key_up_to_capture_stopped_ms,
                    "keyUpToModelReadyMs": null,
                    "modelLoadMs": null,
                    "whisperMs": null,
                    "keyUpToPasteCompletedMs": null,
                    "totalMs": elapsed_ms(key_up_at),
                }));
                c.on_no_speech_detected();
            }
            dispatch_to_main(&app_handle, |app| {
                overlay::hide(app);
                update_tray_status(app, "idle");
            });
            let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
            return;
        }

        // Transcribe (timeout/cancellation logic is owned by a separate work
        // package — left unchanged). Runs in this same task, which is already
        // off the UI thread.
        let ctrl: tauri::State<'_, SharedController> = app_handle.state();
        let whisper: tauri::State<'_, SharedWhisper> = app_handle.state();

        // Extract what we need for transcription (lock briefly)
        let (language, effective_model, opts, glossary) = {
            let c = ctrl.lock().unwrap();
            let profile_id = c.active_hotkey_profile().map(|profile| profile.id.as_str());
            (
                c.language(),
                c.settings().effective_model_for(c.language()),
                commands::build_transcribe_options_for_profile(c.settings(), profile_id),
                sagascript_core::transcription::Glossary::parse(
                    &c.settings().effective_glossary_source(profile_id),
                ),
            )
        };

        let model_name = effective_model.display_name().to_string();
        let language_name = language.display_name().to_string();
        let beam_size = opts.beam_size;
        let temperature_fallback = opts.temperature_fallback;
        let vad_enabled = opts.vad_model_path.is_some();
        let mut model_was_warm = !whisper.needs_reload(effective_model);
        info!("Transcribing with model: {model_name}");

        // Show model loading status in tray
        if !model_was_warm {
            let _ = app_handle.emit(events::event::STATE_CHANGED, "loading_model");
            dispatch_to_main(&app_handle, |app| update_tray_status(app, "loading_model"));
        }

        // Model selection and inference stay in one bounded transaction on
        // the blocking worker. Do not reintroduce a separate ensure_model call:
        // another language could otherwise replace the selected context.
        let mut model_load_ms = None;
        let mut key_up_to_model_ready_ms = None;
        let mut whisper_ms = None;
        let result = {
            // Run blocking transcription on a separate thread with a timeout. On
            // timeout we trigger a REAL abort (whisper-rs abort callback wired in
            // WhisperBackend): request_abort() flips the flag whisper.cpp checks
            // between compute steps, so the blocking task returns and releases the
            // warm state instead of running to completion and wedging the pipeline.
            let whisper_ref = whisper.inner().clone();
            let mut fut = tokio::task::spawn_blocking(move || {
                let mut timings = sagascript_core::transcription::whisper_backend::DictationTimings::default();
                let result = whisper_ref.transcribe_live_dictation(effective_model, &audio, language, &opts, &mut timings);
                (result, timings)
            });

            let timeout = Duration::from_secs(TRANSCRIPTION_TIMEOUT_SECS);
            match tokio::time::timeout(timeout, &mut fut).await {
                Ok(Ok((r, timings))) => {
                    let mut c = ctrl.lock().unwrap();
                    if timings.model_acquisition_started {
                        model_load_ms = Some(timings.model_ms);
                        model_was_warm = timings.model_cached;
                        c.record_phase("model_acquisition", Duration::from_secs_f64(timings.model_ms / 1000.0));
                        c.record_model_cache(timings.model_cached);
                    }
                    key_up_to_model_ready_ms = timings.model_ready_at.map(|ready| {
                        u64::try_from(ready.saturating_duration_since(key_up_at).as_millis())
                            .unwrap_or(u64::MAX)
                    });
                    if timings.inference_started {
                        whisper_ms = Some(timings.inference_ms);
                        c.record_phase("inference", Duration::from_secs_f64(timings.inference_ms / 1000.0));
                    }
                    r
                }
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

        let key_up_to_whisper_complete_ms = whisper_ms
            .and_then(|_| elapsed_ms_if_success(&result, key_up_at));
        let postprocess_started = std::time::Instant::now();
        let result = result.map(|text| commands::apply_glossary(text, &glossary));
        ctrl.lock().unwrap().record_phase("postprocessing", postprocess_started.elapsed());
        match result {
            Ok(text) => {
                info!("Transcription complete: {} chars", text.len());

                if text.trim().is_empty() {
                    let mut c = ctrl.lock().unwrap();
                    c.log_dictation_performance(serde_json::json!({
                        "outcome": "no_speech",
                        "model": model_name,
                        "language": language_name,
                        "modelWasWarm": model_was_warm,
                        "beamSize": beam_size,
                        "temperatureFallback": temperature_fallback,
                        "vadEnabled": vad_enabled,
                        "keyUpToCaptureStoppedMs": key_up_to_capture_stopped_ms,
                        "modelLoadMs": model_load_ms,
                        "keyUpToModelReadyMs": key_up_to_model_ready_ms,
                        "whisperMs": whisper_ms,
                        "keyUpToWhisperCompleteMs": key_up_to_whisper_complete_ms,
                        "keyUpToPasteCompletedMs": null,
                        "totalMs": elapsed_ms(key_up_at),
                    }));
                    c.on_no_speech_detected();
                    drop(c);
                    let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                    dispatch_to_main(&app_handle, |app| {
                        overlay::hide(app);
                        update_tray_status(app, "idle");
                    });
                    info!("No speech detected; returning to idle without paste");
                    return;
                }

                // Check if auto-paste is enabled (lock briefly)
                let should_paste = {
                    let c = ctrl.lock().unwrap();
                    c.settings().auto_paste
                };

                let mut paste_outcome = "disabled";
                let mut key_up_to_paste_completed_ms = None;
                let mut paste_error = None;
                if should_paste {
                    // Auto-paste MUST run on the main thread — enigo's macOS TIS APIs
                    // crash (SIGABRT) if called from a tokio worker thread.
                    let text_for_paste = text.clone();
                    let paste_started = std::time::Instant::now();
                    let (paste_tx, paste_rx) = tokio::sync::oneshot::channel();
                    let paste_task = move || {
                        let paste_result = crate::paste::PasteService::new()
                            .paste(&text_for_paste)
                            .map_err(|error| error.to_string());
                        match &paste_result {
                            Ok(()) => info!("Auto-paste completed successfully"),
                            Err(e) => error!("Auto-paste failed: {e}"),
                        }
                        let _ = paste_tx.send(paste_result);
                    };
                    #[cfg(target_os = "macos")]
                    let dispatch_result = app_handle.run_on_main_thread(paste_task);
                    #[cfg(not(target_os = "macos"))]
                    let dispatch_result: Result<(), String> = {
                        // Clipboard focus/paste can block on Windows. Keep its
                        // native UI thread responsive while preserving macOS's
                        // mandatory main-thread execution above.
                        tokio::task::spawn_blocking(paste_task);
                        Ok(())
                    };
                    if let Err(e) = dispatch_result {
                        error!("Failed to dispatch paste to main thread: {e}");
                        paste_outcome = "dispatch_failed";
                        paste_error = Some("Could not dispatch automatic paste. Copy the recognized text from Dictate.".to_string());
                    } else {
                        let completion = paste_completion::wait(
                            paste_rx,
                            Duration::from_millis(PASTE_COMPLETION_TIMEOUT_MS),
                        )
                        .await;
                        paste_outcome = completion.outcome;
                        paste_error = completion.error;
                        if completion.call_completed {
                            key_up_to_paste_completed_ms = Some(elapsed_ms(key_up_at));
                        } else if paste_outcome == "timed_out" {
                            warn!("Auto-paste completion timed out after {PASTE_COMPLETION_TIMEOUT_MS}ms");
                        }
                    }
                    ctrl.lock().unwrap().record_phase("clipboard_focus_paste", paste_started.elapsed());
                }

                let mut c = ctrl.lock().unwrap();
                c.log_dictation_performance(serde_json::json!({
                    "outcome": "success",
                    "model": model_name,
                    "language": language_name,
                    "modelWasWarm": model_was_warm,
                    "beamSize": beam_size,
                    "temperatureFallback": temperature_fallback,
                    "vadEnabled": vad_enabled,
                    "keyUpToCaptureStoppedMs": key_up_to_capture_stopped_ms,
                    "modelLoadMs": model_load_ms,
                    "keyUpToModelReadyMs": key_up_to_model_ready_ms,
                    "whisperMs": whisper_ms,
                    "keyUpToWhisperCompleteMs": key_up_to_whisper_complete_ms,
                    "pasteOutcome": paste_outcome,
                    "keyUpToPasteCompletedMs": key_up_to_paste_completed_ms,
                    "totalMs": elapsed_ms(key_up_at),
                }));
                if let Some(message) = paste_error {
                    // Log the phase event while correlation is still active,
                    // then finish the terminal session exactly once as error.
                    // The successful recognition remains available for copy.
                    c.preserve_transcription(&text);
                    c.on_transcription_error(&message);
                    drop(c);
                    let _ = app_handle.emit(events::event::TRANSCRIPTION_RESULT, &text);
                    let _ = app_handle.emit(events::event::ERROR, message);
                    let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                    let open_copy_fallback = paste_completion::should_open_copy_fallback(paste_outcome);
                    dispatch_to_main(&app_handle, move |app| {
                        overlay::hide(app);
                        update_tray_status(app, "idle");
                        if open_copy_fallback {
                            open_settings_window(app, Some("dictate"));
                        }
                    });
                    return;
                }
                c.on_transcription_success(&text);
                drop(c);

                let _ = app_handle.emit(events::event::TRANSCRIPTION_RESULT, &text);
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                let text_for_tray = text.clone();
                dispatch_to_main(&app_handle, move |app| {
                    overlay::hide(app);
                    update_tray_status(app, "idle");
                    update_tray_last_result(app, &text_for_tray);
                });
                info!("Transcription flow complete, app should remain running");
            }
            Err(e) => {
                error!("Transcription failed: {e}");
                let mut c = ctrl.lock().unwrap();
                c.log_dictation_performance(serde_json::json!({
                    "outcome": "error",
                    "model": model_name,
                    "language": language_name,
                    "modelWasWarm": model_was_warm,
                    "beamSize": beam_size,
                    "temperatureFallback": temperature_fallback,
                    "vadEnabled": vad_enabled,
                    "keyUpToCaptureStoppedMs": key_up_to_capture_stopped_ms,
                    "modelLoadMs": model_load_ms,
                    "keyUpToModelReadyMs": key_up_to_model_ready_ms,
                    "whisperMs": whisper_ms,
                    "keyUpToWhisperCompleteMs": key_up_to_whisper_complete_ms,
                    "keyUpToPasteCompletedMs": null,
                    "totalMs": elapsed_ms(key_up_at),
                }));
                c.on_transcription_error(&e.to_string());
                drop(c);
                let _ = app_handle.emit(events::event::ERROR, e.to_string());
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                dispatch_to_main(&app_handle, |app| {
                    overlay::hide(app);
                    update_tray_status(app, "idle");
                });
                info!("Error flow complete, app should remain running");
            }
        }
    });
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

/// Watch settings and personal dictionaries for external changes and hot-reload
/// them into the running app. Handles hotkey re-registration and emits a
/// settings-changed event to the frontend.
fn start_settings_watcher(app: tauri::AppHandle) {
    use notify::{Config, RecursiveMode, Watcher};
    #[cfg(not(target_os = "macos"))]
    use notify::RecommendedWatcher;
    #[cfg(target_os = "macos")]
    use notify::PollWatcher;
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

        for event in rx {
            let event = match event {
                Ok(e) => e,
                Err(e) => {
                    error!("File watcher error: {e}");
                    continue;
                }
            };

            if !configuration_event_may_affect(
                &event,
                &settings_path,
                &global_glossary_path,
                &profile_glossary_dir,
            ) {
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
                        hotkey::OperationalHotkey::Registered(shortcuts) => hotkey::unregister_shortcuts(
                            &app,
                            shortcuts,
                        )
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
                    match hotkey::register_shortcuts(&app, &new_shortcuts) {
                        Ok(()) => health.record(&new_settings.hotkey, None, hotkey::OperationalHotkey::registered_many(&new_shortcuts)),
                        Err(error) => {
                            new_settings.hotkey = old_settings.hotkey.clone();
                            new_settings.language = old_settings.language;
                            new_settings.hotkey_profiles = old_settings.hotkey_profiles.clone();
                            match hotkey::unregister_shortcuts(&app, &new_shortcuts) {
                                Err(cleanup_error) => health.record(
                                    &old_settings.hotkey,
                                    Some(format!("failed to register new hotkey profiles: {error}; partial-registration cleanup failed: {cleanup_error}")),
                                    hotkey::OperationalHotkey::Unknown,
                                ),
                                Ok(()) => {
                                    let restored = match &old_operational {
                                        hotkey::OperationalHotkey::Registered(shortcuts) => hotkey::register_shortcuts(&app, shortcuts).is_ok(),
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
            let profiles = new_settings.resolved_hotkey_profiles();
            {
                let mut c = ctrl.lock().unwrap();
                c.update_settings(new_settings);
            }
            update_profiles_menu(&app, &profiles);

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
    fn elapsed_ms_if_success_only_marks_successful_operations_complete() {
        let start = Instant::now();
        assert!(elapsed_ms_if_success::<(), ()>(&Ok(()), start).is_some());
        assert!(elapsed_ms_if_success::<(), ()>(&Err(()), start).is_none());
    }

    #[test]
    fn second_instance_reveals_settings_except_for_background_startup() {
        assert!(second_instance_requests_settings(&[
            "sagascript".to_string()
        ]));
        assert!(second_instance_requests_settings(&[
            "sagascript".to_string(),
            sagascript_cli::open::GUI_OPEN_ARG.to_string()
        ]));
        assert!(second_instance_requests_settings(&[
            sagascript_cli::open::GUI_OPEN_ARG.to_string()
        ]));
        assert!(!second_instance_requests_settings(&[
            "sagascript".to_string(),
            sagascript_cli::open::GUI_BACKGROUND_ARG.to_string()
        ]));
        assert!(!second_instance_requests_settings(&[
            sagascript_cli::open::GUI_BACKGROUND_ARG.to_string(),
            "future-metadata".to_string()
        ]));
    }

    #[test]
    fn update_status_describes_available_and_current_releases() {
        assert_eq!(
            update_status_text(&updates::UpdateCheck::Available {
                version: semver::Version::new(1, 2, 3)
            }),
            "Update available — v1.2.3"
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
        fn activate_app(&self) {
            self.operations.borrow_mut().push("activate_app");
        }

        fn reset_to_normal_level(&self) -> Result<(), String> {
            self.operations.borrow_mut().push("reset_to_normal_level");
            if self.fail_at == Some("reset_to_normal_level") {
                Err("window level failed".to_string())
            } else {
                Ok(())
            }
        }

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
            [
                "reset_to_normal_level",
                "unminimize",
                "show",
                "set_focus",
                "activate_app"
            ]
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
        assert_eq!(
            *window.operations.borrow(),
            ["reset_to_normal_level", "unminimize", "show"]
        );
    }

    #[test]
    fn main_window_reveal_continues_if_normal_window_level_cannot_be_restored() {
        let window = MockMainWindow {
            fail_at: Some("reset_to_normal_level"),
            ..Default::default()
        };

        reveal_existing_main_window(&window).unwrap();

        assert_eq!(
            *window.operations.borrow(),
            [
                "reset_to_normal_level",
                "unminimize",
                "show",
                "set_focus",
                "activate_app"
            ]
        );
    }

    #[test]
    fn completed_onboarding_normal_launch_opens_settings() {
        assert_eq!(
            initial_window_request(true, GuiLaunchMode::Standard),
            InitialWindowRequest::Settings
        );
    }

    #[test]
    fn dictation_state_never_turns_a_reopen_event_into_a_settings_window() {
        assert!(should_reveal_for_reopen(AppState::Idle));
        assert!(!should_reveal_for_reopen(AppState::Recording));
        assert!(!should_reveal_for_reopen(AppState::Transcribing));
    }

    #[test]
    fn explicit_open_starts_on_the_settings_view() {
        assert_eq!(
            initial_window_request(true, GuiLaunchMode::ShowSettings),
            InitialWindowRequest::Settings
        );
    }

    #[test]
    fn completed_onboarding_background_launch_stays_hidden() {
        assert_eq!(
            initial_window_request(true, GuiLaunchMode::Background),
            InitialWindowRequest::Hidden
        );
    }

    #[test]
    fn incomplete_onboarding_starts_on_the_onboarding_view_even_when_headless() {
        assert_eq!(
            initial_window_request(false, GuiLaunchMode::Background),
            InitialWindowRequest::Onboarding
        );
    }

    #[test]
    fn private_gui_markers_are_accepted_only_as_the_sole_argument() {
        use std::ffi::OsString;

        let mode = |args: &[&str]| {
            gui_launch_mode(args.iter().map(OsString::from).collect::<Vec<_>>())
        };

        assert_eq!(mode(&["sagascript"]), GuiLaunchMode::Standard);
        assert_eq!(
            mode(&["sagascript", sagascript_cli::open::GUI_OPEN_ARG]),
            GuiLaunchMode::ShowSettings
        );
        assert_eq!(
            mode(&["sagascript", sagascript_cli::open::GUI_BACKGROUND_ARG]),
            GuiLaunchMode::Background
        );
        assert_eq!(
            mode(&["sagascript", "config", sagascript_cli::open::GUI_OPEN_ARG]),
            GuiLaunchMode::Standard
        );
    }

    #[cfg(target_os = "macos")]
    fn wait_for_settings_event(
        rx: &std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
        settings_path: &std::path::Path,
    ) -> bool {
        let global_glossary_path = settings_path.parent().unwrap().join("glossary.txt");
        let profile_glossary_dir = settings_path.parent().unwrap().join("glossaries");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(Ok(event))
                    if configuration_event_may_affect(
                        &event,
                        settings_path,
                        &global_glossary_path,
                        &profile_glossary_dir,
                    ) =>
                {
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
    fn configuration_event_filter_accepts_settings_and_glossary_changes() {
        use notify::event::{CreateKind, ModifyKind};
        use notify::{Event, EventKind};

        let watch_dir = std::path::Path::new("/tmp/sagascript");
        let settings_path = watch_dir.join("sagascript-settings.json");
        let global_glossary_path = watch_dir.join("glossary.txt");
        let profile_glossary_dir = watch_dir.join("glossaries");

        for kind in [
            EventKind::Create(CreateKind::Any),
            EventKind::Modify(ModifyKind::Any),
        ] {
            for target in [
                settings_path.clone(),
                global_glossary_path.clone(),
                profile_glossary_dir.join("swedish.txt"),
            ] {
                let target_event = Event::new(kind).add_path(target);
                assert!(configuration_event_may_affect(
                    &target_event,
                    &settings_path,
                    &global_glossary_path,
                    &profile_glossary_dir,
                ));
            }
        }
    }

    #[test]
    fn configuration_event_filter_rejects_unrelated_and_non_mutating_events() {
        use notify::event::{AccessKind, ModifyKind, RemoveKind};
        use notify::{Event, EventKind};

        let watch_dir = std::path::Path::new("/tmp/sagascript");
        let settings_path = watch_dir.join("sagascript-settings.json");
        let global_glossary_path = watch_dir.join("glossary.txt");
        let profile_glossary_dir = watch_dir.join("glossaries");
        let unrelated = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(watch_dir.join("settings.tmp"));
        assert!(!configuration_event_may_affect(
            &unrelated,
            &settings_path,
            &global_glossary_path,
            &profile_glossary_dir,
        ));

        let unrelated_profile = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(profile_glossary_dir.join("notes.md"));
        assert!(!configuration_event_may_affect(
            &unrelated_profile,
            &settings_path,
            &global_glossary_path,
            &profile_glossary_dir,
        ));

        let remove = Event::new(EventKind::Remove(RemoveKind::Any))
            .add_path(settings_path.clone());
        assert!(!configuration_event_may_affect(
            &remove,
            &settings_path,
            &global_glossary_path,
            &profile_glossary_dir,
        ));

        let remove_glossary = Event::new(EventKind::Remove(RemoveKind::Any))
            .add_path(profile_glossary_dir.join("swedish.txt"));
        assert!(configuration_event_may_affect(
            &remove_glossary,
            &settings_path,
            &global_glossary_path,
            &profile_glossary_dir,
        ));

        let access = Event::new(EventKind::Access(AccessKind::Any))
            .add_path(settings_path.clone());
        assert!(!configuration_event_may_affect(
            &access,
            &settings_path,
            &global_glossary_path,
            &profile_glossary_dir,
        ));

        let other = Event::new(EventKind::Other).add_path(settings_path.clone());
        assert!(!configuration_event_may_affect(
            &other,
            &settings_path,
            &global_glossary_path,
            &profile_glossary_dir,
        ));
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
    fn update_menu_makes_available_release_actionable() {
        let result = updates::UpdateCheck::Available {
            version: semver::Version::new(1, 2, 0),
        };

        assert_eq!(update_status_text(&result), "Update available — v1.2.0");
        assert_eq!(update_action_text(&result), "Download Sagascript v1.2.0…");
    }

    #[test]
    fn update_menu_keeps_a_clear_recheck_action_when_current() {
        assert_eq!(
            update_action_text(&updates::UpdateCheck::UpToDate),
            "Check Again…"
        );
    }

    #[test]
    fn update_action_targets_the_exact_stable_release() {
        assert_eq!(
            stable_release_url(&semver::Version::new(1, 2, 0)),
            "https://github.com/Magnus-Gille/sagascript/releases/tag/v1.2.0"
        );
    }

    #[test]
    fn build_identity_identifies_the_exact_app_build() {
        assert!(BUILD_IDENTITY.contains(env!("CARGO_PKG_VERSION")));
        assert!(BUILD_IDENTITY.contains(env!("GIT_HASH")));
        assert!(BUILD_IDENTITY.contains(env!("BUILD_DATE")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn tray_autosave_name_is_stable_and_bundle_qualified() {
        assert_eq!(TRAY_AUTOSAVE_NAME, "ai.gille.sagascript.main");
        assert_eq!(
            tray_preferred_position_key(TRAY_AUTOSAVE_NAME),
            "NSStatusItem Preferred Position ai.gille.sagascript.main"
        );
        assert_eq!(DEFAULT_TRAY_PREFERRED_POSITION, 340.0);
        assert_eq!(initial_tray_preferred_position(false), Some(340.0));
        assert_eq!(initial_tray_preferred_position(true), None);
    }

    #[test]
    fn tray_label_idle_not_failed() {
        assert_eq!(tray_label("idle", false), ("Sagascript", "S", "Idle"));
    }

    #[test]
    fn tray_label_recording_not_failed() {
        assert_eq!(
            tray_label("recording", false),
            ("Sagascript - Recording...", "●", "Recording...")
        );
    }

    #[test]
    fn tray_status_uses_compact_native_state_markers() {
        assert_eq!(tray_label("idle", false).1, "S");
        assert_eq!(tray_label("recording", false).1, "●");
        assert_eq!(tray_label("loading_model", false).1, "…");
        assert_eq!(tray_label("transcribing", false).1, "…");
        assert_eq!(tray_label("idle", true).1, "!");
    }

    #[test]
    fn profile_menu_label_explains_language_shortcut_and_selection() {
        let profile = sagascript_core::settings::HotkeyProfile {
            id: "swedish".to_string(),
            name: "Svenska".to_string(),
            shortcut: "Super+Shift+S".to_string(),
            language: Language::Swedish,
        };

        assert_eq!(
            profile_menu_label(&profile, true),
            "✓ Svenska — Swedish · ⇧⌘S"
        );
        assert_eq!(
            profile_menu_label(&profile, false),
            "Svenska — Swedish · ⇧⌘S"
        );
    }

    #[test]
    fn profile_menu_label_keeps_non_macos_shortcuts_readable() {
        let profile = sagascript_core::settings::HotkeyProfile {
            id: "english".to_string(),
            name: "English".to_string(),
            shortcut: "Control+Alt+Space".to_string(),
            language: Language::English,
        };

        assert_eq!(
            profile_menu_label(&profile, false),
            "English — English · ⌃⌥Space"
        );
    }

    #[test]
    fn profile_menu_shortcut_formats_command_or_control_aliases() {
        let expected = if cfg!(target_os = "macos") {
            "⇧⌘Space"
        } else {
            "⌃⇧Space"
        };
        for shortcut in [
            "CommandOrControl+Shift+Space",
            "CommandOrCtrl+Shift+Space",
            "CmdOrCtrl+Shift+Space",
            "CmdOrControl+Shift+Space",
        ] {
            assert_eq!(format_menu_shortcut(shortcut), expected);
        }
    }

    #[test]
    fn profile_menu_shortcut_formats_arrow_keys() {
        assert_eq!(format_menu_shortcut("Control+ArrowUp"), "⌃↑");
        assert_eq!(format_menu_shortcut("Alt+Left"), "⌥←");
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
    fn overridden_settings_path_disables_flowdictate_migration() {
        let dir = migrate_test_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let legacy = dir.join("flowdictate-settings.json");
        let new_path = dir.join("sagascript-settings.json");
        std::fs::write(&legacy, r#"{"language":"sv"}"#).unwrap();

        migrate_legacy_settings_unless_overridden(&legacy, &new_path, true);

        assert!(legacy.exists(), "override mode must not inspect or move legacy settings");
        assert!(!new_path.exists());
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
