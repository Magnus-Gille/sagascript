//! `RecordingSession`: capture on top of the recording store (SAGA-563, design
//! §4.3).
//!
//! A session owns the capture sources, one bounded block queue per track and a
//! **writer thread** that drains the queues into the 1a [`RecordingWriter`].
//! It is the state machine
//!
//! ```text
//! Idle ─start─► Starting ─► Recording ─stop─► Finalizing ─► Done
//!                  │            │                 │
//!                  └────────────┴─────────────────┴──────► Failed
//! ```
//!
//! * **Starting** opens the sources, waits for the first block of every track
//!   (so a track's `start_ns` is the host time of its first frame) and only
//!   then creates the recording folder. A source that never delivers leaves
//!   no folder behind.
//! * **Recording**: the writer thread runs [`SessionCore::step`](core).
//!   Gaps, anchors, endings, free space and the periodic fsync happen there.
//!   A track that ends on its own (a device change, a full disk, ...) does not
//!   stop the session; when the last track has ended the writer finishes the
//!   manifest and the state becomes `Done` without waiting for `stop`.
//! * **Finalizing**: `stop` has stopped the sources; the writer drains what
//!   they delivered, ends the tracks with `stopped` and finishes the manifest.
//!
//! # Concurrency and locks
//!
//! The audio callback shares nothing but atomics and the lock-free queues with
//! the rest (see [`capture`](super::capture)). The session has the mutex
//! `recording.session` (`inner`), which serialises `start` and elects the one
//! `stop` that owns the shutdown, and the mutex `recording.session.sources`
//! around the capture sources. `stop_all` holds the latter while the sources
//! stop, so a second caller (the writer thread ending the recording by
//! itself, or a second `stop`) waits until they have: nobody sees "stopped"
//! while a source still delivers. Both are leaves apart from the status
//! leaves (`recording.session.track_ended`, `recording.session.terminal`),
//! which are taken inside them and never the other way round; a source's
//! `stop` takes no lock of this module, so holding `sources` across it cannot
//! deadlock, and the writer thread never joins itself. No lock is held across
//! an `.await` (the session has none). Poison is recovered with
//! [`lock_or_recover`]: every critical section is a few assignments. The
//! status snapshot takes only the status leaves, so asking for it never waits
//! for `start` or `stop`.
//!
//! # Who stops the sources
//!
//! Whoever ends the recording stops them *before* the terminal state is
//! visible: `stop` (the owner, before it tells the writer to finish) or the
//! writer thread (when every track has ended by itself, on failure, or when
//! it panics). A `Done` or `Failed` session never has an open microphone.
//!
//! [`RecordingWriter`]: super::RecordingWriter

mod core;
pub mod disk;
pub mod fsclass;
pub mod resampler;
pub mod status;
#[cfg(windows)]
mod win_volume;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use self::core::{CoreSettings, PipelineSpec, SessionCore};
use self::disk::{
    uncompressed_bytes_per_second, DiskLevel, DiskPolicy, FreeSpace, VolumeFreeSpace,
};
use self::fsclass::{folder_warnings, FolderWarning, FAT32_TRACK_LIMIT_BYTES};
use self::resampler::COMPACT_RATE;
pub use self::status::{SessionOutcome, SessionState, SessionStatus, SessionWarning, TrackStatus};
use self::status::{StatusShared, TrackCounters};
use super::capture::{
    system_sleep_probe, track_channel, CaptureSource, ChannelConfig, SessionClock, SignalHandle,
    SleepDetector, SleepProbe, SourceSignal, SystemClock, HOST_CLOCK_DOMAIN,
};
use super::manifest::{Client, Quality};
use super::store::{NewRecording, NewTrack, RecordingStore};
use super::RecordingError;
use crate::sync::lock_or_recover;

const INNER_LOCK: &str = "recording.session";
const SOURCES_LOCK: &str = "recording.session.sources";
/// The shortest sleep that is noticed: the clocks agree to nanoseconds while
/// the machine is awake (a Windows tick is 15 ms), so a tenth of a second is
/// a sleep.
const SLEEP_THRESHOLD: Duration = Duration::from_millis(100);
const TERMINAL_LOCK: &str = "recording.session.terminal";

/// One source and the track it records to.
pub struct SourceSpec {
    /// The track id: also the stem of its files (`mic` gives `mic.flac`).
    pub id: String,
    /// What it is: `mic`, later `system`.
    pub role: String,
    pub source: Box<dyn CaptureSource>,
}

impl SourceSpec {
    /// The microphone track.
    pub fn mic(source: impl CaptureSource + 'static) -> Self {
        Self {
            id: "mic".into(),
            role: "mic".into(),
            source: Box::new(source),
        }
    }
}

/// How a session behaves. [`SessionConfig::new`] gives the defaults of the
/// design; the other fields exist for tests and for phase 1c's settings.
#[derive(Clone)]
pub struct SessionConfig {
    /// Free text; the folder name is a sanitised form of it.
    pub title: String,
    /// RFC 3339 with the local offset, for example `2026-11-05T14:02:11.482+01:00`.
    pub started_at: String,
    pub client: Client,
    pub quality: Quality,
    /// How much audio the queue of a track absorbs while the writer is busy
    /// (default 4 s). Past it, frames are dropped and become an overrun gap.
    pub queue_seconds: f64,
    /// Frames a pooled block holds (default 1024).
    pub block_frames: usize,
    /// The watchdog: no block for this long is a stall (500 ms).
    pub stall_timeout: Duration,
    /// A jump of the host stamps of at least this much between two blocks is
    /// a gap, even without a watchdog trip (100 ms).
    pub min_gap: Duration,
    /// A jump of more than this is not filled with silence: the track ends (60 s).
    pub max_gap_fill: Duration,
    /// Clock anchors in the journal every this much host time (10 s).
    pub anchor_interval: Duration,
    /// A device that reported a problem and stays silent this long has gone (10 s).
    pub device_loss_grace: Duration,
    /// How long to wait for the first block of every track (real time, 5 s).
    pub start_timeout: Duration,
    /// How often the writer thread looks when idle (real time, 5 ms).
    pub tick: Duration,
    /// Free-space thresholds; `None` derives them from the tracks.
    pub disk: Option<DiskPolicy>,
    /// What the writer measures timeouts with.
    pub clock: Arc<dyn SessionClock>,
    pub free_space: Arc<dyn FreeSpace>,
    /// Warnings about the folder; `None` detects them from the file system.
    pub folder_warnings: Option<Vec<FolderWarning>>,
    /// Look for a system sleep with `sleep_probe`. Tests that signal a sleep
    /// by hand turn it off.
    pub detect_sleep: bool,
    /// Reads how long the machine has slept (see `capture::sleep_offset_ns`);
    /// a test supplies its own.
    pub sleep_probe: SleepProbe,
    /// Tests: called right after the writer thread was spawned, to interleave
    /// what the writer does with the rest of `start`.
    #[cfg(test)]
    pub(crate) after_spawn: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl SessionConfig {
    pub fn new(
        title: impl Into<String>,
        started_at: impl Into<String>,
        client: Client,
        quality: Quality,
    ) -> Self {
        Self {
            title: title.into(),
            started_at: started_at.into(),
            client,
            quality,
            queue_seconds: 4.0,
            block_frames: 1024,
            stall_timeout: Duration::from_millis(500),
            min_gap: Duration::from_millis(100),
            max_gap_fill: Duration::from_secs(60),
            anchor_interval: Duration::from_secs(10),
            device_loss_grace: Duration::from_secs(10),
            start_timeout: Duration::from_secs(5),
            tick: Duration::from_millis(5),
            disk: None,
            clock: Arc::new(SystemClock::new()),
            free_space: Arc::new(VolumeFreeSpace),
            folder_warnings: None,
            detect_sleep: true,
            sleep_probe: system_sleep_probe(),
            #[cfg(test)]
            after_spawn: None,
        }
    }
}

struct Inner {
    started: bool,
    /// Set by the one caller of `stop` that owns the shutdown.
    shutdown_started: bool,
    thread: Option<JoinHandle<()>>,
}

/// The capture sources, shared between the session (`stop`) and the writer
/// thread (which stops them when the recording ends without a `stop`).
///
/// [`stop_all`](Self::stop_all) holds the set's mutex for the whole time the
/// sources take to stop, so whoever calls it second waits until the first is
/// done: nobody can conclude "the sources are stopped" while one is still
/// delivering. The mutex is a leaf: a source's `stop` takes no lock of this
/// module.
struct SourceSet {
    sources: Mutex<Vec<Box<dyn CaptureSource>>>,
}

impl SourceSet {
    fn new() -> Self {
        Self {
            sources: Mutex::new(Vec::new()),
        }
    }

    fn set(&self, sources: Vec<Box<dyn CaptureSource>>) {
        *lock_or_recover(&self.sources, SOURCES_LOCK) = sources;
    }

    /// Stop every source (each `stop` is idempotent). When this returns no
    /// callback of any source runs any more.
    fn stop_all(&self) {
        for source in lock_or_recover(&self.sources, SOURCES_LOCK).iter_mut() {
            source.stop();
        }
    }
}

/// A recording in progress. `Send + Sync`: the app holds one behind an `Arc`
/// and asks for its status from a poll timer while a command stops it.
pub struct RecordingSession {
    store: RecordingStore,
    config: SessionConfig,
    status: Arc<StatusShared>,
    inner: Mutex<Inner>,
    sources: Arc<SourceSet>,
    stop_requested: Arc<AtomicBool>,
    handles: OnceLock<Vec<SignalHandle>>,
    folder: OnceLock<PathBuf>,
}

impl RecordingSession {
    /// A session in the `Idle` state.
    pub fn new(store: RecordingStore, config: SessionConfig) -> Self {
        Self {
            store,
            config,
            status: Arc::new(StatusShared::new()),
            inner: Mutex::new(Inner {
                started: false,
                shutdown_started: false,
                thread: None,
            }),
            sources: Arc::new(SourceSet::new()),
            stop_requested: Arc::new(AtomicBool::new(false)),
            handles: OnceLock::new(),
            folder: OnceLock::new(),
        }
    }

    pub fn state(&self) -> SessionState {
        self.status.state()
    }

    /// A snapshot: elapsed time, per-source peak level, gap count, bytes
    /// written and free disk. Never waits for the writer.
    pub fn status(&self) -> SessionStatus {
        self.status.snapshot(self.config.clock.now_ns())
    }

    /// The recording folder, once it exists.
    pub fn folder(&self) -> Option<PathBuf> {
        self.folder.get().cloned()
    }

    /// Tell the session the system is going to sleep (or has just woken). The
    /// tracks end with `system_sleep`. On macOS the session also notices a
    /// sleep by itself from the clocks; this is the seam for a notification
    /// (`NSWorkspaceWillSleepNotification`) that phase 1c may add.
    pub fn notify_system_sleep(&self) {
        if let Some(handles) = self.handles.get() {
            for handle in handles {
                handle.signal(SourceSignal::SystemSleep);
            }
        }
    }

    /// Open the sources and start recording. Returns when the first audio has
    /// arrived and the recording folder exists, or with the error that kept
    /// it from starting (the session is then `Failed`). A session starts once.
    pub fn start(&self, sources: Vec<SourceSpec>) -> Result<(), RecordingError> {
        let mut inner = lock_or_recover(&self.inner, INNER_LOCK);
        if inner.started {
            return Err(RecordingError::InvalidInput {
                reason: "the session was already started",
            });
        }
        if sources.is_empty() {
            return Err(RecordingError::InvalidInput {
                reason: "a recording needs a source",
            });
        }
        inner.started = true;
        self.status.set_state(SessionState::Starting);
        match self.start_locked(&mut inner, sources) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.fail(error.clone());
                Err(error)
            }
        }
    }

    fn start_locked(
        &self,
        inner: &mut Inner,
        mut specs: Vec<SourceSpec>,
    ) -> Result<(), RecordingError> {
        let config = &self.config;
        let compact = config.quality == Quality::Compact;

        // What each track will store, before any device is opened.
        let mut stored = Vec::with_capacity(specs.len());
        let mut worst_case = 0u64;
        for spec in &specs {
            let format = spec.source.format();
            if !(1..=2).contains(&format.channels) || format.sample_rate == 0 {
                return Err(RecordingError::UnsupportedFormat {
                    reason: "a source delivers one or two channels at a real sample rate",
                });
            }
            let (rate, channels, bits) = if compact {
                (COMPACT_RATE, 1, 16)
            } else {
                (format.sample_rate, format.channels, 24)
            };
            worst_case += uncompressed_bytes_per_second(rate, channels, bits);
            stored.push((rate, channels, bits));
        }
        let ids: std::collections::BTreeSet<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        if ids.len() != specs.len() {
            return Err(RecordingError::InvalidInput {
                reason: "track ids must be distinct",
            });
        }

        // Space and folder, before the microphone is touched.
        let policy = config
            .disk
            .unwrap_or_else(|| DiskPolicy::for_rate(worst_case));
        if let Ok(free) = config.free_space.available_bytes(self.store.root()) {
            self.status
                .free_bytes
                .store(free, std::sync::atomic::Ordering::Relaxed);
            if policy.level(free) == DiskLevel::Critical {
                return Err(RecordingError::DiskFull {
                    action: "start a recording",
                });
            }
        }
        let warnings = config
            .folder_warnings
            .clone()
            .unwrap_or_else(|| folder_warnings(self.store.root()));
        let file_limit = warnings
            .contains(&FolderWarning::Fat32)
            .then_some(FAT32_TRACK_LIMIT_BYTES);
        let _ = self.status.folder_warnings.set(warnings);

        // Queues and counters.
        let mut sinks = Vec::with_capacity(specs.len());
        let mut receivers = Vec::with_capacity(specs.len());
        let mut counters = Vec::with_capacity(specs.len());
        for spec in &specs {
            let format = spec.source.format();
            let (sink, receiver) = track_channel(
                format,
                ChannelConfig::for_seconds(format, config.block_frames, config.queue_seconds),
            );
            sinks.push(sink);
            receivers.push(receiver);
            counters.push(TrackCounters::new(&spec.id, &spec.role));
        }
        let _ = self.handles.set(sinks.iter().map(|s| s.handle()).collect());
        let _ = self.status.tracks.set(counters);

        // The sleep detector reads its baseline now, before any device is
        // opened, so a sleep that comes before its first poll still counts.
        let sleep = config
            .detect_sleep
            .then(|| SleepDetector::armed(SLEEP_THRESHOLD, Arc::clone(&config.sleep_probe)));

        // Open the sources.
        let mut started = 0;
        let mut start_error = None;
        for (spec, sink) in specs.iter_mut().zip(sinks) {
            match spec.source.start(sink) {
                Ok(()) => started += 1,
                Err(error) => {
                    start_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = start_error {
            for spec in specs.iter_mut().take(started) {
                spec.source.stop();
            }
            return Err(error);
        }

        // Wait for the first block of every track.
        let deadline = Instant::now() + config.start_timeout;
        let mut firsts: Vec<Option<super::capture::Block>> =
            receivers.iter().map(|_| None).collect();
        while firsts.iter().any(Option::is_none) {
            for (first, receiver) in firsts.iter_mut().zip(receivers.iter_mut()) {
                if first.is_none() {
                    *first = receiver.pop();
                }
            }
            if firsts.iter().all(Option::is_some) {
                break;
            }
            if Instant::now() >= deadline {
                for spec in specs.iter_mut() {
                    spec.source.stop();
                }
                return Err(RecordingError::SourceUnavailable {
                    reason: "the source delivered no audio",
                });
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        // The folder, with each track stamped with the time of its first frame.
        let request = NewRecording {
            title: config.title.clone(),
            started_at: config.started_at.clone(),
            client: config.client.clone(),
            quality: config.quality,
            tracks: specs
                .iter()
                .zip(&stored)
                .zip(&firsts)
                .map(|((spec, (rate, channels, bits)), first)| {
                    let native = spec.source.format();
                    NewTrack {
                        id: spec.id.clone(),
                        role: spec.role.clone(),
                        sample_rate: *rate,
                        channels: *channels,
                        bits: *bits,
                        clock_domain: HOST_CLOCK_DOMAIN.to_string(),
                        start_ns: first.as_ref().map_or(0, |block| block.host_ns()),
                        resampled_from: (compact && native.sample_rate != COMPACT_RATE)
                            .then_some(native.sample_rate),
                        device: spec.source.device_name(),
                    }
                })
                .collect(),
        };
        let writer = match self.store.create(request) {
            Ok(writer) => writer,
            Err(error) => {
                for spec in specs.iter_mut() {
                    spec.source.stop();
                }
                return Err(error);
            }
        };
        let _ = self.folder.set(writer.folder().to_path_buf());

        let pipeline_specs: Vec<PipelineSpec> = specs
            .iter()
            .zip(receivers)
            .zip(firsts)
            .map(|((spec, receiver), first)| PipelineSpec {
                id: spec.id.clone(),
                native: spec.source.format(),
                compact,
                receiver,
                first_block: first.expect("every track has its first block"),
            })
            .collect();
        let settings = CoreSettings {
            stall_timeout_ns: config.stall_timeout.as_nanos() as u64,
            min_gap_ns: config.min_gap.as_nanos() as u64,
            max_gap_fill_ns: config.max_gap_fill.as_nanos() as u64,
            anchor_interval_ns: config.anchor_interval.as_nanos() as u64,
            device_loss_grace_ns: config.device_loss_grace.as_nanos() as u64,
            disk: policy,
            file_limit_bytes: file_limit,
        };
        let core = match SessionCore::new(
            writer,
            pipeline_specs,
            settings,
            Arc::clone(&config.clock),
            Arc::clone(&self.status),
            Arc::clone(&config.free_space),
            sleep,
        ) {
            Ok(core) => core,
            Err(error) => {
                for spec in specs.iter_mut() {
                    spec.source.stop();
                }
                return Err(error);
            }
        };

        // The sources belong to the set before the writer exists: a writer that
        // finishes at once must find them there to stop. The state is
        // `Recording` before the writer can publish a terminal state, so
        // nothing here can overwrite one.
        self.sources
            .set(specs.into_iter().map(|spec| spec.source).collect());
        self.status.set_state(SessionState::Recording);
        let status = Arc::clone(&self.status);
        let stop = Arc::clone(&self.stop_requested);
        let sources = Arc::clone(&self.sources);
        let tick = config.tick;
        let spawned = std::thread::Builder::new()
            .name("recording-writer".into())
            .spawn(move || writer_loop(core, stop, status, sources, tick));
        let thread = match spawned {
            Ok(thread) => thread,
            Err(_) => {
                self.sources.stop_all();
                return Err(RecordingError::SessionFailed {
                    reason: "could not start the writer thread",
                });
            }
        };
        inner.thread = Some(thread);
        #[cfg(test)]
        if let Some(hook) = &config.after_spawn {
            hook();
        }
        Ok(())
    }

    /// Stop recording and finish the manifest. Safe to call twice and from two
    /// threads at once: every call returns the same result once the first has
    /// finished. A session that was never started has nothing to stop.
    pub fn stop(&self) -> Result<SessionOutcome, RecordingError> {
        // One caller owns the shutdown: it stops the sources and only then
        // tells the writer to finish. Every other caller just waits for the
        // result, so none of them can make the writer finish while a source
        // is still delivering.
        let owner = {
            let mut inner = lock_or_recover(&self.inner, INNER_LOCK);
            if !inner.started {
                return Err(RecordingError::NotRecording);
            }
            if inner.shutdown_started {
                None
            } else {
                inner.shutdown_started = true;
                self.status
                    .transition(SessionState::Recording, SessionState::Finalizing);
                Some(inner.thread.take())
            }
        };
        if let Some(thread) = owner {
            // No callback runs once this returns, so the writer sees every block.
            self.sources.stop_all();
            self.stop_requested.store(true, Ordering::Release);
            if let Some(thread) = thread {
                let _ = thread.join();
            }
        }
        self.wait_terminal()
    }

    fn wait_terminal(&self) -> Result<SessionOutcome, RecordingError> {
        let mut terminal = lock_or_recover(&self.status.terminal, TERMINAL_LOCK);
        loop {
            if let Some(result) = terminal.result.clone() {
                return result;
            }
            terminal = self
                .status
                .terminal_changed
                .wait(terminal)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn fail(&self, error: RecordingError) {
        finish_with(&self.status, Err(error));
    }
}

impl Drop for RecordingSession {
    fn drop(&mut self) {
        // A session dropped while recording is stopped and finished, as far as
        // it can be, rather than abandoned with a writer thread running.
        let started = lock_or_recover(&self.inner, INNER_LOCK).started;
        if started {
            let _ = self.stop();
        }
    }
}

/// Store the terminal result, set the final state and wake waiters.
fn finish_with(status: &StatusShared, result: Result<SessionOutcome, RecordingError>) {
    {
        let mut terminal = lock_or_recover(&status.terminal, TERMINAL_LOCK);
        if terminal.result.is_some() {
            return;
        }
        match &result {
            Ok(_) => status.set_state(SessionState::Done),
            Err(error) => {
                *lock_or_recover(&status.error_code, TERMINAL_LOCK) = Some(error.code());
                status.set_state(SessionState::Failed);
            }
        }
        terminal.result = Some(result);
    }
    status.terminal_changed.notify_all();
}

/// Marks the session failed if the writer thread unwinds.
struct PanicGuard {
    status: Arc<StatusShared>,
    sources: Arc<SourceSet>,
    armed: bool,
}

impl Drop for PanicGuard {
    fn drop(&mut self) {
        if self.armed {
            // The sources first, as in the normal ending; a panic in a
            // source's `stop` must not become a second panic while unwinding.
            let sources = &self.sources;
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sources.stop_all()));
            finish_with(
                &self.status,
                Err(RecordingError::SessionFailed {
                    reason: "the recording writer stopped unexpectedly",
                }),
            );
        }
    }
}

fn writer_loop(
    mut core: SessionCore,
    stop: Arc<AtomicBool>,
    status: Arc<StatusShared>,
    sources: Arc<SourceSet>,
    tick: Duration,
) {
    let mut guard = PanicGuard {
        status: Arc::clone(&status),
        sources: Arc::clone(&sources),
        armed: true,
    };
    let result = loop {
        let stopping = stop.load(Ordering::Acquire);
        match core.step(stopping) {
            Ok(progress) => {
                if core.is_finished() {
                    break Ok(core
                        .take_outcome()
                        .expect("a finished core has its outcome"));
                }
                if !progress {
                    std::thread::sleep(tick);
                }
            }
            Err(error) => break Err(error),
        }
    };
    guard.armed = false;
    // Dropping the core on an error releases the folder's lock and leaves the
    // manifest at `recording`, which recovery finishes.
    drop(core);
    // The recording is over, by `stop` or by itself (a track that ended, a
    // full disk, a failure): close the sources *before* anyone can see the
    // terminal state, so a finished session never leaves a microphone open.
    // This is the writer thread, which does not join itself: stopping a
    // source never waits for the writer.
    sources.stop_all();
    finish_with(&status, result);
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod core_tests;
#[cfg(test)]
#[path = "session_tests.rs"]
mod session_tests;
