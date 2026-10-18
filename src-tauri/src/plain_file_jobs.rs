//! Plain imports own one warm backend, separate from live dictation/training.
//! A lease stays alive until the actual worker exits; Stop never targets an
//! unrelated inference or a later import.
use crate::command_error::CommandError;
use crate::commands::FileTranscriptionContext;
use crate::events::{EmitEvent, PlainPhase, PlainTranscriptionProgress};
use sagascript_core::sync::lock_or_recover;
use sagascript_core::transcription::whisper_backend::display_transcript;
use sagascript_core::transcription::{
    pianissimo_backend::PianissimoBackend, Glossary, TranscribeRequest,
};
use sagascript_core::{
    audio::decoder,
    error::DictationError,
    transcription::{recommended_parallel_chunks, ContextProfile, WhisperBackend},
};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

// Recovered as-is after a panic (ADR 0005): every critical section is a single
// `Option`/`VecDeque` update, so the data stays valid, and `Lease::drop` runs
// while a panicking worker unwinds, where a second panic would abort.
const ACTIVE_LOCK: &str = "plain_file_jobs.active";
const PENDING_CANCELS_LOCK: &str = "plain_file_jobs.pending_cancels";
const PIANISSIMO_BACKEND_LOCK: &str = "plain_file_jobs.pianissimo_backend";
const DEADLINE_LOCK: &str = "plain_file_jobs.deadline";

pub type SharedPlainFileJobs = Arc<PlainFileJobs>;

pub struct PlainFileJobs {
    active: Mutex<Option<Arc<RunControl>>>,
    pending_cancels: Mutex<VecDeque<(String, Instant)>>,
    backend: Arc<WhisperBackend>,
    pianissimo_backend: Mutex<Option<Arc<PianissimoBackend>>>,
}
impl Default for PlainFileJobs {
    fn default() -> Self {
        Self {
            active: Mutex::new(None),
            pending_cancels: Mutex::new(VecDeque::new()),
            backend: Arc::new(WhisperBackend::new()),
            pianissimo_backend: Mutex::new(None),
        }
    }
}
struct RunControl {
    id: String,
    cancelled: AtomicBool,
    deadline: Mutex<Option<Instant>>,
}
struct Lease {
    jobs: SharedPlainFileJobs,
    control: Arc<RunControl>,
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut active = lock_or_recover(&self.jobs.active, ACTIVE_LOCK);
        if active
            .as_ref()
            .is_some_and(|run| Arc::ptr_eq(run, &self.control))
        {
            *active = None;
        }
    }
}
impl PlainFileJobs {
    fn begin(self: &Arc<Self>, id: String) -> Result<Arc<Lease>, CommandError> {
        if id.is_empty() || id.len() > 128 {
            return Err("Invalid transcription run ID.".into());
        }
        let mut active = lock_or_recover(&self.active, ACTIVE_LOCK);
        if active.is_some() {
            return Err(CommandError::Busy(
                "A file transcription is still running or stopping.".into(),
            ));
        }
        let mut pending = lock_or_recover(&self.pending_cancels, PENDING_CANCELS_LOCK);
        pending.retain(|(_, time)| time.elapsed() < Duration::from_secs(30));
        let cancelled = pending.iter().any(|(pending_id, _)| pending_id == &id);
        pending.retain(|(pending_id, _)| pending_id != &id);
        let control = Arc::new(RunControl {
            id,
            cancelled: AtomicBool::new(cancelled),
            deadline: Mutex::new(None),
        });
        *active = Some(control.clone());
        Ok(Arc::new(Lease {
            jobs: self.clone(),
            control,
        }))
    }
    pub fn cancel(&self, id: &str) -> bool {
        let active = lock_or_recover(&self.active, ACTIVE_LOCK);
        let Some(run) = active.as_ref().filter(|run| run.id == id) else {
            // IPC commands may arrive out of order. Remember a bounded, short-
            // lived stop intent so a following begin cannot erase it.
            if !id.is_empty() && id.len() <= 128 {
                let mut pending = lock_or_recover(&self.pending_cancels, PENDING_CANCELS_LOCK);
                pending.retain(|(pending_id, time)| {
                    pending_id != id && time.elapsed() < Duration::from_secs(30)
                });
                if pending.len() >= 32 {
                    pending.pop_front();
                }
                pending.push_back((id.to_string(), Instant::now()));
            }
            return false;
        };
        run.cancelled.store(true, Ordering::SeqCst);
        // This backend is exclusively leased to this run, never live dictation.
        self.backend.request_abort();
        if let Some(backend) =
            lock_or_recover(&self.pianissimo_backend, PIANISSIMO_BACKEND_LOCK).as_ref()
        {
            backend.request_abort();
        }
        true
    }
    fn finish(
        &self,
        lease: &Lease,
        result: Result<String, CommandError>,
    ) -> Result<String, CommandError> {
        let mut active = lock_or_recover(&self.active, ACTIVE_LOCK);
        let result = if lease.control.cancelled.load(Ordering::SeqCst) {
            Err(CommandError::Cancelled)
        } else {
            result
        };
        if active
            .as_ref()
            .is_some_and(|run| Arc::ptr_eq(run, &lease.control))
        {
            *active = None;
        }
        result
    }
}

pub async fn transcribe_pianissimo(
    app: tauri::AppHandle,
    jobs: SharedPlainFileJobs,
    run_id: String,
    file_path: String,
    glossary: Glossary,
) -> Result<String, CommandError> {
    let lease = jobs.begin(run_id)?;
    let worker_lease = lease.clone();
    let mut worker = tokio::task::spawn_blocking(move || -> Result<String, CommandError> {
        let run = &worker_lease.control;
        let progress = |phase: PlainPhase, percent| {
            let _ = app.emit_event(
                crate::events::event::PLAIN_TRANSCRIPTION_PROGRESS,
                PlainTranscriptionProgress {
                    run_id: &run.id,
                    phase,
                    percent,
                },
            );
        };
        let work = || -> Result<String, DictationError> {
            let audio = decode_for_run(run, std::path::Path::new(&file_path), &progress)?;
            let timeout = Duration::from_secs(((audio.len() / 16_000) as u64 * 10).max(180));
            progress(PlainPhase::Loading, None);
            let backend = Arc::new(PianissimoBackend::start_with_cancel(
                &run.cancelled,
                &sagascript_core::machine::load(),
            )?);
            *lock_or_recover(
                &worker_lease.jobs.pianissimo_backend,
                PIANISSIMO_BACKEND_LOCK,
            ) = Some(backend.clone());
            run.check()?;
            // Load the model before the deadline starts: a first-time Core ML
            // compile can take minutes and is not inference time.
            backend.warm_up_with_cancel(&run.cancelled)?;
            run.check()?;
            *lock_or_recover(&run.deadline, DEADLINE_LOCK) = Some(Instant::now() + timeout);
            progress(PlainPhase::Transcribing, Some(0));
            let result = backend.transcribe_with_cancel(
                &audio,
                |pct| progress(PlainPhase::Transcribing, Some(pct)),
                &run.cancelled,
            )?;
            run.check()?;
            progress(PlainPhase::Finalizing, None);
            Ok(crate::commands::apply_glossary(result.text, &glossary))
        };
        let result = work().map_err(CommandError::from);
        *lock_or_recover(
            &worker_lease.jobs.pianissimo_backend,
            PIANISSIMO_BACKEND_LOCK,
        ) = None;
        result
    });
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut timed_out = false;
    let result = loop {
        tokio::select! {
            result = &mut worker => break result.unwrap_or_else(|error| {
                Err(DictationError::TaskFailed(format!("Transcription worker failed: {error}")).into())
            }),
            _ = tick.tick() => {
                if lock_or_recover(&lease.control.deadline, DEADLINE_LOCK).is_some_and(|end| Instant::now() >= end) {
                    timed_out = true;
                    lease.control.cancelled.store(true, Ordering::SeqCst);
                }
                if lease.control.cancelled.load(Ordering::SeqCst) {
                    if let Some(backend) = lock_or_recover(&jobs.pianissimo_backend, PIANISSIMO_BACKEND_LOCK).as_ref() {
                        backend.request_abort();
                    }
                }
            }
        }
    };
    let result = jobs.finish(&lease, result);
    if timed_out {
        Err(DictationError::TranscriptionTimedOut(
            "Transcription timed out and has stopped.".into(),
        )
        .into())
    } else {
        result
    }
}
/// Decodes a run's input, announcing the decoding and resampling phases and
/// stopping at a cancellation. A file that is gone is `FileNotFound`, which the
/// command reports as `file_not_found`.
fn decode_for_run(
    run: &RunControl,
    path: &std::path::Path,
    progress: &dyn Fn(PlainPhase, Option<u8>),
) -> Result<Vec<f32>, DictationError> {
    run.check()?;
    progress(PlainPhase::Decoding, Some(0));
    let audio = decoder::decode_audio_file_with_stage_progress(
        path,
        Some(&|| run.check()),
        &|pct| progress(PlainPhase::Decoding, Some(pct)),
        &|pct| progress(PlainPhase::Resampling, Some(pct)),
    )?;
    run.check()?;
    Ok(audio)
}

impl RunControl {
    fn check(&self) -> Result<(), DictationError> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(DictationError::Cancelled);
        }
        Ok(())
    }
}

pub async fn transcribe(
    app: tauri::AppHandle,
    jobs: SharedPlainFileJobs,
    run_id: String,
    file_path: String,
    context: FileTranscriptionContext,
) -> Result<String, CommandError> {
    let lease = jobs.begin(run_id)?;
    let worker_lease = lease.clone();
    let mut worker = tokio::task::spawn_blocking(move || -> Result<String, CommandError> {
        let run = &worker_lease.control;
        let backend = &worker_lease.jobs.backend;
        let progress = |phase: PlainPhase, percent| {
            let _ = app.emit_event(
                crate::events::event::PLAIN_TRANSCRIPTION_PROGRESS,
                PlainTranscriptionProgress {
                    run_id: &run.id,
                    phase,
                    percent,
                },
            );
        };
        let work = || -> Result<String, DictationError> {
            let audio = decode_for_run(run, std::path::Path::new(&file_path), &progress)?;
            let timeout = Duration::from_secs(((audio.len() / 16_000) as u64 * 6).max(60));
            *lock_or_recover(&run.deadline, DEADLINE_LOCK) = Some(Instant::now() + timeout);
            let mut options = context.options;
            options.parallel_chunks =
                recommended_parallel_chunks(audio.len(), context.model, options.beam_size);
            progress(
                if backend.needs_reload(context.model) {
                    PlainPhase::Loading
                } else {
                    PlainPhase::Preparing
                },
                None,
            );
            let text =
                backend.with_model(context.model, ContextProfile::FlashAttention, |backend| {
                    run.check()?; // cancellation during model loading must not start inference
                    progress(PlainPhase::Preparing, None);
                    let encode_start = || {
                        // Warm-state acquisition clears old abort flags. Reassert
                        // this run's sticky cancellation after acquiring the lock.
                        if run.cancelled.load(Ordering::SeqCst) {
                            backend.request_abort();
                        }
                        progress(PlainPhase::Encoding, None);
                    };
                    let progress_app = app.clone();
                    let id = run.id.clone();
                    let segments = backend.transcribe_with_gap_recovery(
                        TranscribeRequest::new(&audio, context.language, &options)
                            .on_progress(move |pct| {
                                let _ = progress_app.emit_event(
                                    crate::events::event::PLAIN_TRANSCRIPTION_PROGRESS,
                                    PlainTranscriptionProgress {
                                        run_id: &id,
                                        phase: PlainPhase::Transcribing,
                                        percent: Some(pct.clamp(0, 100) as u8),
                                    },
                                );
                            })
                            .on_encode_start(&encode_start),
                    )?;
                    Ok(display_transcript(&segments, context.language))
                })?;
            run.check()?;
            progress(PlainPhase::Finalizing, None);
            Ok(crate::commands::apply_glossary(text, &context.glossary))
        };
        work().map_err(CommandError::from)
    });
    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut timed_out = false;
    let result = loop {
        tokio::select! {
            result = &mut worker => break result.unwrap_or_else(|error| {
                Err(DictationError::TaskFailed(format!("Transcription worker failed: {error}")).into())
            }),
            _ = tick.tick() => {
                if lock_or_recover(&lease.control.deadline, DEADLINE_LOCK).is_some_and(|end| Instant::now() >= end) {
                    timed_out = true;
                    lease.control.cancelled.store(true, Ordering::SeqCst);
                }
                if lease.control.cancelled.load(Ordering::SeqCst) { jobs.backend.request_abort(); }
            }
        }
    };
    // Never detach native work and claim idle. Finish only after actual join.
    let result = jobs.finish(&lease, result);
    if timed_out {
        Err(DictationError::TranscriptionTimedOut(
            "Transcription timed out and has stopped.".into(),
        )
        .into())
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancel_before_begin_is_sticky_and_pending_storage_is_bounded() {
        let jobs = Arc::new(PlainFileJobs::default());
        assert!(!jobs.cancel("early"));
        let run = jobs.begin("early".into()).unwrap();
        assert!(matches!(
            run.control.check(),
            Err(DictationError::Cancelled)
        ));
        drop(run);
        for i in 0..100 {
            jobs.cancel(&format!("pending-{i}"));
        }
        assert_eq!(jobs.pending_cancels.lock().unwrap().len(), 32);
    }
    /// A plain file job whose file is gone reports `file_not_found`, which the
    /// window uses to forget the remembered file (ADR 0006), not a generic
    /// failure; and a stopped run still reports `cancelled` before it reads the file.
    #[test]
    fn a_missing_file_fails_the_job_with_file_not_found() {
        let jobs = Arc::new(PlainFileJobs::default());
        let lease = jobs.begin("run-1".into()).unwrap();
        let phases = Mutex::new(Vec::new());
        let progress = |phase: PlainPhase, percent: Option<u8>| {
            phases.lock().unwrap().push((phase, percent));
        };
        let missing = std::path::Path::new("/nonexistent/sagascript/missing-audio.wav");

        let failure = decode_for_run(&lease.control, missing, &progress)
            .map(|_| String::new())
            .map_err(CommandError::from);
        let failure = jobs.finish(&lease, failure).unwrap_err();

        assert_eq!(failure.code(), "file_not_found");
        assert!(!failure.retryable());
        assert!(
            !failure.message().contains("nonexistent"),
            "the message names no path: {}",
            failure.message()
        );
        assert_eq!(
            phases.lock().unwrap().first(),
            Some(&(PlainPhase::Decoding, Some(0))),
            "decoding was announced before the file was opened"
        );

        let lease = jobs.begin("run-2".into()).unwrap();
        assert!(jobs.cancel("run-2"));
        let stopped = decode_for_run(&lease.control, missing, &progress).unwrap_err();
        assert!(matches!(stopped, DictationError::Cancelled), "{stopped:?}");
    }

    #[test]
    fn cancellation_targets_one_run_and_old_requests_cannot_stop_next_run() {
        let jobs = Arc::new(PlainFileJobs::default());
        assert!(!jobs.cancel("missing"));
        let first = jobs.begin("first".into()).unwrap();
        let refused = jobs
            .begin("second".into())
            .err()
            .expect("one run at a time");
        assert_eq!(refused.code(), "busy");
        assert!(!jobs.cancel("wrong"));
        assert!(first.control.check().is_ok());
        assert!(jobs.cancel("first"));
        assert!(first.control.check().is_err());
        let finished = jobs.finish(&first, Ok("partial".into()));
        assert!(
            matches!(finished, Err(CommandError::Cancelled)),
            "a cancelled run never returns its partial text: {finished:?}"
        );
        let second = jobs.begin("second".into()).unwrap();
        drop(first);
        assert!(!jobs.cancel("first"));
        assert!(second.control.check().is_ok());
        assert!(jobs.cancel("second"));
    }
    /// A worker panic drops its lease while unwinding; the slot must still be
    /// released (no double panic) so the next import can start.
    #[test]
    fn lease_drop_releases_the_slot_even_when_the_lock_is_poisoned() {
        let jobs = Arc::new(PlainFileJobs::default());
        let lease = jobs.begin("first".into()).unwrap();
        std::thread::scope(|scope| {
            let panicked = scope
                .spawn(|| {
                    let _active = jobs.active.lock().unwrap();
                    panic!("injected panic while holding the lock (#359)");
                })
                .join();
            assert!(panicked.is_err());
        });
        assert!(jobs.active.is_poisoned());

        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(lease)));
        assert!(dropped.is_ok(), "Lease::drop must not panic");
        assert!(jobs.begin("second".into()).is_ok());
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
