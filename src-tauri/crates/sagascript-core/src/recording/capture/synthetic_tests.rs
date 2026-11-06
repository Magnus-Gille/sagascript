use std::time::{Duration, Instant};

use sagascript_testsupport::TempDir;

use super::*;
use crate::recording::list_recordings;
use crate::recording::manifest::{Client, Quality, RecordingState};
use crate::recording::session::{RecordingSession, SessionConfig, SourceSpec};
use crate::recording::store::RecordingStore;
use crate::recording::test_signals::{decode_flac, STARTED_AT};

#[test]
fn the_variable_names_a_synthetic_source_and_optionally_its_pitch() {
    let default = parse_test_source(true, Some("synthetic")).unwrap();
    assert_eq!(default, SyntheticConfig::default());
    assert_eq!(
        parse_test_source(true, Some(" synthetic:1000 "))
            .unwrap()
            .hz,
        1000.0
    );
    assert_eq!(
        parse_test_source(true, Some("synthetic:261.63"))
            .unwrap()
            .hz,
        261.63
    );
    for refused in [
        None,
        Some(""),
        Some("mic"),
        Some("synthetic:"),
        Some("synthetic:abc"),
        Some("synthetic:0"),
        Some("synthetic:-5"),
        Some("synthetic:24000"),
        Some("synthetic:NaN"),
        Some("syntheticx"),
        Some("SYNTHETIC"),
    ] {
        assert_eq!(parse_test_source(true, refused), None, "{refused:?}");
    }
}

#[test]
fn the_variable_may_delay_the_start_of_the_source() {
    let delayed = parse_test_source(true, Some("synthetic:440:1500")).unwrap();
    assert_eq!(delayed.hz, 440.0);
    assert_eq!(delayed.startup_delay, Duration::from_millis(1500));
    assert_eq!(
        parse_test_source(true, Some("synthetic:440"))
            .unwrap()
            .startup_delay,
        Duration::ZERO
    );
    for refused in [
        "synthetic:440:",
        "synthetic:440:abc",
        "synthetic:440:-1",
        "synthetic:440:10001",
        "synthetic:440:5:6",
    ] {
        assert_eq!(parse_test_source(true, Some(refused)), None, "{refused:?}");
    }
}

#[test]
fn a_release_build_ignores_the_variable() {
    assert_eq!(parse_test_source(false, Some("synthetic")), None);
    assert_eq!(parse_test_source(false, Some("synthetic:440")), None);
    // This build's own answer follows its flags.
    let previous = std::env::var_os(TEST_SOURCE_ENV);
    std::env::set_var(TEST_SOURCE_ENV, "synthetic");
    let honoured = test_source_requested().is_some();
    match previous {
        Some(value) => std::env::set_var(TEST_SOURCE_ENV, value),
        None => std::env::remove_var(TEST_SOURCE_ENV),
    }
    assert_eq!(
        honoured,
        cfg!(debug_assertions) || cfg!(feature = "dev-overrides")
    );
}

fn session(store: &RecordingStore, quality: Quality) -> RecordingSession {
    let client = Client {
        kind: "desktop".into(),
        app_version: "test".into(),
        build: "test".into(),
        extra: Default::default(),
    };
    let mut config = SessionConfig::new("Synthetic", STARTED_AT, client, quality);
    config.detect_sleep = false;
    RecordingSession::new(store.clone(), config)
}

fn record_for(quality: Quality, duration: Duration) -> (TempDir, RecordingStore) {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    let session = session(&store, quality);
    let begun = Instant::now();
    session
        .start(vec![SourceSpec::mic(SyntheticSource::default())])
        .expect("the synthetic source starts");
    std::thread::sleep(duration);
    let outcome = session.stop().expect("the session stops");
    assert!(begun.elapsed() < Duration::from_secs(10));
    assert_eq!(outcome.manifest.state, RecordingState::Complete);
    assert_eq!(outcome.ended_early, None);
    (temp, store)
}

#[test]
fn a_lossless_session_over_it_produces_a_complete_folder() {
    let (_temp, store) = record_for(Quality::Lossless, Duration::from_millis(300));
    let list = list_recordings(store.root());
    assert_eq!(list.len(), 1);
    let summary = &list[0];
    assert!(summary.playable && !summary.needs_recovery);
    assert_eq!((summary.sample_rate, summary.channels), (48_000, 1));
    assert_eq!(summary.gap_count, 0, "real time, so no gaps");
    assert!(
        (0.2..1.5).contains(&summary.duration_seconds),
        "{} s",
        summary.duration_seconds
    );

    let folder = store.root().join(&summary.folder_name);
    assert!(!folder.join("recording.lock").exists());
    let manifest =
        crate::recording::parse_manifest(&std::fs::read(folder.join("recording.json")).unwrap())
            .unwrap();
    let track = &manifest.tracks[0];
    assert_eq!(track.device.as_deref(), Some(SYNTHETIC_DEVICE_NAME));
    assert_eq!((track.bits, track.resampled_from), (24, None));
    let decoded = decode_flac(&std::fs::read(folder.join("mic.flac")).unwrap());
    assert_eq!(decoded.samples.len() as u64, track.frames);
    assert_eq!(decoded.md5_verified, Some(true));
    // A 440 Hz sine at 0.5 has an RMS of 0.354 (in 24-bit units).
    let rms = (decoded
        .samples
        .iter()
        .map(|s| (*s as f64).powi(2))
        .sum::<f64>()
        / decoded.samples.len() as f64)
        .sqrt()
        / f64::from(1 << 23);
    assert!((rms - 0.354).abs() < 0.02, "rms {rms}");
}

#[test]
fn a_compact_session_over_it_produces_a_complete_folder() {
    let (_temp, store) = record_for(Quality::Compact, Duration::from_millis(300));
    let list = list_recordings(store.root());
    assert_eq!(list.len(), 1);
    assert!(list[0].playable);
    assert_eq!((list[0].sample_rate, list[0].channels), (16_000, 1));
    let folder = store.root().join(&list[0].folder_name);
    let manifest =
        crate::recording::parse_manifest(&std::fs::read(folder.join("recording.json")).unwrap())
            .unwrap();
    let track = &manifest.tracks[0];
    assert_eq!((track.bits, track.resampled_from), (16, Some(48_000)));
    let decoded = decode_flac(&std::fs::read(folder.join("mic.flac")).unwrap());
    assert_eq!(decoded.samples.len() as u64, track.frames);
}

#[test]
fn it_reports_its_format_delivers_blocks_and_stops_cleanly() {
    let mut source = SyntheticSource::new(SyntheticConfig {
        hz: 1000.0,
        amplitude: 0.25,
        format: NativeFormat {
            sample_rate: 16_000,
            channels: 2,
        },
    });
    assert_eq!(source.format().sample_rate, 16_000);
    assert_eq!(source.device_name().as_deref(), Some(SYNTHETIC_DEVICE_NAME));
    source.stop(); // never started: nothing to do
    let (sink, mut receiver) = crate::recording::capture::track_channel(
        source.format(),
        crate::recording::capture::ChannelConfig {
            block_frames: 256,
            blocks: 32,
        },
    );
    source.start(sink).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let block = loop {
        if let Some(block) = receiver.pop() {
            break block;
        }
        assert!(Instant::now() < deadline, "no block");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(block.frames(), 160, "10 ms at 16 kHz");
    assert_eq!(block.samples()[0], 0.0);
    assert_eq!(block.samples()[1], 0.0);
    source.stop();
    source.stop();
    assert!(source.thread.is_none());
}
