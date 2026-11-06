use std::io::Cursor;

use sagascript_testsupport::TempDir;

use super::*;
use crate::hash::sha256_hex;
use crate::recording::flac::format::{parse_header, BLOCK_SIZE, HEADER_LEN};
use crate::recording::flac::FlacEncoder;
use crate::recording::manifest::{EndedReason, GapReason};
use crate::recording::store::{NewRecording, RecordingWriter};
use crate::recording::test_signals::*;

/// A recording that a process left behind: written through the real writer
/// and dropped without `finish` (what a kill leaves, minus the bytes the
/// `BufWriter` still held).
struct Killed {
    _root: TempDir,
    store: RecordingStore,
    folder: PathBuf,
    /// Everything that was pushed, silence of gaps included.
    pcm: Vec<i32>,
}

fn mic() -> NewRecording {
    recording("Killed", vec![track("mic", 48_000, 1, 24)])
}

fn kill(request: NewRecording, push: impl FnOnce(&mut RecordingWriter) -> Vec<i32>) -> Killed {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut writer = store.create(request).unwrap();
    let folder = writer.folder().to_path_buf();
    let pcm = push(&mut writer);
    drop(writer);
    Killed {
        _root: root,
        store,
        folder,
        pcm,
    }
}

/// `blocks` whole blocks of mono audio pushed in uneven pieces.
fn killed_blocks(blocks: usize) -> Killed {
    kill(mic(), |writer| {
        let pcm = at_bits(&speechlike(blocks * BLOCK_SIZE, 91), 24);
        for piece in pcm.chunks(1237) {
            writer.push("mic", &to_f32(piece, 24)).unwrap();
        }
        pcm
    })
}

fn read_manifest(dir: &Path) -> Manifest {
    parse_manifest(&std::fs::read(dir.join(MANIFEST_FILE)).unwrap()).unwrap()
}

/// The frame lengths an independent in-memory encode of `pcm` gives.
fn oracle_frames(pcm: &[i32]) -> Vec<u32> {
    let mut encoder = FlacEncoder::new(Cursor::new(Vec::new()), 48_000, 1, 24)
        .unwrap()
        .with_frame_log();
    encoder.push_pcm(pcm).unwrap();
    encoder.finish_into_parts().unwrap().frame_lengths.unwrap()
}

/// Everything recovery promises about a recovered mono track made of the
/// first `kept` frames of `pcm`, checked with an independent decoder.
fn assert_recovered_track(folder: &Path, pcm: &[i32], kept: usize) {
    let file = std::fs::read(folder.join("mic.flac")).unwrap();
    let header = parse_header(&file).unwrap();
    assert_eq!(header.total_samples, kept as u64);
    assert_eq!(header.md5, pcm_md5(&pcm[..kept], 24, 1));
    assert_eq!((header.min_block_size, header.max_block_size), (4096, 4096));
    if kept > 0 {
        let decoded = decode_flac(&file);
        assert_eq!(decoded.errors, 0);
        assert!(
            decoded.samples == pcm[..kept],
            "the kept audio is the audio that was pushed"
        );
        assert_eq!(
            decoded.md5_verified,
            Some(true),
            "STREAMINFO's MD5 matches the kept audio"
        );
    }
    let manifest = read_manifest(folder);
    assert_eq!(
        manifest.tracks[0].audio_sha256.as_deref(),
        Some(sha256_hex(&file[HEADER_LEN..]).as_str())
    );
}

#[test]
fn a_killed_recording_is_finished_with_exactly_what_reached_the_disk() {
    // 6000 frames, an anchor, a 480-frame gap, 9000 frames, then a gap of
    // 1000 that carries the track past the fourth block boundary (16384).
    let killed = kill(mic(), |writer| {
        let first = at_bits(&speechlike(6000, 1), 24);
        let second = at_bits(&speechlike(9000, 2), 24);
        writer.push("mic", &to_f32(&first, 24)).unwrap();
        writer.anchor("mic", 5_000_000).unwrap();
        writer.gap("mic", 480, GapReason::Overrun).unwrap();
        writer.push("mic", &to_f32(&second, 24)).unwrap();
        writer.gap("mic", 1000, GapReason::Stall).unwrap();
        writer.anchor("mic", 99_000_000).unwrap();
        let mut pcm = first;
        pcm.extend(std::iter::repeat_n(0, 480));
        pcm.extend_from_slice(&second);
        pcm.extend(std::iter::repeat_n(0, 1000));
        pcm
    });
    assert_eq!(killed.pcm.len(), 16_480);

    let recovered = killed
        .store
        .recover(killed.folder.file_name().unwrap().to_str().unwrap())
        .unwrap();
    let track = &recovered.tracks[0];
    assert_eq!(
        track.frames_kept, 16_384,
        "four whole blocks; the 96 frames still waiting are lost"
    );
    assert_eq!((track.tail_bytes, track.tail_file.clone()), (0, None));
    assert_eq!((track.gaps_kept, track.gaps_dropped), (2, 0));

    let manifest = read_manifest(&killed.folder);
    assert_eq!(manifest, recovered.manifest);
    assert_eq!(manifest.state, RecordingState::Complete);
    assert!(manifest.recovered);
    let mic = &manifest.tracks[0];
    assert_eq!(mic.frames, 16_384);
    assert_eq!(mic.ended_reason, Some(EndedReason::Interrupted));
    assert_eq!(
        mic.gaps,
        vec![
            Gap::new(6000, 480, GapReason::Overrun),
            // The second gap ran to frame 16 480; only 904 frames of it were kept.
            Gap::new(15_480, 904, GapReason::Stall),
        ]
    );
    // The anchor at frame 16 480 is past the kept audio and is dropped.
    assert_eq!(mic.clock.anchors, vec![(1_000_000, 0), (5_000_000, 6000)]);
    assert_eq!(
        (mic.clock.domain.as_str(), mic.clock.start_ns),
        ("mach_continuous", 1_000_000)
    );

    assert_recovered_track(&killed.folder, &killed.pcm, 16_384);
    // The lock is gone, the journal and the audio are all still there.
    assert_eq!(
        names(&killed.folder),
        ["mic.events.jsonl", "mic.flac", "recording.json"]
    );
    // A SEEKTABLE with the first frame was written.
    let file = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    assert_eq!(file[42], 0x03, "a SEEKTABLE follows STREAMINFO");
}

#[test]
fn a_recovered_recording_is_not_recovered_again() {
    let killed = killed_blocks(3);
    killed
        .store
        .recover(killed.folder.file_name().unwrap().to_str().unwrap())
        .unwrap();
    let before = snapshot(&killed.folder);
    assert!(matches!(
        recover_folder(&killed.folder),
        Err(RecordingError::NotRecording)
    ));
    assert_eq!(snapshot(&killed.folder), before);
}

#[test]
fn a_recording_killed_before_any_audio_becomes_an_empty_valid_track() {
    let killed = kill(mic(), |_| Vec::new());
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, 0);
    assert_recovered_track(&killed.folder, &[], 0);
    assert_eq!(
        std::fs::metadata(killed.folder.join("mic.flac"))
            .unwrap()
            .len(),
        HEADER_LEN as u64
    );
}

#[test]
fn recovery_keeps_exactly_the_valid_frames_at_every_cut() {
    let killed = killed_blocks(8);
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let lengths = oracle_frames(&killed.pcm);
    assert_eq!(lengths.len(), 8);
    // The oracle (an independent encode) says where every frame ends on disk.
    let mut boundaries = vec![HEADER_LEN];
    for length in &lengths {
        boundaries.push(boundaries.last().unwrap() + *length as usize);
    }
    assert_eq!(
        *boundaries.last().unwrap(),
        original.len(),
        "the writer and the oracle agree"
    );
    assert!(original[HEADER_LEN..] == original[HEADER_LEN..boundaries[8]]);

    let mut cuts: Vec<usize> = Vec::new();
    for boundary in &boundaries {
        cuts.extend([boundary - 1, *boundary, boundary + 1]);
    }
    for pair in boundaries.windows(2) {
        cuts.push((pair[0] + pair[1]) / 2);
    }
    let mut rng = Rng::new(20_261_106);
    for _ in 0..60 {
        cuts.push(rng.range(HEADER_LEN as i64, original.len() as i64) as usize);
    }
    cuts.retain(|cut| (HEADER_LEN..=original.len()).contains(cut));
    cuts.sort_unstable();
    cuts.dedup();

    let scratch = TempDir::new();
    for cut in cuts {
        let folder = copy_dir(&killed.folder, &scratch.path().join(format!("cut-{cut}")));
        let file = folder.join("mic.flac");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(cut as u64)
            .unwrap();

        let recovered = recover_folder(&folder).unwrap();

        // The frames that lie entirely inside the first `cut` bytes.
        let kept_frames = boundaries.iter().filter(|b| **b <= cut).count() - 1;
        let end = boundaries[kept_frames];
        let report = &recovered.tracks[0];
        assert_eq!(
            report.frames_kept,
            (kept_frames * BLOCK_SIZE) as u64,
            "cut at {cut}"
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().len(),
            end as u64,
            "cut at {cut}"
        );
        if cut > end {
            // A torn frame: its bytes are kept, byte for byte.
            assert_eq!(report.tail_bytes, (cut - end) as u64, "cut at {cut}");
            assert_eq!(report.tail_file.as_deref(), Some("mic.tail"));
            assert!(
                std::fs::read(folder.join("mic.tail")).unwrap() == original[end..cut],
                "cut at {cut}"
            );
        } else {
            assert_eq!(
                (report.tail_bytes, report.tail_file.clone()),
                (0, None),
                "cut at {cut}"
            );
            assert!(!folder.join("mic.tail").exists());
        }
        assert_recovered_track(&folder, &killed.pcm, kept_frames * BLOCK_SIZE);
        // The frames that were kept are the bytes that were written.
        assert!(
            std::fs::read(&file).unwrap()[HEADER_LEN..] == original[HEADER_LEN..end],
            "cut at {cut}"
        );
    }
}

#[test]
fn a_cut_inside_the_header_starts_the_track_over_and_keeps_the_bytes() {
    let killed = killed_blocks(2);
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let scratch = TempDir::new();
    for cut in [0usize, 1, 3, 4, 20, 41, 42, 43, 100, 5000, HEADER_LEN - 1] {
        let folder = copy_dir(
            &killed.folder,
            &scratch.path().join(format!("header-{cut}")),
        );
        let file = folder.join("mic.flac");
        std::fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(cut as u64)
            .unwrap();

        let recovered = recover_folder(&folder).unwrap();
        assert_eq!(recovered.tracks[0].frames_kept, 0, "cut at {cut}");
        assert_eq!(recovered.tracks[0].tail_bytes, cut as u64);
        if cut > 0 {
            assert!(
                std::fs::read(folder.join("mic.tail")).unwrap() == original[..cut],
                "cut at {cut}"
            );
        } else {
            assert!(!folder.join("mic.tail").exists());
        }
        assert_eq!(std::fs::metadata(&file).unwrap().len(), HEADER_LEN as u64);
        assert_recovered_track(&folder, &killed.pcm, 0);
    }
}

#[test]
fn a_damaged_frame_in_the_middle_ends_the_audio_there_and_nothing_is_deleted() {
    let killed = killed_blocks(8);
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let lengths = oracle_frames(&killed.pcm);
    let start_of_fifth = HEADER_LEN + lengths[..4].iter().map(|l| *l as usize).sum::<usize>();
    let mut damaged = original.clone();
    damaged[start_of_fifth + 30] ^= 0x04;
    std::fs::write(killed.folder.join("mic.flac"), &damaged).unwrap();

    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, 4 * BLOCK_SIZE as u64);
    assert_recovered_track(&killed.folder, &killed.pcm, 4 * BLOCK_SIZE);
    // The damaged frame and the good frames after it are all in the tail.
    let tail = std::fs::read(killed.folder.join("mic.tail")).unwrap();
    assert!(
        tail == damaged[start_of_fifth..],
        "the tail is the rest of the file, as it was"
    );
    let kept = std::fs::metadata(killed.folder.join("mic.flac"))
        .unwrap()
        .len() as usize;
    assert_eq!(
        kept + tail.len(),
        original.len(),
        "every byte is still somewhere"
    );
}

#[test]
fn an_existing_tail_file_is_never_overwritten() {
    let killed = killed_blocks(3);
    let file = killed.folder.join("mic.flac");
    let length = std::fs::metadata(&file).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(length - 5)
        .unwrap();
    std::fs::write(killed.folder.join("mic.tail"), b"an earlier tail").unwrap();

    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].tail_file.as_deref(), Some("mic.tail.1"));
    assert_eq!(
        std::fs::read(killed.folder.join("mic.tail")).unwrap(),
        b"an earlier tail"
    );
    assert_eq!(recovered.tracks[0].frames_kept, 2 * BLOCK_SIZE as u64);
}

#[test]
fn a_missing_track_file_becomes_an_empty_valid_track() {
    let killed = killed_blocks(1);
    std::fs::remove_file(killed.folder.join("mic.flac")).unwrap();
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, 0);
    assert_recovered_track(&killed.folder, &[], 0);
}

#[test]
fn a_recording_that_another_handle_owns_is_not_touched() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut writer = store.create(mic()).unwrap();
    writer
        .push(
            "mic",
            &to_f32(&at_bits(&speechlike(3 * BLOCK_SIZE, 3), 24), 24),
        )
        .unwrap();
    writer.sync().unwrap();
    let folder = writer.folder().to_path_buf();
    let before = snapshot(&folder);

    assert!(matches!(
        recover_folder(&folder),
        Err(RecordingError::Locked)
    ));
    let reports = store.recover_pending().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].outcome, RecoveryOutcome::SkippedLocked);
    assert_eq!(snapshot(&folder), before, "not one byte changed");

    // The owner goes on writing and finishes normally.
    writer
        .push("mic", &to_f32(&at_bits(&speechlike(100, 4), 24), 24))
        .unwrap();
    let manifest = writer.finish(EndedReason::Stopped).unwrap();
    assert!(!manifest.recovered);
    assert_eq!(manifest.tracks[0].frames, 3 * BLOCK_SIZE as u64 + 100);
}

#[test]
fn recovery_takes_over_when_the_owner_is_gone() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let writer = store.create(mic()).unwrap();
    let folder = writer.folder().to_path_buf();
    assert!(matches!(
        recover_folder(&folder),
        Err(RecordingError::Locked)
    ));
    drop(writer);
    assert!(recover_folder(&folder).is_ok());
}

#[test]
fn a_finished_recording_is_left_alone_without_taking_its_lock() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut writer = store.create(mic()).unwrap();
    writer
        .push("mic", &to_f32(&at_bits(&speechlike(5000, 5), 24), 24))
        .unwrap();
    let folder = writer.folder().to_path_buf();
    writer.finish(EndedReason::Stopped).unwrap();
    let before = snapshot(&folder);

    assert!(matches!(
        recover_folder(&folder),
        Err(RecordingError::NotRecording)
    ));
    let reports = store.recover_pending().unwrap();
    assert_eq!(reports[0].outcome, RecoveryOutcome::SkippedNotRecording);
    assert_eq!(snapshot(&folder), before);
    assert!(!folder.join(LOCK_FILE).exists(), "no lock file was created");
}

#[test]
fn a_manifest_recovery_cannot_read_is_reported_and_left_as_it_is() {
    let killed = killed_blocks(2);
    // The copy of the start manifest would rescue this (see store_tests); a
    // folder without it, like one an older build left, is what this is about.
    std::fs::remove_file(killed.folder.join(START_COPY_FILE)).unwrap();
    let before = snapshot(&killed.folder);

    let manifest_path = killed.folder.join(MANIFEST_FILE);
    std::fs::write(&manifest_path, b"{ torn").unwrap();
    let reports = killed.store.recover_pending().unwrap();
    assert_eq!(
        reports[0].outcome,
        RecoveryOutcome::Failed {
            code: "recording_manifest_invalid"
        }
    );

    let mut value: serde_json::Value =
        serde_json::from_slice(&before.iter().find(|f| f.0 == MANIFEST_FILE).unwrap().1).unwrap();
    value["min_reader_version"] = serde_json::json!(99);
    std::fs::write(&manifest_path, value.to_string()).unwrap();
    let reports = killed.store.recover_pending().unwrap();
    assert_eq!(
        reports[0].outcome,
        RecoveryOutcome::Failed {
            code: "recording_manifest_too_new"
        }
    );
    // Nothing but the manifest we replaced differs.
    let after = snapshot(&killed.folder);
    assert_eq!(
        after
            .iter()
            .filter(|f| f.0 != MANIFEST_FILE)
            .collect::<Vec<_>>(),
        before
            .iter()
            .filter(|f| f.0 != MANIFEST_FILE)
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_track_file_that_is_not_ours_is_left_alone_and_the_recording_stays_open() {
    for content in [&b"RIFF....WAVEfmt this is not flac at all"[..], &[0u8; 100]] {
        let killed = killed_blocks(2);
        std::fs::write(killed.folder.join("mic.flac"), content).unwrap();
        let before = snapshot(&killed.folder);
        let result = recover_folder(&killed.folder);
        assert!(
            matches!(result, Err(RecordingError::CorruptTrack { .. })),
            "{result:?}"
        );
        assert_eq!(snapshot(&killed.folder), before, "nothing was changed");
        assert_eq!(
            read_manifest(&killed.folder).state,
            RecordingState::Recording
        );
    }
}

#[test]
fn a_track_file_that_does_not_match_the_manifest_is_refused() {
    let killed = killed_blocks(2);
    let manifest_path = killed.folder.join(MANIFEST_FILE);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    value["tracks"][0]["sample_rate"] = serde_json::json!(44_100);
    std::fs::write(&manifest_path, value.to_string()).unwrap();
    let before = snapshot(&killed.folder);
    assert!(matches!(
        recover_folder(&killed.folder),
        Err(RecordingError::CorruptTrack { .. })
    ));
    assert_eq!(snapshot(&killed.folder), before);
}

#[test]
fn a_missing_or_torn_journal_costs_the_gaps_not_the_audio() {
    let make = || {
        kill(mic(), |writer| {
            // The gap starts 50 frames before the second block ends and runs on.
            let pcm = at_bits(&speechlike(2 * BLOCK_SIZE - 50, 6), 24);
            writer.push("mic", &to_f32(&pcm, 24)).unwrap();
            writer.gap("mic", 100, GapReason::Overrun).unwrap();
            let mut all = pcm;
            all.extend(std::iter::repeat_n(0, 100));
            all
        })
    };

    let missing = make();
    std::fs::remove_file(missing.folder.join("mic.events.jsonl")).unwrap();
    let recovered = recover_folder(&missing.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, 2 * BLOCK_SIZE as u64);
    assert!(recovered.manifest.tracks[0].gaps.is_empty());
    assert_eq!(
        recovered.manifest.tracks[0].clock.anchors,
        vec![(1_000_000, 0)],
        "the manifest's own start"
    );

    let torn = make();
    let journal = torn.folder.join("mic.events.jsonl");
    let mut text = std::fs::read(&journal).unwrap();
    text.extend_from_slice(b"{\"t\":\"gap\",\"at_frame\":5,\"len");
    std::fs::write(&journal, text).unwrap();
    let recovered = recover_folder(&torn.folder).unwrap();
    assert_eq!(recovered.tracks[0].journal_lines_ignored, 1);
    assert_eq!(
        recovered.tracks[0].gaps_kept, 1,
        "the complete gap line survived"
    );
    assert_eq!(
        recovered.manifest.tracks[0].gaps,
        vec![Gap::new(2 * BLOCK_SIZE as u64 - 50, 50, GapReason::Overrun)]
    );
}

#[test]
fn a_gap_after_the_kept_audio_is_dropped_and_counted() {
    // One block is on disk; the journal knows of a gap far beyond it.
    let killed = kill(mic(), |writer| {
        let pcm = at_bits(&speechlike(BLOCK_SIZE, 7), 24);
        writer.push("mic", &to_f32(&pcm, 24)).unwrap();
        writer.sync().unwrap();
        pcm
    });
    let journal = killed.folder.join("mic.events.jsonl");
    let mut text = std::fs::read_to_string(&journal).unwrap();
    text.push_str("{\"t\":\"gap\",\"at_frame\":999999,\"length\":50,\"reason\":\"stall\"}\n");
    std::fs::write(&journal, text).unwrap();
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(
        (
            recovered.tracks[0].gaps_kept,
            recovered.tracks[0].gaps_dropped
        ),
        (0, 1)
    );
}

#[test]
fn the_scan_of_a_store_recovers_every_unfinished_folder_in_name_order() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    let mut folders = Vec::new();
    for title in ["Bravo", "Alpha"] {
        let mut writer = store
            .create(recording(title, vec![track("mic", 48_000, 1, 24)]))
            .unwrap();
        writer
            .push("mic", &to_f32(&at_bits(&speechlike(5000, 8), 24), 24))
            .unwrap();
        folders.push(writer.folder().to_path_buf());
    }
    // A finished one, a folder without a manifest, a plain file, a bad one.
    let done = store
        .create(recording("Charlie", vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    done.finish(EndedReason::Stopped).unwrap();
    std::fs::create_dir(root.path().join("empty folder")).unwrap();
    std::fs::write(root.path().join("notes.txt"), b"hello").unwrap();
    let bad = store
        .create(recording("Delta", vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    let bad_folder = bad.folder().to_path_buf();
    drop(bad);
    std::fs::write(bad_folder.join(MANIFEST_FILE), b"nope").unwrap();
    std::fs::remove_file(bad_folder.join(START_COPY_FILE)).unwrap();

    let reports = store.recover_pending().unwrap();
    let summary: Vec<(String, &str)> = reports
        .iter()
        .map(|r| {
            let kind = match &r.outcome {
                RecoveryOutcome::Recovered(_) => "recovered",
                RecoveryOutcome::SkippedLocked => "locked",
                RecoveryOutcome::SkippedNotRecording => "done",
                RecoveryOutcome::Failed { .. } => "failed",
            };
            (r.folder.split(' ').nth(2).unwrap().to_string(), kind)
        })
        .collect();
    // Folder names start with the same minute, so the order is the titles'.
    assert_eq!(
        summary,
        vec![
            ("Alpha".to_string(), "recovered"),
            ("Bravo".to_string(), "recovered"),
            ("Charlie".to_string(), "done"),
            ("Delta".to_string(), "failed"),
        ]
    );
    for folder in &folders {
        assert_eq!(read_manifest(folder).state, RecordingState::Complete);
    }
    // A second scan has nothing to recover.
    assert!(store
        .recover_pending()
        .unwrap()
        .iter()
        .all(|r| !matches!(r.outcome, RecoveryOutcome::Recovered(_))));
    assert!(RecordingStore::new(root.path().join("missing"))
        .recover_pending()
        .unwrap()
        .is_empty());
}

#[test]
fn recovery_refuses_names_that_are_not_plain() {
    let root = TempDir::new();
    let store = RecordingStore::new(root.path());
    for name in ["../x", "a/b", "..", ""] {
        assert!(
            matches!(
                store.recover(name),
                Err(RecordingError::InvalidInput { .. })
            ),
            "{name:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_never_followed() {
    let killed = killed_blocks(1);
    let links = TempDir::new();
    let link = links.path().join("link");
    std::os::unix::fs::symlink(&killed.folder, &link).unwrap();
    assert!(matches!(
        recover_folder(&link),
        Err(RecordingError::InvalidInput { .. })
    ));
    let reports = RecordingStore::new(links.path()).recover_pending().unwrap();
    assert!(
        reports.is_empty(),
        "a link in the store is not a recording folder"
    );
    assert_eq!(
        read_manifest(&killed.folder).state,
        RecordingState::Recording
    );
}

#[test]
fn stereo_and_compact_recordings_recover_too() {
    let mut compact = track("mic", 16_000, 2, 16);
    compact.resampled_from = Some(48_000);
    let mut request = recording("Compact stereo", vec![compact]);
    request.quality = crate::recording::manifest::Quality::Compact;
    let killed = kill(request, |writer| {
        let first = at_bits(&speechlike(2 * BLOCK_SIZE + 321, 9), 16);
        let second = second_channel(&first);
        let pcm = interleave(&[&first, &second]);
        writer.push("mic", &to_f32(&pcm, 16)).unwrap();
        pcm
    });
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, 2 * BLOCK_SIZE as u64);
    let file = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let decoded = decode_flac(&file);
    assert_eq!((decoded.sample_rate, decoded.channels), (16_000, 2));
    assert!(decoded.samples == killed.pcm[..2 * BLOCK_SIZE * 2]);
    assert_eq!(decoded.md5_verified, Some(true));
    let track = &recovered.manifest.tracks[0];
    assert_eq!(
        (track.lossless, track.resampled_from),
        (false, Some(48_000))
    );
}

#[test]
fn two_tracks_are_recovered_independently() {
    let killed = kill(
        recording(
            "Two",
            vec![track("mic", 48_000, 1, 24), track("system", 44_100, 1, 24)],
        ),
        |writer| {
            let long = at_bits(&speechlike(3 * BLOCK_SIZE + 5, 10), 24);
            let short = at_bits(&speechlike(BLOCK_SIZE + 5, 11), 24);
            writer.push("mic", &to_f32(&long, 24)).unwrap();
            writer.push("system", &to_f32(&short, 24)).unwrap();
            long
        },
    );
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(
        recovered
            .tracks
            .iter()
            .map(|t| (t.id.as_str(), t.frames_kept))
            .collect::<Vec<_>>(),
        [
            ("mic", 3 * BLOCK_SIZE as u64),
            ("system", BLOCK_SIZE as u64)
        ]
    );
    assert_eq!(recovered.manifest.tracks[1].clock.domain, "mach_continuous");
}

// ---- the clipped-sample count of a recovered track ---------------------------------

/// `blocks` blocks of quiet speech as f32, with `loud` samples (index, value)
/// replaced by out-of-range values.
fn quiet_block(seed: u64, loud: &[(usize, f32)]) -> Vec<f32> {
    let mut samples = to_f32(&at_bits(&speechlike(BLOCK_SIZE, seed), 24), 24);
    for (index, value) in loud {
        samples[*index] = *value;
    }
    samples
}

#[test]
fn clipping_in_audio_that_was_lost_is_not_reported_for_the_audio_that_was_kept() {
    // One clean block reaches the disk. 100 more frames, three of them
    // clipped, wait for a block; an anchor after them is journalled with a
    // count of 3. The process dies: the 100 frames are gone, the anchor line
    // survives, and no clipped sample is in the kept audio.
    let killed = kill(mic(), |writer| {
        writer.push("mic", &quiet_block(60, &[])).unwrap();
        let mut tail = vec![0.0f32; 100];
        tail[10] = 2.0;
        tail[20] = -3.0;
        tail[30] = 1.5;
        writer.push("mic", &tail).unwrap();
        writer.anchor("mic", 9_000_000).unwrap();
        Vec::new()
    });
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, BLOCK_SIZE as u64);
    assert_eq!(
        recovered.manifest.tracks[0].clipped_samples, 0,
        "the clipped samples were in the lost 100 frames"
    );
}

#[test]
fn clipping_in_the_kept_audio_is_counted_up_to_the_last_event_inside_it() {
    // Five clipped samples in the first block, an anchor at its end (count 5),
    // then two more clipped samples in frames that are lost, another anchor
    // (count 7). The kept audio holds the five, not the seven.
    let killed = kill(mic(), |writer| {
        writer
            .push(
                "mic",
                &quiet_block(
                    61,
                    &[(10, 1.5), (11, -2.0), (12, 9.0), (13, 1.0), (14, f32::NAN)],
                ),
            )
            .unwrap();
        writer.anchor("mic", 5_000_000).unwrap();
        let mut more = vec![0.0f32; 200];
        more[5] = 4.0;
        more[6] = -4.0;
        writer.push("mic", &more).unwrap();
        writer.anchor("mic", 6_000_000).unwrap();
        Vec::new()
    });
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, BLOCK_SIZE as u64);
    assert_eq!(recovered.manifest.tracks[0].clipped_samples, 5);
}

#[test]
fn a_gap_event_carries_the_clipping_that_came_before_it() {
    let killed = kill(mic(), |writer| {
        writer
            .push("mic", &quiet_block(62, &[(100, 3.0), (200, -3.0)]))
            .unwrap();
        // The gap starts at frame 4096 (inside the kept audio's end) and
        // runs 5000 frames, so a second block is kept.
        writer.gap("mic", 5000, GapReason::Stall).unwrap();
        Vec::new()
    });
    let recovered = recover_folder(&killed.folder).unwrap();
    assert_eq!(recovered.tracks[0].frames_kept, 2 * BLOCK_SIZE as u64);
    assert_eq!(recovered.manifest.tracks[0].clipped_samples, 2);
}

#[test]
fn a_journal_with_too_many_events_stops_recovery_before_anything_is_changed() {
    let killed = killed_blocks(3);
    // Cut the file inside its last frame so recovery would have work to do.
    let file = killed.folder.join("mic.flac");
    let length = std::fs::metadata(&file).unwrap().len();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&file)
        .unwrap()
        .set_len(length - 7)
        .unwrap();
    let journal = killed.folder.join("mic.events.jsonl");
    let mut text = std::fs::read_to_string(&journal).unwrap();
    for frame in 0..crate::recording::journal::MAX_JOURNAL_EVENTS + 1 {
        text.push_str(&format!(
            "{{\"t\":\"anchor\",\"host_ns\":1,\"frame\":{frame}}}\n"
        ));
    }
    std::fs::write(&journal, text).unwrap();
    let before = snapshot(&killed.folder);

    let error = recover_folder(&killed.folder).unwrap_err();
    assert_eq!(error.code(), "recording_journal_too_large");
    assert_eq!(snapshot(&killed.folder), before, "no byte was changed");
    assert_eq!(
        read_manifest(&killed.folder).state,
        RecordingState::Recording,
        "the recording is not marked complete with events missing"
    );
    let reports = killed.store.recover_pending().unwrap();
    assert_eq!(
        reports[0].outcome,
        RecoveryOutcome::Failed {
            code: "recording_journal_too_large"
        }
    );
}

// ---- links and other things that are not plain files ---------------------------------

/// Run `recover_folder` on its own thread so a recovery that blocks (on a
/// pipe, say) fails the test instead of hanging it.
#[cfg(unix)]
fn recover_within_a_deadline(folder: &Path) -> Result<RecoveredRecording, RecordingError> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let folder = folder.to_path_buf();
    std::thread::spawn(move || {
        let _ = sender.send(recover_folder(&folder));
    });
    receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("recovery returned instead of blocking")
}

#[cfg(unix)]
#[test]
fn a_track_file_that_is_a_link_is_never_followed_or_written_through() {
    let killed = killed_blocks(3);
    let outside = TempDir::new();
    // The victim is another FLAC file that recovery would truncate and
    // rewrite the header of if it followed the link.
    let victim = outside.path().join("someone-elses.flac");
    let mut bytes = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    bytes.extend_from_slice(b"trailing bytes the owner of this file cares about");
    std::fs::write(&victim, &bytes).unwrap();
    std::fs::remove_file(killed.folder.join("mic.flac")).unwrap();
    std::os::unix::fs::symlink(&victim, killed.folder.join("mic.flac")).unwrap();
    let before = snapshot(&killed.folder);

    let error = recover_folder(&killed.folder).unwrap_err();
    assert_eq!(error.code(), "recording_not_regular_file", "{error:?}");
    assert!(
        std::fs::read(&victim).unwrap() == bytes,
        "the file behind the link is untouched"
    );
    assert_eq!(
        snapshot(&killed.folder).len(),
        before.len(),
        "nothing was added to the folder"
    );
    assert_eq!(
        read_manifest(&killed.folder).state,
        RecordingState::Recording
    );

    // A link to nothing is refused the same way, not "created through".
    std::fs::remove_file(killed.folder.join("mic.flac")).unwrap();
    let missing = outside.path().join("not-there.flac");
    std::os::unix::fs::symlink(&missing, killed.folder.join("mic.flac")).unwrap();
    let error = recover_folder(&killed.folder).unwrap_err();
    assert_eq!(error.code(), "recording_not_regular_file", "{error:?}");
    assert!(!missing.exists(), "nothing was created behind the link");
}

#[cfg(unix)]
#[test]
fn the_manifest_the_journal_and_the_lock_are_not_followed_either() {
    for member in ["recording.json", "mic.events.jsonl", "recording.lock"] {
        let killed = killed_blocks(2);
        let outside = TempDir::new();
        let target = outside.path().join("target");
        // The target is a copy of the real file, so a followed link would
        // work out and the recovery would "succeed" through it.
        let path = killed.folder.join(member);
        std::fs::copy(&path, &target).unwrap();
        let target_before = std::fs::read(&target).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let result = recover_folder(&killed.folder);
        let error = result.expect_err(member);
        assert_eq!(error.code(), "recording_not_regular_file", "{member}");
        assert!(
            std::fs::read(&target).unwrap() == target_before,
            "{member}: the file behind the link is untouched"
        );
        let mic = std::fs::read(killed.folder.join("mic.flac")).unwrap();
        assert!(
            parse_header(&mic).unwrap().total_samples == 0,
            "{member}: the track was not finalised"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_directory_or_a_pipe_in_place_of_a_file_is_refused_without_blocking() {
    // A directory where the journal should be.
    let killed = killed_blocks(2);
    std::fs::remove_file(killed.folder.join("mic.events.jsonl")).unwrap();
    std::fs::create_dir(killed.folder.join("mic.events.jsonl")).unwrap();
    let error = recover_within_a_deadline(&killed.folder).unwrap_err();
    assert_eq!(error.code(), "recording_not_regular_file", "{error:?}");

    // A pipe where the journal and where the track should be: opening one for
    // reading would wait for a writer that never comes.
    for member in ["mic.events.jsonl", "mic.flac"] {
        let killed = killed_blocks(2);
        let path = killed.folder.join(member);
        std::fs::remove_file(&path).unwrap();
        let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let error = recover_within_a_deadline(&killed.folder).unwrap_err();
        assert_eq!(error.code(), "recording_not_regular_file", "{member}");
    }
}

#[cfg(unix)]
#[test]
fn a_manifest_link_in_the_store_is_reported_not_skipped() {
    let killed = killed_blocks(1);
    let outside = TempDir::new();
    let target = outside.path().join("m.json");
    std::fs::copy(killed.folder.join(MANIFEST_FILE), &target).unwrap();
    std::fs::remove_file(killed.folder.join(MANIFEST_FILE)).unwrap();
    std::os::unix::fs::symlink(&target, killed.folder.join(MANIFEST_FILE)).unwrap();
    let reports = killed.store.recover_pending().unwrap();
    assert_eq!(reports.len(), 1);
    assert_eq!(
        reports[0].outcome,
        RecoveryOutcome::Failed {
            code: "recording_not_regular_file"
        }
    );
}

// ---- a damaged track header ----------------------------------------------------------

/// The size of the padding block header: where a damaged length is planted.
const PADDING_HEADER: std::ops::Range<usize> = 42..46;

fn write_track(folder: &Path, bytes: &[u8]) {
    std::fs::write(folder.join("mic.flac"), bytes).unwrap();
}

/// A recovery of `bytes` as the track file of a copy of `killed`.
fn recover_with_track(
    killed: &Killed,
    scratch: &TempDir,
    name: &str,
    bytes: &[u8],
) -> (PathBuf, Result<RecoveredRecording, RecordingError>) {
    let folder = copy_dir(&killed.folder, &scratch.path().join(name));
    write_track(&folder, bytes);
    let result = recover_folder(&folder);
    (folder, result)
}

/// Everything a recovery that kept `frames` frames of `killed` must show.
fn assert_all_audio_kept(
    folder: &Path,
    killed: &Killed,
    original: &[u8],
    frames: usize,
    what: &str,
) {
    let file = std::fs::read(folder.join("mic.flac")).unwrap();
    assert!(
        file[HEADER_LEN..] == original[HEADER_LEN..],
        "{what}: not one byte of audio changed"
    );
    assert_recovered_track(folder, &killed.pcm, frames);
    assert!(
        !folder.join("mic.tail").exists(),
        "{what}: nothing was set aside"
    );
}

#[test]
fn a_damaged_metadata_header_in_front_of_good_frames_costs_no_audio() {
    let killed = killed_blocks(8);
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let scratch = TempDir::new();

    let planted = |range: std::ops::Range<usize>, bytes: &[u8]| {
        let mut file = original.clone();
        file[range].copy_from_slice(bytes);
        file
    };
    let mut garbage = original.clone();
    let mut rng = Rng::new(31);
    for byte in &mut garbage[..HEADER_LEN] {
        *byte = rng.range(0, 256) as u8;
    }
    let mut zeroed = original.clone();
    zeroed[..HEADER_LEN].fill(0);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        // The padding block claims 16 MiB: the header runs past the file.
        (
            "huge padding, not last",
            planted(PADDING_HEADER, &[0x01, 0xFF, 0xFF, 0xFF]),
        ),
        (
            "huge padding, last",
            planted(PADDING_HEADER, &[0x81, 0xFF, 0xFF, 0xFF]),
        ),
        // The last-block flag is gone: the header would run into the frames.
        ("no last flag", planted(42..43, &[0x01])),
        // The padding is shorter than it should be: the "header" ends early.
        (
            "short padding",
            planted(PADDING_HEADER, &[0x81, 0x00, 0x00, 0x10]),
        ),
        ("magic gone", planted(0..4, &[0, 0, 0, 0])),
        ("streaminfo header gone", planted(4..8, &[0xFF; 4])),
        ("everything zero", zeroed),
        ("random bytes", garbage),
    ];
    for (what, bytes) in cases {
        let (folder, result) = recover_with_track(&killed, &scratch, what, &bytes);
        let recovered = result.unwrap_or_else(|error| panic!("{what}: {error:?}"));
        let report = &recovered.tracks[0];
        assert_eq!(report.frames_kept, 8 * BLOCK_SIZE as u64, "{what}");
        assert_eq!(
            (report.tail_bytes, report.tail_file.clone()),
            (0, None),
            "{what}"
        );
        assert!(report.header_rebuilt, "{what}");
        assert_all_audio_kept(&folder, &killed, &original, 8 * BLOCK_SIZE, what);
    }
}

#[test]
fn a_torn_rewrite_of_the_header_at_any_sector_boundary_costs_no_audio() {
    let killed = killed_blocks(6);
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let placeholder = original[..HEADER_LEN].to_vec();
    // The header a finished track carries: what recovery of an intact copy writes.
    let scratch = TempDir::new();
    let (reference, result) = recover_with_track(&killed, &scratch, "reference", &original);
    result.unwrap();
    let finished = std::fs::read(reference.join("mic.flac")).unwrap()[..HEADER_LEN].to_vec();
    assert_ne!(placeholder, finished);

    for cut in [
        1usize,
        4,
        41,
        42,
        43,
        46,
        100,
        512,
        4096,
        8192,
        20_000,
        36_864,
        HEADER_LEN - 1,
    ] {
        // The new header reached the disk up to `cut`, or only from `cut` on.
        for (what, header) in [
            (
                "new then old",
                [&finished[..cut], &placeholder[cut..]].concat(),
            ),
            (
                "old then new",
                [&placeholder[..cut], &finished[cut..]].concat(),
            ),
        ] {
            let mut file = header;
            file.extend_from_slice(&original[HEADER_LEN..]);
            let name = format!("{what} {cut}");
            let (folder, result) = recover_with_track(&killed, &scratch, &name, &file);
            let recovered = result.unwrap_or_else(|error| panic!("{name}: {error:?}"));
            assert_eq!(
                recovered.tracks[0].frames_kept,
                6 * BLOCK_SIZE as u64,
                "{name}"
            );
            assert_all_audio_kept(&folder, &killed, &original, 6 * BLOCK_SIZE, &name);
            // And the header is the finished one, whatever the mixture was.
            assert!(
                std::fs::read(folder.join("mic.flac")).unwrap()[..HEADER_LEN] == finished[..],
                "{name}"
            );
        }
    }
}

#[test]
fn a_damaged_header_in_a_file_larger_than_the_header_read_limit_is_recovered_too() {
    // 100 blocks of noise are stored verbatim: 1.2 MB, past 1 MiB.
    let killed = kill(mic(), |writer| {
        let pcm = edge_signal(Edge::WhiteFullScale, 100 * BLOCK_SIZE);
        writer.push("mic", &to_f32(&pcm, 24)).unwrap();
        pcm
    });
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    assert!(original.len() > 1024 * 1024, "{}", original.len());
    let mut damaged = original.clone();
    damaged[PADDING_HEADER].copy_from_slice(&[0x01, 0xFF, 0xFF, 0xFF]);
    let scratch = TempDir::new();
    let (folder, result) = recover_with_track(&killed, &scratch, "big", &damaged);
    let recovered = result.expect("recovered");
    assert_eq!(recovered.tracks[0].frames_kept, 100 * BLOCK_SIZE as u64);
    assert_all_audio_kept(&folder, &killed, &original, 100 * BLOCK_SIZE, "big");
}

#[test]
fn an_empty_track_with_a_damaged_header_stays_an_empty_track_and_nothing_is_set_aside() {
    let killed = kill(mic(), |_| Vec::new());
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    assert_eq!(original.len(), HEADER_LEN);
    let scratch = TempDir::new();
    for (what, range, bytes) in [
        ("huge padding", PADDING_HEADER, [0x01u8, 0xFF, 0xFF, 0xFF]),
        ("no last flag", 42..46, [0x01, 0x00, 0x8F, 0xFC]),
    ] {
        let mut damaged = original.clone();
        damaged[range].copy_from_slice(&bytes);
        let (folder, result) = recover_with_track(&killed, &scratch, what, &damaged);
        let recovered = result.unwrap_or_else(|error| panic!("{what}: {error:?}"));
        let report = &recovered.tracks[0];
        assert_eq!(
            (
                report.frames_kept,
                report.tail_bytes,
                report.tail_file.clone()
            ),
            (0, 0, None),
            "{what}"
        );
        assert_recovered_track(&folder, &[], 0);
        assert_eq!(
            std::fs::metadata(folder.join("mic.flac")).unwrap().len(),
            HEADER_LEN as u64
        );
        assert!(!folder.join("mic.tail").exists(), "{what}");
    }
}

#[test]
fn a_damaged_header_and_a_damaged_first_frame_set_the_bytes_aside_and_keep_the_track() {
    let killed = killed_blocks(3);
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let mut damaged = original.clone();
    damaged[PADDING_HEADER].copy_from_slice(&[0x01, 0xFF, 0xFF, 0xFF]);
    damaged[HEADER_LEN + 30] ^= 0x10;
    let scratch = TempDir::new();
    let (folder, result) = recover_with_track(&killed, &scratch, "both", &damaged);
    let recovered = result.expect("the prefix still says whose file this is");
    let report = &recovered.tracks[0];
    assert_eq!(report.frames_kept, 0);
    assert_eq!(report.tail_bytes, (original.len() - HEADER_LEN) as u64);
    assert!(
        std::fs::read(folder.join("mic.tail")).unwrap() == damaged[HEADER_LEN..],
        "every byte after the header is in the tail"
    );
    assert_recovered_track(&folder, &[], 0);
}

#[test]
fn a_file_that_cannot_be_shown_to_be_ours_is_left_alone_however_long_it_is() {
    let killed = killed_blocks(3);
    let original = std::fs::read(killed.folder.join("mic.flac")).unwrap();
    let mut wave = b"RIFF\x24\x80\x00\x00WAVEfmt ".to_vec();
    wave.resize(50_000, 0x55);
    let mut noise = vec![0u8; 40_000];
    let mut rng = Rng::new(32);
    for byte in &mut noise {
        *byte = rng.range(0, 256) as u8;
    }
    // Our header, but another sample rate than the manifest says.
    let mut other_rate = original.clone();
    other_rate[PADDING_HEADER].copy_from_slice(&[0x01, 0xFF, 0xFF, 0xFF]);
    other_rate[HEADER_LEN..].fill(0);
    let mut streaminfo = parse_header(&original).unwrap().streaminfo;
    streaminfo[10] = 0x0A; // 44.1 kHz in the top 20 bits: 0x0AC44
    streaminfo[11] = 0xC4;
    streaminfo[12] = 0x40 | (streaminfo[12] & 0x0F);
    other_rate[8..42].copy_from_slice(&streaminfo);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("a wave file", wave),
        ("noise", noise),
        ("zeros", vec![0u8; HEADER_LEN + 1000]),
        ("our header at another rate, no frames", other_rate),
    ];
    let scratch = TempDir::new();
    for (what, bytes) in cases {
        let folder = copy_dir(&killed.folder, &scratch.path().join(what));
        write_track(&folder, &bytes);
        let before = snapshot(&folder);
        let error = recover_folder(&folder).expect_err(what);
        assert_eq!(error.code(), "recording_corrupt_track", "{what}: {error:?}");
        assert_eq!(snapshot(&folder), before, "{what}: nothing was changed");
        assert_eq!(
            read_manifest(&folder).state,
            RecordingState::Recording,
            "{what}"
        );
    }
}
