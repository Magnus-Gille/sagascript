//! Plain imports own one warm backend, separate from live dictation/training.
//! A lease stays alive until the actual worker exits; Stop never targets an
//! unrelated inference or a later import.
use crate::command_error::CommandError;
use crate::commands::FileTranscriptionContext;
use crate::events::{EmitEvent, PlainPhase, PlainTranscriptionProgress};
use sagascript_core::recording::{decode_tracks, Manifest, RecordingError, TrackChunk};
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
use std::path::PathBuf;
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
            return Err(CommandError::InvalidInput(
                "Invalid transcription run ID.".into(),
            ));
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
    input: PlainInput,
    model: sagascript_core::models::PianissimoModel,
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
            let audio = decode_input_for_run(run, &input, &progress)?;
            let timeout = Duration::from_secs(((audio.len() / 16_000) as u64 * 10).max(180));
            progress(PlainPhase::Loading, None);
            let backend = Arc::new(PianissimoBackend::start_with_cancel(
                &run.cancelled,
                &sagascript_core::machine::load(),
                model,
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
/// What a plain run transcribes.
#[derive(Debug, Clone)]
pub enum PlainInput {
    /// An audio file the user chose, decoded to 16 kHz mono.
    File(String),
    /// A finished recording folder (SAGA-564): the samples of its `mic` track,
    /// placed on the recording's timeline by `decode_tracks`.
    Recording(PathBuf),
}

/// Decodes a run's input; see [`PlainInput`].
fn decode_input_for_run(
    run: &RunControl,
    input: &PlainInput,
    progress: &dyn Fn(PlainPhase, Option<u8>),
) -> Result<Vec<f32>, DictationError> {
    match input {
        PlainInput::File(path) => decode_for_run(run, std::path::Path::new(path), progress),
        PlainInput::Recording(folder) => {
            run.check()?;
            progress(PlainPhase::Decoding, Some(0));
            let audio = decode_recording_samples(folder, &|| run.check(), &|pct| {
                progress(PlainPhase::Decoding, Some(pct))
            })?;
            run.check()?;
            Ok(audio)
        }
    }
}

/// The longest recording timeline decoded: a day of 16 kHz samples. A manifest
/// that places audio beyond it is corrupt, and must not allocate for it.
const MAX_RECORDING_SAMPLES: u64 = 24 * 3600 * 16_000;

/// The most samples reserved up front from the manifest's word (about 512 MB);
/// the buffer grows past it only with audio actually decoded.
const MAX_RESERVED_SAMPLES: u64 = 1 << 27;

/// 16 kHz mono samples of the `mic` track(s) of the finished recording in
/// `folder`. A recording that still says `recording` is refused with
/// `RecoveryNeeded` (core's `decode_tracks`). `check` is asked between chunks
/// (cancellation), `progress` gets whole percents.
pub(crate) fn decode_recording_samples(
    folder: &std::path::Path,
    check: &dyn Fn() -> Result<(), DictationError>,
    progress: &dyn Fn(u8),
) -> Result<Vec<f32>, DictationError> {
    let decoder = decode_tracks(folder)?;
    let expected = mic_samples_expected(decoder.manifest());
    assemble_mic_samples(decoder, expected, check, progress)
}

/// How many 16 kHz samples the manifest says the `mic` tracks hold.
fn mic_samples_expected(manifest: &Manifest) -> u64 {
    manifest
        .tracks
        .iter()
        .filter(|track| track.role == "mic")
        .map(|track| {
            let samples = u128::from(track.frames) * 16_000 / u128::from(track.sample_rate.max(1));
            u64::try_from(samples).unwrap_or(u64::MAX)
        })
        .fold(0_u64, u64::saturating_add)
}

/// Place the `mic` chunks on one timeline (`start_sample`), in bounded steps:
/// the buffer is the one allocation, there is no per-track Vec. Other roles
/// are not transcribed. `expected` only paces `progress` and the reservation.
fn assemble_mic_samples(
    chunks: impl Iterator<Item = Result<TrackChunk, RecordingError>>,
    expected: u64,
    check: &dyn Fn() -> Result<(), DictationError>,
    progress: &dyn Fn(u8),
) -> Result<Vec<f32>, DictationError> {
    let mut audio: Vec<f32> =
        Vec::with_capacity(usize::try_from(expected.min(MAX_RESERVED_SAMPLES)).unwrap_or(0));
    let mut decoded: u64 = 0;
    let mut announced: u8 = 0;
    for chunk in chunks {
        check()?;
        let chunk = chunk?;
        if chunk.role != "mic" {
            continue;
        }
        let end = chunk
            .start_sample
            .saturating_add(chunk.samples.len() as u64);
        if end > MAX_RECORDING_SAMPLES {
            return Err(RecordingError::CorruptTrack {
                reason: "a track places audio beyond any plausible length",
            }
            .into());
        }
        let (start, end) = (chunk.start_sample as usize, end as usize);
        if audio.len() < end {
            audio.resize(end, 0.0);
        }
        audio[start..end].copy_from_slice(&chunk.samples);
        decoded += chunk.samples.len() as u64;
        if let Some(pct) = (decoded * 100).checked_div(expected) {
            let pct = pct.min(100) as u8;
            if pct != announced {
                announced = pct;
                progress(pct);
            }
        }
    }
    if audio.is_empty() {
        return Err(DictationError::FileDecodeError(
            "The recording holds no microphone audio".into(),
        ));
    }
    // The resampler's count can be a sample short of the manifest's.
    if announced < 100 {
        progress(100);
    }
    Ok(audio)
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
    input: PlainInput,
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
            let audio = decode_input_for_run(run, &input, &progress)?;
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
    /// A run id the frontend never sends is refused as invalid input, and a
    /// second run while one is active as busy (retryable).
    #[test]
    fn begin_refuses_a_bad_id_and_a_second_run_with_codes() {
        let jobs = Arc::new(PlainFileJobs::default());
        for id in [String::new(), "x".repeat(129)] {
            let error = jobs.begin(id).err().expect("the id is refused");
            assert_eq!(error.code(), "invalid_input");
            assert_eq!(error.to_string(), "Invalid transcription run ID.");
        }
        let _first = jobs.begin("first".into()).expect("the first run starts");
        let second = jobs
            .begin("second".into())
            .err()
            .expect("one run at a time");
        assert_eq!(second.code(), "busy");
        assert!(second.retryable());
    }

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

    fn chunk(role: &str, start: u64, samples: Vec<f32>) -> Result<TrackChunk, RecordingError> {
        Ok(TrackChunk {
            track_id: role.into(),
            role: role.into(),
            start_sample: start,
            samples,
        })
    }

    fn never_cancelled() -> Result<(), DictationError> {
        Ok(())
    }

    /// Chunks are placed by `start_sample`, a gap between them is silence, and
    /// other roles are not transcribed.
    #[test]
    fn mic_chunks_are_placed_on_the_timeline_and_other_roles_are_skipped() {
        let chunks = vec![
            chunk("mic", 0, vec![1.0; 4]),
            chunk("system", 0, vec![9.0; 10]),
            chunk("mic", 6, vec![2.0; 2]),
        ];
        let audio = assemble_mic_samples(chunks.into_iter(), 8, &never_cancelled, &|_| {})
            .expect("assembled");
        assert_eq!(audio, [1.0, 1.0, 1.0, 1.0, 0.0, 0.0, 2.0, 2.0]);
    }

    #[test]
    fn progress_counts_whole_percents_once_each() {
        let chunks = (0..4).map(|i| chunk("mic", i * 25, vec![0.5; 25]));
        let seen = Mutex::new(Vec::new());
        assemble_mic_samples(chunks, 100, &never_cancelled, &|pct| {
            seen.lock().unwrap().push(pct);
        })
        .expect("assembled");
        assert_eq!(*seen.lock().unwrap(), [25, 50, 75, 100]);
    }

    #[test]
    fn a_cancellation_stops_between_chunks() {
        let chunks = vec![chunk("mic", 0, vec![1.0; 4]), chunk("mic", 4, vec![1.0; 4])];
        let calls = std::cell::Cell::new(0);
        let check = || {
            calls.set(calls.get() + 1);
            if calls.get() > 1 {
                Err(DictationError::Cancelled)
            } else {
                Ok(())
            }
        };
        let error = assemble_mic_samples(chunks.into_iter(), 8, &check, &|_| {}).unwrap_err();
        assert!(matches!(error, DictationError::Cancelled), "{error:?}");
    }

    #[test]
    fn a_decode_error_keeps_its_recording_code() {
        let chunks = vec![
            chunk("mic", 0, vec![1.0; 4]),
            Err(RecordingError::CorruptTrack { reason: "short" }),
        ];
        let error =
            assemble_mic_samples(chunks.into_iter(), 8, &never_cancelled, &|_| {}).unwrap_err();
        assert_eq!(CommandError::from(error).code(), "recording_corrupt_track");
    }

    #[test]
    fn a_track_that_claims_audio_beyond_a_day_is_corrupt_and_allocates_nothing() {
        let chunks = vec![chunk("mic", MAX_RECORDING_SAMPLES, vec![1.0; 4])];
        let error = assemble_mic_samples(chunks.into_iter(), u64::MAX, &never_cancelled, &|_| {})
            .unwrap_err();
        assert_eq!(CommandError::from(error).code(), "recording_corrupt_track");
    }

    #[test]
    fn a_recording_without_microphone_audio_is_a_decode_error() {
        let chunks = vec![chunk("system", 0, vec![1.0; 4])];
        let error =
            assemble_mic_samples(chunks.into_iter(), 4, &never_cancelled, &|_| {}).unwrap_err();
        assert_eq!(CommandError::from(error).code(), "file_decode_error");
    }
}
