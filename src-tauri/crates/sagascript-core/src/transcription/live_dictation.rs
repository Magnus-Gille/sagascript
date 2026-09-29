//! Per-operation backend selection for live microphone dictation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::error::DictationError;
use crate::settings::{Language, WhisperModel};

use super::engine_host::{CancelToken, EngineHostClient};
use super::pianissimo_backend::{
    cancelled_error, map_engine_error, shared_client, warm_client, with_cancel_bridge,
};
use super::whisper_backend::DictationTimings;
use super::{TranscribeOptions, WhisperBackend};

/// Routes one live dictation operation to Whisper or Swedish Pianissimo.
///
/// Instances are intended to belong to one operation, so cancellation cannot
/// leak from one recording into a later one. Whisper receives the original
/// model, options, and timings unchanged. Pianissimo uses the process-global
/// engine host (the model stays loaded between utterances, so a warm
/// utterance spends no time on model acquisition) with interactive priority
/// and ignores Whisper-only decoder settings.
pub struct LiveDictationBackend {
    whisper: Arc<WhisperBackend>,
    pianissimo: bool,
    /// Explicit engine client (tests); `None` uses the process-global one.
    engine: Option<EngineHostClient>,
    cancelled: AtomicBool,
    active_job: Mutex<Option<CancelToken>>,
}

impl LiveDictationBackend {
    pub fn new(whisper: Arc<WhisperBackend>, pianissimo: bool) -> Self {
        Self {
            whisper,
            pianissimo,
            engine: None,
            cancelled: AtomicBool::new(false),
            active_job: Mutex::new(None),
        }
    }

    /// Pianissimo dictation on an explicit engine client instead of the
    /// process-global one.
    pub fn with_engine(whisper: Arc<WhisperBackend>, engine: EngineHostClient) -> Self {
        Self {
            engine: Some(engine),
            ..Self::new(whisper, true)
        }
    }

    pub fn transcribe(
        &self,
        model: WhisperModel,
        samples: &[f32],
        language: Language,
        options: &TranscribeOptions,
        timings: &mut DictationTimings,
    ) -> Result<String, DictationError> {
        if !self.pianissimo {
            return self
                .whisper
                .transcribe_live_dictation(model, samples, language, options, timings);
        }

        *timings = DictationTimings::default();
        if language != Language::Swedish {
            return Err(DictationError::TranscriptionFailed(
                "Pianissimo live dictation is only available for Swedish".into(),
            ));
        }
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(cancelled_error());
        }

        if samples.is_empty() {
            return Err(DictationError::NoAudioCaptured);
        }
        if !crate::audio::has_audio_signal(samples)
            .map_err(|reason| DictationError::TranscriptionFailed(reason.to_string()))?
        {
            return Ok(String::new());
        }

        // The cancel token exists before the model is acquired, so an abort
        // during a first-use compile/load is observed immediately (without
        // killing the host: the load still finishes for next time).
        let token = CancelToken::new();
        {
            let mut active = self.active_job.lock().map_err(|_| {
                DictationError::TranscriptionFailed("Pianissimo backend lock was poisoned".into())
            })?;
            *active = Some(token.clone());
        }

        // Model acquisition = attaching to the shared host and making sure the
        // model is loaded. When warm this is a state check (about 0 ms).
        timings.model_acquisition_started = true;
        let model_started = Instant::now();
        let acquired = with_cancel_bridge(&self.cancelled, &token, || {
            match &self.engine {
                Some(client) => Ok(client.clone()),
                None => shared_client(),
            }
            .and_then(|client| warm_client(&client, &token).map(|warm| (client, warm)))
        });
        timings.model_ms = model_started.elapsed().as_secs_f64() * 1000.0;
        let (client, warm) = match acquired {
            Ok(acquired) => acquired,
            Err(error) => {
                self.clear_active_job();
                return Err(error);
            }
        };
        timings.model_cached = warm.was_warm;
        timings.model_ready_at = Some(Instant::now());

        if self.cancelled.load(Ordering::SeqCst) {
            self.clear_active_job();
            return Err(cancelled_error());
        }

        // The selected model and decoder options belong to Whisper. Pianissimo
        // uses its fixed Swedish model and decoder defaults.
        let _ = (model, options);
        timings.inference_started = true;
        let inference_started = Instant::now();
        let result = with_cancel_bridge(&self.cancelled, &token, || {
            client.transcribe_dictation(samples, &token)
        });
        timings.inference_ms = inference_started.elapsed().as_secs_f64() * 1000.0;
        self.clear_active_job();
        result
            .map(|transcription| transcription.text)
            .map_err(map_engine_error)
    }

    /// Abort this operation. Cancellation is sticky, covering requests made
    /// before the engine is reached as well as requests during inference.
    pub fn request_abort(&self) {
        if self.pianissimo {
            self.cancelled.store(true, Ordering::SeqCst);
            if let Ok(active) = self.active_job.lock() {
                if let Some(token) = active.as_ref() {
                    token.cancel();
                }
            }
        } else {
            self.whisper.request_abort();
        }
    }

    fn clear_active_job(&self) {
        if let Ok(mut active) = self.active_job.lock() {
            let _ = active.take();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::WhisperBackend;

    #[test]
    fn pianissimo_silence_does_not_load_or_infer() {
        let backend = LiveDictationBackend::new(Arc::new(WhisperBackend::new()), true);
        for audio in [vec![0.0; 1600], vec![0.00001; 1600]] {
            let mut timings = DictationTimings::default();
            let result = backend
                .transcribe(
                    WhisperModel::Base,
                    &audio,
                    Language::Swedish,
                    &TranscribeOptions::default(),
                    &mut timings,
                )
                .unwrap();
            assert!(result.is_empty());
            assert!(!timings.model_acquisition_started);
            assert!(!timings.inference_started);
        }
    }

    #[test]
    fn pianissimo_invalid_capture_never_starts_model_work() {
        let backend = LiveDictationBackend::new(Arc::new(WhisperBackend::new()), true);
        for audio in [vec![], vec![f32::NAN; 320], vec![f32::INFINITY; 320]] {
            let mut timings = DictationTimings::default();
            assert!(backend
                .transcribe(
                    WhisperModel::Base,
                    &audio,
                    Language::Swedish,
                    &TranscribeOptions::default(),
                    &mut timings
                )
                .is_err());
            assert!(!timings.model_acquisition_started);
            assert!(!timings.inference_started);
        }
    }

    #[test]
    fn cancellation_before_native_start_does_not_require_a_model() {
        let backend = LiveDictationBackend::new(Arc::new(WhisperBackend::new()), true);
        backend.request_abort();

        let mut timings = DictationTimings::default();
        let error = backend
            .transcribe(
                WhisperModel::Base,
                &[0.1; 1_600],
                Language::Swedish,
                &TranscribeOptions::default(),
                &mut timings,
            )
            .unwrap_err();

        assert!(error.to_string().contains("cancelled"));
        assert!(!timings.inference_started);
        assert!(!timings.model_cached);
    }

    #[test]
    fn pianissimo_adapter_rejects_non_swedish_without_starting_runtime() {
        let backend = LiveDictationBackend::new(Arc::new(WhisperBackend::new()), true);
        let mut timings = DictationTimings::default();

        let error = backend
            .transcribe(
                WhisperModel::Base,
                &[0.1; 1_600],
                Language::Norwegian,
                &TranscribeOptions::default(),
                &mut timings,
            )
            .unwrap_err();

        assert!(error.to_string().contains("only available for Swedish"));
        assert!(!timings.inference_started);
    }
}
