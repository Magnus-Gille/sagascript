//! One transcription as data (#371): what to decode, how, and who watches.
//!
//! A [`TranscribeRequest`] replaces the old chain of `transcribe_*` methods,
//! each adding one argument to the next. The entry points on
//! [`super::WhisperBackend`] take a request and differ only in what they
//! return: segments, display text, or text inside a model-selecting dictation
//! transaction.

use tracing::info;

use crate::error::DictationError;
use crate::models::Language;

use super::TranscribeOptions;

/// Callbacks that observe a transcription.
#[derive(Default)]
pub struct TranscribeHooks<'a> {
    /// Percentage done, 0-100. Invoked from the whisper.cpp inference thread,
    /// first with `1` once work has started, then with strictly increasing
    /// values up to `100`.
    pub on_progress: Option<Box<dyn FnMut(i32) + Send + 'static>>,
    /// Fires once, when compute resources are held and mel/encoder work is
    /// about to begin (the silent window before the first progress callback).
    pub on_encode_start: Option<&'a dyn Fn()>,
}

/// The audio of one transcription, the language to decode it in, the
/// decoding options and the hooks that observe it.
pub struct TranscribeRequest<'a> {
    /// 16 kHz mono samples.
    pub audio: &'a [f32],
    pub language: Language,
    pub options: &'a TranscribeOptions,
    pub hooks: TranscribeHooks<'a>,
    /// Live microphone capture. Near-silent audio then yields an empty
    /// transcript without touching the decoder or loading a model, and empty or
    /// non-finite audio is rejected up front. File and diagnostic requests,
    /// and the intentional silent warm-up, leave this off so they stay raw.
    pub live: bool,
}

impl<'a> TranscribeRequest<'a> {
    /// A request without hooks, for a file or diagnostic transcription.
    pub fn new(audio: &'a [f32], language: Language, options: &'a TranscribeOptions) -> Self {
        Self {
            audio,
            language,
            options,
            hooks: TranscribeHooks::default(),
            live: false,
        }
    }

    /// Mark the request as live microphone capture; see [`Self::live`].
    pub fn live(mut self) -> Self {
        self.live = true;
        self
    }

    /// Observe the percentage done; see [`TranscribeHooks::on_progress`].
    pub fn on_progress(mut self, on_progress: impl FnMut(i32) + Send + 'static) -> Self {
        self.hooks.on_progress = Some(Box::new(on_progress));
        self
    }

    /// Observe the start of encoding; see [`TranscribeHooks::on_encode_start`].
    pub fn on_encode_start(mut self, on_encode_start: &'a dyn Fn()) -> Self {
        self.hooks.on_encode_start = Some(on_encode_start);
        self
    }

    /// Apply the live-capture guard once. `Ok(None)` means the audio is
    /// near-silent and there is nothing to transcribe; otherwise the request
    /// comes back with `live` cleared, so that a layer below does not apply
    /// the guard a second time.
    pub(super) fn after_live_guard(mut self) -> Result<Option<Self>, DictationError> {
        if !std::mem::take(&mut self.live) {
            return Ok(Some(self));
        }
        if has_live_audio_signal(self.audio)? {
            return Ok(Some(self));
        }
        info!(
            "Skipping near-silent dictation: {} samples",
            self.audio.len()
        );
        Ok(None)
    }
}

fn has_live_audio_signal(audio: &[f32]) -> Result<bool, DictationError> {
    if audio.is_empty() {
        return Err(DictationError::NoAudioCaptured);
    }
    crate::audio::speech::has_audio_signal(audio)
        .map_err(|reason| DictationError::TranscriptionFailed(reason.to_string()))
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
