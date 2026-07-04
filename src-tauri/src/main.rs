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

use tracing_subscriber::EnvFilter;

use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager,
};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tracing::{error, info, warn};

use app_controller::{AppController, HotkeyDownResult, StopRecordingOutcome};
use commands::{SharedController, SharedWhisper};
use sagascript_core::transcription::WhisperBackend;

/// Minimum recording duration before we allow stop (300ms)
const MIN_RECORDING_MS: u64 = 300;

/// Shared tray status menu item for updating from anywhere
type SharedStatusItem = Mutex<Option<MenuItem<tauri::Wry>>>;

fn main() {
    // CLI mode: if a subcommand is given, run CLI and exit. The desktop
    // binary is a full CLI (CLI-first design) — the GUI only launches on a
    // bare invocation.
    if let Some(parsed) = sagascript_cli::try_parse() {
        // CLI mode uses warn-level logging to keep stdout clean
        tracing_subscriber::fmt()
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
            )
            .with_writer(std::io::stderr)
            .init();
        sagascript_cli::run(parsed);
        return;
    }

    // GUI mode: initialize tracing (console logging)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Sagascript starting...");

    let settings = sagascript_core::settings::store::load();
    info!("Loaded settings: language={:?}, model={:?}, hotkey={}", settings.language, settings.whisper_model, settings.hotkey);
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
                            let result = {
                                let mut c = ctrl.lock().unwrap();
                                match c.handle_hotkey_down() {
                                    Ok(r) => r,
                                    Err(e) => {
                                        error!("Hotkey down error: {e}");
                                        HotkeyDownResult::NoOp
                                    }
                                }
                            };
                            match result {
                                HotkeyDownResult::StartedRecording => {
                                    let show_overlay = {
                                        let c = ctrl.lock().unwrap();
                                        c.settings().show_overlay
                                    };
                                    let _ = app.emit(events::event::STATE_CHANGED, "recording");
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

            // Read hotkey from already-loaded settings and register it
            let shortcut = {
                let ctrl: tauri::State<'_, SharedController> = app.state();
                let c = ctrl.lock().unwrap();
                let shortcut = c.settings().hotkey.clone();
                drop(c);
                shortcut
            };

            // Register global shortcut
            match app.global_shortcut().register(shortcut.as_str()) {
                Ok(()) => info!("Hotkey registered: {shortcut}"),
                Err(e) => error!("Failed to register hotkey: {e}"),
            }

            // Build tray menu
            let quit = MenuItem::with_id(app, "quit", "Quit Sagascript", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Open Sagascript...", true, None::<&str>)?;
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
                    migrate_legacy_settings(&legacy, &new_path);
                }
            }

            // Watch settings file for external changes (e.g. `sagascript config set`)
            start_settings_watcher(app.handle().clone());

            // Auto-open onboarding on first launch
            {
                let settings = sagascript_core::settings::store::load();
                if !settings.has_completed_onboarding {
                    info!("First launch detected, opening onboarding");
                    open_settings_window(app.handle(), Some("onboarding"));
                }
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

                // If VAD is enabled (e.g. set via the CLI), make sure the Silero
                // model is present for the next dictation.
                if vad_enabled && !sagascript_core::transcription::model::is_vad_model_downloaded() {
                    tauri::async_runtime::spawn(async {
                        if let Err(e) =
                            sagascript_core::transcription::model::download_vad_model(|_, _| {}).await
                        {
                            warn!("VAD model preload failed: {e}");
                        }
                    });
                }

                // Best-effort: backfill the CoreML encoder for the effective
                // model — covers models downloaded before CoreML support, so the
                // encoder moves to the Neural Engine without a manual re-download.
                // Only when the GGML model itself is present (don't fetch an
                // encoder for a model the user hasn't downloaded yet).
                if sagascript_core::transcription::model::is_model_downloaded(model) {
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) =
                            sagascript_core::transcription::model::backfill_coreml_encoder(model).await
                        {
                            warn!("CoreML encoder backfill skipped: {e}");
                        }
                    });
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
            commands::set_hotkey,
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
            // Prevent app from exiting when all windows are closed (tray-only app),
            // but allow explicit exit requests (e.g. from tray "Quit" menu)
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
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

/// Update the tray status menu item and tooltip to show the last transcription
fn update_tray_last_result(app: &tauri::AppHandle, text: &str) {
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

/// Open or focus the main window, optionally navigating to a specific tab
fn open_settings_window(app: &tauri::AppHandle, tab: Option<&str>) {
    info!("Opening main window (tab: {:?})", tab);

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
        // Cap default height to 80% of screen so it fits on small displays (e.g. 768p laptops)
        let default_height = if let Ok(Some(monitor)) = app.primary_monitor() {
            let logical_h = monitor.size().height as f64 / monitor.scale_factor();
            (logical_h * 0.8).min(660.0)
        } else {
            660.0
        };

        let _window = tauri::WebviewWindowBuilder::new(
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
        .build();
    }
}

/// Handle hotkey release: stop recording for push-to-talk mode
fn handle_hotkey_release(
    app: &tauri::AppHandle,
    ctrl: &tauri::State<'_, SharedController>,
) {
    let should_stop = {
        let c = ctrl.lock().unwrap();
        c.should_stop_on_key_up()
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
                c.settings().effective_model(),
                commands::build_transcribe_options(c.settings()),
            )
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
            // Run blocking transcription on a separate thread with a timeout.
            // NOTE: The abort callback is currently disabled (whisper-rs error -6).
            // On timeout, request_abort() is called but has no effect — the blocking
            // task continues running and holds the context mutex until whisper finishes.
            let whisper_ref = whisper.inner().clone();
            let fut = tokio::task::spawn_blocking(move || {
                whisper_ref.transcribe_sync_with_options(&audio, language, &opts, |_| {})
            });

            let timeout = Duration::from_secs(TRANSCRIPTION_TIMEOUT_SECS);
            match tokio::time::timeout(timeout, fut).await {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => Err(sagascript_core::error::DictationError::TranscriptionFailed(
                    format!("Task join error: {e}"),
                )),
                Err(_) => {
                    // Timeout — request abort (currently a no-op, see whisper_backend.rs)
                    whisper.request_abort();
                    Err(sagascript_core::error::DictationError::TranscriptionFailed(
                        format!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s"),
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

/// Watch the settings file for external changes and hot-reload into the running app.
/// Handles hotkey re-registration and emits a settings-changed event to the frontend.
fn start_settings_watcher(app: tauri::AppHandle) {
    use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use std::sync::mpsc;

    let settings_path = sagascript_core::settings::store::settings_path();
    let watch_dir = match settings_path.parent() {
        Some(d) => d.to_path_buf(),
        None => {
            error!("Cannot determine settings directory for file watcher");
            return;
        }
    };
    let settings_filename = settings_path
        .file_name()
        .map(|f| f.to_os_string())
        .unwrap_or_default();

    std::thread::spawn(move || {
        let (tx, rx) = mpsc::channel();

        let mut watcher = match RecommendedWatcher::new(tx, Config::default()) {
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

            // Only react to modify/create events on our settings file
            let dominated = matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_)
            );
            if !dominated {
                continue;
            }

            let is_our_file = event.paths.iter().any(|p| {
                p.file_name()
                    .map(|f| f == settings_filename)
                    .unwrap_or(false)
            });
            if !is_our_file {
                continue;
            }

            // Small delay to let atomic rename complete
            std::thread::sleep(Duration::from_millis(50));

            let new_settings = sagascript_core::settings::store::load();

            let ctrl: tauri::State<'_, SharedController> = app.state();
            let old_settings = {
                let c = ctrl.lock().unwrap();
                c.settings().clone()
            };

            // Hot-reload hotkey if changed
            if new_settings.hotkey != old_settings.hotkey {
                info!(
                    "Settings watcher: hotkey changed '{}' -> '{}'",
                    old_settings.hotkey, new_settings.hotkey
                );

                if let Err(e) = app.global_shortcut().unregister(old_settings.hotkey.as_str()) {
                    error!("Failed to unregister old hotkey: {e}");
                }

                match app.global_shortcut().register(new_settings.hotkey.as_str()) {
                    Ok(()) => info!("Hotkey re-registered: {}", new_settings.hotkey),
                    Err(e) => {
                        error!("Failed to register new hotkey '{}': {e}", new_settings.hotkey);
                        // Re-register the old one as fallback
                        let _ = app.global_shortcut().register(old_settings.hotkey.as_str());
                    }
                }
            }

            // Update controller with all new settings
            {
                let mut c = ctrl.lock().unwrap();
                c.update_settings(new_settings);
            }

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
