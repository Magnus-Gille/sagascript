//! The recorder's state in the app (SAGA-564, docs/recorder-design.md §4.5-4.7).
//!
//! `SharedRecorder` is Tauri-managed state. It holds the one
//! [`RecordingSession`] the app may run (the app records the microphone only,
//! one session at a time), the last session's result, and the notes recovery
//! left for the user. The commands in `commands/recording.rs` are thin; the
//! decisions live here as plain functions with tests, over two seams:
//!
//! - [`SourceFactory`] opens the capture source. The app uses
//!   [`MicFactory`]; tests inject a [`SyntheticSource`](sagascript_core::recording::SyntheticSource)
//!   so no test ever opens a device. The app never reads
//!   `SAGASCRIPT_TEST_SOURCE`.
//! - [`RecordingTrash`] moves a recording folder to the Trash (`trash.rs`).
//!
//! # Threads
//!
//! `MicSource::open` only asks `cpal` for the default input device and its
//! format; `start` spawns the thread that owns the `!Send` stream. Neither
//! touches AppKit, `enigo` or the Text Input Sources, the calls that need the
//! main thread (ADR 0003): the dictation capture does the same from its own
//! thread. Both run on the blocking pool because `RecordingSession::start`
//! waits (up to five seconds) for the first audio. The microphone permission
//! prompt is answered before that (`commands::recording`), so the wait never
//! covers a dialog.
//!
//! The events are sent by one `recording-events` thread per session, from the
//! session's status snapshot every 250 ms: never from the audio callback.
//!
//! # Lock
//!
//! `app.recorder` (`SharedRecorder::state`) is a leaf in the lock order of
//! docs/ARCHITECTURE.md §3: it guards a few assignments, is never held across
//! an `.await`, and the calls that wait (`RecordingSession::start`/`stop`,
//! `summarize_folder`, recovery) all run outside it. The commands read the
//! controller first (a short block that is released) and only then take this
//! lock; nothing takes `SharedController` while holding it.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sagascript_core::recording::capture::{CaptureSource, MicConfig, MicSource};
use sagascript_core::recording::session::disk::{FreeSpace, VolumeFreeSpace};
use sagascript_core::recording::session::fsclass::{folder_warnings, FolderWarning};
use sagascript_core::recording::{
    default_recordings_dir, read_recording, summarize_folder, Client, EndedReason, RecordingError,
    RecordingSession, RecordingStore, RecordingSummary, RecoveryOutcome, RecoveryReport,
    SessionConfig, SessionState, SessionStatus, SessionWarning, SourceSpec,
};
use sagascript_core::settings::{expand_home, RecordingQuality, Settings};
use sagascript_core::sync::lock_or_recover;
use serde::Serialize;

use crate::command_error::CommandError;
use crate::trash::{recording_folder, RecordingTrash, SystemTrash};

const RECORDER_LOCK: &str = "app.recorder";

/// How often the session's status is sent while it records or finalizes.
pub const EVENT_INTERVAL: Duration = Duration::from_millis(250);

/// The title of a recording started without one.
pub const DEFAULT_TITLE: &str = "Recording";

/// A peak at or above this level counts as clipping (full scale is 1.0).
const CLIP_LEVEL: f32 = 0.999;

// -- Wire types (the TypeScript twins are in src/lib/record-types.ts) --------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingSessionState {
    Idle,
    Starting,
    Recording,
    Finalizing,
    Done,
    Failed,
}

impl From<SessionState> for RecordingSessionState {
    fn from(state: SessionState) -> Self {
        match state {
            SessionState::Idle => Self::Idle,
            SessionState::Starting => Self::Starting,
            SessionState::Recording => Self::Recording,
            SessionState::Finalizing => Self::Finalizing,
            SessionState::Done => Self::Done,
            SessionState::Failed => Self::Failed,
        }
    }
}

/// The live state of the recording session: what `get_recording_session`
/// returns and `recording-session-changed` carries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordingSessionInfo {
    pub state: RecordingSessionState,
    pub recording_id: Option<String>,
    pub folder_name: Option<String>,
    pub title: Option<String>,
    pub quality: Option<RecordingQuality>,
    pub elapsed_ms: u64,
    /// Peak level 0..1, the maximum over the channels of every track.
    pub level: f32,
    pub clipped: bool,
    pub gap_count: u32,
    pub gap_ms: u64,
    /// Audio stopped arriving from the device.
    pub stalled: bool,
    pub device_lost: bool,
    pub bytes_written: u64,
    pub free_disk_bytes: Option<u64>,
    /// `low_disk`, `fat32`, `exfat`, `network_volume`, `cloud_synced`.
    pub warnings: Vec<String>,
    /// Why the recording ended before the user stopped it (`device_changed`,
    /// `format_changed`, `system_sleep`, `disk_full`, ...).
    pub ended_early: Option<String>,
    /// The `recording_*` code of the error that failed the session.
    pub error_code: Option<String>,
}

impl RecordingSessionInfo {
    /// No session: nothing running, nothing to report.
    pub fn idle() -> Self {
        Self::bare(RecordingSessionState::Idle)
    }

    fn bare(state: RecordingSessionState) -> Self {
        Self {
            state,
            recording_id: None,
            folder_name: None,
            title: None,
            quality: None,
            elapsed_ms: 0,
            level: 0.0,
            clipped: false,
            gap_count: 0,
            gap_ms: 0,
            stalled: false,
            device_lost: false,
            bytes_written: 0,
            free_disk_bytes: None,
            warnings: Vec::new(),
            ended_early: None,
            error_code: None,
        }
    }
}

/// What `stop_recording_session` returns.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordingFinished {
    pub summary: RecordingSummary,
    /// Why the recording ended before the user stopped it, if it did.
    pub ended_early: Option<String>,
}

/// What recovery did to one recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecoveryNote {
    pub folder_name: String,
    pub title: String,
    pub outcome: RecoveryNoteOutcome,
    pub frames_kept: u64,
    pub tail_bytes: u64,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryNoteOutcome {
    Recovered,
    AlreadyComplete,
    SkippedLocked,
    Failed,
}

/// The recordings folder and what is known about its volume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordingFolderInfo {
    pub path: String,
    pub exists: bool,
    pub free_bytes: Option<u64>,
    /// `fat32`, `exfat`, `network_volume`, `cloud_synced`.
    pub warnings: Vec<String>,
    pub is_default: bool,
}

// -- Pure builders -----------------------------------------------------------------

/// What the app remembers about a running session besides its status.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionMeta {
    pub title: String,
    pub quality: RecordingQuality,
    pub recording_id: String,
    pub folder_name: String,
    /// The sample rate the first track is stored at, to turn gap frames into
    /// time (`recording_quality` compact stores 16 kHz whatever the device does).
    pub track_rate: u32,
}

fn warning_id(warning: &SessionWarning) -> &'static str {
    match warning {
        SessionWarning::LowDisk => "low_disk",
        SessionWarning::Folder(folder) => folder_warning_id(*folder),
    }
}

fn folder_warning_id(warning: FolderWarning) -> &'static str {
    match warning {
        FolderWarning::Fat32 => "fat32",
        FolderWarning::ExFat => "exfat",
        FolderWarning::NetworkVolume => "network_volume",
        FolderWarning::CloudSynced => "cloud_synced",
    }
}

/// The reason id the manifest and the UI use for an early end.
pub fn ended_reason_id(reason: &EndedReason) -> String {
    String::from(reason.clone())
}

/// The session's status as the wire payload. Pure: no clock, no I/O.
pub fn session_info(status: &SessionStatus, meta: &SessionMeta) -> RecordingSessionInfo {
    let peak = status
        .tracks
        .iter()
        .flat_map(|track| track.channel_peaks)
        .fold(0.0_f32, f32::max);
    let rate = u128::from(meta.track_rate.max(1));
    // Tracks run side by side, so the gaps of the track with the most silence
    // are the time the recording lost; adding the tracks would count it twice.
    let gap_ms = status
        .tracks
        .iter()
        .map(|track| (u128::from(track.gap_frames) * 1000 / rate) as u64)
        .max()
        .unwrap_or(0);
    RecordingSessionInfo {
        state: status.state.into(),
        recording_id: Some(meta.recording_id.clone()),
        folder_name: Some(meta.folder_name.clone()),
        title: Some(meta.title.clone()),
        quality: Some(meta.quality),
        elapsed_ms: u64::try_from(status.elapsed.as_millis()).unwrap_or(u64::MAX),
        level: peak.clamp(0.0, 1.0),
        clipped: peak >= CLIP_LEVEL,
        gap_count: status.tracks.iter().map(|track| track.gap_count).sum(),
        gap_ms,
        stalled: status.tracks.iter().any(|track| track.stalled),
        device_lost: status.tracks.iter().any(|track| track.device_lost),
        bytes_written: status.bytes_written,
        free_disk_bytes: status.free_disk_bytes,
        warnings: status
            .warnings
            .iter()
            .map(|warning| warning_id(warning).to_string())
            .collect(),
        ended_early: status
            .ended_early
            .as_ref()
            .filter(|reason| **reason != EndedReason::Stopped)
            .map(ended_reason_id),
        error_code: status.error_code.map(str::to_string),
    }
}

/// The microphone permission as the platform reports it
/// (`MicPermission::status`) -> whether a recording may be started.
/// `not_determined` has to be resolved by asking first; `unsupported` is a
/// build outside an app bundle, where the capture's own gate decides.
pub fn check_permission(status: &str) -> Result<(), RecordingError> {
    match status {
        "authorized" | "unsupported" => Ok(()),
        _ => Err(RecordingError::PermissionDenied {
            action: "open the microphone",
        }),
    }
}

/// The title a recording gets: the user's, trimmed, or [`DEFAULT_TITLE`].
pub fn title_or_default(title: Option<&str>) -> String {
    match title.map(str::trim) {
        Some(title) if !title.is_empty() => title.to_string(),
        _ => DEFAULT_TITLE.to_string(),
    }
}

/// The recordings folder and its volume. `requested` is a path the user is
/// considering (`None`: the configured folder).
pub fn folder_info(
    settings: &Settings,
    requested: Option<&str>,
    space: &dyn FreeSpace,
) -> RecordingFolderInfo {
    let path = match requested.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => std::path::PathBuf::from(expand_home(path)),
        None => settings.recordings_dir(),
    };
    RecordingFolderInfo {
        exists: path.is_dir(),
        free_bytes: space.available_bytes(&path).ok(),
        warnings: folder_warnings(&path)
            .into_iter()
            .map(|warning| folder_warning_id(warning).to_string())
            .collect(),
        is_default: path == default_recordings_dir(),
        path: path.to_string_lossy().into_owned(),
    }
}

/// [`folder_info`] on the real volume.
pub fn folder_info_on_disk(settings: &Settings, requested: Option<&str>) -> RecordingFolderInfo {
    folder_info(settings, requested, &VolumeFreeSpace)
}

/// The notes for a recovery scan of `root`. A folder that was complete already
/// needs nothing and gets no note; neither does `skip` (the folder of the
/// session that is recording now, which is locked by design).
pub fn recovery_notes(
    root: &Path,
    reports: Vec<RecoveryReport>,
    skip: Option<&str>,
) -> Vec<RecoveryNote> {
    reports
        .into_iter()
        .filter(|report| Some(report.folder.as_str()) != skip)
        .filter(|report| !matches!(report.outcome, RecoveryOutcome::SkippedNotRecording))
        .map(|report| recovery_note(root, report))
        .collect()
}

fn recovery_note(root: &Path, report: RecoveryReport) -> RecoveryNote {
    let RecoveryReport { folder, outcome } = report;
    let (title, outcome, frames_kept, tail_bytes, message) = match outcome {
        RecoveryOutcome::Recovered(recovered) => {
            let frames: u64 = recovered.tracks.iter().map(|track| track.frames_kept).sum();
            let tail: u64 = recovered.tracks.iter().map(|track| track.tail_bytes).sum();
            let message = if tail > 0 {
                "Recovered. Everything that was fully saved was kept; the unfinished end was set aside in the recording folder.".to_string()
            } else {
                "Recovered. Everything that was saved was kept.".to_string()
            };
            (
                recovered.manifest.title,
                RecoveryNoteOutcome::Recovered,
                frames,
                tail,
                message,
            )
        }
        RecoveryOutcome::SkippedNotRecording => (
            title_of(root, &folder),
            RecoveryNoteOutcome::AlreadyComplete,
            0,
            0,
            "Already complete; there was nothing to recover.".to_string(),
        ),
        RecoveryOutcome::SkippedLocked => (
            title_of(root, &folder),
            RecoveryNoteOutcome::SkippedLocked,
            0,
            0,
            "Another program is still recording this; it was left alone.".to_string(),
        ),
        RecoveryOutcome::Failed { code } => (
            title_of(root, &folder),
            RecoveryNoteOutcome::Failed,
            0,
            0,
            format!("Could not be recovered ({code}). The recording was left as it was; you can try again."),
        ),
    };
    RecoveryNote {
        folder_name: folder,
        title,
        outcome,
        frames_kept,
        tail_bytes,
        message,
    }
}

/// The recording's title, or its folder name when the manifest cannot be read.
fn title_of(root: &Path, folder: &str) -> String {
    read_recording(root, folder)
        .map(|(_, manifest)| manifest.title)
        .unwrap_or_else(|_| folder.to_string())
}

// -- Seams -------------------------------------------------------------------------

/// Opens the capture source of a session.
pub trait SourceFactory: Send + Sync {
    /// The microphone, or a stand-in: `channel_pair` is `recording_channel_pair`
    /// (1-based, `(n, n)` is mono channel `n`). Blocking.
    fn open(&self, channel_pair: (u16, u16)) -> Result<Box<dyn CaptureSource>, RecordingError>;
}

/// The default input device.
pub struct MicFactory;

impl SourceFactory for MicFactory {
    fn open(&self, channel_pair: (u16, u16)) -> Result<Box<dyn CaptureSource>, RecordingError> {
        Ok(Box::new(MicSource::open(MicConfig { channel_pair })?))
    }
}

// -- The managed state -------------------------------------------------------------

/// A session that was started: the handle the app keeps while it runs.
struct Active {
    session: Arc<RecordingSession>,
    meta: SessionMeta,
    /// Dropped with the session's entry: the event thread then sends the last
    /// status and ends.
    _events: Sender<()>,
}

/// What a finished session left behind.
#[derive(Clone)]
struct Last {
    info: RecordingSessionInfo,
    finished: Option<RecordingFinished>,
}

enum Phase {
    Idle,
    /// A start is in flight (the sources open on the blocking pool).
    Starting,
    Active(Active),
}

struct RecorderState {
    phase: Phase,
    last: Option<Last>,
    /// Recovery notes not yet handed to the window.
    notes: Vec<RecoveryNote>,
}

/// The recorder's managed state: see the module documentation.
pub struct SharedRecorder {
    state: Mutex<RecorderState>,
    factory: Arc<dyn SourceFactory>,
    trash: Arc<dyn RecordingTrash>,
}

impl Default for SharedRecorder {
    fn default() -> Self {
        Self::new(Arc::new(MicFactory), Arc::new(SystemTrash))
    }
}

/// What a started session hands back.
struct Started {
    session: Arc<RecordingSession>,
    meta: SessionMeta,
}

impl SharedRecorder {
    pub fn new(factory: Arc<dyn SourceFactory>, trash: Arc<dyn RecordingTrash>) -> Self {
        Self {
            state: Mutex::new(RecorderState {
                phase: Phase::Idle,
                last: None,
                notes: Vec::new(),
            }),
            factory,
            trash,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RecorderState> {
        lock_or_recover(&self.state, RECORDER_LOCK)
    }

    /// Whether a session is starting or running (not one that has ended and
    /// waits for `stop_recording_session` to collect it). For the tray and the
    /// dictation hotkey, which must not start while a session records.
    // Read by the tray indicator and the hotkey path, which the Record window
    // work adds; the tests read it now.
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        match &self.lock().phase {
            Phase::Idle => false,
            Phase::Starting => true,
            Phase::Active(active) => !active.session.state().is_terminal(),
        }
    }

    /// The folder name of the session that records now, if any.
    pub fn active_folder_name(&self) -> Option<String> {
        match &self.lock().phase {
            Phase::Active(active) => Some(active.meta.folder_name.clone()),
            _ => None,
        }
    }

    /// The current state: the running session, a start in flight, the last
    /// finished session, or idle.
    pub fn info(&self) -> RecordingSessionInfo {
        let state = self.lock();
        match &state.phase {
            Phase::Active(active) => session_info(&active.session.status(), &active.meta),
            Phase::Starting => RecordingSessionInfo::bare(RecordingSessionState::Starting),
            Phase::Idle => state
                .last
                .as_ref()
                .map_or_else(RecordingSessionInfo::idle, |last| last.info.clone()),
        }
    }

    /// Take the slot for a new session: `Busy` while one starts or records. A
    /// session that ended on its own is collected into the last result first.
    fn begin(&self) -> Result<(), CommandError> {
        let mut state = self.lock();
        let ended = match &state.phase {
            Phase::Idle => None,
            Phase::Starting => return Err(busy("A recording is starting.")),
            Phase::Active(active) if !active.session.state().is_terminal() => {
                return Err(busy("A recording is already in progress."));
            }
            // Over by itself (every track ended, or it failed) and never stopped.
            Phase::Active(active) => Some(session_info(&active.session.status(), &active.meta)),
        };
        if let Some(info) = ended {
            state.last = Some(Last {
                info,
                finished: None,
            });
        }
        state.phase = Phase::Starting;
        Ok(())
    }

    fn abort_start(&self) {
        let mut state = self.lock();
        if matches!(state.phase, Phase::Starting) {
            state.phase = Phase::Idle;
        }
    }

    fn finish_start(&self, active: Active) {
        self.lock().phase = Phase::Active(active);
    }

    /// The running session and what is known about it.
    fn current(&self) -> Result<(Arc<RecordingSession>, SessionMeta), CommandError> {
        match &self.lock().phase {
            Phase::Active(active) => Ok((Arc::clone(&active.session), active.meta.clone())),
            Phase::Starting => Err(busy("The recording is still starting.")),
            Phase::Idle => Err(RecordingError::NotRecording.into()),
        }
    }

    /// The last finished result, for a `stop` that finds nothing running.
    fn last_finished(&self) -> Option<RecordingFinished> {
        self.lock()
            .last
            .as_ref()
            .and_then(|last| last.finished.clone())
    }

    /// Record the end of `session` and free the slot, unless another session
    /// has taken it already.
    fn conclude(&self, session: &Arc<RecordingSession>, last: Last) {
        let mut state = self.lock();
        let same =
            matches!(&state.phase, Phase::Active(active) if Arc::ptr_eq(&active.session, session));
        if same {
            state.phase = Phase::Idle;
            state.last = Some(last);
        }
    }

    /// Keep recovery notes for the window's next `recover_recordings`.
    // Called by the startup recovery, which the Record window work adds.
    #[allow(dead_code)]
    pub fn remember_notes(&self, notes: Vec<RecoveryNote>) {
        self.lock().notes.extend(notes);
    }

    /// The remembered notes, handed over once.
    pub fn take_notes(&self) -> Vec<RecoveryNote> {
        std::mem::take(&mut self.lock().notes)
    }

    pub fn trash(&self) -> Arc<dyn RecordingTrash> {
        Arc::clone(&self.trash)
    }
}

fn busy(message: &str) -> CommandError {
    CommandError::Busy(message.to_string())
}

// -- Operations --------------------------------------------------------------------

/// Sends a status to whoever listens (the `recording-session-changed` event).
pub type Emit = Arc<dyn Fn(RecordingSessionInfo) + Send + Sync>;

fn client() -> Client {
    Client {
        kind: "desktop".into(),
        app_version: env!("CARGO_PKG_VERSION").into(),
        build: env!("GIT_HASH").into(),
        extra: Default::default(),
    }
}

/// RFC 3339 with the local offset and milliseconds.
fn started_at_now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
}

/// Open the source and start the session. Blocking (the session waits for the
/// first audio): call it from the blocking pool.
fn start_blocking(
    factory: &dyn SourceFactory,
    settings: &Settings,
    title: &str,
) -> Result<Started, RecordingError> {
    let source = factory.open(settings.recording_channel_pair_tuple())?;
    let config = SessionConfig::new(
        title,
        started_at_now(),
        client(),
        settings.recording_quality.to_quality(),
    );
    let session = Arc::new(RecordingSession::new(
        RecordingStore::new(settings.recordings_dir()),
        config,
    ));
    session.start(vec![SourceSpec {
        id: "mic".into(),
        role: "mic".into(),
        source,
    }])?;
    let summary = session
        .folder()
        .ok_or(RecordingError::SessionFailed {
            reason: "the session has no folder",
        })
        .and_then(|folder| summarize_folder(&folder));
    match summary {
        Ok(summary) => Ok(Started {
            meta: SessionMeta {
                title: summary.title,
                quality: settings.recording_quality,
                recording_id: summary.recording_id,
                folder_name: summary.folder_name,
                track_rate: summary.sample_rate,
            },
            session,
        }),
        Err(error) => {
            // Started but unreadable: finish what exists rather than leave it recording.
            let _ = session.stop();
            Err(error)
        }
    }
}

/// Start a recording session. The caller has checked that nothing else uses
/// the microphone and that the permission allows it (`check_permission`).
pub async fn start_session(
    recorder: &SharedRecorder,
    settings: Settings,
    title: Option<String>,
    emit: Emit,
) -> Result<RecordingSessionInfo, CommandError> {
    recorder.begin()?;
    let title = title_or_default(title.as_deref());
    let factory = Arc::clone(&recorder.factory);
    let started = tokio::task::spawn_blocking(move || start_blocking(&*factory, &settings, &title))
        .await
        .map_err(|error| CommandError::task_failed(error.to_string()));
    let started = match started {
        Ok(Ok(started)) => started,
        Ok(Err(error)) => {
            recorder.abort_start();
            return Err(error.into());
        }
        Err(error) => {
            recorder.abort_start();
            return Err(error);
        }
    };
    let info = session_info(&started.session.status(), &started.meta);
    let (nudge, wake) = mpsc::channel();
    spawn_event_thread(
        Arc::clone(&started.session),
        started.meta.clone(),
        EVENT_INTERVAL,
        wake,
        emit,
    );
    recorder.finish_start(Active {
        session: started.session,
        meta: started.meta,
        _events: nudge,
    });
    Ok(info)
}

/// The thread that sends the status: when it changes, every `interval`, and
/// once more when the session is over (done or failed) or its entry is dropped.
/// It reads the status snapshot, which never waits for the writer, and takes no
/// lock of the app.
fn spawn_event_thread(
    session: Arc<RecordingSession>,
    meta: SessionMeta,
    interval: Duration,
    wake: mpsc::Receiver<()>,
    emit: Emit,
) {
    let spawned = std::thread::Builder::new()
        .name("recording-events".into())
        .spawn(move || {
            let mut sent: Option<RecordingSessionInfo> = None;
            loop {
                let status = session.status();
                let over = status.state.is_terminal();
                let info = session_info(&status, &meta);
                if sent.as_ref() != Some(&info) {
                    emit(info.clone());
                    sent = Some(info);
                }
                if over {
                    return;
                }
                match wake.recv_timeout(interval) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => {
                        // The entry is gone (stopped): one last look, then done.
                        let info = session_info(&session.status(), &meta);
                        if sent.as_ref() != Some(&info) {
                            emit(info);
                        }
                        return;
                    }
                }
            }
        });
    if let Err(error) = spawned {
        tracing::error!("Could not start the recording event thread: {error}");
    }
}

/// Stop the running session and finish its recording. Safe to call twice: a
/// second call returns the result of the first.
pub async fn stop_session(recorder: &SharedRecorder) -> Result<RecordingFinished, CommandError> {
    let (session, meta) = match recorder.current() {
        Ok(current) => current,
        // Nothing runs: a second stop (a double click) gets the first one's result.
        Err(error @ CommandError::Recording(RecordingError::NotRecording)) => {
            return recorder.last_finished().ok_or(error);
        }
        Err(error) => return Err(error),
    };
    let stopping = Arc::clone(&session);
    let result = tokio::task::spawn_blocking(move || {
        let outcome = stopping.stop()?;
        let summary = summarize_folder(&outcome.folder)?;
        Ok::<_, RecordingError>(RecordingFinished {
            summary,
            ended_early: outcome
                .ended_early
                .as_ref()
                .filter(|reason| **reason != EndedReason::Stopped)
                .map(ended_reason_id),
        })
    })
    .await
    .map_err(|error| CommandError::task_failed(error.to_string()))?;
    let info = session_info(&session.status(), &meta);
    match result {
        Ok(finished) => {
            recorder.conclude(
                &session,
                Last {
                    info,
                    finished: Some(finished.clone()),
                },
            );
            Ok(finished)
        }
        Err(error) => {
            recorder.conclude(
                &session,
                Last {
                    info,
                    finished: None,
                },
            );
            Err(error.into())
        }
    }
}

/// Move the recording `folder_name` of `root` to the trash. Refuses the
/// recording that is being made now (`active`, its folder name). Blocking.
pub fn delete_recording_in(
    root: &Path,
    folder_name: &str,
    active: Option<&str>,
    trash: &dyn RecordingTrash,
) -> Result<(), CommandError> {
    let folder = recording_folder(root, folder_name)?;
    if active == Some(folder_name) {
        return Err(busy("That recording is still being made."));
    }
    trash.move_to_trash(&folder)
}

/// Recover every interrupted recording in `root` and describe what was done
/// (`skip`: the folder of the session that records now). Blocking. The app's
/// startup uses it too, and keeps the notes (`SharedRecorder::remember_notes`).
pub fn recover_pending(root: &Path, skip: Option<&str>) -> Result<Vec<RecoveryNote>, CommandError> {
    let reports = RecordingStore::new(root).recover_pending()?;
    Ok(recovery_notes(root, reports, skip))
}

#[cfg(test)]
#[path = "recording_session_tests.rs"]
mod tests;
