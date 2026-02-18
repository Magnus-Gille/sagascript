// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_controller;
mod audio;
mod commands;
mod credentials;
mod error;
mod events;
mod hotkey;
mod logging;
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

fn main() {
    // Initialize tracing (console logging)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("FlowDictate starting...");

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
                            let mut c = ctrl.lock().unwrap();
                            if let Err(e) = c.handle_hotkey_down() {
                                error!("Hotkey down error: {e}");
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
        .manage(controller)
        .manage(whisper)
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
            let quit = MenuItem::with_id(app, "quit", "Quit FlowDictate", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let status =
                MenuItem::with_id(app, "status", "FlowDictate - Idle", false, None::<&str>)?;

            let menu = Menu::with_items(app, &[&status, &settings_item, &quit])?;

            let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("FlowDictate")
                .icon(tray_icon)
                .icon_as_template(true)
                .on_menu_event(move |app, event| match event.id().as_ref() {
                    "quit" => {
                        info!("Quit requested");
                        app.exit(0);
                    }
                    "settings" => {
                        info!("Opening settings window");
                        if let Some(window) = app.get_webview_window("settings") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        } else {
                            let _window = tauri::WebviewWindowBuilder::new(
                                app,
                                "settings",
                                tauri::WebviewUrl::App("index.html".into()),
                            )
                            .title("FlowDictate Settings")
                            .inner_size(500.0, 450.0)
                            .resizable(false)
                            .center()
                            .build();
                        }
                    }
                    _ => {}
                })
                .build(app)?;

            info!("Tray icon created");
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_settings,
            commands::get_last_transcription,
            commands::get_last_error,
            commands::is_model_ready,
            commands::update_settings,
            commands::set_language,
            commands::set_backend,
            commands::set_whisper_model,
            commands::set_auto_select_model,
            commands::set_hotkey_mode,
            commands::save_api_key,
            commands::has_api_key,
            commands::delete_api_key,
            commands::start_recording,
            commands::stop_and_transcribe,
            commands::cancel_recording,
            commands::is_model_downloaded,
            commands::get_model_info,
            commands::download_model,
            commands::set_auto_paste,
            commands::set_show_overlay,
            commands::get_build_info,
        ])
        .run(tauri::generate_context!())
        .expect("error while running FlowDictate");
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

    if audio.is_empty() {
        let mut c = ctrl.lock().unwrap();
        c.on_transcription_error("No audio captured");
        return;
    }

    // Transcribe asynchronously to avoid blocking the hotkey thread
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let ctrl: tauri::State<'_, SharedController> = app_handle.state();
        let whisper: tauri::State<'_, SharedWhisper> = app_handle.state();

        // Extract what we need for transcription (lock briefly)
        let (backend, language, keyring, effective_model) = {
            let c = ctrl.lock().unwrap();
            (
                c.backend(),
                c.language(),
                c.keyring().clone(),
                c.settings().effective_model(),
            )
        };
        // Lock dropped

        let result = match backend {
            settings::TranscriptionBackendType::Remote => {
                use transcription::backend::TranscriptionBackend;
                let openai = transcription::OpenAIBackend::new(keyring);
                openai.transcribe(&audio, language).await
            }
            settings::TranscriptionBackendType::Local => {
                // Ensure model is loaded
                if let Err(e) = whisper.ensure_model(effective_model) {
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
                }
            }
        };

        match result {
            Ok(text) => {
                info!("Transcription complete: {} chars", text.len());
                let mut c = ctrl.lock().unwrap();
                if let Err(e) = c.auto_paste(&text) {
                    error!("Auto-paste failed: {e}");
                }
                c.on_transcription_success(&text);

                let _ = app_handle.emit(events::event::TRANSCRIPTION_RESULT, &text);
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
            }
            Err(e) => {
                error!("Transcription failed: {e}");
                let mut c = ctrl.lock().unwrap();
                c.on_transcription_error(&e.to_string());
                let _ = app_handle.emit(events::event::ERROR, e.to_string());
                let _ = app_handle.emit(events::event::STATE_CHANGED, "idle");
            }
        }
    });
}
