use std::time::{Duration, Instant};

use tokio::sync::oneshot;
use tracing::{error, info, warn};

use super::inference_wait::{wait_for_inference, WaitOutcome};
use super::runtime::{DesktopRuntime, DictationRuntime, InferenceRequest, UiAction};

#[path = "session.rs"]
mod session;
use crate::app_controller::{self, StopRecordingOutcome};
use crate::command_error::CommandError;
use crate::commands;
use crate::events::{self, UiState};
#[cfg(target_os = "macos")]
use crate::focus_diag;
use crate::paste_completion;
#[cfg(target_os = "macos")]
use crate::platform;
use session::{LiveSession, TerminalOutcome};

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// The text the hotkey path reports for a failed dictation, in the `error`
/// event and the controller's last error (pipeline_tests.rs pins it; ADR 0006).
fn dictation_failure_text(error: &sagascript_core::error::DictationError) -> String {
    error.to_string()
}

/// The `error` event of a failed dictation: the error's own code and retry hint
/// and the text of [`dictation_failure_text`], without absolute paths
/// ([`CommandError::message`]).
fn dictation_failure_event(error: &sagascript_core::error::DictationError) -> CommandError {
    CommandError::from(error.clone())
}

/// How the hotkey path ends a dictation whose transcription returned an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TranscriptionEnd {
    /// Stopped on request (`cancelled`): back to idle, nothing to show. Not a
    /// failure (ADR 0006).
    Quiet,
    /// A failure: the error event, the overlay's error state, the controller's
    /// last error.
    Failed,
}

fn transcription_end(error: &sagascript_core::error::DictationError) -> TranscriptionEnd {
    match error {
        sagascript_core::error::DictationError::Cancelled => TranscriptionEnd::Quiet,
        _ => TranscriptionEnd::Failed,
    }
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

/// Minimum recording duration before we allow stop (300ms)
const MIN_RECORDING_MS: u64 = 300;

fn elapsed_ms(start: Instant, now: Instant) -> u64 {
    u64::try_from(now.saturating_duration_since(start).as_millis()).unwrap_or(u64::MAX)
}

fn elapsed_ms_if_success<T, E>(result: &Result<T, E>, start: Instant, now: Instant) -> Option<u64> {
    result.as_ref().ok().map(|_| elapsed_ms(start, now))
}

/// Stop recording, enforce minimum duration, and spawn transcription.
/// Shared by both push-to-talk (on key-up) and toggle (on second key-down).
pub(super) fn stop_recording_and_transcribe(app: &tauri::AppHandle) {
    request_stop(DesktopRuntime(app.clone()));
}

pub(super) fn request_stop(runtime: impl DictationRuntime) {
    let key_up_at = runtime.now();
    runtime.focus_log("key_up_stop_requested", serde_json::json!({}));

    // Compute how long we still need to hold to satisfy the minimum recording
    // duration — but do NOT block the global-shortcut (UI) thread waiting for it
    // (finding 2): a std::thread::sleep here freezes UI redraw and stalls
    // subsequent hotkey events. The delay is offloaded to an async task below.
    let (elapsed, generation) = {
        let mut c = runtime.controller();
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

    let task_runtime = runtime.clone();
    runtime.spawn(async move {
        run(task_runtime, remaining, generation, key_up_at).await;
    });
}

fn dispatch_ui(runtime: &impl DictationRuntime, generation: u64, action: UiAction) {
    runtime.dispatch(move |runtime| {
        // Native dispatch may run after the pipeline has finished or another
        // recording/meeting has started. Check at execution, not at enqueue.
        let current = {
            let c = runtime.controller();
            let expected_state = match &action {
                UiAction::Tray("idle") | UiAction::Finish { .. } | UiAction::HideOverlay => {
                    app_controller::AppState::Idle
                }
                UiAction::Tray(_) => app_controller::AppState::Transcribing,
            };
            c.recording_generation() == generation
                && c.state() == expected_state
                && (expected_state != app_controller::AppState::Transcribing
                    || c.active_hotkey_profile().is_some())
        };
        if current {
            runtime.apply_ui(action);
        }
    });
}

fn start_paste(
    runtime: &impl DictationRuntime,
    generation: u64,
    text: String,
) -> Result<oneshot::Receiver<Result<(), String>>, String> {
    let (sender, receiver) = oneshot::channel();
    runtime.dispatch_paste(move |runtime| {
        // A callback that has not begun is discardable after completion or a
        // new session. Once native paste begins it may still finish late; keep
        // the existing timeout warning and manual-recovery behavior.
        let current = {
            let c = runtime.controller();
            c.recording_generation() == generation
                && c.state() == app_controller::AppState::Transcribing
                && c.active_hotkey_profile().is_some()
        };
        if !current || sender.is_closed() {
            return;
        }
        let _ = sender.send(runtime.paste(&text));
    })?;
    Ok(receiver)
}

async fn run(
    runtime: impl DictationRuntime,
    remaining: Option<Duration>,
    generation: u64,
    key_up_at: Instant,
) {
    let session = LiveSession::new(generation);
    // Min-duration top-up delay, offloaded so the UI thread stays responsive.
    if let Some(rem) = remaining {
        runtime.sleep(rem).await;
    }

    // Stop recording (single lock acquisition, re-acquired here since the
    // controller State can't be moved into the task). Guarded so a stop that
    // races an already-stopped session is a no-op, and a capture/resample
    // failure surfaces as a real error (findings 3 & 4).
    let outcome = {
        let mut c = runtime.controller();
        c.stop_recording_generation(generation)
    };
    let audio = match outcome {
        StopRecordingOutcome::NotRecording => return,
        StopRecordingOutcome::Failed(failure) => {
            let msg = failure.to_string();
            error!("Recording stop failed: {msg}");
            session.finish(&runtime, TerminalOutcome::CaptureFailed(failure), None);
            return;
        }
        StopRecordingOutcome::Stopped(audio) => {
            runtime.focus_log(
                    "capture_stopped",
                    serde_json::json!({ "keyUpToCaptureStoppedMs": elapsed_ms(key_up_at, runtime.now()) }),
                );
            audio
        }
    };
    let key_up_to_capture_stopped_ms = elapsed_ms(key_up_at, runtime.now());

    // Keep the recording indicator visible through processing, so key-up
    // never leaves the user without feedback while inference/paste runs.
    // Show the transcribing state — re-dispatched to the main
    // thread now that this runs on a worker.
    dispatch_ui(&runtime, generation, UiAction::Tray("transcribing"));
    let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::Transcribing);

    if audio.is_empty() {
        session.finish(
            &runtime,
            TerminalOutcome::EmptyAudio,
            Some(serde_json::json!({
                "outcome": "no_speech",
                "keyUpToCaptureStoppedMs": key_up_to_capture_stopped_ms,
                "keyUpToModelReadyMs": null,
                "modelLoadMs": null,
                "whisperMs": null,
                "keyUpToPasteCompletedMs": null,
                "totalMs": elapsed_ms(key_up_at, runtime.now()),
            })),
        );
        return;
    }

    // Select the live engine off the UI thread. Both engines share the
    // bounded transaction and postprocessing/paste path below.

    // Extract what we need for transcription (lock briefly)
    let selection = {
        let c = runtime.controller();
        if !session.is_processing(&c) {
            return;
        }
        let profile_id = c
            .active_hotkey_profile()
            .map(|profile| profile.id.clone())
            .unwrap_or_else(|| c.settings().default_profile().id);
        c.settings()
            .live_model_for_profile(&profile_id)
            .map(|(effective_model, use_pianissimo)| {
                (
                    c.language(),
                    effective_model,
                    commands::build_transcribe_options_for_profile(c.settings(), Some(&profile_id)),
                    sagascript_core::transcription::Glossary::parse(
                        &c.settings().effective_glossary_source(Some(&profile_id)),
                    ),
                    use_pianissimo,
                )
            })
    };
    let (language, effective_model, opts, glossary, use_pianissimo) = match selection {
        Ok(selection) => selection,
        Err(refusal) => {
            let error = refusal.to_string();
            error!("Cannot resolve profile model: {error}");
            session.finish(
                &runtime,
                TerminalOutcome::ModelSelectionFailed(refusal),
                None,
            );
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
    let mut model_was_warm = runtime.model_is_warm(effective_model, use_pianissimo);
    info!(
        key_up_to_transcribe_ms = elapsed_ms(key_up_at, runtime.now()),
        model_was_warm, "Transcribing with model: {model_name}"
    );

    // Show model loading status in tray
    if !model_was_warm {
        let _ = runtime.emit_event(events::event::STATE_CHANGED, UiState::LoadingModel);
        dispatch_ui(&runtime, generation, UiAction::Tray("loading_model"));
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
        let inference = runtime.start_inference(InferenceRequest {
            model: effective_model,
            audio,
            language,
            options: opts,
            use_pianissimo,
        });
        let mut fut = inference.task;

        match wait_for_inference(
                &mut fut,
                Duration::from_secs(TRANSCRIPTION_TIMEOUT_SECS),
                Duration::from_secs(ABORT_GRACE_SECS),
                || {
                    warn!("Transcription timed out after {TRANSCRIPTION_TIMEOUT_SECS}s — requesting abort");
                    (inference.abort)();
                },
            )
            .await
            {
                WaitOutcome::Completed(Ok((r, timings))) => {
                    let mut c = runtime.controller();
                    if !session.is_processing(&c) { return; }
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
                WaitOutcome::Completed(Err(e)) => Err(dictation_task_failed(e)),
                WaitOutcome::TimedOut { unwound } => {
                    // Report whether the timeout-triggered abort unwound within
                    // the grace period so a genuine hang is visible.
                    if unwound {
                        info!("Aborted transcription task exited — warm-state lock released")
                    } else {
                        error!(
                            "Transcription task still running {ABORT_GRACE_SECS}s after abort — \
                             warm state may stay locked until it unwinds; further transcriptions \
                             will report ModelBusy rather than block forever"
                        );
                    }
                    Err(dictation_timed_out())
                }
            }
    };

    if !session.is_processing(&runtime.controller()) {
        return;
    }
    let key_up_to_whisper_complete_ms =
        whisper_ms.and_then(|_| elapsed_ms_if_success(&result, key_up_at, runtime.now()));
    let postprocess_started = runtime.now();
    let result = result.map(|text| commands::apply_glossary(text, &glossary));
    runtime.controller().record_phase(
        "postprocessing",
        runtime.now().saturating_duration_since(postprocess_started),
    );
    match result {
        Ok(text) => {
            info!("Transcription complete: {} chars", text.len());
            runtime.focus_log(
                "transcription_result_ready",
                serde_json::json!({ "keyUpToResultMs": elapsed_ms(key_up_at, runtime.now()) }),
            );

            if text.trim().is_empty() {
                session.finish(
                    &runtime,
                    TerminalOutcome::NoSpeech,
                    Some(serde_json::json!({
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
                        "totalMs": elapsed_ms(key_up_at, runtime.now()),
                    })),
                );
                info!("No speech detected; returning to idle without paste");
                return;
            }

            // Check if auto-paste is enabled (lock briefly)
            let should_paste = {
                let c = runtime.controller();
                c.settings().auto_paste
            };

            let mut paste_outcome = "disabled";
            let mut key_up_to_paste_completed_ms = None;
            let mut paste_error = None;
            if should_paste {
                // Auto-paste MUST run on the main thread — enigo's macOS TIS APIs
                // crash (SIGABRT) if called from a tokio worker thread.
                let text_for_paste = text.clone();
                let paste_started = runtime.now();
                runtime.restore_paste_target().await;
                if !session.is_processing(&runtime.controller()) {
                    return;
                }
                match start_paste(&runtime, generation, text_for_paste) {
                    Err(e) => {
                        error!("Failed to dispatch paste to main thread: {e}");
                        paste_outcome = "dispatch_failed";
                        paste_error = Some("Could not dispatch automatic paste. Copy the recognized text from Dictate.".to_string());
                    }
                    Ok(paste_rx) => {
                        let completion = paste_completion::wait(
                            paste_rx,
                            Duration::from_millis(PASTE_COMPLETION_TIMEOUT_MS),
                        )
                        .await;
                        paste_outcome = completion.outcome;
                        paste_error = completion.error;
                        info!(
                            paste_ms = runtime
                                .now()
                                .saturating_duration_since(paste_started)
                                .as_millis() as u64,
                            key_up_to_paste_ms = elapsed_ms(key_up_at, runtime.now()),
                            outcome = completion.outcome,
                            "Dictation paste finished"
                        );
                        if completion.call_completed {
                            key_up_to_paste_completed_ms =
                                Some(elapsed_ms(key_up_at, runtime.now()));
                        } else if paste_outcome == "timed_out" {
                            warn!("Auto-paste completion timed out after {PASTE_COMPLETION_TIMEOUT_MS}ms");
                        }
                    }
                }
                if !session.is_processing(&runtime.controller()) {
                    return;
                }
                runtime.controller().record_phase(
                    "clipboard_focus_paste",
                    runtime.now().saturating_duration_since(paste_started),
                );
            }

            session.finish(
                &runtime,
                TerminalOutcome::Recognized {
                    text,
                    paste_outcome,
                    paste_error,
                },
                Some(serde_json::json!({
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
                    "totalMs": elapsed_ms(key_up_at, runtime.now()),
                })),
            );
            info!("Transcription flow complete, app should remain running");
        }
        Err(e) if transcription_end(&e) == TranscriptionEnd::Quiet => {
            // Stopped on request: not a failure. Back to idle like a
            // dictation without speech; no error event, no error state, no
            // last error. No latency record either: the stopped run is no
            // completed dictation (`latency-report` knows three outcomes).
            info!("Transcription cancelled: {e}");
            session.finish(&runtime, TerminalOutcome::Cancelled, None);
            info!("Cancelled flow complete; back to idle");
        }
        Err(e) => {
            error!("Transcription failed: {e}");
            session.finish(
                &runtime,
                TerminalOutcome::Failed(e),
                Some(serde_json::json!({
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
                    "totalMs": elapsed_ms(key_up_at, runtime.now()),
                })),
            );
            info!("Error flow complete, app should remain running");
        }
    }
}

/// Issue #256: a deferred activation can leave Sagascript frontmost at key-up.
/// Re-activate the dictation target before the paste guard runs. The wait runs
/// on a blocking thread; each AppKit call is dispatched to the main thread so
/// its event loop keeps running and the activation can actually land. Never
/// pastes: the guard still decides afterwards.
#[cfg(target_os = "macos")]
pub(super) async fn restore_target_before_paste(app: &tauri::AppHandle) {
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

/// How long the overlay keeps its error state visible after a failed dictation.
const OVERLAY_ERROR_VISIBLE: Duration = Duration::from_millis(2500);

/// Hide the overlay after its error state has been readable, unless a new
/// dictation started in the meantime.
fn hide_overlay_after_error(runtime: impl DictationRuntime, generation: u64) {
    let task_runtime = runtime.clone();
    runtime.spawn(async move {
        task_runtime.sleep(OVERLAY_ERROR_VISIBLE).await;
        let idle = matches!(
            task_runtime.controller().state(),
            app_controller::AppState::Idle
        );
        if idle {
            dispatch_ui(&task_runtime, generation, UiAction::HideOverlay);
        }
    });
}

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pipeline_lifecycle_tests.rs"]
mod lifecycle_tests;
