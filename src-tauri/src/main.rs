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
mod dictation;
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

use tauri_plugin_global_shortcut::ShortcutState;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use dictation::handle_hotkey_event;
#[cfg(windows)]
use launch::{attach_parent_console, cli_args_request_console_output};
use launch::{gui_launch_mode, handle_run_event, handle_second_instance, setup_app, GuiLaunchMode};
// commands.rs reaches the profile menu refresh as `crate::update_profiles_menu`.
// Drop this alias once commands/ names `crate::tray::update_profiles_menu` (#369 follow-up).
pub(crate) use tray::update_profiles_menu;
#[cfg(target_os = "macos")]
use updater::updates;
// commands/ and app_controller.rs reach these under their old crate paths. Drop the aliases
// once they name `crate::updater::{activity, recovery}` (#369 follow-up).
pub(crate) use updater::{activity as update_activity, recovery as update_recovery};

const SAFE_FALLBACK_HOTKEY: &str = "Control+Shift+Space";

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
            return sagascript_cli::run_with_build(
                parsed,
                &sagascript_cli::output::Out::stdio(),
                sagascript_core::acceptance::BuildObservation::new(
                    env!("CARGO_PKG_VERSION"),
                    env!("SAGASCRIPT_SOURCE_REVISION"),
                    env!("SAGASCRIPT_SOURCE_DIRTY"),
                ),
            )
            .into();
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
            commands::acceptance_preflight,
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
