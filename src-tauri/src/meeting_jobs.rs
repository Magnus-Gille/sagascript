//! One explicit, in-memory meeting import. No library, capture, or auto-paste.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sagascript_core::meeting::{MeetingExportFormat, MeetingTranscript};
use sagascript_core::transcription::WhisperBackend;
use serde::{Deserialize, Serialize};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::command_error::CommandError;
use crate::commands::{file_transcription_context, lock_controller, SharedController};
#[cfg(feature = "diarization")]
use crate::events::{EmitEvent, UiState};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobStatus {
    Running,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}

impl JobStatus {
    fn busy(self) -> bool {
        matches!(self, Self::Running | Self::Cancelling)
    }
}

#[derive(Clone, Serialize)]
pub struct MeetingSnapshot {
    id: String,
    status: JobStatus,
    phase: String,
    elapsed_ms: u64,
    phase_elapsed_ms: u64,
    error: Option<String>,
    transcript: Option<MeetingTranscript>,
    #[cfg(feature = "diarization")]
    reprocessing: Option<sagascript_cli::meeting_reprocessing::ReprocessingResult>,
}

enum JobOutput {
    Import(Box<MeetingTranscript>),
    #[cfg(feature = "diarization")]
    Reprocessing(Box<sagascript_cli::meeting_reprocessing::ReprocessingResult>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReprocessingRequest {
    pub plan: sagascript_core::meeting_reprocess_plan::ReprocessingPlan,
    pub expected_revision: String,
    pub previous: sagascript_core::meeting_review::MeetingReview,
    pub cache_path: Option<String>,
    pub cache_output: Option<String>,
}

struct MeetingJob {
    snapshot: MeetingSnapshot,
    started_at: Instant,
    phase_started_at: Instant,
    cancelled: Arc<AtomicBool>,
    backend: Option<Arc<WhisperBackend>>,
}

impl MeetingJob {
    fn snapshot_at(&self, now: Instant) -> MeetingSnapshot {
        let mut snapshot = self.snapshot.clone();
        if snapshot.status.busy() {
            snapshot.elapsed_ms = now.saturating_duration_since(self.started_at).as_millis() as u64;
            snapshot.phase_elapsed_ms = now
                .saturating_duration_since(self.phase_started_at)
                .as_millis() as u64;
        }
        snapshot
    }

    #[cfg(any(feature = "diarization", test))]
    fn set_phase(&mut self, phase: String, now: Instant) {
        if self.snapshot.phase != phase {
            self.snapshot.phase = phase;
            self.phase_started_at = now;
        }
    }
}

/// The snapshot's `phase`: the closed enum's wire name, never text or a
/// source path. The frontend labels these in src/lib/meeting-progress.ts.
#[cfg(any(feature = "diarization", test))]
fn phase_name(phase: sagascript_cli::transcribe::MeetingPhase) -> String {
    serde_json::to_value(phase)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "processing".into())
}

#[derive(Default)]
pub struct MeetingJobs(Mutex<Option<MeetingJob>>);
pub type SharedMeetingJobs = Arc<MeetingJobs>;

fn lock_error() -> CommandError {
    CommandError::Unavailable(
        "Meeting job state is unavailable; restart the app before importing again.".into(),
    )
}

#[tauri::command]
pub async fn begin_meeting_file(
    app: tauri::AppHandle,
    jobs: State<'_, SharedMeetingJobs>,
    controller: State<'_, SharedController>,
    file_path: String,
    prompt: Option<String>,
    profile_id: Option<String>,
) -> Result<String, CommandError> {
    begin_job(app, jobs, controller, file_path, prompt, profile_id, None).await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri injects the application/window/state arguments.
pub async fn begin_meeting_reprocessing(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    jobs: State<'_, SharedMeetingJobs>,
    controller: State<'_, SharedController>,
    file_path: String,
    prompt: Option<String>,
    profile_id: Option<String>,
    request: ReprocessingRequest,
) -> Result<String, CommandError> {
    crate::meeting_review_commands::require_settings(&window)?;
    begin_job(
        app,
        jobs,
        controller,
        file_path,
        prompt,
        profile_id,
        Some(request),
    )
    .await
}

async fn begin_job(
    app: tauri::AppHandle,
    jobs: State<'_, SharedMeetingJobs>,
    controller: State<'_, SharedController>,
    file_path: String,
    prompt: Option<String>,
    profile_id: Option<String>,
    request: Option<ReprocessingRequest>,
) -> Result<String, CommandError> {
    #[cfg(not(feature = "diarization"))]
    {
        let _ = (
            app, jobs, controller, file_path, prompt, profile_id, request,
        );
        Err(CommandError::Unavailable(
            "This build has no speaker diarization support.".into(),
        ))
    }
    #[cfg(feature = "diarization")]
    {
        let mut slot = jobs.0.lock().map_err(|_| lock_error())?;
        if slot.as_ref().is_some_and(|job| job.snapshot.status.busy()) {
            return Err(CommandError::Busy(
                "A meeting import is still running or cancelling. Wait for it to finish.".into(),
            ));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut ctrl = lock_controller(&controller);
        let settings = ctrl.settings().clone();
        let context =
            file_transcription_context(&settings, profile_id.as_deref(), prompt.as_deref())?;
        if !ctrl.begin_meeting_job(&id) {
            return Err(CommandError::Busy(
                "Finish the current dictation before importing a meeting.".into(),
            ));
        }
        let backend = Arc::new(WhisperBackend::new());
        let cancelled = Arc::new(AtomicBool::new(false));
        let started_at = Instant::now();
        *slot = Some(MeetingJob {
            snapshot: MeetingSnapshot {
                id: id.clone(),
                status: JobStatus::Running,
                phase: "preparing".into(),
                elapsed_ms: 0,
                phase_elapsed_ms: 0,
                error: None,
                transcript: None,
                reprocessing: None,
            },
            backend: Some(backend.clone()),
            started_at,
            phase_started_at: started_at,
            cancelled: cancelled.clone(),
        });
        drop(ctrl);
        drop(slot);

        let state = jobs.inner().clone();
        let worker_id = id.clone();
        let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Transcribing);
        tauri::async_runtime::spawn(async move {
            let processing_state = state.clone();
            let processing_id = worker_id.clone();
            let processing_cancel = cancelled.clone();
            let processing_backend = backend.clone();
            let mut worker = tauri::async_runtime::spawn_blocking(move || {
                use sagascript_cli::transcribe::{
                    transcribe_meeting_file_with_control, MeetingControl,
                };
                let progress = |phase| {
                    if let Ok(mut slot) = processing_state.0.lock() {
                        if let Some(job) =
                            slot.as_mut().filter(|job| job.snapshot.id == processing_id)
                        {
                            if job.snapshot.status == JobStatus::Running {
                                job.set_phase(phase_name(phase), Instant::now());
                            }
                        }
                    }
                };
                let control = MeetingControl {
                    cancellation: &processing_cancel,
                    progress: &progress,
                };
                if let Some(request) = request {
                    use sagascript_cli::meeting_reprocessing::{
                        execute_reprocessing, ReprocessingInput,
                    };
                    let input = ReprocessingInput {
                        audio: std::path::Path::new(&file_path),
                        cache: request.cache_path.as_deref().map(std::path::Path::new),
                        previous: &request.previous,
                        language: context.language,
                        model: context.model,
                        glossary: &context.glossary,
                    };
                    execute_reprocessing(
                        &request.plan,
                        &request.expected_revision,
                        &input,
                        &settings,
                        &processing_backend,
                        request.cache_output.as_deref().map(std::path::Path::new),
                        Some(&control),
                    )
                    .map(|result| JobOutput::Reprocessing(Box::new(result)))
                    .map_err(|error| error.to_string())
                } else {
                    transcribe_meeting_file_with_control(
                        std::path::Path::new(&file_path),
                        &settings,
                        context.language,
                        context.model,
                        &context.glossary,
                        &processing_backend,
                        &control,
                    )
                    .map(|document| JobOutput::Import(Box::new(document)))
                    .map_err(|error| error.to_string())
                }
            });
            let started = Instant::now();
            let mut tick = tokio::time::interval(Duration::from_millis(100));
            let mut timed_out = false;
            let result = loop {
                tokio::select! {
                    result = &mut worker => break result.unwrap_or_else(|error| {
                        tracing::error!(%error, "Meeting worker failed to join");
                        Err("Meeting worker stopped unexpectedly. The previous transcript was not replaced.".into())
                    }),
                    _ = tick.tick() => {
                        if started.elapsed() >= Duration::from_secs(1800) {
                            timed_out = true;
                            cancelled.store(true, Ordering::SeqCst);
                            if let Ok(mut slot) = state.0.lock() {
                                if let Some(job) = slot.as_mut().filter(|job| job.snapshot.id == worker_id) {
                                    job.snapshot.status = JobStatus::Cancelling;
                                }
                            }
                        }
                        if cancelled.load(Ordering::SeqCst) {
                            // The backend clears stale aborts between native calls. Keep
                            // signalling THIS job's backend until the actual worker exits.
                            backend.request_abort();
                        }
                    }
                }
            };
            // Do not detach native work on timeout/cancel, or release its lease early.
            // This block runs only after spawn_blocking and its scoped children joined.
            let mut released = false;
            if let Ok(mut slot) = state.0.lock() {
                if let Some(job) = slot.as_mut().filter(|job| job.snapshot.id == worker_id) {
                    // Freeze the durations only once the actual worker has exited.
                    job.snapshot = job.snapshot_at(Instant::now());
                    let has_result = match &result {
                        Ok(JobOutput::Import(_)) => true,
                        #[cfg(feature = "diarization")]
                        Ok(JobOutput::Reprocessing(_)) => true,
                        _ => false,
                    };
                    released = release_meeting_lease(
                        &app.state::<SharedController>(),
                        &worker_id,
                        has_result,
                    );
                    if released {
                        finish_snapshot(
                            &mut job.snapshot,
                            result,
                            cancelled.load(Ordering::SeqCst),
                            timed_out,
                        );
                    } else {
                        finish_snapshot(
                            &mut job.snapshot,
                            Err(lock_error().to_string()),
                            false,
                            false,
                        );
                    }
                    job.backend = None;
                }
            }
            if released {
                let _ = app.emit_event(crate::events::event::STATE_CHANGED, UiState::Idle);
            } else {
                tracing::error!(
                    "Meeting worker exited but its controller lease could not be released"
                );
            }
        });
        Ok(id)
    }
}

/// Hand the controller back after the meeting worker exited: track a pending
/// result for the updater and end the controller's meeting lease. Returns
/// whether the lease for `worker_id` was released.
fn release_meeting_lease(controller: &SharedController, worker_id: &str, has_result: bool) -> bool {
    // Recover a poisoned controller (ADR 0005): skipping the release would
    // leave it Transcribing with this job's lease for the rest of the session.
    let mut ctrl = lock_controller(controller);
    if has_result {
        if let Err(error) = ctrl.set_update_result_pending(&format!("meeting:{worker_id}"), true) {
            tracing::warn!(%error, "Could not track pending meeting result");
        }
    }
    ctrl.finish_meeting_job(worker_id)
}

fn finish_snapshot(
    snapshot: &mut MeetingSnapshot,
    result: Result<JobOutput, String>,
    cancelled: bool,
    timed_out: bool,
) {
    snapshot.transcript = None;
    #[cfg(feature = "diarization")]
    {
        snapshot.reprocessing = None;
    }
    snapshot.error = None;
    if timed_out {
        snapshot.status = JobStatus::Failed;
        snapshot.error = Some(
            "Meeting import timed out and has stopped. Try a shorter file or a smaller model."
                .into(),
        );
    } else if cancelled {
        snapshot.status = JobStatus::Cancelled;
    } else {
        match result {
            Ok(JobOutput::Import(document)) => {
                snapshot.status = JobStatus::Completed;
                snapshot.transcript = Some(*document);
            }
            #[cfg(feature = "diarization")]
            Ok(JobOutput::Reprocessing(result)) => {
                snapshot.status = JobStatus::Completed;
                snapshot.reprocessing = Some(*result);
            }
            Err(error) => {
                snapshot.status = JobStatus::Failed;
                snapshot.error = Some(error);
            }
        }
    }
}

#[tauri::command]
pub fn get_meeting_job(
    jobs: State<'_, SharedMeetingJobs>,
    job_id: String,
) -> Result<MeetingSnapshot, CommandError> {
    jobs.0
        .lock()
        .map_err(|_| lock_error())?
        .as_ref()
        .filter(|job| job.snapshot.id == job_id)
        .map(|job| job.snapshot_at(Instant::now()))
        .ok_or_else(|| {
            CommandError::NotFound(
                "This meeting job is no longer available. No transcript was saved automatically."
                    .into(),
            )
        })
}

#[tauri::command]
pub fn cancel_meeting_job(
    jobs: State<'_, SharedMeetingJobs>,
    job_id: String,
) -> Result<bool, CommandError> {
    let mut slot = jobs.0.lock().map_err(|_| lock_error())?;
    cancel_job(&mut slot, &job_id)
}

fn cancel_job(slot: &mut Option<MeetingJob>, job_id: &str) -> Result<bool, CommandError> {
    let job = slot
        .as_mut()
        .filter(|job| job.snapshot.id == job_id)
        .ok_or_else(|| CommandError::NotFound("This meeting job is no longer available.".into()))?;
    if !job.snapshot.status.busy() {
        return Ok(false);
    }
    job.cancelled.store(true, Ordering::SeqCst);
    job.snapshot.status = JobStatus::Cancelling;
    if let Some(backend) = &job.backend {
        backend.request_abort();
    }
    Ok(true)
}

#[tauri::command]
pub fn rename_meeting_speaker(
    transcript: MeetingTranscript,
    speaker_id: String,
    label: String,
) -> Result<MeetingTranscript, CommandError> {
    Ok(transcript.rename_speaker(&speaker_id, label)?)
}

#[tauri::command]
pub fn merge_meeting_speakers(
    transcript: MeetingTranscript,
    from_id: String,
    into_id: String,
) -> Result<MeetingTranscript, CommandError> {
    Ok(transcript.merge_speakers(&from_id, &into_id)?)
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Plain,
    Markdown,
    Json,
    Srt,
    Vtt,
}

impl ExportFormat {
    pub(crate) fn details(self) -> (MeetingExportFormat, &'static str) {
        match self {
            Self::Plain => (MeetingExportFormat::Plain, "txt"),
            Self::Markdown => (MeetingExportFormat::Markdown, "md"),
            Self::Json => (MeetingExportFormat::Json, "json"),
            Self::Srt => (MeetingExportFormat::Srt, "srt"),
            Self::Vtt => (MeetingExportFormat::Vtt, "vtt"),
        }
    }
}

#[tauri::command]
pub async fn save_meeting_export(
    app: tauri::AppHandle,
    transcript: MeetingTranscript,
    format: ExportFormat,
) -> Result<bool, CommandError> {
    let (format, extension) = format.details();
    let output = transcript.export(format)?;
    tauri::async_runtime::spawn_blocking(move || {
        let selected = app
            .dialog()
            .file()
            .set_title("Export meeting — choose a new file")
            .set_file_name(format!("meeting.{extension}"))
            .add_filter("Meeting transcript", &[extension])
            .blocking_save_file();
        let Some(selected) = selected else {
            return Ok(false);
        };
        let path = selected.into_path().map_err(|error| {
            CommandError::InvalidInput(format!("Choose a local export file: {error}"))
        })?;
        write_new_export(&path, output.as_bytes())?;
        Ok(true)
    })
    .await
    .map_err(|error| CommandError::task_failed(format!("Export worker failed: {error}")))?
}

/// A user-selected NEW destination only; never truncate an existing recording
/// or document, even if a native dialog offered replacement. Published through
/// [`sagascript_core::fsutil::publish_new`]: complete or not at all, never
/// over an existing file.
pub(crate) fn write_new_export(path: &std::path::Path, bytes: &[u8]) -> Result<(), CommandError> {
    write_new_export_with(path, bytes, &sagascript_core::fsutil::RealFs)
}

fn write_new_export_with(
    path: &std::path::Path,
    bytes: &[u8],
    ops: &dyn sagascript_core::fsutil::PublishFs,
) -> Result<(), CommandError> {
    use sagascript_core::fsutil::{publish_new_with, Leftover, PublishErrorKind};
    // The file is named, not located: the webview never gets a folder (ADR
    // 0006), and the staging file lies in the folder the user chose for the
    // export, which the note says.
    let unsaved_note = |leftover: &Leftover| {
        let name = leftover
            .path
            .file_name()
            .map_or_else(|| "(unnamed)".into(), |name| name.to_string_lossy());
        format!(
            "The export was not saved, but its private temporary file {name} could not be removed from the folder you chose ({}). Remove that file manually.",
            leftover.error
        )
    };
    match publish_new_with(path, bytes, ops) {
        Ok(published) => {
            if let Some(leftover) = &published.leftover {
                // The export is complete; only a private second name of it
                // remains. The path is the user's, so the log carries the
                // error kind alone.
                tracing::warn!(
                    kind = ?leftover.error.kind(),
                    "an export's private staging file could not be removed"
                );
            }
            Ok(())
        }
        Err(error) => {
            if let Some(leftover) = &error.leftover {
                return Err(CommandError::Io(unsaved_note(leftover)));
            }
            Err(match &error.kind {
                PublishErrorKind::AlreadyExists => CommandError::ExportExists(
                    "That file already exists. Choose a new name; no existing file was changed."
                        .into(),
                ),
                PublishErrorKind::NoFileName => {
                    CommandError::InvalidInput("Choose a local export file.".into())
                }
                PublishErrorKind::Commit(source) => CommandError::Io(format!(
                    "Export was not published. Choose a writable local folder supporting atomic file creation: {source}"
                )),
                other => CommandError::Io(format!("Could not create export: {other}")),
            })
        }
    }
}

#[cfg(test)]
#[path = "meeting_jobs_tests.rs"]
mod tests;
