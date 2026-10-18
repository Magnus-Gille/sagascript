//! DTW word timestamps for speaker diarization. Compiled only with the
//! `diarization` feature, which gates the module in `mod.rs`.

use std::time::Instant;

use tracing::warn;
use whisper_rs::{FullParams, SamplingStrategy, WhisperVadParams};

use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::transcription::model;

use super::entry::{segment_mean_logprob, whisper_threads, DecodeFlags, DecodePath};
use super::stitch::strip_timestamp_tokens;
use super::{
    effective_diarization_decoder_config, ContextProfile, TranscribeOptions, TranscriptSegment,
    WhisperBackend,
};

/// Timestamped transcript plus phase timings used by the diarization path.
#[derive(Debug, Clone)]
pub struct DiarizationTranscription {
    pub segments: Vec<(f64, f64, String)>,
    pub coverage_segments: Vec<TranscriptSegment>,
    pub timings: DiarizationTranscriptionTimings,
}

/// Separates native Whisper inference from Rust-side DTW word attribution.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct DiarizationTranscriptionTimings {
    pub whisper_inference_seconds: f64,
    pub word_timestamp_attribution_seconds: f64,
}

/// DTW model preset for accurate attention-based token timestamps.
/// KB-Whisper and NB-Whisper are fine-tunes of the corresponding base architecture,
/// so they use the same alignment heads.
pub(super) fn dtw_preset(model: WhisperModel) -> whisper_rs::DtwModelPreset {
    use whisper_rs::DtwModelPreset;
    match model {
        WhisperModel::TinyEn => DtwModelPreset::TinyEn,
        WhisperModel::Tiny
        | WhisperModel::FinnishWhisperTiny
        | WhisperModel::KbWhisperTiny
        | WhisperModel::NbWhisperTiny => DtwModelPreset::Tiny,
        WhisperModel::BaseEn => DtwModelPreset::BaseEn,
        WhisperModel::Base | WhisperModel::KbWhisperBase | WhisperModel::NbWhisperBase => {
            DtwModelPreset::Base
        }
        WhisperModel::SmallEn => DtwModelPreset::SmallEn,
        WhisperModel::Small | WhisperModel::KbWhisperSmall | WhisperModel::NbWhisperSmall => {
            DtwModelPreset::Small
        }
        WhisperModel::MediumEn => DtwModelPreset::MediumEn,
        WhisperModel::Medium | WhisperModel::KbWhisperMedium | WhisperModel::NbWhisperMedium => {
            DtwModelPreset::Medium
        }
        WhisperModel::LargeV3Turbo | WhisperModel::LargeV3TurboQ8 => DtwModelPreset::LargeV3Turbo,
        // KbWhisperLarge / NbWhisperLarge are large-v3 fine-tunes
        WhisperModel::KbWhisperLarge | WhisperModel::NbWhisperLarge => DtwModelPreset::LargeV3,
    }
}

impl WhisperBackend {
    /// Transcribe audio and return per-segment timestamps.
    ///
    /// For audio longer than 30s, uses overlapping 30s windows to prevent
    /// the Whisper decoder from drifting or looping on long recordings.
    pub fn transcribe_sync_with_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        // Process full audio in one call — whisper.cpp handles long audio internally
        // via its own sliding window. With no_timestamps=true + DTW there is no
        // looping risk, and the internal context is better than manual chunking.
        Ok(self
            .transcribe_chunk_timestamps_profiled(
                audio,
                language,
                0.0,
                prompt,
                Granularity::Segment,
                &TranscribeOptions::default(),
            )?
            .segments)
    }

    /// Transcribe audio and return per-word timestamps.
    ///
    /// Uses DTW per-token timing from whisper.cpp to group BPE tokens into words,
    /// producing fine-grained `(start, end, word)` tuples. This is the preferred
    /// input for the diarization merge pipeline because it avoids collapsing a
    /// multi-speaker Whisper segment into a single speaker label.
    ///
    /// Falls through to empty if DTW timestamps are unavailable (caller should
    /// fall back to `transcribe_sync_with_timestamps`).
    pub fn transcribe_sync_with_word_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        Ok(self
            .transcribe_chunk_timestamps_profiled(
                audio,
                language,
                0.0,
                prompt,
                Granularity::Word,
                &TranscribeOptions::default(),
            )?
            .segments)
    }

    /// Transcribe audio at the finest timestamp granularity available for
    /// speaker attribution.
    ///
    /// Word-level DTW timestamps prevent a long Whisper segment containing
    /// multiple turns from being assigned wholesale to one speaker. Some
    /// models/platforms may not expose valid DTW timestamps, so this preserves
    /// the segment-level fallback in one shared path for both CLI and GUI.
    pub fn transcribe_sync_for_diarization(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        Ok(self
            .transcribe_sync_for_diarization_profiled(audio, language, prompt)?
            .segments)
    }

    /// Profiled variant used by the CLI benchmark and cache path.
    pub fn transcribe_sync_for_diarization_profiled(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<DiarizationTranscription, DictationError> {
        self.transcribe_sync_for_diarization_profiled_with_options(
            audio,
            language,
            prompt,
            &TranscribeOptions::default(),
        )
    }

    /// Profiled diarization transcription with the effective decoder and VAD
    /// options selected by the caller. The legacy profiled method above keeps
    /// its greedy/no-VAD defaults for GUI and shared reprocessing callers.
    pub fn transcribe_sync_for_diarization_profiled_with_options(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
        options: &TranscribeOptions,
    ) -> Result<DiarizationTranscription, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        let decoder_config = effective_diarization_decoder_config(options)?;
        if self.loaded_context_profile() != Some(ContextProfile::TokenAlignment) {
            return Err(DictationError::TranscriptionFailed(
                "Diarization transcription requires a token-alignment model context".to_string(),
            ));
        }
        // whisper.cpp maps segment timestamps through its VAD mapping table,
        // while DTW token timestamps remain in the compacted audio timeline.
        // Use the mapped segment granularity for VAD runs so speaker merge
        // never receives silently shifted word coordinates. The explicit
        // fallback is surfaced in the CLI diagnostics and keeps timing valid.
        let granularity = if decoder_config.vad_enabled {
            warn!("VAD diarization uses mapped segment timestamps; DTW word timestamps are unavailable for compacted audio");
            Granularity::Segment
        } else {
            Granularity::Word
        };
        let mut words = self.transcribe_chunk_timestamps_profiled(
            audio,
            language,
            0.0,
            prompt,
            granularity,
            options,
        )?;
        if granularity == Granularity::Segment || !words.segments.is_empty() {
            return Ok(words);
        }

        warn!("Word-level DTW timestamps unavailable; falling back to segment timestamps");
        let fallback = self.transcribe_chunk_timestamps_profiled(
            audio,
            language,
            0.0,
            prompt,
            Granularity::Segment,
            options,
        )?;
        words.segments = fallback.segments;
        words.coverage_segments = fallback.coverage_segments;
        words.timings.whisper_inference_seconds += fallback.timings.whisper_inference_seconds;
        words.timings.word_timestamp_attribution_seconds +=
            fallback.timings.word_timestamp_attribution_seconds;
        Ok(words)
    }

    /// Transcribe a single audio chunk with timestamps via DTW.
    ///
    /// `granularity` controls whether to emit one entry per Whisper segment
    /// (`Segment`) or one entry per word (`Word`).
    ///
    /// Decodes with generative timestamps ON (see [`DecodeFlags`]) so whisper.cpp
    /// seeks to the last real timestamp instead of skipping a whole 30 s window
    /// after an early end-of-text. Word timing still comes from `token.t_dtw`
    /// (cross-attention DTW alignment); timestamp tokens are skipped when grouping.
    /// `t_offset` (seconds) is added to all returned timestamps.
    fn transcribe_chunk_timestamps_profiled(
        &self,
        audio: &[f32],
        language: Language,
        t_offset: f64,
        prompt: Option<&str>,
        granularity: Granularity,
        options: &TranscribeOptions,
    ) -> Result<DiarizationTranscription, DictationError> {
        let (model, token_eot) = self.active_model_and_eot()?;

        let n_threads = whisper_threads();
        let no_speech_thold = model.no_speech_threshold();

        let decoder_config = effective_diarization_decoder_config(options)?;
        let strategy = if decoder_config.beam_size >= 2 {
            SamplingStrategy::BeamSearch {
                beam_size: decoder_config.beam_size as i32,
                patience: -1.0,
            }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut params = FullParams::new(strategy);
        params.set_language(language.whisper_code());
        params.set_n_threads(n_threads);
        params.set_temperature(0.0);
        params.set_temperature_inc(if decoder_config.temperature_fallback {
            0.2
        } else {
            0.0
        });
        params.set_translate(false);
        let flags = DecodeFlags::for_path(DecodePath::WordTimestamps);
        params.set_no_timestamps(flags.no_timestamps);
        params.set_token_timestamps(flags.token_timestamps); // token.t0/t1/t_dtw via cross-attention
        params.set_print_timestamps(false);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(no_speech_thold);
        params.set_suppress_blank(true);
        if let Some(p) = prompt {
            params.set_initial_prompt(p);
        }
        if let Some(vad_path) = &options.vad_model_path {
            model::verify_vad_model(std::path::Path::new(vad_path))?;
            params.set_vad_model_path(Some(vad_path.as_str()));
            let mut vad = WhisperVadParams::new();
            vad.set_threshold(decoder_config.vad_threshold);
            vad.set_min_silence_duration(decoder_config.vad_min_silence_duration_ms);
            vad.set_speech_pad(decoder_config.vad_speech_pad_ms);
            vad.set_samples_overlap(decoder_config.vad_samples_overlap);
            params.set_vad_params(vad);
            params.enable_vad(true);
        }

        // Same real-abort wiring as the dictation path so a timed-out diarization
        // transcription can be aborted instead of pinning the warm-state mutex.
        // The flag's lifecycle is owned by with_warm_state under the state lock.
        self.install_abort_callback(&mut params);

        self.with_warm_state(|state| {
            let inference_started = Instant::now();
            state.full(params, audio).map_err(|e| {
                DictationError::TranscriptionFailed(format!("Whisper inference failed: {e}"))
            })?;
            let whisper_inference_seconds = inference_started.elapsed().as_secs_f64();

            let attribution_started = Instant::now();
            let n_segments = state.full_n_segments();
            let mut results = Vec::with_capacity(n_segments as usize);

            let coverage_segments = match granularity {
                Granularity::Segment => {
                    let mut coverage_segments = Vec::with_capacity(n_segments as usize);
                    for i in 0..n_segments {
                        let Some(segment) = state.get_segment(i) else { continue };
                        let text = match segment.to_str() {
                            Ok(text) => strip_timestamp_tokens(text).trim().to_string(),
                            Err(error) => {
                                warn!(
                                    "Segment {i} failed UTF-8 conversion, dropping from transcript: {error}"
                                );
                                continue;
                            }
                        };
                        if text.is_empty() {
                            continue;
                        }
                        coverage_segments.push(TranscriptSegment {
                            start: segment.start_timestamp() as f64 / 100.0 + t_offset,
                            end: segment.end_timestamp() as f64 / 100.0 + t_offset,
                            text: text.clone(),
                            avg_logprob: segment_mean_logprob(&segment, token_eot),
                            no_speech_prob: segment.no_speech_probability(),
                        });
                        // Derive segment timing from DTW timestamps on first/last non-special
                        // token. DTW timestamps are in centiseconds; -1 means not computed.
                        // Fall back to segment-level timestamps if DTW was not available.
                        let (t0, t1) = if options.vad_model_path.is_some() {
                            // WhisperSegment exposes the original-timeline
                            // bounds when VAD is active. DTW token values are
                            // in compacted coordinates and must not be used.
                            (
                                segment.start_timestamp() as f64 / 100.0 + t_offset,
                                segment.end_timestamp() as f64 / 100.0 + t_offset,
                            )
                        } else {
                            dtw_segment_timestamps(&segment, t_offset).unwrap_or_else(|| {
                            let t0 = segment.start_timestamp() as f64 / 100.0 + t_offset;
                            let t1 = segment.end_timestamp() as f64 / 100.0 + t_offset;
                            (t0, t1)
                            })
                        };
                        results.push((t0, t1, text));
                    }
                    coverage_segments
                }
                Granularity::Word => {
                    // Collect per-token timings for ALL segments first, then group
                    // in a single pass so cross-segment DTW inheritance is correct.
                    let mut segs: Vec<Vec<TokenTiming>> = Vec::with_capacity(n_segments as usize);
                    let mut coverage_segments = Vec::with_capacity(n_segments as usize);
                    for i in 0..n_segments {
                        let Some(segment) = state.get_segment(i) else { continue };
                        if let Some(text) = meaningful_whisper_segment_coverage_text(&segment, i) {
                            coverage_segments.push(TranscriptSegment {
                                start: segment.start_timestamp() as f64 / 100.0 + t_offset,
                                end: segment.end_timestamp() as f64 / 100.0 + t_offset,
                                text,
                                avg_logprob: segment_mean_logprob(&segment, token_eot),
                                no_speech_prob: segment.no_speech_probability(),
                            });
                        }
                        let n_tok = segment.n_tokens();
                        let mut token_timings = Vec::with_capacity(n_tok as usize);
                        for j in 0..n_tok {
                            let Some(token) = segment.get_token(j) else { continue };
                            let td = token.token_data();
                            let text = token.to_str().unwrap_or("").to_string();
                            // t_dtw in centiseconds; -1 means invalid
                            let t_dtw = if td.t_dtw >= 0 {
                                td.t_dtw as f64 / 100.0 + t_offset
                            } else {
                                -1.0 // invalid sentinel
                            };
                            token_timings.push(TokenTiming { t_dtw, text });
                        }
                        segs.push(token_timings);
                    }
                    // words_from_segments returns empty if no valid DTW exists,
                    // which triggers the CLI fallback to segment-level timestamps.
                    results = words_from_segments(&segs);
                    coverage_segments
                }
            };

            Ok(DiarizationTranscription {
                segments: results,
                coverage_segments,
                timings: DiarizationTranscriptionTimings {
                    whisper_inference_seconds,
                    word_timestamp_attribution_seconds: attribution_started
                        .elapsed()
                        .as_secs_f64(),
                },
            })
        })
    }
}

/// Controls whether `transcribe_chunk_timestamps` emits one entry per segment or per word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Segment,
    Word,
}

/// Return meaningful text after stripping generated timestamp/special tokens.
fn meaningful_coverage_text(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let text = strip_timestamp_tokens(&text).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Return meaningful text from a Whisper segment for coarse coverage diagnostics.
///
/// Coverage is diagnostic metadata, so invalid UTF-8 is replaced lossily. The
/// user-facing diarized transcript remains strict and is handled separately.
fn meaningful_whisper_segment_coverage_text(
    segment: &whisper_rs::WhisperSegment<'_>,
    index: i32,
) -> Option<String> {
    let bytes = match segment.to_bytes() {
        Ok(bytes) => bytes,
        Err(error) => {
            warn!(
                "Segment {index} text could not be read, dropping from coverage diagnostics: {error}"
            );
            return None;
        }
    };
    meaningful_coverage_text(bytes)
}

/// Extract segment timing from per-token DTW timestamps.
///
/// Returns `(start_secs, end_secs)` using the DTW timestamps of the first and last
/// content tokens (skipping special tokens which have t_dtw = -1).
/// Returns `None` if no token has a valid DTW timestamp.
pub(super) fn dtw_segment_timestamps(
    segment: &whisper_rs::WhisperSegment<'_>,
    t_offset: f64,
) -> Option<(f64, f64)> {
    let n = segment.n_tokens();
    let mut t0 = None::<f64>;
    let mut t1 = None::<f64>;

    for j in 0..n {
        let Some(token) = segment.get_token(j) else {
            continue;
        };
        let td = token.token_data();
        if td.t_dtw >= 0 {
            let t = td.t_dtw as f64 / 100.0 + t_offset;
            if t0.is_none() {
                t0 = Some(t);
            }
            t1 = Some(t);
        }
    }

    match (t0, t1) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => None,
    }
}

/// Input token with DTW timing and text for word grouping.
/// `t_dtw` is in seconds; use `f64::NAN` or a negative value if invalid.
#[derive(Debug, Clone)]
pub struct TokenTiming {
    /// DTW timestamp in seconds. Negative or NAN means invalid/not computed.
    pub t_dtw: f64,
    /// Raw token text as emitted by whisper (may have a leading space).
    pub text: String,
}

/// Group raw whisper tokens into words with `(start_secs, end_secs, word_text)`.
///
/// Word boundaries: a new word starts when the token text begins with a leading
/// ASCII space `' '`. The first content token also starts a word. Tokens without
/// a leading space are appended verbatim to the current word (so punctuation
/// attaches to the preceding token).
///
/// Special tokens are skipped: empty text, text starting with `'['`, or text
/// containing `"<|"`.
///
/// Timing: each word's start = min valid t_dtw among its tokens; end = max.
/// If a word has no valid DTW token, it inherits the previous word's end time
/// (best-effort interpolation). Words are never dropped.
pub fn group_tokens_into_words(tokens: &[TokenTiming]) -> Vec<(f64, f64, String)> {
    // Collect (text, Option<min_t>, Option<max_t>) per word first
    struct WordAcc {
        text: String,
        t_min: Option<f64>,
        t_max: Option<f64>,
    }

    let mut words: Vec<WordAcc> = Vec::new();

    for token in tokens {
        let raw = &token.text;

        // Skip special tokens
        if raw.is_empty() || raw.starts_with('[') || raw.contains("<|") {
            continue;
        }

        let starts_new_word = raw.starts_with(' ');
        let text_part = if starts_new_word {
            raw.trim_start_matches(' ')
        } else {
            raw.as_str()
        };

        // A leading space OR first content token ever starts a new word
        if starts_new_word || words.is_empty() {
            words.push(WordAcc {
                text: text_part.to_string(),
                t_min: None,
                t_max: None,
            });
        } else {
            // Append to the current word (punctuation etc.)
            words.last_mut().unwrap().text.push_str(text_part);
        }

        // Record valid DTW timestamp
        let t = token.t_dtw;
        if t.is_finite() && t >= 0.0 {
            let acc = words.last_mut().unwrap();
            acc.t_min = Some(acc.t_min.map_or(t, |prev| prev.min(t)));
            acc.t_max = Some(acc.t_max.map_or(t, |prev| prev.max(t)));
        }
    }

    // Second pass: resolve missing timestamps by inheriting from neighbours
    // Forward pass: propagate previous word's end to words with no timestamps
    let mut last_valid_end = 0.0f64;
    let mut resolved: Vec<(f64, f64, String)> = Vec::with_capacity(words.len());

    for acc in &words {
        let (start, end) = match (acc.t_min, acc.t_max) {
            (Some(s), Some(e)) => {
                last_valid_end = e;
                (s, e)
            }
            _ => {
                // No valid DTW: inherit previous end for both start and end
                (last_valid_end, last_valid_end)
            }
        };
        resolved.push((start, end, acc.text.clone()));
    }

    resolved
}

/// Flatten per-segment token timings into words, fixing two issues over calling
/// `group_tokens_into_words` per segment: (a) if NO token across all segments has a
/// valid DTW timestamp, return empty so the caller falls back to segment-level
/// timestamps; (b) carry missing-DTW inheritance across segment boundaries by
/// grouping in a single pass. A word boundary is forced at each segment start so
/// words never merge across Whisper segments.
fn words_from_segments(segments: &[Vec<TokenTiming>]) -> Vec<(f64, f64, String)> {
    // (a) If no token in any segment has a valid DTW timestamp, return empty so
    // the caller can fall back to segment-level timestamps.
    let any_valid = segments
        .iter()
        .flatten()
        .any(|t| t.t_dtw.is_finite() && t.t_dtw >= 0.0);
    if !any_valid {
        return Vec::new();
    }

    // Build one flat Vec<TokenTiming>, injecting a leading space on each
    // segment's first content token (unless it already has one) to force a word
    // boundary at every segment start.
    let mut flat: Vec<TokenTiming> = Vec::new();
    for segment in segments {
        let mut first_content = true;
        for token in segment {
            // Skip special tokens (same predicate as group_tokens_into_words)
            let raw = &token.text;
            if raw.is_empty() || raw.starts_with('[') || raw.contains("<|") {
                // Include as-is so group_tokens_into_words can skip them too;
                // don't touch first_content for specials.
                flat.push(token.clone());
                continue;
            }
            if first_content {
                // Force a word boundary: prepend a space if not already present.
                let text = if token.text.starts_with(' ') {
                    token.text.clone()
                } else {
                    format!(" {}", token.text)
                };
                flat.push(TokenTiming {
                    t_dtw: token.t_dtw,
                    text,
                });
                first_content = false;
            } else {
                flat.push(token.clone());
            }
        }
    }

    // (b) Single-pass grouping carries last_valid_end across all segments.
    group_tokens_into_words(&flat)
}

#[cfg(test)]
#[path = "words_tests.rs"]
mod tests;
