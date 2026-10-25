use super::*;

fn zeros(length: usize) -> Vec<f32> {
    vec![0.0; length]
}

#[test]
fn first_host_timestamp_anchors_timeline_and_missing_frames_are_zero() {
    let mut timeline = NativeTimeline::new(1_000_000_000);
    let outcome = timeline
        .push(1_100_000_000, 16_000, &vec![0.25; 160])
        .expect("anchored push");
    assert_eq!(outcome.offset_samples, 0);
    for start_sample in (0..=1_600).step_by(160) {
        let frame = timeline
            .drain_frame(start_sample)
            .expect("chronological frame");
        if start_sample == 1_600 {
            assert!(frame.iter().all(|value| *value == 0.25));
        } else {
            assert_eq!(frame, [0.0; 160]);
        }
    }
}

#[test]
fn drain_preserves_ramp_across_chunk_and_frame_boundaries() {
    let mut timeline = NativeTimeline::new(0);
    let ramp: Vec<f32> = (0..512).map(|sample| sample as f32).collect();
    timeline.push(0, 16_000, &ramp).expect("ramp push");

    let first = timeline.drain_frame(0).expect("first frame");
    assert_eq!(first, std::array::from_fn(|index| index as f32));
    let second = timeline.drain_frame(160).expect("cross-boundary frame");
    assert_eq!(second, std::array::from_fn(|index| (160 + index) as f32));
}

#[test]
fn drain_places_audio_at_non_aligned_anchor_without_dropping_it() {
    let mut timeline = NativeTimeline::new(0);
    let ramp: Vec<f32> = (0..256).map(|sample| sample as f32).collect();
    timeline
        .push(1_062_500, 16_000, &ramp)
        .expect("non-aligned ramp push");

    let first = timeline.drain_frame(0).expect("leading gap frame");
    assert!(first[..17].iter().all(|sample| *sample == 0.0));
    assert_eq!(first[17], 0.0);
    assert_eq!(first[159], 142.0);

    let second = timeline.drain_frame(160).expect("anchored audio frame");
    assert_eq!(second[0], 143.0);
    assert_eq!(second[112], 255.0);
    assert!(second[113..].iter().all(|sample| *sample == 0.0));
}

#[test]
fn separate_timelines_can_have_different_offsets() {
    let mut microphone = NativeTimeline::new(10_000);
    let mut system = NativeTimeline::new(10_000);
    microphone.push(10_000, 16_000, &vec![0.1; 160]).unwrap();
    system.push(110_000, 16_000, &vec![0.2; 160]).unwrap();
    assert!(microphone
        .drain_frame(0)
        .unwrap()
        .iter()
        .all(|sample| *sample == 0.1));
    let system_frame = system.drain_frame(0).unwrap();
    assert!(system_frame[..2].iter().all(|sample| *sample == 0.0));
    assert!(system_frame[2..].iter().all(|sample| *sample == 0.2));
}

#[test]
fn rate_changes_are_rejected() {
    let mut timeline = NativeTimeline::new(0);
    timeline.push(0, 16_000, &zeros(160)).unwrap();
    assert_eq!(
        timeline.push(10_000_000, 48_000, &zeros(480)),
        Err(TimelineError::RateChange)
    );
}

#[test]
fn invalid_first_rate_does_not_poison_timeline() {
    let mut timeline = NativeTimeline::new(0);
    assert!(matches!(
        timeline.push(0, 7_999, &zeros(160)),
        Err(TimelineError::Resampler(_))
    ));
    timeline
        .push(0, 16_000, &zeros(160))
        .expect("valid first rate remains usable");
}

#[test]
fn large_timestamp_jump_resets_with_discontinuity_interval() {
    let mut timeline = NativeTimeline::new(0);
    timeline.push(0, 16_000, &zeros(160)).unwrap();
    let outcome = timeline
        .push(100_000_000, 16_000, &vec![0.5; 160])
        .expect("forward discontinuity");
    let discontinuity = outcome.discontinuity.expect("interval");
    assert!(discontinuity.end_sample > discontinuity.start_sample);
    assert!(outcome.offset_samples > 700);
}

#[test]
fn backward_overlap_is_rejected() {
    let mut timeline = NativeTimeline::new(0);
    timeline.push(0, 16_000, &zeros(160)).unwrap();
    let outcome = timeline
        .push(100_000_000, 16_000, &zeros(160))
        .expect("forward discontinuity");
    assert!(outcome.discontinuity.is_some());
    assert_eq!(
        timeline.push(0, 16_000, &zeros(160)),
        Err(TimelineError::BackwardOverlap)
    );
}

#[test]
fn late_pcm_after_drain_is_rejected() {
    let mut timeline = NativeTimeline::new(0);
    timeline.drain_frame(0).unwrap();
    assert_eq!(
        timeline.push(0, 16_000, &zeros(160)),
        Err(TimelineError::LateData)
    );
}

#[test]
fn idle_flush_releases_filter_tail_and_reanchors_next_burst() {
    let mut timeline = NativeTimeline::new(0);
    timeline
        .push(0, 48_000, &vec![0.5; 480])
        .expect("short resampled burst");
    timeline.flush_idle().expect("idle flush");
    timeline.flush_idle().expect("idempotent idle flush");

    let first = timeline.drain_frame(0).expect("flushed frame");
    assert!(first.iter().filter(|sample| sample.abs() > 1.0e-6).count() >= 120);
    timeline
        .push(200_000_000, 48_000, &vec![0.25; 480])
        .expect("resumed burst");
    timeline.flush_idle().expect("flush resumed burst");
    let mut start = 160;
    while start < 3_200 {
        let frame = timeline.drain_frame(start).expect("idle gap frame");
        assert!(frame.iter().all(|sample| *sample == 0.0));
        start += 160;
    }
    let resumed = timeline.drain_frame(3_200).expect("resumed frame");
    assert!(
        resumed
            .iter()
            .filter(|sample| sample.abs() > 1.0e-6)
            .count()
            >= 120
    );
}

#[test]
fn idle_flush_rejects_overlap_and_finished_timeline() {
    let mut timeline = NativeTimeline::new(0);
    timeline
        .push(0, 48_000, &vec![0.5; 480])
        .expect("short resampled burst");
    timeline.flush_idle().expect("idle flush");
    assert_eq!(
        timeline.push(5_000_000, 48_000, &vec![0.25; 480]),
        Err(TimelineError::LateData)
    );
    timeline.finish().expect("finish after idle");
    assert_eq!(timeline.flush_idle(), Err(TimelineError::Finished));
}

#[test]
fn idle_flush_without_audio_is_safe_for_native_rate() {
    let mut timeline = NativeTimeline::new(0);
    timeline
        .push(0, 16_000, &vec![0.5; 160])
        .expect("native burst");
    timeline.flush_idle().expect("native idle flush");
    timeline.finish().expect("finish after idle");
}

#[test]
fn retained_pcm_is_bounded_and_finish_is_terminal() {
    let mut timeline = NativeTimeline::new(0);
    let mut saw_capacity = false;
    for index in 0..256u64 {
        match timeline.push(index * 128_000_000, 16_000, &zeros(2_048)) {
            Ok(_) => {}
            Err(TimelineError::Capacity) => {
                saw_capacity = true;
                break;
            }
            Err(error) => panic!("unexpected timeline error: {error:?}"),
        }
        assert!(timeline.retained_samples() <= MAX_RETAINED_SAMPLES);
        assert!(timeline.retained_chunks() <= MAX_RETAINED_CHUNKS);
    }
    assert!(saw_capacity);
    timeline.finish().expect("finish");
    timeline.finish().expect("idempotent finish");
    assert_eq!(
        timeline.push(256 * 128_000_000, 16_000, &zeros(1)),
        Err(TimelineError::Finished)
    );
}
