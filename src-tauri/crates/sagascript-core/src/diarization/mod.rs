#[cfg(feature = "diarization")]
pub mod engine;
pub mod merge;
pub mod model;
pub mod report;
pub mod speaker_hint;

// The old paths of the stages that moved into `engine/` (#372).
#[cfg(feature = "diarization")]
pub use engine::{
    analyze, analyze_with_control, cluster, cluster_with_outcome, clustering, diarize, embedding,
    fbank, segmentation,
};

use serde::{Deserialize, Serialize};

use crate::error::DictationError;
pub use speaker_hint::{SpeakerCountHint, SpeakerHintOutcome};

/// Configuration for the diarization pipeline.
pub struct DiarizeConfig {
    /// Cosine distance threshold for agglomerative clustering (0.0–2.0).
    /// Lower = stricter (more speakers). Default [`DEFAULT_THRESHOLD`] (see docs/benchmarks/diarization-sv.md).
    pub threshold: f32,
    /// Minimum segment duration in seconds to keep. Default 0.3s.
    pub min_segment: f64,
    /// Merge same-speaker segments closer than this gap (seconds). Default 0.5s.
    pub min_gap: f64,
    /// A speaker must be heard for at least this many seconds in total; smaller embedding
    /// clusters are absorbed into the nearest cluster that reaches it. Default
    /// [`MIN_SPEAKER_SECONDS`].
    pub min_speaker_seconds: f64,
    /// A small cluster is only absorbed into a cluster whose centroid cosine distance is at most
    /// this; otherwise it stays a distinct speaker. Default [`ABSORB_MAX_DISTANCE`].
    pub absorb_max_distance: f32,
    /// Optional speaker-count hint (exact, or min and/or max). `None` (the default) leaves
    /// clustering exactly as the threshold decides. With a hint the dendrogram is cut by count
    /// instead; see [`cluster`] for the precise rules. Applies only to the clustering stage, so it
    /// works on a cached [`DiarizationAnalysis`] without re-running analysis.
    pub speaker_hint: Option<SpeakerCountHint>,
    /// When a (non-forced) hint asks for fewer speakers than were found, clusters are merged
    /// toward the count only while their centroid cosine distance is at most this. Default
    /// [`HINT_MERGE_MAX_DISTANCE`].
    pub hint_merge_max_distance: f32,
    /// Experimental opt-in: embed only same-region samples where this track
    /// is active and every other segmentation track is inactive.
    pub exclusive_speech_embeddings: bool,
}

/// Distance limit for merging toward a lower speaker-count hint; see
/// [`DiarizeConfig::hint_merge_max_distance`] and docs/benchmarks/diarization-sv.md.
pub const HINT_MERGE_MAX_DISTANCE: f32 = 0.9;

/// Default agglomerative clustering threshold (cosine distance). The single source of truth
/// for the CLI, reprocessing and UI defaults; the clap/Svelte literals must match (tests pin
/// the CLI ones). Chosen with the evaluation in docs/benchmarks/diarization-sv.md.
pub const DEFAULT_THRESHOLD: f32 = 0.34;

/// A speaker must be heard for at least this many seconds in total; smaller
/// embedding clusters are absorbed into the nearest cluster that reaches it.
pub const MIN_SPEAKER_SECONDS: f64 = 8.0;

/// Maximum centroid cosine distance for absorbing a small cluster (see [`DiarizeConfig`]).
pub const ABSORB_MAX_DISTANCE: f32 = 0.75;

/// Size of one speaker embedding: the output of the WeSpeaker ResNet34-LM model.
pub const EMBEDDING_DIM: usize = 256;

/// A usable embedding is finite and non-degenerate. Zero (or non-finite) vectors carry no
/// speaker information; cosine similarity maps them to 0, which would make every one a separate
/// speaker. Callers route such segments through the track fallback instead.
pub fn is_usable_embedding(e: &[f32; EMBEDDING_DIM]) -> bool {
    e.iter().all(|x| x.is_finite()) && e.iter().map(|x| x * x).sum::<f32>().sqrt() > 1e-6
}

/// Threshold-independent output of segmentation and speaker embedding.
///
/// This is deliberately serializable so callers can persist the expensive
/// analysis and cheaply retry only agglomerative clustering with a different
/// threshold. The schema is versioned by the caller that persists it.
///
/// The clustering algorithm (and its default threshold) is deliberately NOT part of a cache
/// identity: an existing cache stays valid, and re-clustering it with a newer build can
/// produce a different speaker set than the build that created a stored review did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiarizationAnalysis {
    raw_segments: Vec<(f64, f64, usize)>,
    embeddings: Vec<(usize, Vec<f32>)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    region_activity_support: Option<Vec<RegionActivitySupport>>,
}

/// Binary segmentation support recorded for each original raw diarization
/// region. This is diagnostic metadata only; it contains no audio or vectors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RegionActivitySupport {
    pub start: f64,
    pub end: f64,
    pub track: usize,
    pub active_speech_seconds: f64,
    pub overlap_seconds: f64,
}

impl DiarizationAnalysis {
    /// Access region-level binary activity diagnostics when the experimental
    /// extraction policy was enabled.
    pub fn region_activity_support(&self) -> Option<&[RegionActivitySupport]> {
        self.region_activity_support.as_deref()
    }

    /// Reject malformed persisted analysis before it reaches clustering.
    pub fn validate(&self) -> Result<(), DictationError> {
        for &(start, end, _) in &self.raw_segments {
            if !start.is_finite() || !end.is_finite() || start < 0.0 || end < start {
                return Err(DictationError::DiarizationError(
                    "Cached diarization contains invalid segment bounds".to_string(),
                ));
            }
        }
        let mut embedding_indices = std::collections::BTreeSet::new();
        for (index, embedding) in &self.embeddings {
            if *index >= self.raw_segments.len()
                || !embedding_indices.insert(*index)
                || embedding.len() != EMBEDDING_DIM
                || embedding.iter().any(|value| !value.is_finite())
            {
                return Err(DictationError::DiarizationError(
                    "Cached diarization contains an invalid speaker embedding".to_string(),
                ));
            }
        }
        if let Some(support) = &self.region_activity_support {
            if support.len() != self.raw_segments.len()
                || support.iter().enumerate().any(|(index, item)| {
                    let (start, end, track) = self.raw_segments[index];
                    let region_seconds = end - start;
                    item.start.to_bits() != start.to_bits()
                        || item.end.to_bits() != end.to_bits()
                        || item.track != track
                        || !item.start.is_finite()
                        || !item.end.is_finite()
                        || !item.active_speech_seconds.is_finite()
                        || !item.overlap_seconds.is_finite()
                        || item.active_speech_seconds < 0.0
                        || item.overlap_seconds < 0.0
                        || item.active_speech_seconds
                            > region_seconds
                                + crate::diarization::report::ACTIVITY_DURATION_TOLERANCE
                        || item.overlap_seconds > item.active_speech_seconds + 1e-9
                })
            {
                return Err(DictationError::DiarizationError(
                    "Cached diarization contains invalid region activity support".to_string(),
                ));
            }
        }
        Ok(())
    }
}

/// Phase-level timings for the threshold-independent diarization analysis.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct DiarizationTimings {
    pub model_load_seconds: f64,
    pub segmentation_seconds: f64,
    pub segment_extraction_seconds: f64,
    pub embeddings_seconds: f64,
    pub total_seconds: f64,
}

impl Default for DiarizeConfig {
    fn default() -> Self {
        Self {
            // Segment-level average linkage (no per-track cap, see `assign_speaker_labels`).
            // Swedish recordings only (14 Riksdag recordings, leave-one-recording-out;
            // docs/benchmarks/diarization-sv.md): clean-audio confusion is flat for 0.26-0.46 (the
            // selection script's midpoint is 0.36). 0.34 is the owner's choice: no measurable cost
            // on clean Swedish, and the last value before the band-limited-audio edge at 0.36.
            threshold: DEFAULT_THRESHOLD,
            min_segment: 0.3,
            min_gap: 0.5,
            min_speaker_seconds: MIN_SPEAKER_SECONDS,
            absorb_max_distance: ABSORB_MAX_DISTANCE,
            speaker_hint: None,
            hint_merge_max_distance: HINT_MERGE_MAX_DISTANCE,
            exclusive_speech_embeddings: false,
        }
    }
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
