//! `--progress-json` events (#343): the phase table and the writers. Events go
//! to stderr, one compact JSON object per line; stdout stays the transcript.

use std::time::Instant;

#[cfg(feature = "diarization")]
use sagascript_core::diarization_report::DecoderEvidence;

use crate::output::{errln, Out};

/// Where one file's `--progress-json` events go, and what they carry: the
/// stream, whether events are wanted, the file's start (their `elapsed_ms`
/// counts from it) and, for a diarized run, the effective decoder settings.
/// Owns what it needs, so a progress callback of another thread can hold a
/// clone.
#[derive(Clone)]
pub(super) struct Progress {
    out: Out,
    enabled: bool,
    started: Instant,
    decoder: Option<serde_json::Value>,
}

impl Progress {
    pub(super) fn new(
        out: &Out,
        enabled: bool,
        started: Instant,
        decoder: Option<serde_json::Value>,
    ) -> Self {
        Self {
            out: out.clone(),
            enabled,
            started,
            decoder,
        }
    }

    /// One phase event, with the decoder settings when the run has them.
    pub(super) fn phase(&self, phase: &str, percent: Option<u8>) {
        emit_progress_with_decoder(
            &self.out,
            self.enabled,
            self.started,
            phase,
            percent,
            self.decoder.as_ref(),
        );
    }

    /// One phase of a diarized run: indeterminate, without a step number.
    #[cfg(feature = "diarization")]
    pub(super) fn meeting_phase(&self, phase: &str) {
        emit_meeting_progress(
            &self.out,
            self.enabled,
            self.started,
            phase,
            self.decoder.as_ref(),
        );
    }
}

pub(super) fn progress_step(phase: &str) -> Option<u8> {
    match phase {
        "decoding" => Some(1),
        "resampling" | "loading" | "language_detection" | "preparing" | "encoding" => Some(2),
        "transcribing" | "finalizing" | "completed" => Some(3),
        _ => None,
    }
}

pub(super) fn progress_event(
    phase: &str,
    percent: Option<u8>,
    step: Option<u8>,
    steps_total: Option<u8>,
    elapsed_ms: u128,
) -> serde_json::Value {
    serde_json::json!({
        "event": "transcription_progress",
        "phase": phase,
        "step": step,
        "steps_total": steps_total,
        "percent": percent,
        "elapsed_ms": elapsed_ms,
    })
}

pub(super) fn progress_event_with_decoder(
    phase: &str,
    percent: Option<u8>,
    step: Option<u8>,
    steps_total: Option<u8>,
    elapsed_ms: u128,
    decoder: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut event = progress_event(phase, percent, step, steps_total, elapsed_ms);
    if let Some(decoder) = decoder {
        event["decoder"] = decoder.clone();
    }
    event
}

pub(super) fn emit_progress(
    out: &Out,
    enabled: bool,
    started: Instant,
    phase: &str,
    percent: Option<u8>,
) {
    if enabled {
        errln!(
            out,
            "{}",
            progress_event(
                phase,
                percent,
                progress_step(phase),
                Some(3),
                started.elapsed().as_millis(),
            )
        );
    }
}

pub(super) fn emit_progress_with_decoder(
    out: &Out,
    enabled: bool,
    started: Instant,
    phase: &str,
    percent: Option<u8>,
    decoder: Option<&serde_json::Value>,
) {
    if enabled {
        errln!(
            out,
            "{}",
            progress_event_with_decoder(
                phase,
                percent,
                progress_step(phase),
                Some(3),
                started.elapsed().as_millis(),
                decoder,
            )
        );
    }
}

#[cfg(feature = "diarization")]
pub(super) fn emit_meeting_progress(
    out: &Out,
    enabled: bool,
    started: Instant,
    phase: &str,
    decoder: Option<&serde_json::Value>,
) {
    if enabled {
        errln!(
            out,
            "{}",
            progress_event_with_decoder(
                phase,
                None,
                None,
                None,
                started.elapsed().as_millis(),
                decoder,
            )
        );
    }
}

#[cfg(feature = "diarization")]
pub(super) fn decoder_progress_value(
    decoder: &DecoderEvidence,
    meeting_json: bool,
) -> serde_json::Value {
    if meeting_json {
        // Native meeting JSON serializes typed f32 fields directly. Match that
        // wire representation without changing the legacy JSON report format.
        let native = serde_json::to_string(decoder).expect("decoder evidence serializes");
        serde_json::from_str(&native).expect("serialized decoder evidence is valid JSON")
    } else {
        serde_json::to_value(decoder).expect("decoder evidence serializes")
    }
}
