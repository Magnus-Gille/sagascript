//! The system-audio capture seam (phase 2, design §4.2).
//!
//! [`SystemSource::open`] is where the macOS process tap and the Windows
//! WASAPI loopback plug in. Until those backends land it reports
//! `recording_source_unavailable` everywhere, so the rest of phase 2 (source
//! selection, two tracks, CLI) is built and tested against fakes.

use super::availability::{probe, SystemAudioSupport};
use crate::recording::RecordingError;

pub struct SystemSource;

impl SystemSource {
    /// Open the system-audio source, or say why it is not available.
    pub fn open() -> Result<Box<dyn super::CaptureSource>, RecordingError> {
        open_with(probe())
    }
}

fn open_with(support: SystemAudioSupport) -> Result<Box<dyn super::CaptureSource>, RecordingError> {
    if !support.is_supported() {
        return Err(RecordingError::SourceUnavailable {
            reason: support.reason(),
        });
    }
    Err(RecordingError::SourceUnavailable {
        reason: "the system-audio backend is not built into this version",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_machines_are_refused_with_the_reason() {
        let error = open_with(SystemAudioSupport::OsTooOld).err().unwrap();
        assert_eq!(error.code(), "recording_source_unavailable");
        assert!(format!("{error:?}").contains("macOS 14.4"));
    }

    #[test]
    fn a_supported_machine_without_a_backend_is_still_unavailable() {
        let error = open_with(SystemAudioSupport::Supported).err().unwrap();
        assert_eq!(error.code(), "recording_source_unavailable");
    }
}
