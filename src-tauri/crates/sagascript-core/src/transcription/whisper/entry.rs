use std::time::Instant;

use tracing::{info, warn};
use whisper_rs::{get_lang_str, FullParams, SamplingStrategy, WhisperState, WhisperVadParams};

use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::transcription::chunking::plan_chunks;
use crate::transcription::diagnostics::LanguageDetection;
use crate::transcription::model;

use super::parallel::{ParallelJob, MAX_PARALLEL_CHUNKS, MIN_PARALLEL_CHUNK_SAMPLES};
use super::progress::{clamped_progress_callback, started_progress_callback};
use super::stitch::{display_transcript, sanitize_segment_bounds, strip_timestamp_tokens};
#[cfg(feature = "diarization")]
use super::words::dtw_segment_timestamps;
use super::{
    ContextProfile, DictationTimings, TranscribeOptions, TranscribeRequest, TranscriptSegment,
    WhisperBackend,
};

/// Mean of per-token log-probabilities; `None` for an empty slice.
fn mean_logprob(plogs: &[f32]) -> Option<f32> {
    if plogs.is_empty() {
        return None;
    }
    Some(plogs.iter().sum::<f32>() / plogs.len() as f32)
}

/// Derive the same text-token log-probability mean for every Whisper output
/// path. The binding exposes token data, but not Whisper's internal segment
/// confidence value; token IDs at or above EOT are special tokens and are
/// excluded from this diagnostic.
pub(super) fn segment_mean_logprob(
    segment: &whisper_rs::WhisperSegment<'_>,
    token_eot: i32,
) -> Option<f32> {
    let mut plogs = Vec::with_capacity(segment.n_tokens().max(0) as usize);
    for token_index in 0..segment.n_tokens() {
        if let Some(token) = segment.get_token(token_index) {
            if token.token_id() < token_eot {
                plogs.push(token.token_data().plog);
            }
        }
    }
    mean_logprob(&plogs)
}

/// How a piece of audio is decoded, apart from the audio itself and the state
/// it runs on.
#[derive(Clone, Copy)]
pub(super) struct DecodeSettings<'a> {
    pub(super) model: WhisperModel,
    pub(super) language: Language,
    pub(super) options: &'a TranscribeOptions,
    /// End-of-text token id of the loaded context; token ids at or above it
    /// are special tokens, excluded from the avg-logprob diagnostic.
    pub(super) token_eot: i32,
}

/// One chunk to decode: its audio, where it starts in the whole recording and
/// how to decode it.
#[derive(Clone, Copy)]
pub(super) struct ChunkJob<'a> {
    pub(super) audio: &'a [f32],
    /// Seconds added to every segment time.
    pub(super) offset_seconds: f64,
    pub(super) settings: DecodeSettings<'a>,
}

/// Which decoding call site is asking for flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DecodePath {
    /// File transcription (single or parallel chunk), live dictation, CLI
    /// record and every other text path that goes through `full_params`.
    Text { segment_timestamps: bool },
    /// Word-timestamp / diarization / meeting path.
    #[cfg(feature = "diarization")]
    WordTimestamps,
}

/// Per-call-site whisper decoding flags, kept pure so they can be unit tested.
///
/// Generative timestamps must stay ON everywhere: with `no_timestamps` whisper.cpp
/// advances its seek by a full 30 s window after each decode, so an early
/// end-of-text after a pause silently drops the rest of the window (issue #236,
/// about 11% of words on long Swedish audio, and the tail of any <=30 s clip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct DecodeFlags {
    pub(super) no_timestamps: bool,
    pub(super) token_timestamps: bool,
}

impl DecodeFlags {
    pub(super) fn for_path(path: DecodePath) -> Self {
        match path {
            DecodePath::Text { segment_timestamps } => Self {
                no_timestamps: false,
                token_timestamps: segment_timestamps,
            },
            #[cfg(feature = "diarization")]
            DecodePath::WordTimestamps => Self {
                no_timestamps: false,
                token_timestamps: true,
            },
        }
    }
}

/// Re-decode the dense-speech spans the first pass left uncovered (#273),
/// once, within the redecode budget. Re-decodes are plain requests: no hooks
/// and no parallel chunks. A cancel (`cancelled`) during or after the
/// re-decodes discards the partial result as `Cancelled`.
///
/// Split out of [`WhisperBackend::transcribe_with_gap_recovery`] so the
/// wiring can be tested with a fake `decode`.
fn recover_gaps(
    audio: &[f32],
    language: Language,
    options: &TranscribeOptions,
    model: WhisperModel,
    segments: Vec<TranscriptSegment>,
    cancelled: &dyn Fn() -> bool,
    mut decode: impl FnMut(TranscribeRequest<'_>) -> Result<Vec<TranscriptSegment>, DictationError>,
) -> Result<Vec<TranscriptSegment>, DictationError> {
    let limits = crate::transcription::gap_redecode::RedecodeLimits::for_audio(
        audio.len() as f64 / 16_000.0,
        model.no_speech_threshold(),
    );
    let mut redecode_options = options.clone();
    redecode_options.parallel_chunks = 1;
    let recovered = crate::transcription::gap_redecode::redecode_uncovered_speech_cancellable(
        audio,
        segments,
        &limits,
        cancelled,
        |slice| decode(TranscribeRequest::new(slice, language, &redecode_options)),
    );
    if cancelled() {
        return Err(WhisperBackend::cancelled_error());
    }
    Ok(recovered)
}

impl WhisperBackend {
    /// Run Whisper's local language classifier on a bounded, speech-rich
    /// preview of the decoded audio. English-only models cannot classify
    /// languages and return `Ok(None)`.
    ///
    /// The preview is capped at 30 seconds and chosen from the first five
    /// minutes by RMS energy. This keeps the check cheap while avoiding a
    /// silent intro when the recording starts later.
    pub fn detect_language(
        &self,
        audio: &[f32],
    ) -> Result<Option<LanguageDetection>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }

        let is_multilingual = self.active_context()?.is_multilingual();
        if !is_multilingual {
            return Ok(None);
        }

        let preview = language_detection_preview(audio);
        let n_threads = whisper_threads().max(1) as usize;
        self.with_warm_state(|state| {
            state.pcm_to_mel(preview, n_threads).map_err(|error| {
                DictationError::TranscriptionFailed(format!(
                    "Language detection spectrogram failed: {error}"
                ))
            })?;
            let (language_id, probabilities) =
                state.lang_detect(0, n_threads).map_err(|error| {
                    DictationError::TranscriptionFailed(format!(
                        "Language detection failed: {error}"
                    ))
                })?;
            let language = get_lang_str(language_id).ok_or_else(|| {
                DictationError::TranscriptionFailed(format!(
                    "Language detection returned unknown language id {language_id}"
                ))
            })?;
            let probability = probabilities
                .get(language_id as usize)
                .copied()
                .unwrap_or_default();
            Ok(Some(LanguageDetection {
                language: language.to_string(),
                probability,
            }))
        })
    }

    /// Keep selection and inference in one transaction: startup warmup or a
    /// second language must not swap the active context between these steps.
    /// The request's hooks are honoured; a `live` request skips near-silent
    /// audio before any model is acquired. Blocking; desktop callers must use
    /// spawn_blocking.
    pub fn transcribe_dictation(
        &self,
        model: WhisperModel,
        request: TranscribeRequest<'_>,
        timings: &mut DictationTimings,
    ) -> Result<String, DictationError> {
        *timings = DictationTimings::default();
        let Some(request) = request.after_live_guard()? else {
            return Ok(String::new());
        };
        self.dictation_transaction(
            timings,
            |timings| {
                timings.model_cached = !self.needs_reload(model);
                self.ensure_model_inner(model, ContextProfile::FlashAttention)
            },
            || self.transcribe_text(request),
        )
    }

    /// The transaction behind [`Self::transcribe_dictation`]: wait for the load
    /// lock, make the model active (`select`, which also records whether it was
    /// already resident), then run `infer` while the selection stays pinned,
    /// timing each step. Split out so the bookkeeping can be tested without a
    /// loadable model.
    pub(super) fn dictation_transaction(
        &self,
        timings: &mut DictationTimings,
        select: impl FnOnce(&mut DictationTimings) -> Result<(), DictationError>,
        infer: impl FnOnce() -> Result<String, DictationError>,
    ) -> Result<String, DictationError> {
        *timings = DictationTimings::default();
        let model_started = Instant::now();
        timings.model_acquisition_started = true;
        let _load = match self.lock_load_bounded() {
            Ok(load) => load,
            Err(error) => {
                timings.model_ms = model_started.elapsed().as_secs_f64() * 1000.0;
                return Err(error);
            }
        };
        let selection = select(timings);
        timings.model_ms = model_started.elapsed().as_secs_f64() * 1000.0;
        selection?;
        timings.model_ready_at = Some(Instant::now());
        timings.inference_started = true;
        let inference_started = Instant::now();
        let result = infer();
        timings.inference_ms = inference_started.elapsed().as_secs_f64() * 1000.0;
        result
    }

    pub fn warmup_model(
        &self,
        model: WhisperModel,
        language: Language,
    ) -> Result<(), DictationError> {
        let mut timings = DictationTimings::default();
        self.transcribe_dictation(
            model,
            TranscribeRequest::new(&[0.0; 1600], language, &TranscribeOptions::default()),
            &mut timings,
        )?;
        Ok(())
    }

    /// Pre-compile inference kernels and warm the reusable state by running a
    /// short silent inference. Call once after the model is loaded so the first
    /// real dictation doesn't pay the Metal/CoreML kernel-compile + state-init
    /// cost. Best-effort — a failure here is non-fatal.
    pub fn warmup(&self, language: Language) -> Result<(), DictationError> {
        // 0.1s of silence is enough to build the compute graph and compile the
        // kernels (whisper pads to its 30s window internally regardless).
        let silence = vec![0.0f32; 1600];
        self.transcribe_text(TranscribeRequest::new(
            &silence,
            language,
            &TranscribeOptions::default(),
        ))?;
        info!("Whisper warmup complete");
        Ok(())
    }

    /// Transcribe on the loaded model and return the display text: the raw
    /// segment texts joined, then display-only non-speech marker normalization
    /// (see [`display_transcript`]). Timestamped segment text stays raw; use
    /// [`Self::transcribe_segments`] for it. Blocking — call from
    /// spawn_blocking.
    pub fn transcribe_text(
        &self,
        request: TranscribeRequest<'_>,
    ) -> Result<String, DictationError> {
        let language = request.language;
        let segments = self.transcribe_segments(request)?;
        Ok(display_transcript(&segments, language))
    }

    /// Plain-file transcription with recovery of speech Whisper's windowing
    /// skipped (#273): runs [`Self::transcribe_segments`], then re-decodes
    /// dense-speech uncovered spans once, within a bounded budget (see
    /// [`crate::transcription::gap_redecode`]). The request's hooks observe the
    /// first pass only. Shared by the CLI and the app's plain file
    /// transcription; diarization and meeting paths do not use it. The display
    /// text of the result is [`display_transcript`].
    pub fn transcribe_with_gap_recovery(
        &self,
        request: TranscribeRequest<'_>,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        let (audio, language, options) = (request.audio, request.language, request.options);
        let segments = self.transcribe_segments(request)?;
        let model = self.loaded_model().ok_or(DictationError::ModelNotLoaded)?;
        recover_gaps(
            audio,
            language,
            options,
            model,
            segments,
            &|| self.is_cancel_requested(),
            |request| self.transcribe_segments(request),
        )
    }

    /// Core transcription entry point: the individual whisper segments with
    /// timing and confidence metadata ([`TranscriptSegment`]). Honors the
    /// request's [`TranscribeOptions`] (prompt, beam search, temperature
    /// fallback, VAD, parallel chunks) and hooks. Blocking — call from
    /// spawn_blocking.
    pub fn transcribe_segments(
        &self,
        request: TranscribeRequest<'_>,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        let Some(request) = request.after_live_guard()? else {
            return Ok(Vec::new());
        };
        let TranscribeRequest {
            audio,
            language,
            options: opts,
            hooks,
            live: _,
        } = request;
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }

        // End-of-text token id, used to exclude special tokens from the
        // avg-logprob computation (matching whisper.cpp's confidence examples:
        // text tokens have id < eot). Read from a context handle taken under
        // one short lock and dropped again, so no runtime lock is held when
        // with_warm_state acquires the state lock.
        let (model, token_eot) = self.active_model_and_eot()?;

        // Leave 0% (#237) once compute resources are held, i.e. right after
        // `on_encode_start` fires (or on the first real callback), so the
        // phase never goes "transcribing 1%" -> "encoding" and state-lock
        // waiting is not counted as transcribing.
        let on_progress = hooks.on_progress.unwrap_or_else(|| Box::new(|_| {}));
        let (on_progress, begin_progress) = started_progress_callback(on_progress);
        let on_encode_start = hooks.on_encode_start;
        let encode_start_then_begin = || {
            if let Some(on_encode_start) = on_encode_start {
                on_encode_start();
            }
            begin_progress();
        };
        let on_encode_start: Option<&dyn Fn()> = Some(&encode_start_then_begin);

        let settings = DecodeSettings {
            model,
            language,
            options: opts,
            token_eot,
        };
        let chunks = plan_chunks(
            audio,
            opts.parallel_chunks.clamp(1, MAX_PARALLEL_CHUNKS),
            MIN_PARALLEL_CHUNK_SAMPLES,
        );

        info!(
            "Starting local transcription: {} samples, {} threads/state, {} chunk(s), lang={:?}, beam={}, temp_fallback={}, vad={}",
            audio.len(),
            whisper_threads(),
            chunks.len(),
            language,
            opts.beam_size,
            opts.temperature_fallback,
            opts.vad_model_path.is_some()
        );

        if chunks.len() == 1 {
            return self.with_warm_state(|state| {
                if let Some(on_encode_start) = on_encode_start {
                    on_encode_start();
                }
                self.transcribe_chunk_segments(
                    state,
                    ChunkJob {
                        audio,
                        offset_seconds: 0.0,
                        settings,
                    },
                    on_progress,
                )
            });
        }

        self.transcribe_parallel_segments(ParallelJob {
            audio,
            chunks: &chunks,
            settings,
            on_progress: Box::new(on_progress),
            on_encode_start,
        })
    }

    /// Decode one chunk on `state` and return its segments, shifted by the
    /// chunk's offset.
    pub(super) fn transcribe_chunk_segments(
        &self,
        state: &mut WhisperState,
        job: ChunkJob<'_>,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<Vec<TranscriptSegment>, DictationError> {
        let ChunkJob {
            audio,
            offset_seconds,
            settings:
                DecodeSettings {
                    model,
                    language,
                    options: opts,
                    token_eot,
                },
        } = job;
        let mut params = self.full_params(model, language, opts, on_progress)?;
        self.install_abort_callback(&mut params);
        state.full(params, audio).map_err(|error| {
            DictationError::TranscriptionFailed(format!("Whisper inference failed: {error}"))
        })?;

        let n_segments = state.full_n_segments();
        let mut segments = Vec::with_capacity(n_segments.max(0) as usize);
        let chunk_duration = audio.len() as f64 / 16_000.0;
        let mut previous_end = 0.0;
        for index in 0..n_segments {
            let Some(segment) = state.get_segment(index) else {
                continue;
            };
            let text = match segment.to_str() {
                Ok(text) => strip_timestamp_tokens(text),
                Err(error) => {
                    warn!(
                        "Segment {index} failed UTF-8 conversion, dropping from transcript: {error}"
                    );
                    continue;
                }
            };
            let raw_bounds = {
                #[cfg(feature = "diarization")]
                {
                    if opts.segment_timestamps {
                        dtw_segment_timestamps(&segment, 0.0).unwrap_or_else(|| {
                            (
                                segment.start_timestamp() as f64 / 100.0,
                                segment.end_timestamp() as f64 / 100.0,
                            )
                        })
                    } else {
                        (
                            segment.start_timestamp() as f64 / 100.0,
                            segment.end_timestamp() as f64 / 100.0,
                        )
                    }
                }
                #[cfg(not(feature = "diarization"))]
                {
                    (
                        segment.start_timestamp() as f64 / 100.0,
                        segment.end_timestamp() as f64 / 100.0,
                    )
                }
            };
            let (start, end) =
                sanitize_segment_bounds(raw_bounds.0, raw_bounds.1, chunk_duration, previous_end);
            previous_end = end;
            segments.push(TranscriptSegment {
                start: start + offset_seconds,
                end: end + offset_seconds,
                text,
                avg_logprob: segment_mean_logprob(&segment, token_eot),
                no_speech_prob: segment.no_speech_probability(),
            });
        }
        Ok(segments)
    }

    fn full_params(
        &self,
        model: WhisperModel,
        language: Language,
        opts: &TranscribeOptions,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<FullParams<'static, 'static>, DictationError> {
        let beam_size = opts.beam_size.clamp(0, 8);
        let strategy = if beam_size >= 2 {
            SamplingStrategy::BeamSearch {
                beam_size: beam_size as i32,
                patience: -1.0,
            }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(strategy);
        params.set_language(language.whisper_code());
        params.set_n_threads(whisper_threads());
        params.set_temperature(0.0);
        params.set_temperature_inc(if opts.temperature_fallback { 0.2 } else { 0.0 });
        params.set_translate(false);
        let flags = DecodeFlags::for_path(DecodePath::Text {
            segment_timestamps: opts.segment_timestamps,
        });
        params.set_no_timestamps(flags.no_timestamps);
        params.set_token_timestamps(flags.token_timestamps);
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(model.no_speech_threshold());
        params.set_suppress_blank(true);
        if let Some(prompt) = &opts.prompt {
            if !prompt.is_empty() {
                params.set_initial_prompt(prompt);
            }
        }
        if let Some(vad_path) = &opts.vad_model_path {
            model::verify_vad_model(std::path::Path::new(vad_path))?;
            params.set_vad_model_path(Some(vad_path.as_str()));
            let mut vad = WhisperVadParams::new();
            vad.set_threshold(0.5);
            vad.set_min_silence_duration(200);
            vad.set_speech_pad(50);
            params.set_vad_params(vad);
            params.enable_vad(true);
        }
        params.set_progress_callback_safe(clamped_progress_callback(on_progress));
        Ok(params)
    }
}

/// Choose at most 30 seconds from the first five minutes for language
/// detection, preferring the non-overlapping window with the highest RMS
/// energy. Scanning non-overlapping windows keeps this linear in the bounded
/// prefix and is sufficient to skip long silent intros.
fn language_detection_preview(audio: &[f32]) -> &[f32] {
    const PREVIEW_SAMPLES: usize = 30 * 16_000;
    const SCAN_SAMPLES: usize = 5 * 60 * 16_000;

    if audio.len() <= PREVIEW_SAMPLES {
        return audio;
    }

    let scan_end = audio.len().min(SCAN_SAMPLES);
    let mut best_start = 0usize;
    let mut best_mean_square = f64::NEG_INFINITY;
    for start in (0..scan_end).step_by(PREVIEW_SAMPLES) {
        let end = (start + PREVIEW_SAMPLES).min(scan_end);
        let window = &audio[start..end];
        let mean_square = window
            .iter()
            .map(|&sample| f64::from(sample) * f64::from(sample))
            .sum::<f64>()
            / window.len().max(1) as f64;
        if mean_square > best_mean_square {
            best_mean_square = mean_square;
            best_start = start;
        }
    }
    let best_end = (best_start + PREVIEW_SAMPLES).min(audio.len());
    &audio[best_start..best_end]
}

/// Choose a whisper CPU thread count. `num_cpus` counts all cores; on Apple
/// Silicon that includes the slower efficiency cores, and scheduling whisper's
/// CPU work onto them hurts latency. Target the performance-core count
/// (`hw.perflevel0.logicalcpu`) on macOS, falling back to `num_cpus / 2`
/// elsewhere or if the sysctl is unavailable. Computed once and cached.
pub(super) fn whisper_threads() -> i32 {
    use std::sync::OnceLock;
    static THREADS: OnceLock<i32> = OnceLock::new();
    *THREADS.get_or_init(|| {
        #[cfg(target_os = "macos")]
        if let Some(n) = macos_perf_cores() {
            return n;
        }
        (num_cpus::get() / 2).max(1) as i32
    })
}

/// Number of performance (P) cores on Apple Silicon via sysctl. Returns `None`
/// on Intel Macs (key absent) or if the query fails.
#[cfg(target_os = "macos")]
fn macos_perf_cores() -> Option<i32> {
    let out = std::process::Command::new("sysctl")
        .args(["-n", "hw.perflevel0.logicalcpu"])
        .output()
        .ok()?;
    String::from_utf8(out.stdout)
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|&n| n > 0)
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
