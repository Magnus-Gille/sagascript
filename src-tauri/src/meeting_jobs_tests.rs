use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use sagascript_core::meeting::{
    MeetingExportFormat, MeetingSegmentInput, MeetingSpeaker, MeetingTranscript,
};

use super::*;

struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "sagascript-meeting-jobs-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).expect("create unique meeting test directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let Some(name) = self.path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name.starts_with("sagascript-meeting-jobs-test-") && self.path.is_dir() {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

fn valid_transcript() -> MeetingTranscript {
    MeetingTranscript::new(
        "a".repeat(64),
        "en",
        "whisper-base",
        3.0,
        vec![MeetingSegmentInput {
            start: 0.0,
            end: 1.0,
            text: "Hello meeting".into(),
            speaker: "speaker-1".into(),
        }],
        vec![MeetingSpeaker {
            id: "speaker-1".into(),
            label: "Speaker 1".into(),
        }],
    )
    .expect("valid meeting fixture")
}

fn snapshot(id: &str, status: JobStatus) -> MeetingSnapshot {
    MeetingSnapshot {
        id: id.into(),
        status,
        phase: "preparing".into(),
        error: Some("old error".into()),
        transcript: Some(valid_transcript()),
    }
}

fn job(id: &str, status: JobStatus) -> MeetingJob {
    MeetingJob {
        snapshot: snapshot(id, status),
        cancelled: Arc::new(AtomicBool::new(false)),
        backend: None,
    }
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
                .is_some_and(|name| name.starts_with(".sagascript-export-"))
        })
        .collect()
}

#[test]
fn finish_snapshot_applies_cancel_timeout_and_errors_without_stale_data() {
    let document = valid_transcript();

    let mut cancelled = snapshot("cancelled", JobStatus::Running);
    finish_snapshot(&mut cancelled, Ok(document.clone()), true, false);
    assert_eq!(cancelled.status, JobStatus::Cancelled);
    assert!(cancelled.error.is_none());
    assert!(cancelled.transcript.is_none());

    let mut timed_out = snapshot("timed-out", JobStatus::Running);
    finish_snapshot(&mut timed_out, Ok(document.clone()), true, true);
    assert_eq!(timed_out.status, JobStatus::Failed);
    assert_eq!(
        timed_out.error.as_deref(),
        Some("Meeting import timed out and has stopped. Try a shorter file or a smaller model.")
    );
    assert!(timed_out.transcript.is_none());

    let mut failed = snapshot("failed", JobStatus::Running);
    finish_snapshot(&mut failed, Err("backend failed".into()), false, false);
    assert_eq!(failed.status, JobStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("backend failed"));
    assert!(failed.transcript.is_none());

    let mut completed = snapshot("completed", JobStatus::Running);
    finish_snapshot(&mut completed, Ok(document.clone()), false, false);
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
    let scratch = ScratchDir::new();
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
    let scratch = ScratchDir::new();
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
    let scratch = ScratchDir::new();
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
