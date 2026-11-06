use std::io;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use sagascript_core::recording::capture::{SyntheticConfig, SyntheticSource};
use sagascript_core::recording::session::fsclass::FolderWarning;
use sagascript_core::recording::{RecoveredRecording, TrackRecovery, TrackStatus};
use sagascript_testsupport::TempDir;
use serde_json::{json, Value};

use super::*;
use crate::contract;

// -- Fixtures ----------------------------------------------------------------------

fn meta() -> SessionMeta {
    SessionMeta {
        title: "Interview".into(),
        quality: RecordingQuality::Lossless,
        recording_id: "0d2c1f3a-1111-4222-8333-444455556666".into(),
        folder_name: "2026-11-06 10.00 Interview [0d2c1f]".into(),
        track_rate: 48_000,
    }
}

fn track(peaks: [f32; 2]) -> TrackStatus {
    TrackStatus {
        id: "mic".into(),
        role: "mic".into(),
        peak: peaks[0].max(peaks[1]),
        channel_peaks: peaks,
        frames: 0,
        frames_received: 0,
        gap_count: 0,
        gap_frames: 0,
        dropped_frames: 0,
        stalled: false,
        device_lost: false,
        ended: None,
        queued_blocks: 0,
    }
}

fn status(state: SessionState, tracks: Vec<TrackStatus>) -> SessionStatus {
    SessionStatus {
        state,
        elapsed: Duration::from_millis(12_345),
        tracks,
        bytes_written: 4_096,
        free_disk_bytes: Some(50_000_000_000),
        warnings: Vec::new(),
        error_code: None,
        ended_early: None,
    }
}

// -- session_info ------------------------------------------------------------------

#[test]
fn the_level_is_the_loudest_channel_of_any_track() {
    let info = session_info(
        &status(
            SessionState::Recording,
            vec![track([0.2, 0.6]), track([0.4, 0.1])],
        ),
        &meta(),
    );
    assert_eq!(info.level, 0.6);
    assert!(!info.clipped);
    assert_eq!(info.state, RecordingSessionState::Recording);
    assert_eq!(info.elapsed_ms, 12_345);
    assert_eq!(info.bytes_written, 4_096);
    assert_eq!(info.free_disk_bytes, Some(50_000_000_000));
    assert_eq!(info.title.as_deref(), Some("Interview"));
    assert_eq!(info.quality, Some(RecordingQuality::Lossless));
}

#[test]
fn a_peak_at_full_scale_clips_and_the_level_stays_within_zero_and_one() {
    let loud = session_info(
        &status(SessionState::Recording, vec![track([1.0, 0.0])]),
        &meta(),
    );
    assert!(loud.clipped);
    assert_eq!(loud.level, 1.0);
    // A float device can exceed full scale.
    let over = session_info(
        &status(SessionState::Recording, vec![track([0.0, 1.7])]),
        &meta(),
    );
    assert!(over.clipped);
    assert_eq!(over.level, 1.0);
    let silent = session_info(&status(SessionState::Recording, vec![]), &meta());
    assert_eq!((silent.level, silent.clipped), (0.0, false));
}

#[test]
fn gaps_are_counted_and_timed_at_the_stored_rate() {
    let mut gapped = track([0.0, 0.0]);
    gapped.gap_count = 2;
    gapped.gap_frames = 72_000;
    let lossless = session_info(
        &status(SessionState::Recording, vec![gapped.clone()]),
        &meta(),
    );
    assert_eq!(lossless.gap_count, 2);
    assert_eq!(lossless.gap_ms, 1_500, "72000 frames at 48 kHz");

    let mut compact = meta();
    compact.quality = RecordingQuality::Compact;
    compact.track_rate = 16_000;
    let info = session_info(&status(SessionState::Recording, vec![gapped]), &compact);
    assert_eq!(info.gap_ms, 4_500, "the same frames at 16 kHz");
    assert_eq!(info.quality, Some(RecordingQuality::Compact));
}

#[test]
fn gaps_of_parallel_tracks_are_not_added_up() {
    let mut a = track([0.0, 0.0]);
    a.gap_count = 1;
    a.gap_frames = 48_000;
    let mut b = a.clone();
    b.gap_frames = 24_000;
    let info = session_info(&status(SessionState::Recording, vec![a, b]), &meta());
    assert_eq!(info.gap_count, 2);
    assert_eq!(info.gap_ms, 1_000);
}

#[test]
fn stalled_and_lost_devices_are_reported() {
    let mut stalled = track([0.0, 0.0]);
    stalled.stalled = true;
    let mut lost = track([0.0, 0.0]);
    lost.device_lost = true;
    let info = session_info(
        &status(SessionState::Recording, vec![stalled, track([0.0; 2])]),
        &meta(),
    );
    assert_eq!((info.stalled, info.device_lost), (true, false));
    let info = session_info(
        &status(SessionState::Recording, vec![lost, track([0.0; 2])]),
        &meta(),
    );
    assert_eq!((info.stalled, info.device_lost), (false, true));
}

#[test]
fn warnings_are_the_ids_the_window_knows() {
    let mut s = status(SessionState::Recording, vec![track([0.0; 2])]);
    s.warnings = vec![
        SessionWarning::Folder(FolderWarning::Fat32),
        SessionWarning::Folder(FolderWarning::ExFat),
        SessionWarning::Folder(FolderWarning::NetworkVolume),
        SessionWarning::Folder(FolderWarning::CloudSynced),
        SessionWarning::LowDisk,
    ];
    assert_eq!(
        session_info(&s, &meta()).warnings,
        [
            "fat32",
            "exfat",
            "network_volume",
            "cloud_synced",
            "low_disk"
        ]
    );
}

#[test]
fn an_early_end_is_named_and_a_user_stop_is_not_one() {
    let mut s = status(SessionState::Done, vec![track([0.0; 2])]);
    for (reason, id) in [
        (EndedReason::DeviceChanged, "device_changed"),
        (EndedReason::FormatChanged, "format_changed"),
        (EndedReason::SystemSleep, "system_sleep"),
        (EndedReason::DiskFull, "disk_full"),
        (EndedReason::Other("newer_reason".into()), "newer_reason"),
    ] {
        s.ended_early = Some(reason);
        assert_eq!(session_info(&s, &meta()).ended_early.as_deref(), Some(id));
    }
    s.ended_early = Some(EndedReason::Stopped);
    assert_eq!(session_info(&s, &meta()).ended_early, None);
    s.ended_early = None;
    assert_eq!(session_info(&s, &meta()).ended_early, None);
}

#[test]
fn a_failed_session_carries_its_error_code() {
    let mut s = status(SessionState::Failed, vec![]);
    s.error_code = Some("recording_disk_full");
    let info = session_info(&s, &meta());
    assert_eq!(info.state, RecordingSessionState::Failed);
    assert_eq!(info.error_code.as_deref(), Some("recording_disk_full"));
}

#[test]
fn every_session_state_has_its_wire_name() {
    for (state, name) in [
        (SessionState::Idle, "idle"),
        (SessionState::Starting, "starting"),
        (SessionState::Recording, "recording"),
        (SessionState::Finalizing, "finalizing"),
        (SessionState::Done, "done"),
        (SessionState::Failed, "failed"),
    ] {
        let info = session_info(&status(state, vec![]), &meta());
        assert_eq!(contract::to_value(info)["state"], name);
    }
}

// -- Small decisions ---------------------------------------------------------------

#[test]
fn only_an_authorized_microphone_may_record() {
    assert!(check_permission("authorized").is_ok());
    // Outside an app bundle the capture's own gate decides.
    assert!(check_permission("unsupported").is_ok());
    for status in ["denied", "restricted", "not_determined", ""] {
        let error = check_permission(status).expect_err(status);
        assert_eq!(error.code(), "recording_permission_denied", "{status}");
    }
}

#[test]
fn a_blank_title_becomes_the_default() {
    assert_eq!(title_or_default(None), "Recording");
    assert_eq!(title_or_default(Some("  \t")), "Recording");
    assert_eq!(title_or_default(Some("  Standup ")), "Standup");
}

struct FreeSpaceOf(Option<u64>);

impl FreeSpace for FreeSpaceOf {
    fn available_bytes(&self, _dir: &Path) -> io::Result<u64> {
        self.0
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }
}

#[test]
fn folder_info_describes_an_existing_folder_and_its_volume() {
    let dir = TempDir::new();
    let settings = Settings {
        recording_directory: dir.path().to_string_lossy().into_owned(),
        ..Settings::default()
    };
    let info = folder_info(&settings, None, &FreeSpaceOf(Some(7_000)));
    assert_eq!(info.path, dir.path().to_string_lossy());
    assert!(info.exists);
    assert_eq!(info.free_bytes, Some(7_000));
    assert!(!info.is_default);
    assert!(info.warnings.iter().all(|w| w != "low_disk"));
}

#[test]
fn folder_info_of_a_path_that_does_not_exist_yet() {
    let dir = TempDir::new();
    let wanted = dir.path().join("later");
    let info = folder_info(
        &Settings::default(),
        Some(&wanted.to_string_lossy()),
        &FreeSpaceOf(None),
    );
    assert!(!info.exists);
    assert_eq!(info.free_bytes, None);
    assert_eq!(info.path, wanted.to_string_lossy());
}

#[test]
fn folder_info_defaults_to_the_default_folder_and_says_so() {
    let info = folder_info(&Settings::default(), None, &FreeSpaceOf(Some(1)));
    assert!(info.is_default);
    assert_eq!(
        info.path,
        default_recordings_dir().to_string_lossy(),
        "an empty setting is the default folder"
    );
    // A blank request asks about the configured folder, too.
    let blank = folder_info(&Settings::default(), Some("  "), &FreeSpaceOf(Some(1)));
    assert_eq!(blank, info);
}

#[test]
fn folder_info_names_the_warnings_of_the_volume() {
    let synced = folder_info(
        &Settings::default(),
        Some(&PathBuf::from("/Users/x/Dropbox/Recordings").to_string_lossy()),
        &FreeSpaceOf(Some(1)),
    );
    assert!(synced.warnings.contains(&"cloud_synced".to_string()));
    assert!(!synced.is_default);
}

// -- Recovery notes ----------------------------------------------------------------

fn complete_manifest() -> sagascript_core::recording::Manifest {
    let text = include_str!("../crates/sagascript-core/tests/fixtures/recording/manifest-v1.json");
    sagascript_core::recording::parse_manifest(text.as_bytes()).expect("the golden manifest parses")
}

#[test]
fn recovery_notes_describe_every_outcome_and_skip_what_needs_nothing() {
    let root = TempDir::new();
    let manifest = complete_manifest();
    let reports = vec![
        RecoveryReport {
            folder: "a".into(),
            outcome: RecoveryOutcome::Recovered(Box::new(RecoveredRecording {
                manifest: manifest.clone(),
                tracks: vec![
                    TrackRecovery {
                        id: "mic".into(),
                        frames_kept: 1_000,
                        tail_bytes: 12,
                        tail_file: Some("mic.tail".into()),
                        gaps_kept: 0,
                        gaps_dropped: 0,
                        journal_lines_ignored: 0,
                    },
                    TrackRecovery {
                        id: "system".into(),
                        frames_kept: 500,
                        tail_bytes: 0,
                        tail_file: None,
                        gaps_kept: 0,
                        gaps_dropped: 0,
                        journal_lines_ignored: 0,
                    },
                ],
            })),
        },
        RecoveryReport {
            folder: "b".into(),
            outcome: RecoveryOutcome::SkippedLocked,
        },
        RecoveryReport {
            folder: "c".into(),
            outcome: RecoveryOutcome::SkippedNotRecording,
        },
        RecoveryReport {
            folder: "d".into(),
            outcome: RecoveryOutcome::Failed {
                code: "recording_corrupt_track",
            },
        },
        RecoveryReport {
            folder: "now".into(),
            outcome: RecoveryOutcome::SkippedLocked,
        },
    ];
    let notes = recovery_notes(root.path(), reports, Some("now"));
    let outcomes: Vec<_> = notes
        .iter()
        .map(|n| (n.folder_name.as_str(), n.outcome))
        .collect();
    assert_eq!(
        outcomes,
        [
            ("a", RecoveryNoteOutcome::Recovered),
            ("b", RecoveryNoteOutcome::SkippedLocked),
            ("d", RecoveryNoteOutcome::Failed),
        ],
        "a complete recording and the one recording now need no note"
    );
    assert_eq!(notes[0].title, manifest.title);
    assert_eq!((notes[0].frames_kept, notes[0].tail_bytes), (1_500, 12));
    assert!(
        notes[0].message.contains("set aside"),
        "{}",
        notes[0].message
    );
    // An unreadable manifest falls back to the folder name for the title.
    assert_eq!(notes[1].title, "b");
    assert!(notes[2].message.contains("recording_corrupt_track"));
    for note in &notes {
        assert!(!note.message.contains(&*root.path().to_string_lossy()));
    }
}

#[test]
fn the_wire_names_of_a_note_are_the_ones_the_window_reads() {
    let value = contract::to_value(RecoveryNote {
        folder_name: "f".into(),
        title: "t".into(),
        outcome: RecoveryNoteOutcome::AlreadyComplete,
        frames_kept: 1,
        tail_bytes: 2,
        message: "m".into(),
    });
    assert_eq!(
        value,
        json!({
            "folder_name": "f", "title": "t", "outcome": "already_complete",
            "frames_kept": 1, "tail_bytes": 2, "message": "m",
        })
    );
    for (outcome, name) in [
        (RecoveryNoteOutcome::Recovered, "recovered"),
        (RecoveryNoteOutcome::SkippedLocked, "skipped_locked"),
        (RecoveryNoteOutcome::Failed, "failed"),
    ] {
        assert_eq!(contract::to_value(outcome), Value::from(name));
    }
}

// -- The session over a synthetic source -------------------------------------------

/// Opens a [`SyntheticSource`] (no device) and remembers the channel pairs it
/// was asked for.
#[derive(Default)]
struct SyntheticFactory {
    pairs: Mutex<Vec<(u16, u16)>>,
    refuse: Option<RecordingError>,
}

impl SourceFactory for SyntheticFactory {
    fn open(&self, pair: (u16, u16)) -> Result<Box<dyn CaptureSource>, RecordingError> {
        self.pairs.lock().expect("lock").push(pair);
        match &self.refuse {
            Some(error) => Err(error.clone()),
            None => Ok(Box::new(SyntheticSource::new(SyntheticConfig::default()))),
        }
    }
}

#[derive(Default)]
struct FakeTrash(Mutex<Vec<PathBuf>>);

impl RecordingTrash for FakeTrash {
    fn move_to_trash(&self, folder: &Path) -> Result<(), CommandError> {
        self.0.lock().expect("lock").push(folder.to_path_buf());
        Ok(())
    }
}

struct Harness {
    recorder: SharedRecorder,
    factory: Arc<SyntheticFactory>,
    trash: Arc<FakeTrash>,
    dir: TempDir,
    events: Arc<Mutex<Vec<RecordingSessionInfo>>>,
}

impl Harness {
    fn new() -> Self {
        Self::with(SyntheticFactory::default())
    }

    fn with(factory: SyntheticFactory) -> Self {
        let factory = Arc::new(factory);
        let trash = Arc::new(FakeTrash::default());
        Self {
            recorder: SharedRecorder::new(factory.clone(), trash.clone()),
            factory,
            trash,
            dir: TempDir::new(),
            events: Arc::default(),
        }
    }

    fn settings(&self) -> Settings {
        Settings {
            recording_directory: self.dir.path().to_string_lossy().into_owned(),
            ..Settings::default()
        }
    }

    fn emit(&self) -> Emit {
        let events = Arc::clone(&self.events);
        Arc::new(move |info| events.lock().expect("lock").push(info))
    }

    async fn start(&self, title: Option<&str>) -> Result<RecordingSessionInfo, CommandError> {
        start_session(
            &self.recorder,
            self.settings(),
            title.map(str::to_string),
            self.emit(),
        )
        .await
    }

    fn events(&self) -> Vec<RecordingSessionInfo> {
        self.events.lock().expect("lock").clone()
    }
}

#[tokio::test]
async fn a_session_records_reports_and_stops_into_a_playable_recording() {
    let h = Harness::new();
    let started = h.start(Some("  Standup ")).await.expect("starts");
    assert_eq!(started.state, RecordingSessionState::Recording);
    assert_eq!(started.title.as_deref(), Some("Standup"));
    assert_eq!(started.quality, Some(RecordingQuality::Lossless));
    let folder_name = started.folder_name.clone().expect("a folder");
    assert!(h.dir.path().join(&folder_name).is_dir());
    assert!(h.recorder.is_active());
    assert_eq!(
        h.recorder.active_folder_name().as_deref(),
        Some(&*folder_name)
    );
    assert_eq!(
        *h.factory.pairs.lock().expect("lock"),
        [(1, 2)],
        "the configured channel pair reaches the source"
    );

    tokio::time::sleep(Duration::from_millis(700)).await;
    let live = h.recorder.info();
    assert_eq!(live.state, RecordingSessionState::Recording);
    assert!(live.elapsed_ms >= 300, "{live:?}");
    assert!(live.level > 0.1, "a sine is audible: {live:?}");
    assert!(live.bytes_written > 0);

    let finished = stop_session(&h.recorder).await.expect("stops");
    assert!(finished.summary.playable);
    assert_eq!(finished.summary.folder_name, folder_name);
    assert_eq!(finished.summary.title, "Standup");
    assert_eq!(finished.summary.track_count, 1);
    assert_eq!(finished.ended_early, None);
    assert!(finished.summary.duration_seconds > 0.2);
    assert!(!h.recorder.is_active());
    assert_eq!(h.recorder.active_folder_name(), None);

    let after = h.recorder.info();
    assert_eq!(after.state, RecordingSessionState::Done);
    assert_eq!(after.folder_name.as_deref(), Some(&*folder_name));

    // A second stop (a double click) returns the first one's result.
    assert_eq!(stop_session(&h.recorder).await.expect("again"), finished);
}

#[tokio::test]
async fn the_event_thread_reports_changes_every_tick_and_the_end() {
    let h = Harness::new();
    h.start(None).await.expect("starts");
    tokio::time::sleep(Duration::from_millis(900)).await;
    stop_session(&h.recorder).await.expect("stops");
    // The thread notices the dropped entry and sends the last status.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let events = h.events();
    assert!(events.len() >= 3, "{} events", events.len());
    assert_eq!(
        events.first().map(|e| e.state),
        Some(RecordingSessionState::Recording)
    );
    assert_eq!(
        events.last().map(|e| e.state),
        Some(RecordingSessionState::Done)
    );
    let elapsed: Vec<u64> = events.iter().map(|e| e.elapsed_ms).collect();
    assert!(
        elapsed.windows(2).all(|pair| pair[0] <= pair[1]),
        "{elapsed:?}"
    );
    let done = events
        .iter()
        .filter(|e| e.state == RecordingSessionState::Done)
        .count();
    assert_eq!(done, 1, "the end is sent once");
    // Nothing more arrives after the end.
    let count = events.len();
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(h.events().len(), count);
}

#[tokio::test]
async fn only_one_session_runs_at_a_time() {
    let h = Harness::new();
    h.start(None).await.expect("starts");
    let error = h.start(None).await.expect_err("busy");
    assert_eq!(error.code(), "busy");
    assert!(error.retryable());
    stop_session(&h.recorder).await.expect("stops");
    // The slot is free again.
    h.start(Some("Second")).await.expect("starts again");
    stop_session(&h.recorder).await.expect("stops again");
}

#[tokio::test]
async fn a_start_that_fails_frees_the_slot_and_keeps_the_code() {
    let h = Harness::with(SyntheticFactory {
        refuse: Some(RecordingError::SourceUnavailable {
            reason: "no input device",
        }),
        ..Default::default()
    });
    let error = h.start(None).await.expect_err("no device");
    assert_eq!(error.code(), "recording_source_unavailable");
    assert!(!h.recorder.is_active());
    assert_eq!(h.recorder.info().state, RecordingSessionState::Idle);
    assert!(h.events().is_empty(), "a failed start sends no event");
    let again = h.start(None).await.expect_err("still no device");
    assert_eq!(again.code(), "recording_source_unavailable", "not busy");
}

#[tokio::test]
async fn a_start_in_a_folder_that_cannot_be_made_is_a_typed_error() {
    let h = Harness::new();
    let file = h.dir.path().join("a-file");
    std::fs::write(&file, b"x").expect("write");
    let settings = Settings {
        recording_directory: file.join("inside").to_string_lossy().into_owned(),
        ..Settings::default()
    };
    let error = start_session(&h.recorder, settings, None, h.emit())
        .await
        .expect_err("the folder cannot exist");
    assert!(error.code().starts_with("recording_"), "{}", error.code());
    assert!(!error.message().contains(&*h.dir.path().to_string_lossy()));
    assert!(!h.recorder.is_active());
}

#[tokio::test]
async fn stopping_nothing_is_not_recording() {
    let h = Harness::new();
    let error = stop_session(&h.recorder).await.expect_err("nothing runs");
    assert_eq!(error.code(), "recording_not_recording");
}

#[tokio::test]
async fn the_state_before_any_session_is_idle() {
    let h = Harness::new();
    assert_eq!(h.recorder.info(), RecordingSessionInfo::idle());
    assert!(!h.recorder.is_active());
}

#[tokio::test]
async fn a_recording_left_unfinished_is_recovered_and_described() {
    let h = Harness::new();
    h.start(Some("Cut short")).await.expect("starts");
    tokio::time::sleep(Duration::from_millis(400)).await;
    let finished = stop_session(&h.recorder).await.expect("stops");
    // Make it look like a process died while recording.
    let manifest = h
        .dir
        .path()
        .join(&finished.summary.folder_name)
        .join("recording.json");
    let text = std::fs::read_to_string(&manifest).expect("read");
    assert!(text.contains("\"state\": \"complete\""), "{text}");
    std::fs::write(
        &manifest,
        text.replace("\"state\": \"complete\"", "\"state\": \"recording\""),
    )
    .expect("write");

    let notes = recover_pending(h.dir.path(), None).expect("recovers");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].outcome, RecoveryNoteOutcome::Recovered);
    assert_eq!(notes[0].title, "Cut short");
    assert!(notes[0].frames_kept > 0);
    // Done: a second pass has nothing to say.
    assert!(recover_pending(h.dir.path(), None)
        .expect("again")
        .is_empty());
}

#[tokio::test]
async fn notes_are_kept_for_the_window_and_handed_over_once() {
    let h = Harness::new();
    let note = |folder: &str| RecoveryNote {
        folder_name: folder.into(),
        title: folder.into(),
        outcome: RecoveryNoteOutcome::Recovered,
        frames_kept: 1,
        tail_bytes: 0,
        message: String::new(),
    };
    h.recorder.remember_notes(vec![note("a")]);
    h.recorder.remember_notes(vec![note("b")]);
    let taken = h.recorder.take_notes();
    assert_eq!(
        taken
            .iter()
            .map(|n| n.folder_name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert!(h.recorder.take_notes().is_empty());
}

// -- Deleting ----------------------------------------------------------------------

fn make_recording(root: &Path, name: &str) {
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).expect("folder");
    std::fs::write(folder.join("recording.json"), b"{}").expect("manifest");
}

#[test]
fn deleting_moves_the_folder_to_the_trash_and_nothing_else() {
    let root = TempDir::new();
    make_recording(root.path(), "one");
    let trash = FakeTrash::default();
    delete_recording_in(root.path(), "one", None, &trash).expect("moved");
    assert_eq!(*trash.0.lock().expect("lock"), [root.path().join("one")]);
    // The seam did not delete anything for good.
    assert!(root.path().join("one").is_dir());
}

#[test]
fn deleting_refuses_what_is_not_a_recording_of_this_folder() {
    let root = TempDir::new();
    std::fs::create_dir_all(root.path().join("photos")).expect("folder");
    let trash = FakeTrash::default();
    for (name, code) in [
        ("photos", "invalid_input"),
        ("..", "invalid_input"),
        ("a/b", "invalid_input"),
        ("gone", "not_found"),
    ] {
        let error = delete_recording_in(root.path(), name, None, &trash).expect_err(name);
        assert_eq!(error.code(), code, "{name}");
    }
    assert!(trash.0.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn the_recording_being_made_is_not_deleted() {
    let h = Harness::new();
    let started = h.start(None).await.expect("starts");
    let name = started.folder_name.expect("folder");
    let active = h.recorder.active_folder_name();
    let error = delete_recording_in(h.dir.path(), &name, active.as_deref(), h.trash.as_ref())
        .expect_err("still recording");
    assert_eq!(error.code(), "busy");
    assert!(h.trash.0.lock().expect("lock").is_empty());
    stop_session(&h.recorder).await.expect("stops");
    // Finished: it may go to the trash.
    delete_recording_in(h.dir.path(), &name, None, h.trash.as_ref()).expect("moved");
}
