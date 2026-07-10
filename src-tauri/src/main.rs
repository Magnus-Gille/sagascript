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

/// Grace after a timeout-triggered abort for the blocking inference to unwind and
/// release the warm-state lock before we log it as still stuck.
const ABORT_GRACE_SECS: u64 = 5;

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

#[cfg(any(target_os = "macos", test))]
fn auto_paste_permitted(requested: bool, accessibility_trusted: bool) -> bool {
    !requested || accessibility_trusted
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
        .plugin(tauri_plugin_dialog::init())
        .manage(controller)
        .manage(whisper)
        .manage(hotkey_health)
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

            // Register global shortcut. Failure here (combo already claimed by
            // Spotlight/Raycast/etc.) used to be log-only: the app would look
            // fine (tray shows "Idle") while being completely unable to
            // dictate. Recorded in the process-wide health flag so the tray
            // and Settings UI can surface it.
            let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
            match app.global_shortcut().register(shortcut.as_str()) {
                Ok(()) => {
                    info!("Hotkey registered: {shortcut}");
                    let change = health.record(&shortcut, None);
                    if change.changed {
                        let _ = app.emit(events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
                    }
                }
                Err(e) => {
                    error!("Failed to register hotkey: {e}");
                    let change = health.record(&shortcut, Some(e.to_string()));
                    if change.changed {
                        let _ = app.emit(events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
                    }
                }
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
            commands::set_language,
            commands::set_whisper_model,
            commands::set_auto_select_model,
            commands::set_hotkey_mode,
            commands::set_hotkey,
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
            // Prevent app from exiting when all windows are closed (tray-only app),
            // but allow explicit exit requests (e.g. from tray "Quit" menu)
            if let tauri::RunEvent::ExitRequested { api, code, .. } = event {
                if code.is_none() {
                    api.prevent_exit();
                }
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

            let new_settings = load_settings_with_permission_gate();

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

                let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
                let change = match app.global_shortcut().register(new_settings.hotkey.as_str()) {
                    Ok(()) => {
                        info!("Hotkey re-registered: {}", new_settings.hotkey);
                        health.record(&new_settings.hotkey, None)
                    }
                    Err(e) => {
                        error!("Failed to register new hotkey '{}': {e}", new_settings.hotkey);
                        // Re-register the old one as fallback so the app doesn't
                        // end up with no hotkey bound at all. Either way the
                        // SAVED shortcut is not registered — health must report
                        // the saved shortcut's failure, not a false-normal for
                        // the operational fallback.
                        match app.global_shortcut().register(old_settings.hotkey.as_str()) {
                            Ok(()) => {
                                info!("Re-registered old hotkey '{}' as fallback", old_settings.hotkey);
                                health.record(
                                    &new_settings.hotkey,
                                    Some(format!(
                                        "failed to register: {e}; still using previous hotkey '{}'",
                                        old_settings.hotkey
                                    )),
                                )
                            }
                            Err(e2) => {
                                error!("Failed to re-register old hotkey: {e2}");
                                health.record(
                                    &new_settings.hotkey,
                                    Some(format!(
                                        "failed to register: {e}; fallback to '{}' also failed: {e2}",
                                        old_settings.hotkey
                                    )),
                                )
                            }
                        }
                    }
                };
                if change.changed {
                    let _ = app.emit(events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
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
    fn auto_paste_requires_runtime_accessibility_approval() {
        assert!(auto_paste_permitted(false, false));
        assert!(auto_paste_permitted(false, true));
        assert!(!auto_paste_permitted(true, false));
        assert!(auto_paste_permitted(true, true));
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
