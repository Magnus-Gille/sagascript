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
    ///
    /// pyannote's speaker slots are *local to each 10s window* (slot 0 in one
    /// window may be a different physical speaker than slot 0 in the next), so
    /// each window's decoded activity is permutation-aligned against the
    /// accumulated global tracks over the overlap region before aggregation.
    /// See [`WindowStitcher`].
    pub fn segment(&mut self, audio: &[f32]) -> Result<FrameActivations, DictationError> {
        if audio.is_empty() {
            return Ok(FrameActivations {
                activity: Vec::new(),
                frame_duration: FRAME_DURATION_S,
            });
        }

        let n_windows = if audio.len() <= WINDOW_SAMPLES {
            1
        } else {
            (audio.len() - WINDOW_SAMPLES) / STEP_SAMPLES + 2
        };
        // Frame offsets are derived per-window from the sample position (not
        // win_idx * rounded-frames-per-step, which drifts ~0.26 frames/window).
        let total_frames =
            frame_offset_for_sample((n_windows - 1) * STEP_SAMPLES) + FRAMES_PER_WINDOW;

        let mut stitcher = WindowStitcher::new(total_frames);

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

            // Decode this window's powerset logits to binary per-slot activity
            // (argmax per frame), then let the stitcher align + accumulate.
            let mut local = Vec::with_capacity(FRAMES_PER_WINDOW);
            for f in 0..FRAMES_PER_WINDOW {
                let mut best_class = 0usize;
                let mut best_logit = f32::MIN;
                for c in 0..NUM_CLASSES {
                    let v = logits[[0, f, c]];
                    if v > best_logit {
                        best_logit = v;
                        best_class = c;
                    }
                }
                local.push(POWERSET[best_class]);
            }

            stitcher.add_window(frame_offset_for_sample(win_start), &local);
        }

        Ok(FrameActivations {
            activity: stitcher.finish(),
            frame_duration: FRAME_DURATION_S,
        })
    }
}

/// Global frame index for a sample position (receptive_field_shift = 270 samples).
fn frame_offset_for_sample(sample: usize) -> usize {
    (sample as f64 / (FRAME_DURATION_S * 16_000.0)).round() as usize
}

/// All permutations of the 3 local speaker slots. Identity first, so that
/// silence/no-signal overlap regions tie-break to "no relabeling".
const SLOT_PERMUTATIONS: [[usize; MAX_SPEAKERS]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

/// Pick the local→global slot permutation maximizing activity agreement over
/// the overlap region. `local[f][s]` is this window's binary activity for
/// local slot `s`; `global_avg[f][g]` is the running mean activity of global
/// track `g` on the same frames. Returns `perm` where local slot `s` maps to
/// global track `perm[s]`. Ties resolve to the earliest (identity-first) entry.
fn best_permutation(
    local: &[[f32; MAX_SPEAKERS]],
    global_avg: &[[f32; MAX_SPEAKERS]],
) -> [usize; MAX_SPEAKERS] {
    let mut best = SLOT_PERMUTATIONS[0];
    let mut best_score = f32::MIN;
    for perm in SLOT_PERMUTATIONS {
        let mut score = 0.0f32;
        for (l, g) in local.iter().zip(global_avg.iter()) {
            for s in 0..MAX_SPEAKERS {
                score += l[s] * g[perm[s]];
            }
        }
        if score > best_score {
            best_score = score;
            best = perm;
        }
    }
    best
}

/// Accumulates per-window binary speaker activity into globally-consistent
/// tracks. Each incoming window is permutation-aligned against the running
/// average of already-accumulated frames in its overlap region, then majority
/// voting (`finish`) binarizes the result.
struct WindowStitcher {
    /// Sum of aligned per-track activity per frame.
    acc: Vec<[f32; MAX_SPEAKERS]>,
    /// Number of windows contributing to each frame.
    counts: Vec<u32>,
}

impl WindowStitcher {
    fn new(total_frames: usize) -> Self {
        Self {
            acc: vec![[0.0f32; MAX_SPEAKERS]; total_frames],
            counts: vec![0u32; total_frames],
        }
    }

    /// Add one window's binary activity starting at `frame_offset`.
    fn add_window(&mut self, frame_offset: usize, local: &[[f32; MAX_SPEAKERS]]) {
        if frame_offset >= self.acc.len() {
            return;
        }
        // Overlap = the window's leading frames already covered by previous
        // windows (windows arrive in order, so coverage is a prefix).
        let overlap_len = self.counts[frame_offset..]
            .iter()
            .take(local.len())
            .take_while(|&&c| c > 0)
            .count();

        let perm = if overlap_len == 0 {
            SLOT_PERMUTATIONS[0]
        } else {
            let global_avg: Vec<[f32; MAX_SPEAKERS]> = (0..overlap_len)
                .map(|f| {
                    let i = frame_offset + f;
                    let n = self.counts[i] as f32;
                    [
                        self.acc[i][0] / n,
                        self.acc[i][1] / n,
                        self.acc[i][2] / n,
                    ]
                })
                .collect();
            best_permutation(&local[..overlap_len], &global_avg)
        };

        for (f, frame) in local.iter().enumerate() {
            let i = frame_offset + f;
            if i >= self.acc.len() {
                break;
            }
            for s in 0..MAX_SPEAKERS {
                self.acc[i][perm[s]] += frame[s];
            }
            self.counts[i] += 1;
        }
    }

    /// Majority-vote binarization of the accumulated tracks.
    fn finish(self) -> Vec<[f32; MAX_SPEAKERS]> {
        self.acc
            .iter()
            .zip(self.counts.iter())
            .map(|(a, &c)| {
                let n = c.max(1) as f32;
                let mut out = [0.0f32; MAX_SPEAKERS];
                for s in 0..MAX_SPEAKERS {
                    out[s] = if a[s] / n >= 0.5 { 1.0 } else { 0.0 };
                }
                out
            })
            .collect()
    }
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
        for (c, row) in POWERSET.iter().enumerate().take(4).skip(1) {
            let sum: f32 = row.iter().sum();
            assert_eq!(sum, 1.0, "class {c} should activate exactly 1 speaker");
        }
    }

    #[test]
    fn powerset_pairs_activate_two() {
        // Classes 4-6 each activate exactly two speakers
        for (c, row) in POWERSET.iter().enumerate().take(7).skip(4) {
            let sum: f32 = row.iter().sum();
            assert_eq!(sum, 2.0, "class {c} should activate exactly 2 speakers");
        }
    }

    #[test]
    fn frame_activations_to_segments_basic() {
        // Speaker 0 active for frames 0-9, silent after
        let mut activity = vec![[0.0f32; MAX_SPEAKERS]; 20];
        for frame in activity.iter_mut().take(10) {
            frame[0] = 1.0;
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
        for frame in activity.iter_mut().take(18) {
            frame[0] = 1.0;
        }
        // Gap: frames 18-24 (~7 frames, ~0.12s < 0.5s min_gap)
        // Burst 2: frames 25-42 (~0.3s)
        for frame in activity.iter_mut().take(43).skip(25) {
            frame[0] = 1.0;
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
        for frame in activity.iter_mut().take(30) {
            frame[0] = 1.0;
        }
        // Speaker 1: frames 30-59
        for frame in activity.iter_mut().take(60).skip(30) {
            frame[1] = 1.0;
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
    fn frame_offset_no_cumulative_drift() {
        // Offsets must track the true sample position, not accumulate the
        // rounding error of a per-step frame count (16000/270 ≈ 59.26).
        assert_eq!(frame_offset_for_sample(0), 0);
        // Window 10 starts at sample 160000 → 160000/270 = 592.6 → 593.
        // The old `win_idx * round(59.26)` scheme would give 590.
        assert_eq!(frame_offset_for_sample(10 * STEP_SAMPLES), 593);
        // Window 50: 800000/270 = 2963.0 (old scheme: 2950 — 13 frames ≈ 0.22s off)
        assert_eq!(frame_offset_for_sample(50 * STEP_SAMPLES), 2963);
    }

    // -- slot permutation alignment --

    /// Frame with exactly one active local slot.
    fn frame(active_slot: usize) -> [f32; MAX_SPEAKERS] {
        let mut f = [0.0f32; MAX_SPEAKERS];
        f[active_slot] = 1.0;
        f
    }

    const SILENT: [f32; MAX_SPEAKERS] = [0.0, 0.0, 0.0];

    #[test]
    fn best_permutation_identity_when_aligned() {
        let local = vec![frame(0); 10];
        let global = vec![frame(0); 10];
        assert_eq!(best_permutation(&local, &global), [0, 1, 2]);
    }

    #[test]
    fn best_permutation_detects_swap() {
        // Same physical speaker: local slot 1, global track 0 → map 1→0.
        let local = vec![frame(1); 10];
        let global = vec![frame(0); 10];
        let perm = best_permutation(&local, &global);
        assert_eq!(perm[1], 0, "local slot 1 should map to global track 0, got {perm:?}");
    }

    #[test]
    fn best_permutation_silence_ties_to_identity() {
        let local = vec![SILENT; 10];
        let global = vec![SILENT; 10];
        assert_eq!(best_permutation(&local, &global), [0, 1, 2]);
    }

    #[test]
    fn best_permutation_two_speakers_crossed() {
        // Two concurrent speakers with slots crossed between window and global.
        let mut local = Vec::new();
        let mut global = Vec::new();
        for i in 0..20 {
            // Alternate frames: speaker A then speaker B.
            let (l, g) = if i % 2 == 0 {
                (frame(0), frame(2)) // A: local 0 ≡ global 2
            } else {
                (frame(2), frame(0)) // B: local 2 ≡ global 0
            };
            local.push(l);
            global.push(g);
        }
        let perm = best_permutation(&local, &global);
        assert_eq!(perm[0], 2);
        assert_eq!(perm[2], 0);
    }

    // -- window stitching --

    #[test]
    fn stitch_single_window_passthrough() {
        let mut st = WindowStitcher::new(10);
        let local: Vec<[f32; MAX_SPEAKERS]> =
            (0..10).map(|f| if f < 5 { frame(1) } else { SILENT }).collect();
        st.add_window(0, &local);
        let out = st.finish();
        for (f, fr) in out.iter().enumerate() {
            assert_eq!(fr[1], if f < 5 { 1.0 } else { 0.0 }, "frame {f}");
        }
    }

    /// The regression test for the collapse bug: one physical speaker that
    /// pyannote assigns to slot 0 in window A but slot 1 in window B must come
    /// out as a SINGLE global track, not two. Raw index-summed aggregation
    /// (the old behavior) splits it across two tracks.
    #[test]
    fn stitch_swapped_slots_reunifies_speaker() {
        let mut st = WindowStitcher::new(150);
        // Window A: frames 0..100, speaker continuously active in slot 0.
        st.add_window(0, &[frame(0); 100]);
        // Window B: frames 50..150, SAME speaker (still talking through the
        // overlap 50..100) but in local slot 1.
        st.add_window(50, &[frame(1); 100]);
        let out = st.finish();

        let active_per_track: Vec<usize> = (0..MAX_SPEAKERS)
            .map(|s| out.iter().filter(|f| f[s] >= 0.5).count())
            .collect();
        let total_active: usize = active_per_track.iter().sum();
        let tracks_used = active_per_track.iter().filter(|&&n| n > 0).count();

        assert_eq!(
            tracks_used, 1,
            "one continuous speaker must occupy one global track, got per-track {active_per_track:?}"
        );
        assert_eq!(total_active, 150, "speaker active for all 150 frames");
    }

    #[test]
    fn stitch_two_speakers_stay_distinct() {
        // Speaker A talks frames 0..60 (slot 0 in both windows); speaker B
        // talks frames 90..150 (slot 1 in window B). A brief overlap region
        // 50..60 lets alignment lock in.
        let mut st = WindowStitcher::new(150);
        let win_a: Vec<[f32; MAX_SPEAKERS]> =
            (0..100).map(|f| if f < 60 { frame(0) } else { SILENT }).collect();
        st.add_window(0, &win_a);
        let win_b: Vec<[f32; MAX_SPEAKERS]> = (0..100)
            .map(|f| {
                let g = f + 50;
                if g < 60 {
                    frame(0) // A still talking in overlap, same slot
                } else if g >= 90 {
                    frame(1) // B
                } else {
                    SILENT
                }
            })
            .collect();
        st.add_window(50, &win_b);
        let out = st.finish();

        let a_frames = out.iter().filter(|f| f[0] >= 0.5).count();
        let b_frames = out.iter().filter(|f| f[1] >= 0.5).count();
        assert_eq!(a_frames, 60, "speaker A track");
        assert_eq!(b_frames, 60, "speaker B track");
    }

    #[test]
    fn stitch_offset_beyond_total_is_ignored() {
        let mut st = WindowStitcher::new(10);
        st.add_window(20, &[frame(0); 5]); // out of range — must not panic
        let out = st.finish();
        assert!(out.iter().all(|f| f.iter().all(|&v| v == 0.0)));
    }

    #[test]
    fn stitch_window_clipped_at_total_frames() {
        let mut st = WindowStitcher::new(10);
        st.add_window(5, &[frame(2); 20]); // extends past end — clipped
        let out = st.finish();
        assert_eq!(out.iter().filter(|f| f[2] >= 0.5).count(), 5);
    }
}
