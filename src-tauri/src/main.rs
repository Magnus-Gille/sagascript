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
mod command_error;
mod commands;
#[cfg(test)]
mod contract;
mod events;
mod focus_diag;
mod hotkey;
mod launch;
mod meeting_jobs;
mod meeting_media;
mod meeting_media_range;
mod meeting_reprocessing_commands;
mod meeting_review_commands;
mod overlay;
mod paste;
#[path = "paste/completion.rs"]
mod paste_completion;
mod plain_file_jobs;
mod platform;
mod settings_watcher;
mod tray;
mod updater;

use tracing_subscriber::EnvFilter;

use std::time::{Duration, Instant};

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// The text the hotkey path reports for a failed dictation, in the `error`
/// event and the controller's last error (main_tests.rs pins it; ADR 0006).
fn dictation_failure_text(error: &sagascript_core::error::DictationError) -> String {
    error.to_string()
}

/// The dictation outran [`TRANSCRIPTION_TIMEOUT_SECS`] and was aborted.
fn dictation_timed_out() -> sagascript_core::error::DictationError {
    sagascript_core::error::DictationError::TranscriptionTimedOut(format!(
        "Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s (inference aborted)"
    ))
}

/// The blocking transcription task ended abnormally (a join error).
fn dictation_task_failed(error: impl std::fmt::Display) -> sagascript_core::error::DictationError {
    sagascript_core::error::DictationError::TaskFailed(format!("Task join error: {error}"))
}

/// Grace after a timeout-triggered abort for the blocking inference to unwind and
/// release the warm-state lock before we log it as still stuck.
const ABORT_GRACE_SECS: u64 = 5;

/// Maximum time to wait for the native paste callback to report its result.
/// macOS paste stays on its mandatory main thread; other platforms use a
/// blocking worker. A lost callback must not leave dictation stuck processing.
const PASTE_COMPLETION_TIMEOUT_MS: u64 = paste_completion::COMPLETION_TIMEOUT_MS;

use events::{EmitEvent, UiState};
use tauri::Manager;
use tauri_plugin_global_shortcut::ShortcutState;
use tracing::{error, info, warn};

#[cfg(windows)]
use launch::{attach_parent_console, cli_args_request_console_output};
use launch::{
    gui_launch_mode, handle_run_event, handle_second_instance, open_settings_window, setup_app,
    GuiLaunchMode,
};
use tray::{select_profile_menu, update_tray_last_result, update_tray_status};
// commands.rs reaches the profile menu refresh as `crate::update_profiles_menu`.
pub(crate) use tray::update_profiles_menu;

#[cfg(target_os = "macos")]
use updater::updates;
// commands.rs and app_controller.rs reach these under their old crate paths.
pub(crate) use updater::{activity as update_activity, recovery as update_recovery};

use app_controller::{HotkeyDownResult, StopRecordingOutcome};
use commands::{lock_controller, SharedController, SharedWhisper};
use sagascript_core::settings::canonical_hotkey;
use sagascript_core::settings::HotkeyMode;
use sagascript_core::transcription::{WARM_MODEL_CACHE_BUDGET_MB, WARM_MODEL_CACHE_MAX_MODELS};

/// Minimum recording duration before we allow stop (300ms)
const MIN_RECORDING_MS: u64 = 300;
const SAFE_FALLBACK_HOTKEY: &str = "Control+Shift+Space";

fn should_use_safe_fallback(
    shortcut: &str,
    settings_valid: bool,
    operational_hotkey: &hotkey::OperationalHotkey,
) -> bool {
    if settings_valid {
        return false;
    }

    let (Ok(canonical_shortcut), Ok(canonical_fallback)) = (
        sagascript_core::settings::canonical_hotkey(shortcut),
        sagascript_core::settings::canonical_hotkey(SAFE_FALLBACK_HOTKEY),
    ) else {
        return false;
    };

    canonical_shortcut == canonical_fallback
        && operational_hotkey.matches(&[SAFE_FALLBACK_HOTKEY.to_string()])
}

/// Key-down pre-warm: start loading the Pianissimo model while the user is
/// still speaking (also after an idle unload under `on_app_start`). Never
/// blocks the hotkey path: the work runs on its own thread and this only reads
/// settings.
fn prewarm_engine_on_key_down(ctrl: &tauri::State<'_, SharedController>) {
    use sagascript_core::transcription::pianissimo_backend::{
        warm_in_background, warm_on, WarmTrigger,
    };
    let c = lock_controller(ctrl);
    let profile_id = c.active_hotkey_profile().map(|profile| profile.id.clone());
    warm_on(
        c.settings(),
        WarmTrigger::KeyDown,
        profile_id.as_deref(),
        warm_in_background,
    );
}

/// Tell the overlay when the engine starts loading, is ready, failed or was
/// cancelled. The last event is cached in core for late-created overlays.
fn forward_engine_load_state(app: &tauri::AppHandle) {
    use sagascript_core::transcription::pianissimo_backend as backend;
    let app = app.clone();
    backend::set_load_listener(move |event, source| {
        let payload = backend::load_state_payload(Some(&(event, source)));
        focus_diag::log(
            "engine_load_state_event",
            serde_json::json!({ "origin": "load_listener" }),
        );
        let _ = app.emit_event(events::event::ENGINE_LOAD_STATE, payload);
    });
}

/// Re-send the current engine load state (the overlay may have just been
/// created or shown and missed earlier events).
fn reemit_engine_load_state(app: &tauri::AppHandle) {
    let payload = sagascript_core::transcription::pianissimo_backend::current_load_state_payload();
    focus_diag::log(
        "engine_load_state_event",
        serde_json::json!({ "origin": "reemit_after_overlay" }),
    );
    let _ = app.emit_event(events::event::ENGINE_LOAD_STATE, payload);
}

/// Warm the engine for `trigger` (wake, app start) when settings call for it.
fn prewarm_engine_for(
    app: &tauri::AppHandle,
    trigger: sagascript_core::transcription::pianissimo_backend::WarmTrigger,
) {
    use sagascript_core::transcription::pianissimo_backend::{warm_in_background, warm_on};
    let ctrl: tauri::State<'_, SharedController> = app.state();
    // Wake callbacks run on the main thread inside an ObjC block: never block
    // on the controller there. Skip this warm when it is busy; a poisoned
    // controller is recovered like everywhere else (ADR 0005), not "busy".
    let c = match ctrl.try_lock() {
        Ok(c) => c,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            sagascript_core::sync::recover_poisoned(&ctrl, commands::CONTROLLER_LOCK, poisoned)
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            tracing::debug!(
                reason = trigger.reason(),
                "pre-warm skipped: controller busy"
            );
            return;
        }
    };
    warm_on(c.settings(), trigger, None, warm_in_background);
}

/// Hide the overlay after its error state has been readable, unless a new
/// dictation started in the meantime.
fn hide_overlay_after_error(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(OVERLAY_ERROR_VISIBLE).await;
        let idle = {
            let ctrl: tauri::State<'_, SharedController> = app.state();
            let idle = matches!(
                lock_controller(&ctrl).state(),
                app_controller::AppState::Idle
            );
            idle
        };
        if idle {
            dispatch_to_main(&app, overlay::hide);
        }
    });
}

/// How long the overlay keeps its error state visible after a failed dictation.
const OVERLAY_ERROR_VISIBLE: Duration = Duration::from_millis(2500);

fn handle_hotkey_event(app: &tauri::AppHandle, shortcut: &str, state: hotkey::BareHotkeyState) {
    let ctrl: tauri::State<'_, SharedController> = app.state();

    match state {
        hotkey::BareHotkeyState::Pressed => {
            info!("Hotkey pressed: {shortcut}");
            #[cfg(target_os = "macos")]
            let pressed_target = platform::macos::frontmost_pid();
            #[cfg(target_os = "macos")]
            let pressed_target_opt = pressed_target;
            #[cfg(not(target_os = "macos"))]
            let pressed_target_opt: Option<i32> = None;
            let health: tauri::State<'_, hotkey::HotkeyHealth> = app.state();
            let safe_fallback = {
                let c = lock_controller(&ctrl);
                should_use_safe_fallback(
                    shortcut,
                    c.settings().validate_shortcut_configuration().is_ok(),
                    &health.operational_hotkey(),
                )
            };
            let (result, active_profile) = {
                let mut c = lock_controller(&ctrl);
                let binding = c.settings().resolved_hotkey_bindings().into_iter().find(
                    |(_, configured, _)| {
                        canonical_hotkey(configured).ok() == canonical_hotkey(shortcut).ok()
                    },
                );
                let result = match binding {
                    Some((profile, configured_shortcut, mode)) => {
                        // A failed operational configuration always uses the
                        // dedicated fallback as PTT, even if the persisted
                        // shortcut happens to be an explicit toggle binding.
                        let (mode, shortcut) = if safe_fallback {
                            (HotkeyMode::PushToTalk, SAFE_FALLBACK_HOTKEY)
                        } else {
                            (mode, configured_shortcut.as_str())
                        };
                        match c.handle_hotkey_down_for_binding(profile, mode, shortcut) {
                            Ok(result) => {
                                if safe_fallback && result == HotkeyDownResult::StartedRecording {
                                    c.use_safe_fallback_lifecycle();
                                }
                                result
                            }
                            Err(error) => {
                                error!("Hotkey down error: {error}");
                                let _ = app.emit_event(events::event::ERROR, &error.to_string());
                                HotkeyDownResult::NoOp
                            }
                        }
                    }
                    None if safe_fallback => {
                        let profile = c.settings().resolved_hotkey_profiles()[0].clone();
                        match c.handle_hotkey_down_for_binding(
                            profile,
                            HotkeyMode::PushToTalk,
                            SAFE_FALLBACK_HOTKEY,
                        ) {
                            Ok(result) => {
                                if result == HotkeyDownResult::StartedRecording {
                                    c.use_safe_fallback_lifecycle();
                                }
                                result
                            }
                            Err(error) => {
                                error!("Hotkey down error: {error}");
                                let _ = app.emit_event(events::event::ERROR, &error.to_string());
                                HotkeyDownResult::NoOp
                            }
                        }
                    }
                    None => {
                        warn!("Ignoring unconfigured shortcut event: {shortcut}");
                        HotkeyDownResult::NoOp
                    }
                };
                (result, c.active_hotkey_profile().cloned())
            };
            match result {
                HotkeyDownResult::StartedRecording => {
                    #[cfg(target_os = "macos")]
                    platform::macos::remember_dictation_target(pressed_target);
                    focus_diag::log(
                        "hotkey_down",
                        serde_json::json!({ "pressedTargetPid": pressed_target_opt }),
                    );
                    prewarm_engine_on_key_down(&ctrl);
                    let show_overlay = {
                        let c = lock_controller(&ctrl);
                        c.settings().show_overlay
                    };
                    let _ = app.emit_event(events::event::STATE_CHANGED, UiState::Recording);
                    if let Some(profile) = active_profile {
                        select_profile_menu(app, &profile);
                        let _ =
                            app.emit_event(events::event::ACTIVE_HOTKEY_PROFILE_CHANGED, &profile);
                    }
                    update_tray_status(app, "recording");
                    if show_overlay {
                        overlay::show(app);
                        reemit_engine_load_state(app);
                        #[cfg(target_os = "macos")]
                        {
                            let restore = platform::macos::restore_dictation_target_if_stolen();
                            let (restored, restore_error) = match &restore {
                                Ok(restored) => (Some(*restored), None),
                                Err(error) => (None, Some(error.clone())),
                            };
                            focus_diag::log(
                                "focus_restore_after_overlay",
                                serde_json::json!({
                                    "restored": restored,
                                    "restoreFailed": restore_error.is_some(),
                                }),
                            );
                            if let Err(error) = restore {
                                warn!("Could not return focus after showing the recording overlay: {error}");
                            }
                        }
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

fn main() -> std::process::ExitCode {
    // Remember panic messages so a lock recovered after a panic can name the
    // cause in its error log (ADR 0005); the default stderr report is kept.
    sagascript_core::sync::install_panic_recorder();
    let startup_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let gui_launch_mode = gui_launch_mode(startup_args.iter().cloned());

    // CLI mode: if a subcommand is given, run CLI and exit. The desktop
    // binary is a full CLI (CLI-first design) — the GUI only launches on a
    // bare invocation. The private GUI-open marker is consumed here rather
    // than passed to clap so `sagascript open` can explicitly reveal Settings.
    if gui_launch_mode == GuiLaunchMode::Standard {
        #[cfg(windows)]
        if cli_args_request_console_output(&startup_args) {
            attach_parent_console();
        }
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
            return sagascript_cli::run(parsed);
        }
    }

    // Build identity announced to the Pianissimo engine host (same values as the tray menu).
    sagascript_core::transcription::pianissimo_backend::set_client_identity(
        sagascript_core::transcription::engine_host::ClientIdentity {
            name: "sagascript".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            git_sha: env!("GIT_HASH").into(),
        },
    );

    // GUI mode: initialize tracing (console logging)
    let configured_filter = std::env::var("RUST_LOG").ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(sagascript_cli::effective_log_filter(
            configured_filter.as_deref(),
            "info",
        )))
        .init();

    info!("Sagascript starting...");

    build_app(gui_launch_mode).run(handle_run_event);
    std::process::ExitCode::SUCCESS
}

/// Build the Tauri application: plugins, the setup hook, window events and the
/// command table. Running it is `main`'s job.
fn build_app(gui_launch_mode: GuiLaunchMode) -> tauri::App {
    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .register_asynchronous_uri_scheme_protocol("meeting-audio", meeting_media::protocol)
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            handle_second_instance(app, args);
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
        .plugin(tauri_plugin_dialog::init());

    #[cfg(target_os = "macos")]
    if let Some(pubkey) = updates::updater_public_key() {
        builder = builder.plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(pubkey.to_owned())
                .build(),
        );
    }

    builder
        .setup(move |app| setup_app(app, gui_launch_mode))
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
            commands::get_engine_load_state,
            commands::get_settings,
            commands::get_active_hotkey_profile,
            commands::get_last_transcription,
            commands::get_last_error,
            commands::is_model_ready,
            commands::get_loaded_model,
            commands::set_language,
            commands::set_whisper_model,
            commands::set_file_transcription_model,
            commands::set_profile_model,
            commands::get_profile_model_info,
            commands::set_pianissimo_dictation,
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
            commands::get_file_model_options,
            commands::get_effective_model_info,
            commands::get_auto_file_model_info,
            commands::get_dictation_model_info,
            commands::download_model,
            commands::download_pianissimo_model,
            commands::set_auto_paste,
            commands::set_show_overlay,
            commands::set_initial_prompt,
            commands::set_profile_glossary,
            commands::suggest_training_glossary,
            commands::apply_training_glossary,
            commands::set_beam_size,
            commands::set_temperature_fallback,
            commands::set_vad_enabled,
            commands::set_engine_prewarm,
            commands::set_engine_idle_unload_minutes,
            commands::engine_status,
            commands::get_build_info,
            commands::transcribe_file,
            commands::cancel_file_transcription,
            commands::set_update_result_pending,
            commands::get_update_result_pending,
            commands::acknowledge_update_result,
            commands::save_update_recovery,
            commands::load_update_recovery,
            commands::clear_update_recovery,
            commands::complete_update_preparation,
            commands::save_transcription_text,
            commands::copy_transcription_text,
            meeting_jobs::begin_meeting_file,
            meeting_jobs::begin_meeting_reprocessing,
            meeting_reprocessing_commands::plan_meeting_reprocessing,
            meeting_reprocessing_commands::preview_meeting_proposal,
            meeting_reprocessing_commands::resolve_meeting_proposal,
            meeting_reprocessing_commands::accept_meeting_proposal,
            meeting_reprocessing_commands::save_meeting_proposal,
            meeting_reprocessing_commands::open_meeting_proposal,
            meeting_jobs::get_meeting_job,
            meeting_jobs::cancel_meeting_job,
            meeting_jobs::rename_meeting_speaker,
            meeting_jobs::merge_meeting_speakers,
            meeting_jobs::save_meeting_export,
            meeting_review_commands::create_meeting_review,
            meeting_review_commands::apply_meeting_corrections,
            meeting_review_commands::undo_meeting_review,
            meeting_review_commands::reset_meeting_review,
            meeting_review_commands::open_meeting_review,
            meeting_review_commands::save_meeting_review,
            meeting_media::attach_meeting_audio,
            meeting_media::detach_meeting_audio,
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
}

/// Preload and warm the models and the engine selected by the settings.
fn warm_models_at_startup(app: &tauri::App) {
    let whisper: tauri::State<'_, SharedWhisper> = app.state();
    let whisper = whisper.inner().clone();
    let (warm_plan, vad_enabled) = {
        let ctrl: tauri::State<'_, SharedController> = app.state();
        let c = lock_controller(&ctrl);
        (
            c.settings()
                .warm_model_plan(WARM_MODEL_CACHE_MAX_MODELS, WARM_MODEL_CACHE_BUDGET_MB),
            c.settings().vad_enabled,
        )
    };
    {
        // `engine_prewarm = on_app_start`: load the Pianissimo engine in
        // the background when a dictation profile uses it.
        let handle = app.handle().clone();
        forward_engine_load_state(&handle);
        // Dictation audio a killed earlier run left in the temp dir.
        sagascript_core::transcription::engine_host::pipeline::sweep_stale_pcm_dirs_at_startup();
        prewarm_engine_for(
            &handle,
            sagascript_core::transcription::pianissimo_backend::WarmTrigger::AppStart,
        );
        #[cfg(target_os = "macos")]
        {
            platform::macos::observe_memory_pressure(|| {
                sagascript_core::transcription::pianissimo_backend::unload_if_idle_for_memory_pressure();
            });
            let wake_app = handle.clone();
            platform::macos::observe_system_wake(move || {
                prewarm_engine_for(
                    &wake_app,
                    sagascript_core::transcription::pianissimo_backend::WarmTrigger::Wake,
                );
            });
        }
    }
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
            if let Err(e) = sagascript_core::transcription::model::verify_vad_model(&vad_path) {
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

/// Handle hotkey release: stop recording for push-to-talk mode
fn handle_hotkey_release(
    app: &tauri::AppHandle,
    ctrl: &tauri::State<'_, SharedController>,
    shortcut: &str,
) {
    let should_stop = {
        let mut c = lock_controller(ctrl);
        let should_stop = c.should_stop_profile_on_key_up(shortcut);
        c.note_hotkey_release(shortcut);
        should_stop
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
    focus_diag::log("key_up_stop_requested", serde_json::json!({}));

    // Compute how long we still need to hold to satisfy the minimum recording
    // duration — but do NOT block the global-shortcut (UI) thread waiting for it
    // (finding 2): a std::thread::sleep here freezes UI redraw and stalls
    // subsequent hotkey events. The delay is offloaded to an async task below.
    let (elapsed, generation) = {
        let mut c = lock_controller(ctrl);
        c.mark_release();
        (c.recording_elapsed(), c.recording_generation())
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
            let mut c = lock_controller(&ctrl);
            c.stop_recording_generation(generation)
        };
        let audio = match outcome {
            StopRecordingOutcome::NotRecording => return,
            StopRecordingOutcome::Failed(failure) => {
                let msg = failure.to_string();
                error!("Recording stop failed: {msg}");
                dispatch_to_main(&app_handle, |app| {
                    overlay::hide(app);
                    update_tray_status(app, "idle");
                });
                let _ = app_handle.emit_event(events::event::ERROR, &msg);
                let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                return;
            }
            StopRecordingOutcome::Stopped(audio) => {
                focus_diag::log(
                    "capture_stopped",
                    serde_json::json!({ "keyUpToCaptureStoppedMs": elapsed_ms(key_up_at) }),
                );
                audio
            }
        };
        let key_up_to_capture_stopped_ms = elapsed_ms(key_up_at);

        // Keep the recording indicator visible through processing, so key-up
        // never leaves the user without feedback while inference/paste runs.
        // Show the transcribing state — re-dispatched to the main
        // thread now that this runs on a worker.
        dispatch_to_main(&app_handle, |app| {
            update_tray_status(app, "transcribing");
        });
        let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Transcribing);

        if audio.is_empty() {
            {
                let ctrl: tauri::State<'_, SharedController> = app_handle.state();
                let mut c = lock_controller(&ctrl);
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
            let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Idle);
            return;
        }

        // Select the live engine off the UI thread. Both engines share the
        // bounded transaction and postprocessing/paste path below.
        let ctrl: tauri::State<'_, SharedController> = app_handle.state();
        let whisper: tauri::State<'_, SharedWhisper> = app_handle.state();

        // Extract what we need for transcription (lock briefly)
        let selection = {
            let c = lock_controller(&ctrl);
            let profile_id = c
                .active_hotkey_profile()
                .map(|profile| profile.id.clone())
                .unwrap_or_else(|| c.settings().default_profile().id);
            c.settings().live_model_for_profile(&profile_id).map(
                |(effective_model, use_pianissimo)| {
                    (
                        c.language(),
                        effective_model,
                        commands::build_transcribe_options_for_profile(
                            c.settings(),
                            Some(&profile_id),
                        ),
                        sagascript_core::transcription::Glossary::parse(
                            &c.settings().effective_glossary_source(Some(&profile_id)),
                        ),
                        use_pianissimo,
                    )
                },
            )
        };
        let (language, effective_model, opts, glossary, use_pianissimo) = match selection {
            Ok(selection) => selection,
            Err(error) => {
                error!("Cannot resolve profile model: {error}");
                if let Err(finish_error) =
                    lock_controller(&ctrl).finish_transcription(Err(error.clone()))
                {
                    error!("Profile-model failure completion: {finish_error}");
                }
                dispatch_to_main(&app_handle, |app| {
                    overlay::hide(app);
                    update_tray_status(app, "idle");
                });
                let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                let _ = app_handle.emit_event(events::event::ERROR, &error);
                return;
            }
        };

        let model_name = if use_pianissimo {
            "Pianissimo"
        } else {
            effective_model.display_name()
        }
        .to_string();
        let language_name = language.display_name().to_string();
        let beam_size = if use_pianissimo { 0 } else { opts.beam_size };
        let temperature_fallback = !use_pianissimo && opts.temperature_fallback;
        let vad_enabled = !use_pianissimo && opts.vad_model_path.is_some();
        let mut model_was_warm = if use_pianissimo {
            sagascript_core::transcription::pianissimo_backend::engine_is_warm()
        } else {
            !whisper.needs_reload(effective_model)
        };
        info!(
            key_up_to_transcribe_ms = elapsed_ms(key_up_at),
            model_was_warm, "Transcribing with model: {model_name}"
        );

        // Show model loading status in tray
        if !model_was_warm {
            let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::LoadingModel);
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
            let live_backend = std::sync::Arc::new(
                sagascript_core::transcription::live_dictation::LiveDictationBackend::new(
                    whisper.inner().clone(),
                    use_pianissimo,
                ),
            );
            let live_ref = live_backend.clone();
            let mut fut = tokio::task::spawn_blocking(move || {
                let mut timings =
                    sagascript_core::transcription::whisper_backend::DictationTimings::default();
                let result =
                    live_ref.transcribe(effective_model, &audio, language, &opts, &mut timings);
                (result, timings)
            });

            let timeout = Duration::from_secs(TRANSCRIPTION_TIMEOUT_SECS);
            match tokio::time::timeout(timeout, &mut fut).await {
                Ok(Ok((r, timings))) => {
                    let mut c = lock_controller(&ctrl);
                    if timings.model_acquisition_started {
                        model_load_ms = Some(timings.model_ms);
                        model_was_warm = timings.model_cached;
                        c.record_phase(
                            "model_acquisition",
                            Duration::from_secs_f64(timings.model_ms / 1000.0),
                        );
                        c.record_model_cache(timings.model_cached);
                    }
                    key_up_to_model_ready_ms = timings.model_ready_at.map(|ready| {
                        u64::try_from(ready.saturating_duration_since(key_up_at).as_millis())
                            .unwrap_or(u64::MAX)
                    });
                    if timings.inference_started {
                        whisper_ms = Some(timings.inference_ms);
                        c.record_phase(
                            "inference",
                            Duration::from_secs_f64(timings.inference_ms / 1000.0),
                        );
                    }
                    r
                }
                Ok(Err(e)) => Err(dictation_task_failed(e)),
                Err(_) => {
                    warn!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s — requesting abort");
                    live_backend.request_abort();
                    // Brief grace for the aborted inference to unwind; log which
                    // outcome occurred so a genuine hang is visible.
                    match tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut fut)
                        .await
                    {
                        Ok(_) => {
                            info!("Aborted transcription task exited — warm-state lock released")
                        }
                        Err(_) => error!(
                            "Transcription task still running {ABORT_GRACE_SECS}s after abort — \
                             warm state may stay locked until it unwinds; further transcriptions \
                             will report ModelBusy rather than block forever"
                        ),
                    }
                    Err(dictation_timed_out())
                }
            }
        };

        let key_up_to_whisper_complete_ms =
            whisper_ms.and_then(|_| elapsed_ms_if_success(&result, key_up_at));
        let postprocess_started = std::time::Instant::now();
        let result = result.map(|text| commands::apply_glossary(text, &glossary));
        lock_controller(&ctrl).record_phase("postprocessing", postprocess_started.elapsed());
        match result {
            Ok(text) => {
                info!("Transcription complete: {} chars", text.len());
                focus_diag::log(
                    "transcription_result_ready",
                    serde_json::json!({ "keyUpToResultMs": elapsed_ms(key_up_at) }),
                );

                if text.trim().is_empty() {
                    let mut c = lock_controller(&ctrl);
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
                    let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                    dispatch_to_main(&app_handle, |app| {
                        overlay::hide(app);
                        update_tray_status(app, "idle");
                    });
                    info!("No speech detected; returning to idle without paste");
                    return;
                }

                // Check if auto-paste is enabled (lock briefly)
                let should_paste = {
                    let c = lock_controller(&ctrl);
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
                    #[cfg(target_os = "macos")]
                    restore_target_before_paste(&app_handle).await;
                    let (paste_tx, paste_rx) = tokio::sync::oneshot::channel();
                    let paste_task = move || {
                        #[cfg(target_os = "macos")]
                        let paste_result = crate::paste::PasteService::new()
                            .paste_checked(&text_for_paste, || {
                                let check = platform::macos::dictation_paste_guard_check();
                                focus_diag::log(
                                    "paste_guard_check",
                                    serde_json::json!({
                                        "decision": if check.valid { "allow" } else { "reject" },
                                        "reason": check.reason,
                                        "targetPid": check.target_pid,
                                        "currentFrontmostPid": check.current_pid,
                                        "ownPid": check.own_pid,
                                        "frontmostIsSagascript": check.current_pid == Some(check.own_pid),
                                        "frontmostMatchesTarget": check.current_pid == Some(check.target_pid),
                                    }),
                                );
                                check.valid
                            })
                            .map_err(|error| error.to_string());
                        #[cfg(not(target_os = "macos"))]
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
                        info!(
                            paste_ms = paste_started.elapsed().as_millis() as u64,
                            key_up_to_paste_ms = elapsed_ms(key_up_at),
                            outcome = completion.outcome,
                            "Dictation paste finished"
                        );
                        if completion.call_completed {
                            key_up_to_paste_completed_ms = Some(elapsed_ms(key_up_at));
                        } else if paste_outcome == "timed_out" {
                            warn!("Auto-paste completion timed out after {PASTE_COMPLETION_TIMEOUT_MS}ms");
                        }
                    }
                    lock_controller(&ctrl)
                        .record_phase("clipboard_focus_paste", paste_started.elapsed());
                }

                let mut c = lock_controller(&ctrl);
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
                    if let Err(error) = c.set_update_result_pending("live-dictation", true) {
                        warn!("Could not mark failed paste result for recovery: {error}");
                    }
                    c.on_transcription_error(&message);
                    drop(c);
                    focus_diag::log(
                        "result_emitted_to_ui",
                        serde_json::json!({ "outcome": "paste_error", "pasteOutcome": paste_outcome }),
                    );
                    let _ = app_handle.emit_event(events::event::TRANSCRIPTION_RESULT, &text);
                    let _ = app_handle.emit_event(events::event::ERROR, &message);
                    let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                    let open_copy_fallback =
                        paste_completion::should_open_copy_fallback(paste_outcome);
                    dispatch_to_main(&app_handle, move |app| {
                        overlay::hide(app);
                        update_tray_status(app, "idle");
                        if open_copy_fallback {
                            open_settings_window(
                                app,
                                Some("dictate"),
                                "paste_failed_copy_fallback",
                            );
                        }
                    });
                    return;
                }
                c.on_transcription_success(&text);
                if paste_outcome == "succeeded" {
                    if let Err(error) = c.set_update_result_pending("live-dictation", false) {
                        warn!("Could not clear successfully pasted result from recovery: {error}");
                    }
                }
                drop(c);

                let _ = app_handle.emit_event(events::event::TRANSCRIPTION_RESULT, &text);
                let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Idle);
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
                let mut c = lock_controller(&ctrl);
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
                let failure = dictation_failure_text(&e);
                c.on_transcription_error(&failure);
                drop(c);
                let _ = app_handle.emit_event(events::event::ERROR, &failure);
                // Let the overlay show its error state briefly before hiding.
                let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Error);
                let _ = app_handle.emit_event(events::event::STATE_CHANGED, UiState::Idle);
                dispatch_to_main(&app_handle, |app| update_tray_status(app, "idle"));
                hide_overlay_after_error(app_handle.clone());
                info!("Error flow complete, app should remain running");
            }
        }
    });
}

/// Issue #256: a deferred activation can leave Sagascript frontmost at key-up.
/// Re-activate the dictation target before the paste guard runs. The wait runs
/// on a blocking thread; each AppKit call is dispatched to the main thread so
/// its event loop keeps running and the activation can actually land. Never
/// pastes: the guard still decides afterwards.
#[cfg(target_os = "macos")]
async fn restore_target_before_paste(app: &tauri::AppHandle) {
    use platform::macos as mac;
    fn on_main<T: Send + 'static>(
        app: &tauri::AppHandle,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> Option<T> {
        let (tx, rx) = std::sync::mpsc::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(f());
        })
        .ok()?;
        rx.recv_timeout(Duration::from_millis(500)).ok()
    }
    let app = app.clone();
    let result = tokio::task::spawn_blocking(move || {
        let target = mac::dictation_target_pid();
        let own = mac::current_pid();
        let a1 = app.clone();
        let a2 = app.clone();
        let a3 = app.clone();
        mac::restore_target_before_paste(
            target,
            own,
            move || on_main(&a1, mac::frontmost_pid).flatten(),
            move |pid| on_main(&a2, move || mac::target_is_running(pid)).unwrap_or(false),
            move |pid| on_main(&a3, move || mac::activate_dictation_target(pid)).unwrap_or(false),
            std::thread::sleep,
        )
    })
    .await;
    match result {
        Ok(outcome) => {
            if outcome.attempted || outcome.reason != "not_needed" {
                focus_diag::log(
                    "focus_restore_before_paste",
                    serde_json::json!({
                        "attempted": outcome.attempted,
                        "restored": outcome.restored,
                        "attempts": outcome.attempts,
                        "waitMs": outcome.wait_ms,
                        "reason": outcome.reason,
                    }),
                );
            }
        }
        Err(error) => warn!("Focus restore before paste failed to run: {error}"),
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
