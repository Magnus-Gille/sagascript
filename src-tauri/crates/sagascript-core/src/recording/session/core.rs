//! The writer side of a session, as a plain object driven by `step`.
//!
//! [`SessionCore`] owns the [`RecordingWriter`] of 1a, one [`Pipeline`] per
//! track and the policy (gaps, anchors, endings, disk). It holds no lock and
//! starts no thread: the session's writer thread calls [`SessionCore::step`]
//! in a loop, and the unit tests call it directly, which makes every test
//! deterministic. All of its time comes from the injected [`SessionClock`] and
//! from the host stamps of the blocks, never from the wall clock.
//!
//! # What a block does (`process_block`)
//!
//! 1. **Continuity.** A block that says frames were dropped before it
//!    (`dropped_before`, counted by the callback) is an **overrun** gap of
//!    exactly that length. Otherwise the block's host stamp is compared with
//!    where the previous block ended: a jump of at least `min_gap` is a
//!    **stall** gap (or `device_loss` if the source reported a problem) whose
//!    length comes from the stamps, never from the number of blocks. A jump of
//!    more than `max_gap_fill` is not filled with silence: it ends the track
//!    (`system_sleep` if a sleep was seen, else `clock_discontinuity`).
//! 2. **Silence** for the gap is inserted into the stored track, so the frame
//!    index still maps to time, and the gap is journalled (1a).
//! 3. **Anchor.** After a gap, and when `anchor_interval` of host time has
//!    passed, `(host_ns, stored_frame)` is journalled.
//! 4. **Audio** goes to the encoder (lossless) or through the streaming
//!    resampler (compact).
//!
//! # Endings
//!
//! A signal from the source (device change, format change, sleep) marks the
//! track for ending; the queue is drained first, so every block delivered
//! before the event is stored, and then the track is finished with its
//! `ended_reason`. A write error that is a full disk cuts the track back to
//! its last complete frame (`RecordingWriter::abort_track`).

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::recording::capture::{Block, NativeFormat, SessionClock, SleepDetector, TrackReceiver};
use crate::recording::manifest::{EndedReason, GapReason};
use crate::recording::store::RecordingWriter;
use crate::recording::RecordingError;
use crate::sync::lock_or_recover;

use super::disk::{DiskLevel, DiskPolicy, FreeSpace};
use super::resampler::{StreamResampler, COMPACT_RATE};
use super::status::{SessionOutcome, StatusShared};

const ENDED_LOCK: &str = "recording.session.track_ended";
const EARLY_LOCK: &str = "recording.session.terminal";

/// Blocks one track may process in one step, so that a backlog on one track
/// cannot starve the periodic work.
const BLOCKS_PER_STEP: usize = 256;
/// The peak shown in the status covers at least this much time.
const PEAK_WINDOW_NS: u64 = 100_000_000;
/// How often the real clocks are looked at for a sleep.
const SLEEP_POLL_NS: u64 = 250_000_000;

/// The numbers that drive the policy.
#[derive(Clone, Debug)]
pub(crate) struct CoreSettings {
    pub stall_timeout_ns: u64,
    pub min_gap_ns: u64,
    pub max_gap_fill_ns: u64,
    pub anchor_interval_ns: u64,
    pub device_loss_grace_ns: u64,
    pub disk: DiskPolicy,
    /// End a track at this many bytes (FAT32).
    pub file_limit_bytes: Option<u64>,
}

/// One track's source of blocks and what the writer knows about it.
pub(crate) struct PipelineSpec {
    pub id: String,
    pub receiver: TrackReceiver,
    pub native: NativeFormat,
    pub compact: bool,
    pub first_block: Block,
}

struct Pipeline {
    id: String,
    receiver: TrackReceiver,
    native: NativeFormat,
    stored_rate: u32,
    resampler: Option<StreamResampler>,
    scratch: Vec<f32>,
    /// Where the previous block ended on the host clock.
    expected_next_ns: Option<u64>,
    last_arrival_ns: u64,
    last_anchor_host: u64,
    stalled: bool,
    device_lost: bool,
    device_lost_since_ns: u64,
    /// Native frames consumed since the current segment began.
    segment_input: u64,
    /// Stored frame at which the current segment began.
    segment_start: u64,
    /// Set by a signal; applied once the queue is empty.
    pending_end: Option<EndedReason>,
    sleep_seen: bool,
    ended: bool,
    frames_received: u64,
    gap_count: u32,
    gap_frames: u64,
    window_peaks: [f32; 2],
    window_start_ns: u64,
}

impl Pipeline {
    /// The stored frame that native frame `segment_input` of the segment maps to.
    fn stored_position(&self) -> u64 {
        let rate = u64::from(self.native.sample_rate);
        self.segment_start + (self.segment_input * u64::from(self.stored_rate) + rate / 2) / rate
    }

    fn to_stored(&self, native_frames: u64) -> u64 {
        let rate = u64::from(self.native.sample_rate);
        (native_frames * u64::from(self.stored_rate) + rate / 2) / rate
    }
}

pub(crate) struct SessionCore {
    writer: Option<RecordingWriter>,
    pipes: Vec<Pipeline>,
    settings: CoreSettings,
    clock: Arc<dyn SessionClock>,
    status: Arc<StatusShared>,
    free_space: Arc<dyn FreeSpace>,
    sleep: Option<SleepDetector>,
    base: Instant,
    next_sleep_poll_ns: u64,
    next_disk_poll_ns: u64,
    disk_critical: bool,
    /// Bytes of the files of tracks that have ended.
    ended_bytes: u64,
    /// Periodic fsyncs done (the 10 s hook of 1a's write order).
    syncs: u64,
    finished: bool,
    outcome: Option<SessionOutcome>,
}

impl SessionCore {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        writer: RecordingWriter,
        specs: Vec<PipelineSpec>,
        settings: CoreSettings,
        clock: Arc<dyn SessionClock>,
        status: Arc<StatusShared>,
        free_space: Arc<dyn FreeSpace>,
        sleep: Option<SleepDetector>,
    ) -> Result<Self, RecordingError> {
        let now = clock.now_ns();
        let mut pipes = Vec::with_capacity(specs.len());
        let mut firsts = Vec::with_capacity(specs.len());
        for spec in specs {
            let (stored_rate, resampler) = if spec.compact {
                let resampler =
                    StreamResampler::new(spec.native.sample_rate, spec.native.channels)?;
                (COMPACT_RATE, Some(resampler))
            } else {
                (spec.native.sample_rate, None)
            };
            pipes.push(Pipeline {
                id: spec.id,
                receiver: spec.receiver,
                native: spec.native,
                stored_rate,
                resampler,
                scratch: Vec::new(),
                expected_next_ns: None,
                last_arrival_ns: now,
                last_anchor_host: spec.first_block.host_ns(),
                stalled: false,
                device_lost: false,
                device_lost_since_ns: now,
                segment_input: 0,
                segment_start: 0,
                pending_end: None,
                sleep_seen: false,
                ended: false,
                frames_received: 0,
                gap_count: 0,
                gap_frames: 0,
                window_peaks: [0.0; 2],
                window_start_ns: now,
            });
            firsts.push(spec.first_block);
        }
        let base = Instant::now();
        let mut core = Self {
            writer: Some(writer),
            pipes,
            settings,
            clock,
            status,
            free_space,
            sleep,
            base,
            next_sleep_poll_ns: now + SLEEP_POLL_NS,
            next_disk_poll_ns: now,
            disk_critical: false,
            ended_bytes: 0,
            syncs: 0,
            finished: false,
            outcome: None,
        };
        core.status.mark_started(now);
        for (index, block) in firsts.into_iter().enumerate() {
            core.process_block(index, &block, now)?;
            core.pipes[index].receiver.recycle(block);
        }
        core.publish();
        Ok(core)
    }

    /// Periodic fsyncs done so far.
    #[cfg(test)]
    pub(crate) fn syncs(&self) -> u64 {
        self.syncs
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.finished
    }

    pub(crate) fn take_outcome(&mut self) -> Option<SessionOutcome> {
        self.outcome.take()
    }

    fn writer(&mut self) -> &mut RecordingWriter {
        self.writer
            .as_mut()
            .expect("the writer lives until the manifest is finished")
    }

    fn instant(&self, now_ns: u64) -> Instant {
        self.base + Duration::from_nanos(now_ns)
    }

    /// One pass over every track. Returns whether any block was handled.
    /// With `stopping` the sources are gone: the queues are drained, the
    /// tracks are ended with `Stopped` and the manifest is finished.
    pub(crate) fn step(&mut self, stopping: bool) -> Result<bool, RecordingError> {
        if self.finished {
            return Ok(false);
        }
        let now = self.clock.now_ns();
        let mut progress = false;

        self.read_signals(now);
        if now >= self.next_sleep_poll_ns {
            self.next_sleep_poll_ns = now + SLEEP_POLL_NS;
            if self.sleep.as_mut().is_some_and(SleepDetector::poll) {
                for pipe in self.pipes.iter_mut().filter(|p| !p.ended) {
                    pipe.sleep_seen = true;
                    pipe.pending_end.get_or_insert(EndedReason::SystemSleep);
                }
            }
        }

        for index in 0..self.pipes.len() {
            progress |= self.drain(index, now, stopping)?;
        }

        self.watchdog(now);
        self.periodic(now)?;
        if self.disk_critical {
            self.end_all(EndedReason::DiskFull)?;
        }
        self.publish();

        if self.pipes.iter().all(|p| p.ended) {
            self.finalize(now)?;
        }
        Ok(progress)
    }

    fn read_signals(&mut self, now: u64) {
        for pipe in self.pipes.iter_mut().filter(|p| !p.ended) {
            let signals = pipe.receiver.take_signals();
            if !signals.any() {
                continue;
            }
            if signals.device_lost && !pipe.device_lost {
                pipe.device_lost = true;
                pipe.device_lost_since_ns = now;
            }
            if signals.system_sleep {
                pipe.sleep_seen = true;
            }
            // One reason per track: the first signal wins, in the order of
            // how specific the cause is.
            let reason = if signals.system_sleep {
                Some(EndedReason::SystemSleep)
            } else if signals.format_changed.is_some_and(|f| f != pipe.native) {
                Some(EndedReason::FormatChanged)
            } else if signals.device_changed {
                Some(EndedReason::DeviceChanged)
            } else {
                None
            };
            if let Some(reason) = reason {
                pipe.pending_end.get_or_insert(reason);
            }
        }
    }

    /// Handle the blocks that are waiting on one track.
    fn drain(&mut self, index: usize, now: u64, stopping: bool) -> Result<bool, RecordingError> {
        let mut progress = false;
        let mut emptied = false;
        for _ in 0..BLOCKS_PER_STEP {
            let Some(block) = self.pipes[index].receiver.pop() else {
                emptied = true;
                break;
            };
            progress = true;
            if !self.pipes[index].ended {
                self.process_block(index, &block, now)?;
            }
            self.pipes[index].receiver.recycle(block);
        }
        if self.pipes[index].ended || !emptied {
            return Ok(progress);
        }
        // The queue is empty: everything the source delivered before an event
        // is stored. Now the track may end.
        if let Some(reason) = self.pipes[index].pending_end.clone() {
            self.end_track(index, reason)?;
        } else if stopping {
            self.stop_track(index)?;
        }
        Ok(progress)
    }

    /// The user stopped: the audio the source dropped after its last block and
    /// the time it stalled at the end are gaps too, then the track ends.
    fn stop_track(&mut self, index: usize) -> Result<(), RecordingError> {
        let trailing = self.pipes[index].receiver.closed().unwrap_or(0);
        if trailing > 0 {
            self.insert_gap(index, trailing, GapReason::Overrun)?;
        }
        let host_now = self.clock.host_ns();
        let pipe = &self.pipes[index];
        if !pipe.ended {
            if let Some(expected) = pipe.expected_next_ns {
                let missing = host_now.saturating_sub(expected);
                if trailing == 0
                    && missing >= self.settings.stall_timeout_ns.max(self.settings.min_gap_ns)
                    && missing <= self.settings.max_gap_fill_ns
                {
                    let frames = missing * u64::from(pipe.native.sample_rate) / 1_000_000_000;
                    let reason = if pipe.device_lost {
                        GapReason::DeviceLoss
                    } else {
                        GapReason::Stall
                    };
                    self.insert_gap(index, frames, reason)?;
                }
            }
        }
        if self.pipes[index].ended {
            return Ok(());
        }
        self.end_track(index, EndedReason::Stopped)
    }

    fn process_block(
        &mut self,
        index: usize,
        block: &Block,
        now: u64,
    ) -> Result<(), RecordingError> {
        let host = block.host_ns();
        let frames = block.frames() as u64;
        let settings = self.settings.clone();
        let (rate, mut gap) = {
            let pipe = &mut self.pipes[index];
            pipe.last_arrival_ns = now;
            pipe.stalled = false;
            let rate = u64::from(pipe.native.sample_rate);
            let mut gap: Option<(u64, GapReason)> = None;
            if block.dropped_before() > 0 {
                gap = Some((block.dropped_before(), GapReason::Overrun));
            } else if let Some(expected) = pipe.expected_next_ns {
                let missing = host.saturating_sub(expected);
                if missing >= settings.min_gap_ns {
                    if missing > settings.max_gap_fill_ns {
                        let reason = if pipe.sleep_seen {
                            EndedReason::SystemSleep
                        } else {
                            EndedReason::Other("clock_discontinuity".into())
                        };
                        pipe.pending_end.get_or_insert(reason);
                        return Ok(());
                    }
                    let reason = if pipe.device_lost {
                        GapReason::DeviceLoss
                    } else {
                        GapReason::Stall
                    };
                    gap = Some((missing * rate / 1_000_000_000, reason));
                }
            }
            (rate, gap)
        };
        if self.pipes[index].pending_end.is_some()
            && matches!(gap, Some((_, GapReason::Stall | GapReason::DeviceLoss)))
        {
            // The track is going to end (a sleep, a device change): audio that
            // arrives after a long silence is not stored behind a stall.
            return Ok(());
        }
        if let Some((native_frames, reason)) = gap.take() {
            self.insert_gap(index, native_frames, reason)?;
            if self.pipes[index].ended {
                return Ok(());
            }
            self.pipes[index].device_lost = false;
            self.anchor(index, host)?;
            if self.pipes[index].ended {
                return Ok(());
            }
        } else if host.saturating_sub(self.pipes[index].last_anchor_host)
            >= settings.anchor_interval_ns
        {
            self.anchor(index, host)?;
            if self.pipes[index].ended {
                return Ok(());
            }
        }
        self.pipes[index].device_lost = false;

        self.push_audio(index, block.samples())?;
        let pipe = &mut self.pipes[index];
        if pipe.ended {
            return Ok(());
        }
        pipe.segment_input += frames;
        pipe.frames_received += frames;
        pipe.expected_next_ns = Some(host + frames * 1_000_000_000 / rate);
        Ok(())
    }

    fn anchor(&mut self, index: usize, host: u64) -> Result<(), RecordingError> {
        let position = self.pipes[index].stored_position();
        let id = self.pipes[index].id.clone();
        let result = self.writer().anchor_at(&id, host, position);
        self.pipes[index].last_anchor_host = host;
        self.guard(index, result)
    }

    fn push_audio(&mut self, index: usize, samples: &[f32]) -> Result<(), RecordingError> {
        let id = self.pipes[index].id.clone();
        let mut scratch = std::mem::take(&mut self.pipes[index].scratch);
        scratch.clear();
        let processed = self.pipes[index]
            .resampler
            .as_mut()
            .map(|resampler| resampler.process(samples, &mut scratch));
        let result = match processed {
            Some(Err(error)) => Err(error),
            Some(Ok(())) => self.writer().push(&id, &scratch),
            None => self.writer().push(&id, samples),
        };
        self.pipes[index].scratch = scratch;
        self.guard(index, result)
    }

    /// Insert `native_frames` of silence into the stored track, ending the
    /// resampler's segment first so stored positions stay exact.
    fn insert_gap(
        &mut self,
        index: usize,
        native_frames: u64,
        reason: GapReason,
    ) -> Result<(), RecordingError> {
        if native_frames == 0 || self.pipes[index].ended {
            return Ok(());
        }
        self.flush_resampler(index)?;
        if self.pipes[index].ended {
            return Ok(());
        }
        let id = self.pipes[index].id.clone();
        let stored = self.pipes[index].to_stored(native_frames);
        if stored > 0 {
            let result = self.writer().gap(&id, stored, reason);
            self.guard(index, result)?;
            if self.pipes[index].ended {
                return Ok(());
            }
            let pipe = &mut self.pipes[index];
            pipe.gap_count += 1;
            pipe.gap_frames += stored;
        }
        let at = self.writer().frames_pushed(&id);
        let pipe = &mut self.pipes[index];
        pipe.segment_start = at;
        pipe.segment_input = 0;
        Ok(())
    }

    /// Push what the resampler still holds and start a fresh one.
    fn flush_resampler(&mut self, index: usize) -> Result<(), RecordingError> {
        let Some(mut resampler) = self.pipes[index].resampler.take() else {
            return Ok(());
        };
        let id = self.pipes[index].id.clone();
        let mut scratch = std::mem::take(&mut self.pipes[index].scratch);
        scratch.clear();
        let finished = resampler.finish(&mut scratch);
        let result = finished.and_then(|()| {
            if scratch.is_empty() {
                Ok(())
            } else {
                self.writer().push(&id, &scratch)
            }
        });
        self.pipes[index].scratch = scratch;
        let pipe = &mut self.pipes[index];
        pipe.resampler = Some(StreamResampler::new(
            pipe.native.sample_rate,
            pipe.native.channels,
        )?);
        self.guard(index, result)
    }

    /// A write error that is a full disk ends the track (cut back); any other
    /// error is the session's.
    fn guard(
        &mut self,
        index: usize,
        result: Result<(), RecordingError>,
    ) -> Result<(), RecordingError> {
        let Err(error) = result else {
            return Ok(());
        };
        if !error.is_disk_full() {
            return Err(error);
        }
        self.cut_back(index)
    }

    fn cut_back(&mut self, index: usize) -> Result<(), RecordingError> {
        let id = self.pipes[index].id.clone();
        if !self.writer().is_track_finished(&id) {
            self.writer().abort_track(&id, EndedReason::DiskFull)?;
        }
        self.mark_ended(index, EndedReason::DiskFull);
        Ok(())
    }

    fn mark_ended(&mut self, index: usize, reason: EndedReason) {
        self.pipes[index].ended = true;
        if let Some(writer) = self.writer.as_ref() {
            let file = writer
                .folder()
                .join(format!("{}.flac", self.pipes[index].id));
            self.ended_bytes += std::fs::metadata(file).map_or(0, |m| m.len());
        }
        if let Some(counters) = self.status.tracks.get() {
            *lock_or_recover(&counters[index].ended, ENDED_LOCK) = Some(reason.clone());
        }
        if reason != EndedReason::Stopped {
            lock_or_recover(&self.status.ended_early, EARLY_LOCK).get_or_insert(reason);
        }
    }

    fn end_track(&mut self, index: usize, reason: EndedReason) -> Result<(), RecordingError> {
        if self.pipes[index].ended {
            return Ok(());
        }
        self.flush_resampler(index)?;
        if self.pipes[index].ended {
            return Ok(());
        }
        let id = self.pipes[index].id.clone();
        match self.writer().finish_track(&id, reason.clone()) {
            Ok(_) => {
                self.mark_ended(index, reason);
                Ok(())
            }
            Err(error) if error.is_disk_full() => self.cut_back(index),
            Err(error) => Err(error),
        }
    }

    fn end_all(&mut self, reason: EndedReason) -> Result<(), RecordingError> {
        for index in 0..self.pipes.len() {
            // Whatever the source delivered is stored first.
            while let Some(block) = self.pipes[index].receiver.pop() {
                if !self.pipes[index].ended {
                    let now = self.clock.now_ns();
                    self.process_block(index, &block, now)?;
                }
                self.pipes[index].receiver.recycle(block);
            }
            self.end_track(index, reason.clone())?;
        }
        Ok(())
    }

    /// The watchdog, the loss grace, the peak window.
    fn watchdog(&mut self, now: u64) {
        let stall = self.settings.stall_timeout_ns;
        let grace = self.settings.device_loss_grace_ns;
        for pipe in self.pipes.iter_mut().filter(|p| !p.ended) {
            pipe.stalled = now.saturating_sub(pipe.last_arrival_ns) >= stall;
            if pipe.device_lost
                && now.saturating_sub(pipe.device_lost_since_ns) >= grace
                && now.saturating_sub(pipe.last_arrival_ns) >= grace
            {
                // A device that reported a problem and did not come back.
                pipe.pending_end.get_or_insert(EndedReason::DeviceChanged);
            }
            let peaks = pipe.receiver.take_peaks();
            for (held, new) in pipe.window_peaks.iter_mut().zip(peaks) {
                *held = held.max(new);
            }
        }
    }

    /// The periodic work: fsync, free space, the FAT limit.
    fn periodic(&mut self, now: u64) -> Result<(), RecordingError> {
        if self.pipes.iter().all(|p| p.ended) {
            return Ok(());
        }
        let instant = self.instant(now);
        match self.writer().sync_if_due(instant) {
            Ok(synced) => self.syncs += u64::from(synced),
            Err(error) if error.is_disk_full() => {
                for index in 0..self.pipes.len() {
                    if !self.pipes[index].ended {
                        self.cut_back(index)?;
                    }
                }
                return Ok(());
            }
            Err(error) => return Err(error),
        }

        if now >= self.next_disk_poll_ns {
            self.next_disk_poll_ns = now + self.settings.disk.poll_interval.as_nanos() as u64;
            let dir = self.writer().folder().to_path_buf();
            if let Ok(free) = self.free_space.available_bytes(&dir) {
                self.status.free_bytes.store(free, Ordering::Relaxed);
                let level = self.settings.disk.level(free);
                self.status
                    .low_disk
                    .store(level >= DiskLevel::Low, Ordering::Relaxed);
                if level == DiskLevel::Critical {
                    self.disk_critical = true;
                }
            }
        }

        if let Some(limit) = self.settings.file_limit_bytes {
            for index in 0..self.pipes.len() {
                if self.pipes[index].ended {
                    continue;
                }
                let id = self.pipes[index].id.clone();
                if self.writer().track_bytes(&id) >= limit {
                    self.pipes[index]
                        .pending_end
                        .get_or_insert(EndedReason::Other("file_size_limit".into()));
                }
            }
        }
        Ok(())
    }

    fn publish(&mut self) {
        let now = self.clock.now_ns();
        let bytes = self.ended_bytes
            + self
                .writer
                .as_ref()
                .map_or(0, RecordingWriter::bytes_written);
        self.status.bytes_written.store(bytes, Ordering::Relaxed);
        let Some(counters) = self.status.tracks.get() else {
            return;
        };
        for (pipe, counters) in self.pipes.iter_mut().zip(counters) {
            let frames = self
                .writer
                .as_ref()
                .map_or(counters.frames.load(Ordering::Relaxed), |w| {
                    w.frames_pushed(&pipe.id)
                });
            if frames > 0 || !pipe.ended {
                counters.frames.store(frames, Ordering::Relaxed);
            }
            counters
                .frames_received
                .store(pipe.frames_received, Ordering::Relaxed);
            counters.gap_count.store(pipe.gap_count, Ordering::Relaxed);
            counters
                .gap_frames
                .store(pipe.gap_frames, Ordering::Relaxed);
            counters
                .dropped
                .store(pipe.receiver.dropped_total(), Ordering::Relaxed);
            counters.stalled.store(pipe.stalled, Ordering::Relaxed);
            counters
                .device_lost
                .store(pipe.device_lost, Ordering::Relaxed);
            counters
                .queued
                .store(pipe.receiver.queued() as u64, Ordering::Relaxed);
            if now.saturating_sub(pipe.window_start_ns) >= PEAK_WINDOW_NS {
                for (slot, peak) in counters.peaks.iter().zip(pipe.window_peaks) {
                    slot.store(peak.to_bits(), Ordering::Relaxed);
                }
                pipe.window_peaks = [0.0; 2];
                pipe.window_start_ns = now;
            }
        }
    }

    /// Every track has ended: write the manifest (1a's step 5).
    fn finalize(&mut self, now: u64) -> Result<(), RecordingError> {
        let writer = self
            .writer
            .take()
            .expect("the writer lives until the manifest is finished");
        let folder = writer.folder().to_path_buf();
        let ended_early = lock_or_recover(&self.status.ended_early, EARLY_LOCK).clone();
        let manifest = writer.finish(EndedReason::Stopped)?;
        if let Some(counters) = self.status.tracks.get() {
            for (entry, counters) in manifest.tracks.iter().zip(counters) {
                counters.frames.store(entry.frames, Ordering::Relaxed);
            }
        }
        self.status
            .bytes_written
            .store(self.ended_bytes, Ordering::Relaxed);
        self.status.mark_ended(now);
        self.outcome = Some(SessionOutcome {
            manifest,
            folder,
            ended_early,
        });
        self.finished = true;
        Ok(())
    }
}
