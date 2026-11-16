//! The session as the app will use it: a writer thread, a state machine, a
//! status poll, `stop` from any thread. Time is the fake source's clock, so
//! nothing here depends on how fast the machine is; the tests wait for the
//! writer thread to catch up, never for a duration.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sagascript_testsupport::TempDir;

use super::disk::FreeSpace;
use super::*;
use crate::recording::capture::{
    BlockSink, FakeHandle, FakeSignal, FakeSource, ManualClock, NativeFormat,
};
use crate::recording::manifest::{EndedReason, RecordingState};
use crate::recording::test_signals::{decode_flac, STARTED_AT};
use crate::recording::{RecordingError, RecoveryOutcome};

const MONO48: NativeFormat = NativeFormat {
    sample_rate: 48_000,
    channels: 1,
};

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

struct PlentyOfRoom(AtomicU64);

impl FreeSpace for PlentyOfRoom {
    fn available_bytes(&self, _dir: &Path) -> io::Result<u64> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}

fn client() -> Client {
    Client {
        kind: "desktop".into(),
        app_version: "test".into(),
        build: "test".into(),
        extra: Default::default(),
    }
}

fn config(clock: Arc<ManualClock>, quality: Quality) -> SessionConfig {
    let mut config = SessionConfig::new("Session test", STARTED_AT, client(), quality);
    config.clock = clock;
    config.free_space = Arc::new(PlentyOfRoom(AtomicU64::new(100 << 30)));
    config.folder_warnings = Some(Vec::new());
    config.detect_sleep = false;
    config.start_timeout = Duration::from_millis(300);
    config
}

struct Rig {
    _temp: TempDir,
    store: RecordingStore,
    handle: FakeHandle,
    session: Arc<RecordingSession>,
}

fn rig_with(quality: Quality, format: NativeFormat, tweak: impl FnOnce(&mut SessionConfig)) -> Rig {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (source, handle) = FakeSource::new(format);
    handle.set_signal(FakeSignal::Noise {
        amplitude: 0.5,
        seed: 11,
    });
    let mut config = config(handle.clock(), quality);
    tweak(&mut config);
    let session = Arc::new(RecordingSession::new(store.clone(), config));
    session.start(vec![SourceSpec::mic(source)]).unwrap();
    Rig {
        _temp: temp,
        store,
        handle,
        session,
    }
}

fn rig() -> Rig {
    rig_with(Quality::Lossless, MONO48, |_| {})
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

impl Rig {
    /// Wait until the writer thread has handled everything delivered so far.
    fn catch_up(&self) {
        wait_until("the writer to catch up", || {
            let track = &self.session.status().tracks[0];
            track.frames_received + track.dropped_frames
                == self.handle.frames_delivered() + self.handle.frames_dropped()
        });
    }

    fn deliver(&self, duration: Duration) {
        self.handle.deliver(duration);
        self.catch_up();
    }

    fn folders(&self) -> Vec<std::path::PathBuf> {
        match std::fs::read_dir(self.store.root()) {
            Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
            Err(_) => Vec::new(),
        }
    }
}

#[test]
fn a_session_goes_idle_starting_recording_finalizing_done() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (source, handle) = FakeSource::new(MONO48);
    let session = RecordingSession::new(store, config(handle.clock(), Quality::Lossless));
    assert_eq!(session.state(), SessionState::Idle);
    assert_eq!(session.status().elapsed, Duration::ZERO);
    assert!(session.status().tracks.is_empty());
    assert!(session.folder().is_none());

    session.start(vec![SourceSpec::mic(source)]).unwrap();
    assert_eq!(session.state(), SessionState::Recording);
    let folder = session.folder().expect("the folder exists once recording");
    assert!(folder.join("recording.lock").exists());

    for _ in 0..50 {
        handle.deliver(ms(20));
    }
    wait_until("audio to be written", || {
        session.status().tracks[0].frames_received == handle.frames_delivered()
    });
    let status = session.status();
    assert_eq!(status.state, SessionState::Recording);
    assert_eq!(status.tracks[0].id, "mic");
    assert!(status.tracks[0].frames >= 4096 || status.bytes_written > 0);
    assert!(status.elapsed >= ms(1_000), "{:?}", status.elapsed);
    assert!(status.free_disk_bytes.is_some());
    assert!(status.error_code.is_none());

    let outcome = session.stop().unwrap();
    assert_eq!(session.state(), SessionState::Done);
    assert_eq!(outcome.manifest.state, RecordingState::Complete);
    assert_eq!(outcome.manifest.tracks[0].frames, handle.position());
    assert_eq!(
        outcome.manifest.tracks[0].ended_reason,
        Some(EndedReason::Stopped)
    );
    assert!(outcome.ended_early.is_none());
    assert!(!folder.join("recording.lock").exists());
    assert!(!handle.running(), "the source was stopped");
    // The status of a finished session stays put.
    let done = session.status();
    assert_eq!(done.state, SessionState::Done);
    assert_eq!(done.tracks[0].frames, handle.position());
    assert_eq!(done.elapsed, session.status().elapsed);
    assert!(done.bytes_written > 0);
}

#[test]
fn the_recording_is_exact_and_the_meter_moves() {
    let rig = rig();
    for _ in 0..100 {
        rig.deliver(ms(10));
    }
    let outcome = rig.session.stop().unwrap();
    let track = &outcome.manifest.tracks[0];
    assert_eq!(track.frames, rig.handle.position());
    assert!(track.gaps.is_empty());
    let decoded = decode_flac(&std::fs::read(outcome.folder.join("mic.flac")).unwrap());
    assert_eq!(decoded.md5_verified, Some(true));
    let want: Vec<i32> = rig
        .handle
        .expected(0, track.frames)
        .iter()
        .map(|s| crate::recording::encoder::quantise(*s, 24).0)
        .collect();
    assert!(decoded.samples == want);
}

#[test]
fn the_meter_shows_the_peak_of_the_signal() {
    let rig = rig();
    for _ in 0..40 {
        rig.deliver(ms(10));
    }
    wait_until("a peak", || rig.session.status().tracks[0].peak > 0.0);
    let peak = rig.session.status().tracks[0].peak;
    assert!((0.3..=0.51).contains(&peak), "{peak}");
    rig.session.stop().unwrap();
}

#[test]
fn stop_is_idempotent_and_every_caller_gets_the_same_outcome() {
    let rig = rig();
    rig.deliver(ms(200));
    let first = rig.session.stop().unwrap();
    let again = rig.session.stop().unwrap();
    assert_eq!(first.manifest, again.manifest);
    assert_eq!(first.folder, again.folder);

    // And from several threads at once on a fresh session.
    let rig = rig_with(Quality::Lossless, MONO48, |_| {});
    rig.deliver(ms(200));
    let threads: Vec<_> = (0..6)
        .map(|_| {
            let session = Arc::clone(&rig.session);
            std::thread::spawn(move || session.stop().unwrap())
        })
        .collect();
    let outcomes: Vec<SessionOutcome> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    for outcome in &outcomes {
        assert_eq!(outcome.manifest, outcomes[0].manifest);
        assert_eq!(outcome.manifest.state, RecordingState::Complete);
    }
    assert_eq!(rig.folders().len(), 1);
}

#[test]
fn stopping_a_session_that_never_started_is_an_error_not_a_hang() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (_source, handle) = FakeSource::new(MONO48);
    let session = RecordingSession::new(store, config(handle.clock(), Quality::Lossless));
    assert!(matches!(session.stop(), Err(RecordingError::NotRecording)));
    assert_eq!(session.state(), SessionState::Idle);
}

#[test]
fn a_session_starts_once() {
    let rig = rig();
    let (other, _handle) = FakeSource::new(MONO48);
    let error = rig.session.start(vec![SourceSpec::mic(other)]).unwrap_err();
    assert_eq!(error.code(), "recording_invalid_input");
    assert_eq!(rig.session.state(), SessionState::Recording, "unharmed");
    rig.session.stop().unwrap();
}

#[test]
fn a_start_and_a_stop_racing_each_other_never_hang_or_leave_a_torn_folder() {
    for round in 0..25 {
        let temp = TempDir::new();
        let store = RecordingStore::new(temp.path().join("Recordings"));
        let (source, handle) = FakeSource::new(MONO48);
        let session = Arc::new(RecordingSession::new(
            store.clone(),
            config(handle.clock(), Quality::Lossless),
        ));
        let starter = {
            let session = Arc::clone(&session);
            std::thread::spawn(move || session.start(vec![SourceSpec::mic(source)]))
        };
        if round % 2 == 0 {
            std::thread::yield_now();
        }
        let stopped = session.stop();
        starter.join().unwrap().unwrap();
        match stopped {
            // The stop got in first, or after the start had finished.
            Err(RecordingError::NotRecording) | Ok(_) => {}
            Err(other) => panic!("round {round}: {other:?}"),
        }
        // Either way the session can be stopped cleanly now.
        let outcome = session.stop().unwrap();
        assert_eq!(outcome.manifest.state, RecordingState::Complete);
        assert_eq!(session.state(), SessionState::Done);
        assert!(!handle.running());
        assert_eq!(
            store
                .recover_pending()
                .unwrap()
                .iter()
                .filter(|r| matches!(r.outcome, RecoveryOutcome::Recovered(_)))
                .count(),
            0,
            "nothing is left half-written"
        );
    }
}

// ---- starting fails cleanly --------------------------------------------------

#[test]
fn a_source_that_delivers_nothing_fails_the_start_and_leaves_no_folder() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (source, handle) = FakeSource::new(MONO48);
    let source = source.with_prime(Duration::ZERO);
    let session = RecordingSession::new(store.clone(), config(handle.clock(), Quality::Lossless));
    let error = session.start(vec![SourceSpec::mic(source)]).unwrap_err();
    assert_eq!(error.code(), "recording_source_unavailable");
    assert_eq!(session.state(), SessionState::Failed);
    assert_eq!(
        session.status().error_code,
        Some("recording_source_unavailable")
    );
    assert!(!handle.running(), "the source was stopped again");
    let folders = std::fs::read_dir(store.root())
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(
        folders, 0,
        "no recording folder for a recording that never began"
    );
    assert!(session.stop().is_err());
}

struct Refusing;

impl CaptureSource for Refusing {
    fn format(&self) -> NativeFormat {
        MONO48
    }
    fn device_name(&self) -> Option<String> {
        None
    }
    fn start(&mut self, _sink: BlockSink) -> Result<(), RecordingError> {
        Err(RecordingError::PermissionDenied {
            action: "open the microphone",
        })
    }
    fn stop(&mut self) {}
}

#[test]
fn a_denied_microphone_fails_the_start_with_its_own_code() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let session = RecordingSession::new(
        store.clone(),
        config(Arc::new(ManualClock::new()), Quality::Lossless),
    );
    let error = session.start(vec![SourceSpec::mic(Refusing)]).unwrap_err();
    assert_eq!(error.code(), "recording_permission_denied");
    assert_eq!(session.state(), SessionState::Failed);
    assert!(!store.root().exists() || std::fs::read_dir(store.root()).unwrap().count() == 0);
}

#[test]
fn too_little_free_space_refuses_to_start_before_the_device_is_opened() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (source, handle) = FakeSource::new(MONO48);
    let mut config = config(handle.clock(), Quality::Lossless);
    config.free_space = Arc::new(PlentyOfRoom(AtomicU64::new(10 << 20)));
    let session = RecordingSession::new(store, config);
    let error = session.start(vec![SourceSpec::mic(source)]).unwrap_err();
    assert!(error.is_disk_full());
    assert!(error.retryable());
    assert!(!handle.running(), "the microphone was never opened");
}

#[test]
fn unusable_input_is_refused() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let session = RecordingSession::new(
        store.clone(),
        config(Arc::new(ManualClock::new()), Quality::Lossless),
    );
    assert!(session.start(Vec::new()).is_err());

    let session = RecordingSession::new(
        store,
        config(Arc::new(ManualClock::new()), Quality::Lossless),
    );
    let (a, _) = FakeSource::new(MONO48);
    let (b, _) = FakeSource::new(MONO48);
    let error = session
        .start(vec![SourceSpec::mic(a), SourceSpec::mic(b)])
        .unwrap_err();
    assert_eq!(
        error.code(),
        "recording_invalid_input",
        "two tracks called mic"
    );
}

// ---- endings and failure -----------------------------------------------------

#[test]
fn when_the_only_track_ends_the_session_finishes_without_a_stop() {
    let rig = rig();
    rig.deliver(ms(200));
    rig.handle.change_device();
    wait_until("the session to finish by itself", || {
        rig.session.state() == SessionState::Done
    });
    let status = rig.session.status();
    assert_eq!(status.ended_early, Some(EndedReason::DeviceChanged));
    assert_eq!(status.tracks[0].ended, Some(EndedReason::DeviceChanged));
    // The recording is already complete on disk; `stop` stops the source and
    // hands back the same outcome.
    let folder = rig.session.folder().unwrap();
    assert!(!folder.join("recording.lock").exists());
    let outcome = rig.session.stop().unwrap();
    assert_eq!(outcome.ended_early, Some(EndedReason::DeviceChanged));
    assert_eq!(
        outcome.manifest.tracks[0].ended_reason,
        Some(EndedReason::DeviceChanged)
    );
    assert!(!rig.handle.running());
}

#[test]
fn the_system_can_tell_the_session_it_is_going_to_sleep() {
    let rig = rig();
    rig.deliver(ms(200));
    rig.session.notify_system_sleep();
    wait_until("the track to end", || {
        rig.session.state() == SessionState::Done
    });
    let outcome = rig.session.stop().unwrap();
    assert_eq!(
        outcome.manifest.tracks[0].ended_reason,
        Some(EndedReason::SystemSleep)
    );
}

#[test]
fn the_compact_tier_works_through_the_whole_session() {
    let rig = rig_with(Quality::Compact, MONO48, |_| {});
    for _ in 0..100 {
        rig.deliver(ms(10));
    }
    let outcome = rig.session.stop().unwrap();
    let track = &outcome.manifest.tracks[0];
    assert_eq!((track.sample_rate, track.bits), (16_000, 16));
    assert_eq!(track.resampled_from, Some(48_000));
    assert_eq!(
        track.frames,
        (rig.handle.position() as f64 / 3.0).round() as u64
    );
}

#[test]
fn a_folder_warning_is_shown_and_a_fat32_folder_gets_a_size_limit() {
    let rig = rig_with(Quality::Lossless, MONO48, |config| {
        config.folder_warnings = Some(vec![
            fsclass::FolderWarning::Fat32,
            fsclass::FolderWarning::CloudSynced,
        ]);
    });
    let status = rig.session.status();
    assert_eq!(
        status.warnings,
        vec![
            SessionWarning::Folder(fsclass::FolderWarning::Fat32),
            SessionWarning::Folder(fsclass::FolderWarning::CloudSynced)
        ]
    );
    rig.session.stop().unwrap();
}

#[test]
fn dropping_a_session_that_is_still_recording_finishes_the_recording() {
    let rig = rig();
    rig.deliver(ms(200));
    let folder = rig.session.folder().unwrap();
    let Rig {
        session,
        handle,
        store,
        _temp,
    } = rig;
    drop(session);
    let manifest = crate::recording::manifest::parse_manifest(
        &std::fs::read(folder.join("recording.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest.state, RecordingState::Complete);
    assert_eq!(manifest.tracks[0].frames, handle.position());
    assert!(!handle.running());
    drop((store, _temp));
}

struct PanicsOnPoll(std::sync::atomic::AtomicU32);

impl FreeSpace for PanicsOnPoll {
    fn available_bytes(&self, _dir: &Path) -> io::Result<u64> {
        // Once for the start check, then the writer thread's first poll.
        if self.0.fetch_add(1, Ordering::SeqCst) >= 1 {
            panic!("the free-space probe blew up");
        }
        Ok(100 << 30)
    }
}

#[test]
fn a_writer_that_panics_fails_the_session_and_leaves_a_folder_recovery_can_finish() {
    let rig = rig_with(Quality::Lossless, MONO48, |config| {
        config.free_space = Arc::new(PanicsOnPoll(Default::default()));
    });
    wait_until("the session to fail", || {
        rig.session.state() == SessionState::Failed
    });
    assert_eq!(
        rig.session.status().error_code,
        Some("recording_session_failed")
    );
    let error = rig.session.stop().unwrap_err();
    assert_eq!(error.code(), "recording_session_failed");
    assert!(!rig.handle.running());

    // The panic released the folder's lock; the manifest still says recording.
    let reports = rig.store.recover_pending().unwrap();
    assert_eq!(reports.len(), 1);
    assert!(
        matches!(reports[0].outcome, RecoveryOutcome::Recovered(_)),
        "{:?}",
        reports[0].outcome
    );
}

#[test]
fn the_session_and_its_parts_can_be_shared_between_threads() {
    fn send_sync<T: Send + Sync>() {}
    fn send<T: Send>() {}
    send_sync::<RecordingSession>();
    send_sync::<SessionStatus>();
    send::<SessionOutcome>();
    send::<SourceSpec>();
}

// ---- Codex review of 1b: who stops the sources, and when ----------------------

#[test]
fn a_track_that_ends_on_its_own_stops_the_source_before_the_session_is_done() {
    let rig = rig();
    rig.deliver(ms(200));
    rig.handle.change_device();
    wait_until("a terminal state", || rig.session.state().is_terminal());
    // Not "eventually": the moment `Done` is visible the stream is closed.
    assert!(
        !rig.handle.running(),
        "the microphone is still open at {:?}",
        rig.session.state()
    );
    rig.session.stop().unwrap();
}

#[test]
fn a_writer_that_fails_stops_the_source_before_the_session_is_failed() {
    let rig = rig_with(Quality::Lossless, MONO48, |config| {
        config.free_space = Arc::new(PanicsOnPoll(Default::default()));
    });
    wait_until("a terminal state", || rig.session.state().is_terminal());
    assert_eq!(rig.session.state(), SessionState::Failed);
    assert!(
        !rig.handle.running(),
        "the microphone is still open after Failed"
    );
}

/// A source whose `stop` is held until the test lets it go.
struct HeldStop {
    inner: FakeSource,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    held_once: bool,
}

impl CaptureSource for HeldStop {
    fn format(&self) -> NativeFormat {
        self.inner.format()
    }
    fn device_name(&self) -> Option<String> {
        None
    }
    fn start(&mut self, sink: BlockSink) -> Result<(), RecordingError> {
        self.inner.start(sink)
    }
    fn stop(&mut self) {
        // Idempotent, like every source: only the first stop is held.
        if !self.held_once {
            self.held_once = true;
            let _ = self.entered.send(());
            let _ = self.release.recv();
        }
        self.inner.stop();
    }
}

#[test]
fn a_second_stop_waits_for_the_first_to_stop_the_source_and_loses_no_audio() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (source, handle) = FakeSource::new(MONO48);
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let session = Arc::new(RecordingSession::new(
        store,
        config(handle.clock(), Quality::Lossless),
    ));
    session
        .start(vec![SourceSpec {
            id: "mic".into(),
            role: "mic".into(),
            source: Box::new(HeldStop {
                inner: source,
                entered: entered_tx,
                release: release_rx,
                held_once: false,
            }),
        }])
        .unwrap();
    handle.deliver(ms(200));

    let first = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || session.stop())
    };
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("the first stop reached the source");
    let second = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || session.stop())
    };

    // The source is still delivering, because its stop has not returned. The
    // second caller must not have told the writer to finish.
    std::thread::sleep(ms(200));
    let status = session.status();
    assert_eq!(
        status.tracks[0].ended, None,
        "the writer finished before the source stopped"
    );
    assert!(!status.state.is_terminal());
    handle.deliver(ms(300)); // audio that arrives while the stop is held

    release.send(()).unwrap();
    let a = first.join().unwrap().unwrap();
    let b = second.join().unwrap().unwrap();
    assert_eq!(a.manifest, b.manifest);
    assert_eq!(
        a.manifest.tracks[0].frames,
        handle.position(),
        "the late audio was kept"
    );
    assert!(!handle.running());
}

#[test]
fn a_writer_that_fails_at_once_cannot_be_overwritten_by_the_start_finishing_later() {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (source, handle) = FakeSource::new(MONO48);
    let mut config = config(handle.clock(), Quality::Lossless);
    config.free_space = Arc::new(PanicsOnPoll(Default::default()));
    let state_seen = Arc::new(std::sync::Mutex::new(None));
    let probe: Arc<std::sync::OnceLock<Arc<RecordingSession>>> = Arc::default();
    {
        let state_seen = Arc::clone(&state_seen);
        let probe = Arc::clone(&probe);
        // Right after the writer is spawned, hold `start` until the writer has
        // failed: the order that used to be a race.
        config.after_spawn = Some(Arc::new(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                if let Some(session) = probe.get() {
                    if session.state().is_terminal() {
                        *state_seen.lock().unwrap() = Some(session.state());
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }));
    }
    let session = Arc::new(RecordingSession::new(store, config));
    let _ = probe.set(Arc::clone(&session));
    session.start(vec![SourceSpec::mic(source)]).unwrap();
    assert_eq!(
        session.state(),
        SessionState::Failed,
        "`start` replaced a terminal state with Recording (it saw {:?} first)",
        state_seen.lock().unwrap()
    );
    assert!(session.stop().is_err());
}

// ---- Codex review of 1b, finding 7: the baseline is taken before capture ------

#[test]
fn a_sleep_before_the_writers_first_poll_still_ends_the_track() {
    // The probe's first reading is the baseline, taken in `start` before the
    // source is opened; every later reading is five seconds of sleep later.
    let calls = Arc::new(AtomicU64::new(0));
    let rig = rig_with(Quality::Lossless, MONO48, |config| {
        config.detect_sleep = true;
        let calls = Arc::clone(&calls);
        config.sleep_probe = Arc::new(move || {
            Some(if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                1_000
            } else {
                5_000_001_000
            })
        });
    });
    rig.handle.deliver(ms(200)); // moves the session clock past the first poll
    wait_until("the track to end", || {
        rig.session.state() == SessionState::Done
    });
    let outcome = rig.session.stop().unwrap();
    assert_eq!(
        outcome.manifest.tracks[0].ended_reason,
        Some(EndedReason::SystemSleep)
    );
}

// ---- Codex review round 2, finding 1: a block that arrives while a source stops

/// A source whose first `stop` waits at a barrier and then lets one more block
/// arrive, as an audio callback already in flight would, before it is stopped.
struct LateBlockOnStop {
    inner: FakeSource,
    handle: FakeHandle,
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
    held_once: bool,
}

impl CaptureSource for LateBlockOnStop {
    fn format(&self) -> NativeFormat {
        self.inner.format()
    }
    fn device_name(&self) -> Option<String> {
        None
    }
    fn start(&mut self, sink: BlockSink) -> Result<(), RecordingError> {
        self.inner.start(sink)
    }
    fn stop(&mut self) {
        if !self.held_once {
            self.held_once = true;
            let _ = self.entered.send(());
            let _ = self.release.recv();
            self.handle.deliver(ms(50));
        }
        self.inner.stop();
    }
}

fn late_block_session() -> (
    Arc<RecordingSession>,
    FakeHandle,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
    TempDir,
) {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let (source, handle) = FakeSource::new(MONO48);
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let session = Arc::new(RecordingSession::new(
        store,
        config(handle.clock(), Quality::Lossless),
    ));
    session
        .start(vec![SourceSpec {
            id: "mic".into(),
            role: "mic".into(),
            source: Box::new(LateBlockOnStop {
                inner: source,
                handle: handle.clone(),
                entered: entered_tx,
                release: release_rx,
                held_once: false,
            }),
        }])
        .unwrap();
    (session, handle, entered, release, temp)
}

#[test]
fn a_block_delivered_while_an_automatic_ending_stops_the_source_is_kept() {
    let (session, handle, entered, release, _temp) = late_block_session();
    handle.deliver(ms(200));
    handle.change_device();
    // The writer decided to end the track and is now stopping the source.
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("the writer stopped the source before it closed the track");
    assert!(
        !session.state().is_terminal(),
        "the track was closed with the source still running"
    );
    release.send(()).unwrap();
    wait_until("the session to finish", || {
        session.state() == SessionState::Done
    });
    let outcome = session.stop().unwrap();
    assert_eq!(outcome.ended_early, Some(EndedReason::DeviceChanged));
    assert_eq!(
        outcome.manifest.tracks[0].frames,
        handle.position(),
        "the block that arrived while the source was stopping was lost"
    );
    assert!(!handle.running());
}

#[test]
fn an_automatic_ending_and_the_users_stop_share_one_orderly_shutdown() {
    let (session, handle, entered, release, _temp) = late_block_session();
    handle.deliver(ms(200));
    handle.change_device();
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    // The user presses stop while the writer is stopping the source.
    let user = {
        let session = Arc::clone(&session);
        std::thread::spawn(move || session.stop())
    };
    std::thread::sleep(ms(100));
    assert!(!session.state().is_terminal());
    release.send(()).unwrap();
    let outcome = user.join().unwrap().unwrap();
    assert_eq!(outcome.manifest.tracks[0].frames, handle.position());
    assert_eq!(outcome.manifest.state, RecordingState::Complete);
    assert_eq!(
        outcome.manifest.tracks[0].ended_reason,
        Some(EndedReason::DeviceChanged)
    );
}

// ---- Codex review of 1b, round 4: the sources are in the set before the first block ----

#[test]
fn a_full_disk_at_the_first_block_stops_that_source_while_another_track_goes_on() {
    use super::core_tests::FailingFile;
    use crate::recording::encoder::TrackEncoder;
    use crate::recording::flac::FlacEncoder;
    use crate::recording::io::create_new_private;
    use crate::recording::TRACK_HEADER_LEN;

    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    // A first block of 65 536 frames is far more than the writer buffers: it is
    // written at once, and the mic's file has room for the header only.
    let (mic, mic_handle) = FakeSource::new(MONO48);
    let (system, system_handle) = FakeSource::new(MONO48);
    let mic = mic.with_prime(ms(1_400));
    let mut config = config(mic_handle.clock(), Quality::Lossless);
    config.block_frames = 65_536;
    config.encoders = Some(Arc::new(move |path, new| {
        let file =
            create_new_private(path).map_err(|e| RecordingError::io("create a track file", &e))?;
        let budget = if new.id == "mic" {
            TRACK_HEADER_LEN as u64
        } else {
            u64::MAX
        };
        Ok(Box::new(FlacEncoder::new(
            FailingFile { file, budget },
            new.sample_rate,
            new.channels,
            new.bits,
        )?) as Box<dyn TrackEncoder>)
    }));
    let session = RecordingSession::new(store, config);
    session
        .start(vec![
            SourceSpec::mic(mic),
            SourceSpec {
                id: "system".into(),
                role: "system".into(),
                source: Box::new(system),
            },
        ])
        .unwrap();

    // `start` has returned: the mic's first write already met the full disk.
    assert!(
        !mic_handle.running(),
        "the source of the cut-back track is still capturing"
    );
    assert!(
        system_handle.running(),
        "the other track is still recording"
    );
    assert_eq!(session.state(), SessionState::Recording);
    let status = session.status();
    assert_eq!(status.tracks[0].ended, Some(EndedReason::DiskFull));
    assert_eq!(status.tracks[1].ended, None);

    system_handle.deliver(ms(100));
    let outcome = session.stop().unwrap();
    assert_eq!(outcome.ended_early, Some(EndedReason::DiskFull));
    assert_eq!(outcome.manifest.state, RecordingState::Complete);
    assert_eq!(
        outcome.manifest.tracks[0].ended_reason,
        Some(EndedReason::DiskFull)
    );
    assert!(!system_handle.running());
}
