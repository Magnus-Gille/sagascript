//! The state machine's states and the status snapshot (design §4.3).
//!
//! The snapshot is assembled from atomics the writer thread publishes, plus
//! two short leaf mutexes (the ended reason of a track, the terminal result),
//! so asking for it never waits for the writer and the audio callback is not
//! involved at all. A UI polls [`RecordingSession::status`](super::RecordingSession::status)
//! on a timer; nothing is emitted from the callback.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use super::fsclass::FolderWarning;
use crate::recording::manifest::{EndedReason, Manifest};
use crate::recording::RecordingError;
use crate::sync::lock_or_recover;

const TERMINAL_LOCK: &str = "recording.session.terminal";
const ENDED_LOCK: &str = "recording.session.track_ended";

/// `Idle -> Starting -> Recording -> Finalizing -> Done | Failed`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SessionState {
    /// Created, not started.
    Idle = 0,
    /// Sources are opening and the recording folder is being created.
    Starting = 1,
    /// Audio is being written (also when every track has already ended on its
    /// own and the manifest is being written).
    Recording = 2,
    /// Stopped; the writer is draining and the manifest is being finished.
    Finalizing = 3,
    /// The manifest says `complete`.
    Done = 4,
    /// The session failed. If a folder was created it keeps `state: recording`
    /// and recovery finishes it.
    Failed = 5,
}

impl SessionState {
    pub(crate) fn from_u8(value: u8) -> Self {
        match value {
            0 => Self::Idle,
            1 => Self::Starting,
            2 => Self::Recording,
            3 => Self::Finalizing,
            4 => Self::Done,
            _ => Self::Failed,
        }
    }

    /// Whether the session is over, one way or the other.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed)
    }
}

/// Something the user should know about, besides gaps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionWarning {
    /// Free space is below the low threshold.
    LowDisk,
    /// The folder is on a kind of volume that deserves a warning (D9).
    Folder(FolderWarning),
}

/// One track in a [`SessionStatus`].
#[derive(Clone, Debug, PartialEq)]
pub struct TrackStatus {
    pub id: String,
    pub role: String,
    /// The loudest sample of any channel, 0.0 to 1.0 (more for a clipping
    /// float device), over the last few tenths of a second.
    pub peak: f32,
    /// The same per channel (the second is 0 for a mono track).
    pub channel_peaks: [f32; 2],
    /// Frames stored, silence of gaps included (stored-track frames).
    pub frames: u64,
    /// Frames of audio received from the source (native rate).
    pub frames_received: u64,
    pub gap_count: u32,
    /// Frames of silence in gaps, in stored-track frames.
    pub gap_frames: u64,
    /// Frames the callback dropped because the writer was behind, or that the
    /// source reported lost.
    pub dropped_frames: u64,
    /// No block for the watchdog period right now.
    pub stalled: bool,
    /// The device reported a problem and may come back.
    pub device_lost: bool,
    /// Why the track ended, once it has.
    pub ended: Option<EndedReason>,
    /// Blocks waiting for the writer (a sign it is falling behind).
    pub queued_blocks: usize,
}

/// A point-in-time view of a session.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionStatus {
    pub state: SessionState,
    /// Time since the first block, frozen when the session ends.
    pub elapsed: Duration,
    pub tracks: Vec<TrackStatus>,
    /// Bytes of audio files written so far, headers included.
    pub bytes_written: u64,
    /// Free space on the recordings volume at the last look.
    pub free_disk_bytes: Option<u64>,
    pub warnings: Vec<SessionWarning>,
    /// The code of the error that failed the session.
    pub error_code: Option<&'static str>,
    /// The reason that ended the recording before the user stopped it (a
    /// device change, a full disk, ...), if any.
    pub ended_early: Option<EndedReason>,
}

/// What a finished session produced.
#[derive(Clone, Debug)]
pub struct SessionOutcome {
    /// The manifest now on disk (`state: complete`).
    pub manifest: Manifest,
    /// The recording folder (never put it in an error).
    pub folder: std::path::PathBuf,
    /// The first reason, other than the user stopping, that ended a track.
    pub ended_early: Option<EndedReason>,
}

/// Counters of one track, written by the writer thread.
pub(crate) struct TrackCounters {
    pub id: String,
    pub role: String,
    pub peaks: [AtomicU32; 2],
    pub frames: AtomicU64,
    pub frames_received: AtomicU64,
    pub gap_count: AtomicU32,
    pub gap_frames: AtomicU64,
    pub dropped: AtomicU64,
    pub stalled: AtomicBool,
    pub device_lost: AtomicBool,
    pub queued: AtomicU64,
    pub ended: Mutex<Option<EndedReason>>,
}

impl TrackCounters {
    pub(crate) fn new(id: &str, role: &str) -> Self {
        Self {
            id: id.to_string(),
            role: role.to_string(),
            peaks: [AtomicU32::new(0), AtomicU32::new(0)],
            frames: AtomicU64::new(0),
            frames_received: AtomicU64::new(0),
            gap_count: AtomicU32::new(0),
            gap_frames: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            stalled: AtomicBool::new(false),
            device_lost: AtomicBool::new(false),
            queued: AtomicU64::new(0),
            ended: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> TrackStatus {
        let peak = |slot: &AtomicU32| f32::from_bits(slot.load(Ordering::Relaxed));
        let channel_peaks = [peak(&self.peaks[0]), peak(&self.peaks[1])];
        TrackStatus {
            id: self.id.clone(),
            role: self.role.clone(),
            peak: channel_peaks[0].max(channel_peaks[1]),
            channel_peaks,
            frames: self.frames.load(Ordering::Relaxed),
            frames_received: self.frames_received.load(Ordering::Relaxed),
            gap_count: self.gap_count.load(Ordering::Relaxed),
            gap_frames: self.gap_frames.load(Ordering::Relaxed),
            dropped_frames: self.dropped.load(Ordering::Relaxed),
            stalled: self.stalled.load(Ordering::Relaxed),
            device_lost: self.device_lost.load(Ordering::Relaxed),
            ended: lock_or_recover(&self.ended, ENDED_LOCK).clone(),
            queued_blocks: self.queued.load(Ordering::Relaxed) as usize,
        }
    }
}

/// The result a stopped session leaves for whoever asks.
#[derive(Default)]
pub(crate) struct Terminal {
    pub result: Option<Result<SessionOutcome, RecordingError>>,
}

/// Everything the writer thread publishes and the status snapshot reads.
pub(crate) struct StatusShared {
    state: AtomicU8,
    /// Session clock at the first block; `u64::MAX` before.
    started_ns: AtomicU64,
    /// Session clock when the session ended; `u64::MAX` while it runs.
    ended_ns: AtomicU64,
    pub bytes_written: AtomicU64,
    /// `u64::MAX` when unknown.
    pub free_bytes: AtomicU64,
    pub low_disk: AtomicBool,
    pub tracks: OnceLock<Vec<TrackCounters>>,
    pub folder_warnings: OnceLock<Vec<FolderWarning>>,
    pub ended_early: Mutex<Option<EndedReason>>,
    pub error_code: Mutex<Option<&'static str>>,
    pub terminal: Mutex<Terminal>,
    pub terminal_changed: Condvar,
}

impl StatusShared {
    pub(crate) fn new() -> Self {
        Self {
            state: AtomicU8::new(SessionState::Idle as u8),
            started_ns: AtomicU64::new(u64::MAX),
            ended_ns: AtomicU64::new(u64::MAX),
            bytes_written: AtomicU64::new(0),
            free_bytes: AtomicU64::new(u64::MAX),
            low_disk: AtomicBool::new(false),
            tracks: OnceLock::new(),
            folder_warnings: OnceLock::new(),
            ended_early: Mutex::new(None),
            error_code: Mutex::new(None),
            terminal: Mutex::new(Terminal::default()),
            terminal_changed: Condvar::new(),
        }
    }

    pub(crate) fn state(&self) -> SessionState {
        SessionState::from_u8(self.state.load(Ordering::Acquire))
    }

    pub(crate) fn set_state(&self, state: SessionState) {
        self.state.store(state as u8, Ordering::Release);
    }

    /// Move to `to` only from `from`; false when the state was something else.
    pub(crate) fn transition(&self, from: SessionState, to: SessionState) -> bool {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn mark_started(&self, now_ns: u64) {
        let _ =
            self.started_ns
                .compare_exchange(u64::MAX, now_ns, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(crate) fn mark_ended(&self, now_ns: u64) {
        let _ =
            self.ended_ns
                .compare_exchange(u64::MAX, now_ns, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(crate) fn snapshot(&self, now_ns: u64) -> SessionStatus {
        let state = self.state();
        let started = self.started_ns.load(Ordering::Acquire);
        let ended = self.ended_ns.load(Ordering::Acquire);
        let elapsed = if started == u64::MAX {
            Duration::ZERO
        } else {
            let end = if ended == u64::MAX { now_ns } else { ended };
            Duration::from_nanos(end.saturating_sub(started))
        };
        let mut warnings: Vec<SessionWarning> = self
            .folder_warnings
            .get()
            .map(|all| all.iter().copied().map(SessionWarning::Folder).collect())
            .unwrap_or_default();
        if self.low_disk.load(Ordering::Relaxed) {
            warnings.push(SessionWarning::LowDisk);
        }
        let free = self.free_bytes.load(Ordering::Relaxed);
        SessionStatus {
            state,
            elapsed,
            tracks: self
                .tracks
                .get()
                .map(|tracks| tracks.iter().map(TrackCounters::snapshot).collect())
                .unwrap_or_default(),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
            free_disk_bytes: (free != u64::MAX).then_some(free),
            warnings,
            error_code: *lock_or_recover(&self.error_code, TERMINAL_LOCK),
            ended_early: lock_or_recover(&self.ended_early, TERMINAL_LOCK).clone(),
        }
    }
}
