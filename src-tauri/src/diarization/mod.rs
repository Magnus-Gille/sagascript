pub mod clustering;
pub mod embedding;
pub mod fbank;
pub mod merge;
pub mod model;
pub mod segmentation;

use serde::Serialize;

use crate::error::DictationError;

/// Configuration for the diarization pipeline.
pub struct DiarizeConfig {
    /// Cosine distance threshold for agglomerative clustering (0.0–2.0).
    /// Lower = stricter (more speakers). Default 0.8.
    pub threshold: f32,
    /// Minimum segment duration in seconds to keep. Default 0.3s.
    pub min_segment: f64,
    /// Merge same-speaker segments closer than this gap (seconds). Default 0.5s.
    pub min_gap: f64,
}

impl Default for DiarizeConfig {
    fn default() -> Self {
        Self {
            threshold: clustering::DEFAULT_THRESHOLD,
            min_segment: 0.3,
            min_gap: 0.5,
        }
    }
}

/// Run the full diarization pipeline on 16kHz mono audio.
///
/// Returns speaker segments with labels "SPEAKER_0", "SPEAKER_1", ...
/// Requires the `diarization` feature and both ONNX models to be downloaded.
pub fn diarize(audio: &[f32], config: &DiarizeConfig) -> Result<Vec<SpeakerSegment>, DictationError> {
    use crate::diarization::model::{DiarizationModel, model_path};

    // Load models
    let seg_path = model_path(DiarizationModel::PyannoteSegmentation3);
    let emb_path = model_path(DiarizationModel::WeSpeakerResNet34LM);

    let mut segmenter = segmentation::Segmenter::new(&seg_path)?;
    let mut embedder = embedding::Embedder::new(&emb_path)?;

    // 1. Segmentation: frame-level speaker activity
    let frame_activations = segmenter.segment(audio)?;

    // 2. Convert to (start, end, local_speaker_idx) tuples
    let raw_segments = frame_activations.to_speaker_segments(config.min_segment, config.min_gap);

    if raw_segments.is_empty() {
        return Ok(Vec::new());
    }

    // 3. Extract embeddings per segment
    let embeddings = embedder.extract_embeddings(audio, &raw_segments)?;

    // 4. Cluster embeddings → global speaker IDs
    let speaker_map = if embeddings.is_empty() {
        Vec::new()
    } else {
        clustering::cluster_speakers(&embeddings, config.threshold)
    };

    // Build local_speaker → global_speaker lookup
    // (local speaker idx may appear multiple times with different global IDs due to
    // different segments; take the modal assignment per local speaker)
    let max_local = raw_segments.iter().map(|(_, _, s)| *s).max().unwrap_or(0) + 1;
    let mut local_to_global = vec![0usize; max_local];
    for (local_idx, global_id) in &speaker_map {
        local_to_global[*local_idx] = *global_id;
    }

    // 5. Build SpeakerSegment output
    let segments: Vec<SpeakerSegment> = raw_segments
        .iter()
        .map(|(start, end, local_idx)| {
            let global_id = local_to_global[*local_idx];
            SpeakerSegment {
                start: *start,
                end: *end,
                speaker: format!("SPEAKER_{global_id}"),
            }
        })
        .collect();

    Ok(segments)
}

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
