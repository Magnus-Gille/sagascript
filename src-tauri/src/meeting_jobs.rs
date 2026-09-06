//! One explicit, in-memory meeting import. No library, capture, or auto-paste.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sagascript_core::meeting::{MeetingExportFormat, MeetingTranscript};
use sagascript_core::transcription::WhisperBackend;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::{file_transcription_context, SharedController};

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
    error: Option<String>,
    transcript: Option<MeetingTranscript>,
}

struct MeetingJob {
    snapshot: MeetingSnapshot,
    cancelled: Arc<AtomicBool>,
    backend: Option<Arc<WhisperBackend>>,
}

#[derive(Default)]
pub struct MeetingJobs(Mutex<Option<MeetingJob>>);
pub type SharedMeetingJobs = Arc<MeetingJobs>;

fn lock_error() -> String {
    "Meeting job state is unavailable; restart the app before importing again.".into()
}

#[tauri::command]
pub async fn begin_meeting_file(
    app: tauri::AppHandle,
    jobs: State<'_, SharedMeetingJobs>,
    controller: State<'_, SharedController>,
    file_path: String,
    prompt: Option<String>,
    profile_id: Option<String>,
) -> Result<String, String> {
    #[cfg(not(feature = "diarization"))]
    {
        let _ = (app, jobs, controller, file_path, prompt, profile_id);
        Err("This build has no speaker diarization support.".into())
    }
    #[cfg(feature = "diarization")]
    {
        let mut slot = jobs.0.lock().map_err(|_| lock_error())?;
        if slot.as_ref().is_some_and(|job| job.snapshot.status.busy()) {
            return Err(
                "A meeting import is still running or cancelling. Wait for it to finish.".into(),
            );
        }
        let id = uuid::Uuid::new_v4().to_string();
        let mut ctrl = controller.lock().map_err(|_| lock_error())?;
        let settings = ctrl.settings().clone();
        let context =
            file_transcription_context(&settings, profile_id.as_deref(), prompt.as_deref())?;
        if !ctrl.begin_meeting_job(&id) {
            return Err("Finish the current dictation before importing a meeting.".into());
        }
        let backend = Arc::new(WhisperBackend::new());
        let cancelled = Arc::new(AtomicBool::new(false));
        *slot = Some(MeetingJob {
            snapshot: MeetingSnapshot {
                id: id.clone(),
                status: JobStatus::Running,
                phase: "preparing".into(),
                error: None,
                transcript: None,
            },
            backend: Some(backend.clone()),
            cancelled: cancelled.clone(),
        });
        drop(ctrl);
        drop(slot);

        let state = jobs.inner().clone();
        let worker_id = id.clone();
        let _ = app.emit(crate::events::event::STATE_CHANGED, "transcribing");
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
                                // Serialize the closed enum, never text or a source path.
                                job.snapshot.phase = serde_json::to_value(phase)
                                    .ok()
                                    .and_then(|value| value.as_str().map(str::to_owned))
                                    .unwrap_or_else(|| "processing".into());
                            }
                        }
                    }
                };
                let control = MeetingControl {
                    cancellation: &processing_cancel,
                    progress: &progress,
                };
                transcribe_meeting_file_with_control(
                    std::path::Path::new(&file_path),
                    &settings,
                    context.language,
                    context.model,
                    &context.glossary,
                    &processing_backend,
                    &control,
                )
                .map_err(|error| error.to_string())
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
                    let controller = app.state::<SharedController>();
                    if let Ok(mut ctrl) = controller.lock() {
                        released = ctrl.finish_meeting_job(&worker_id);
                    }
                    if released {
                        finish_snapshot(
                            &mut job.snapshot,
                            result,
                            cancelled.load(Ordering::SeqCst),
                            timed_out,
                        );
                    } else {
                        finish_snapshot(&mut job.snapshot, Err(lock_error()), false, false);
                    }
                    job.backend = None;
                }
            }
            if released {
                let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
            } else {
                tracing::error!(
                    "Meeting worker exited but its controller lease could not be released"
                );
            }
        });
        Ok(id)
    }
}

fn finish_snapshot(
    snapshot: &mut MeetingSnapshot,
    result: Result<MeetingTranscript, String>,
    cancelled: bool,
    timed_out: bool,
) {
    snapshot.transcript = None;
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
            Ok(document) => {
                snapshot.status = JobStatus::Completed;
                snapshot.transcript = Some(document);
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
) -> Result<MeetingSnapshot, String> {
    jobs.0
        .lock()
        .map_err(|_| lock_error())?
        .as_ref()
        .filter(|job| job.snapshot.id == job_id)
        .map(|job| job.snapshot.clone())
        .ok_or_else(|| {
            "This meeting job is no longer available. No transcript was saved automatically.".into()
        })
}

#[tauri::command]
pub fn cancel_meeting_job(
    jobs: State<'_, SharedMeetingJobs>,
    job_id: String,
) -> Result<bool, String> {
    let mut slot = jobs.0.lock().map_err(|_| lock_error())?;
    cancel_job(&mut slot, &job_id)
}

fn cancel_job(slot: &mut Option<MeetingJob>, job_id: &str) -> Result<bool, String> {
    let job = slot
        .as_mut()
        .filter(|job| job.snapshot.id == job_id)
        .ok_or_else(|| "This meeting job is no longer available.".to_string())?;
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
) -> Result<MeetingTranscript, String> {
    transcript
        .rename_speaker(&speaker_id, label)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn merge_meeting_speakers(
    transcript: MeetingTranscript,
    from_id: String,
    into_id: String,
) -> Result<MeetingTranscript, String> {
    transcript
        .merge_speakers(&from_id, &into_id)
        .map_err(|error| error.to_string())
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
    fn details(self) -> (MeetingExportFormat, &'static str) {
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
) -> Result<bool, String> {
    let (format, extension) = format.details();
    let output = transcript
        .export(format)
        .map_err(|error| error.to_string())?;
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
        let path = selected
            .into_path()
            .map_err(|error| format!("Choose a local export file: {error}"))?;
        write_new_export(&path, output.as_bytes())?;
        Ok(true)
    })
    .await
    .map_err(|error| format!("Export worker failed: {error}"))?
}

/// A user-selected NEW destination only; never truncate an existing recording
/// or document, even if a native dialog offered replacement. Publish atomically.
fn write_new_export(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let parent = path.parent().ok_or("Choose a local export folder.")?;
    let temporary = parent.join(format!(".sagascript-export-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("Could not create export: {error}"))?;
    let result = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::hard_link(&temporary, path));
    drop(file);
    if let Err(error) = std::fs::remove_file(&temporary) {
        let publication = if result.is_ok() {
            "The export was saved"
        } else {
            "The export was not saved"
        };
        return Err(format!("{publication}, but its private temporary file could not be removed: {} ({error}). Remove that temporary file manually.", temporary.display()));
    }
    result.map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            "That file already exists. Choose a new name; no existing file was changed.".into()
        } else {
            format!("Export was not published. Choose a writable local folder supporting atomic file creation: {error}")
        }
    })
}

#[cfg(test)]
#[path = "meeting_jobs_tests.rs"]
mod tests;
