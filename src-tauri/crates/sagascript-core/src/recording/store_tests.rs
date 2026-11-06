use std::time::{Duration, Instant};

use sagascript_testsupport::TempDir;

use super::*;
use crate::hash::sha256_hex;
use crate::recording::flac::format::{parse_header, HEADER_LEN};
use crate::recording::journal::{read_journal, JournalEvent};
use crate::recording::manifest::parse_manifest;
use crate::recording::test_signals::*;

fn mic_recording() -> NewRecording {
    recording("Team meeting", vec![track("mic", 48_000, 1, 24)])
}

fn read_manifest(dir: &Path) -> Manifest {
    parse_manifest(&std::fs::read(dir.join(MANIFEST_FILE)).unwrap()).unwrap()
}

#[test]
fn a_new_recording_is_a_folder_with_its_final_name_and_four_files() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let writer = store.create(mic_recording()).unwrap();

    let name = writer.folder_name().to_string();
    assert!(
        name.starts_with("2026-11-05 14.02 Team meeting ["),
        "{name}"
    );
    assert!(name.ends_with(']'));
    assert_eq!(
        &name[name.len() - 7..name.len() - 1],
        &writer.recording_id().replace('-', "")[..6]
    );
    assert_eq!(writer.folder(), root.path().join(&name));
    assert_eq!(
        names(writer.folder()),
        [
            "mic.events.jsonl",
            "mic.flac",
            "recording.json",
            "recording.lock"
        ]
    );

    // The manifest on disk says the recording is in progress and does not yet
    // know how it will end.
    let manifest = read_manifest(writer.folder());
    assert_eq!(&manifest, writer.manifest());
    assert_eq!(manifest.state, RecordingState::Recording);
    assert!(!manifest.recovered);
    assert_eq!(manifest.title, "Team meeting");
    assert_eq!(manifest.started_at, STARTED_AT);
    let mic = &manifest.tracks[0];
    assert_eq!(
        (
            mic.frames,
            mic.audio_sha256.clone(),
            mic.ended_reason.clone()
        ),
        (0, None, None)
    );
    assert_eq!(mic.file, "mic.flac");
    assert!(mic.lossless);
    assert_eq!(mic.clock.anchors, vec![(1_000_000, 0)]);

    // The track file starts with a placeholder header, the journal with the start.
    let header = parse_header(&std::fs::read(writer.folder().join("mic.flac")).unwrap()).unwrap();
    assert_eq!((header.total_samples, header.first_frame), (0, HEADER_LEN));
    let journal = read_journal(&writer.folder().join("mic.events.jsonl")).unwrap();
    assert_eq!(journal.start, Some(("mach_continuous".into(), 1_000_000)));
}

#[cfg(unix)]
#[test]
fn the_folder_is_private_to_the_owner() {
    use std::os::unix::fs::PermissionsExt;
    let root = TempDir::new();
    let writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(writer.folder()), 0o700);
    for name in [
        "mic.flac",
        "mic.events.jsonl",
        "recording.json",
        "recording.lock",
    ] {
        assert_eq!(mode(&writer.folder().join(name)), 0o600, "{name}");
    }
}

#[test]
fn the_lock_is_held_while_the_recording_is_written_and_gone_when_it_is_finished() {
    let root = TempDir::new();
    let writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let folder = writer.folder().to_path_buf();
    assert!(matches!(
        lock_folder(&folder, false),
        Err(RecordingError::Locked)
    ));
    writer.finish(EndedReason::Stopped).unwrap();
    assert!(!folder.join(LOCK_FILE).exists());
}

#[test]
fn dropping_a_writer_releases_the_lock_and_leaves_the_recording_state() {
    let root = TempDir::new();
    let writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let folder = writer.folder().to_path_buf();
    drop(writer);
    assert!(folder.join(LOCK_FILE).exists());
    drop(lock_folder(&folder, false).expect("the lock is free after a drop"));
    assert_eq!(read_manifest(&folder).state, RecordingState::Recording);
}

#[test]
fn a_folder_that_exists_is_never_replaced_the_recording_gets_a_new_id() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let first_id = "aaaaaaaa-0000-4000-8000-000000000001".to_string();
    let second_id = "bbbbbbbb-0000-4000-8000-000000000002".to_string();

    // Someone else's folder under the very name the first id would give.
    let taken = root.path().join(naming::folder_name(
        "2026-11-05 14.02",
        "Team meeting",
        &first_id,
    ));
    std::fs::create_dir(&taken).unwrap();
    std::fs::write(taken.join("precious.txt"), b"keep me").unwrap();

    let mut ids = vec![second_id.clone(), first_id.clone()];
    let writer = store
        .create_with_ids(mic_recording(), &mut || ids.pop().unwrap())
        .unwrap();
    assert_eq!(writer.recording_id(), second_id);
    assert_ne!(writer.folder(), taken);
    assert_eq!(
        snapshot(&taken),
        vec![("precious.txt".to_string(), b"keep me".to_vec())]
    );
}

#[test]
fn when_no_free_name_is_found_nothing_is_created() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let id = "cccccccc-0000-4000-8000-000000000003".to_string();
    let taken = root
        .path()
        .join(naming::folder_name("2026-11-05 14.02", "Team meeting", &id));
    std::fs::create_dir(&taken).unwrap();

    let mut attempts = 0;
    let result = store.create_with_ids(mic_recording(), &mut || {
        attempts += 1;
        id.clone()
    });
    assert!(matches!(result, Err(RecordingError::NameUnavailable)));
    assert_eq!(attempts, 8);
    assert_eq!(
        names(root.path()),
        vec![taken.file_name().unwrap().to_string_lossy().into_owned()]
    );
    assert!(names(&taken).is_empty());
}

#[test]
fn two_recordings_of_the_same_title_and_minute_get_different_folders() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let a = store.create(mic_recording()).unwrap();
    let b = store.create(mic_recording()).unwrap();
    assert_ne!(a.folder(), b.folder());
    assert_ne!(a.recording_id(), b.recording_id());
}

#[test]
fn a_hostile_title_cannot_leave_the_recordings_folder() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let title = "../../etc/passwd:*?\"<>|\\..";
    let writer = store
        .create(recording(title, vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    assert_eq!(writer.folder().parent().unwrap(), root.path());
    assert!(
        !writer.folder_name().contains(['/', '\\', ':', '*', '?']),
        "{}",
        writer.folder_name()
    );
    // The manifest keeps what the user typed.
    assert_eq!(read_manifest(writer.folder()).title, title);
}

#[test]
fn a_finished_recording_has_a_complete_manifest_that_matches_the_files() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let first = at_bits(&speechlike(6000, 1), 24);
    let second = at_bits(&speechlike(9000, 2), 24);

    for chunk in first.chunks(1000) {
        writer.push("mic", &to_f32(chunk, 24)).unwrap();
    }
    writer.anchor("mic", 5_000_000).unwrap();
    writer.gap("mic", 480, GapReason::Overrun).unwrap();
    for chunk in second.chunks(777) {
        writer.push("mic", &to_f32(chunk, 24)).unwrap();
    }
    writer.anchor("mic", 9_000_000).unwrap();
    let folder = writer.folder().to_path_buf();
    let manifest = writer.finish(EndedReason::Stopped).unwrap();

    let mut expected = first.clone();
    expected.extend(std::iter::repeat_n(0, 480));
    expected.extend_from_slice(&second);
    let file = std::fs::read(folder.join("mic.flac")).unwrap();

    assert_eq!(
        manifest,
        read_manifest(&folder),
        "what finish returns is what is on disk"
    );
    assert_eq!(manifest.state, RecordingState::Complete);
    assert!(!manifest.recovered);
    let mic = &manifest.tracks[0];
    assert_eq!(mic.frames, expected.len() as u64);
    assert_eq!(
        mic.audio_sha256.as_deref(),
        Some(sha256_hex(&file[HEADER_LEN..]).as_str())
    );
    assert_eq!(mic.ended_reason, Some(EndedReason::Stopped));
    assert_eq!(mic.gaps, vec![Gap::new(6000, 480, GapReason::Overrun)]);
    assert_eq!(
        mic.clock.anchors,
        vec![(1_000_000, 0), (5_000_000, 6000), (9_000_000, 15_480)]
    );
    assert_eq!(mic.clipped_samples, 0);

    let decoded = decode_flac(&file);
    assert!(
        decoded.samples == expected,
        "the stored audio is the pushed audio plus the silence"
    );
    assert_eq!(decoded.md5_verified, Some(true));
    assert_eq!(
        names(&folder),
        ["mic.events.jsonl", "mic.flac", "recording.json"]
    );
    // The journal is kept and agrees with the manifest.
    let journal = read_journal(&folder.join("mic.events.jsonl")).unwrap();
    assert_eq!(journal.gaps, mic.gaps);
    assert_eq!(journal.anchors, mic.clock.anchors);
}

#[test]
fn tracks_end_one_at_a_time_with_their_own_reasons() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(recording(
            "Call",
            vec![track("mic", 48_000, 1, 24), track("system", 44_100, 2, 24)],
        ))
        .unwrap();
    let mono = at_bits(&speechlike(5000, 3), 24);
    let side = second_channel(&mono);
    writer.push("mic", &to_f32(&mono, 24)).unwrap();
    writer
        .push("system", &to_f32(&interleave(&[&mono, &side]), 24))
        .unwrap();

    let summary = writer
        .finish_track("system", EndedReason::DeviceChanged)
        .unwrap();
    assert_eq!(summary.frames, 5000);
    assert!(matches!(
        writer.push("system", &[0.0, 0.0]),
        Err(RecordingError::TrackFinished)
    ));
    assert!(matches!(
        writer.gap("system", 10, GapReason::Stall),
        Err(RecordingError::TrackFinished)
    ));
    assert!(matches!(
        writer.finish_track("system", EndedReason::Stopped),
        Err(RecordingError::TrackFinished)
    ));
    writer.push("mic", &to_f32(&mono, 24)).unwrap();

    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    assert_eq!(
        manifest.track("system").unwrap().ended_reason,
        Some(EndedReason::DeviceChanged)
    );
    assert_eq!(manifest.track("system").unwrap().frames, 5000);
    assert_eq!(
        manifest.track("mic").unwrap().ended_reason,
        Some(EndedReason::Stopped)
    );
    assert_eq!(manifest.track("mic").unwrap().frames, 10_000);
}

#[test]
fn the_compact_tier_is_described_as_resampled_and_not_lossless() {
    let root = TempDir::new();
    let mut compact = track("mic", 16_000, 1, 16);
    compact.resampled_from = Some(48_000);
    let mut request = recording("Compact", vec![compact]);
    request.quality = Quality::Compact;
    let writer = RecordingStore::new(root.path()).create(request).unwrap();
    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    let mic = &manifest.tracks[0];
    assert!(!mic.lossless);
    assert_eq!(
        (mic.sample_rate, mic.bits, mic.resampled_from),
        (16_000, 16, Some(48_000))
    );
    assert_eq!(manifest.quality, Quality::Compact);
}

#[test]
fn clipped_samples_reach_the_manifest() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    writer
        .push("mic", &[0.0, 1.0, 5.0, -0.5, f32::NAN])
        .unwrap();
    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    assert_eq!(manifest.tracks[0].clipped_samples, 3);
}

#[test]
fn bad_requests_are_refused_and_leave_nothing_behind() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let bad: Vec<(NewRecording, &str)> = vec![
        (
            NewRecording {
                started_at: "yesterday".into(),
                ..mic_recording()
            },
            "started_at",
        ),
        (
            NewRecording {
                tracks: vec![],
                ..mic_recording()
            },
            "no tracks",
        ),
        (
            recording(
                "Dup",
                vec![track("mic", 48_000, 1, 24), track("mic", 48_000, 1, 24)],
            ),
            "duplicate ids",
        ),
        (
            recording("Slash", vec![track("a/b", 48_000, 1, 24)]),
            "id with a separator",
        ),
        (
            recording("Dots", vec![track("..", 48_000, 1, 24)]),
            "id of dots",
        ),
        (
            NewRecording {
                title: "x".repeat(2000),
                ..mic_recording()
            },
            "long title",
        ),
    ];
    for (request, what) in bad {
        let result = store.create(request);
        assert!(
            matches!(result, Err(RecordingError::InvalidInput { .. })),
            "{what}: {:?}",
            result.err()
        );
    }
    // A format the encoder refuses is found after the folder exists: it is removed again.
    let result = store.create(recording("Odd", vec![track("mic", 48_000, 3, 24)]));
    assert!(matches!(
        result,
        Err(RecordingError::UnsupportedFormat { .. })
    ));
    let result = store.create(recording("Odd", vec![track("mic", 48_000, 1, 20)]));
    assert!(matches!(
        result,
        Err(RecordingError::UnsupportedFormat { .. })
    ));
    assert!(names(root.path()).is_empty(), "{:?}", names(root.path()));
}

#[test]
fn an_unknown_track_is_an_error_not_a_panic() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    assert!(matches!(
        writer.push("nope", &[0.0]),
        Err(RecordingError::UnknownTrack)
    ));
    assert!(matches!(
        writer.anchor("nope", 1),
        Err(RecordingError::UnknownTrack)
    ));
    assert!(matches!(
        writer.gap("nope", 1, GapReason::Stall),
        Err(RecordingError::UnknownTrack)
    ));
    assert!(matches!(
        writer.finish_track("nope", EndedReason::Stopped),
        Err(RecordingError::UnknownTrack)
    ));
}

#[test]
fn the_periodic_sync_runs_when_the_interval_has_passed() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let start = Instant::now();
    writer
        .push("mic", &to_f32(&at_bits(&speechlike(9000, 4), 24), 24))
        .unwrap();
    assert!(!writer.sync_if_due(start).unwrap(), "just created");
    assert!(!writer
        .sync_if_due(start + SYNC_INTERVAL - Duration::from_millis(1))
        .unwrap());
    let due = start + SYNC_INTERVAL + Duration::from_secs(1);
    assert!(writer.sync_if_due(due).unwrap());
    // The two full blocks are on disk now, before any finish.
    let file = std::fs::read(writer.folder().join("mic.flac")).unwrap();
    assert!(file.len() > HEADER_LEN);
    assert!(
        !writer.sync_if_due(due + Duration::from_secs(1)).unwrap(),
        "the clock restarted"
    );
}

#[test]
fn a_gap_is_in_the_journal_and_the_track_before_the_recording_ends() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    writer
        .push("mic", &to_f32(&at_bits(&speechlike(3000, 5), 24), 24))
        .unwrap();
    writer.gap("mic", 10_000, GapReason::Stall).unwrap();
    let folder = writer.folder().to_path_buf();
    let file = std::fs::read(folder.join("mic.flac")).unwrap();
    assert!(
        file.len() > HEADER_LEN,
        "the silence's full blocks were flushed by the gap"
    );
    let journal = read_journal(&folder.join("mic.events.jsonl")).unwrap();
    assert_eq!(journal.gaps, vec![Gap::new(3000, 10_000, GapReason::Stall)]);
    // A zero-length gap is nothing.
    writer.gap("mic", 0, GapReason::Overrun).unwrap();
    assert_eq!(
        read_journal(&folder.join("mic.events.jsonl"))
            .unwrap()
            .gaps
            .len(),
        1
    );
}

#[test]
fn the_start_event_of_every_track_is_journalled() {
    let root = TempDir::new();
    let writer = RecordingStore::new(root.path())
        .create(recording(
            "Two",
            vec![track("mic", 48_000, 1, 24), track("system", 44_100, 2, 24)],
        ))
        .unwrap();
    for (id, rate, channels) in [("mic", 48_000, 1), ("system", 44_100, 2)] {
        let text =
            std::fs::read_to_string(writer.folder().join(format!("{id}.events.jsonl"))).unwrap();
        let event: JournalEvent = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(
            event,
            JournalEvent::Start {
                domain: "mach_continuous".into(),
                host_ns: 1_000_000,
                sample_rate: rate,
                channels
            }
        );
    }
}
