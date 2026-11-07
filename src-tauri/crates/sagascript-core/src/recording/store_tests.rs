use std::time::{Duration, Instant};

use sagascript_testsupport::TempDir;

use super::*;
use crate::hash::sha256_hex;
use crate::recording::flac::format::{parse_header, HEADER_LEN};
use crate::recording::journal::{read_journal, JournalEvent, JournalFault};
use crate::recording::manifest::parse_manifest;
use crate::recording::recover::recover_folder;
use crate::recording::test_signals::*;
use crate::recording::TRACK_HEADER_LEN;

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
            "recording.lock",
            "recording.start.json"
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

/// The writer thread of phase 1b owns the writer: it must be `Send`.
#[test]
fn the_writer_and_the_encoder_can_move_to_a_writer_thread() {
    fn is_send<T: Send>() {}
    is_send::<RecordingWriter>();
    is_send::<FlacEncoder<std::fs::File>>();
    is_send::<Box<dyn TrackEncoder>>();
}

// ---- a crash at every step of a gap ---------------------------------------------

/// A recording with 6000 frames of speech and then a 3000-frame gap, stopped
/// by a "crash" after `step` of the gap, and the audio that was pushed in all
/// (silence included). The block that straddles the start of the gap is
/// frames 4096..8192; the rest of the gap is still waiting for a block.
fn gap_crashed_after(step: GapStep) -> (TempDir, PathBuf, Vec<i32>) {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut writer = store.create(mic_recording()).unwrap();
    let first = at_bits(&speechlike(6000, 51), 24);
    writer.push("mic", &to_f32(&first, 24)).unwrap();
    writer.crash_after = Some(step);
    let error = writer.gap("mic", 3000, GapReason::Overrun).unwrap_err();
    assert!(
        error.to_string().contains("injected"),
        "the crash is the injected one: {error}"
    );
    let folder = writer.folder().to_path_buf();
    drop(writer);
    let mut pcm = first;
    pcm.extend(std::iter::repeat_n(0, 3000));
    (root, folder, pcm)
}

#[test]
fn whatever_step_of_a_gap_a_crash_stops_at_the_silence_that_survives_is_marked() {
    for step in GapStep::ALL {
        let (_root, folder, pcm) = gap_crashed_after(step);
        let recovered = recover_folder(&folder).unwrap();
        let kept = recovered.tracks[0].frames_kept as usize;

        // The audio on disk is the audio that was pushed, as far as it goes.
        let file = std::fs::read(folder.join("mic.flac")).unwrap();
        if kept > 0 {
            let decoded = decode_flac(&file);
            assert!(decoded.samples == pcm[..kept], "{step:?}: the kept audio");
        }

        // Every frame of inserted silence that was kept is inside a gap, and
        // no gap covers a frame that is not inserted silence.
        let expected: Vec<Gap> = if kept > 6000 {
            vec![Gap::new(
                6000,
                (kept.min(9000) - 6000) as u64,
                GapReason::Overrun,
            )]
        } else {
            Vec::new()
        };
        assert_eq!(
            recovered.manifest.tracks[0].gaps, expected,
            "{step:?}: {kept} frames kept; the silence from frame 6000 must be marked"
        );
    }
}

#[test]
fn the_journal_line_is_durable_before_any_silence_can_reach_the_file() {
    // Stopped after the check: nothing was journaled or written.
    let (_root, folder, _) = gap_crashed_after(GapStep::Checked);
    let journal = read_journal(&folder.join("mic.events.jsonl")).unwrap();
    assert!(journal.gaps.is_empty());

    // Stopped after the journal line was written and synced: the line is
    // there, and not a frame of the silence is in the file.
    let (_root, folder, _) = gap_crashed_after(GapStep::JournalSynced);
    let journal = read_journal(&folder.join("mic.events.jsonl")).unwrap();
    assert_eq!(journal.gaps, vec![Gap::new(6000, 3000, GapReason::Overrun)]);
    let recovered = recover_folder(&folder).unwrap();
    assert_eq!(
        recovered.tracks[0].frames_kept, 4096,
        "only the block that was complete before the gap"
    );
    assert_eq!(
        (
            recovered.tracks[0].gaps_kept,
            recovered.tracks[0].gaps_dropped
        ),
        (0, 1),
        "a journalled gap whose silence never arrived is dropped"
    );
}

#[test]
fn a_crash_after_the_silence_is_written_but_before_the_writer_knew_of_it_is_still_marked() {
    for step in [GapStep::SilencePushed, GapStep::GapRecorded] {
        let (_root, folder, _) = gap_crashed_after(step);
        let recovered = recover_folder(&folder).unwrap();
        assert_eq!(recovered.tracks[0].frames_kept, 8192, "{step:?}");
        assert_eq!(
            recovered.manifest.tracks[0].gaps,
            vec![Gap::new(6000, 2192, GapReason::Overrun)],
            "{step:?}"
        );
    }
}

#[test]
fn a_gap_the_track_cannot_hold_is_refused_before_it_is_journaled_or_written() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut writer = store.create(mic_recording()).unwrap();
    let pcm = at_bits(&speechlike(1000, 52), 24);
    writer.push("mic", &to_f32(&pcm, 24)).unwrap();
    for length in [1u64 << 40, u64::MAX] {
        let error = writer
            .gap("mic", length, GapReason::Stall)
            .expect_err("too long for FLAC");
        assert_eq!(error.code(), "recording_unsupported_format", "{length}");
    }
    let journal = read_journal(&writer.folder().join("mic.events.jsonl")).unwrap();
    assert!(
        journal.gaps.is_empty(),
        "no line claims a gap that was refused"
    );

    // The track goes on exactly as if nothing had been asked.
    writer.push("mic", &to_f32(&pcm, 24)).unwrap();
    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    assert_eq!(manifest.tracks[0].frames, 2000);
    assert!(manifest.tracks[0].gaps.is_empty());
}

// ---- a gap that fails part-way, and a caller that goes on ----------------------------

/// Wraps an encoder so a test can make its silence or its sync fail.
struct FaultyEncoder {
    inner: Box<dyn TrackEncoder>,
    fail_silence: bool,
    fail_sync: bool,
    /// `finish` does its work and then reports a failure (a last fsync that
    /// fails): the track is complete on disk and the writer is told it is not.
    fail_finish: bool,
}

fn injected(action: &'static str) -> RecordingError {
    RecordingError::io(action, &std::io::Error::other("injected"))
}

impl TrackEncoder for FaultyEncoder {
    fn push(&mut self, interleaved: &[f32]) -> Result<(), RecordingError> {
        self.inner.push(interleaved)
    }
    fn push_silence(&mut self, frames: u64) -> Result<(), RecordingError> {
        if self.fail_silence {
            return Err(injected("write a track frame"));
        }
        self.inner.push_silence(frames)
    }
    fn ensure_room(&self, frames: u64) -> Result<(), RecordingError> {
        self.inner.ensure_room(frames)
    }
    fn frames_pushed(&self) -> u64 {
        self.inner.frames_pushed()
    }
    fn clipped_samples(&self) -> u64 {
        self.inner.clipped_samples()
    }
    fn bytes_written(&self) -> u64 {
        self.inner.bytes_written()
    }
    fn sync(&mut self) -> Result<(), RecordingError> {
        if self.fail_sync {
            return Err(injected("sync a track file"));
        }
        self.inner.sync()
    }
    fn salvage(&mut self, keep_frames: u64) -> Result<(), RecordingError> {
        self.inner.salvage(keep_frames)
    }
    fn finish(self: Box<Self>) -> Result<TrackSummary, RecordingError> {
        let fail = self.fail_finish;
        let summary = self.inner.finish()?;
        if fail {
            return Err(injected("sync a track file"));
        }
        Ok(summary)
    }
}

#[derive(Clone, Copy, Debug)]
enum GapFault {
    /// The journal line cannot be appended (nothing written).
    Append,
    /// Half of the journal line is written, then the append fails.
    TornAppend,
    /// The line is appended; fsyncing it fails.
    JournalSync,
    /// The intent is durable; the silence cannot be written.
    Silence,
    /// The gap is committed (silence and in-memory record); the final fsync of
    /// the track fails.
    FinalSync,
}

/// Make the next `gap` of `track` fail as `fault` says.
fn arm_gap_fault(track: &mut TrackWriter, fault: GapFault) {
    match fault {
        GapFault::Append => track.journal.fault = Some(JournalFault::AppendNothing),
        GapFault::TornAppend => track.journal.fault = Some(JournalFault::AppendTorn),
        GapFault::JournalSync => track.journal.fault = Some(JournalFault::SyncFails),
        GapFault::Silence | GapFault::FinalSync => {
            let inner = track.encoder.take().unwrap();
            track.encoder = Some(Box::new(FaultyEncoder {
                inner,
                fail_silence: matches!(fault, GapFault::Silence),
                fail_sync: matches!(fault, GapFault::FinalSync),
                fail_finish: false,
            }));
        }
    }
}

/// A writer with 6000 frames of speech whose next `gap` of 3000 frames fails
/// as `fault` says.
fn writer_whose_next_gap_fails(fault: GapFault) -> (TempDir, RecordingWriter, Vec<i32>) {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut writer = store.create(mic_recording()).unwrap();
    let first = at_bits(&speechlike(6000, 71), 24);
    writer.push("mic", &to_f32(&first, 24)).unwrap();
    arm_gap_fault(&mut writer.tracks[0], fault);
    (root, writer, first)
}

/// Every gap of a recovered track is silence in the audio that was kept.
fn assert_gaps_are_silence(folder: &Path, kept: usize, what: &str) {
    let file = std::fs::read(folder.join("mic.flac")).unwrap();
    let manifest = read_manifest(folder);
    if kept == 0 {
        return;
    }
    let decoded = decode_flac(&file);
    for gap in &manifest.tracks[0].gaps {
        let range = gap.at_frame as usize..(gap.at_frame + gap.length) as usize;
        assert!(
            decoded.samples[range.clone()].iter().all(|s| *s == 0),
            "{what}: the gap {range:?} covers audio that was captured"
        );
    }
}

#[test]
fn a_gap_that_fails_part_way_ends_the_track_until_recovery() {
    for fault in [
        GapFault::Append,
        GapFault::TornAppend,
        GapFault::JournalSync,
        GapFault::Silence,
    ] {
        let (_root, mut writer, _first) = writer_whose_next_gap_fails(fault);
        let error = writer.gap("mic", 3000, GapReason::Overrun).unwrap_err();
        assert_eq!(
            error.code(),
            "recording_io",
            "{fault:?}: the injected failure"
        );

        // The caller handled the error and goes on: none of it is accepted,
        // because the journal may hold an intent that audio written now would
        // fall inside.
        let more = to_f32(&at_bits(&speechlike(9000, 72), 24), 24);
        for (what, result) in [
            ("push", writer.push("mic", &more)),
            ("anchor", writer.anchor("mic", 9_000_000)),
            ("gap", writer.gap("mic", 10, GapReason::Stall)),
        ] {
            assert!(
                matches!(result, Err(RecordingError::TrackFinished)),
                "{fault:?}: {what} after a failed gap: {result:?}"
            );
        }
        // The healthy parts keep working, and the writer cannot end the
        // recording as if nothing had happened.
        writer.sync().unwrap();
        assert!(
            matches!(
                writer.finish_track("mic", EndedReason::Stopped),
                Err(RecordingError::TrackFinished)
            ),
            "{fault:?}"
        );
        let folder = writer.folder().to_path_buf();
        assert_eq!(read_manifest(&folder).state, RecordingState::Recording);
        drop(writer);

        // Recovery sees an intent that no audio contradicts, and the track
        // holds only what was captured before it.
        let recovered = recover_folder(&folder).unwrap();
        let kept = recovered.tracks[0].frames_kept as usize;
        assert!(kept <= 6000, "{fault:?}: {kept}");
        assert_gaps_are_silence(&folder, kept, &format!("{fault:?}"));
        assert!(recovered.manifest.tracks[0].gaps.is_empty(), "{fault:?}");
    }
}

#[test]
fn audio_pushed_after_a_failed_gap_is_never_called_silence_whatever_failed() {
    // The same, from the other end: let the caller push as soon as it likes
    // and look at what recovery says about every byte that was accepted.
    for fault in [
        GapFault::Append,
        GapFault::TornAppend,
        GapFault::JournalSync,
        GapFault::Silence,
    ] {
        let (_root, mut writer, _first) = writer_whose_next_gap_fails(fault);
        let _ = writer.gap("mic", 3000, GapReason::Overrun);
        let _ = writer.push("mic", &to_f32(&at_bits(&speechlike(9000, 73), 24), 24));
        let folder = writer.folder().to_path_buf();
        drop(writer);
        let recovered = recover_folder(&folder).unwrap();
        assert_gaps_are_silence(
            &folder,
            recovered.tracks[0].frames_kept as usize,
            &format!("{fault:?}"),
        );
    }
}

#[test]
fn a_failed_final_sync_leaves_a_committed_gap_and_the_track_goes_on() {
    let (_root, mut writer, first) = writer_whose_next_gap_fails(GapFault::FinalSync);
    let error = writer.gap("mic", 3000, GapReason::Overrun).unwrap_err();
    assert_eq!(error.code(), "recording_io", "the injected failure");
    // The silence is in the track and the gap is recorded: nothing about it
    // is in doubt, so the track is not ended and takes more audio.
    let later = at_bits(&speechlike(8000, 74), 24);
    writer.push("mic", &to_f32(&later, 24)).unwrap();
    writer.anchor("mic", 20_000_000).unwrap();
    let folder = writer.folder().to_path_buf();
    drop(writer);

    let recovered = recover_folder(&folder).unwrap();
    let kept = recovered.tracks[0].frames_kept as usize;
    assert_eq!(kept, 16_384, "6000 + 3000 + 8000 frames, four whole blocks");
    assert_eq!(
        recovered.manifest.tracks[0].gaps,
        vec![Gap::new(6000, 3000, GapReason::Overrun)]
    );
    let mut pcm = first;
    pcm.extend(std::iter::repeat_n(0, 3000));
    pcm.extend_from_slice(&later);
    let decoded = decode_flac(&std::fs::read(folder.join("mic.flac")).unwrap());
    assert!(
        decoded.samples == pcm[..kept],
        "the audio after the gap is untouched"
    );
}

// ---- what a live session needs (SAGA-563) ------------------------------------

#[test]
fn the_in_progress_manifest_is_padded_so_the_last_one_can_be_written_in_place() {
    let root = TempDir::new();
    let writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let on_disk = std::fs::read(writer.folder().join(MANIFEST_FILE)).unwrap();
    assert_eq!(on_disk.len(), MANIFEST_RESERVED_BYTES);
    // Padded, and still sealed: the seal covers the manifest, not the padding.
    assert_eq!(
        crate::recording::manifest_integrity(&on_disk),
        crate::recording::Integrity::Valid
    );
    assert_eq!(
        crate::recording::manifest_integrity(
            &std::fs::read(writer.folder().join(START_COPY_FILE)).unwrap()
        ),
        crate::recording::Integrity::Valid
    );
    // Trailing spaces are JSON whitespace: the manifest reads as before.
    assert_eq!(read_manifest(writer.folder()), *writer.manifest());
}

#[test]
fn an_anchor_names_the_stored_frame_it_belongs_to() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(recording("Anchors", vec![track("mic", 16_000, 1, 16)]))
        .unwrap();
    writer.push("mic", &[0.0; 1000]).unwrap();
    // The resampler holds frames back, so the frame is not "frames pushed".
    writer.anchor_at("mic", 5_000_000_000, 777).unwrap();
    writer.anchor("mic", 6_000_000_000).unwrap();
    let dir = writer.folder().to_path_buf();
    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    assert_eq!(
        manifest.tracks[0].clock.anchors,
        vec![(1_000_000, 0), (5_000_000_000, 777), (6_000_000_000, 1000)]
    );
    assert_eq!(
        read_journal(&dir.join("mic.events.jsonl")).unwrap().anchors,
        manifest.tracks[0].clock.anchors
    );
}

#[test]
fn a_track_that_cannot_be_written_any_more_is_cut_back_to_its_last_whole_block() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let signal = to_f32(&speechlike(10_240, 4), 24); // two and a half blocks
    writer.push("mic", &signal).unwrap();
    writer.gap("mic", 5_000, GapReason::Overrun).unwrap(); // 10 240 .. 15 240
    writer
        .push("mic", &to_f32(&speechlike(100, 5), 24))
        .unwrap();
    let dir = writer.folder().to_path_buf();

    // 15 340 frames were accepted: three whole blocks (12 288) reached the
    // file, the rest was waiting for a block.
    let kept = writer.abort_track("mic", EndedReason::DiskFull).unwrap();
    assert_eq!(kept, 12_288);
    assert!(writer.is_track_finished("mic"));
    assert!(matches!(
        writer.push("mic", &[0.0; 4]),
        Err(RecordingError::TrackFinished)
    ));
    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    let track = &manifest.tracks[0];
    assert_eq!(track.frames, 12_288);
    assert_eq!(track.ended_reason, Some(EndedReason::DiskFull));
    // The gap straddled the cut: the part inside is kept.
    assert_eq!(track.gaps.len(), 1);
    assert_eq!(
        (track.gaps[0].at_frame, track.gaps[0].length),
        (10_240, 2_048)
    );
    assert_eq!(manifest.state, RecordingState::Complete);

    let decoded = decode_flac(&std::fs::read(dir.join("mic.flac")).unwrap());
    assert_eq!(decoded.md5_verified, Some(true));
    assert_eq!(decoded.samples.len(), 12_288);
    assert!(decoded.samples[10_240..].iter().all(|s| *s == 0));
    assert!(!dir.join("mic.tail").exists(), "no space to copy a tail to");
    assert_eq!(
        track.audio_sha256.as_deref(),
        Some(
            sha256_hex(&std::fs::read(dir.join("mic.flac")).unwrap()[TRACK_HEADER_LEN..]).as_str()
        )
    );
}

#[test]
fn the_byte_counts_follow_the_files() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    assert_eq!(writer.bytes_written(), TRACK_HEADER_LEN as u64);
    writer
        .push("mic", &to_f32(&speechlike(8_192, 6), 24))
        .unwrap();
    assert!(writer.track_bytes("mic") > TRACK_HEADER_LEN as u64);
    assert_eq!(writer.bytes_written(), writer.track_bytes("mic"));
    assert_eq!(writer.frames_pushed("mic"), 8_192);
    let dir = writer.folder().to_path_buf();
    let expected = writer.bytes_written();
    writer.sync().unwrap();
    assert_eq!(
        std::fs::metadata(dir.join("mic.flac")).unwrap().len(),
        expected
    );
    assert_eq!(writer.track_bytes("nope"), 0);
}

/// Unix only: a read-only folder takes the place of a full disk, since either
/// way no second file can be created.
#[cfg(unix)]
mod no_room_for_a_second_file {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn can_create_in(dir: &Path) -> bool {
        let probe = dir.join("probe");
        let made = std::fs::File::create(&probe).is_ok();
        let _ = std::fs::remove_file(&probe);
        made
    }

    #[test]
    fn the_final_manifest_is_written_over_the_old_one_in_place() {
        let root = TempDir::new();
        let mut writer = RecordingStore::new(root.path())
            .create(mic_recording())
            .unwrap();
        writer
            .push("mic", &to_f32(&speechlike(5_000, 7), 24))
            .unwrap();
        let dir = writer.folder().to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let restricted = !can_create_in(&dir);
        let result = writer.finish(EndedReason::Stopped);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let manifest = result.expect("the manifest fits in the space it reserved");
        assert_eq!(manifest.state, RecordingState::Complete);
        assert_eq!(read_manifest(&dir), manifest);
        if restricted {
            // Written in place over the padded file, and its seal holds: the
            // finished recording is not mistaken for a torn one.
            let on_disk = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
            assert_eq!(on_disk.len(), MANIFEST_RESERVED_BYTES);
            assert_eq!(
                crate::recording::manifest_integrity(&on_disk),
                crate::recording::Integrity::Valid
            );
            assert!(matches!(
                crate::recording::recover_folder(&dir),
                Err(RecordingError::NotRecording)
            ));
        }
        if restricted {
            // It really was written in place: the reserved size is unchanged.
            assert_eq!(
                std::fs::metadata(dir.join(MANIFEST_FILE)).unwrap().len(),
                MANIFEST_RESERVED_BYTES as u64
            );
        }
    }

    #[test]
    fn when_even_that_fails_the_recording_stays_open_for_recovery() {
        let root = TempDir::new();
        let mut writer = RecordingStore::new(root.path())
            .create(mic_recording())
            .unwrap();
        writer
            .push("mic", &to_f32(&speechlike(5_000, 8), 24))
            .unwrap();
        let dir = writer.folder().to_path_buf();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let manifest_path = dir.join(MANIFEST_FILE);
        std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o400)).unwrap();
        let restricted = !can_create_in(&dir);
        let result = writer.finish(EndedReason::Stopped);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o600)).unwrap();
        if !restricted {
            return; // running as root: permissions do not apply
        }
        assert!(result.is_err());
        assert_eq!(read_manifest(&dir).state, RecordingState::Recording);
        // The writer is gone, so the lock is free and recovery can finish it.
        let recovered = crate::recording::recover_folder(&dir).unwrap();
        assert_eq!(recovered.manifest.state, RecordingState::Complete);
        // The tracks had been finished (the last block written) before the
        // manifest failed, so nothing is lost.
        assert_eq!(recovered.tracks[0].frames_kept, 5_000);
    }
}

// ---- Codex review of 1b -------------------------------------------------------

#[test]
fn a_full_disk_at_the_journal_sync_of_a_finished_track_still_gives_the_disk_full_manifest() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    writer
        .push("mic", &to_f32(&speechlike(6_000, 21), 24))
        .unwrap();
    writer.inject_journal_sync_failure("mic");
    // The last block was written; only the journal's fsync found no room.
    let error = writer
        .finish_track("mic", EndedReason::Stopped)
        .unwrap_err();
    assert!(error.is_disk_full(), "{error:?}");
    assert!(writer.is_track_finished("mic"));
    let dir = writer.folder().to_path_buf();
    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    let track = &manifest.tracks[0];
    assert_eq!(track.ended_reason, Some(EndedReason::DiskFull));
    assert_eq!(track.frames, 6_000, "the audio was complete");
    assert_eq!(manifest.state, RecordingState::Complete);
    let decoded = decode_flac(&std::fs::read(dir.join("mic.flac")).unwrap());
    assert_eq!(decoded.md5_verified, Some(true));
    assert_eq!(decoded.samples.len(), 6_000);
}

/// A rewrite of `recording.json` in place can be cut short anywhere. The
/// recording must stay recoverable at every offset (Codex finding 1).
#[test]
fn an_interrupted_in_place_manifest_write_leaves_a_recording_recovery_can_finish() {
    let root = TempDir::new();
    let mut writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    writer
        .push("mic", &to_f32(&speechlike(9_000, 22), 24))
        .unwrap();
    writer.sync().unwrap();
    let dir = writer.folder().to_path_buf();
    let original = std::fs::read(dir.join(MANIFEST_FILE)).unwrap();
    let mut finished = writer.manifest().clone();
    finished.state = RecordingState::Complete;
    finished.tracks[0].frames = 9_000;
    finished.tracks[0].ended_reason = Some(EndedReason::Stopped);
    let complete = serialize_manifest_sealed(&finished).unwrap();
    assert!(complete.len() < original.len());
    drop(writer); // the process "dies" in the middle of the rewrite

    // Missing only the final newline is not torn: the seal covers the
    // manifest, and the rewrite had written all of it.
    {
        let copy = copy_dir(&dir, &root.path().join("torn-newline"));
        let mut almost = original.clone();
        almost[..complete.len() - 1].copy_from_slice(&complete[..complete.len() - 1]);
        std::fs::write(copy.join(MANIFEST_FILE), &almost).unwrap();
        assert_eq!(
            crate::recording::manifest_integrity(&almost),
            crate::recording::Integrity::Valid
        );
        assert!(matches!(
            crate::recording::recover_folder(&copy),
            Err(RecordingError::NotRecording)
        ));
    }

    let json_len = complete.len();
    for cut in [
        0,
        1,
        2,
        17,
        json_len / 3,
        json_len / 2,
        json_len - 3,
        json_len - 2,
    ] {
        let copy = copy_dir(&dir, &root.path().join(format!("torn-{cut}")));
        let mut torn = original.clone();
        torn[..cut].copy_from_slice(&complete[..cut]);
        std::fs::write(copy.join(MANIFEST_FILE), &torn).unwrap();
        let recovered = crate::recording::recover_folder(&copy)
            .unwrap_or_else(|e| panic!("cut at {cut} of {json_len}: {e:?}"));
        assert_eq!(
            recovered.manifest.state,
            RecordingState::Complete,
            "cut {cut}"
        );
        assert!(recovered.manifest.recovered, "cut {cut}");
        assert_eq!(
            recovered.tracks[0].frames_kept, 8_192,
            "cut {cut}: whole blocks"
        );
        let on_disk = read_manifest(&copy);
        assert_eq!(on_disk.state, RecordingState::Complete);
        assert_eq!(on_disk.recording_id, recovered.manifest.recording_id);
        assert_eq!(
            on_disk.title, "Team meeting",
            "the start copy supplied the description"
        );
    }
}

#[test]
fn a_finished_recording_does_not_keep_the_start_copy() {
    let root = TempDir::new();
    let writer = RecordingStore::new(root.path())
        .create(mic_recording())
        .unwrap();
    let dir = writer.folder().to_path_buf();
    assert!(
        dir.join(START_COPY_FILE).is_file(),
        "written with the recording"
    );
    writer.finish(EndedReason::Stopped).unwrap();
    assert!(!dir.join(START_COPY_FILE).exists());
}

// ---- a track that ended early keeps what came before, and the others are finished ----

/// The faults after which the track is in doubt (the final fsync is not one).
const IN_DOUBT: [GapFault; 4] = [
    GapFault::Append,
    GapFault::TornAppend,
    GapFault::JournalSync,
    GapFault::Silence,
];

/// Two mono tracks, each with a partial block waiting for the rest of its
/// block: "mic" (6000 frames, 24 bit, one block and 1904 frames waiting),
/// whose next gap fails as `fault` says, and "system" (5000 frames, 16 bit,
/// 904 waiting), which is healthy. `mic_first` is their order in the recording
/// (the writer used to stop at the first track that did not finish).
fn two_tracks_one_failing(
    fault: GapFault,
    mic_first: bool,
) -> (TempDir, RecordingWriter, Vec<i32>, Vec<i32>) {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mic = track("mic", 48_000, 1, 24);
    let system = track("system", 48_000, 1, 16);
    let tracks = if mic_first {
        vec![mic, system]
    } else {
        vec![system, mic]
    };
    let mut writer = store.create(recording("Team meeting", tracks)).unwrap();
    let mic_pcm = at_bits(&speechlike(6000, 81), 24);
    let system_pcm = at_bits(&speechlike(5000, 82), 16);
    writer.push("mic", &to_f32(&mic_pcm, 24)).unwrap();
    writer.push("system", &to_f32(&system_pcm, 16)).unwrap();
    let mic_writer = writer.tracks.iter_mut().find(|t| t.id == "mic").unwrap();
    arm_gap_fault(mic_writer, fault);
    (root, writer, mic_pcm, system_pcm)
}

fn decoded_track(folder: &Path, file: &str) -> Vec<i32> {
    decode_flac(&std::fs::read(folder.join(file)).unwrap()).samples
}

#[test]
fn a_failed_gap_loses_none_of_the_audio_that_came_before_it_on_any_track() {
    for fault in IN_DOUBT {
        for mic_first in [true, false] {
            let what = format!("{fault:?}, mic first: {mic_first}");
            let (_root, mut writer, mic_pcm, system_pcm) = two_tracks_one_failing(fault, mic_first);
            writer.gap("mic", 3000, GapReason::Overrun).unwrap_err();

            // The healthy track goes on; then the session ends.
            let more = at_bits(&speechlike(700, 83), 16);
            writer.push("system", &to_f32(&more, 16)).unwrap();
            let folder = writer.folder().to_path_buf();
            let error = writer.finish(EndedReason::Stopped).unwrap_err();
            assert_eq!(error.code(), "recording_tracks_incomplete", "{what}");
            assert_eq!(read_manifest(&folder).state, RecordingState::Recording);

            let recovered = recover_folder(&folder).unwrap();
            let kept = |id: &str| {
                recovered
                    .tracks
                    .iter()
                    .find(|t| t.id == id)
                    .unwrap()
                    .frames_kept
            };
            // Not a frame of what was accepted before the gap is gone: not
            // the 1904 frames of the partial block, and not the 904 + 700 of
            // the other track.
            assert_eq!(kept("mic"), 6000, "{what}: the failed track");
            assert_eq!(kept("system"), 5700, "{what}: the other track");
            assert!(
                decoded_track(&folder, "mic.flac") == mic_pcm,
                "{what}: the failed track's audio"
            );
            let mut system_all = system_pcm;
            system_all.extend_from_slice(&more);
            assert!(
                decoded_track(&folder, "system.flac") == system_all,
                "{what}: the other track's audio"
            );
            // The disputed gap did not get any audio, or any silence.
            assert!(recovered.manifest.tracks.iter().all(|t| t.gaps.is_empty()));
            assert_eq!(recovered.manifest.state, RecordingState::Complete);
        }
    }
}

#[test]
fn finishing_ends_the_healthy_tracks_and_names_the_ones_that_are_incomplete() {
    for fault in IN_DOUBT {
        for mic_first in [true, false] {
            let what = format!("{fault:?}, mic first: {mic_first}");
            let (_root, mut writer, _mic, system_pcm) = two_tracks_one_failing(fault, mic_first);
            writer.gap("mic", 3000, GapReason::Overrun).unwrap_err();
            let folder = writer.folder().to_path_buf();
            let error = writer.finish(EndedReason::Stopped).unwrap_err();
            let message = error.to_string();
            assert!(
                message.contains("mic") && !message.contains("system"),
                "{what}: the message names the incomplete track and only it: {message}"
            );

            // The healthy track was finished by the writer itself: its header
            // is the final one, before any recovery ran.
            let bytes = std::fs::read(folder.join("system.flac")).unwrap();
            let header = parse_header(&bytes).expect("a header");
            assert_eq!(header.total_samples, 5000, "{what}");
            assert!(
                decode_flac(&bytes).samples == system_pcm,
                "{what}: the healthy track is whole"
            );
            // The manifest stays `recording`, the lock file stays, and the
            // journal of the failed track is still there for recovery.
            assert_eq!(read_manifest(&folder).state, RecordingState::Recording);
            assert!(folder.join(LOCK_FILE).exists(), "{what}");
        }
    }
}

#[test]
fn a_track_that_cannot_be_finished_does_not_keep_the_others_from_being_finished() {
    for mic_first in [true, false] {
        let (_root, mut writer, mic_pcm, system_pcm) =
            two_tracks_one_failing(GapFault::FinalSync, mic_first);
        // No gap this time: the mic track's own finish reports a failure after
        // it did its work (a last fsync that fails).
        let mic_writer = writer.tracks.iter_mut().find(|t| t.id == "mic").unwrap();
        let encoder = mic_writer.encoder.take().unwrap();
        mic_writer.encoder = Some(Box::new(FaultyEncoder {
            inner: encoder,
            fail_silence: false,
            fail_sync: false,
            fail_finish: true,
        }));
        let folder = writer.folder().to_path_buf();
        let error = writer.finish(EndedReason::Stopped).unwrap_err();
        assert_eq!(
            error.code(),
            "recording_tracks_incomplete",
            "mic first: {mic_first}"
        );
        assert!(error.to_string().contains("mic"), "{error}");
        assert!(!error.to_string().contains("system"), "{error}");

        let system = std::fs::read(folder.join("system.flac")).unwrap();
        assert_eq!(parse_header(&system).unwrap().total_samples, 5000);
        assert_eq!(read_manifest(&folder).state, RecordingState::Recording);

        // Recovery completes the recording and loses nothing.
        let recovered = recover_folder(&folder).unwrap();
        assert!(decoded_track(&folder, "mic.flac") == mic_pcm);
        assert!(decoded_track(&folder, "system.flac") == system_pcm);
        assert_eq!(recovered.manifest.state, RecordingState::Complete);
    }
}
