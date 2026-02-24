use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tracing::{info, warn};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

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

    /// Signal the whisper inference to abort. The abort takes effect at the
    /// next whisper.cpp checkpoint (typically once per audio segment).
    pub fn request_abort(&self) {
        warn!("Transcription abort requested");
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

        let ctx_params = WhisperContextParameters::default();

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
        params.set_progress_callback_safe(on_progress);

        // Wire abort flag so timeouts can cancel inference mid-flight
        // TEMPORARILY DISABLED for A/B test — investigating error -6
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
}
