//! A real process killed in the middle of a recording *session* (SAGA-563,
//! design §7).
//!
//! The test binary runs itself again: the child (`child_entry`, active only
//! when `SAGASCRIPT_SESSION_KILL_ROOT` is set) records a fake source through a
//! `RecordingSession` (capture queue, writer thread, gaps, anchors, periodic
//! fsync) and the parent kills it with SIGKILL at an arbitrary moment. The
//! parent then requires that 1a recovery finishes the folder with exactly the
//! frames that reached the disk, and that the audio is the child's signal with
//! the gap the child injected. The oracle is the signal itself: the fake
//! source's audio is a pure function of the frame index.
//!
//! Unix only: `Child::kill` is SIGKILL there and the advisory lock is released
//! by the kernel when the process dies.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use sagascript_core::recording::capture::{
    FakeSignal, FakeSource, ManualClock, NativeFormat, MANUAL_HOST_BASE_NS,
};
use sagascript_core::recording::{
    parse_manifest, Client, GapReason, Quality, RecordingSession, RecordingState, RecordingStore,
    RecoveryOutcome, SessionConfig, SourceSpec, MANIFEST_FILE,
};
use sagascript_testsupport::TempDir;

const ROOT_ENV: &str = "SAGASCRIPT_SESSION_KILL_ROOT";
const READY_ENV: &str = "SAGASCRIPT_SESSION_KILL_READY";
const FORMAT: NativeFormat = NativeFormat {
    sample_rate: 48_000,
    channels: 1,
};
/// The child delivers 10 ms at a time.
const CHUNK_FRAMES: u64 = 480;
/// The prime of the fake source (20 ms).
const PRIME_FRAMES: u64 = 960;
/// Chunk number at which the child injects an overrun.
const OVERRUN_AT_CHUNK: u64 = 30;
const OVERRUN_FRAMES: u64 = 480;
const READY_AFTER_CHUNKS: u64 = 60;
const MAX_CHUNKS: u64 = 3_000;
const CHILD_PATIENCE: Duration = Duration::from_secs(60);
const SIGNAL: FakeSignal = FakeSignal::Noise {
    amplitude: 0.5,
    seed: 21,
};

fn quantise24(sample: f32) -> i32 {
    ((f64::from(sample) * 8_388_608.0).round() as i64).clamp(-8_388_608, 8_388_607) as i32
}

/// The child: record without ever stopping. Does nothing in a normal run.
#[test]
fn child_entry() {
    let Ok(root) = std::env::var(ROOT_ENV) else {
        return;
    };
    let ready = PathBuf::from(std::env::var(READY_ENV).expect("the parent names the ready file"));
    let (source, handle) = FakeSource::with_clock(FORMAT, Arc::new(ManualClock::new()));
    handle.set_signal(SIGNAL);
    let mut config = SessionConfig::new(
        "Session kill test",
        "2026-11-06T09:30:00.000+01:00",
        Client {
            kind: "desktop".into(),
            app_version: "test".into(),
            build: "test".into(),
            extra: Default::default(),
        },
        Quality::Lossless,
    );
    config.clock = handle.clock();
    config.detect_sleep = false;
    config.folder_warnings = Some(Vec::new());
    let session = RecordingSession::new(RecordingStore::new(root), config);
    session
        .start(vec![SourceSpec::mic(source)])
        .expect("the child starts its session");

    for chunk in 0..MAX_CHUNKS {
        if chunk == OVERRUN_AT_CHUNK {
            handle.overrun(OVERRUN_FRAMES);
        }
        handle.deliver_frames(CHUNK_FRAMES);
        // Pace the child by the writer so nothing is dropped by accident.
        while {
            let track = &session.status().tracks[0];
            track.frames_received + track.dropped_frames
                < handle.frames_delivered() + handle.frames_dropped()
        } {
            std::thread::sleep(Duration::from_micros(200));
        }
        if chunk + 1 == READY_AFTER_CHUNKS {
            let name = session.folder().unwrap();
            let name = name.file_name().unwrap().to_string_lossy().into_owned();
            let partial = ready.with_extension("part");
            std::fs::write(&partial, name).unwrap();
            std::fs::rename(&partial, &ready).unwrap();
        }
    }
    // Wait to be killed. `exit` runs no destructors, so the session is never
    // stopped politely.
    std::thread::sleep(CHILD_PATIENCE);
    std::process::exit(0);
}

struct RunningChild {
    child: Child,
    folder: PathBuf,
}

impl Drop for RunningChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_child(root: &Path, markers: &Path) -> RunningChild {
    let ready = markers.join("ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["child_entry", "--exact", "--test-threads=1"])
        .env(ROOT_ENV, root)
        .env(READY_ENV, &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the test binary starts itself");
    let started = Instant::now();
    loop {
        if let Ok(name) = std::fs::read_to_string(&ready) {
            return RunningChild {
                child,
                folder: root.join(name.trim()),
            };
        }
        if let Some(status) = child.try_wait().unwrap() {
            panic!("the child exited ({status}) before it was recording");
        }
        if started.elapsed() > Duration::from_secs(120) {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the child did not start recording within two minutes");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn kill(mut running: RunningChild) -> PathBuf {
    running.child.kill().expect("SIGKILL");
    running.child.wait().unwrap();
    running.folder.clone()
}

/// Decode with symphonia, an independent implementation, and check that the
/// MD5 in STREAMINFO matches what it decoded.
fn decode(bytes: &[u8]) -> Vec<i32> {
    use std::io::Cursor;
    use symphonia::core::audio::{AudioBufferRef, Signal};
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes.to_vec())), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("flac");
    let mut reader = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .expect("a FLAC file")
        .format;
    let track = reader.default_track().unwrap().clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions { verify: true })
        .unwrap();
    let mut samples = Vec::new();
    while let Ok(packet) = reader.next_packet() {
        if let AudioBufferRef::S32(buffer) = decoder.decode(&packet).expect("every frame decodes") {
            samples.extend(buffer.chan(0).iter().map(|s| *s >> 8));
        }
    }
    assert_eq!(
        decoder.finalize().verify_ok,
        Some(true),
        "STREAMINFO's MD5 matches"
    );
    samples
}

#[test]
fn a_session_killed_mid_recording_is_recovered_with_the_gap_and_exactly_the_frames_on_disk() {
    let gap_at = PRIME_FRAMES + OVERRUN_AT_CHUNK * CHUNK_FRAMES;
    for delay_ms in [0u64, 7, 40] {
        let root = TempDir::new();
        let markers = TempDir::new();
        let running = spawn_child(root.path(), markers.path());
        std::thread::sleep(Duration::from_millis(delay_ms));
        let folder = kill(running);

        let on_disk = std::fs::read(folder.join("mic.flac")).unwrap();
        let live = parse_manifest(&std::fs::read(folder.join(MANIFEST_FILE)).unwrap()).unwrap();
        assert_eq!(live.state, RecordingState::Recording, "delay {delay_ms}");

        let store = RecordingStore::new(root.path());
        let reports = store.recover_pending().unwrap();
        assert_eq!(reports.len(), 1);
        let RecoveryOutcome::Recovered(recovered) = &reports[0].outcome else {
            panic!("delay {delay_ms}: {:?}", reports[0].outcome);
        };
        let frames = recovered.tracks[0].frames_kept;
        // Whole blocks only, and at least the three before the gap, which
        // was fsynced when it happened.
        assert!(frames.is_multiple_of(4096), "delay {delay_ms}");
        assert!(frames >= 3 * 4096, "delay {delay_ms}: {frames} frames");
        eprintln!(
            "killed after {delay_ms} ms: {} bytes on disk, {frames} frames kept, {} torn bytes",
            on_disk.len(),
            recovered.tracks[0].tail_bytes
        );

        // The audio is the signal, with the injected overrun as silence.
        let decoded = decode(&std::fs::read(folder.join("mic.flac")).unwrap());
        assert_eq!(decoded.len() as u64, frames);
        for (index, sample) in decoded.iter().enumerate() {
            let index = index as u64;
            let want = if (gap_at..gap_at + OVERRUN_FRAMES).contains(&index) {
                0
            } else {
                quantise24(SIGNAL.sample(index, 0, FORMAT))
            };
            assert_eq!(*sample, want, "delay {delay_ms}: frame {index}");
        }

        // The gap came back from the journal, cut to the audio that was kept.
        let manifest = parse_manifest(&std::fs::read(folder.join(MANIFEST_FILE)).unwrap()).unwrap();
        assert_eq!(manifest.state, RecordingState::Complete);
        assert!(manifest.recovered);
        let track = &manifest.tracks[0];
        assert_eq!(track.frames, frames);
        if frames > gap_at {
            assert_eq!(track.gaps.len(), 1, "delay {delay_ms}");
            assert_eq!(track.gaps[0].at_frame, gap_at);
            assert_eq!(track.gaps[0].length, OVERRUN_FRAMES.min(frames - gap_at));
            assert_eq!(track.gaps[0].reason, GapReason::Overrun);
        } else {
            assert!(track.gaps.is_empty());
        }
        // The clock: the first frame was captured at the fake host clock's base.
        assert_eq!(track.clock.start_ns, MANUAL_HOST_BASE_NS);
        assert_eq!(track.clock.anchors[0], (MANUAL_HOST_BASE_NS, 0));
    }
}

#[test]
fn recovery_leaves_a_live_session_alone_and_takes_over_when_it_dies() {
    let root = TempDir::new();
    let markers = TempDir::new();
    let running = spawn_child(root.path(), markers.path());
    let folder = running.folder.clone();
    let store = RecordingStore::new(root.path());

    let manifest_before = std::fs::read(folder.join(MANIFEST_FILE)).unwrap();
    let reports = store.recover_pending().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, RecoveryOutcome::SkippedLocked);
    assert_eq!(
        std::fs::read(folder.join(MANIFEST_FILE)).unwrap(),
        manifest_before
    );

    kill(running);
    let reports = store.recover_pending().unwrap();
    assert!(
        matches!(reports[0].outcome, RecoveryOutcome::Recovered(_)),
        "{:?}",
        reports[0].outcome
    );
}
