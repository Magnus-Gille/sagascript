use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sagascript_core::meeting::{MeetingExportFormat, MeetingTranscript};
use sagascript_core::test_support::meeting_transcript;
use sagascript_testsupport::TempDir;

use super::*;

fn valid_transcript() -> MeetingTranscript {
    meeting_transcript()
        .model("whisper-base")
        .duration(3.0)
        .segment(0.0, 1.0, "Hello meeting", "speaker-1")
        .speaker("speaker-1", "Speaker 1")
        .build()
}

fn snapshot(id: &str, status: JobStatus) -> MeetingSnapshot {
    MeetingSnapshot {
        id: id.into(),
        status,
        phase: "preparing".into(),
        elapsed_ms: 0,
        phase_elapsed_ms: 0,
        error: Some("old error".into()),
        transcript: Some(valid_transcript()),
        #[cfg(feature = "diarization")]
        reprocessing: None,
    }
}

#[cfg(feature = "diarization")]
fn reprocessing_result() -> sagascript_cli::meeting_reprocessing::ReprocessingResult {
    let previous = sagascript_core::meeting_review::MeetingReview::new(valid_transcript())
        .expect("valid synthetic review");
    let proposal = sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal::new(
        previous.clone(),
        previous.original.clone(),
    )
    .expect("valid synthetic proposal");
    sagascript_cli::meeting_reprocessing::ReprocessingResult {
        proposal,
        required_work: sagascript_core::meeting_reprocess_plan::RequiredWork {
            decode_audio: false,
            transcription: false,
            language_detection: false,
            segmentation: false,
            embeddings: false,
            clustering: true,
        },
        timings: Default::default(),
    }
}

fn job(id: &str, status: JobStatus) -> MeetingJob {
    let started_at = Instant::now();
    MeetingJob {
        snapshot: snapshot(id, status),
        started_at,
        phase_started_at: started_at,
        cancelled: Arc::new(AtomicBool::new(false)),
        backend: None,
    }
}

#[test]
fn meeting_clock_tracks_phase_changes_and_freezes_after_worker_exit() {
    let mut running = job("clock", JobStatus::Running);
    let started = running.started_at;
    let first = running.snapshot_at(started + Duration::from_secs(65));
    assert_eq!(first.elapsed_ms, 65_000);
    assert_eq!(first.phase_elapsed_ms, 65_000);

    running.set_phase("analyzing".into(), started + Duration::from_secs(65));
    // Repeated phase checkpoints and polls must not restart the phase clock.
    running.set_phase("analyzing".into(), started + Duration::from_secs(80));
    let second = running.snapshot_at(started + Duration::from_secs(90));
    assert_eq!(second.elapsed_ms, 90_000);
    assert_eq!(second.phase_elapsed_ms, 25_000);

    running.snapshot.status = JobStatus::Cancelling;
    let cancelling = running.snapshot_at(started + Duration::from_secs(95));
    assert_eq!(cancelling.elapsed_ms, 95_000);
    assert_eq!(cancelling.phase_elapsed_ms, 30_000);

    running.snapshot = cancelling;
    finish_snapshot(&mut running.snapshot, Err("cancelled".into()), true, false);
    let finished = running.snapshot_at(started + Duration::from_secs(120));
    assert_eq!(finished.elapsed_ms, 95_000);
    assert_eq!(finished.phase_elapsed_ms, 30_000);
    let json = serde_json::to_value(finished).expect("timing snapshot serializes");
    assert_eq!(json["elapsed_ms"], 95_000);
    assert_eq!(json["phase_elapsed_ms"], 30_000);
}

fn staging_files(path: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .expect("read scratch directory")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|entry| {
            entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.') && name.ends_with(".tmp"))
        })
        .collect()
}

#[test]
fn finish_snapshot_applies_cancel_timeout_and_errors_without_stale_data() {
    let document = valid_transcript();

    let mut cancelled = snapshot("cancelled", JobStatus::Running);
    #[cfg(feature = "diarization")]
    {
        cancelled.reprocessing = Some(reprocessing_result());
    }
    finish_snapshot(
        &mut cancelled,
        Ok(JobOutput::Import(Box::new(document.clone()))),
        true,
        false,
    );
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(cancelled.error.is_none());
    assert!(cancelled.transcript.is_none());
    #[cfg(feature = "diarization")]
    assert!(cancelled.reprocessing.is_none());

    let mut timed_out = snapshot("timed-out", JobStatus::Running);
    #[cfg(feature = "diarization")]
    {
        timed_out.reprocessing = Some(reprocessing_result());
    }
    finish_snapshot(
        &mut timed_out,
        Ok(JobOutput::Import(Box::new(document.clone()))),
        true,
        true,
    );
    assert_eq!(timed_out.status, JobStatus::Failed);
    assert_eq!(
        timed_out.error.as_deref(),
        Some("Meeting import timed out and has stopped. Try a shorter file or a smaller model.")
    );
    assert!(timed_out.transcript.is_none());
    #[cfg(feature = "diarization")]
    assert!(timed_out.reprocessing.is_none());

    let mut failed = snapshot("failed", JobStatus::Running);
    #[cfg(feature = "diarization")]
    {
        failed.reprocessing = Some(reprocessing_result());
    }
    finish_snapshot(&mut failed, Err("backend failed".into()), false, false);
    assert_eq!(failed.status, JobStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("backend failed"));
    assert!(failed.transcript.is_none());
    #[cfg(feature = "diarization")]
    assert!(failed.reprocessing.is_none());

    let mut completed = snapshot("completed", JobStatus::Running);
    finish_snapshot(
        &mut completed,
        Ok(JobOutput::Import(Box::new(document.clone()))),
        false,
        false,
    );
    assert_eq!(completed.status, JobStatus::Completed);
    assert_eq!(completed.error, None);
    assert_eq!(completed.transcript, Some(document));
}

#[test]
fn cancel_job_rejects_foreign_ids_and_transitions_only_matching_running_jobs() {
    let mut slot = Some(job("running", JobStatus::Running));
    let result = cancel_job(&mut slot, "foreign");
    let refused = result.expect_err("a job that is not in the slot cannot be cancelled");
    assert_eq!(refused.code(), "not_found");
    assert_eq!(
        refused.to_string(),
        "This meeting job is no longer available."
    );
    let running = slot.as_ref().expect("running job remains");
    assert_eq!(running.snapshot.status, JobStatus::Running);
    assert!(!running.cancelled.load(Ordering::SeqCst));

    assert!(cancel_job(&mut slot, "running").expect("a running job is cancelled"));
    let running = slot.as_ref().expect("cancelling job remains");
    assert_eq!(running.snapshot.status, JobStatus::Cancelling);
    assert!(running.cancelled.load(Ordering::SeqCst));
    assert!(cancel_job(&mut slot, "running").expect("cancelling twice is fine"));

    let mut terminal = Some(job("done", JobStatus::Completed));
    assert!(!cancel_job(&mut terminal, "done").expect("a finished job has nothing to cancel"));
    let terminal = terminal.as_ref().expect("terminal job remains");
    assert_eq!(terminal.snapshot.status, JobStatus::Completed);
    assert!(!terminal.cancelled.load(Ordering::SeqCst));
}

#[cfg(feature = "diarization")]
#[test]
fn completed_reprocessing_stores_proposal_without_transcript() {
    let mut completed = snapshot("reprocessing", JobStatus::Running);
    let result = reprocessing_result();
    let expected_proposal = result.proposal.clone();

    finish_snapshot(
        &mut completed,
        Ok(JobOutput::Reprocessing(Box::new(result))),
        false,
        false,
    );

    assert_eq!(completed.status, JobStatus::Completed);
    assert!(completed.error.is_none());
    assert!(completed.transcript.is_none());
    assert_eq!(
        completed
            .reprocessing
            .expect("completed reprocessing result")
            .proposal,
        expected_proposal
    );
}

#[test]
fn export_formats_use_the_core_format_mapping() {
    let expected = [
        (ExportFormat::Plain, MeetingExportFormat::Plain, "txt"),
        (ExportFormat::Markdown, MeetingExportFormat::Markdown, "md"),
        (ExportFormat::Json, MeetingExportFormat::Json, "json"),
        (ExportFormat::Srt, MeetingExportFormat::Srt, "srt"),
        (ExportFormat::Vtt, MeetingExportFormat::Vtt, "vtt"),
    ];
    let document = valid_transcript();
    for (app_format, core_format, extension) in expected {
        let (mapped, actual_extension) = app_format.details();
        assert_eq!(mapped, core_format);
        assert_eq!(actual_extension, extension);
        assert_eq!(
            document.export(mapped).expect("export fixture").as_bytes(),
            document
                .export(core_format)
                .expect("core export fixture")
                .as_bytes()
        );
    }
}

#[test]
fn write_new_export_writes_exact_bytes_and_never_replaces_existing_files() {
    let scratch = TempDir::new();
    let destination = scratch.path().join("meeting.txt");
    let bytes = b"exact export\n\0bytes";
    write_new_export(&destination, bytes).expect("publish new export");
    assert_eq!(std::fs::read(&destination).expect("read export"), bytes);
    assert!(staging_files(scratch.path()).is_empty());

    let existing = b"keep this file";
    std::fs::write(&destination, existing).expect("create existing export");
    let error = write_new_export(&destination, b"replacement").expect_err("reject overwrite");
    assert_eq!(error.code(), "export_exists");
    assert!(error.to_string().contains("already exists"));
    assert_eq!(
        std::fs::read(&destination).expect("read preserved export"),
        existing
    );
    assert!(staging_files(scratch.path()).is_empty());
}

#[test]
fn write_new_export_succeeds_when_only_the_temporary_file_cannot_be_removed() {
    use sagascript_core::test_support::FaultyFs;
    let scratch = TempDir::new();
    let destination = scratch.path().join("meeting.txt");

    write_new_export_with(
        &destination,
        b"exact export",
        &FaultyFs::new().failing_remove(),
    )
    .expect("the export is published, so the command succeeds");

    assert_eq!(
        std::fs::read(&destination).expect("read export"),
        b"exact export"
    );
}

#[cfg(unix)]
#[test]
fn write_new_export_refuses_existing_symlink_without_changing_target_or_link() {
    let scratch = TempDir::new();
    let target = scratch.path().join("target.txt");
    let link = scratch.path().join("meeting.txt");
    std::fs::write(&target, b"target bytes").expect("create symlink target");
    std::os::unix::fs::symlink(&target, &link).expect("create destination symlink");

    let error = write_new_export(&link, b"replacement").expect_err("reject symlink overwrite");
    assert_eq!(error.code(), "export_exists");
    assert!(error.to_string().contains("already exists"));
    assert_eq!(
        std::fs::read(&target).expect("read symlink target"),
        b"target bytes"
    );
    assert!(std::fs::symlink_metadata(&link)
        .expect("read symlink metadata")
        .file_type()
        .is_symlink());
    assert!(staging_files(scratch.path()).is_empty());
}

/// The note about a staging file that could not be removed names the file and
/// the folder the user chose, and carries no absolute path of its own: the
/// webview scrubs a folder out of a message, so a note that was only a path
/// would read `<path>/.sagascript-….tmp`.
#[test]
fn a_leftover_staging_file_is_named_without_its_folder() {
    use sagascript_core::test_support::FaultyFs;
    let scratch = TempDir::new();
    let destination = scratch.path().join("meeting.txt");

    let error = write_new_export_with(
        &destination,
        b"exact export",
        &FaultyFs::new().failing_link(1).failing_remove(),
    )
    .expect_err("the commit fails and the staging file stays");

    assert_eq!(error.code(), "io_error");
    let text = error.to_string();
    assert!(
        text.starts_with("The export was not saved, but its private temporary file ."),
        "{text}"
    );
    assert!(
        text.contains(" could not be removed from the folder you chose ("),
        "{text}"
    );
    assert!(text.ends_with("). Remove that file manually."), "{text}");
    let folder = scratch.path().to_string_lossy().into_owned();
    assert!(!text.contains(&folder), "no folder in the note: {text}");
    // The file it names is really there, in the folder of the export.
    let staging = staging_files(scratch.path());
    assert_eq!(staging.len(), 1);
    assert!(
        text.contains(
            &staging[0]
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        ),
        "{text}"
    );
}

#[test]
fn write_new_export_failures_say_what_kind_of_failure_they_are() {
    let scratch = TempDir::new();
    let missing_parent = scratch.path().join("missing").join("meeting.txt");
    let unwritable =
        write_new_export(&missing_parent, b"bytes").expect_err("no folder to write in");
    assert_eq!(unwritable.code(), "io_error");
    assert!(
        unwritable
            .to_string()
            .starts_with("Could not create export:"),
        "{unwritable}"
    );
    let nameless = write_new_export(std::path::Path::new(".."), b"bytes")
        .expect_err("a path without a file name is no destination");
    assert_eq!(nameless.code(), "invalid_input");
    assert_eq!(nameless.to_string(), "Choose a local export file.");
}

#[test]
fn write_new_export_leaves_no_partial_file_for_invalid_destinations() {
    let scratch = TempDir::new();
    let missing_parent = scratch.path().join("missing").join("meeting.txt");
    assert!(write_new_export(&missing_parent, b"bytes").is_err());
    assert!(!missing_parent.exists());
    assert!(!missing_parent.parent().expect("missing parent").exists());
    assert!(staging_files(scratch.path()).is_empty());

    let destination_directory = scratch.path().join("meeting-directory");
    std::fs::create_dir(&destination_directory).expect("create destination directory");
    assert!(write_new_export(&destination_directory, b"bytes").is_err());
    assert!(destination_directory.is_dir());
    assert!(staging_files(scratch.path()).is_empty());
}

/// #359: a panic elsewhere that poisoned the controller must not keep a
/// finished meeting job's lease (state stays Transcribing, blocking dictation
/// and further imports until the app restarts).
#[test]
fn finished_meeting_releases_its_lease_from_a_poisoned_controller() {
    let controller: SharedController = std::sync::Mutex::new(
        crate::app_controller::AppController::new(sagascript_core::settings::Settings::default()),
    );
    assert!(crate::commands::lock_controller(&controller).begin_meeting_job("meeting-1"));
    std::thread::scope(|scope| {
        let panicked = scope
            .spawn(|| {
                let _guard = controller.lock().unwrap();
                panic!("injected panic while holding the controller (#359)");
            })
            .join();
        assert!(panicked.is_err());
    });
    assert!(controller.is_poisoned());

    assert!(
        release_meeting_lease(&controller, "meeting-1", true),
        "the worker must release its lease despite the poison"
    );
    let mut ctrl = crate::commands::lock_controller(&controller);
    assert!(ctrl.is_update_result_pending("meeting:meeting-1"));
    assert!(
        ctrl.begin_meeting_job("meeting-2"),
        "the next import can start"
    );
}

/// The `get_meeting_job` payload as the shared fixture
/// contracts/transport/get_meeting_job.json (#347): one snapshot per job
/// status, produced by the job's own transitions (`snapshot_at`, `cancel_job`,
/// `finish_snapshot`), and parsed by the frontend's `MeetingJobSnapshot`
/// type (src/test/transport.test.ts).
mod transport_contract {
    use sagascript_cli::transcribe::MeetingPhase;
    use sagascript_core::diarization_report::{
        ActivitySpan, AsrSegmentEvidence, AttributionEvidence, AttributionReason, DecoderEvidence,
        DiarizationParameters, DiarizationReport, EmbeddingStatus, RegionEvidence, SpeakerSupport,
    };
    use sagascript_core::meeting::{MeetingSegmentInput, MeetingSpeaker};
    use sagascript_core::speaker_hint::{SpeakerCountHint, SpeakerHintOutcome};
    use serde_json::{Map, Value};

    use super::*;
    use crate::contract;

    const SOURCE: &str = "3b1f6c0f9a8e2d4c5b7a6e9d8c0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b";
    const DURATION: f64 = 6.0;

    /// Diarization evidence with every optional field both present and
    /// absent somewhere; it passes `DiarizationReport::validate`.
    fn report() -> DiarizationReport {
        DiarizationReport {
            schema_version: 1,
            source_sha256: SOURCE.into(),
            duration_seconds: DURATION,
            build_revision: "abc1234".into(),
            build_version: "1.4.4".into(),
            diagnostics_included: true,
            speaker_hint: Some(SpeakerCountHint::exact(2)),
            speaker_hint_outcome: Some(SpeakerHintOutcome {
                satisfied: true,
                delivered: 2,
            }),
            segmentation_model_sha256: Some("a".repeat(64)),
            embedding_model_sha256: None,
            decoder: Some(DecoderEvidence {
                strategy: "beam_search".into(),
                beam_size: 5,
                temperature_fallback: true,
                vad_enabled: true,
                vad_threshold: 0.5,
                vad_min_silence_duration_ms: 100,
                vad_speech_pad_ms: 30,
                vad_samples_overlap: 0.1,
                timestamp_method: "source_mapped_segments".into(),
            }),
            asr_segments: vec![
                AsrSegmentEvidence {
                    start: 0.0,
                    end: 2.5,
                    avg_logprob: Some(-0.25),
                    no_speech_prob: Some(0.01),
                },
                AsrSegmentEvidence {
                    start: 2.5,
                    end: 6.0,
                    avg_logprob: None,
                    no_speech_prob: None,
                },
            ],
            transcript_modified: false,
            parameters: DiarizationParameters {
                threshold: 0.7,
                min_segment_seconds: 0.5,
                min_gap_seconds: 0.25,
                min_speaker_seconds: 1.0,
                absorb_max_distance: 0.5,
                hint_merge_max_distance: 0.75,
                exclusive_speech_embeddings: false,
            },
            activity: vec![
                ActivitySpan {
                    start: 0.0,
                    end: 2.5,
                    speakers: vec!["s1".into()],
                },
                ActivitySpan {
                    start: 2.5,
                    end: 3.0,
                    speakers: vec!["s1".into(), "s2".into()],
                },
                ActivitySpan {
                    start: 3.0,
                    end: 6.0,
                    speakers: vec!["s2".into()],
                },
            ],
            regions: vec![
                RegionEvidence {
                    index: 0,
                    start: 0.0,
                    end: 3.0,
                    track: 0,
                    speaker: "s1".into(),
                    embedding_status: EmbeddingStatus::Usable,
                    assigned_centroid_distance: Some(0.25),
                    nearest_other_centroid_distance: Some(0.75),
                    used_track_fallback: false,
                    active_speech_seconds: Some(2.75),
                    overlapping_speech_seconds: Some(0.5),
                },
                RegionEvidence {
                    index: 1,
                    start: 2.5,
                    end: 6.0,
                    track: 1,
                    speaker: "s2".into(),
                    embedding_status: EmbeddingStatus::Missing,
                    assigned_centroid_distance: None,
                    nearest_other_centroid_distance: None,
                    used_track_fallback: true,
                    active_speech_seconds: None,
                    overlapping_speech_seconds: None,
                },
            ],
            attributions: vec![
                AttributionEvidence {
                    index: 0,
                    start: 0.0,
                    end: 2.0,
                    speaker: "s1".into(),
                    reason: AttributionReason::TemporalOverlap,
                    support: vec![SpeakerSupport {
                        speaker: "s1".into(),
                        overlap_seconds: 2.0,
                    }],
                    margin_seconds: Some(2.0),
                    gap_seconds: None,
                },
                AttributionEvidence {
                    index: 1,
                    start: 3.5,
                    end: 5.5,
                    speaker: "s2".into(),
                    reason: AttributionReason::NearestGap,
                    support: vec![],
                    margin_seconds: None,
                    gap_seconds: Some(0.5),
                },
            ],
        }
    }

    /// A diarized transcript with a speaker hint and its outcome.
    fn diarized_transcript() -> MeetingTranscript {
        let segment = |start: f64, end: f64, text: &str, speaker: &str| MeetingSegmentInput {
            start,
            end,
            text: text.into(),
            speaker: speaker.into(),
        };
        let speaker = |id: &str, label: &str| MeetingSpeaker {
            id: id.into(),
            label: label.into(),
        };
        MeetingTranscript::new(
            SOURCE,
            "sv",
            "kb-whisper-small",
            DURATION,
            vec![
                segment(0.0, 2.5, "Hej och välkomna.", "s1"),
                segment(3.0, 6.0, "Tack, då börjar vi.", "s2"),
            ],
            vec![speaker("s1", "Talare 1"), speaker("s2", "Talare 2")],
        )
        .expect("valid synthetic transcript")
        .with_speaker_hint(Some(SpeakerCountHint::exact(2)))
        .with_speaker_hint_outcome(Some(SpeakerHintOutcome {
            satisfied: true,
            delivered: 2,
        }))
        .with_diarization(report())
        .expect("valid synthetic diarization")
    }

    /// A finished reprocessing run: a reviewed meeting (two correction
    /// operations, the first already resolved) and its re-clustered proposal.
    #[cfg(feature = "diarization")]
    fn reprocessing() -> sagascript_cli::meeting_reprocessing::ReprocessingResult {
        use sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal;
        use sagascript_core::meeting_review::{CorrectionFile, CorrectionOperation, MeetingReview};

        let original = diarized_transcript();
        let review = MeetingReview::new(original.clone()).expect("valid review");
        let rename = CorrectionOperation::RenameSpeaker {
            speaker_id: "s1".into(),
            label: "Astrid".into(),
        };
        let edit = CorrectionOperation::EditSegment {
            segment_id: original.segments[1].id.clone(),
            text: Some("Tack, då börjar vi nu.".into()),
            speaker_id: None,
        };
        let review = review
            .apply_corrections(&CorrectionFile {
                schema_version: 1,
                source_sha256: SOURCE.into(),
                original_revision: review.original_revision.clone(),
                expected_revision: review.revision.clone(),
                operations: vec![rename.clone(), edit],
            })
            .expect("valid corrections");
        let proposal = MeetingReprocessingProposal::new(review, original).expect("valid proposal");
        let proposal = proposal
            .resolve(&proposal.revision.clone(), 0, vec![rename])
            .expect("valid resolution");
        sagascript_cli::meeting_reprocessing::ReprocessingResult {
            proposal,
            required_work: sagascript_core::meeting_reprocess_plan::RequiredWork {
                decode_audio: false,
                transcription: false,
                language_detection: false,
                segmentation: false,
                embeddings: false,
                clustering: true,
            },
            timings: sagascript_cli::meeting_reprocessing::ReprocessingTimings {
                validation_seconds: 0.25,
                decode_seconds: 0.0,
                analysis_seconds: 0.0,
                clustering_seconds: 0.5,
                full_pipeline_seconds: 0.0,
                proposal_seconds: 0.125,
                total_seconds: 0.875,
            },
        }
    }

    /// Every meeting phase; the match fails to compile when one is added.
    fn meeting_phases() -> Vec<MeetingPhase> {
        let all = vec![
            MeetingPhase::Preparing,
            MeetingPhase::Decoding,
            MeetingPhase::LoadingModel,
            MeetingPhase::Analyzing,
            MeetingPhase::Clustering,
            MeetingPhase::Finalizing,
        ];
        for phase in &all {
            match phase {
                MeetingPhase::Preparing
                | MeetingPhase::Decoding
                | MeetingPhase::LoadingModel
                | MeetingPhase::Analyzing
                | MeetingPhase::Clustering
                | MeetingPhase::Finalizing => {}
            }
        }
        all
    }

    fn running_job() -> MeetingJob {
        let started_at = Instant::now();
        MeetingJob {
            snapshot: MeetingSnapshot {
                id: "job-1".into(),
                status: JobStatus::Running,
                phase: "preparing".into(),
                elapsed_ms: 0,
                phase_elapsed_ms: 0,
                error: None,
                transcript: None,
                #[cfg(feature = "diarization")]
                reprocessing: None,
            },
            started_at,
            phase_started_at: started_at,
            cancelled: Arc::new(AtomicBool::new(false)),
            backend: None,
        }
    }

    /// The snapshot `get_meeting_job` returns after the worker finished.
    fn finished(result: Result<JobOutput, String>, cancelled: bool) -> MeetingSnapshot {
        let job = running_job();
        let mut snapshot = job.snapshot_at(job.started_at + Duration::from_millis(4200));
        finish_snapshot(&mut snapshot, result, cancelled, false);
        snapshot
    }

    /// `{ variant: payload }`: every job status, a transcript with and
    /// without optional evidence, and (diarization builds) a reprocessing
    /// result.
    fn fixture() -> Value {
        let mut variants = Map::new();
        let mut add = |name: &str, snapshot: MeetingSnapshot| {
            variants.insert(name.into(), contract::to_value(snapshot));
        };

        for phase in meeting_phases() {
            let mut running = running_job();
            running.set_phase(
                phase_name(phase),
                running.started_at + Duration::from_millis(500),
            );
            let snapshot = running.snapshot_at(running.started_at + Duration::from_millis(1500));
            add(&format!("running_{}", snapshot.phase), snapshot);
        }

        let mut analyzing = running_job();
        analyzing.set_phase(
            phase_name(MeetingPhase::Analyzing),
            analyzing.started_at + Duration::from_millis(500),
        );
        let mut slot = Some(analyzing);
        assert!(cancel_job(&mut slot, "job-1").expect("the running job is cancelled"));
        let cancelling = slot.expect("the job stays in its slot");
        add(
            "cancelling",
            cancelling.snapshot_at(cancelling.started_at + Duration::from_millis(2000)),
        );

        add(
            "completed",
            finished(
                Ok(JobOutput::Import(Box::new(diarized_transcript()))),
                false,
            ),
        );
        add(
            "completed_plain",
            finished(
                Ok(JobOutput::Import(Box::new(super::valid_transcript()))),
                false,
            ),
        );
        add(
            "cancelled",
            finished(Err("Meeting transcription cancelled".into()), true),
        );
        add(
            "failed",
            finished(Err("No audio decoded from file".into()), false),
        );
        #[cfg(feature = "diarization")]
        add(
            "reprocessing",
            finished(Ok(JobOutput::Reprocessing(Box::new(reprocessing()))), false),
        );
        Value::Object(variants)
    }

    #[cfg(feature = "diarization")]
    #[test]
    fn transport_contract_get_meeting_job() {
        contract::assert_fixture("get_meeting_job.json", &fixture());
    }

    /// Without diarization the `reprocessing` field and variant do not exist;
    /// everything else is identical, so the frontend must treat the field as
    /// optional (it does: `reprocessing?:`).
    #[cfg(not(feature = "diarization"))]
    #[test]
    fn transport_contract_get_meeting_job() {
        assert!(
            !contract::blessing(),
            "bless the fixtures from a diarization build (the default features)"
        );
        let mut expected = contract::read_fixture("get_meeting_job.json");
        let variants = expected.as_object_mut().expect("variants by name");
        variants.remove("reprocessing");
        for snapshot in variants.values_mut() {
            snapshot
                .as_object_mut()
                .expect("a snapshot object")
                .remove("reprocessing");
        }
        assert_eq!(fixture(), expected);
    }

    /// Rust reads the nested documents back unchanged.
    #[test]
    fn transport_contract_get_meeting_job_documents_read_back() {
        if contract::blessing() {
            return; // the fixture is being rewritten; the next run reads it
        }
        let fixture = contract::read_fixture("get_meeting_job.json");
        let variants = fixture.as_object().expect("variants by name");
        for (name, snapshot) in variants {
            if let Some(transcript) = snapshot.get("transcript").filter(|value| !value.is_null()) {
                let document: MeetingTranscript = serde_json::from_value(transcript.clone())
                    .unwrap_or_else(|error| panic!("{name}: {error}"));
                assert_eq!(&contract::to_value(document), transcript, "{name}");
            }
            if let Some(result) = snapshot
                .get("reprocessing")
                .filter(|value| !value.is_null())
            {
                let proposal: sagascript_core::meeting_reprocess_proposal::MeetingReprocessingProposal =
                    serde_json::from_value(result["proposal"].clone())
                        .unwrap_or_else(|error| panic!("{name}: {error}"));
                assert_eq!(contract::to_value(proposal), result["proposal"], "{name}");
            }
        }
        let statuses: std::collections::BTreeSet<_> = variants
            .values()
            .map(|snapshot| snapshot["status"].as_str().expect("a status").to_string())
            .collect();
        let all: std::collections::BTreeSet<_> = [
            JobStatus::Running,
            JobStatus::Cancelling,
            JobStatus::Completed,
            JobStatus::Cancelled,
            JobStatus::Failed,
        ]
        .into_iter()
        .map(|status| match status {
            JobStatus::Running
            | JobStatus::Cancelling
            | JobStatus::Completed
            | JobStatus::Cancelled
            | JobStatus::Failed => contract::to_value(status)
                .as_str()
                .expect("a status string")
                .to_string(),
        })
        .collect();
        assert_eq!(statuses, all, "the fixture covers every job status");
    }
}
