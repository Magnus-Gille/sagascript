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

use std::sync::Mutex;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

use app_controller::AppController;
use settings::Settings;

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

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(controller)
        .setup(|app| {
            // Hide from dock on macOS (tray-only app)
            #[cfg(target_os = "macos")]
            platform::macos::set_activation_policy_accessory();

            // Build tray menu
            let quit = MenuItem::with_id(app, "quit", "Quit FlowDictate", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings...", true, None::<&str>)?;
            let status =
                MenuItem::with_id(app, "status", "FlowDictate - Idle", false, None::<&str>)?;

            let menu = Menu::with_items(app, &[&status, &settings_item, &quit])?;

            let _tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("FlowDictate")
                .icon(app.default_window_icon().cloned().unwrap())
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running FlowDictate");
}
