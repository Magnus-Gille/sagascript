use super::*;
use crate::meeting::live::document::Source;

fn config() -> Config {
    Config {
        sample_rate: 16_000,
        pre_roll_ms: 100,
        min_speech_ms: 100,
        silence_ms: 100,
        max_segment_ms: 1_000,
        preview_interval_ms: 200,
        overlap_ms: 100,
    }
}

fn samples(length: usize) -> Vec<f32> {
    (0..length)
        .map(|index| (index % 17) as f32 / 17.0)
        .collect()
}

fn feed_in_chunks(chunk_lengths: &[usize]) -> Vec<WindowJob> {
    let mut segmenter = Segmenter::new(config(), Source::Microphone).expect("config");
    let all = samples(12_800);
    let mut offset = 0;
    let mut output = Vec::new();
    for &length in chunk_lengths {
        let end = offset + length;
        output.extend(
            segmenter
                .push(offset as u64, &all[offset..end], true)
                .expect("contiguous chunk"),
        );
        offset = end;
    }
    output.extend(
        segmenter
            .push(offset as u64, &samples(2_000), false)
            .expect("contiguous silence"),
    );
    output.extend(segmenter.finish());
    output
}

#[test]
fn chunk_boundaries_do_not_change_windows() {
    let one_chunk = feed_in_chunks(&[12_800]);
    let split_chunks = feed_in_chunks(&[1_000, 3_000, 2_000, 4_800, 2_000]);
    assert_eq!(one_chunk, split_chunks);
}

#[test]
fn thirty_minutes_of_continuous_speech_stays_bounded() {
    let mut segmenter = Segmenter::new(config(), Source::System).expect("config");
    let chunk = vec![0.25; 4_800];
    for index in 0..6_000u64 {
        let output = segmenter
            .push(index * chunk.len() as u64, &chunk, true)
            .expect("contiguous speech");
        assert!(output.iter().all(|job| job.pcm.len() <= 16_000));
        assert!(segmenter.buffered_samples() <= 16_000);
    }
}

#[test]
fn max_length_cuts_preserve_overlap_offsets() {
    let mut segmenter = Segmenter::new(config(), Source::Microphone).expect("config");
    let output = segmenter.push(0, &samples(40_000), true).expect("speech");
    let finals: Vec<_> = output.iter().filter(|job| job.final_window).collect();

    assert!(finals.len() >= 2);
    assert_eq!((finals[0].start_sample, finals[0].end_sample), (0, 16_000));
    assert_eq!(
        (finals[1].start_sample, finals[1].end_sample),
        (14_400, 30_400)
    );
    assert_eq!(finals[0].end_sample - finals[1].start_sample, 1_600);
}

#[test]
fn exact_max_boundary_does_not_flush_overlap_as_a_duplicate() {
    let mut segmenter = Segmenter::new(config(), Source::Microphone).expect("config");
    let mut output = segmenter.push(0, &samples(16_000), true).expect("speech");
    output.extend(segmenter.finish());
    assert_eq!(
        output.iter().filter(|job| job.final_window).count(),
        1,
        "the overlap is waiting for new audio"
    );
}

#[test]
fn previews_share_id_and_final_revision_is_higher() {
    let mut segmenter = Segmenter::new(config(), Source::Microphone).expect("config");
    let mut output = segmenter.push(0, &samples(8_000), true).expect("speech");
    output.extend(
        segmenter
            .push(8_000, &samples(2_000), false)
            .expect("silence"),
    );
    let final_job = output
        .iter()
        .find(|job| job.final_window)
        .expect("final window");
    let previews: Vec<_> = output.iter().filter(|job| !job.final_window).collect();
    assert!(!previews.is_empty());
    assert!(previews.iter().all(|job| job.id == final_job.id));
    assert!(final_job.revision > previews.iter().map(|job| job.revision).max().unwrap());
}

#[test]
fn abort_discards_pending_audio_and_does_not_flush_stale_state() {
    let mut segmenter = Segmenter::new(config(), Source::Microphone).expect("config");
    assert!(!segmenter
        .push(0, &samples(2_000), true)
        .expect("speech")
        .is_empty());
    segmenter.abort();
    assert!(segmenter.finish().is_empty());

    let output = segmenter
        .push(2_000, &samples(2_000), true)
        .expect("new stream");
    assert!(output.iter().all(|job| job.id > 0));
}

#[test]
fn finish_flushes_short_final_word_without_clipping() {
    let mut segmenter = Segmenter::new(
        Config {
            min_speech_ms: 1_000,
            ..config()
        },
        Source::Microphone,
    )
    .expect("config");
    let output = segmenter.push(0, &samples(400), true).expect("short word");
    assert!(output.is_empty());
    let flushed = segmenter.finish();
    assert_eq!(flushed.len(), 1);
    assert!(flushed[0].final_window);
    assert_eq!(flushed[0].pcm.len(), 400);
}

#[test]
fn signal_gap_is_rejected() {
    let mut segmenter = Segmenter::new(config(), Source::Microphone).expect("config");
    segmenter
        .push(100, &samples(10), false)
        .expect("first chunk");
    assert_eq!(
        segmenter.push(111, &samples(10), false),
        Err(SegmentError::DiscontinuousInput)
    );
}

#[test]
fn invalid_config_and_nonfinite_pcm_are_rejected() {
    for invalid in [
        Config {
            sample_rate: 8_000,
            ..config()
        },
        Config {
            max_segment_ms: 30_001,
            ..config()
        },
        Config {
            preview_interval_ms: 1_001,
            ..config()
        },
        Config {
            overlap_ms: 1_000,
            ..config()
        },
    ] {
        assert!(matches!(
            Segmenter::new(invalid, Source::Microphone),
            Err(SegmentError::InvalidConfig)
        ));
    }

    let mut segmenter = Segmenter::new(config(), Source::Microphone).expect("config");
    assert_eq!(
        segmenter.push(0, &[0.0, f32::NAN], true),
        Err(SegmentError::NonFinitePcm)
    );
}
