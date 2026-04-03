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
        self.transcribe_chunk_timestamps(audio, language, 0.0, prompt)
    }

    /// Transcribe a single audio chunk (≤30s) with timestamps via DTW.
    ///
    /// Uses `set_no_timestamps(true)` for clean text generation, then derives
    /// per-segment timing from `token.t_dtw` (cross-attention DTW alignment)
    /// rather than the generative `<|t.xx|>` tokens that degrade quality.
    /// `t_offset` (seconds) is added to all returned timestamps.
    #[cfg(feature = "diarization")]
    fn transcribe_chunk_timestamps(
        &self,
        audio: &[f32],
        language: Language,
        t_offset: f64,
        prompt: Option<&str>,
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
        let mut results = Vec::with_capacity(n_segments as usize);

        for i in 0..n_segments {
            let Some(segment) = state.get_segment(i) else { continue };

            let text = segment.to_str().unwrap_or("").trim().to_string();
            if text.is_empty() {
                continue;
            }

            // Derive segment timing from DTW timestamps on first/last non-special token.
            // DTW timestamps are in centiseconds; -1 means not computed.
            // Fall back to segment-level timestamps if DTW was not available.
            let (t0, t1) = dtw_segment_timestamps(&segment, t_offset).unwrap_or_else(|| {
                let t0 = segment.start_timestamp() as f64 / 100.0 + t_offset;
                let t1 = segment.end_timestamp() as f64 / 100.0 + t_offset;
                (t0, t1)
            });

            results.push((t0, t1, text));
        }

        Ok(results)
    }
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

