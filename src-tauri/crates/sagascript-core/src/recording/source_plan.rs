//! Which sources a recording uses (`--source mic|system|both`, decision D3).
//!
//! `system` on a machine without system audio is an error: the user asked for
//! something that cannot happen. `both` falls back to the microphone alone and
//! says so, so a recording is still made (D3: older macOS and Intel record the
//! microphone only).

use super::capture::SystemAudioSupport;
use super::RecordingError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordSource {
    #[default]
    Mic,
    System,
    Both,
}

impl RecordSource {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "mic" => Some(Self::Mic),
            "system" => Some(Self::System),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mic => "mic",
            Self::System => "system",
            Self::Both => "both",
        }
    }
}

/// The tracks to open, and a notice when the request was reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourcePlan {
    pub mic: bool,
    pub system: bool,
    /// Set when `both` fell back to the microphone: the reason to show.
    pub fallback: Option<&'static str>,
}

pub fn plan(
    source: RecordSource,
    support: SystemAudioSupport,
) -> Result<SourcePlan, RecordingError> {
    let supported = support.is_supported();
    match source {
        RecordSource::Mic => Ok(SourcePlan {
            mic: true,
            system: false,
            fallback: None,
        }),
        RecordSource::System if supported => Ok(SourcePlan {
            mic: false,
            system: true,
            fallback: None,
        }),
        RecordSource::System => Err(RecordingError::SourceUnavailable {
            reason: support.reason(),
        }),
        RecordSource::Both if supported => Ok(SourcePlan {
            mic: true,
            system: true,
            fallback: None,
        }),
        RecordSource::Both => Ok(SourcePlan {
            mic: true,
            system: false,
            fallback: Some(support.reason()),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SystemAudioSupport::*;

    #[test]
    fn mic_never_needs_system_audio() {
        for s in [
            Supported,
            OsTooOld,
            UnsupportedArch,
            UnsupportedOs,
            NoBackend,
        ] {
            let p = plan(RecordSource::Mic, s).unwrap();
            assert!(p.mic && !p.system && p.fallback.is_none());
        }
    }

    #[test]
    fn system_alone_is_refused_where_it_is_unsupported() {
        assert_eq!(
            plan(RecordSource::System, Supported).unwrap(),
            SourcePlan {
                mic: false,
                system: true,
                fallback: None
            }
        );
        for s in [OsTooOld, UnsupportedArch, UnsupportedOs, NoBackend] {
            let e = plan(RecordSource::System, s).unwrap_err();
            assert_eq!(e.code(), "recording_source_unavailable");
        }
    }

    #[test]
    fn both_falls_back_to_the_microphone_with_the_reason() {
        let ok = plan(RecordSource::Both, Supported).unwrap();
        assert!(ok.mic && ok.system && ok.fallback.is_none());
        let p = plan(RecordSource::Both, OsTooOld).unwrap();
        assert!(p.mic && !p.system);
        assert_eq!(p.fallback, Some(OsTooOld.reason()));
    }

    #[test]
    fn names_round_trip() {
        for s in [RecordSource::Mic, RecordSource::System, RecordSource::Both] {
            assert_eq!(RecordSource::parse(s.as_str()), Some(s));
        }
        assert_eq!(RecordSource::parse("speakers"), None);
    }
}
