use super::*;

/// Moved from the former `segment_confidence_tests` (#371); the module name is kept so
/// the test ids stay stable.
#[cfg(test)]
mod segment_confidence_tests {
    use super::*;

    #[test]
    fn auto_parallelism_requires_long_beam_audio_and_bounded_model() {
        let long_audio = 10 * 60 * 16_000;
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperSmall, 5),
            2
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio - 1, WhisperModel::KbWhisperSmall, 5),
            1
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperSmall, 0),
            1
        );
        assert_eq!(
            recommended_parallel_chunks(long_audio, WhisperModel::KbWhisperLarge, 5),
            1
        );
    }
}

/// Characterisation of the parallel-chunk limits (#371). Chunk planning itself
/// is tested in `chunking.rs`; this pins the numbers this module feeds it and
/// the progress weights it derives from the plan.
#[cfg(test)]
mod parallel_limit_tests {
    use super::*;
    use crate::transcription::chunking::plan_chunks;

    const SECOND: usize = 16_000;

    fn planned(seconds: usize, requested: usize) -> Vec<AudioChunk> {
        let audio = vec![0.25; seconds * SECOND];
        plan_chunks(
            &audio,
            requested.clamp(1, MAX_PARALLEL_CHUNKS),
            MIN_PARALLEL_CHUNK_SAMPLES,
        )
    }

    #[test]
    fn the_limits_are_four_states_and_one_minute_pieces() {
        assert_eq!(MAX_PARALLEL_CHUNKS, 4);
        assert_eq!(MIN_PARALLEL_CHUNK_SAMPLES, 60 * SECOND);
        assert_eq!(AUTO_PARALLEL_MIN_SAMPLES, 10 * 60 * SECOND);
        assert_eq!(AUTO_PARALLEL_MAX_MODEL_MB, 500);
    }

    #[test]
    fn a_recording_is_never_split_into_pieces_shorter_than_a_minute() {
        assert_eq!(planned(59, 4).len(), 1);
        assert_eq!(planned(119, 4).len(), 1);
        assert_eq!(planned(120, 4).len(), 2);
        assert_eq!(planned(300, 4).len(), 4);
    }

    #[test]
    fn requests_of_zero_or_above_the_maximum_are_clamped() {
        assert_eq!(planned(600, 0).len(), 1);
        assert_eq!(planned(600, 1).len(), 1);
        assert_eq!(planned(600, 99).len(), MAX_PARALLEL_CHUNKS);
    }

    #[test]
    fn automatic_parallelism_starts_with_beam_two_and_never_asks_for_more_than_two() {
        let long = AUTO_PARALLEL_MIN_SAMPLES;
        let small = WhisperModel::KbWhisperSmall;
        assert_eq!(recommended_parallel_chunks(long, small, 1), 1);
        assert_eq!(recommended_parallel_chunks(long, small, 2), 2);
        assert_eq!(recommended_parallel_chunks(usize::MAX, small, 8), 2);
        assert!(recommended_parallel_chunks(usize::MAX, small, u32::MAX) <= MAX_PARALLEL_CHUNKS);
    }
}
