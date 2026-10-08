//! Diagnostic projection of cached analysis; does not change its labels.
use std::collections::BTreeMap;

use crate::diarization_report::{
    activity_spans, AttributionEvidence, DiarizationParameters, DiarizationReport, EmbeddingStatus,
    RegionEvidence,
};
use crate::error::DictationError;

use super::{clustering, DiarizationAnalysis, DiarizeConfig, SpeakerSegment};

fn distance(left: &[f32], right: &[f64]) -> Option<f64> {
    let norm_left = left
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    let norm_right = right.iter().map(|v| v.powi(2)).sum::<f64>().sqrt();
    if norm_left <= 1e-12 || norm_right <= 1e-12 {
        return None;
    }
    let dot = left
        .iter()
        .zip(right)
        .map(|(a, b)| f64::from(*a) * b)
        .sum::<f64>();
    Some((1.0 - dot / (norm_left * norm_right)).clamp(0.0, 2.0))
}

impl DiarizationAnalysis {
    #[allow(clippy::too_many_arguments)]
    pub fn report(
        &self,
        speakers: &[SpeakerSegment],
        config: &DiarizeConfig,
        mut attributions: Vec<AttributionEvidence>,
        source_sha256: String,
        duration: f64,
        build_revision: String,
        build_version: String,
    ) -> Result<DiarizationReport, DictationError> {
        self.validate()?;
        if speakers.len() != self.raw_segments.len() || !duration.is_finite() || duration < 0.0 {
            return Err(DictationError::DiarizationError(
                "diagnostic regions do not match analysis".into(),
            ));
        }
        let embeddings = self
            .embeddings
            .iter()
            .map(|(i, v)| (*i, v))
            .collect::<BTreeMap<_, _>>();
        let mut centroids = BTreeMap::<&str, Vec<f64>>::new();
        for (index, embedding) in &embeddings {
            let vector: [f32; super::embedding::EMBEDDING_DIM] = embedding
                .as_slice()
                .try_into()
                .expect("validated embedding dimension");
            if !clustering::is_usable_embedding(&vector) {
                continue;
            }
            let sum = centroids
                .entry(&speakers[*index].speaker)
                .or_insert_with(|| vec![0.0; super::embedding::EMBEDDING_DIM]);
            for (target, value) in sum.iter_mut().zip(embedding.iter()) {
                *target += f64::from(*value);
            }
        }
        let regions = self
            .raw_segments
            .iter()
            .enumerate()
            .map(|(index, (start, end, track))| {
                let speaker = &speakers[index].speaker;
                let embedding = embeddings.get(&index).copied();
                let status = match embedding {
                    None => EmbeddingStatus::Missing,
                    Some(vector) => {
                        let array: [f32; super::embedding::EMBEDDING_DIM] = vector
                            .as_slice()
                            .try_into()
                            .expect("validated embedding dimension");
                        if clustering::is_usable_embedding(&array) {
                            EmbeddingStatus::Usable
                        } else {
                            EmbeddingStatus::Degenerate
                        }
                    }
                };
                let assigned = embedding.and_then(|vector| {
                    centroids
                        .get(speaker.as_str())
                        .and_then(|centroid| distance(vector, centroid))
                });
                let other = embedding.and_then(|vector| {
                    centroids
                        .iter()
                        .filter(|(id, _)| **id != speaker)
                        .filter_map(|(_, centroid)| distance(vector, centroid))
                        .min_by(f64::total_cmp)
                });
                RegionEvidence {
                    index,
                    start: start.min(duration),
                    end: end.min(duration),
                    track: *track,
                    speaker: speaker.clone(),
                    used_track_fallback: status != EmbeddingStatus::Usable,
                    embedding_status: status,
                    assigned_centroid_distance: assigned,
                    nearest_other_centroid_distance: other,
                    active_speech_seconds: None,
                    overlapping_speech_seconds: None,
                }
            })
            .collect();
        for word in &mut attributions {
            if !word.start.is_finite()
                || !word.end.is_finite()
                || word.start < 0.0
                || word.end > duration
                || word.end < word.start
            {
                word.reason = crate::diarization_report::AttributionReason::InvalidTimestamp;
                word.start = if word.start.is_finite() {
                    word.start.clamp(0.0, duration)
                } else {
                    0.0
                };
                word.end = if word.end.is_finite() {
                    word.end.clamp(word.start, duration)
                } else {
                    word.start
                };
                word.support.clear();
                word.margin_seconds = None;
                word.gap_seconds = None;
            }
        }
        let activity = activity_spans(
            &speakers
                .iter()
                .map(|s| {
                    (
                        s.start.min(duration),
                        s.end.min(duration),
                        s.speaker.clone(),
                    )
                })
                .collect::<Vec<_>>(),
            duration,
        );
        let report = DiarizationReport {
            schema_version: 1,
            source_sha256,
            duration_seconds: duration,
            build_revision,
            build_version,
            segmentation_model_sha256: Some(
                super::model::DiarizationModel::PyannoteSegmentation3
                    .download_integrity()
                    .sha256
                    .into(),
            ),
            embedding_model_sha256: Some(
                super::model::DiarizationModel::WeSpeakerResNet34LM
                    .download_integrity()
                    .sha256
                    .into(),
            ),
            decoder: None,
            asr_segments: Vec::new(),
            transcript_modified: false,
            parameters: DiarizationParameters {
                threshold: config.threshold,
                min_segment_seconds: config.min_segment,
                min_gap_seconds: config.min_gap,
                min_speaker_seconds: config.min_speaker_seconds,
                absorb_max_distance: config.absorb_max_distance,
                hint_merge_max_distance: config.hint_merge_max_distance,
            },
            activity,
            regions,
            attributions,
        };
        report
            .validate()
            .map_err(|e| DictationError::DiarizationError(e.to_string()))?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_embeddings_are_explained_without_changing_speakers() {
        let analysis: DiarizationAnalysis = serde_json::from_value(
            serde_json::json!({"raw_segments":[[0.0,1.0,0],[0.5,1.5,1]],"embeddings":[]}),
        )
        .unwrap();
        let speakers = vec![
            SpeakerSegment {
                start: 0.0,
                end: 1.0,
                speaker: "SPEAKER_0".into(),
            },
            SpeakerSegment {
                start: 0.5,
                end: 1.5,
                speaker: "SPEAKER_1".into(),
            },
        ];
        let report = analysis
            .report(
                &speakers,
                &DiarizeConfig::default(),
                Vec::new(),
                "a".repeat(64),
                2.0,
                "revision".into(),
                "1.4.4".into(),
            )
            .unwrap();
        assert_eq!(report.regions.len(), 2);
        assert!(report
            .regions
            .iter()
            .all(|r| r.used_track_fallback && r.embedding_status == EmbeddingStatus::Missing));
        assert_eq!(report.activity[1].speakers.len(), 2);
        assert_eq!(
            report,
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap()
        );
    }
}
