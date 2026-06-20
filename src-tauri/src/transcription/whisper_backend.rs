use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};
#[cfg(feature = "diarization")]
use whisper_rs::{DtwMode, DtwParameters};

use crate::error::DictationError;
use crate::settings::{Language, WhisperModel};
use crate::transcription::model;

/// Local transcription backend using whisper-rs (whisper.cpp bindings)
/// Uses GGML model files with optional CoreML acceleration on macOS.
///
/// This is managed as a separate Tauri state (not inside AppController)
/// because transcription is blocking and we must not hold the AppController
/// lock across async boundaries.
pub struct WhisperBackend {
    /// Loaded whisper context (model weights). None until load_model() is called.
    context: Mutex<Option<WhisperContext>>,
    /// Currently loaded model
    loaded_model: Mutex<Option<WhisperModel>>,
    /// Abort flag — set to true to cancel in-progress transcription
    abort_flag: Arc<AtomicBool>,
}

// WhisperContext is Send+Sync (it wraps a C pointer that's thread-safe)
// The Mutex handles interior mutability safely
unsafe impl Send for WhisperBackend {}
unsafe impl Sync for WhisperBackend {}

impl WhisperBackend {
    pub fn new() -> Self {
        Self {
            context: Mutex::new(None),
            loaded_model: Mutex::new(None),
            abort_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Signal the whisper inference to abort.
    ///
    /// NOTE: The abort callback that reads this flag is currently disabled
    /// (whisper-rs 0.15.1 `set_abort_callback_safe` causes error -6).
    /// This flag is still set for forward compatibility but has no effect
    /// on in-progress inference until the callback is restored.
    pub fn request_abort(&self) {
        warn!("Transcription abort requested (NOTE: abort callback disabled — inference will run to completion)");
        self.abort_flag.store(true, Ordering::SeqCst);
    }

    /// Load a specific model, replacing any previously loaded model
    pub fn load_model(&self, whisper_model: WhisperModel) -> Result<(), DictationError> {
        let model_path = model::model_path(whisper_model);

        if !model_path.exists() {
            return Err(DictationError::TranscriptionFailed(format!(
                "Model '{}' not downloaded. Please download it from Settings first.",
                whisper_model.display_name()
            )));
        }

        info!(
            "Loading whisper model: {} from {}",
            whisper_model.display_name(),
            model_path.display()
        );

        // Enable DTW for attention-based token timestamps (used by --diarize).
        // DTW is incompatible with flash_attn; flash_attn defaults to false so this is safe.
        let ctx_params = {
            #[allow(unused_mut)]
            let mut p = WhisperContextParameters::default();
            #[cfg(feature = "diarization")]
            p.dtw_parameters(DtwParameters {
                mode: DtwMode::ModelPreset {
                    model_preset: whisper_model.dtw_preset(),
                },
                ..DtwParameters::default()
            });
            p
        };

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or_else(|| {
                DictationError::TranscriptionFailed("Invalid model path".to_string())
            })?,
            ctx_params,
        )
        .map_err(|e| {
            DictationError::TranscriptionFailed(format!("Failed to load model: {e}"))
        })?;

        *self.context.lock().unwrap() = Some(ctx);
        *self.loaded_model.lock().unwrap() = Some(whisper_model);

        info!("Model loaded: {}", whisper_model.display_name());
        Ok(())
    }

    /// Get the currently loaded model
    pub fn loaded_model(&self) -> Option<WhisperModel> {
        *self.loaded_model.lock().unwrap()
    }

    /// Check if the correct model is loaded for the given settings
    pub fn needs_reload(&self, desired_model: WhisperModel) -> bool {
        self.loaded_model() != Some(desired_model)
    }

    /// Ensure the correct model is loaded
    pub fn ensure_model(&self, desired_model: WhisperModel) -> Result<(), DictationError> {
        if self.needs_reload(desired_model) {
            info!("Loading model: {:?}", desired_model);
            self.load_model(desired_model)?;
        }
        Ok(())
    }

    /// Run transcription on loaded model (blocking — call from spawn_blocking)
    pub fn transcribe_sync(
        &self,
        audio: &[f32],
        language: Language,
    ) -> Result<String, DictationError> {
        self.transcribe_sync_with_progress(audio, language, |_| {})
    }

    /// Run transcription with a progress callback that receives percentage (0–100).
    /// The callback is invoked from the whisper.cpp inference thread.
    pub fn transcribe_sync_with_progress(
        &self,
        audio: &[f32],
        language: Language,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<String, DictationError> {
        self.transcribe_sync_with_progress_and_prompt(audio, language, None, on_progress)
    }

    /// Like `transcribe_sync_with_progress` but accepts an optional initial prompt
    /// to prime the decoder with domain-specific vocabulary and context.
    pub fn transcribe_sync_with_progress_and_prompt(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
        on_progress: impl FnMut(i32) + 'static,
    ) -> Result<String, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }

        let ctx_guard = self.context.lock().unwrap();
        let ctx = ctx_guard
            .as_ref()
            .ok_or(DictationError::ModelNotLoaded)?;

        let model = self
            .loaded_model()
            .ok_or(DictationError::ModelNotLoaded)?;

        let n_threads = (num_cpus::get() / 2).max(1) as i32;
        let no_speech_thold = model.no_speech_threshold();

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(language.whisper_code());
        params.set_n_threads(n_threads);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_translate(false);
        params.set_no_timestamps(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(no_speech_thold);
        params.set_suppress_blank(true);
        if let Some(p) = prompt {
            params.set_initial_prompt(p);
        }
        params.set_progress_callback_safe(on_progress);

        // Abort callback: DISABLED — whisper-rs 0.15.1 set_abort_callback_safe()
        // causes error -6 on all models. Without this callback, request_abort() sets
        // the flag but nothing reads it during inference. If whisper hangs, the context
        // mutex remains held until inference completes naturally. The tokio timeout in
        // commands.rs / main.rs will return an error to the caller, but the blocking
        // task continues running in the background.
        // TODO: Restore when whisper-rs fixes the FFI callback lifetime issue.
        self.abort_flag.store(false, Ordering::SeqCst);
        // let abort = Arc::clone(&self.abort_flag);
        // params.set_abort_callback_safe(move || abort.load(Ordering::SeqCst));

        info!(
            "Starting local transcription: {} samples, {} threads, lang={:?}, no_speech_thold={}",
            audio.len(),
            n_threads,
            language,
            no_speech_thold
        );

        let mut state = ctx.create_state().map_err(|e| {
            DictationError::TranscriptionFailed(format!("Failed to create whisper state: {e}"))
        })?;

        state.full(params, audio).map_err(|e| {
            DictationError::TranscriptionFailed(format!("Whisper inference failed: {e}"))
        })?;

        let n_segments = state.full_n_segments();

        let mut transcript = String::new();
        for i in 0..n_segments {
            if let Some(segment) = state.get_segment(i) {
                if let Ok(text) = segment.to_str() {
                    transcript.push_str(text);
                }
            }
        }

        let result = transcript.trim().to_string();
        info!("Local transcription complete: {} chars", result.len());
        Ok(result)
    }

    /// Transcribe audio and return per-segment timestamps.
    ///
    /// For audio longer than 30s, uses overlapping 30s windows to prevent
    /// the Whisper decoder from drifting or looping on long recordings.
    #[cfg(feature = "diarization")]
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
        self.transcribe_chunk_timestamps(audio, language, 0.0, prompt, Granularity::Segment)
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
    #[cfg(feature = "diarization")]
    pub fn transcribe_sync_with_word_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        prompt: Option<&str>,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        if audio.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        self.transcribe_chunk_timestamps(audio, language, 0.0, prompt, Granularity::Word)
    }

    /// Transcribe a single audio chunk with timestamps via DTW.
    ///
    /// `granularity` controls whether to emit one entry per Whisper segment
    /// (`Segment`) or one entry per word (`Word`).
    ///
    /// Uses `set_no_timestamps(true)` for clean text generation, then derives
    /// timing from `token.t_dtw` (cross-attention DTW alignment) rather than
    /// the generative `<|t.xx|>` tokens that degrade quality.
    /// `t_offset` (seconds) is added to all returned timestamps.
    #[cfg(feature = "diarization")]
    fn transcribe_chunk_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        t_offset: f64,
        prompt: Option<&str>,
        granularity: Granularity,
    ) -> Result<Vec<(f64, f64, String)>, DictationError> {
        let ctx_guard = self.context.lock().unwrap();
        let ctx = ctx_guard
            .as_ref()
            .ok_or(DictationError::ModelNotLoaded)?;

        let model = self
            .loaded_model()
            .ok_or(DictationError::ModelNotLoaded)?;

        let n_threads = (num_cpus::get() / 2).max(1) as i32;
        let no_speech_thold = model.no_speech_threshold();

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(language.whisper_code());
        params.set_n_threads(n_threads);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.2);
        params.set_translate(false);
        params.set_no_timestamps(true);   // clean text — no generative timestamp tokens
        params.set_token_timestamps(true); // populate token.t0/t1/t_dtw via cross-attention
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_no_speech_thold(no_speech_thold);
        params.set_suppress_blank(true);
        if let Some(p) = prompt {
            params.set_initial_prompt(p);
        }

        let mut state = ctx.create_state().map_err(|e| {
            DictationError::TranscriptionFailed(format!("Failed to create whisper state: {e}"))
        })?;

        state.full(params, audio).map_err(|e| {
            DictationError::TranscriptionFailed(format!("Whisper inference failed: {e}"))
        })?;

        let n_segments = state.full_n_segments();
        let mut results: Vec<(f64, f64, String)> = Vec::new();

        match granularity {
            Granularity::Segment => {
                for i in 0..n_segments {
                    let Some(segment) = state.get_segment(i) else { continue };
                    let text = segment.to_str().unwrap_or("").trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    let (t0, t1) = dtw_segment_timestamps(&segment, t_offset).unwrap_or_else(|| {
                        let t0 = segment.start_timestamp() as f64 / 100.0 + t_offset;
                        let t1 = segment.end_timestamp() as f64 / 100.0 + t_offset;
                        (t0, t1)
                    });
                    results.push((t0, t1, text));
                }
            }
            Granularity::Word => {
                // Collect per-token timings for ALL segments first, then group
                // in a single pass so cross-segment DTW inheritance is correct.
                let mut segs: Vec<Vec<TokenTiming>> = Vec::with_capacity(n_segments as usize);
                for i in 0..n_segments {
                    let Some(segment) = state.get_segment(i) else { continue };
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
            }
        }

        Ok(results)
    }
}

/// Controls whether `transcribe_chunk_timestamps` emits one entry per segment or per word.
#[cfg(feature = "diarization")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Granularity {
    Segment,
    Word,
}

/// Extract segment timing from per-token DTW timestamps.
///
/// Returns `(start_secs, end_secs)` using the DTW timestamps of the first and last
/// content tokens (skipping special tokens which have t_dtw = -1).
/// Returns `None` if no token has a valid DTW timestamp.
#[cfg(feature = "diarization")]
fn dtw_segment_timestamps(
    segment: &whisper_rs::WhisperSegment<'_>,
    t_offset: f64,
) -> Option<(f64, f64)> {
    let n = segment.n_tokens();
    let mut t0 = None::<f64>;
    let mut t1 = None::<f64>;

    for j in 0..n {
        let Some(token) = segment.get_token(j) else { continue };
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
#[cfg(feature = "diarization")]
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
#[cfg(feature = "diarization")]
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
#[cfg(feature = "diarization")]
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
                flat.push(TokenTiming { t_dtw: token.t_dtw, text });
                first_content = false;
            } else {
                flat.push(token.clone());
            }
        }
    }

    // (b) Single-pass grouping carries last_valid_end across all segments.
    group_tokens_into_words(&flat)
}

#[cfg(all(test, feature = "diarization"))]
mod word_grouping_tests {
    use super::{TokenTiming, group_tokens_into_words, words_from_segments};

    fn tok(text: &str, t_dtw: f64) -> TokenTiming {
        TokenTiming { t_dtw, text: text.to_string() }
    }

    fn tok_invalid(text: &str) -> TokenTiming {
        TokenTiming { t_dtw: -1.0, text: text.to_string() }
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(group_tokens_into_words(&[]).is_empty());
    }

    #[test]
    fn single_token_is_one_word() {
        let tokens = vec![tok(" Hello", 1.0)];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].2, "Hello");
        assert!((words[0].0 - 1.0).abs() < 1e-9);
        assert!((words[0].1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn leading_space_creates_word_boundary() {
        // " Hello" starts word 1, " world" starts word 2
        let tokens = vec![
            tok(" Hello", 1.0),
            tok(" world", 2.0),
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].2, "Hello");
        assert_eq!(words[1].2, "world");
    }

    #[test]
    fn punctuation_attaches_to_prior_word() {
        // "," has no leading space → appends to "Hello"
        let tokens = vec![
            tok(" Hello", 1.0),
            tok(",", 1.2),
            tok(" world", 2.0),
            tok(".", 2.1),
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].2, "Hello,");
        assert_eq!(words[1].2, "world.");
    }

    #[test]
    fn multi_token_word_gets_min_max_timing() {
        // Word "un" composed of two tokens at t=1.0 and t=1.5
        let tokens = vec![
            tok(" un", 1.0),
            tok("e", 1.5),  // no leading space → appends
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].2, "une");
        assert!((words[0].0 - 1.0).abs() < 1e-9, "start should be min");
        assert!((words[0].1 - 1.5).abs() < 1e-9, "end should be max");
    }

    #[test]
    fn special_tokens_skipped() {
        let tokens = vec![
            tok(" Hello", 1.0),
            tok("[_BEG_]", 0.0),     // bracket special → skip
            tok("<|nospeech|>", 0.5), // angle bracket special → skip
            tok("",  0.0),            // empty → skip
            tok(" world", 2.0),
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2, "specials should be skipped");
        assert_eq!(words[0].2, "Hello");
        assert_eq!(words[1].2, "world");
    }

    #[test]
    fn invalid_dtw_word_inherits_previous_end() {
        // Word 1: t=1.0, Word 2: no valid DTW → should inherit 1.0
        let tokens = vec![
            tok(" Hello", 1.0),
            tok_invalid(" world"),
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 2);
        assert!((words[1].0 - 1.0).abs() < 1e-9, "start should inherit previous end");
        assert!((words[1].1 - 1.0).abs() < 1e-9, "end should inherit previous end");
        assert_eq!(words[1].2, "world");
    }

    #[test]
    fn first_content_token_without_leading_space_starts_word() {
        // First token has no leading space but should still create a word
        let tokens = vec![tok("Bonjour", 0.5)];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].2, "Bonjour");
        assert!((words[0].0 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn start_never_exceeds_end() {
        // Two valid tokens at the same time → start == end
        let tokens = vec![
            tok(" test", 3.0),
            tok("ing", 3.0),
        ];
        let words = group_tokens_into_words(&tokens);
        assert_eq!(words.len(), 1);
        assert!(words[0].0 <= words[0].1, "start must not exceed end");
    }

    // ---- words_from_segments tests ----

    fn seg(tokens: Vec<TokenTiming>) -> Vec<TokenTiming> {
        tokens
    }

    /// (a) All tokens invalid across all segments → returns empty (bug #1 fix).
    #[test]
    fn words_from_segments_all_invalid_returns_empty() {
        let segs = vec![
            seg(vec![tok_invalid(" Hello"), tok_invalid(" world")]),
            seg(vec![tok_invalid(" foo"), tok_invalid(" bar")]),
        ];
        let result = words_from_segments(&segs);
        assert!(result.is_empty(), "should be empty when no valid DTW exists, got: {result:?}");
    }

    /// (b) Second segment's first content word has invalid DTW → inherits from
    /// last valid end of segment 1, NOT 0.0 (bug #2 fix).
    /// Asserts monotonic non-decreasing timestamps.
    #[test]
    fn words_from_segments_cross_segment_inheritance_is_monotonic() {
        let segs = vec![
            // Segment 0: one valid word at t=2.0
            seg(vec![tok(" Hello", 2.0)]),
            // Segment 1: first word has no valid DTW, should inherit 2.0 not 0.0
            seg(vec![tok_invalid(" world"), tok(" there", 3.0)]),
        ];
        let result = words_from_segments(&segs);
        // Should have 3 words: "Hello", "world", "there"
        assert_eq!(result.len(), 3, "expected 3 words, got: {result:?}");

        // All timestamps must be monotonic non-decreasing
        for i in 1..result.len() {
            assert!(
                result[i].0 >= result[i - 1].0,
                "start[{i}]={} < start[{}]={} — not monotonic",
                result[i].0, i - 1, result[i - 1].0
            );
            assert!(
                result[i].1 >= result[i - 1].1,
                "end[{i}]={} < end[{}]={} — not monotonic",
                result[i].1, i - 1, result[i - 1].1
            );
        }

        // "world" should inherit segment 0's last valid end (2.0), not 0.0
        let world = &result[1];
        assert_eq!(world.2, "world");
        assert!(
            (world.0 - 2.0).abs() < 1e-9,
            "world start should be 2.0 (inherited), got {}",
            world.0
        );
    }

    /// (c) Second segment's first token lacks a leading space → still starts a
    /// NEW word (no cross-segment merge).
    #[test]
    fn words_from_segments_segment_boundary_forces_word_break() {
        let segs = vec![
            // Segment 0: word "Hello" (token without leading space but first of seg)
            seg(vec![tok("Hello", 1.0)]),
            // Segment 1: "world" also lacks a leading space — must NOT merge with "Hello"
            seg(vec![tok("world", 2.0)]),
        ];
        let result = words_from_segments(&segs);
        assert_eq!(result.len(), 2, "segment boundary must create word break; got: {result:?}");
        assert_eq!(result[0].2, "Hello");
        assert_eq!(result[1].2, "world");
    }

    /// (d) First segment all-invalid, later segment has valid DTW → any_valid is
    /// true so result is non-empty.
    #[test]
    fn words_from_segments_any_valid_in_later_segment_returns_words() {
        let segs = vec![
            // Segment 0: all invalid
            seg(vec![tok_invalid(" silence")]),
            // Segment 1: valid
            seg(vec![tok(" speech", 5.0)]),
        ];
        let result = words_from_segments(&segs);
        assert!(!result.is_empty(), "should have words when any segment has valid DTW");
        // "silence" inherits 0.0 (nothing before it), "speech" has t=5.0
        let speech = result.iter().find(|w| w.2 == "speech").expect("should have 'speech'");
        assert!((speech.0 - 5.0).abs() < 1e-9, "speech start should be 5.0, got {}", speech.0);
    }
}

