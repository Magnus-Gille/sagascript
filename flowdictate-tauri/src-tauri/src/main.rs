// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

mod app_controller;
mod audio;
mod cli;
mod commands;
mod error;
mod events;
mod hotkey;
mod logging;
mod overlay;
mod paste;
mod platform;
mod settings;
mod transcription;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use app_controller::AppController;
use commands::{SharedController, SharedWhisper};
use settings::Settings;
use transcription::WhisperBackend;

/// Minimum recording duration before we allow stop (300ms)
const MIN_RECORDING_MS: u64 = 300;

/// Shared tray status menu item for updating from anywhere
type SharedStatusItem = Mutex<Option<MenuItem<tauri::Wry>>>;

fn main() {
    // CLI mode: if a subcommand is given, run CLI and exit
    if let Some(parsed) = cli::try_parse() {
        // CLI mode uses warn-level logging to keep stdout clean
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
            )
            .with_writer(std::io::stderr)
            .init();
        cli::run(parsed);
        return;
    }

    // GUI mode: initialize tracing (console logging)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Sagascript starting...");

    let settings = Settings::default();
    let controller = Mutex::new(AppController::new(settings));
    let whisper: SharedWhisper = Arc::new(WhisperBackend::new());

    tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    let ctrl: tauri::State<'_, SharedController> = app.state();

                    match event.state {
                        ShortcutState::Pressed => {
                            info!("Hotkey pressed: {shortcut}");
                            let (is_recording, show_overlay) = {
                                let mut c = ctrl.lock().unwrap();
                                if let Err(e) = c.handle_hotkey_down() {
                                    error!("Hotkey down error: {e}");
                                }
                                (c.state().is_recording(), c.settings().show_overlay)
                            };
                            if is_recording {
                                let _ = app.emit(events::event::STATE_CHANGED, "recording");
                                update_tray_status(app, "recording");
                                if show_overlay {
                                    overlay::show(app);
                                }
                            }
                        }
                        ShortcutState::Released => {
                            info!("Hotkey released: {shortcut}");
                            handle_hotkey_release(app, &ctrl);
                        }
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(controller)
        .manage(whisper)
        .manage(Mutex::new(None::<MenuItem<tauri::Wry>>) as SharedStatusItem)
        .setup(|app| {
            // Hide from dock on macOS (tray-only app)
            #[cfg(target_os = "macos")]
            platform::macos::set_activation_policy_accessory();

            // Register global shortcut (Ctrl+Shift+Space)
            let shortcut = "Control+Shift+Space";
            match app.global_shortcut().register(shortcut) {
                Ok(()) => info!("Hotkey registered: {shortcut}"),
                Err(e) => error!("Failed to register hotkey: {e}"),
            }

            // Build tray menu
            let quit = MenuItem::with_id(app, "quit", "Quit Sagascript", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let transcribe_file_item =
                MenuItem::with_id(app, "transcribe_file", "Transcribe File...", true, None::<&str>)?;
            let status =
                MenuItem::with_id(app, "status", "Sagascript - Idle", false, None::<&str>)?;

            // Store status item so we can update it after transcription
            {
                let status_state: tauri::State<'_, SharedStatusItem> = app.state();
                *status_state.lock().unwrap() = Some(status.clone());
            }

            let menu = Menu::with_items(app, &[&status, &settings_item, &transcribe_file_item, &quit])?;

            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

            let _tray = TrayIconBuilder::with_id("main")
                .menu(&menu)
                .tooltip("Sagascript")
                .icon(tray_icon)
                .icon_as_template(true)
                .on_menu_event(move |app, event| match event.id().as_ref() {
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
                    _ => {}
                })
                .build(app)?;

            info!("Tray icon created");

            // Migrate settings store from FlowDictate
            {
                let app_dir = app.path().app_data_dir().ok();
                if let Some(dir) = app_dir {
                    let legacy = dir.join("flowdictate-settings.json");
                    let new_path = dir.join("sagascript-settings.json");
                    if legacy.exists() && !new_path.exists() {
                        info!("Migrating settings store from FlowDictate");
                        let _ = std::fs::rename(&legacy, &new_path);
                    }
                }
            }

            // Auto-open onboarding on first launch
            {
                use tauri_plugin_store::StoreExt;
                let store = app.store("sagascript-settings.json")
                    .map_err(|e| format!("Failed to open store: {e}"))?;
                let completed = store
                    .get("hasCompletedOnboarding")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !completed {
                    info!("First launch detected, opening onboarding");
                    open_settings_window(app.handle(), Some("onboarding"));
                }
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
            commands::get_last_transcription,
            commands::get_last_error,
            commands::is_model_ready,
            commands::get_loaded_model,
            commands::update_settings,
            commands::set_language,
            commands::set_whisper_model,
            commands::set_auto_select_model,
            commands::set_hotkey_mode,
            commands::start_recording,
            commands::stop_and_transcribe,
            commands::cancel_recording,
            commands::is_model_downloaded,
            commands::get_model_info,
            commands::download_model,
            commands::set_auto_paste,
            commands::set_show_overlay,
            commands::get_build_info,
            commands::transcribe_file,
            commands::get_supported_formats,
            commands::check_accessibility_permission,
            commands::request_accessibility_permission,
            commands::check_microphone_permission,
            commands::request_microphone_permission,
            commands::get_platform,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Sagascript")
        .run(|_app_handle, event| match event {
            // Prevent app from exiting when all windows are closed (tray-only app),
            // but allow explicit exit requests (e.g. from tray "Quit" menu)
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
            _ => {}
        });
}

/// Update the tray tooltip, title, and status menu item to reflect current state
fn update_tray_status(app: &tauri::AppHandle, state: &str) {
    let (tooltip, title, menu_text) = match state {
        "recording" => ("Sagascript - Recording...", "Rec", "Recording..."),
        "loading_model" => ("Sagascript - Loading model...", "Loading...", "Loading model..."),
        "transcribing" => ("Sagascript - Transcribing...", "...", "Transcribing..."),
        _ => ("Sagascript", "", "Idle"),
    };

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_tooltip(Some(tooltip));
        let _ = tray.set_title(Some(title));
    }

    set_status_menu_text(app, &format!("Sagascript - {menu_text}"));
}

/// Update the tray status menu item and tooltip to show the last transcription
fn update_tray_last_result(app: &tauri::AppHandle, text: &str) {
    let display = if text.len() > 60 {
        format!("{}...", &text[..57])
    } else {
        text.to_string()
    };

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

/// Open or focus the settings window, optionally navigating to a specific tab
fn open_settings_window(app: &tauri::AppHandle, tab: Option<&str>) {
    info!("Opening settings window (tab: {:?})", tab);

    // Build a URL with optional query parameter
    let url = match tab {
        Some("onboarding") => "index.html?onboarding=true".to_string(),
        Some(t) => format!("index.html?tab={t}"),
        None => "index.html".to_string(),
    };

    if let Some(window) = app.get_webview_window("settings") {
        // If switching tab on existing window, emit an event
        if let Some(t) = tab {
            let _ = window.emit("navigate_tab", t);
        }
        let _ = window.show();
        let _ = window.set_focus();
    } else {
        let _window = tauri::WebviewWindowBuilder::new(
            app,
            "settings",
            tauri::WebviewUrl::App(url.into()),
        )
        .title("Sagascript Settings")
        .inner_size(500.0, 550.0)
        .resizable(false)
        .center()
        .build();
    }
}

/// Handle hotkey release: check minimum duration, stop recording, transcribe
fn handle_hotkey_release(
    app: &tauri::AppHandle,
    ctrl: &tauri::State<'_, SharedController>,
) {
    // Check if we should stop (push-to-talk mode + currently recording)
    let (should_stop, elapsed) = {
        let c = ctrl.lock().unwrap();
        (c.should_stop_on_key_up(), c.recording_elapsed())
    };

    if !should_stop {
        return;
    }

    // Enforce minimum recording duration
    if elapsed < Duration::from_millis(MIN_RECORDING_MS) {
        let remaining = Duration::from_millis(MIN_RECORDING_MS) - elapsed;
        info!(
            "Recording too short ({:.0}ms), waiting {:.0}ms...",
            elapsed.as_millis(),
            remaining.as_millis()
        );
        std::thread::sleep(remaining);
    }

    // Stop recording (single lock acquisition)
    let audio = {
        let mut c = ctrl.lock().unwrap();
        if c.state().is_recording() {
            c.stop_recording()
        } else {
            return;
        }
    };

    // Hide overlay now that recording has stopped
    overlay::hide(app);

    // Update tray to show transcribing state
    let _ = app.emit(events::event::STATE_CHANGED, "transcribing");
    update_tray_status(app, "transcribing");

    if audio.is_empty() {
        let mut c = ctrl.lock().unwrap();
        c.on_transcription_error("No audio captured");
        let _ = app.emit(events::event::STATE_CHANGED, "idle");
        update_tray_status(app, "idle");
        return;
    }

    // Transcribe asynchronously to avoid blocking the hotkey thread
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctrl: tauri::State<'_, SharedController> = app_handle.state();
        let whisper: tauri::State<'_, SharedWhisper> = app_handle.state();

        // Extract what we need for transcription (lock briefly)
        let (language, effective_model) = {
            let c = ctrl.lock().unwrap();
            (c.language(), c.settings().effective_model())
        };

        info!("Transcribing with model: {}", effective_model.display_name());

        // Show model loading status in tray
        if whisper.needs_reload(effective_model) {
            let _ = app_handle.emit(events::event::STATE_CHANGED, "loading_model");
            update_tray_status(&app_handle, "loading_model");
        }

        // Ensure model is loaded
        let result = if let Err(e) = whisper.ensure_model(effective_model) {
            Err(e)
        } else {
            // Run blocking transcription on a separate thread
            let whisper = whisper.inner().clone();
            let audio = audio.clone();
            match tokio::task::spawn_blocking(move || {
                whisper.transcribe_sync(&audio, language)
            })
            .await
            {
                Ok(r) => r,
                Err(e) => Err(error::DictationError::TranscriptionFailed(
                    format!("Task join error: {e}"),
                )),
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

                let _ = app_handle.emit(events::event::TRANSCRIPTION_RESULT, &text);
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                update_tray_status(&app_handle, "idle");
                update_tray_last_result(&app_handle, &text);
                info!("Transcription flow complete, app should remain running");
            }
            Err(e) => {
                error!("Transcription failed: {e}");
                let mut c = ctrl.lock().unwrap();
                c.on_transcription_error(&e.to_string());
                let _ = app_handle.emit(events::event::ERROR, e.to_string());
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
                update_tray_status(&app_handle, "idle");
                info!("Error flow complete, app should remain running");
            }
        }
    });
}
