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
    assert!(result.is_err());
    let running = slot.as_ref().expect("running job remains");
    assert_eq!(running.snapshot.status, JobStatus::Running);
    assert!(!running.cancelled.load(Ordering::SeqCst));

    assert_eq!(cancel_job(&mut slot, "running"), Ok(true));
    let running = slot.as_ref().expect("cancelling job remains");
    assert_eq!(running.snapshot.status, JobStatus::Cancelling);
    assert!(running.cancelled.load(Ordering::SeqCst));
    assert_eq!(cancel_job(&mut slot, "running"), Ok(true));

    let mut terminal = Some(job("done", JobStatus::Completed));
    assert_eq!(cancel_job(&mut terminal, "done"), Ok(false));
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
    assert!(error.contains("already exists"));
    assert_eq!(
        std::fs::read(&destination).expect("read preserved export"),
        existing
    );
    assert!(staging_files(scratch.path()).is_empty());
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
    assert!(error.contains("already exists"));
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
