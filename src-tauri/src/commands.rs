use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::State;
use tracing::{error, info, warn};

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// After a timeout fires and we request an abort, how long to wait for the
/// blocking inference to actually unwind and release the warm-state lock before
/// logging that it is still stuck. The real abort callback returns within a
/// compute step or two, so this rarely elapses.
const ABORT_GRACE_SECS: u64 = 5;

use crate::app_controller::{AppController, AppState, StopRecordingOutcome};
use crate::hotkey::{HotkeyHealth, HotkeyStatus};
use sagascript_core::audio::decoder;
use sagascript_core::settings::{HotkeyMode, Language, Settings, WhisperModel};
use sagascript_core::transcription::{model, FILE_TRANSCRIBE_BEAM, TranscribeOptions, WhisperBackend};

/// Build the per-transcription options from the current settings. Resolves the
/// VAD model path only when VAD is enabled and the model is present (otherwise
/// VAD is silently skipped — whisper would fail on a missing model).
pub(crate) fn build_transcribe_options(settings: &Settings) -> TranscribeOptions {
    let prompt = settings.initial_prompt.trim();
    let vad_model_path = if settings.vad_enabled {
        let p = model::vad_model_path();
        if p.exists() {
            p.to_str().map(str::to_string)
        } else {
            tracing::warn!("VAD enabled but model not downloaded — skipping VAD");
            None
        }
    } else {
        None
    };
    TranscribeOptions {
        prompt: if prompt.is_empty() {
            None
        } else {
            Some(prompt.to_string())
        },
        beam_size: settings.beam_size,
        temperature_fallback: settings.temperature_fallback,
        vad_model_path,
        segment_timestamps: false,
    }
}

/// Like [`build_transcribe_options`] but for file transcription: defaults to
/// beam search for quality (unless the user explicitly set a beam width), and
/// uses the file dialog's prompt when provided (otherwise the saved prompt).
pub(crate) fn build_file_transcribe_options(
    settings: &Settings,
    prompt: Option<String>,
) -> TranscribeOptions {
    let mut opts = build_transcribe_options(settings);
    if opts.beam_size < 2 {
        opts.beam_size = FILE_TRANSCRIBE_BEAM;
    }
    if let Some(p) = prompt {
        if !p.trim().is_empty() {
            opts.prompt = Some(p.trim().to_string());
        }
    }
    opts
}

/// Shared app state type — uses std::sync::Mutex (not tokio) because
/// cpal::Stream is !Send and we need sync access from Tauri commands
pub type SharedController = Mutex<AppController>;

/// Shared whisper backend — separate from AppController to avoid holding
/// the controller lock during blocking transcription
pub type SharedWhisper = Arc<WhisperBackend>;

// -- State queries --

#[tauri::command]
pub async fn get_state(controller: State<'_, SharedController>) -> Result<AppState, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.state())
}

#[tauri::command]
pub async fn get_settings(controller: State<'_, SharedController>) -> Result<Settings, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.settings().clone())
}

#[tauri::command]
pub async fn get_last_transcription(
    controller: State<'_, SharedController>,
) -> Result<Option<String>, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.last_transcription().map(|s| s.to_string()))
}

#[tauri::command]
pub async fn get_last_error(
    controller: State<'_, SharedController>,
) -> Result<Option<String>, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.last_error().map(|s| s.to_string()))
}

#[tauri::command]
pub async fn is_model_ready(controller: State<'_, SharedController>) -> Result<bool, String> {
    let ctrl = controller.lock().unwrap();
    Ok(ctrl.is_model_ready())
}

/// Returns the display name of the currently loaded (or effective) model
#[tauri::command]
pub async fn get_loaded_model(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<LoadedModelInfo, String> {
    let ctrl = controller.lock().unwrap();
    let effective = ctrl.settings().effective_model();
    let loaded = whisper.loaded_model();
    Ok(LoadedModelInfo {
        effective_model: effective.display_name().to_string(),
        effective_model_id: serde_json::to_value(effective)
            .and_then(serde_json::from_value::<String>)
            .unwrap_or_else(|_| format!("{:?}", effective)),
        loaded_model: loaded.map(|m| m.display_name().to_string()),
        is_loaded: loaded == Some(effective),
        is_downloaded: model::is_model_downloaded(effective),
    })
}

// -- Settings mutations --

#[tauri::command]
pub async fn set_language(
    controller: State<'_, SharedController>,
    language: Language,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.language = language;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().language = persisted.language;
    info!("Language set to {:?}", language);
    Ok(())
}

#[tauri::command]
pub async fn set_onboarding_completed(
    controller: State<'_, SharedController>,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.has_completed_onboarding = true;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().has_completed_onboarding = persisted.has_completed_onboarding;
    info!("Onboarding marked as completed");
    Ok(())
}

#[tauri::command]
pub async fn set_whisper_model(
    controller: State<'_, SharedController>,
    model: WhisperModel,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.whisper_model = model;
        settings.auto_select_model = false;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().whisper_model = persisted.whisper_model;
    ctrl.settings_mut().auto_select_model = persisted.auto_select_model;
    info!("Model set to {:?}", model);
    Ok(())
}

#[tauri::command]
pub async fn set_auto_select_model(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.auto_select_model = enabled;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().auto_select_model = persisted.auto_select_model;
    info!("Auto-select model: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_hotkey_mode(
    controller: State<'_, SharedController>,
    mode: HotkeyMode,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.hotkey_mode = mode;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().hotkey_mode = persisted.hotkey_mode;
    info!("Hotkey mode set to {:?}", mode);
    Ok(())
}

#[tauri::command]
pub async fn set_hotkey(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    health: State<'_, HotkeyHealth>,
    shortcut: String,
) -> Result<(), String> {
    use tauri::Emitter;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let old_shortcut = {
        let ctrl = controller.lock().unwrap();
        ctrl.settings().hotkey.clone()
    };

    // Unregister old shortcut
    if let Err(e) = app.global_shortcut().unregister(old_shortcut.as_str()) {
        error!("Failed to unregister old hotkey '{}': {}", old_shortcut, e);
        // Continue anyway — might already be unregistered
    }

    // Register new shortcut
    if let Err(e) = app.global_shortcut().register(shortcut.as_str()) {
        error!("Failed to register new hotkey '{}': {}", shortcut, e);
        // Try to re-register the old one so the app isn't left with no
        // hotkey bound at all. If that succeeds, the app is still healthy
        // (the *requested* change failed, which is already surfaced to the
        // caller via the returned Err below) — only record a health failure
        // if even the fallback re-registration fails.
        let change = match app.global_shortcut().register(old_shortcut.as_str()) {
            Ok(()) => {
                info!("Re-registered old hotkey '{}' after failed change", old_shortcut);
                health.record(&old_shortcut, None)
            }
            Err(e2) => {
                error!("Failed to re-register old hotkey '{}': {}", old_shortcut, e2);
                health.record(&old_shortcut, Some(e2.to_string()))
            }
        };
        if change.changed {
            let _ = app.emit(crate::events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
        }
        return Err(format!("Failed to register hotkey '{}': {}", shortcut, e));
    }

    let persisted = match sagascript_core::settings::store::update(|settings| {
        settings.hotkey = shortcut.clone();
    }) {
        Ok(settings) => settings,
        Err(save_error) => {
            // Registration already changed process-global state. If the disk
            // write fails, restore the operational shortcut so controller,
            // disk, and registration cannot diverge.
            if let Err(e) = app.global_shortcut().unregister(shortcut.as_str()) {
                error!("Failed to unregister unpersisted hotkey '{shortcut}': {e}");
            }
            let rollback_error = app
                .global_shortcut()
                .register(old_shortcut.as_str())
                .err()
                .map(|e| e.to_string());
            let change = health.record(&old_shortcut, rollback_error.clone());
            if change.changed {
                let _ = app.emit(crate::events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
            }
            return match rollback_error {
                Some(error) => Err(format!(
                    "Failed to persist hotkey: {save_error}; restoring '{old_shortcut}' also failed: {error}"
                )),
                None => Err(format!(
                    "Failed to persist hotkey: {save_error}; restored '{old_shortcut}'"
                )),
            };
        }
    };

    // Update controller state after persistence succeeds.
    {
        let mut ctrl = controller.lock().unwrap();
        ctrl.settings_mut().hotkey = persisted.hotkey.clone();
        ctrl.hotkey_service_mut().set_shortcut(&persisted.hotkey);
    }

    let change = health.record(&shortcut, None);
    if change.changed {
        let _ = app.emit(crate::events::event::HOTKEY_REGISTRATION_CHANGED, &change.status);
    }

    info!("Hotkey changed to: {shortcut}");
    Ok(())
}

/// Current hotkey registration health — whether the last registration
/// attempt (at startup, from this command, or from the settings-file
/// watcher's hot-reload) actually succeeded. Reads the process-wide flag
/// rather than querying the global-shortcut plugin's `is_registered()`,
/// which only tells you *a* shortcut is bound, not whether *our* most recent
/// attempt to bind it succeeded.
#[tauri::command]
pub async fn hotkey_status(health: State<'_, HotkeyHealth>) -> Result<HotkeyStatus, String> {
    Ok(health.status())
}

// -- Recording --

#[tauri::command]
pub async fn start_recording(controller: State<'_, SharedController>) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    gui_start_recording_result(ctrl.start_recording())
}

fn gui_start_recording_result(
    result: Result<bool, sagascript_core::error::DictationError>,
) -> Result<(), String> {
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(
            "Cannot start recording while Sagascript is busy. Wait for the current transcription to finish."
                .to_string(),
        ),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod gui_recording_tests {
    use super::gui_start_recording_result;

    #[test]
    fn gui_start_while_transcribing_returns_busy_error() {
        let error = gui_start_recording_result(Ok(false)).unwrap_err();

        assert!(error.contains("busy"));
        assert!(error.contains("current transcription"));
    }
}

#[tauri::command]
pub async fn stop_and_transcribe(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<String, String> {
    let (audio, language, effective_model, opts) = {
        let mut ctrl = controller.lock().unwrap();
        // Guard against a late/duplicate invoke racing the hotkey stop path
        // (finding 3): if we're not recording, do nothing and return Ok-empty
        // (NOT Err — an error would surface a misleading toast in the UI) so an
        // in-flight transcription's state/last_error is not clobbered.
        let audio = match ctrl.stop_recording_guarded() {
            StopRecordingOutcome::NotRecording => return Ok(String::new()),
            // Capture/resample failure (finding 4): the controller already
            // recorded the error and returned to Idle; surface the real error.
            StopRecordingOutcome::Failed(msg) => return Err(msg),
            StopRecordingOutcome::Stopped(audio) => audio,
        };
        let language = ctrl.language();
        let effective_model = ctrl.settings().effective_model();
        let opts = build_transcribe_options(ctrl.settings());
        (audio, language, effective_model, opts)
    };

    if audio.is_empty() {
        return controller
            .lock()
            .unwrap()
            .finish_transcription(Err("No audio captured".to_string()));
    }

    // Every outcome after recording stops must flow through
    // `finish_transcription`: stop_recording_guarded has already moved the
    // controller to Transcribing, so returning early would wedge subsequent
    // recording attempts until the app restarts.
    let result = if let Err(error) = whisper.ensure_model(effective_model) {
        Err(error.to_string())
    } else {
        // Run blocking transcription on a separate thread with a timeout. On timeout
        // we now trigger a REAL abort (the whisper-rs abort callback wired in
        // WhisperBackend): request_abort() flips the flag whisper.cpp checks between
        // compute steps, so the blocking task returns promptly and releases the warm
        // state instead of running to completion and wedging the pipeline. The handle
        // is kept borrowed (`&mut fut`) across the timeout so we can await its actual
        // exit after abort and log whether the lock was released.
        let whisper_ref = whisper.inner().clone();
        let mut fut = tokio::task::spawn_blocking(move || {
            whisper_ref.transcribe_sync_with_options(&audio, language, &opts, |_| {})
        });

        let timeout = Duration::from_secs(TRANSCRIPTION_TIMEOUT_SECS);
        match tokio::time::timeout(timeout, &mut fut).await {
            Ok(Ok(result)) => result.map_err(|error| error.to_string()),
            Ok(Err(error)) => Err(format!("Transcription task failed: {error}")),
            Err(_) => {
                warn!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s — requesting abort");
                whisper.request_abort();
                // Give the aborted inference a brief grace to unwind, and log which
                // outcome occurred so a genuine hang is distinguishable from a clean
                // abort.
                match tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut fut).await {
                    Ok(_) => info!("Aborted transcription task exited — warm-state lock released"),
                    Err(_) => error!(
                        "Transcription task still running {ABORT_GRACE_SECS}s after abort — the \
                         warm state may stay locked until it unwinds; further transcriptions will \
                         report ModelBusy rather than block forever"
                    ),
                }
                Err(format!(
                    "Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s (inference aborted)"
                ))
            }
        }
    };

    // NOTE: auto-paste is NOT done here — enigo's macOS TIS APIs crash if
    // called from a tokio worker thread (SIGTRAP in dispatch_assert_queue).
    // The hotkey path in main.rs handles paste via run_on_main_thread(). This
    // command returns the text to the frontend for display instead.
    controller.lock().unwrap().finish_transcription(result)
}

#[tauri::command]
pub async fn cancel_recording(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
) -> Result<(), String> {
    let mut ctrl = controller.lock().unwrap();
    ctrl.cancel_recording();
    drop(ctrl);
    crate::overlay::hide(&app);
    Ok(())
}

// -- Model management --

#[tauri::command]
pub async fn is_model_downloaded(whisper_model: WhisperModel) -> Result<bool, String> {
    Ok(model::is_model_downloaded(whisper_model))
}

#[tauri::command]
pub async fn get_model_info(
    controller: State<'_, SharedController>,
) -> Result<Vec<ModelInfo>, String> {
    let ctrl = controller.lock().unwrap();
    let language = ctrl.settings().language;
    let effective = ctrl.settings().effective_model();
    let models = WhisperModel::models_for_language(language);

    Ok(models
        .iter()
        .map(|m| ModelInfo {
            id: serde_json::to_value(m)
                .and_then(serde_json::from_value::<String>)
                .unwrap_or_else(|_| format!("{:?}", m)),
            display_name: m.display_name().to_string(),
            description: m.description().to_string(),
            size_mb: m.size_mb(),
            downloaded: model::is_model_downloaded(*m),
            active: *m == effective,
        })
        .collect())
}

// -- Model download --

#[tauri::command]
pub async fn download_model(
    app: tauri::AppHandle,
    whisper_model: WhisperModel,
) -> Result<(), String> {
    use tauri::Emitter;
    let app_handle = app.clone();
    model::download_model(whisper_model, move |downloaded, total| {
        let progress = if total > 0 {
            (downloaded as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };
        let _ = app_handle.emit(
            crate::events::event::MODEL_DOWNLOAD_PROGRESS,
            serde_json::json!({
                "model": format!("{:?}", whisper_model),
                "downloaded": downloaded,
                "total": total,
                "progress": progress,
            }),
        );
    })
    .await
    .map_err(|e| e.to_string())?;

    let _ = app.emit(crate::events::event::MODEL_READY, ());
    Ok(())
}

// -- Settings toggles --

fn effective_auto_paste(requested: bool, permission_granted: bool) -> bool {
    requested && permission_granted
}

#[tauri::command]
pub async fn set_auto_paste(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let permission_granted = !enabled || crate::platform::macos::is_accessibility_trusted();
    #[cfg(not(target_os = "macos"))]
    let permission_granted = true;

    let effective = effective_auto_paste(enabled, permission_granted);
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.auto_paste = effective;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().auto_paste = persisted.auto_paste;
    info!("Auto-paste: {effective}");
    if enabled && !permission_granted {
        Err("Accessibility permission is required before auto-paste can be enabled".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod auto_paste_tests {
    use super::effective_auto_paste;

    #[test]
    fn enabling_never_persists_true_before_accessibility_is_trusted() {
        assert!(!effective_auto_paste(true, false));
        assert!(effective_auto_paste(true, true));
        assert!(!effective_auto_paste(false, true));
    }
}

#[tauri::command]
pub async fn set_show_overlay(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.show_overlay = enabled;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().show_overlay = persisted.show_overlay;
    info!("Show overlay: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_initial_prompt(
    controller: State<'_, SharedController>,
    prompt: String,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.initial_prompt = prompt.clone();
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().initial_prompt = persisted.initial_prompt;
    info!("Initial prompt set ({} chars)", prompt.len());
    Ok(())
}

#[tauri::command]
pub async fn set_beam_size(
    controller: State<'_, SharedController>,
    beam_size: u32,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.beam_size = beam_size;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().beam_size = persisted.beam_size;
    info!("Beam size: {beam_size}");
    Ok(())
}

#[tauri::command]
pub async fn set_temperature_fallback(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.temperature_fallback = enabled;
    })?;
    let mut ctrl = controller.lock().unwrap();
    ctrl.settings_mut().temperature_fallback = persisted.temperature_fallback;
    info!("Temperature fallback: {enabled}");
    Ok(())
}

#[tauri::command]
pub async fn set_vad_enabled(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.vad_enabled = enabled;
    })?;
    {
        let mut ctrl = controller.lock().unwrap();
        ctrl.settings_mut().vad_enabled = persisted.vad_enabled;
    }
    // Fetch the Silero VAD model so it's ready for the next dictation. Done
    // after releasing the lock (no lock held across await).
    if enabled {
        info!("Verifying or downloading VAD model...");
        model::download_vad_model(|_, _| {})
            .await
            .map_err(|e| format!("Failed to download VAD model: {e}"))?;
    }
    info!("VAD enabled: {enabled}");
    Ok(())
}

// -- File transcription --

#[tauri::command]
pub async fn transcribe_file(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    file_path: String,
    prompt: Option<String>,
    diarize: Option<bool>,
) -> Result<String, String> {
    use tauri::Emitter;

    let path = std::path::PathBuf::from(&file_path);

    // Decode audio file
    let audio = tokio::task::spawn_blocking(move || decoder::decode_audio_file(&path))
        .await
        .map_err(|e| format!("Decode task failed: {e}"))?
        .map_err(|e| e.to_string())?;

    if audio.is_empty() {
        return Err("No audio decoded from file".to_string());
    }

    // File transcription (beam search / diarization) is far slower than live
    // dictation, so scale the timeout by the decoded duration rather than using
    // the short live-dictation timeout (which beam search could otherwise hit).
    let file_timeout = Duration::from_secs(
        ((audio.len() / 16_000) as u64 * 6).max(TRANSCRIPTION_TIMEOUT_SECS),
    );

    // Suppress unused-variable warning on `diarize` when the diarization feature is off
    #[cfg(not(feature = "diarization"))]
    let _ = &diarize;

    // Get transcription settings
    let (language, effective_model) = {
        let ctrl = controller.lock().unwrap();
        (ctrl.language(), ctrl.settings().effective_model())
    };

    // Show model loading status if needed
    if whisper.needs_reload(effective_model) {
        let _ = app.emit(crate::events::event::STATE_CHANGED, "loading_model");
    }

    // Ensure model is loaded
    if let Err(error) = whisper.ensure_model(effective_model) {
        let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
        return Err(error.to_string());
    }

    let _ = app.emit(crate::events::event::STATE_CHANGED, "transcribing");

    // Diarization path — runs both diarization and timestamped transcription in parallel,
    // then merges and consolidates speaker-attributed segments.
    #[cfg(feature = "diarization")]
    if diarize.unwrap_or(false) {
        use sagascript_core::diarization::{
            DiarizeConfig, TimestampedSegment,
            diarize as run_diarize,
            merge::{consolidate, merge_with_transcript},
            model::{DiarizationModel, download_model as download_diarization_model},
        };

        // Checking diarization in the file-transcription UI is an explicit
        // action: verify existing app-managed artifacts and repair only exact
        // integrity mismatches before native ONNX parsing. This never runs as
        // a silent startup download.
        for diarization_model in DiarizationModel::ALL {
            if let Err(error) = download_diarization_model(*diarization_model, |_, _| {}).await {
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(error.to_string());
            }
        }

        let whisper_ref = whisper.inner().clone();
        // Fall back to the saved initial_prompt when the file-dialog prompt is
        // empty (matches the standard file path).
        let prompt_ref: Option<String> = prompt.clone().filter(|p| !p.trim().is_empty()).or_else(|| {
            let saved = controller.lock().unwrap().settings().initial_prompt.trim().to_string();
            (!saved.is_empty()).then_some(saved)
        });
        let audio_for_diarize = audio.clone();
        let audio_for_transcribe = audio.clone();

        // Run diarization
        let mut diarize_fut = tokio::task::spawn_blocking(move || {
            run_diarize(&audio_for_diarize, &DiarizeConfig::default())
        });

        // Run word-level timestamped transcription when DTW is available.
        // Segment-level timestamps can span multiple speaker turns and would
        // cause maximum-overlap merging to collapse the GUI output to one label.
        let mut transcribe_fut = tokio::task::spawn_blocking(move || {
            whisper_ref.transcribe_sync_for_diarization(
                &audio_for_transcribe,
                language,
                prompt_ref.as_deref(),
            )
        });

        let timeout = file_timeout;
        // Join over BORROWED handles so the transcription handle stays available
        // for the post-abort grace await on the timeout path below.
        let (speaker_segments, raw_segments) = match tokio::time::timeout(timeout, async {
            tokio::join!(&mut diarize_fut, &mut transcribe_fut)
        })
        .await
        {
            Ok((Ok(Ok(spk)), Ok(Ok(trx)))) => (spk, trx),
            Ok((Ok(Err(e)), _)) | Ok((_, Ok(Err(e)))) => {
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(e.to_string());
            }
            Ok((Err(e), _)) | Ok((_, Err(e))) => {
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(format!("Task join error: {e}"));
            }
            Err(_) => {
                // Real abort: releases the whisper warm-state lock so the next
                // transcription isn't wedged. (The diarization half runs its
                // own compute and simply detaches when its handle is dropped.)
                warn!(
                    "Diarized transcription timed out after {}s — requesting abort",
                    timeout.as_secs()
                );
                whisper.request_abort();
                // Brief grace for the aborted inference to unwind; log which
                // outcome occurred so a genuine hang is visible.
                match tokio::time::timeout(
                    Duration::from_secs(ABORT_GRACE_SECS),
                    &mut transcribe_fut,
                )
                .await
                {
                    Ok(_) => info!("Aborted transcription task exited — warm-state lock released"),
                    Err(_) => error!(
                        "Transcription task still running {ABORT_GRACE_SECS}s after abort — \
                         warm state may stay locked until it unwinds; further transcriptions \
                         will report ModelBusy rather than block forever"
                    ),
                }
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
                return Err(format!(
                    "Transcription timed out after {}s (inference aborted)",
                    timeout.as_secs()
                ));
            }
        };

        let transcript: Vec<TimestampedSegment> = raw_segments
            .into_iter()
            .map(|(start, end, text)| TimestampedSegment { start, end, text })
            .collect();

        let diarized = merge_with_transcript(&speaker_segments, &transcript);
        let mut consolidated = consolidate(&diarized);
        for segment in &mut consolidated {
            segment.text = sagascript_core::transcription::normalize_nonspeech_markers(
                &segment.text,
                language,
            );
        }

        let text = consolidated
            .iter()
            .map(|s| format!("[{}] {}", s.speaker, s.text.trim()))
            .collect::<Vec<_>>()
            .join("\n");

        info!("Diarized file transcription complete: {} chars", text.len());

        let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");

        // Auto-paste if enabled
        let should_paste = {
            let c = controller.lock().unwrap();
            c.settings().auto_paste
        };
        if should_paste {
            let text_for_paste = text.clone();
            if let Err(e) = app.run_on_main_thread(move || {
                let paste_svc = crate::paste::PasteService::new();
                if let Err(e) = paste_svc.paste(&text_for_paste) {
                    error!("Auto-paste failed: {e}");
                }
            }) {
                error!("Failed to dispatch paste to main thread: {e}");
            }
        }

        return Ok(text);
    }

    // Standard (non-diarize) transcription path. File transcription defaults to
    // beam search (quality over latency).
    let opts = {
        let ctrl = controller.lock().unwrap();
        build_file_transcribe_options(ctrl.settings(), prompt)
    };
    let whisper_ref = whisper.inner().clone();
    let app_progress = app.clone();
    // Borrowed handle (`&mut fut`) so the timeout path can await the task's
    // actual exit after requesting an abort — mirrors the live dictation path.
    let mut fut = tokio::task::spawn_blocking(move || {
        whisper_ref.transcribe_sync_with_options(&audio, language, &opts, move |pct| {
            let _ = app_progress.emit(crate::events::event::TRANSCRIPTION_PROGRESS, pct);
        })
    });

    let timeout = file_timeout;
    let result = match tokio::time::timeout(timeout, &mut fut).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
            return Err(format!("Transcription task failed: {e}"));
        }
        Err(_) => {
            warn!(
                "File transcription timed out after {}s — requesting abort",
                timeout.as_secs()
            );
            whisper.request_abort();
            // Brief grace for the aborted inference to unwind; log which outcome
            // occurred so a genuine hang is visible.
            match tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut fut).await {
                Ok(_) => info!("Aborted transcription task exited — warm-state lock released"),
                Err(_) => error!(
                    "Transcription task still running {ABORT_GRACE_SECS}s after abort — \
                     warm state may stay locked until it unwinds; further transcriptions \
                     will report ModelBusy rather than block forever"
                ),
            }
            let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
            return Err(format!(
                "Transcription timed out after {}s (inference aborted)",
                timeout.as_secs()
            ));
        }
    };

    let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");

    match result {
        Ok(text) => {
            info!("File transcription complete: {} chars", text.len());

            // Auto-paste if enabled
            let should_paste = {
                let c = controller.lock().unwrap();
                c.settings().auto_paste
            };

            if should_paste {
                let text_for_paste = text.clone();
                if let Err(e) = app.run_on_main_thread(move || {
                    let paste_svc = crate::paste::PasteService::new();
                    if let Err(e) = paste_svc.paste(&text_for_paste) {
                        error!("Auto-paste failed: {e}");
                    }
                }) {
                    error!("Failed to dispatch paste to main thread: {e}");
                }
            }

            Ok(text)
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn get_supported_formats() -> Result<Vec<String>, String> {
    Ok(decoder::SUPPORTED_EXTENSIONS
        .iter()
        .map(|s| s.to_string())
        .collect())
}

// -- Build info --

#[tauri::command]
pub async fn get_build_info() -> Result<BuildInfo, String> {
    Ok(BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        git_hash: env!("GIT_HASH").to_string(),
        build_date: env!("BUILD_DATE").to_string(),
    })
}

#[derive(serde::Serialize)]
pub struct BuildInfo {
    version: String,
    git_hash: String,
    build_date: String,
}

#[derive(serde::Serialize)]
pub struct ModelInfo {
    id: String,
    display_name: String,
    description: String,
    size_mb: u32,
    downloaded: bool,
    active: bool,
}

// -- Permission / platform queries (for onboarding) --

#[tauri::command]
pub async fn check_accessibility_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        Ok(crate::platform::macos::is_accessibility_trusted())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

#[tauri::command]
pub async fn request_accessibility_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        crate::platform::macos::request_accessibility_permission()?;
    }
    Ok(())
}

/// Returns the microphone authorization status as a string.
/// Possible values: "authorized", "not_determined", "denied", "restricted", "unsupported".
/// "unsupported" is returned when the binary is not running from a proper .app bundle
/// (e.g. during `cargo run` or `cargo tauri dev` without a bundle).
#[tauri::command]
pub async fn microphone_status() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        if !macos_mic::is_in_app_bundle() {
            return Ok("unsupported".to_string());
        }
        Ok(macos_mic::authorization_status_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("authorized".to_string())
    }
}

/// Triggers AVCaptureDevice.requestAccessForMediaType:completionHandler: and returns
/// the new status string after the user responds (or immediately if already determined).
/// Possible return values: "authorized", "not_determined", "denied", "restricted", "unsupported".
#[tauri::command]
pub async fn request_microphone_access() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        if !macos_mic::is_in_app_bundle() {
            return Ok("unsupported".to_string());
        }
        // Use spawn_blocking to avoid starving the tokio runtime during the
        // up-to-60s wait for the user to respond to the permission dialog.
        tokio::task::spawn_blocking(|| {
            use std::sync::mpsc;
            let (tx, rx) = mpsc::channel();
            macos_mic::request_access(move |_granted| {
                let _ = tx.send(());
            });
            let _ = rx.recv_timeout(std::time::Duration::from_secs(60));
            macos_mic::authorization_status_string()
        })
        .await
        .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok("authorized".to_string())
    }
}

#[tauri::command]
pub async fn open_microphone_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            .spawn()
            .map_err(|e| format!("Failed to open System Settings: {}", e))?;
    }
    Ok(())
}

/// macOS-specific microphone permission helpers using AVCaptureDevice via objc.
#[cfg(target_os = "macos")]
mod macos_mic {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};
    use std::sync::Once;

    /// AVAuthorizationStatus values
    const AV_AUTH_STATUS_NOT_DETERMINED: isize = 0;
    const AV_AUTH_STATUS_DENIED: isize = 2;

    /// Returns true if the binary is running from inside a proper .app bundle.
    /// When running via `cargo run` or `cargo tauri dev` without a bundle,
    /// NSBundle.mainBundle.bundleIdentifier returns nil — TCC won't attribute
    /// permission requests correctly in that case.
    pub fn is_in_app_bundle() -> bool {
        unsafe {
            let ns_bundle_class = match Class::get("NSBundle") {
                Some(c) => c,
                None => return false,
            };
            let main_bundle: *mut Object = msg_send![ns_bundle_class, mainBundle];
            if main_bundle.is_null() {
                return false;
            }
            let bundle_id: *mut Object = msg_send![main_bundle, bundleIdentifier];
            if bundle_id.is_null() {
                return false;
            }
            // Check that the string is non-empty
            let len: usize = msg_send![bundle_id, length];
            len > 0
        }
    }

    /// Ensure AVFoundation framework is loaded (required for AVCaptureDevice class lookup).
    fn ensure_avfoundation_loaded() {
        static LOAD: Once = Once::new();
        LOAD.call_once(|| {
            unsafe {
                let ns_bundle_class = Class::get("NSBundle").expect("NSBundle class");
                let path: *mut Object = msg_send![
                    Class::get("NSString").expect("NSString"),
                    stringWithUTF8String: c"/System/Library/Frameworks/AVFoundation.framework".as_ptr()
                ];
                let bundle: *mut Object = msg_send![ns_bundle_class, bundleWithPath: path];
                if !bundle.is_null() {
                    let _loaded: bool = msg_send![bundle, load];
                }
            }
        });
    }

    fn get_av_capture_device_class() -> Option<&'static Class> {
        ensure_avfoundation_loaded();
        let cls = Class::get("AVCaptureDevice");
        if cls.is_none() {
            tracing::warn!("AVCaptureDevice class not found");
        }
        cls
    }

    /// AVMediaTypeAudio constant
    fn av_media_type_audio() -> *mut Object {
        let ns_string_class = Class::get("NSString").expect("NSString class");
        unsafe { msg_send![ns_string_class, stringWithUTF8String: c"soun".as_ptr()] }
    }

    fn authorization_status_label(status: isize, denied_recheck_granted: bool) -> &'static str {
        match status {
            0 => "not_determined",
            1 => "restricted",
            2 if denied_recheck_granted => "authorized",
            2 => "denied",
            3 => "authorized",
            _ => "not_determined",
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum AccessRequestDecision {
        QuerySystem,
        Complete(bool),
    }

    fn access_request_decision(status: isize) -> AccessRequestDecision {
        match status {
            AV_AUTH_STATUS_NOT_DETERMINED | AV_AUTH_STATUS_DENIED => {
                AccessRequestDecision::QuerySystem
            }
            3 => AccessRequestDecision::Complete(true),
            _ => AccessRequestDecision::Complete(false),
        }
    }

    /// Return the raw authorization status as a string.
    /// AVCaptureDevice can cache `authorizationStatus` in-process, so when it
    /// reports "denied" we re-query via `requestAccess` which returns the
    /// current TCC state immediately (no dialog shown for already-determined
    /// states). This avoids opening a real audio stream as a side effect.
    pub fn authorization_status_string() -> String {
        let cls = match get_av_capture_device_class() {
            Some(c) => c,
            None => {
                tracing::warn!("AVCaptureDevice class not found — returning not_determined");
                return "not_determined".to_string();
            }
        };
        let media_type = av_media_type_audio();
        let status: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: media_type] };
        // AVCaptureDevice may cache "denied" after a System Settings change.
        // The raw request API returns the current TCC state without a new dialog
        // for an already-determined permission.
        let denied_recheck_granted = status == 2 && recheck_access_granted();
        authorization_status_label(status, denied_recheck_granted).to_string()
    }

    /// Re-query TCC permission via `requestAccessForMediaType:completionHandler:`.
    /// For already-determined states, this returns immediately without showing a dialog.
    /// This defeats AVCaptureDevice's in-process cache of `authorizationStatus`.
    fn recheck_access_granted() -> bool {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        request_access_from_system(move |granted| {
            let _ = tx.send(granted);
        });
        rx.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap_or(false)
    }

    /// Request microphone access using AVCaptureDevice.requestAccessForMediaType:completionHandler:.
    /// Calls `callback` once the user responds (or immediately if status is already determined).
    pub fn request_access<F: FnOnce(bool) + Send + 'static>(callback: F) {
        let cls = match get_av_capture_device_class() {
            Some(c) => c,
            None => {
                callback(false);
                return;
            }
        };
        let media_type = av_media_type_audio();

        let status: isize = unsafe { msg_send![cls, authorizationStatusForMediaType: media_type] };
        match access_request_decision(status) {
            AccessRequestDecision::QuerySystem => request_access_from_system(callback),
            AccessRequestDecision::Complete(granted) => callback(granted),
        }
    }

    /// Invoke AVFoundation's request API without consulting the potentially
    /// stale in-process `authorizationStatus` value first.
    fn request_access_from_system<F: FnOnce(bool) + Send + 'static>(callback: F) {
        let cls = match get_av_capture_device_class() {
            Some(c) => c,
            None => {
                callback(false);
                return;
            }
        };
        let media_type = av_media_type_audio();

        // Use AVCaptureDevice.requestAccessForMediaType:completionHandler: with an objc block.
        // This properly attributes the permission request to the app's bundle ID.
        let callback = std::sync::Mutex::new(Some(callback));
        let completion = block::ConcreteBlock::new(move |granted: bool| {
            if let Some(cb) = callback.lock().unwrap().take() {
                cb(granted);
            }
        });
        // The block must be copied to the heap for async use by the framework
        let completion = completion.copy();

        unsafe {
            let _: () = msg_send![cls, requestAccessForMediaType: media_type completionHandler: &*completion];
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{AccessRequestDecision, access_request_decision, authorization_status_label};

        #[test]
        fn maps_avfoundation_authorization_constants_correctly() {
            assert_eq!(authorization_status_label(0, false), "not_determined");
            assert_eq!(authorization_status_label(1, false), "restricted");
            assert_eq!(authorization_status_label(2, false), "denied");
            assert_eq!(authorization_status_label(3, false), "authorized");
        }

        #[test]
        fn denied_cache_can_be_refreshed_without_remapping_authorized() {
            assert_eq!(authorization_status_label(2, true), "authorized");
            assert_eq!(authorization_status_label(3, false), "authorized");
        }

        #[test]
        fn request_decision_live_queries_undetermined_and_cached_denied() {
            assert_eq!(access_request_decision(0), AccessRequestDecision::QuerySystem);
            assert_eq!(access_request_decision(2), AccessRequestDecision::QuerySystem);
        }

        #[test]
        fn request_decision_completes_known_restricted_and_authorized_states() {
            assert_eq!(
                access_request_decision(1),
                AccessRequestDecision::Complete(false)
            );
            assert_eq!(
                access_request_decision(3),
                AccessRequestDecision::Complete(true)
            );
        }
    }
}

#[tauri::command]
pub async fn get_platform() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        Ok("macos".to_string())
    }
    #[cfg(target_os = "windows")]
    {
        Ok("windows".to_string())
    }
    #[cfg(target_os = "linux")]
    {
        Ok("linux".to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Ok("unknown".to_string())
    }
}

#[derive(serde::Serialize)]
pub struct LoadedModelInfo {
    effective_model: String,
    effective_model_id: String,
    loaded_model: Option<String>,
    is_loaded: bool,
    is_downloaded: bool,
}
