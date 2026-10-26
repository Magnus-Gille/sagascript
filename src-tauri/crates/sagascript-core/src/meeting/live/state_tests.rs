use super::*;
use crate::meeting::live::document::{Gap, LiveSegment, SegmentStatus, Source};

fn preview(id: &str, source: Source, revision: u64, text: &str) -> LiveSegment {
    LiveSegment {
        id: id.to_string(),
        source,
        start_ms: 100,
        end_ms: 300,
        text: text.to_string(),
        status: SegmentStatus::Preview,
        revision,
    }
}

fn final_segment(id: &str, source: Source, revision: u64, text: &str) -> LiveSegment {
    LiveSegment {
        status: SegmentStatus::Final,
        ..preview(id, source, revision, text)
    }
}

fn gap(source: Source, start_ms: u64, end_ms: u64, reason: &str) -> Gap {
    Gap {
        source,
        start_ms,
        end_ms,
        reason: reason.to_string(),
    }
}

#[test]
fn lifecycle_accepts_only_ordered_transitions() {
    let mut state = State::new("session-1".to_string()).expect("valid id");
    assert_eq!(state.snapshot().phase, Phase::Starting);
    assert_eq!(state.begin_stop(), Err(StateError::InvalidTransition));

    state.started().expect("starting to running");
    state.advance(250).expect("elapsed");
    state.begin_stop().expect("running to draining");
    state.finish(true).expect("draining to completed");

    let snapshot = state.snapshot();
    assert_eq!(snapshot.phase, Phase::Completed);
    assert_eq!(snapshot.elapsed_ms, 250);
    assert!(snapshot.audio_available);
    assert!(state.finish(false).is_err());
    assert!(state.cancel().is_err());
}

#[test]
fn failure_and_cancellation_are_terminal() {
    let mut failed = State::new("failed".to_string()).expect("valid id");
    failed.started().expect("running");
    failed.fail("engine stopped").expect("failed");
    assert_eq!(failed.snapshot().phase, Phase::Failed);
    assert_eq!(failed.snapshot().error.as_deref(), Some("engine stopped"));
    assert_eq!(failed.started(), Err(StateError::InvalidTransition));

    let mut cancelled = State::new("cancelled".to_string()).expect("valid id");
    cancelled.cancel().expect("cancel from starting");
    assert_eq!(cancelled.snapshot().phase, Phase::Cancelled);
    assert_eq!(
        cancelled.fail("late").unwrap_err(),
        StateError::InvalidTransition
    );
}

#[test]
fn stale_preview_is_rejected_and_final_wins_forever() {
    let mut state = State::new("session".to_string()).expect("valid id");
    state
        .upsert(preview("s1", Source::Microphone, 1, "one"))
        .expect("preview");
    assert_eq!(
        state.upsert(preview("s1", Source::Microphone, 1, "stale")),
        Err(StateError::StaleSegment)
    );
    state
        .upsert(preview("s1", Source::Microphone, 2, "two"))
        .expect("new preview");
    state
        .upsert(final_segment("s1", Source::Microphone, 3, "final"))
        .expect("final");
    assert_eq!(
        state.upsert(preview("s1", Source::Microphone, 4, "late preview")),
        Err(StateError::FinalSegment)
    );
    assert_eq!(
        state.upsert(final_segment("s1", Source::Microphone, 4, "late final")),
        Err(StateError::FinalSegment)
    );
    assert_eq!(state.snapshot().segments[0].text, "final");
}

#[test]
fn same_id_on_two_sources_and_overlapping_times_are_retained() {
    let mut state = State::new("session".to_string()).expect("valid id");
    state
        .upsert(final_segment("mic", Source::Microphone, 1, "mic"))
        .expect("microphone");
    state
        .upsert(final_segment("system", Source::System, 1, "system"))
        .expect("system");

    let snapshot = state.snapshot();
    assert_eq!(snapshot.segments.len(), 2);
    assert_eq!(snapshot.segments[0].source, Source::Microphone);
    assert_eq!(snapshot.segments[1].source, Source::System);
}

#[test]
fn global_id_collision_across_sources_is_rejected_atomically() {
    let mut state = State::new("session".to_string()).expect("valid id");
    state
        .upsert(final_segment("shared", Source::Microphone, 1, "mic"))
        .expect("microphone");
    let before = state.snapshot();
    assert_eq!(
        state.upsert(final_segment("shared", Source::System, 1, "system")),
        Err(StateError::IdCollision)
    );
    assert_eq!(state.snapshot(), before);
}

#[test]
fn gaps_coalesce_and_make_export_incomplete() {
    let mut state = State::new("session".to_string()).expect("valid id");
    state.advance(1_000).expect("elapsed");
    state
        .upsert(final_segment("s1", Source::Microphone, 1, "hello"))
        .expect("segment");
    state
        .append_gap(gap(Source::Microphone, 300, 500, "drop"))
        .expect("gap");
    state
        .append_gap(gap(Source::Microphone, 500, 700, "drop"))
        .expect("coalesced gap");
    state.started().expect("running");
    state.begin_stop().expect("draining");
    state.finish(true).expect("completed");

    let document = state
        .export_document("sv", "model", None)
        .expect("terminal export");
    assert!(!document.complete);
    assert_eq!(document.gaps.len(), 1);
    assert_eq!(
        (document.gaps[0].start_ms, document.gaps[0].end_ms),
        (300, 700)
    );
    assert_eq!(document.segments.len(), 1);
}

#[test]
fn export_requires_terminal_state_and_drops_previews() {
    let mut state = State::new("session".to_string()).expect("valid id");
    state.advance(1_000).expect("elapsed");
    state
        .upsert(preview("p1", Source::Microphone, 1, "preview"))
        .expect("preview");
    state
        .upsert(final_segment("f1", Source::System, 1, "final"))
        .expect("final");
    assert_eq!(
        state.export_document("sv", "model", None).unwrap_err(),
        StateError::InvalidTransition
    );
    state.started().expect("running");
    state.begin_stop().expect("draining");
    state.finish(true).expect("completed");
    let document = state.export_document("sv", "model", None).expect("export");
    assert_eq!(document.segments.len(), 1);
    assert_eq!(document.segments[0].id, "f1");
    assert!(!document.complete);
}

#[test]
fn capacity_refusal_is_atomic_for_rows_and_text() {
    let mut state = State::new("session".to_string()).expect("valid id");
    for index in 0..MAX_SEGMENTS {
        state
            .upsert(final_segment(
                &format!("s{index}"),
                Source::Microphone,
                1,
                "x",
            ))
            .expect("within row capacity");
    }
    let before = state.snapshot();
    assert_eq!(
        state.upsert(final_segment("overflow", Source::Microphone, 1, "x")),
        Err(StateError::Capacity)
    );
    assert_eq!(state.snapshot(), before);

    let mut text_state = State::new("text".to_string()).expect("valid id");
    let large = "x".repeat(MAX_SEGMENT_TEXT_BYTES);
    for index in 0..MAX_TOTAL_TEXT_BYTES / MAX_SEGMENT_TEXT_BYTES {
        text_state
            .upsert(final_segment(
                &format!("t{index}"),
                Source::Microphone,
                1,
                &large,
            ))
            .expect("within text capacity");
    }
    let before = text_state.snapshot();
    assert_eq!(
        text_state.upsert(final_segment("text-overflow", Source::System, 1, "x")),
        Err(StateError::TextTooLarge)
    );
    assert_eq!(text_state.snapshot(), before);
}

#[test]
fn health_and_elapsed_updates_are_validated() {
    let mut state = State::new("session".to_string()).expect("valid id");
    assert_eq!(
        state.set_source_last_audio_ms(Source::Microphone, Some(100)),
        Err(StateError::InvalidTime)
    );
    state.advance_by(100).expect("elapsed");
    state
        .set_source_connected(Source::Microphone, true)
        .expect("connected");
    state
        .set_source_signal_level(Source::Microphone, 0.5)
        .expect("signal");
    state
        .set_source_last_audio_ms(Source::Microphone, Some(100))
        .expect("audio time");
    state
        .set_source_last_text_ms(Source::Microphone, Some(100))
        .expect("text time");
    state
        .set_source_queued_ms(Source::Microphone, 50)
        .expect("queue");
    state
        .add_source_dropped_frames(Source::Microphone, 3)
        .expect("dropped");
    state
        .set_source_error(Source::Microphone, Some("device unavailable".to_string()))
        .expect("error");
    assert_eq!(state.snapshot().sources[0].dropped_frames, 3);
    assert_eq!(
        state.set_source_signal_level(Source::Microphone, f32::NAN),
        Err(StateError::InvalidHealth)
    );
    assert_eq!(state.advance(MAX_DURATION_MS), Err(StateError::InvalidTime));
}

#[test]
fn metadata_accessors_and_partial_audio_failure_are_available() {
    let mut state = State::new("session".to_string()).expect("valid id");
    assert_eq!(state.phase(), Phase::Starting);
    assert_eq!(state.elapsed_ms(), 0);
    state
        .finish_failed("asr stopped", true)
        .expect("failed with partial audio");
    assert_eq!(state.phase(), Phase::Failed);
    assert_eq!(state.source_status(Source::System).source, Source::System);
    assert!(state.snapshot().audio_available);
    assert_eq!(state.fail("late"), Err(StateError::InvalidTransition));
}

#[test]
fn construction_rejects_invalid_ids() {
    assert!(matches!(
        State::new(" ".to_string()),
        Err(StateError::InvalidId)
    ));
}

#[test]
fn refinement_lifecycle_freezes_live_state_and_tracks_monotonic_progress() {
    let mut state = State::new("refine".to_string()).expect("valid id");
    state.started().expect("running");
    state.advance(250).expect("elapsed");
    state
        .upsert(preview("live", Source::Microphone, 1, "live text"))
        .expect("preview");
    state.begin_stop().expect("draining");
    state.begin_refinement(true).expect("refining");

    let snapshot = state.snapshot();
    assert_eq!(snapshot.phase, Phase::Refining);
    assert_eq!(snapshot.elapsed_ms, 250);
    assert_eq!(
        snapshot.refinement.as_ref().unwrap().status,
        RefinementStatus::Running
    );
    assert_eq!(snapshot.refinement.as_ref().unwrap().progress_percent, 0);
    assert!(snapshot.refinement.as_ref().unwrap().error.is_none());
    assert!(snapshot.audio_available);

    state.set_refinement_progress(40).expect("progress");
    assert_eq!(
        state.set_refinement_progress(39),
        Err(StateError::InvalidHealth)
    );
    assert_eq!(
        state.set_refinement_progress(101),
        Err(StateError::InvalidHealth)
    );
    assert_eq!(state.advance(1), Err(StateError::InvalidTransition));
    assert_eq!(
        state.upsert(final_segment("late", Source::System, 1, "late")),
        Err(StateError::InvalidTransition)
    );
    assert_eq!(state.snapshot().elapsed_ms, 250);
    assert_eq!(
        state
            .snapshot()
            .refinement
            .as_ref()
            .unwrap()
            .progress_percent,
        40
    );
}

#[test]
fn invalid_refinement_replacement_is_transactional() {
    let mut state = State::new("refine-invalid".to_string()).expect("valid id");
    state.started().expect("running");
    state.advance(500).expect("elapsed");
    state
        .upsert(final_segment("old", Source::Microphone, 1, "old"))
        .expect("old segment");
    state
        .append_gap(gap(Source::System, 100, 200, "missing"))
        .expect("gap");
    state.begin_stop().expect("draining");
    state.begin_refinement(true).expect("refining");
    let before = state.snapshot();

    let invalid = vec![
        final_segment("new", Source::System, 2, "new"),
        preview("preview", Source::Microphone, 2, "not final"),
    ];
    assert_eq!(
        state.complete_refinement(invalid),
        Err(StateError::FinalSegment)
    );
    assert_eq!(state.snapshot(), before);
    assert_eq!(state.phase(), Phase::Refining);
}

#[test]
fn successful_refinement_replaces_segments_atomically_and_preserves_context() {
    let mut state = State::new("refine-success".to_string()).expect("valid id");
    state.started().expect("running");
    state.advance(500).expect("elapsed");
    state
        .set_source_connected(Source::System, true)
        .expect("source");
    state
        .append_gap(gap(Source::Microphone, 100, 200, "missing"))
        .expect("gap");
    state.begin_stop().expect("draining");
    state.begin_refinement(false).expect("refining");

    state
        .complete_refinement(vec![final_segment(
            "replacement",
            Source::System,
            9,
            "better",
        )])
        .expect("replacement");

    let snapshot = state.snapshot();
    assert_eq!(snapshot.phase, Phase::Completed);
    assert_eq!(snapshot.elapsed_ms, 500);
    assert!(!snapshot.audio_available);
    assert!(snapshot.sources[1].connected);
    assert_eq!(
        snapshot.gaps,
        vec![gap(Source::Microphone, 100, 200, "missing")]
    );
    assert_eq!(snapshot.segments.len(), 1);
    assert_eq!(snapshot.segments[0].text, "better");
    assert_eq!(
        snapshot.refinement.as_ref().unwrap().status,
        RefinementStatus::Completed
    );
    assert_eq!(snapshot.refinement.as_ref().unwrap().progress_percent, 100);
    assert!(snapshot.refinement.as_ref().unwrap().error.is_none());
}

#[test]
fn refinement_fallback_preserves_live_segments_and_gaps() {
    for (cancelled, status) in [
        (true, RefinementStatus::Cancelled),
        (false, RefinementStatus::Failed),
    ] {
        let mut state = State::new(format!("fallback-{cancelled}")).expect("valid id");
        state.started().expect("running");
        state.advance(500).expect("elapsed");
        state
            .upsert(final_segment("live", Source::Microphone, 1, "keep"))
            .expect("segment");
        state
            .append_gap(gap(Source::System, 100, 200, "missing"))
            .expect("gap");
        state.begin_stop().expect("draining");
        state.begin_refinement(true).expect("refining");
        state.set_refinement_progress(20).expect("progress");
        let before = state.snapshot();
        let message = (!cancelled).then_some("refinement failed".to_string());

        state
            .finish_refinement_fallback(cancelled, message.clone())
            .expect("fallback");
        let snapshot = state.snapshot();
        assert_eq!(snapshot.phase, Phase::Completed);
        assert_eq!(snapshot.elapsed_ms, before.elapsed_ms);
        assert_eq!(snapshot.sources, before.sources);
        assert_eq!(snapshot.segments, before.segments);
        assert_eq!(snapshot.gaps, before.gaps);
        assert_eq!(snapshot.audio_available, before.audio_available);
        assert_eq!(snapshot.error, message);
        assert_eq!(snapshot.refinement.as_ref().unwrap().status, status);
        assert_eq!(snapshot.refinement.as_ref().unwrap().progress_percent, 20);
        assert_eq!(snapshot.refinement.as_ref().unwrap().error, message);
    }
}

#[test]
fn refinement_snapshot_is_backward_compatible_and_statuses_are_snake_case() {
    let mut state = State::new("wire".to_string()).expect("valid id");
    let mut old = serde_json::to_value(state.snapshot()).expect("snapshot json");
    old.as_object_mut().unwrap().remove("refinement");
    let decoded: Snapshot = serde_json::from_value(old).expect("old snapshot");
    assert!(decoded.refinement.is_none());

    state.started().expect("running");
    state.begin_stop().expect("draining");
    state.begin_refinement(false).expect("refining");
    let json = serde_json::to_value(state.snapshot()).expect("refinement json");
    assert_eq!(json["phase"], "refining");
    assert_eq!(json["refinement"]["status"], "running");
}
