/// Speaker segmentation using the pyannote segmentation-3.0 ONNX model.
///
/// Input: 16kHz mono f32 audio
/// Output: per-frame speaker activity for up to 3 simultaneous speakers
use std::path::Path;

use ndarray::Array3;
use ort::{session::Session, value::Tensor};
use tracing::info;

use crate::error::DictationError;

/// 10-second window at 16kHz
pub const WINDOW_SAMPLES: usize = 160_000;
/// 1-second shift (90% overlap between consecutive windows)
pub const STEP_SAMPLES: usize = 16_000;
/// Number of output frames per 10-second window
pub const FRAMES_PER_WINDOW: usize = 589;
/// receptive_field_shift = 270 samples → 16.875ms per frame
pub const FRAME_DURATION_S: f64 = 270.0 / 16_000.0;
/// Powerset encoding: 7 classes for up to 3 simultaneous speakers
pub const NUM_CLASSES: usize = 7;
/// Maximum simultaneous speakers supported by the model
pub const MAX_SPEAKERS: usize = 3;

/// Powerset mapping: class index → [speaker0_active, speaker1_active, speaker2_active]
/// Class 0: silence, classes 1-3: single speakers, classes 4-6: speaker pairs
const POWERSET: [[f32; MAX_SPEAKERS]; NUM_CLASSES] = [
    [0.0, 0.0, 0.0], // 0: silence
    [1.0, 0.0, 0.0], // 1: speaker 0
    [0.0, 1.0, 0.0], // 2: speaker 1
    [0.0, 0.0, 1.0], // 3: speaker 2
    [1.0, 1.0, 0.0], // 4: speakers 0+1
    [1.0, 0.0, 1.0], // 5: speakers 0+2
    [0.0, 1.0, 1.0], // 6: speakers 1+2
];

/// Frame-level speaker activity across the full audio.
pub struct FrameActivations {
    /// Binary speaker activity: activity[frame][speaker] ∈ {0.0, 1.0}
    pub activity: Vec<[f32; MAX_SPEAKERS]>,
    /// Duration per frame in seconds
    pub frame_duration: f64,
}

impl FrameActivations {
    /// Convert frame-level activity to speaker time segments.
    ///
    /// Returns `(start_sec, end_sec, speaker_idx)` tuples, one per contiguous
    /// active region per speaker. Regions shorter than `min_duration` are dropped.
    /// Adjacent regions from the same speaker separated by less than `min_gap` are merged.
    pub fn to_speaker_segments(
        &self,
        min_duration: f64,
        min_gap: f64,
    ) -> Vec<(f64, f64, usize)> {
        let mut segments: Vec<(f64, f64, usize)> = Vec::new();

        for spk in 0..MAX_SPEAKERS {
            let mut raw: Vec<(f64, f64)> = Vec::new();
            let mut start: Option<usize> = None;

            for (f, frame) in self.activity.iter().enumerate() {
                let active = frame[spk] >= 0.5;
                match (start, active) {
                    (None, true) => start = Some(f),
                    (Some(s), false) => {
                        raw.push((s as f64 * self.frame_duration, f as f64 * self.frame_duration));
                        start = None;
                    }
                    _ => {}
                }
            }
            if let Some(s) = start {
                let end = self.activity.len() as f64 * self.frame_duration;
                raw.push((s as f64 * self.frame_duration, end));
            }

            // Merge segments closer than min_gap
            let mut merged: Vec<(f64, f64)> = Vec::new();
            for (seg_start, seg_end) in raw {
                if let Some(last) = merged.last_mut() {
                    if seg_start - last.1 < min_gap {
                        last.1 = seg_end;
                        continue;
                    }
                }
                merged.push((seg_start, seg_end));
            }

            // Filter by minimum duration and emit
            for (seg_start, seg_end) in merged {
                if seg_end - seg_start >= min_duration {
                    segments.push((seg_start, seg_end, spk));
                }
            }
        }

        // Sort by start time
        segments.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        segments
    }
}

/// Wraps the pyannote segmentation ONNX model for sliding-window inference.
pub struct Segmenter {
    session: Session,
}

impl Segmenter {
    /// Load the segmentation model from disk.
    pub fn new(model_path: &Path) -> Result<Self, DictationError> {
        let session = Session::builder()
            .map_err(|e| DictationError::DiarizationError(format!("Failed to create ORT session builder: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| DictationError::DiarizationError(format!("Failed to load segmentation model: {e}")))?;
        info!("Segmentation model loaded from {}", model_path.display());
        Ok(Self { session })
    }

    /// Run sliding-window segmentation on full audio.
    ///
    /// Returns `FrameActivations` covering the entire audio duration.
    pub fn segment(&mut self, audio: &[f32]) -> Result<FrameActivations, DictationError> {
        if audio.is_empty() {
            return Ok(FrameActivations {
                activity: Vec::new(),
                frame_duration: FRAME_DURATION_S,
            });
        }

        // Compute total frames across all windows after stitching
        let n_windows = if audio.len() <= WINDOW_SAMPLES {
            1
        } else {
            (audio.len() - WINDOW_SAMPLES) / STEP_SAMPLES + 2
        };
        let total_frames =
            FRAMES_PER_WINDOW + (n_windows.saturating_sub(1)) * frames_per_step();

        let mut accumulated = vec![[0.0f32; NUM_CLASSES]; total_frames];
        let mut counts = vec![0u32; total_frames];

        for win_idx in 0..n_windows {
            let win_start = win_idx * STEP_SAMPLES;
            if win_start >= audio.len() {
                break;
            }

            // Build the 10s window, zero-padding if needed
            let mut window = vec![0.0f32; WINDOW_SAMPLES];
            let available = (audio.len() - win_start).min(WINDOW_SAMPLES);
            window[..available].copy_from_slice(&audio[win_start..win_start + available]);

            // Run ONNX inference
            let input = Array3::from_shape_vec((1, 1, WINDOW_SAMPLES), window)
                .map_err(|e| DictationError::DiarizationError(format!("Shape error: {e}")))?;
            let tensor = Tensor::from_array(input)
                .map_err(|e| DictationError::DiarizationError(format!("Tensor error: {e}")))?;
            let outputs = self
                .session
                .run(ort::inputs![tensor])
                .map_err(|e| DictationError::DiarizationError(format!("Inference failed: {e}")))?;

            // Extract [1, 589, 7] output
            let logits = outputs[0]
                .try_extract_array::<f32>()
                .map_err(|e| DictationError::DiarizationError(format!("Output extraction failed: {e}")))?;

            let logits = logits.view();

            // Accumulate frame probabilities in overlap region
            let frame_offset = win_idx * frames_per_step();
            for f in 0..FRAMES_PER_WINDOW {
                let global_f = frame_offset + f;
                if global_f >= total_frames {
                    break;
                }
                for c in 0..NUM_CLASSES {
                    accumulated[global_f][c] += logits[[0, f, c]];
                }
                counts[global_f] += 1;
            }
        }

        // Average overlapping frames, then argmax → powerset decode
        let activity: Vec<[f32; MAX_SPEAKERS]> = accumulated
            .iter()
            .zip(counts.iter())
            .map(|(frame_logits, &count)| {
                let n = count.max(1) as f32;
                let averaged: Vec<f32> = frame_logits.iter().map(|&v| v / n).collect();

                // Argmax across 7 classes
                let class = averaged
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
                    .unwrap_or(0);

                POWERSET[class]
            })
            .collect();

        Ok(FrameActivations {
            activity,
            frame_duration: FRAME_DURATION_S,
        })
    }
}

/// How many frames correspond to one STEP_SAMPLES shift.
fn frames_per_step() -> usize {
    // STEP_SAMPLES / receptive_field_shift = 16000 / 270 ≈ 59
    (STEP_SAMPLES as f64 / (FRAME_DURATION_S * 16_000.0)).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn powerset_has_correct_shape() {
        assert_eq!(POWERSET.len(), NUM_CLASSES);
        for row in &POWERSET {
            assert_eq!(row.len(), MAX_SPEAKERS);
        }
    }

    #[test]
    fn powerset_silence_is_all_zeros() {
        assert_eq!(POWERSET[0], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn powerset_single_speakers_exclusive() {
        // Classes 1-3 each activate exactly one speaker
        for c in 1..=3 {
            let sum: f32 = POWERSET[c].iter().sum();
            assert_eq!(sum, 1.0, "class {c} should activate exactly 1 speaker");
        }
    }

    #[test]
    fn powerset_pairs_activate_two() {
        // Classes 4-6 each activate exactly two speakers
        for c in 4..=6 {
            let sum: f32 = POWERSET[c].iter().sum();
            assert_eq!(sum, 2.0, "class {c} should activate exactly 2 speakers");
        }
    }

    #[test]
    fn frame_activations_to_segments_basic() {
        // Speaker 0 active for frames 0-9, silent after
        let mut activity = vec![[0.0f32; MAX_SPEAKERS]; 20];
        for f in 0..10 {
            activity[f][0] = 1.0;
        }
        let fa = FrameActivations {
            activity,
            frame_duration: FRAME_DURATION_S,
        };
        let segs = fa.to_speaker_segments(0.1, 0.5);
        assert_eq!(segs.len(), 1, "should find exactly 1 segment");
        assert_eq!(segs[0].2, 0, "should be speaker 0");
        assert!(segs[0].0 < 0.01, "start should be ~0s");
        assert!(
            (segs[0].1 - 10.0 * FRAME_DURATION_S).abs() < 0.01,
            "end should be ~10 frames"
        );
    }

    #[test]
    fn frame_activations_min_duration_filter() {
        // 2 frames of activity — at 16.875ms each that's ~33ms, below 0.3s min
        let mut activity = vec![[0.0f32; MAX_SPEAKERS]; 20];
        activity[0][0] = 1.0;
        activity[1][0] = 1.0;
        let fa = FrameActivations {
            activity,
            frame_duration: FRAME_DURATION_S,
        };
        let segs = fa.to_speaker_segments(0.3, 0.5);
        assert!(segs.is_empty(), "short segment should be filtered out");
    }

    #[test]
    fn frame_activations_gap_merging() {
        // Two short bursts close together should merge
        let mut activity = vec![[0.0f32; MAX_SPEAKERS]; 100];
        // Burst 1: frames 0-17 (~0.3s)
        for f in 0..18 {
            activity[f][0] = 1.0;
        }
        // Gap: frames 18-24 (~7 frames, ~0.12s < 0.5s min_gap)
        // Burst 2: frames 25-42 (~0.3s)
        for f in 25..43 {
            activity[f][0] = 1.0;
        }
        let fa = FrameActivations {
            activity,
            frame_duration: FRAME_DURATION_S,
        };
        let segs = fa.to_speaker_segments(0.2, 0.5);
        // Should merge into 1 segment since gap < 0.5s
        assert_eq!(segs.len(), 1, "close segments should merge: got {:?}", segs);
    }

    #[test]
    fn frame_activations_two_speakers() {
        let mut activity = vec![[0.0f32; MAX_SPEAKERS]; 60];
        // Speaker 0: frames 0-29
        for f in 0..30 {
            activity[f][0] = 1.0;
        }
        // Speaker 1: frames 30-59
        for f in 30..60 {
            activity[f][1] = 1.0;
        }
        let fa = FrameActivations {
            activity,
            frame_duration: FRAME_DURATION_S,
        };
        let segs = fa.to_speaker_segments(0.1, 0.5);
        assert_eq!(segs.len(), 2, "should find 2 segments");
        // Check they're different speakers
        let speakers: Vec<usize> = segs.iter().map(|s| s.2).collect();
        assert!(speakers.contains(&0));
        assert!(speakers.contains(&1));
    }

    #[test]
    fn empty_audio_returns_empty_activations() {
        // FrameActivations with empty activity
        let fa = FrameActivations {
            activity: Vec::new(),
            frame_duration: FRAME_DURATION_S,
        };
        let segs = fa.to_speaker_segments(0.3, 0.5);
        assert!(segs.is_empty());
    }

    #[test]
    fn frames_per_step_is_reasonable() {
        let fps = frames_per_step();
        // Should be roughly 16000/270 ≈ 59
        assert!(fps >= 55 && fps <= 65, "frames_per_step should be ~59, got {fps}");
    }
}
