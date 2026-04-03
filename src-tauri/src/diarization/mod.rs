pub mod clustering;
pub mod embedding;
pub mod fbank;
pub mod merge;
pub mod model;
pub mod segmentation;

use serde::Serialize;

/// A single diarization segment: who spoke when.
#[derive(Debug, Clone, Serialize)]
pub struct SpeakerSegment {
    /// Start time in seconds
    pub start: f64,
    /// End time in seconds
    pub end: f64,
    /// Speaker label (e.g. "SPEAKER_0")
    pub speaker: String,
}

/// A Whisper transcript segment with timestamps.
#[derive(Debug, Clone, Serialize)]
pub struct TimestampedSegment {
    /// Start time in seconds
    pub start: f64,
    /// End time in seconds
    pub end: f64,
    /// Transcribed text
    pub text: String,
}

/// A transcript segment with speaker attribution (final output).
#[derive(Debug, Clone, Serialize)]
pub struct DiarizedSegment {
    /// Start time in seconds
    pub start: f64,
    /// End time in seconds
    pub end: f64,
    /// Speaker label (e.g. "SPEAKER_0")
    pub speaker: String,
    /// Transcribed text
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speaker_segment_serializes() {
        let seg = SpeakerSegment {
            start: 0.0,
            end: 2.5,
            speaker: "SPEAKER_0".to_string(),
        };
        let json = serde_json::to_value(&seg).unwrap();
        assert_eq!(json["speaker"], "SPEAKER_0");
        assert_eq!(json["start"], 0.0);
        assert_eq!(json["end"], 2.5);
    }

    #[test]
    fn timestamped_segment_serializes() {
        let seg = TimestampedSegment {
            start: 1.0,
            end: 3.0,
            text: "hello world".to_string(),
        };
        let json = serde_json::to_value(&seg).unwrap();
        assert_eq!(json["text"], "hello world");
    }

    #[test]
    fn diarized_segment_serializes() {
        let seg = DiarizedSegment {
            start: 0.0,
            end: 2.0,
            speaker: "SPEAKER_1".to_string(),
            text: "test".to_string(),
        };
        let json = serde_json::to_value(&seg).unwrap();
        assert_eq!(json["speaker"], "SPEAKER_1");
        assert_eq!(json["text"], "test");
    }
}
