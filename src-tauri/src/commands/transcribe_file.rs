//! The `transcribe_file` command (plain and legacy diarized runs) and its
//! cancellation.

use std::sync::Arc;
use std::time::Duration;

use sagascript_core::audio::decoder;
use sagascript_core::error::DictationError;
use sagascript_core::settings::FileModel;
use sagascript_core::transcription::whisper_backend::display_transcript;
use sagascript_core::transcription::{
    recommended_parallel_chunks, ContextProfile, Glossary, TranscribeRequest,
};
use tauri::State;
use tracing::{error, info, warn};

use super::{
    apply_glossary, file_transcription_context, file_transcription_language, lock_controller,
    FileTranscriptionContext, SharedController, SharedWhisper, ABORT_GRACE_SECS,
    TRANSCRIPTION_TIMEOUT_SECS,
};
use crate::command_error::CommandError;
use crate::events::{EmitEvent, PlainPhase, UiState};

/// User-requested abort for the plain (non-diarized) `transcribe_file` path
/// (#234). Only the identified run's exclusively leased backend is signalled.
/// Stale or idle requests return false without touching live dictation.
#[tauri::command]
pub async fn cancel_file_transcription(
    jobs: State<'_, crate::plain_file_jobs::SharedPlainFileJobs>,
    run_id: String,
) -> Result<bool, String> {
    Ok(jobs.cancel(&run_id))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects AppHandle + States; the
                                     // frontend call shape (file/prompt/diarize/profile) is unchanged.
pub async fn transcribe_file(
    app: tauri::AppHandle,
    controller: State<'_, SharedController>,
    whisper: State<'_, SharedWhisper>,
    jobs: State<'_, crate::plain_file_jobs::SharedPlainFileJobs>,
    activity: State<'_, Arc<crate::update_activity::UpdateActivity>>,
    file_path: String,
    prompt: Option<String>,
    diarize: Option<bool>,
    profile_id: Option<String>,
    run_id: Option<String>,
    auto_paste: Option<bool>,
) -> Result<String, CommandError> {
    let activity = activity.inner().clone();
    let _work_lease = activity.begin_work()?;
    let result_id = run_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let pianissimo_glossary = {
        let ctrl = lock_controller(&controller);
        let settings = ctrl.settings();
        let language = file_transcription_language(settings, profile_id.as_deref())?;
        // Diarization needs Whisper, so an Auto preference must not pick Pianissimo for it.
        let resolved = if diarize.unwrap_or(false) {
            settings.effective_whisper_file_model_for(language)?
        } else {
            settings.effective_file_model_for(language)?
        };
        match resolved {
            FileModel::PianissimoOriginal => Some(Glossary::parse(
                &settings.effective_glossary_source_with_prompt(
                    profile_id.as_deref(),
                    prompt.as_deref(),
                ),
            )),
            FileModel::Whisper(_) => None,
        }
    };
    if let Some(glossary) = pianissimo_glossary {
        if diarize.unwrap_or(false) {
            return Err("Pianissimo supports plain Swedish file transcription only; choose a Whisper file model for diarization".into());
        }
        let text = crate::plain_file_jobs::transcribe_pianissimo(
            app.clone(),
            jobs.inner().clone(),
            result_id.clone(),
            file_path,
            glossary,
        )
        .await?;
        activity.set_result_pending(&result_id, true)?;
        if auto_paste.unwrap_or(true) && lock_controller(&controller).settings().auto_paste {
            let paste_text = text.clone();
            app.run_on_main_thread(move || {
                if let Err(error) = crate::paste::PasteService::new().paste(&paste_text) {
                    error!(%error, "Auto-paste failed");
                }
            })
            .map_err(|error| format!("Could not dispatch auto-paste: {error}"))?;
        }
        return Ok(text);
    }
    let context = {
        let ctrl = lock_controller(&controller);
        file_transcription_context(ctrl.settings(), profile_id.as_deref(), prompt.as_deref())?
    };

    if !diarize.unwrap_or(false) {
        let text = crate::plain_file_jobs::transcribe(
            app.clone(),
            jobs.inner().clone(),
            result_id.clone(),
            file_path,
            context,
        )
        .await?;
        activity.set_result_pending(&result_id, true)?;
        if auto_paste.unwrap_or(true) && lock_controller(&controller).settings().auto_paste {
            let paste_text = text.clone();
            app.run_on_main_thread(move || {
                if let Err(error) = crate::paste::PasteService::new().paste(&paste_text) {
                    error!(%error, "Auto-paste failed");
                }
            })
            .map_err(|error| format!("Could not dispatch auto-paste: {error}"))?;
        }
        return Ok(text);
    }
    let FileTranscriptionContext {
        language,
        model: effective_model,
        glossary,
        options: mut opts,
    } = context;

    let path = std::path::PathBuf::from(&file_path);

    // Suppress unused-variable warning on `diarize` when the diarization feature is off
    #[cfg(not(feature = "diarization"))]
    let _ = &diarize;

    #[cfg(feature = "diarization")]
    let context_profile = ContextProfile::for_diarization(diarize.unwrap_or(false));
    #[cfg(not(feature = "diarization"))]
    let context_profile = ContextProfile::FlashAttention;

    // Legacy diarized command path. The GUI uses the meeting-job API instead.
    let audio = tokio::task::spawn_blocking(move || decoder::decode_audio_file(&path))
        .await
        .map_err(|error| DictationError::TaskFailed(format!("Decode task failed: {error}")))??;

    if audio.is_empty() {
        return Err(DictationError::FileDecodeError("No audio decoded from file".into()).into());
    }

    // File transcription (beam search / diarization) is far slower than live
    // dictation, so scale the timeout by the decoded duration rather than using
    // the short live-dictation timeout (which beam search could otherwise hit).
    let file_timeout =
        Duration::from_secs(((audio.len() / 16_000) as u64 * 6).max(TRANSCRIPTION_TIMEOUT_SECS));

    // Show model loading status if the exact model/profile runtime is not warm.
    if whisper.needs_reload_with_profile(effective_model, context_profile) {
        let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::LoadingModel);
        let _ = app.emit_event(
            crate::events::event::TRANSCRIPTION_PHASE,
            PlainPhase::Loading,
        );
    }

    // Diarization path — runs both diarization and timestamped transcription in parallel,
    // then merges and consolidates speaker-attributed segments.
    #[cfg(feature = "diarization")]
    if diarize.unwrap_or(false) {
        use sagascript_core::diarization::{
            diarize as run_diarize,
            merge::{consolidate, merge_with_transcript},
            model::{download_model as download_diarization_model, DiarizationModel},
            DiarizeConfig, TimestampedSegment,
        };

        // Checking diarization in the file-transcription UI is an explicit
        // action: verify existing app-managed artifacts and repair only exact
        // integrity mismatches before native ONNX parsing. This never runs as
        // a silent startup download.
        for diarization_model in DiarizationModel::ALL {
            if let Err(error) = download_diarization_model(*diarization_model, |_, _| {}).await {
                let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);
                return Err(error.into());
            }
        }

        let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Transcribing);

        let whisper_ref = whisper.inner().clone();
        // Profile-scoped dictionary hints are already frozen in the glossary;
        // an empty one-run prompt leaves that profile source intact.
        let prompt_ref = glossary.decoder_prompt();
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
            whisper_ref.with_model(effective_model, ContextProfile::TokenAlignment, |backend| {
                backend.transcribe_sync_for_diarization(
                    &audio_for_transcribe,
                    language,
                    prompt_ref.as_deref(),
                )
            })
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
                let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);
                return Err(e.into());
            }
            Ok((Err(e), _)) | Ok((_, Err(e))) => {
                let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);
                return Err(DictationError::TaskFailed(format!("Task join error: {e}")).into());
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
                let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);
                return Err(DictationError::TranscriptionTimedOut(format!(
                    "Transcription timed out after {}s (inference aborted)",
                    timeout.as_secs()
                ))
                .into());
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
        let text = apply_glossary(text, &glossary);
        activity.set_result_pending(&result_id, true)?;

        info!("Diarized file transcription complete: {} chars", text.len());

        let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);

        // Auto-paste if enabled
        let should_paste = {
            let c = lock_controller(&controller);
            auto_paste.unwrap_or(true) && c.settings().auto_paste
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
    opts.parallel_chunks =
        recommended_parallel_chunks(audio.len(), effective_model, opts.beam_size);
    let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Transcribing);
    // Leave 0% immediately (#237): model load inside the blocking task is
    // silent, so report "started" now; inference re-reports 1% itself.
    let _ = app.emit_event(crate::events::event::TRANSCRIPTION_PROGRESS, 1);
    let whisper_ref = whisper.inner().clone();
    let app_progress = app.clone();
    // Borrowed handle (`&mut fut`) so the timeout path can await the task's
    // actual exit after requesting an abort — mirrors the live dictation path.
    let mut fut = tokio::task::spawn_blocking(move || {
        // State prep (lock, chunk states) is silent work before the first
        // progress callback — name it so the UI never shows a frozen 1%.
        let phase = if whisper_ref.needs_reload_with_profile(effective_model, context_profile) {
            PlainPhase::Loading
        } else {
            PlainPhase::Preparing
        };
        let _ = app_progress.emit_event(crate::events::event::TRANSCRIPTION_PHASE, phase);
        whisper_ref.with_model(effective_model, context_profile, |backend| {
            let _ = app_progress.emit_event(
                crate::events::event::TRANSCRIPTION_PHASE,
                PlainPhase::Preparing,
            );
            let encode_app = app_progress.clone();
            let on_encode_start = move || {
                let _ = encode_app.emit_event(
                    crate::events::event::TRANSCRIPTION_PHASE,
                    PlainPhase::Encoding,
                );
            };
            let segments = backend.transcribe_with_gap_recovery(
                TranscribeRequest::new(&audio, language, &opts)
                    .on_progress(move |pct| {
                        let _ = app_progress
                            .emit_event(crate::events::event::TRANSCRIPTION_PROGRESS, pct);
                    })
                    .on_encode_start(&on_encode_start),
            )?;
            Ok(display_transcript(&segments, language))
        })
    });

    let timeout = file_timeout;
    let result = match tokio::time::timeout(timeout, &mut fut).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);
            return Err(
                DictationError::TaskFailed(format!("Transcription task failed: {e}")).into(),
            );
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
            let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);
            return Err(DictationError::TranscriptionTimedOut(format!(
                "Transcription timed out after {}s (inference aborted)",
                timeout.as_secs()
            ))
            .into());
        }
    };

    let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);

    match result {
        Ok(text) => {
            let text = apply_glossary(text, &glossary);
            activity.set_result_pending(&result_id, true)?;
            info!("File transcription complete: {} chars", text.len());

            // Auto-paste if enabled
            let should_paste = {
                let c = lock_controller(&controller);
                auto_paste.unwrap_or(true) && c.settings().auto_paste
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
        Err(e) => Err(e.into()),
    }
}
