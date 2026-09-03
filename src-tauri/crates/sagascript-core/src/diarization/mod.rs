pub mod clustering;
pub mod embedding;
pub mod fbank;
pub mod merge;
pub mod model;
pub mod segmentation;

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::error::DictationError;

/// Keep both CPU-bound ONNX stages within the Apple-Silicon performance-core
/// budget while Whisper runs concurrently on Metal.
const ORT_INTRA_THREADS: usize = 4;

/// Configuration for the diarization pipeline.
pub struct DiarizeConfig {
    /// Cosine distance threshold for agglomerative clustering (0.0–2.0).
    /// Lower = stricter (more speakers). Default 0.75.
    pub threshold: f32,
    /// Minimum segment duration in seconds to keep. Default 0.3s.
    pub min_segment: f64,
    /// Merge same-speaker segments closer than this gap (seconds). Default 0.5s.
    pub min_gap: f64,
}

/// Threshold-independent output of segmentation and speaker embedding.
///
/// This is deliberately serializable so callers can persist the expensive
/// analysis and cheaply retry only agglomerative clustering with a different
/// threshold. The schema is versioned by the caller that persists it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiarizationAnalysis {
    raw_segments: Vec<(f64, f64, usize)>,
    embeddings: Vec<(usize, Vec<f32>)>,
}

impl DiarizationAnalysis {
    /// Reject malformed persisted analysis before it reaches clustering.
    pub fn validate(&self) -> Result<(), DictationError> {
        for &(start, end, _) in &self.raw_segments {
            if !start.is_finite() || !end.is_finite() || start < 0.0 || end < start {
                return Err(DictationError::DiarizationError(
                    "Cached diarization contains invalid segment bounds".to_string(),
                ));
            }
        }
        for (index, embedding) in &self.embeddings {
            if *index >= self.raw_segments.len()
                || embedding.len() != embedding::EMBEDDING_DIM
                || embedding.iter().any(|value| !value.is_finite())
            {
                return Err(DictationError::DiarizationError(
                    "Cached diarization contains an invalid speaker embedding".to_string(),
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
            // Tuned against the two ground-truthed test files (dj_2022_feu FR,
            // nb_samtale_nb12 NO) with permutation-aligned window stitching:
            // both are DER-optimal across 0.70–0.80; 0.85 sits on a cliff where
            // nb_samtale's two speakers merge into one cluster. 0.75 is the
            // midpoint of the common plateau.
            threshold: 0.75,
            min_segment: 0.3,
            min_gap: 0.5,
        }
    }
}

/// Run the full diarization pipeline on 16kHz mono audio.
///
/// Returns speaker segments with labels "SPEAKER_0", "SPEAKER_1", ...
/// Requires the `diarization` feature and both ONNX models to be downloaded.
pub fn diarize(
    audio: &[f32],
    config: &DiarizeConfig,
) -> Result<Vec<SpeakerSegment>, DictationError> {
    let (analysis, _) = analyze(audio, config)?;
    cluster(&analysis, config)
}

/// Run threshold-independent segmentation and embedding extraction.
///
/// `min_segment` and `min_gap` affect which regions are embedded, while the
/// clustering threshold does not. Persisting this output therefore makes a
/// threshold-only retry cheap without retaining the source audio.
pub fn analyze(
    audio: &[f32],
    config: &DiarizeConfig,
) -> Result<(DiarizationAnalysis, DiarizationTimings), DictationError> {
    use crate::diarization::model::{model_path, DiarizationModel};

    let total_started = Instant::now();

    // Load models
    let seg_path = model_path(DiarizationModel::PyannoteSegmentation3);
    let emb_path = model_path(DiarizationModel::WeSpeakerResNet34LM);

    // ONNX Runtime is a native protobuf parser. Verify both artifacts in full
    // before giving it either path, including models saved by older releases.
    crate::download::verify_file(
        &seg_path,
        DiarizationModel::PyannoteSegmentation3.download_integrity(),
    )?;
    crate::download::verify_file(
        &emb_path,
        DiarizationModel::WeSpeakerResNet34LM.download_integrity(),
    )?;

    let model_load_started = Instant::now();
    let mut segmenter = segmentation::Segmenter::new(&seg_path)?;
    let mut embedder = embedding::Embedder::new(&emb_path)?;
    let model_load_seconds = model_load_started.elapsed().as_secs_f64();

    // 1. Segmentation: frame-level speaker activity
    let segmentation_started = Instant::now();
    let frame_activations = segmenter.segment(audio)?;
    let segmentation_seconds = segmentation_started.elapsed().as_secs_f64();

    // 2. Convert to (start, end, local_speaker_idx) tuples
    let segment_extraction_started = Instant::now();
    let raw_segments = frame_activations.to_speaker_segments(config.min_segment, config.min_gap);
    let segment_extraction_seconds = segment_extraction_started.elapsed().as_secs_f64();

    if raw_segments.is_empty() {
        return Ok((
            DiarizationAnalysis {
                raw_segments,
                embeddings: Vec::new(),
            },
            DiarizationTimings {
                model_load_seconds,
                segmentation_seconds,
                segment_extraction_seconds,
                embeddings_seconds: 0.0,
                total_seconds: total_started.elapsed().as_secs_f64(),
            },
        ));
    }

    // 3. Extract embeddings per segment
    let embeddings_started = Instant::now();
    let embeddings = embedder.extract_embeddings(audio, &raw_segments)?;
    let embeddings_seconds = embeddings_started.elapsed().as_secs_f64();

    if std::env::var("SAGA_DIAR_DEBUG").is_ok() {
        for (i, segment) in raw_segments.iter().enumerate() {
            eprintln!(
                "RAWDIAR\t{i}\t{:.3}\t{:.3}\t{}",
                segment.0, segment.1, segment.2
            );
        }
    }

    Ok((
        DiarizationAnalysis {
            raw_segments,
            embeddings: embeddings
                .into_iter()
                .map(|(index, embedding)| (index, embedding.to_vec()))
                .collect(),
        },
        DiarizationTimings {
            model_load_seconds,
            segmentation_seconds,
            segment_extraction_seconds,
            embeddings_seconds,
            total_seconds: total_started.elapsed().as_secs_f64(),
        },
    ))
}

/// Apply the cheap threshold-dependent clustering stage to prior analysis.
pub fn cluster(
    analysis: &DiarizationAnalysis,
    config: &DiarizeConfig,
) -> Result<Vec<SpeakerSegment>, DictationError> {
    analysis.validate()?;
    let raw_segments = &analysis.raw_segments;
    let embeddings = analysis
        .embeddings
        .iter()
        .map(|(index, values)| {
            let embedding: [f32; embedding::EMBEDDING_DIM] = values
                .as_slice()
                .try_into()
                .expect("DiarizationAnalysis was validated with exact embeddings");
            (*index, embedding)
        })
        .collect::<Vec<_>>();

    // Cluster embeddings → global speaker IDs
    // speaker_map[i] = (segment_index, global_id) — segment_index keys into raw_segments
    let clustered = if embeddings.is_empty() {
        Vec::new()
    } else {
        clustering::cluster_speakers(&embeddings, config.threshold)
    };
    let speaker_map = stabilize_clusters_by_track(raw_segments, &clustered);
    let n_global = speaker_map
        .iter()
        .map(|(_, g)| g)
        .max()
        .map(|&m| m + 1)
        .unwrap_or(0);
    eprintln!("  Found {n_global} speaker(s)");

    // Build segment_index → global_id map (default 0 for segments with no embedding)
    let mut seg_to_global = vec![0usize; raw_segments.len()];
    for (seg_idx, global_id) in &speaker_map {
        seg_to_global[*seg_idx] = *global_id;
    }

    // 5. Build SpeakerSegment output
    let segments: Vec<SpeakerSegment> = raw_segments
        .iter()
        .enumerate()
        .map(|(i, (start, end, _local_idx))| {
            let global_id = seg_to_global[i];
            SpeakerSegment {
                start: *start,
                end: *end,
                speaker: format!("SPEAKER_{global_id}"),
            }
        })
        .collect();

    Ok(segments)
}

/// Reconcile embedding clusters with the globally aligned pyannote tracks.
///
/// Since window permutation alignment was introduced, the third field of each
/// raw segment is a stable track ID rather than a window-local speaker slot.
/// Short utterances (especially children or noisy speech) produce volatile
/// WeSpeaker embeddings and can otherwise split one stable track into many
/// output speakers. For each track, use the embedding cluster covering the
/// greatest amount of speech, while still allowing embedding clustering to
/// merge two tracks that represent the same physical speaker.
fn stabilize_clusters_by_track(
    raw_segments: &[(f64, f64, usize)],
    clustered: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let mut duration_by_track_cluster: HashMap<usize, HashMap<usize, f64>> = HashMap::new();
    for &(segment_index, cluster) in clustered {
        let Some(&(start, end, track)) = raw_segments.get(segment_index) else {
            continue;
        };
        *duration_by_track_cluster
            .entry(track)
            .or_default()
            .entry(cluster)
            .or_default() += (end - start).max(0.0);
    }

    let mut canonical_cluster_by_track = HashMap::new();
    for (track, durations) in duration_by_track_cluster {
        let canonical = durations
            .into_iter()
            .max_by(|(cluster_a, duration_a), (cluster_b, duration_b)| {
                duration_a
                    .partial_cmp(duration_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // Deterministic tie-break: the lower original cluster ID wins.
                    .then_with(|| cluster_b.cmp(cluster_a))
            })
            .map(|(cluster, _)| cluster)
            .unwrap_or(0);
        canonical_cluster_by_track.insert(track, canonical);
    }

    // Remap canonical cluster IDs to contiguous output labels in segment order.
    // Tracks without an embedding get a distinct deterministic fallback key.
    let fallback_base = clustered
        .iter()
        .map(|(_, cluster)| cluster)
        .max()
        .map_or(0, |c| c + 1);
    let mut output_label_by_key = BTreeMap::new();
    let mut next_label = 0usize;
    raw_segments
        .iter()
        .enumerate()
        .map(|(segment_index, &(_, _, track))| {
            let key = canonical_cluster_by_track
                .get(&track)
                .copied()
                .unwrap_or(fallback_base + track);
            let output_label = *output_label_by_key.entry(key).or_insert_with(|| {
                let label = next_label;
                next_label += 1;
                label
            });
            (segment_index, output_label)
        })
        .collect()
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

    #[test]
    fn cached_analysis_rejects_wrong_embedding_dimensions() {
        let analysis: DiarizationAnalysis = serde_json::from_value(serde_json::json!({
            "raw_segments": [[0.0, 1.0, 0]],
            "embeddings": [[0, [0.0, 1.0]]],
        }))
        .unwrap();

        assert!(analysis.validate().is_err());
        assert!(cluster(&analysis, &DiarizeConfig::default()).is_err());
    }

    #[test]
    fn cached_analysis_accepts_exact_finite_embeddings() {
        let analysis = DiarizationAnalysis {
            raw_segments: vec![(0.0, 1.0, 0)],
            embeddings: vec![(0, vec![0.0; embedding::EMBEDDING_DIM])],
        };

        assert!(analysis.validate().is_ok());
    }

    #[test]
    fn stable_track_is_not_split_by_noisy_short_utterance_embeddings() {
        let raw = vec![
            (0.0, 10.0, 0),
            (10.0, 10.5, 1),
            (11.0, 12.0, 1),
            (13.0, 13.4, 1),
        ];
        // Track 1 was over-clustered into three labels. Its longest segment's
        // label is canonical; all three utterances must remain one speaker.
        let clustered = vec![(0, 0), (1, 2), (2, 3), (3, 4)];
        let stable = stabilize_clusters_by_track(&raw, &clustered);
        assert_eq!(stable, vec![(0, 0), (1, 1), (2, 1), (3, 1)]);
    }

    #[test]
    fn embedding_cluster_can_merge_two_aligned_tracks() {
        let raw = vec![(0.0, 2.0, 0), (3.0, 5.0, 1), (6.0, 8.0, 0)];
        let clustered = vec![(0, 7), (1, 7), (2, 7)];
        let stable = stabilize_clusters_by_track(&raw, &clustered);
        assert_eq!(stable, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn track_canonical_cluster_is_weighted_by_speech_duration() {
        let raw = vec![(0.0, 0.2, 1), (1.0, 5.0, 1), (6.0, 6.2, 1)];
        let clustered = vec![(0, 2), (1, 5), (2, 2)];
        let stable = stabilize_clusters_by_track(&raw, &clustered);
        assert_eq!(stable, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn tracks_remain_distinct_when_all_embeddings_are_missing() {
        let raw = vec![(0.0, 0.01, 2), (0.02, 0.03, 1), (0.04, 0.05, 2)];
        let stable = stabilize_clusters_by_track(&raw, &[]);
        assert_eq!(stable, vec![(0, 0), (1, 1), (2, 0)]);
    }
}
