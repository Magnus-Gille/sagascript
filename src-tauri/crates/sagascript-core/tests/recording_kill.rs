//! A real process killed in the middle of a recording (SAGA-562, design §7).
//!
//! The test binary runs itself again: the child (`child_entry`, active only
//! when `SAGASCRIPT_KILL_TEST_ROOT` is set) records into a temporary folder as
//! fast as it can without ever flushing, and the parent kills it with SIGKILL
//! at an arbitrary moment. Whatever the child's `BufWriter` still held is lost,
//! and the parent then requires that recovery keeps exactly the frames that
//! reached the file. The oracle is an independent in-memory encode of the same
//! deterministic signal.
//!
//! A kill does not tear a frame here: the writer hands the file whole frames
//! and the kernel finishes a `write` that has started, so the file ends on a
//! frame boundary and the lost frames are the ones still in the buffer. A torn
//! last frame is what a power loss, a full disk or a copy cut short leaves;
//! `recording/recover_tests.rs` cuts a file at every frame boundary, inside
//! every frame and at random offsets for that.
//!
//! The child says it is running by writing a small file, not by printing:
//! libtest captures the output of a passing test unless it is told not to, and
//! the flag that says so has been spelled differently by different toolchains.
//!
//! Unix only: `Child::kill` is SIGKILL there, and the advisory lock is released
//! by the kernel when the process dies.
#![cfg(unix)]

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use sagascript_core::recording::{
    parse_manifest, Client, FlacEncoder, NewRecording, NewTrack, Quality, RecordingError,
    RecordingState, RecordingStore, RecoveryOutcome, MANIFEST_FILE, TRACK_HEADER_LEN,
};
use sagascript_testsupport::TempDir;

const ROOT_ENV: &str = "SAGASCRIPT_KILL_TEST_ROOT";
const READY_ENV: &str = "SAGASCRIPT_KILL_TEST_READY";
const BLOCK: usize = 4096;
/// The child announces itself after this many blocks and goes on.
const READY_AFTER_BLOCKS: usize = 40;
/// The child stops writing after this many blocks (and waits to be killed).
const MAX_BLOCKS: usize = 400;
/// How long the child waits to be killed before it gives up and exits.
const CHILD_PATIENCE: Duration = Duration::from_secs(60);

/// The signal both processes know: tones and a noise floor, by sample index.
fn signal(start: usize, len: usize) -> Vec<i32> {
    (start..start + len)
        .map(|i| {
            let t = i as f64 / 48_000.0;
            let tone = 0.3 * (2.0 * std::f64::consts::PI * 220.0 * t).sin()
                + 0.1 * (2.0 * std::f64::consts::PI * 1771.0 * t).sin();
            let noise = ((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 44) as i64 - (1 << 19);
            (tone * 8_388_607.0) as i32 + (noise >> 8) as i32
        })
        .collect()
}

fn request() -> NewRecording {
    NewRecording {
        title: "Kill test".into(),
        started_at: "2026-11-06T09:30:00.000+01:00".into(),
        client: Client {
            kind: "desktop".into(),
            app_version: "test".into(),
            build: "test".into(),
            extra: Default::default(),
        },
        quality: Quality::Lossless,
        tracks: vec![NewTrack {
            id: "mic".into(),
            role: "mic".into(),
            sample_rate: 48_000,
            channels: 1,
            bits: 24,
            clock_domain: "mach_continuous".into(),
            start_ns: 1,
            resampled_from: None,
            device: None,
        }],
    }
}

/// Decode a FLAC file with symphonia, an independent implementation, and check
/// the MD5 STREAMINFO carries against what it decoded.
fn decode(bytes: &[u8]) -> Vec<i32> {
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
        "STREAMINFO's MD5 matches the decoded audio"
    );
    samples
}

/// The child: record without flushing until killed. Does nothing in a normal run.
#[test]
fn child_entry() {
    let Ok(root) = std::env::var(ROOT_ENV) else {
        return;
    };
    let ready = PathBuf::from(std::env::var(READY_ENV).expect("the parent names the ready file"));
    let store = RecordingStore::new(root);
    let mut writer = store
        .create(request())
        .expect("the child creates its recording");
    for block in 0..MAX_BLOCKS {
        let samples = signal(block * BLOCK, BLOCK);
        let floats: Vec<f32> = samples.iter().map(|s| *s as f32 / 8_388_608.0).collect();
        writer.push("mic", &floats).expect("the child writes");
        if block + 1 == READY_AFTER_BLOCKS {
            // Write and rename, so the parent never reads half a name.
            let partial = ready.with_extension("part");
            std::fs::write(&partial, writer.folder_name()).unwrap();
            std::fs::rename(&partial, &ready).unwrap();
        }
    }
    // Wait to be killed; do not outlive a parent that died.
    std::thread::sleep(CHILD_PATIENCE);
    std::process::exit(0);
}

/// A running child recording. Dropping it kills the child, so a failed
/// assertion does not leave a process writing to disk.
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

/// Start the child and wait until it has written `READY_AFTER_BLOCKS` blocks.
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

/// SIGKILL the child and wait for it to be gone; returns its recording folder.
fn kill(mut running: RunningChild) -> PathBuf {
    running.child.kill().expect("SIGKILL");
    running.child.wait().unwrap();
    running.folder.clone()
}

/// Byte offsets at which each frame of the first `blocks` blocks of `signal`
/// ends in a track file, from an independent in-memory encode.
fn frame_ends(blocks: usize) -> (Vec<usize>, Vec<i32>) {
    let pcm = signal(0, blocks * BLOCK);
    let mut encoder = FlacEncoder::new(Cursor::new(Vec::new()), 48_000, 1, 24)
        .unwrap()
        .with_frame_log();
    encoder.push_pcm(&pcm).unwrap();
    let lengths = encoder.finish_into_parts().unwrap().frame_lengths.unwrap();
    let mut ends = vec![TRACK_HEADER_LEN];
    for length in lengths {
        ends.push(ends.last().unwrap() + length as usize);
    }
    (ends, pcm)
}

#[test]
fn a_process_killed_mid_write_is_recovered_with_exactly_the_frames_that_reached_the_disk() {
    // The oracle covers everything the child can write.
    let (ends, pcm) = frame_ends(MAX_BLOCKS);

    // Different delays land the kill in different places: between frames, in
    // the middle of one, with the buffer empty or full.
    for delay_ms in [0u64, 3, 11, 29, 60] {
        let root = TempDir::new();
        let markers = TempDir::new();
        let running = spawn_child(root.path(), markers.path());
        std::thread::sleep(Duration::from_millis(delay_ms));
        let folder = kill(running);

        // What the dead process left: a recording, a free lock, bytes on disk.
        let file = folder.join("mic.flac");
        let on_disk = std::fs::read(&file).unwrap();
        let manifest = parse_manifest(&std::fs::read(folder.join(MANIFEST_FILE)).unwrap()).unwrap();
        assert_eq!(manifest.state, RecordingState::Recording);
        assert!(
            on_disk.len() <= *ends.last().unwrap(),
            "the oracle covers the file"
        );

        let store = RecordingStore::new(root.path());
        let reports = store.recover_pending().unwrap();
        assert_eq!(reports.len(), 1);
        let RecoveryOutcome::Recovered(recovered) = &reports[0].outcome else {
            panic!(
                "expected a recovery after {delay_ms} ms, got {:?}",
                reports[0].outcome
            );
        };
        let report = &recovered.tracks[0];

        // The frames that lie entirely inside what reached the disk.
        let complete = ends.iter().filter(|end| **end <= on_disk.len()).count() - 1;
        assert!(
            complete >= 1,
            "the child had written nothing after {delay_ms} ms"
        );
        // Visible with --no-capture: where each kill landed.
        eprintln!(
            "killed after {delay_ms} ms: {} bytes on disk, {complete} whole frames, {} torn bytes",
            on_disk.len(),
            on_disk.len() - ends[complete]
        );
        assert_eq!(
            report.frames_kept,
            (complete * BLOCK) as u64,
            "delay {delay_ms} ms"
        );
        assert_eq!(
            report.tail_bytes,
            (on_disk.len() - ends[complete]) as u64,
            "delay {delay_ms} ms"
        );

        // The frames that were kept are the bytes that were written, and the
        // rest is in the tail untouched.
        let recovered_bytes = std::fs::read(&file).unwrap();
        assert_eq!(recovered_bytes.len(), ends[complete]);
        assert!(recovered_bytes[TRACK_HEADER_LEN..] == on_disk[TRACK_HEADER_LEN..ends[complete]]);
        if report.tail_bytes > 0 {
            assert!(std::fs::read(folder.join("mic.tail")).unwrap() == on_disk[ends[complete]..]);
        }

        // The recovered file is the first `complete` blocks of the signal.
        let decoded = decode(&recovered_bytes);
        assert_eq!(decoded.len(), complete * BLOCK);
        assert!(
            decoded == pcm[..complete * BLOCK],
            "the audio is the audio the child pushed"
        );

        let manifest = parse_manifest(&std::fs::read(folder.join(MANIFEST_FILE)).unwrap()).unwrap();
        assert_eq!(manifest.state, RecordingState::Complete);
        assert!(manifest.recovered);
        assert_eq!(manifest.tracks[0].frames, (complete * BLOCK) as u64);
    }
}

#[test]
fn recovery_leaves_a_live_process_alone_and_takes_over_when_it_dies() {
    let root = TempDir::new();
    let markers = TempDir::new();
    let running = spawn_child(root.path(), markers.path());
    let folder = running.folder.clone();
    let store = RecordingStore::new(root.path());

    // The child is still writing: another process (the app starting up while
    // the CLI records) must not touch the folder.
    let manifest_before = std::fs::read(folder.join(MANIFEST_FILE)).unwrap();
    let reports = store.recover_pending().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, RecoveryOutcome::SkippedLocked);
    assert!(matches!(
        store.recover(folder.file_name().unwrap().to_str().unwrap()),
        Err(RecordingError::Locked)
    ));
    assert_eq!(
        std::fs::read(folder.join(MANIFEST_FILE)).unwrap(),
        manifest_before
    );
    assert!(folder.join("recording.lock").exists());

    // Once it is dead the lock is free and the recording is recovered.
    kill(running);
    let reports = store.recover_pending().unwrap();
    assert!(
        matches!(reports[0].outcome, RecoveryOutcome::Recovered(_)),
        "{:?}",
        reports[0].outcome
    );
}
