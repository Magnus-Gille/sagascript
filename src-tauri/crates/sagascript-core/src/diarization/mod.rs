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
}

/// Default agglomerative clustering threshold (cosine distance). The single source of truth
/// for the CLI, reprocessing and UI defaults; the clap/Svelte literals must match (tests pin
/// the CLI ones). Chosen with the evaluation in docs/benchmarks/diarization-sv.md.
pub const DEFAULT_THRESHOLD: f32 = 0.34;

/// A speaker must be heard for at least this many seconds in total; smaller
/// embedding clusters are absorbed into the nearest cluster that reaches it.
pub const MIN_SPEAKER_SECONDS: f64 = 8.0;

/// Maximum centroid cosine distance for absorbing a small cluster (see [`DiarizeConfig`]).
pub const ABSORB_MAX_DISTANCE: f32 = 0.75;

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
    analyze_with_control(audio, config, &|| Ok(()))
}

/// Run threshold-independent segmentation and embedding extraction while
/// polling a caller-owned cancellation or progress callback at bounded
/// pipeline boundaries. Individual ONNX calls are not interruptible.
pub fn analyze_with_control(
    audio: &[f32],
    config: &DiarizeConfig,
    check: &dyn Fn() -> Result<(), DictationError>,
) -> Result<(DiarizationAnalysis, DiarizationTimings), DictationError> {
    use crate::diarization::model::{model_path, DiarizationModel};

    let total_started = Instant::now();
    check()?;

    // Load models
    let seg_path = model_path(DiarizationModel::PyannoteSegmentation3);
    let emb_path = model_path(DiarizationModel::WeSpeakerResNet34LM);

    // ONNX Runtime is a native protobuf parser. Verify both artifacts in full
    // before giving it either path, including models saved by older releases.
    crate::download::verify_file(
        &seg_path,
        DiarizationModel::PyannoteSegmentation3.download_integrity(),
    )?;
    check()?;
    crate::download::verify_file(
        &emb_path,
        DiarizationModel::WeSpeakerResNet34LM.download_integrity(),
    )?;
    check()?;

    let model_load_started = Instant::now();
    check()?;
    let mut segmenter = segmentation::Segmenter::new(&seg_path)?;
    check()?;
    let mut embedder = embedding::Embedder::new(&emb_path)?;
    let model_load_seconds = model_load_started.elapsed().as_secs_f64();
    check()?;

    // 1. Segmentation: frame-level speaker activity
    let segmentation_started = Instant::now();
    let frame_activations = segmenter.segment_with_control(audio, check)?;
    let segmentation_seconds = segmentation_started.elapsed().as_secs_f64();
    check()?;

    // 2. Convert to (start, end, local_speaker_idx) tuples
    let segment_extraction_started = Instant::now();
    let raw_segments = frame_activations.to_speaker_segments(config.min_segment, config.min_gap);
    let segment_extraction_seconds = segment_extraction_started.elapsed().as_secs_f64();
    check()?;

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
    let embeddings = embedder.extract_embeddings_with_control(audio, &raw_segments, check)?;
    let embeddings_seconds = embeddings_started.elapsed().as_secs_f64();
    check()?;

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
        // Zero or degenerate (near-zero norm) vectors are unusable: leave them to the track fallback.
        // (Persisted non-finite components are rejected by `validate()` above; live inference
        // sanitises them in `l2_normalize`.)
        .filter(|(_, e)| clustering::is_usable_embedding(e))
        .collect::<Vec<_>>();

    // Cluster embeddings → global speaker IDs. Every embedded segment keeps its own
    // embedding cluster; the pyannote tracks only label segments that could not be
    // embedded (too short).
    let clustered = if embeddings.is_empty() {
        Vec::new()
    } else {
        let mut clustered = clustering::cluster_speakers(&embeddings, config.threshold);
        let durations: Vec<f64> = embeddings
            .iter()
            .map(|(index, _)| {
                let (start, end, _) = raw_segments[*index];
                (end - start).max(0.0)
            })
            .collect();
        let mut labels: Vec<usize> = clustered.iter().map(|(_, label)| *label).collect();
        clustering::absorb_small_clusters(&embeddings, &mut labels, &durations,
            config.min_speaker_seconds,
            config.absorb_max_distance,
        );
        for (entry, label) in clustered.iter_mut().zip(labels) {
            entry.1 = label;
        }
        clustered
    };
    let speaker_map = assign_speaker_labels(raw_segments, &clustered);
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

/// Turn embedding clusters into contiguous output speaker labels.
///
/// Each embedded segment keeps its own cluster. Earlier versions forced every
/// pyannote track (a window-stitched speaker slot, at most three per 10 s window)
/// into a single cluster; on a debate where speakers take turns, stitching reuses
/// the same few slots for different people, so that cap merged speakers no matter
/// how low the threshold was (issue #284). Tracks are now only a fallback for
/// segments too short to embed: they take the duration-weighted majority cluster
/// of their track, or a distinct fallback key when the track has no embedding.
fn assign_speaker_labels(
    raw_segments: &[(f64, f64, usize)],
    clustered: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    let cluster_of_segment: HashMap<usize, usize> = clustered.iter().copied().collect();
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

    let mut fallback_cluster_by_track = HashMap::new();
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
        fallback_cluster_by_track.insert(track, canonical);
    }

    // Remap cluster IDs to contiguous output labels in segment order. Tracks
    // without any embedding get a distinct deterministic fallback key.
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
            let key = cluster_of_segment
                .get(&segment_index)
                .or_else(|| fallback_cluster_by_track.get(&track))
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
    use std::sync::atomic::{AtomicUsize, Ordering};

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
    fn zero_embeddings_fall_back_to_their_track_instead_of_becoming_speakers() {
        let dim = embedding::EMBEDDING_DIM;
        let mut real = vec![0.0f32; dim];
        real[0] = 1.0;
        let analysis = DiarizationAnalysis {
            raw_segments: vec![(0.0, 20.0, 0), (21.0, 30.0, 0), (31.0, 40.0, 1), (41.0, 50.0, 1)],
            embeddings: vec![
                (0, real.clone()),
                (1, real),
                (2, vec![0.0; dim]),
                (3, vec![0.0; dim]),
            ],
        };
        let segments = cluster(&analysis, &DiarizeConfig::default()).unwrap();
        let speakers: std::collections::BTreeSet<_> = segments.iter().map(|s| s.speaker.clone()).collect();
        // One embedded speaker; the two zero-embedding segments share their track's fallback label
        // (not one speaker per zero vector).
        assert_eq!(speakers.len(), 2, "{segments:?}");
        assert_eq!(segments[2].speaker, segments[3].speaker);
        assert_ne!(segments[0].speaker, segments[2].speaker);
    }

    #[test]
    fn embedded_segments_of_one_track_can_belong_to_different_speakers() {
        // Regression for #284: window stitching reuses a track for different people.
        let raw = vec![(0.0, 10.0, 1), (11.0, 20.0, 1), (21.0, 30.0, 1)];
        let clustered = vec![(0, 4), (1, 9), (2, 4)];
        let labels = assign_speaker_labels(&raw, &clustered);
        assert_eq!(labels, vec![(0, 0), (1, 1), (2, 0)]);
    }

    #[test]
    fn unembedded_segment_follows_majority_cluster_of_its_track() {
        let raw = vec![(0.0, 10.0, 0), (10.0, 10.1, 1), (11.0, 20.0, 1), (21.0, 21.1, 1)];
        // Segments 1 and 3 are too short to embed; segment 2 is the only embedded one of track 1.
        let clustered = vec![(0, 3), (2, 8)];
        let labels = assign_speaker_labels(&raw, &clustered);
        assert_eq!(labels, vec![(0, 0), (1, 1), (2, 1), (3, 1)]);
    }

    #[test]
    fn embedding_cluster_can_merge_two_aligned_tracks() {
        let raw = vec![(0.0, 2.0, 0), (3.0, 5.0, 1), (6.0, 8.0, 0)];
        let clustered = vec![(0, 7), (1, 7), (2, 7)];
        let stable = assign_speaker_labels(&raw, &clustered);
        assert_eq!(stable, vec![(0, 0), (1, 0), (2, 0)]);
    }

    #[test]
    fn tracks_remain_distinct_when_all_embeddings_are_missing() {
        let raw = vec![(0.0, 0.01, 2), (0.02, 0.03, 1), (0.04, 0.05, 2)];
        let stable = assign_speaker_labels(&raw, &[]);
        assert_eq!(stable, vec![(0, 0), (1, 1), (2, 0)]);
    }

    #[test]
    fn cluster_separates_speakers_sharing_one_track_and_absorbs_tiny_clusters() {
        use crate::diarization::embedding::EMBEDDING_DIM;
        let unit = |d: usize| {
            let mut v = vec![0.0f32; EMBEDDING_DIM];
            v[d] = 1.0;
            v
        };
        // Two real speakers (20 s each) on ONE track, plus a 1 s glitch at cosine distance 0.5 from
        // speaker 0 (above the 0.34 clustering threshold, so only absorption can join it) and a 1 s
        // segment far from both (a distinct, if tiny, speaker: not forced into either).
        let mut near0 = unit(0);
        near0[0] = 0.5;
        near0[2] = 0.866;
        let analysis = DiarizationAnalysis {
            raw_segments: vec![(0.0, 20.0, 0), (21.0, 41.0, 0), (42.0, 43.0, 0), (44.0, 45.0, 0)],
            embeddings: vec![(0, unit(0)), (1, unit(1)), (2, near0), (3, unit(5))],
        };
        // Without absorption (distance limit 0) the glitch stays its own speaker.
        let no_absorb = DiarizeConfig { absorb_max_distance: 0.0, ..DiarizeConfig::default() };
        let raw = cluster(&analysis, &no_absorb).unwrap();
        assert_ne!(raw[2].speaker, raw[0].speaker);
        let segments = cluster(&analysis, &DiarizeConfig::default()).unwrap();
        assert_ne!(segments[0].speaker, segments[1].speaker);
        assert_eq!(segments[2].speaker, segments[0].speaker);
        assert!(segments[3].speaker != segments[0].speaker && segments[3].speaker != segments[1].speaker);
    }

    #[test]
    fn controlled_analysis_checks_before_loading_models() {
        let calls = AtomicUsize::new(0);
        let check = || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(DictationError::DiarizationError("cancelled".to_owned()))
        };

        let result = analyze_with_control(&[], &DiarizeConfig::default(), &check);

        assert!(matches!(
            result,
            Err(DictationError::DiarizationError(message)) if message == "cancelled"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
