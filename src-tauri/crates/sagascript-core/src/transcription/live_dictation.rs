//! Per-operation backend selection for live microphone dictation.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::error::DictationError;
use crate::models::{Language, WhisperModel};
use crate::settings::Settings;

use super::engine_host::{CancelToken, EngineHostClient};
use super::pianissimo_backend::{
    cancelled_error, map_engine_error, shared_client, warm_client, with_cancel_bridge,
};
use super::whisper_backend::DictationTimings;
use super::{TranscribeOptions, TranscribeRequest, WhisperBackend};

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
    engine: Engine,
    cancelled: AtomicBool,
    active_job: Mutex<Option<CancelToken>>,
}

/// What an operation transcribes with.
enum Engine {
    Whisper,
    /// The process-global engine host. `load_settings` is called when the
    /// operation attaches to it (see [`LiveDictationBackend::new`]).
    Pianissimo {
        load_settings: fn() -> Settings,
    },
    /// An explicit engine client (tests).
    Client(EngineHostClient),
}

impl LiveDictationBackend {
    /// `pianissimo` selects the engine of the operation. For Pianissimo,
    /// `load_settings` (`machine::load`) is called when the operation attaches
    /// to the shared engine host, after the audio has been checked and not
    /// before: the idle setting that engine is configured from is read at that
    /// moment, as `shared_client` always did. It is never called for Whisper,
    /// so Whisper dictation reads no settings file.
    pub fn new(
        whisper: Arc<WhisperBackend>,
        pianissimo: bool,
        load_settings: fn() -> Settings,
    ) -> Self {
        Self::with(
            whisper,
            if pianissimo {
                Engine::Pianissimo { load_settings }
            } else {
                Engine::Whisper
            },
        )
    }

    /// Pianissimo dictation on an explicit engine client instead of the
    /// process-global one.
    pub fn with_engine(whisper: Arc<WhisperBackend>, engine: EngineHostClient) -> Self {
        Self::with(whisper, Engine::Client(engine))
    }

    fn with(whisper: Arc<WhisperBackend>, engine: Engine) -> Self {
        Self {
            whisper,
            engine,
            cancelled: AtomicBool::new(false),
            active_job: Mutex::new(None),
        }
    }

    fn is_pianissimo(&self) -> bool {
        !matches!(self.engine, Engine::Whisper)
    }

    pub fn transcribe(
        &self,
        model: WhisperModel,
        samples: &[f32],
        language: Language,
        options: &TranscribeOptions,
        timings: &mut DictationTimings,
    ) -> Result<String, DictationError> {
        if !self.is_pianissimo() {
            return self.whisper.transcribe_dictation(
                model,
                TranscribeRequest::new(samples, language, options).live(),
                timings,
            );
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
                Engine::Client(client) => Ok(client.clone()),
                Engine::Pianissimo { load_settings } => shared_client(&load_settings()),
                Engine::Whisper => unreachable!("handled before the audio checks"),
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
        let snapshot = client.snapshot();
        tracing::info!(
            samples = samples.len(),
            was_warm = warm.was_warm,
            model_ms = timings.model_ms as u64,
            inference_ms = timings.inference_ms as u64,
            pid = snapshot.pid,
            spawn_count = snapshot.spawns,
            load_count = snapshot.loads,
            ok = result.is_ok(),
            "Pianissimo dictation request finished"
        );
        if let Ok(transcription) = &result {
            // Splits the client-observed wait into host stages plus IPC/PCM overhead
            // (round_trip_ms minus the stage sum).
            for window in &transcription.windows {
                tracing::info!(
                    audio_s = transcription.audio_s,
                    inference_ms = timings.inference_ms as u64,
                    preprocess_ms = window.preprocess_ms,
                    encode_ms = window.encode_ms,
                    decode_ms = window.decode_ms,
                    round_trip_ms = window.round_trip_ms,
                    pcm_and_ipc_overhead_ms =
                        (timings.inference_ms as u64).saturating_sub(window.round_trip_ms),
                    tokens = window.tokens,
                    "Pianissimo dictation window timings"
                );
            }
        }
        result
            .map(|transcription| transcription.text)
            .map_err(map_engine_error)
    }

    /// Abort this operation. Cancellation is sticky, covering requests made
    /// before the engine is reached as well as requests during inference.
    pub fn request_abort(&self) {
        if self.is_pianissimo() {
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
    use std::sync::atomic::AtomicUsize;

    /// For the paths that must not reach the settings: calling it fails the test.
    fn settings_must_not_be_read() -> Settings {
        panic!("the settings file was read where it must not be");
    }

    fn pianissimo_that_reads_no_settings() -> LiveDictationBackend {
        LiveDictationBackend::new(
            Arc::new(WhisperBackend::new()),
            true,
            settings_must_not_be_read,
        )
    }

    #[test]
    fn pianissimo_silence_does_not_load_or_infer() {
        let backend = pianissimo_that_reads_no_settings();
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
        let backend = pianissimo_that_reads_no_settings();
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
        let backend = pianissimo_that_reads_no_settings();
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
        let backend = pianissimo_that_reads_no_settings();
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

    /// When the settings are read (#372): where `shared_client` used to read them,
    /// as the operation attaches to the engine host, and never on the Whisper path.
    #[test]
    fn pianissimo_reads_the_settings_once_when_it_attaches_to_the_engine() {
        static READS: AtomicUsize = AtomicUsize::new(0);
        fn counted() -> Settings {
            READS.fetch_add(1, Ordering::SeqCst);
            Settings::default()
        }
        let backend = LiveDictationBackend::new(Arc::new(WhisperBackend::new()), true, counted);
        let mut timings = DictationTimings::default();
        // Audio with a signal gets past the checks to the engine; this host has no
        // engine to run (unsupported, or no bundled host), so the attach fails.
        let result = backend.transcribe(
            WhisperModel::Base,
            &[0.1; 1_600],
            Language::Swedish,
            &TranscribeOptions::default(),
            &mut timings,
        );
        assert!(result.is_err());
        assert!(timings.model_acquisition_started);
        assert_eq!(READS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn whisper_dictation_never_reads_the_settings() {
        let backend = LiveDictationBackend::new(
            Arc::new(WhisperBackend::new()),
            false,
            settings_must_not_be_read,
        );
        let mut timings = DictationTimings::default();
        // Silence is skipped by Whisper's own guard before any model is needed;
        // the point is that the Whisper path gets that far without the settings.
        let text = backend
            .transcribe(
                WhisperModel::Base,
                &[0.0; 1_600],
                Language::Swedish,
                &TranscribeOptions::default(),
                &mut timings,
            )
            .unwrap();
        assert!(text.is_empty());
    }
}
