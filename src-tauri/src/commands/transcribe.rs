//! Live dictation from the GUI and the Teach tab: start, stop, cancel, and the
//! decoder options built from the settings.

use std::sync::Arc;
use std::time::Duration;

use sagascript_core::audio::decoder;
use sagascript_core::error::DictationError;
use sagascript_core::settings::Settings;
use sagascript_core::transcription::{model, ContextProfile, Glossary, TranscribeOptions};
use tauri::State;
use tracing::{error, info, warn};

use super::{ensure_known_profile, lock_controller, SharedController, SharedWhisper};
use crate::app_controller::StopRecordingOutcome;
use crate::command_error::CommandError;
use crate::events::{EmitEvent, UiState};

/// Maximum time to wait for whisper inference before aborting (seconds)
pub(super) const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// After a timeout fires and we request an abort, how long to wait for the
/// blocking inference to actually unwind and release the warm-state lock before
/// logging that it is still stuck. The real abort callback returns within a
/// compute step or two, so this rarely elapses.
pub(super) const ABORT_GRACE_SECS: u64 = 5;

/// Build the per-transcription options from the current settings. Resolves the
/// VAD model path only when VAD is enabled and the model is present (otherwise
/// VAD is silently skipped — whisper would fail on a missing model).
pub(crate) fn build_transcribe_options(settings: &Settings) -> TranscribeOptions {
    build_transcribe_options_for_profile(settings, None)
}

pub(crate) fn build_transcribe_options_for_profile(
    settings: &Settings,
    profile_id: Option<&str>,
) -> TranscribeOptions {
    let prompt = Glossary::parse(&settings.effective_glossary_source(profile_id)).decoder_prompt();
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
        prompt,
        beam_size: settings.beam_size,
        temperature_fallback: settings.temperature_fallback,
        vad_model_path,
        segment_timestamps: false,
        parallel_chunks: 1,
    }
}

pub(crate) fn apply_glossary(text: String, glossary: &Glossary) -> String {
    let (corrected, corrections) = glossary.correct_text(&text);
    if !corrections.is_empty() {
        info!(
            correction_count = corrections.len(),
            "Applied personal glossary corrections"
        );
    }
    corrected
}

#[tauri::command]
pub async fn start_recording(controller: State<'_, SharedController>) -> Result<(), CommandError> {
    let mut ctrl = lock_controller(&controller);
    gui_start_recording_result(ctrl.start_recording())
}

#[tauri::command]
pub async fn start_training_recording(
    controller: State<'_, SharedController>,
    profile_id: String,
) -> Result<(), CommandError> {
    let mut ctrl = lock_controller(&controller);
    ensure_known_profile(ctrl.settings(), &profile_id)?;
    let profile = ctrl
        .settings()
        .resolved_hotkey_profiles()
        .into_iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| format!("Unknown dictation profile '{profile_id}'"))?;
    gui_start_recording_result(ctrl.start_training_recording_for_profile(profile))
}

fn gui_start_recording_result(result: Result<bool, DictationError>) -> Result<(), CommandError> {
    match result {
        Ok(true) => Ok(()),
        Ok(false) => Err(CommandError::Busy(
            "Cannot start recording while Sagascript is busy. Stop the current recording or wait for transcription to finish."
                .to_string(),
        )),
        Err(error) => Err(error.into()),
    }
}

#[tauri::command]
pub async fn stop_and_transcribe(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<String, CommandError> {
    stop_and_transcribe_impl(controller, whisper, true)
        .await
        .map(|transcript| transcript.effective_text)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TrainingTranscript {
    pub raw_text: String,
    pub effective_text: String,
}

#[tauri::command]
pub async fn stop_and_transcribe_training(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
) -> Result<TrainingTranscript, CommandError> {
    stop_and_transcribe_impl(controller, whisper, false).await
}

fn not_recording_result(allow_empty: bool) -> Result<TrainingTranscript, CommandError> {
    if allow_empty {
        Ok(TrainingTranscript {
            raw_text: String::new(),
            effective_text: String::new(),
        })
    } else {
        Err(CommandError::NotRecording)
    }
}

async fn stop_and_transcribe_impl(
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    allow_empty_not_recording: bool,
) -> Result<TrainingTranscript, CommandError> {
    let (audio, language, effective_model, opts, glossary, use_pianissimo) = {
        let mut ctrl = lock_controller(&controller);
        // A late/duplicate normal stop racing the hotkey path remains an
        // Ok-empty no-op so it cannot clobber an in-flight transcription.
        // Teach owns its recording lifecycle, so a missing recording there is
        // an actionable UI error instead of a silently accepted empty result.
        let audio = match ctrl.stop_recording_guarded() {
            StopRecordingOutcome::NotRecording => {
                return not_recording_result(allow_empty_not_recording)
            }
            // Capture/resample failure (finding 4): the controller already
            // recorded the error and returned to Idle; surface the real error.
            StopRecordingOutcome::Failed(error) => return Err(error.into()),
            StopRecordingOutcome::Stopped(audio) => audio,
        };
        let language = ctrl.language();
        let profile_id = ctrl
            .active_hotkey_profile()
            .map(|profile| profile.id.clone())
            .unwrap_or_else(|| ctrl.settings().default_profile().id);
        let (effective_model, use_pianissimo) =
            match ctrl.settings().live_model_for_profile(&profile_id) {
                Ok(choice) => choice,
                Err(error) => {
                    let completion = ctrl.finish_transcription(Err(error.clone()));
                    return Err(completion.err().unwrap_or(error).into());
                }
            };
        let opts = build_transcribe_options_for_profile(ctrl.settings(), Some(&profile_id));
        let glossary =
            Glossary::parse(&ctrl.settings().effective_glossary_source(Some(&profile_id)));
        (
            audio,
            language,
            effective_model,
            opts,
            glossary,
            use_pianissimo,
        )
    };

    if audio.is_empty() {
        let error = DictationError::NoAudioCaptured;
        // The controller records the text and returns to Idle; the caller gets
        // the typed error.
        let _ = lock_controller(&controller).finish_transcription(Err(error.to_string()));
        return Err(error.into());
    }

    // Every outcome after recording stops must flow through
    // `finish_transcription`: stop_recording_guarded has already moved the
    // controller to Transcribing, so returning early would wedge subsequent
    // recording attempts until the app restarts.
    let result = {
        // Run blocking transcription on a separate thread with a timeout. On timeout
        // we now trigger a REAL abort (the whisper-rs abort callback wired in
        // WhisperBackend): request_abort() flips the flag whisper.cpp checks between
        // compute steps, so the blocking task returns promptly and releases the warm
        // state instead of running to completion and wedging the pipeline. The handle
        // is kept borrowed (`&mut fut`) across the timeout so we can await its actual
        // exit after abort and log whether the lock was released.
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
            Ok(Ok((result, timings))) => {
                let mut ctrl = lock_controller(&controller);
                if timings.model_acquisition_started {
                    ctrl.record_phase(
                        "model_acquisition",
                        Duration::from_secs_f64(timings.model_ms / 1000.0),
                    );
                    ctrl.record_model_cache(timings.model_cached);
                }
                if timings.inference_started {
                    ctrl.record_phase(
                        "inference",
                        Duration::from_secs_f64(timings.inference_ms / 1000.0),
                    );
                }
                result.map_err(CommandError::from)
            }
            Ok(Err(error)) => Err(DictationError::TaskFailed(format!(
                "Transcription task failed: {error}"
            ))
            .into()),
            Err(_) => {
                warn!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s — requesting abort");
                live_backend.request_abort();
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
                Err(DictationError::TranscriptionTimedOut(format!(
                    "Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s (inference aborted)"
                ))
                .into())
            }
        }
    };

    let training_result = result.map(|raw_text| TrainingTranscript {
        effective_text: apply_glossary(raw_text.clone(), &glossary),
        raw_text,
    });

    // NOTE: auto-paste is NOT done here — enigo's macOS TIS APIs crash if
    // called from a tokio worker thread (SIGTRAP in dispatch_assert_queue).
    // The hotkey path in main.rs handles paste via run_on_main_thread(). This
    // command returns the text to the frontend for display instead.
    let completion = training_result
        .as_ref()
        .map(|transcript| transcript.effective_text.clone())
        .map_err(ToString::to_string);
    // The controller keeps the text for `get_last_error`; the caller gets the
    // typed error (finish_transcription only echoes what it was given).
    let _ = lock_controller(&controller).finish_transcription(completion);
    training_result
}

#[tauri::command]
pub async fn transcribe_training_file(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    activity: State<'_, Arc<crate::update_activity::UpdateActivity>>,
    file_path: String,
    profile_id: String,
) -> Result<TrainingTranscript, CommandError> {
    let _work_lease = activity.begin_work()?;

    let path = std::path::PathBuf::from(file_path);
    let audio = tokio::task::spawn_blocking(move || decoder::decode_audio_file(&path))
        .await
        .map_err(|error| DictationError::TaskFailed(format!("Decode task failed: {error}")))??;
    if audio.is_empty() {
        return Err(DictationError::FileDecodeError("No audio decoded from file".into()).into());
    }

    let (language, effective_model, opts, glossary, use_pianissimo) = {
        let ctrl = lock_controller(&controller);
        ensure_known_profile(ctrl.settings(), &profile_id)?;
        let profile = ctrl
            .settings()
            .resolved_hotkey_profiles()
            .into_iter()
            .find(|profile| profile.id == profile_id)
            .expect("validated profile must exist");
        let (effective_model, use_pianissimo) =
            ctrl.settings().live_model_for_profile(&profile_id)?;
        (
            profile.language,
            effective_model,
            build_transcribe_options_for_profile(ctrl.settings(), Some(&profile_id)),
            Glossary::parse(&ctrl.settings().effective_glossary_source(Some(&profile_id))),
            use_pianissimo,
        )
    };

    if use_pianissimo || whisper.needs_reload(effective_model) {
        let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::LoadingModel);
    }
    let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Transcribing);

    let live_backend = std::sync::Arc::new(
        sagascript_core::transcription::live_dictation::LiveDictationBackend::new(
            whisper.inner().clone(),
            use_pianissimo,
        ),
    );
    let live_ref = live_backend.clone();
    let whisper_ref = whisper.inner().clone();
    let progress_app = app.clone();
    let duration_seconds = (audio.len() / 16_000) as u64;
    let timeout = Duration::from_secs((duration_seconds * 6).max(TRANSCRIPTION_TIMEOUT_SECS));
    let mut task = tokio::task::spawn_blocking(move || {
        if use_pianissimo {
            return live_ref.transcribe(
                effective_model,
                &audio,
                language,
                &opts,
                &mut Default::default(),
            );
        }
        whisper_ref.with_model(effective_model, ContextProfile::FlashAttention, |backend| {
            backend.transcribe_sync_with_options(
                &audio,
                language,
                &opts,
                move |progress| {
                    let _ = progress_app
                        .emit_event(crate::events::event::TRANSCRIPTION_PROGRESS, progress);
                },
                None,
            )
        })
    });

    let result = match tokio::time::timeout(timeout, &mut task).await {
        Ok(Ok(result)) => result.map_err(CommandError::from),
        Ok(Err(error)) => {
            Err(DictationError::TaskFailed(format!("Transcription task failed: {error}")).into())
        }
        Err(_) => {
            live_backend.request_abort();
            let _ = tokio::time::timeout(Duration::from_secs(ABORT_GRACE_SECS), &mut task).await;
            Err(DictationError::TranscriptionTimedOut(format!(
                "Training transcription timed out after {}s",
                timeout.as_secs()
            ))
            .into())
        }
    };
    let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);

    result.map(|raw_text| TrainingTranscript {
        effective_text: apply_glossary(raw_text.clone(), &glossary),
        raw_text,
    })
}

#[tauri::command]
pub async fn cancel_recording(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
) -> Result<(), String> {
    let mut ctrl = lock_controller(&controller);
    ctrl.cancel_recording();
    drop(ctrl);
    crate::overlay::hide(&app);
    Ok(())
}

#[cfg(test)]
#[path = "transcribe_tests.rs"]
mod tests;
