use std::time::{Duration, Instant};

use tauri::Manager;
use tracing::{error, info, warn};

use crate::app_controller::{self, StopRecordingOutcome};
use crate::commands::{self, lock_controller, SharedController, SharedWhisper};
use crate::events::{self, EmitEvent, UiState};
use crate::launch::open_settings_window;
#[cfg(target_os = "macos")]
use crate::platform;
use crate::tray::{update_tray_last_result, update_tray_status};
use crate::{dispatch_to_main, focus_diag, overlay, paste_completion};

/// Maximum time to wait for whisper inference before aborting (seconds)
const TRANSCRIPTION_TIMEOUT_SECS: u64 = 60;

/// The text the hotkey path reports for a failed dictation, in the `error`
/// event and the controller's last error (pipeline_tests.rs pins it; ADR 0006).
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

/// Minimum recording duration before we allow stop (300ms)
const MIN_RECORDING_MS: u64 = 300;

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn elapsed_ms_if_success<T, E>(result: &Result<T, E>, start: Instant) -> Option<u64> {
    result.as_ref().ok().map(|_| elapsed_ms(start))
}

/// Stop recording, enforce minimum duration, and spawn transcription.
/// Shared by both push-to-talk (on key-up) and toggle (on second key-down).
pub(super) fn stop_recording_and_transcribe(
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

/// How long the overlay keeps its error state visible after a failed dictation.
const OVERLAY_ERROR_VISIBLE: Duration = Duration::from_millis(2500);

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

#[cfg(test)]
#[path = "pipeline_tests.rs"]
mod tests;
