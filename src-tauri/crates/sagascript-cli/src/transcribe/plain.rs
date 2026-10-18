//! A plain run of one file (#343): Whisper, no diarization.
//!
//! The steps, in order: the file is decoded; the model is loaded and the
//! language checked; the decoder is resolved from the flags and the saved
//! settings; the backend transcribes; and the segments are assembled into the
//! `--json` object and the text ([`assemble_plain_result`]).

use std::time::Instant;

use indicatif::{ProgressBar, ProgressStyle};

use sagascript_core::error::DictationError;
use sagascript_core::run_context::file_beam_size;
use sagascript_core::transcription::{
    recommended_parallel_chunks, ContextProfile, TranscribeOptions, TranscriptSegment,
};

use super::inference::{prepare_language, verify_vad_model, InferenceBackend};
use super::output::{assemble_plain_result, emit_warnings, FileTranscription, PlainInput};
use super::plan::FileRequest;
use super::progress::Progress;
use super::services::Services;
use super::source::{decode_file, DecodedAudio};
use crate::output::{errln, Out};

/// Runs the plain path for one file.
pub(super) fn run_plain(
    request: &FileRequest<'_>,
    started: Instant,
    services: Services<'_>,
    model_loaded: &mut bool,
    out: &Out,
) -> Result<FileTranscription, DictationError> {
    let FileRequest {
        options: file_options,
        file,
        language,
        model,
        glossary,
        correction_vocabulary,
        control,
        ..
    } = *request;
    let progress = Progress::new(out, file_options.progress_json, started, None);
    let backend = services.backend;
    let audio = decode_file(file, services.canceller, control, &progress, out)?;
    let language_evidence = prepare_language(
        request,
        services,
        &audio.samples,
        ContextProfile::FlashAttention,
        model_loaded,
        &progress,
        out,
    )?;

    let options = resolve_decoder(request, backend, audio.samples.len(), out)?;
    let segments = transcribe(backend, request, &audio, &options, &progress, out)?;

    progress.phase("finalizing", None);
    let result = assemble_plain_result(PlainInput {
        segments,
        audio: &audio.samples,
        language,
        model,
        file,
        duration: audio.duration_seconds(),
        glossary,
        hint_vocabulary: file_options.correct_hints.then_some(correction_vocabulary),
        language_evidence: &language_evidence,
    });
    emit_warnings(out, &result.warnings);
    progress.phase("completed", Some(100));
    Ok(FileTranscription {
        json: result.json,
        plain: result.text,
        #[cfg(feature = "diarization")]
        meeting: None,
    })
}

/// The decoder settings of an ordinary file run: the flags override the saved
/// settings. Verifies the VAD model when VAD is on, and says on stderr which
/// of beam search, parallel states and VAD the run uses.
fn resolve_decoder(
    request: &FileRequest<'_>,
    backend: &dyn InferenceBackend,
    audio_samples: usize,
    out: &Out,
) -> Result<TranscribeOptions, DictationError> {
    let FileRequest {
        options: file_options,
        stored,
        model,
        glossary,
        ..
    } = *request;
    let decoder = &file_options.decoder;
    let vad_enabled = if decoder.no_vad {
        false
    } else if decoder.vad {
        true
    } else {
        stored.vad_enabled
    };
    let vad_model_path = if vad_enabled {
        let path = verify_vad_model(backend, out)?;
        path.to_str().map(str::to_string)
    } else {
        None
    };
    let mut options = TranscribeOptions {
        prompt: glossary.decoder_prompt(),
        // File transcription isn't latency-sensitive, so default to beam search
        // (fewer repetition loops). Honor an explicit beam setting/flag.
        beam_size: decoder
            .beam_size
            .unwrap_or_else(|| file_beam_size(stored.beam_size)),
        temperature_fallback: stored.temperature_fallback,
        vad_model_path,
        // Coverage diagnostics need real segment bounds for both human and
        // JSON output. Text decoding remains in no-timestamps mode.
        segment_timestamps: true,
        parallel_chunks: 1,
    };
    options.parallel_chunks = decoder
        .parallel
        .unwrap_or_else(|| recommended_parallel_chunks(audio_samples, model, options.beam_size));
    if options.beam_size >= 2 {
        errln!(out, "Beam search: width {}", options.beam_size);
    }
    if options.parallel_chunks > 1 {
        errln!(out, "Parallel Whisper states: {}", options.parallel_chunks);
    }
    if vad_enabled {
        errln!(out, "VAD: enabled");
    }
    Ok(options)
}

/// Has the backend transcribe `audio`, narrating the phases: "Preparing"
/// (lock acquisition and parallel-state creation are silent multi-second work
/// on first use), "Encoding" (compute resources are held and mel/encoder work
/// begins) and the percent of the transcription, as a bar for files over ten
/// seconds (GUI parity, #237).
fn transcribe(
    backend: &dyn InferenceBackend,
    request: &FileRequest<'_>,
    audio: &DecodedAudio,
    options: &TranscribeOptions,
    progress: &Progress,
    out: &Out,
) -> Result<Vec<TranscriptSegment>, DictationError> {
    errln!(out, "Preparing Whisper states...");
    progress.phase("preparing", None);
    let on_encode_start = || {
        errln!(out, "Encoding audio...");
        progress.phase("encoding", None);
    };
    let transcribing = progress.clone();
    if audio.duration_seconds() > 10.0 {
        let pb = ProgressBar::new(100);
        pb.set_style(ProgressStyle::with_template("  Transcribing [{bar:40}] {pos}%").unwrap());
        let pb_cb = pb.clone();
        let segments = backend.transcribe_segments(
            &audio.samples,
            request.language,
            options,
            Box::new(move |pct| {
                crate::set_transcription_progress(&pb_cb, pct);
                report_percent(&transcribing, pct);
            }),
            &on_encode_start,
        )?;
        pb.finish_and_clear();
        Ok(segments)
    } else {
        errln!(out, "Transcribing...");
        backend.transcribe_segments(
            &audio.samples,
            request.language,
            options,
            Box::new(move |pct| report_percent(&transcribing, pct)),
            &on_encode_start,
        )
    }
}

/// Whisper's first percent values are not news; the rest is clamped to 0-100.
fn report_percent(progress: &Progress, pct: i32) {
    if pct > 1 {
        progress.phase("transcribing", Some(pct.clamp(0, 100) as u8));
    }
}
