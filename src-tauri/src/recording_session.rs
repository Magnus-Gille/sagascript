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
//! `summarize_folder`, recovery) all run outside it. Nothing takes
//! `SharedController` while holding it, so it may be taken **inside** the
//! controller lock (order: controller, then `app.recorder`).
//!
//! # One owner of the microphone
//!
//! Dictation and the recorder share the microphone. Both starts are decided
//! under the controller lock, which makes the decision atomic: the recorder
//! reserves its slot in [`reserve_for_recording`] (controller not busy, slot
//! free, then `Phase::Starting`) *before* it waits for the microphone
//! permission, and every dictation start takes the controller through
//! [`lock_controller_for_dictation`], which refuses while the slot is
//! reserved or live. The [`Reservation`] gives the slot back on every error
//! path and when the command is dropped before the blocking start is spawned;
//! after that the blocking start supervises the slot itself ([`start_reserved`]).

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sagascript_core::recording::capture::{CaptureSource, MicConfig, MicSource};
use sagascript_core::recording::session::disk::{FreeSpace, VolumeFreeSpace};
use sagascript_core::recording::session::fsclass::{folder_warnings, FolderWarning};
use sagascript_core::recording::{
    default_recordings_dir, lock_recording_for_removal, read_recording, summarize_folder, Client,
    EndedReason, RecordingError, RecordingSession, RecordingStore, RecordingSummary,
    RecoveryOutcome, RecoveryReport, SessionConfig, SessionState, SessionStatus, SessionWarning,
    SourceSpec,
};
use sagascript_core::settings::{expand_home, RecordingQuality, Settings};
use sagascript_core::sync::lock_or_recover;
use serde::Serialize;

use crate::command_error::CommandError;
use crate::commands::{lock_controller, SharedController};
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

/// Whether a session in `state` holds the microphone, so that the tray shows
/// the recording indicator: from the start until it is over. The same
/// question `SharedRecorder::is_active` answers from the recorder.
pub fn state_is_live(state: RecordingSessionState) -> bool {
    matches!(
        state,
        RecordingSessionState::Starting
            | RecordingSessionState::Recording
            | RecordingSessionState::Finalizing
    )
}

/// Dictation and the recorder share the microphone, so dictation does not
/// start while the recorder is active. `Busy`, as when dictation is busy.
pub fn ensure_recorder_idle(recorder_active: bool) -> Result<(), CommandError> {
    if recorder_active {
        return Err(busy(
            "Cannot start dictation while Sagascript is recording audio. Stop the recording first.",
        ));
    }
    Ok(())
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
    state: Arc<Mutex<RecorderState>>,
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
            state: Arc::new(Mutex::new(RecorderState {
                phase: Phase::Idle,
                last: None,
                notes: Vec::new(),
            })),
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
        abort_start_in(&self.state);
    }

    /// A handle on the slot that outlives this borrow, for the supervisor of a
    /// blocking start (see [`start_reserved`]).
    fn slot(&self) -> Slot {
        Slot(Arc::clone(&self.state))
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

fn abort_start_in(state: &Mutex<RecorderState>) {
    let mut state = lock_or_recover(state, RECORDER_LOCK);
    if matches!(state.phase, Phase::Starting) {
        state.phase = Phase::Idle;
    }
}

/// The recorder's slot as the supervisor of a blocking start holds it.
#[derive(Clone)]
struct Slot(Arc<Mutex<RecorderState>>);

impl Slot {
    fn finish_start(&self, active: Active) {
        lock_or_recover(&self.0, RECORDER_LOCK).phase = Phase::Active(active);
    }

    /// Give the slot back if the start never became a session (an error, a
    /// panic, a cancelled start whose session was stopped).
    fn release(&self) {
        abort_start_in(&self.0);
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

/// The recorder's slot, reserved for a session that is about to start. It
/// keeps dictation out while the microphone permission is asked for and the
/// source opens, and gives the slot back when dropped, on every error path and
/// when the command is cancelled. [`start_reserved`] consumes it.
#[must_use = "a reservation that is dropped frees the slot at once"]
pub struct Reservation<'a> {
    recorder: &'a SharedRecorder,
    armed: bool,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.recorder.abort_start();
        }
    }
}

impl SharedRecorder {
    /// Take the slot for a new session; see [`Reservation`].
    fn reserve(&self) -> Result<Reservation<'_>, CommandError> {
        self.begin()?;
        Ok(Reservation {
            recorder: self,
            armed: true,
        })
    }
}

/// Reserve the microphone for a recording: the dictation controller must not
/// be busy and the recorder's slot must be free, decided in one step under the
/// controller lock (no dictation start can slip in between). The settings the
/// session uses are copied under the same lock.
pub fn reserve_for_recording<'a>(
    controller: &SharedController,
    recorder: &'a SharedRecorder,
) -> Result<(Reservation<'a>, Settings), CommandError> {
    let ctrl = lock_controller(controller);
    if ctrl.state().is_busy() {
        return Err(busy(
            "Cannot start a recording while Sagascript is dictating or transcribing. Wait for it to finish.",
        ));
    }
    let reservation = recorder.reserve()?;
    Ok((reservation, ctrl.settings().clone()))
}

/// Lock the controller for a dictation start (hotkey, the start commands),
/// refusing while the recorder holds the microphone. The check is made under
/// the controller lock the start itself runs under, so a recording cannot be
/// reserved between the check and the start.
pub fn lock_controller_for_dictation<'a>(
    controller: &'a SharedController,
    recorder: &SharedRecorder,
) -> Result<std::sync::MutexGuard<'a, crate::app_controller::AppController>, CommandError> {
    let ctrl = lock_controller(controller);
    ensure_recorder_idle(recorder.is_active())?;
    Ok(ctrl)
}

/// Reserve the slot and start a session, for the tests of the session itself;
/// the command reserves first ([`reserve_for_recording`]) and starts later.
#[cfg(test)]
pub async fn start_session(
    recorder: &SharedRecorder,
    settings: Settings,
    title: Option<String>,
    emit: Emit,
) -> Result<RecordingSessionInfo, CommandError> {
    start_reserved(recorder.reserve()?, settings, title, emit).await
}

/// Where the session a blocking start produced stands between the worker that
/// started it and the command that asked for it.
enum Handoff {
    /// The worker has not decided yet.
    Waiting,
    /// The worker started a session and offers it; nobody has taken it.
    Offered(Started),
    /// Taken by the command, or the command went away first.
    Closed,
}

type HandoffCell = Arc<Mutex<Handoff>>;

fn lock_handoff(cell: &HandoffCell) -> std::sync::MutexGuard<'_, Handoff> {
    lock_or_recover(cell, "app.recorder.handoff")
}

/// Stop a session nobody owns and give the slot back, off the async thread
/// (stopping waits for the writer).
fn discard_session(started: Started, slot: Slot) {
    let cleanup = move || {
        let _ = started.session.stop();
        slot.release();
    };
    // A thread that cannot be spawned still must not leave the microphone held.
    let cleanup = std::sync::Arc::new(Mutex::new(Some(cleanup)));
    let again = Arc::clone(&cleanup);
    let spawned = std::thread::Builder::new()
        .name("recording-discard".into())
        .spawn(move || {
            if let Some(cleanup) = lock_or_recover(&cleanup, "app.recorder.discard").take() {
                cleanup();
            }
        });
    if spawned.is_err() {
        if let Some(cleanup) = lock_or_recover(&again, "app.recorder.discard").take() {
            cleanup();
        }
    }
}

/// Held by the command's future while it waits. Dropped before it took the
/// session (the command was cancelled), it closes the handoff: a session the
/// worker has offered but nobody took is stopped, and one the worker has not
/// finished starting is stopped by the worker when it sees the handoff closed.
struct CancelOnDrop {
    cell: HandoffCell,
    slot: Slot,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let previous = std::mem::replace(&mut *lock_handoff(&self.cell), Handoff::Closed);
        if let Handoff::Offered(started) = previous {
            discard_session(started, self.slot.clone());
        }
    }
}

/// Start the session in a slot reserved by [`reserve_for_recording`].
///
/// From the moment the blocking start is spawned, **it** owns the slot, not
/// this future: a command that is cancelled meanwhile (the future dropped)
/// leaves the start running, and the start must not hand the microphone to a
/// dictation before it is over. The worker never installs a session itself. It
/// *offers* it through a [`Handoff`]; the command takes it in its last,
/// synchronous step and installs it, so a session is installed exactly when a
/// caller received ownership. A session nobody took (the command was dropped
/// before the worker decided, or between the offer and the delivery) is stopped
/// before the slot is given back, on every path including a panic.
pub async fn start_reserved(
    mut reservation: Reservation<'_>,
    settings: Settings,
    title: Option<String>,
    emit: Emit,
) -> Result<RecordingSessionInfo, CommandError> {
    let recorder = reservation.recorder;
    let title = title_or_default(title.as_deref());
    let factory = Arc::clone(&recorder.factory);
    let slot = recorder.slot();
    let cell: HandoffCell = Arc::new(Mutex::new(Handoff::Waiting));
    let worker_cell = Arc::clone(&cell);
    let worker_slot = slot.clone();
    let task = tokio::task::spawn_blocking(move || {
        /// Frees the slot unless the session was offered.
        struct Release<'a>(&'a Slot, bool);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                if self.1 {
                    self.0.release();
                }
            }
        }
        let mut release = Release(&worker_slot, true);
        let started = start_blocking(&*factory, &settings, &title)?;
        let rejected = {
            let mut handoff = lock_handoff(&worker_cell);
            if matches!(*handoff, Handoff::Waiting) {
                *handoff = Handoff::Offered(started);
                None
            } else {
                Some(started)
            }
        };
        if let Some(started) = rejected {
            // The command is gone: finish what was started before the
            // microphone is given back (the guard frees the slot).
            let _ = started.session.stop();
            return Err(CommandError::task_failed(
                "The recording start was cancelled.".to_string(),
            ));
        }
        // Offered: whoever closes the handoff now owns the session and the slot.
        release.1 = false;
        Ok::<_, CommandError>(())
    });
    // From here the blocking task owns the slot; this future's drop only
    // closes the handoff.
    reservation.armed = false;
    let cancel = CancelOnDrop {
        cell: Arc::clone(&cell),
        slot: slot.clone(),
    };
    task.await
        .map_err(|error| CommandError::task_failed(error.to_string()))??;
    // The last step is synchronous: the session is taken and installed with no
    // `.await` in between, so the command cannot be dropped halfway.
    let started = match std::mem::replace(&mut *lock_handoff(&cell), Handoff::Closed) {
        Handoff::Offered(started) => started,
        _ => {
            slot.release();
            return Err(CommandError::task_failed(
                "The recording start was cancelled.".to_string(),
            ));
        }
    };
    drop(cancel);
    let info = session_info(&started.session.status(), &started.meta);
    let (nudge, wake) = mpsc::channel();
    spawn_event_thread(
        Arc::clone(&started.session),
        started.meta.clone(),
        EVENT_INTERVAL,
        wake,
        emit,
    );
    slot.finish_start(Active {
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
/// recording that is being made now (`active`, its folder name) and any
/// recording whose `recording.lock` another process holds or cannot be
/// established. Blocking.
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
    // The lock decides, not this instance's own bookkeeping: the CLI or a
    // second app instance may be writing the recording. It is held for the
    // whole move, which keeps recovery and a second remover out, and any
    // failure to establish ownership refuses the move.
    let _removal = lock_recording_for_removal(root, folder_name)?;
    trash.move_to_trash(&folder)
}

/// Recover every interrupted recording in `root` and describe what was done
/// (`skip`: the folder of the session that records now). Blocking. The app's
/// startup uses it too, and keeps the notes (`SharedRecorder::remember_notes`).
pub fn recover_pending(root: &Path, skip: Option<&str>) -> Result<Vec<RecoveryNote>, CommandError> {
    let reports = RecordingStore::new(root).recover_pending()?;
    Ok(recovery_notes(root, reports, skip))
}

/// The recovery at app start: recover the interrupted recordings of `root`
/// (the configured recordings folder) and describe what was done. A folder that
/// does not exist is nothing to recover and is not created. A failure is
/// logged (no path) and gives no notes: the Record window's own recovery
/// reports it when the user asks. Blocking.
pub fn recover_at_startup(root: &Path) -> Vec<RecoveryNote> {
    match recover_pending(root, None) {
        Ok(notes) => notes,
        Err(error) => {
            tracing::warn!(
                "Recovering interrupted recordings at start failed ({})",
                error.code()
            );
            Vec::new()
        }
    }
}

#[cfg(test)]
#[path = "recording_session_tests.rs"]
mod tests;
