//! Per-operation backend selection for live microphone dictation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::error::DictationError;
use crate::settings::{Language, WhisperModel};

use super::pianissimo_backend::PianissimoBackend;
use super::whisper_backend::DictationTimings;
use super::{TranscribeOptions, WhisperBackend};

/// Routes one live dictation operation to Whisper or Swedish Pianissimo.
///
/// Instances are intended to belong to one operation, so cancellation cannot
/// leak from one recording into a later one. Whisper receives the original
/// model, options, and timings unchanged. Pianissimo starts its native runtime
/// for the operation and ignores Whisper-only decoder settings.
pub struct LiveDictationBackend {
    whisper: Arc<WhisperBackend>,
    pianissimo: bool,
    cancelled: AtomicBool,
    active_pianissimo: Mutex<Option<Arc<PianissimoBackend>>>,
}

impl LiveDictationBackend {
    pub fn new(whisper: Arc<WhisperBackend>, pianissimo: bool) -> Self {
        Self {
            whisper,
            pianissimo,
            cancelled: AtomicBool::new(false),
            active_pianissimo: Mutex::new(None),
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

        // Startup verifies the downloaded artifact; it does not load the
        // recognizer. The native command loads and infers in one process with
        // no stage markers, so keep model_ready_at unset and report the full
        // subprocess duration as inference latency.
        timings.model_cached = false;
        timings.model_acquisition_started = true;
        let model_started = Instant::now();
        let backend_result = PianissimoBackend::start_with_cancel(&self.cancelled);
        timings.model_ms = model_started.elapsed().as_secs_f64() * 1000.0;
        let backend = Arc::new(backend_result?);
        {
            let mut active = self.active_pianissimo.lock().map_err(|_| {
                DictationError::TranscriptionFailed("Pianissimo backend lock was poisoned".into())
            })?;
            *active = Some(Arc::clone(&backend));
        }

        if self.cancelled.load(Ordering::SeqCst) {
            backend.request_abort();
            self.clear_active_pianissimo();
            return Err(cancelled_error());
        }

        // The selected model and decoder options belong to Whisper. Pianissimo
        // uses its fixed Swedish Pianissimo model and native decoder defaults.
        let _ = (model, options);
        timings.inference_started = true;
        let inference_started = Instant::now();
        let result = backend.transcribe_with_cancel(samples, |_| {}, &self.cancelled);
        timings.inference_ms = inference_started.elapsed().as_secs_f64() * 1000.0;
        self.clear_active_pianissimo();
        result.map(|transcript| transcript.text)
    }

    /// Abort this operation. Native cancellation is sticky, covering requests
    /// made before process startup as well as requests during the subprocess.
    pub fn request_abort(&self) {
        if self.pianissimo {
            self.cancelled.store(true, Ordering::SeqCst);
            if let Ok(active) = self.active_pianissimo.lock() {
                if let Some(backend) = active.as_ref() {
                    backend.request_abort();
                }
            }
        } else {
            self.whisper.request_abort();
        }
    }

    fn clear_active_pianissimo(&self) {
        if let Ok(mut active) = self.active_pianissimo.lock() {
            let _ = active.take();
        }
    }
}

fn cancelled_error() -> DictationError {
    DictationError::TranscriptionFailed("Pianissimo transcription cancelled".into())
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
