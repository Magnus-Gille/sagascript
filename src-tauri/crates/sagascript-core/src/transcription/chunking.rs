const SAMPLE_RATE: usize = 16_000;
const SEARCH_RADIUS_SAMPLES: usize = 15 * SAMPLE_RATE;
const ENERGY_WINDOW_SAMPLES: usize = SAMPLE_RATE * 2 / 5;
const SEARCH_STRIDE_SAMPLES: usize = SAMPLE_RATE / 100;
const DECODE_OVERLAP_SAMPLES: usize = 5 * SAMPLE_RATE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AudioChunk {
    /// Non-overlapping ownership interval used for progress and ordering.
    pub(crate) start_sample: usize,
    pub(crate) end_sample: usize,
    /// Audio actually decoded. Internal boundaries include context on both
    /// sides so a word crossing the ownership boundary is never hard-cut.
    pub(crate) decode_start_sample: usize,
    pub(crate) decode_end_sample: usize,
}

/// Split audio near balanced boundaries, preferring quiet points within 15 seconds.
///
/// The planner samples boundary candidates every 10 ms and measures a centered
/// 400 ms energy window. Its work is bounded by the number of requested chunks,
/// rather than the total recording length, and it allocates no audio-sized buffer.
pub(crate) fn plan_chunks(
    audio: &[f32],
    requested_chunks: usize,
    min_chunk_samples: usize,
) -> Vec<AudioChunk> {
    if audio.is_empty() {
        return Vec::new();
    }

    let minimum = min_chunk_samples.max(1);
    let feasible_chunks = (audio.len() / minimum).max(1);
    let chunk_count = requested_chunks
        .max(1)
        .min(feasible_chunks)
        .min(audio.len());
    if chunk_count == 1 {
        return vec![AudioChunk {
            start_sample: 0,
            end_sample: audio.len(),
            decode_start_sample: 0,
            decode_end_sample: audio.len(),
        }];
    }

    let mut boundaries = Vec::with_capacity(chunk_count + 1);
    boundaries.push(0);

    for boundary_index in 1..chunk_count {
        let previous = *boundaries.last().expect("initial boundary exists");
        let remaining_chunks = chunk_count - boundary_index;
        let earliest = previous + minimum;
        let latest = audio.len() - remaining_chunks * minimum;
        let ideal = audio.len() * boundary_index / chunk_count;
        let balanced = ideal.clamp(earliest, latest);
        let search_start = ideal.saturating_sub(SEARCH_RADIUS_SAMPLES).max(earliest);
        let search_end = ideal.saturating_add(SEARCH_RADIUS_SAMPLES).min(latest);

        let boundary = if search_start <= search_end {
            quietest_boundary(audio, search_start, search_end, balanced)
        } else {
            balanced
        };
        boundaries.push(boundary);
    }
    boundaries.push(audio.len());

    boundaries
        .windows(2)
        .enumerate()
        .map(|(index, pair)| AudioChunk {
            start_sample: pair[0],
            end_sample: pair[1],
            decode_start_sample: if index == 0 {
                pair[0]
            } else {
                pair[0].saturating_sub(DECODE_OVERLAP_SAMPLES)
            },
            decode_end_sample: if index + 1 == chunk_count {
                pair[1]
            } else {
                pair[1]
                    .saturating_add(DECODE_OVERLAP_SAMPLES)
                    .min(audio.len())
            },
        })
        .collect()
}

fn quietest_boundary(audio: &[f32], start: usize, end: usize, ideal: usize) -> usize {
    let mut best = ideal.clamp(start, end);
    let mut best_energy = window_energy(audio, best);

    let mut consider = |candidate: usize| {
        let energy = window_energy(audio, candidate);
        let ordering = energy.total_cmp(&best_energy);
        let distance = candidate.abs_diff(ideal);
        let best_distance = best.abs_diff(ideal);
        if ordering.is_lt()
            || (ordering.is_eq()
                && (distance < best_distance || (distance == best_distance && candidate < best)))
        {
            best = candidate;
            best_energy = energy;
        }
    };

    consider(start);
    consider(end);
    let mut candidate = start;
    while candidate <= end {
        consider(candidate);
        let Some(next) = candidate.checked_add(SEARCH_STRIDE_SAMPLES) else {
            break;
        };
        candidate = next;
    }
    best
}

fn window_energy(audio: &[f32], center: usize) -> f64 {
    let half_window = ENERGY_WINDOW_SAMPLES / 2;
    let start = center.saturating_sub(half_window);
    let end = center.saturating_add(half_window).min(audio.len());
    let sum = audio[start..end].iter().fold(0.0, |sum, sample| {
        if sample.is_finite() {
            let sample = f64::from(*sample);
            sum + sample * sample
        } else {
            f64::INFINITY
        }
    });
    sum / (end - start).max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::{plan_chunks, AudioChunk};

    #[test]
    fn empty_and_single_chunk_inputs_are_total() {
        assert!(plan_chunks(&[], 4, 10).is_empty());
        let audio = vec![0.0; 100];
        let full = AudioChunk {
            start_sample: 0,
            end_sample: 100,
            decode_start_sample: 0,
            decode_end_sample: 100,
        };
        assert_eq!(plan_chunks(&audio, 0, 10), vec![full]);
        assert_eq!(plan_chunks(&audio, 1, 10), vec![full]);
    }

    #[test]
    fn chunks_cover_audio_contiguously_and_respect_minimum() {
        let audio = vec![0.25; 100_003];
        let chunks = plan_chunks(&audio, 4, 20_000);
        assert_eq!(chunks.len(), 4);
        assert_eq!(chunks.first().unwrap().start_sample, 0);
        assert_eq!(chunks.last().unwrap().end_sample, audio.len());
        for pair in chunks.windows(2) {
            assert_eq!(pair[0].end_sample, pair[1].start_sample);
        }
        assert!(chunks
            .iter()
            .all(|chunk| chunk.end_sample - chunk.start_sample >= 20_000));
        assert!(chunks.iter().all(|chunk| {
            chunk.decode_start_sample <= chunk.start_sample
                && chunk.decode_end_sample >= chunk.end_sample
        }));
    }

    #[test]
    fn boundary_moves_into_silent_gap_near_ideal() {
        let mut audio = vec![1.0; 64_000];
        audio[35_200..43_200].fill(0.0);
        let chunks = plan_chunks(&audio, 2, 10_000);
        assert_eq!(chunks.len(), 2);
        assert!((38_400..=40_000).contains(&chunks[0].end_sample));
    }

    #[test]
    fn continuous_speech_chunks_decode_across_the_core_boundary() {
        let audio = vec![0.5; 10 * 60 * 16_000];
        let chunks = plan_chunks(&audio, 2, 60 * 16_000);

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].end_sample, chunks[1].start_sample);
        assert_eq!(
            chunks[0].decode_end_sample - chunks[0].end_sample,
            5 * 16_000
        );
        assert_eq!(
            chunks[1].start_sample - chunks[1].decode_start_sample,
            5 * 16_000
        );
        assert!(chunks[0].decode_end_sample > chunks[1].decode_start_sample);
    }

    #[test]
    fn infeasible_request_is_capped_without_empty_chunks() {
        let audio = vec![0.0; 1_000];
        let chunks = plan_chunks(&audio, 100, 400);
        assert_eq!(chunks.len(), 2);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.end_sample > chunk.start_sample));
        assert!(chunks
            .iter()
            .all(|chunk| chunk.end_sample - chunk.start_sample >= 400));
    }

    #[test]
    fn flat_audio_prefers_exact_balanced_boundaries() {
        let audio = vec![0.5; 80_000];
        let first = plan_chunks(&audio, 4, 10_000);
        let second = plan_chunks(&audio, 4, 10_000);
        assert_eq!(first, second);
        assert_eq!(
            first,
            vec![
                AudioChunk {
                    start_sample: 0,
                    end_sample: 20_000,
                    decode_start_sample: 0,
                    decode_end_sample: 80_000,
                },
                AudioChunk {
                    start_sample: 20_000,
                    end_sample: 40_000,
                    decode_start_sample: 0,
                    decode_end_sample: 80_000,
                },
                AudioChunk {
                    start_sample: 40_000,
                    end_sample: 60_000,
                    decode_start_sample: 0,
                    decode_end_sample: 80_000,
                },
                AudioChunk {
                    start_sample: 60_000,
                    end_sample: 80_000,
                    decode_start_sample: 0,
                    decode_end_sample: 80_000,
                },
            ]
        );
    }

    #[test]
    fn zero_minimum_remains_safe() {
        let audio = vec![0.0; 3];
        let chunks = plan_chunks(&audio, 10, 0);
        assert_eq!(chunks.len(), 3);
        assert!(chunks
            .iter()
            .all(|chunk| chunk.end_sample > chunk.start_sample));
    }
}
