//! Capture owner and bounded ASR worker. Native callbacks never run inference or disk I/O.
use super::{
    capture::{self, Block, Health},
    document::{Gap, LiveSegment, SegmentStatus, Source},
    echo::EchoCanceller3,
    queue::{QueueError, WindowQueue},
    recording::Recording,
    segmenter::{Config, Segmenter, WindowJob},
    session::Session,
    state::{Phase, State},
    timeline::NativeTimeline,
};
use crate::transcription::{
    engine_host::{BoostSpec, EngineHostClient, JobOptions, Priority},
    pianissimo_backend,
};
use crate::{glossary::Glossary, settings::Settings, sync::lock_or_recover};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver},
        Arc, Condvar, Mutex,
    },
    thread,
    time::Duration,
};
const HOLD_BACK_MS: u64 = 250;
const MAX_MS: u64 = 4 * 60 * 60 * 1000;

struct Pending {
    queue: WindowQueue,
    closed: bool,
}
struct Lane {
    pending: Mutex<Pending>,
    ready: Condvar,
}
impl Lane {
    fn new() -> Self {
        Self {
            pending: Mutex::new(Pending {
                queue: WindowQueue::new(16_000 * 60),
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }
    fn push(&self, job: WindowJob) -> Result<(), String> {
        let is_final = job.final_window;
        let result = lock_or_recover(&self.pending, "live.queue").queue.push(job);
        match result {
            // A preview can be skipped. Audio and final windows are retained.
            Err(QueueError::QueueFull) if !is_final=>Ok(()),
            Err(error)=>Err(format!("Transcription cannot keep up ({error}). Recording stopped; the result is incomplete.")),
            Ok(_)=>{self.ready.notify_one();Ok(())}
        }
    }
    fn close(&self) {
        lock_or_recover(&self.pending, "live.queue").closed = true;
        self.ready.notify_all();
    }
}
impl Drop for Lane {
    fn drop(&mut self) {
        self.close();
    }
}

pub(super) fn run(session: &Arc<Session>, settings: Settings) {
    let client = match pianissimo_backend::shared_client(&settings).and_then(|client| {
        client
            .warm_with_cancel(&session.cancel)
            .map_err(|e| crate::error::DictationError::TranscriptionFailed(e.to_string()))?;
        Ok(client)
    }) {
        Ok(client) => client,
        Err(error) => {
            let mut state = lock_or_recover(&session.state, "live.state");
            if session.cancel.is_cancelled() {
                let _ = state.cancel();
            } else {
                let _ = state.fail(clean_error(&error.to_string()));
            }
            return;
        }
    };
    if session.stop.load(Ordering::Acquire) || session.cancel.is_cancelled() {
        let _ = lock_or_recover(&session.state, "live.state").cancel();
        return;
    }
    let mut recording = match Recording::new() {
        Ok(value) => value,
        Err(error) => {
            let _ =
                lock_or_recover(&session.state, "live.state").fail(clean_error(&error.to_string()));
            return;
        }
    };
    let glossary = Glossary::parse(&settings.effective_glossary_source(None));
    let lane = Arc::new(Lane::new());
    let worker_session = session.clone();
    let worker_lane = lane.clone();
    let worker = thread::Builder::new()
        .name("live-asr".into())
        .spawn(move || {
            run_asr_worker(&worker_session.stop, || {
                recognize(&worker_session, &worker_lane, client, glossary)
            })
        });
    let worker = match worker {
        Ok(worker) => worker,
        Err(error) => {
            let _ =
                lock_or_recover(&session.state, "live.state").fail(clean_error(&error.to_string()));
            return;
        }
    };
    // Always close the ASR lane even if capture processing panics, so drain cannot deadlock.
    let audio = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        capture_audio(session, &lane, &mut recording)
    }))
    .unwrap_or_else(|_| Err("Audio processing stopped unexpectedly.".into()));
    lane.close();
    {
        let mut state = lock_or_recover(&session.state, "live.state");
        if state.phase() == Phase::Running {
            let _ = state.begin_stop();
        }
    }
    let asr = worker
        .join()
        .unwrap_or_else(|_| Err("Transcription worker stopped unexpectedly.".into()));
    let finalized = recording.finalize().map_err(|e| e.to_string());
    let has_audio = finalized.is_ok() && recording.duration_samples() > 0;
    if has_audio {
        *lock_or_recover(&session.recording, "live.recording") = Some(recording);
    }
    let mut state = lock_or_recover(&session.state, "live.state");
    for source in [Source::Microphone, Source::System] {
        let _ = state.set_source_connected(source, false);
        let _ = state.set_source_queued_ms(source, 0);
    }
    if session.cancel.is_cancelled() {
        let _ = state.cancel();
        return;
    }
    match audio.and(asr).and(finalized) {
        Ok(()) => {
            let _ = state.finish(has_audio);
        }
        Err(error) => {
            let _ = state.finish_failed(clean_error(&error), has_audio);
        }
    }
}

// Stop capture on every ASR failure, including a panic or a state update
// failure, instead of waiting for the pending PCM queue to overflow.
fn run_asr_worker(
    stop: &std::sync::atomic::AtomicBool,
    work: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
        .unwrap_or_else(|_| Err("Transcription worker stopped unexpectedly.".into()));
    if result.is_err() {
        stop.store(true, Ordering::Release);
    }
    result
}

fn recognize(
    session: &Session,
    lane: &Lane,
    client: EngineHostClient,
    glossary: Glossary,
) -> Result<(), String> {
    // Some(empty) freezes boost OFF instead of falling back to a mutable shared-client glossary.
    // Alias corrections below use a frozen snapshot of the meeting's settings.
    let boost = Arc::new(BoostSpec {
        terms: vec![],
        weight: 0.,
        truncated: 0,
        too_long: 0,
    });
    let mut committed_end = [0u64; 2];
    loop {
        let job = {
            let mut pending = lock_or_recover(&lane.pending, "live.queue");
            loop {
                if let Some(job) = pending.queue.pop() {
                    break Some(job);
                }
                if pending.closed || session.cancel.is_cancelled() {
                    break None;
                }
                pending = lane
                    .ready
                    .wait_timeout(pending, Duration::from_millis(100))
                    .unwrap_or_else(|poison| poison.into_inner())
                    .0;
            }
        };
        let Some(job) = job else {
            return Ok(());
        };
        let outcome = client.transcribe_samples(
            &job.pcm,
            JobOptions {
                priority: Priority::Interactive,
                boost: Some(boost.clone()),
            },
            &session.cancel,
            None,
        );
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                session.stop.store(true, Ordering::Release);
                let message = clean_error(&error.to_string());
                let _ = lock_or_recover(&session.state, "live.state")
                    .set_source_error(job.source, Some(message.clone()));
                return Err(message);
            }
        };
        let index = source_index(job.source);
        let overlap = (committed_end[index].saturating_sub(job.start_sample)) as f64 / 16_000.;
        let raw = if overlap > 0. && !result.words.is_empty() {
            result
                .words
                .iter()
                .filter(|word| word.end > overlap)
                .map(|word| word.word.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            result.text
        };
        let (text, _) = glossary.correct_text(&raw);
        let segment = LiveSegment {
            id: format!("{}-{}", index, job.id),
            source: job.source,
            start_ms: job.start_sample / 16,
            end_ms: job.end_sample / 16,
            text,
            status: if job.final_window {
                SegmentStatus::Final
            } else {
                SegmentStatus::Preview
            },
            revision: job.revision,
        };
        let mut state = lock_or_recover(&session.state, "live.state");
        // Capture advances wall time before queueing audio; drain never moves timestamps backwards.
        if let Err(error) = state.upsert(segment) {
            session.stop.store(true, Ordering::Release);
            return Err(error.to_string());
        }
        let text_delay = (capture::host_ns()
            .saturating_sub(session.epoch_ns.load(Ordering::Acquire))
            / 1_000_000)
            .saturating_sub(job.end_sample / 16)
            .min(MAX_MS);
        state
            .set_source_text_delay_ms(job.source, text_delay)
            .map_err(|e| e.to_string())?;
        state
            .set_source_last_text_ms(job.source, Some(job.end_sample / 16))
            .map_err(|e| e.to_string())?;
        if job.final_window {
            committed_end[index] = job.end_sample;
        }
    }
}
fn source_index(source: Source) -> usize {
    if source == Source::Microphone {
        0
    } else {
        1
    }
}
fn clean_error(error: &str) -> String {
    crate::path_scrub::scrub_paths(error)
        .chars()
        .take(240)
        .collect()
}

fn capture_audio(session: &Session, lane: &Lane, recording: &mut Recording) -> Result<(), String> {
    let (mic_tx, mic_rx) = mpsc::sync_channel(capture::QUEUE_BLOCKS);
    let (sys_tx, sys_rx) = mpsc::sync_channel(capture::QUEUE_BLOCKS);
    let health = [Arc::new(Health::default()), Arc::new(Health::default())];
    let activation_epoch = Arc::new(AtomicU64::new(0));
    let mut segmenters = [
        Segmenter::new(Config::default(), Source::Microphone).map_err(|e| e.to_string())?,
        Segmenter::new(Config::default(), Source::System).map_err(|e| e.to_string())?,
    ];
    let mut echo = EchoCanceller3::new()?;
    let native = capture::Capture::start(
        mic_tx,
        sys_tx,
        health[0].clone(),
        health[1].clone(),
        activation_epoch.clone(),
    )?;
    if session.stop.load(Ordering::Acquire) || session.cancel.is_cancelled() {
        return Ok(());
    }
    let epoch = capture::host_ns();
    let mut timelines = [NativeTimeline::new(epoch), NativeTimeline::new(epoch)];
    lock_or_recover(&session.state, "live.state")
        .started()
        .map_err(|e| e.to_string())?;
    if session.stop.load(Ordering::Acquire) || session.cancel.is_cancelled() {
        return Ok(());
    }
    session.epoch_ns.store(epoch, Ordering::Release);
    activation_epoch.store(epoch, Ordering::Release);
    let mut frame = 0u64;
    let mut elapsed = 0u64;
    let outcome = (|| -> Result<(), String> {
        while !session.stop.load(Ordering::Acquire) && !session.cancel.is_cancelled() {
            elapsed = (capture::host_ns().saturating_sub(epoch) / 1_000_000).min(MAX_MS);
            {
                let mut state = lock_or_recover(&session.state, "live.state");
                let delta = elapsed.saturating_sub(state.elapsed_ms());
                state.advance_by(delta).map_err(|e| e.to_string())?;
            }
            native.check_devices()?;
            ingest(
                session,
                &mic_rx,
                &mut timelines[0],
                &health[0],
                Source::Microphone,
                epoch,
            )?;
            ingest(
                session,
                &sys_rx,
                &mut timelines[1],
                &health[1],
                Source::System,
                epoch,
            )?;
            // CoreAudio may stop system callbacks during silence. Flush the
            // resampler tail before the holdback cursor passes these samples.
            let last_system = health[1].last_host_ns.load(Ordering::Acquire);
            flush_idle_system(
                &mut timelines[1],
                &session.state,
                last_system,
                capture::host_ns(),
            )?;
            let target = elapsed.saturating_sub(HOLD_BACK_MS) * 16;
            while frame + 160 <= target {
                process_frame(
                    session,
                    lane,
                    recording,
                    &mut timelines,
                    &mut segmenters,
                    &mut echo,
                    frame,
                )?;
                frame += 160;
            }
            if elapsed >= MAX_MS {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    })();
    // Native handles are dropped on their owning thread before the final queue read.
    drop(native);
    elapsed = (capture::host_ns().saturating_sub(epoch) / 1_000_000).min(MAX_MS);
    {
        let mut state = lock_or_recover(&session.state, "live.state");
        let delta = elapsed.saturating_sub(state.elapsed_ms());
        state.advance_by(delta).map_err(|e| e.to_string())?;
    }
    let tail = (|| -> Result<(), String> {
        ingest(
            session,
            &mic_rx,
            &mut timelines[0],
            &health[0],
            Source::Microphone,
            epoch,
        )?;
        ingest(
            session,
            &sys_rx,
            &mut timelines[1],
            &health[1],
            Source::System,
            epoch,
        )?;
        for timeline in &mut timelines {
            timeline.finish().map_err(|e| e.to_string())?;
        }
        while frame + 160 <= elapsed * 16 {
            process_frame(
                session,
                lane,
                recording,
                &mut timelines,
                &mut segmenters,
                &mut echo,
                frame,
            )?;
            frame += 160;
        }
        Ok(())
    })();
    // Even on a capture error, finalize already accepted utterances.
    let mut flush = Ok(());
    for segmenter in &mut segmenters {
        for job in segmenter.finish() {
            if let Err(error) = lane.push(job) {
                flush = Err(error);
            }
        }
    }
    outcome.and(tail).and(flush)
}

fn ingest(
    session: &Session,
    rx: &Receiver<Block>,
    timeline: &mut NativeTimeline,
    health: &Health,
    source: Source,
    epoch: u64,
) -> Result<(), String> {
    let dropped = health.dropped_samples.swap(0, Ordering::AcqRel);
    if dropped > 0 {
        let mut state = lock_or_recover(&session.state, "live.state");
        let _ = state.add_source_dropped_frames(source, dropped);
        let message =
            "Audio buffer overflow. Some audio was lost; the recording is incomplete.".to_string();
        let _ = state.set_source_error(source, Some(message.clone()));
        return Err(message);
    }
    if health.failed.load(Ordering::Acquire) {
        return Err("An audio source stopped or changed format.".into());
    }
    for block in rx.try_iter() {
        let update = timeline
            .push(
                block.host_ns,
                block.sample_rate,
                &block.samples[..block.len],
            )
            .map_err(|e| format!("{source:?}: {e}"))?;
        let rms = (block.samples[..block.len]
            .iter()
            .map(|x| f64::from(*x).powi(2))
            .sum::<f64>()
            / block.len.max(1) as f64)
            .sqrt() as f32;
        let mut state = lock_or_recover(&session.state, "live.state");
        let last = (block.host_ns.saturating_sub(epoch) / 1_000_000).min(state.elapsed_ms());
        state
            .set_source_connected(source, true)
            .map_err(|e| e.to_string())?;
        state
            .set_source_last_audio_ms(source, Some(last))
            .map_err(|e| e.to_string())?;
        state
            .set_source_signal_level(source, rms.min(1.))
            .map_err(|e| e.to_string())?;
        if let Some(gap) = update.discontinuity {
            // System taps may omit idle audio. Microphone discontinuities are never silence.
            if source == Source::Microphone {
                let gap_end = (gap.end_sample / 16).min(state.elapsed_ms());
                state
                    .append_gap(Gap {
                        source,
                        start_ms: gap.start_sample / 16,
                        end_ms: gap_end,
                        reason: "Microphone capture discontinuity".into(),
                    })
                    .map_err(|e| e.to_string())?;
                return Err("Microphone audio was interrupted. The session is incomplete.".into());
            }
        }
        if update.discontinuity.is_none() && update.offset_samples.unsigned_abs() > 480 {
            return Err("Audio clocks drifted more than 30 ms. The session is incomplete.".into());
        }
    }
    let last = health.last_host_ns.load(Ordering::Acquire);
    if source == Source::Microphone
        && capture::host_ns().saturating_sub(if last == 0 { epoch } else { last }) > 2_000_000_000
    {
        return Err("No microphone audio received for two seconds. Check microphone permission and connection.".into());
    }
    Ok(())
}

fn flush_idle_system(
    timeline: &mut NativeTimeline,
    state: &Mutex<State>,
    last: u64,
    now: u64,
) -> Result<(), String> {
    if last != 0 && now.saturating_sub(last) > 100_000_000 {
        timeline.flush_idle().map_err(|e| e.to_string())?;
        // An idle tap has no fresh signal measurement. Preserve its last audio
        // timestamp and connection state, but never display the previous RMS.
        lock_or_recover(state, "live.state")
            .set_source_signal_level(Source::System, 0.)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn process_frame(
    session: &Session,
    lane: &Lane,
    recording: &mut Recording,
    timelines: &mut [NativeTimeline; 2],
    segmenters: &mut [Segmenter; 2],
    echo: &mut EchoCanceller3,
    start: u64,
) -> Result<(), String> {
    let mic = timelines[0].drain_frame(start).map_err(|e| e.to_string())?;
    let system = timelines[1].drain_frame(start).map_err(|e| e.to_string())?;
    recording.append(&mic, &system).map_err(|e| e.to_string())?;
    let clean = echo.process(&mic, &system)?;
    for (index, pcm) in [&clean, &system].into_iter().enumerate() {
        let signal = crate::audio::has_audio_signal(pcm).map_err(str::to_string)?;
        for job in segmenters[index]
            .push(start, pcm, signal)
            .map_err(|e| e.to_string())?
        {
            lane.push(job)?;
        }
    }
    let pending = lock_or_recover(&lane.pending, "live.queue");
    let mut state = lock_or_recover(&session.state, "live.state");
    for source in [Source::Microphone, Source::System] {
        state
            .set_source_queued_ms(
                source,
                pending.queue.pending_samples_for(source) as u64 / 16,
            )
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn idle_system_clears_stale_signal_without_fabricating_audio() {
        let epoch = 1_000_000_000;
        let mut timeline = NativeTimeline::new(epoch);
        timeline.push(epoch, 48_000, &[0.5; 480]).unwrap();
        let mut initial = State::new("idle-system".into()).unwrap();
        initial.started().unwrap();
        initial.set_source_connected(Source::System, true).unwrap();
        initial
            .set_source_last_audio_ms(Source::System, Some(0))
            .unwrap();
        initial
            .set_source_signal_level(Source::System, 0.5)
            .unwrap();
        let state = Mutex::new(initial);
        flush_idle_system(&mut timeline, &state, epoch, epoch + 100_000_000).unwrap();
        assert_eq!(
            state.lock().unwrap().snapshot().sources[1].signal_level,
            0.5
        );
        flush_idle_system(&mut timeline, &state, epoch, epoch + 100_000_001).unwrap();
        let snapshot = state.lock().unwrap().snapshot();
        let source = snapshot
            .sources
            .iter()
            .find(|s| s.source == Source::System)
            .unwrap();
        assert_eq!(source.signal_level, 0.0);
        assert!(source.connected);
        assert_eq!(source.last_audio_ms, Some(0));
        assert!(snapshot.gaps.is_empty());
    }

    #[test]
    fn failed_or_panicked_recognition_stops_capture() {
        let stop = AtomicBool::new(false);
        assert!(run_asr_worker(&stop, || Err("model failed".into())).is_err());
        assert!(stop.load(Ordering::Acquire));
        stop.store(false, Ordering::Release);
        assert!(run_asr_worker(&stop, || panic!("synthetic failure")).is_err());
        assert!(stop.load(Ordering::Acquire));
        stop.store(false, Ordering::Release);
        assert!(run_asr_worker(&stop, || Ok(())).is_ok());
        assert!(!stop.load(Ordering::Acquire));
    }
}
