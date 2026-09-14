//! Plain imports own one warm backend, separate from live dictation/training.
//! A lease stays alive until the actual worker exits; Stop never targets an
//! unrelated inference or a later import.
use std::sync::{atomic::{AtomicBool, Ordering}, Arc, Mutex};
use std::time::{Duration, Instant};
use std::collections::VecDeque;
use sagascript_core::{audio::decoder, error::DictationError, transcription::{ContextProfile, WhisperBackend, recommended_parallel_chunks}};
use tauri::Emitter;
use crate::commands::FileTranscriptionContext;

pub type SharedPlainFileJobs = Arc<PlainFileJobs>;

pub struct PlainFileJobs {
    active: Mutex<Option<Arc<RunControl>>>,
    pending_cancels: Mutex<VecDeque<(String, Instant)>>,
    backend: Arc<WhisperBackend>,
}
impl Default for PlainFileJobs {
    fn default() -> Self {
        Self { active: Mutex::new(None), pending_cancels: Mutex::new(VecDeque::new()), backend: Arc::new(WhisperBackend::new()) }
    }
}
struct RunControl {
    id: String,
    cancelled: AtomicBool,
    deadline: Mutex<Option<Instant>>,
}
struct Lease { jobs: SharedPlainFileJobs, control: Arc<RunControl> }
impl Drop for Lease {
    fn drop(&mut self) {
        let mut active = self.jobs.active.lock().unwrap();
        if active.as_ref().is_some_and(|run| Arc::ptr_eq(run, &self.control)) { *active = None; }
    }
}
impl PlainFileJobs {
    fn begin(self: &Arc<Self>, id: String) -> Result<Arc<Lease>, String> {
        if id.is_empty() || id.len() > 128 { return Err("Invalid transcription run ID.".into()); }
        let mut active = self.active.lock().unwrap();
        if active.is_some() { return Err("A file transcription is still running or stopping.".into()); }
        let mut pending = self.pending_cancels.lock().unwrap();
        pending.retain(|(_, time)| time.elapsed() < Duration::from_secs(30));
        let cancelled = pending.iter().any(|(pending_id, _)| pending_id == &id);
        pending.retain(|(pending_id, _)| pending_id != &id);
        let control = Arc::new(RunControl { id, cancelled: AtomicBool::new(cancelled), deadline: Mutex::new(None) });
        *active = Some(control.clone());
        Ok(Arc::new(Lease { jobs: self.clone(), control }))
    }
    pub fn cancel(&self, id: &str) -> bool {
        let active = self.active.lock().unwrap();
        let Some(run) = active.as_ref().filter(|run| run.id == id) else {
            // IPC commands may arrive out of order. Remember a bounded, short-
            // lived stop intent so a following begin cannot erase it.
            if !id.is_empty() && id.len() <= 128 {
                let mut pending = self.pending_cancels.lock().unwrap();
                pending.retain(|(pending_id, time)| pending_id != id && time.elapsed() < Duration::from_secs(30));
                if pending.len() >= 32 { pending.pop_front(); }
                pending.push_back((id.to_string(), Instant::now()));
            }
            return false;
        };
        run.cancelled.store(true, Ordering::SeqCst);
        // This backend is exclusively leased to this run, never live dictation.
        self.backend.request_abort();
        true
    }
    fn finish(&self, lease: &Lease, result: Result<String, String>) -> Result<String, String> {
        let mut active = self.active.lock().unwrap();
        let result = if lease.control.cancelled.load(Ordering::SeqCst) { Err("Transcription cancelled.".into()) } else { result };
        if active.as_ref().is_some_and(|run| Arc::ptr_eq(run, &lease.control)) { *active = None; }
        result
    }
}
impl RunControl {
    fn check(&self) -> Result<(), DictationError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(DictationError::TranscriptionFailed("Transcription cancelled.".into()));
        }
        Ok(())
    }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress<'a> { run_id: &'a str, phase: &'a str, percent: Option<u8> }

pub async fn transcribe(
    app: tauri::AppHandle, jobs: SharedPlainFileJobs, run_id: String,
    file_path: String, context: FileTranscriptionContext,
) -> Result<String, String> {
    let lease = jobs.begin(run_id)?;
    let worker_lease = lease.clone();
    let mut worker = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let run = &worker_lease.control;
        let backend = &worker_lease.jobs.backend;
        let progress = |phase: &str, percent| {
            let _ = app.emit(crate::events::event::PLAIN_TRANSCRIPTION_PROGRESS, Progress { run_id: &run.id, phase, percent });
        };
        let work = || -> Result<String, DictationError> {
            run.check()?;
            progress("decoding", Some(0));
            let audio = decoder::decode_audio_file_with_stage_progress(
                std::path::Path::new(&file_path), Some(&|| run.check()),
                &|pct| progress("decoding", Some(pct)),
                &|pct| progress("resampling", Some(pct)),
            )?;
            run.check()?;
            let timeout = Duration::from_secs(((audio.len() / 16_000) as u64 * 6).max(60));
            *run.deadline.lock().unwrap() = Some(Instant::now() + timeout);
            let mut options = context.options;
            options.parallel_chunks = recommended_parallel_chunks(audio.len(), context.model, options.beam_size);
            progress(if backend.needs_reload(context.model) { "loading" } else { "preparing" }, None);
            let text = backend.with_model(context.model, ContextProfile::FlashAttention, |backend| {
                run.check()?; // cancellation during model loading must not start inference
                progress("preparing", None);
                let encode_start = || {
                    // Warm-state acquisition clears old abort flags. Reassert
                    // this run's sticky cancellation after acquiring the lock.
                    if run.cancelled.load(Ordering::SeqCst) { backend.request_abort(); }
                    progress("encoding", None);
                };
                let progress_app = app.clone();
                let id = run.id.clone();
                backend.transcribe_sync_with_options(&audio, context.language, &options, move |pct| {
                    let _ = progress_app.emit(crate::events::event::PLAIN_TRANSCRIPTION_PROGRESS, Progress {
                        run_id: &id, phase: "transcribing", percent: Some(pct.clamp(0, 100) as u8),
                    });
                }, Some(&encode_start))
            })?;
            run.check()?;
            progress("finalizing", None);
            Ok(crate::commands::apply_glossary(text, &context.glossary))
        };
        work().map_err(|error| error.to_string())
    });
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut timed_out = false;
    let result = loop {
        tokio::select! {
            result = &mut worker => break result.unwrap_or_else(|error| Err(format!("Transcription worker failed: {error}"))),
            _ = tick.tick() => {
                if lease.control.deadline.lock().unwrap().is_some_and(|end| Instant::now() >= end) {
                    timed_out = true;
                    lease.control.cancelled.store(true, Ordering::SeqCst);
                }
                if lease.control.cancelled.load(Ordering::SeqCst) { jobs.backend.request_abort(); }
            }
        }
    };
    // Never detach native work and claim idle. Finish only after actual join.
    let result = jobs.finish(&lease, result);
    if timed_out { Err("Transcription timed out and has stopped.".into()) } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_before_begin_is_sticky_and_pending_storage_is_bounded() {
        let jobs = Arc::new(PlainFileJobs::default());
        assert!(!jobs.cancel("early"));
        let run = jobs.begin("early".into()).unwrap();
        assert!(run.control.check().is_err());
        drop(run);
        for i in 0..100 { jobs.cancel(&format!("pending-{i}")); }
        assert_eq!(jobs.pending_cancels.lock().unwrap().len(), 32);
    }
    #[test]
    fn cancellation_targets_one_run_and_old_requests_cannot_stop_next_run() {
        let jobs = Arc::new(PlainFileJobs::default());
        assert!(!jobs.cancel("missing"));
        let first = jobs.begin("first".into()).unwrap();
        assert!(jobs.begin("second".into()).is_err());
        assert!(!jobs.cancel("wrong"));
        assert!(first.control.check().is_ok());
        assert!(jobs.cancel("first"));
        assert!(first.control.check().is_err());
        assert!(jobs.finish(&first, Ok("partial".into())).is_err());
        let second = jobs.begin("second".into()).unwrap();
        drop(first);
        assert!(!jobs.cancel("first"));
        assert!(second.control.check().is_ok());
        assert!(jobs.cancel("second"));
    }
    #[test]
    fn dropped_caller_keeps_slot_until_worker_releases_lease() {
        let jobs = Arc::new(PlainFileJobs::default());
        let caller = jobs.begin("first".into()).unwrap();
        let worker = caller.clone();
        drop(caller);
        assert!(jobs.begin("second".into()).is_err());
        drop(worker);
        assert!(jobs.begin("second".into()).is_ok());
    }
}
