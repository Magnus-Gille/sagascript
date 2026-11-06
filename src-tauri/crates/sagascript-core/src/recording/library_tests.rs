use std::time::Duration;

use sagascript_testsupport::TempDir;

use super::*;
use crate::recording::test_recordings::*;
use crate::recording::test_signals::{copy_dir, recording, track};
use crate::recording::{EndedReason, RecordingStore};

fn store() -> (TempDir, RecordingStore) {
    let temp = TempDir::new();
    let store = RecordingStore::new(temp.path().join("Recordings"));
    (temp, store)
}

fn mic(store: &RecordingStore, title: &str, started_at: &str) -> Made {
    record_mic(store, title, started_at, Quality::Lossless, MONO48, 0.3)
}

#[test]
fn recordings_are_listed_newest_first_across_time_zones() {
    let (_temp, store) = store();
    // 22:30Z, 22:45Z and 20:00Z; compared as text the order would be wrong.
    mic(&store, "B", "2026-11-05T23:30:00.000+01:00");
    mic(&store, "A", "2026-11-05T22:45:00.000+00:00");
    mic(&store, "C", "2026-11-05T20:00:00.000Z");

    let titles: Vec<String> = list_recordings(store.root())
        .into_iter()
        .map(|summary| summary.title)
        .collect();
    assert_eq!(titles, ["A", "B", "C"]);
}

#[test]
fn equal_start_times_fall_back_to_the_folder_name() {
    let (_temp, store) = store();
    for title in ["Second", "First"] {
        mic(&store, title, "2026-11-05T10:00:00.000+01:00");
    }
    let names: Vec<String> = list_recordings(store.root())
        .into_iter()
        .map(|summary| summary.folder_name)
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
    assert_eq!(names.len(), 2);
}

#[test]
fn a_summary_describes_the_recording() {
    let (_temp, store) = store();
    let made = record_mic(
        &store,
        "Interview",
        "2026-11-05T10:00:00.000+01:00",
        Quality::Lossless,
        MONO48,
        1.0,
    );
    let list = list_recordings(store.root());
    assert_eq!(list.len(), 1);
    let summary = &list[0];
    assert_eq!(
        summary.folder_name,
        made.folder.file_name().unwrap().to_string_lossy()
    );
    assert_eq!(summary.recording_id, made.outcome.manifest.recording_id);
    assert_eq!(summary.title, "Interview");
    assert_eq!(summary.state, RecordingState::Complete);
    assert_eq!(summary.quality, Quality::Lossless);
    assert!((summary.duration_seconds - 1.02).abs() < 0.05);
    assert_eq!(
        (
            summary.codec.as_str(),
            summary.sample_rate,
            summary.channels
        ),
        ("flac", 48_000, 1)
    );
    assert_eq!(summary.track_count, 1);
    assert_eq!((summary.gap_count, summary.gap_seconds), (0, 0.0));
    assert_eq!(summary.ended_reason.as_deref(), Some("stopped"));
    assert!(summary.playable && !summary.needs_recovery && !summary.recovered);
    assert!(summary.bytes > 1000);

    let json = serde_json::to_value(summary).unwrap();
    assert_eq!(json["state"], "complete");
    assert_eq!(json["quality"], "lossless");
}

#[test]
fn a_folder_can_be_summarised_without_listing_the_root() {
    let (_temp, store) = store();
    let made = mic(&store, "Solo", "2026-11-05T10:00:00.000+01:00");
    let listed = list_recordings(store.root());
    assert_eq!(summarize_folder(&made.folder).unwrap(), listed[0]);
    assert!(summarize_folder(&made.folder.join("missing")).is_err());
}

#[test]
fn gaps_and_an_early_ending_show_in_the_summary() {
    let (_temp, store) = store();
    let made = record(
        &store,
        "Gappy",
        "2026-11-05T10:00:00.000+01:00",
        Quality::Lossless,
        &[Take::mic(MONO48)],
        |session, handles| {
            deliver_all(
                session,
                handles,
                Duration::from_millis(300),
                Duration::from_millis(100),
            );
            handles[0].overrun(24_000);
            deliver_all(
                session,
                handles,
                Duration::from_millis(300),
                Duration::from_millis(100),
            );
            handles[0].change_device();
        },
    );
    assert_eq!(made.outcome.ended_early, Some(EndedReason::DeviceChanged));
    let summary = &list_recordings(store.root())[0];
    assert_eq!(summary.gap_count, 1);
    assert!((summary.gap_seconds - 0.5).abs() < 0.01);
    assert_eq!(summary.ended_reason.as_deref(), Some("device_changed"));
}

#[test]
fn broken_folders_are_skipped_and_a_missing_root_is_empty() {
    let (_temp, store) = store();
    let good = mic(&store, "Good", "2026-11-05T10:00:00.000+01:00");
    let root = store.root();
    std::fs::create_dir(root.join("garbage")).unwrap();
    std::fs::write(root.join("garbage").join(MANIFEST_FILE), b"not json").unwrap();
    std::fs::create_dir(root.join("empty")).unwrap();
    std::fs::write(root.join("a file"), b"x").unwrap();

    let list = list_recordings(root);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].recording_id, good.outcome.manifest.recording_id);
    assert!(list_recordings(&root.join("nowhere")).is_empty());
}

#[test]
fn an_unfinished_recording_needs_recovery_unless_someone_owns_it() {
    let (_temp, store) = store();
    let writer = store
        .create(recording("Open", vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    let owned = &list_recordings(store.root())[0];
    assert_eq!(owned.state, RecordingState::Recording);
    assert!(!owned.needs_recovery, "a live process owns it");
    assert!(!owned.playable);
    assert_eq!(owned.ended_reason, None);

    drop(writer); // the process "dies": the lock is released
    let orphan = &list_recordings(store.root())[0];
    assert!(orphan.needs_recovery);
    assert!(!orphan.playable);

    store.recover_pending().unwrap();
    let recovered = &list_recordings(store.root())[0];
    assert!(recovered.recovered && recovered.playable && !recovered.needs_recovery);
}

#[test]
fn a_recording_is_found_by_folder_name_or_id_prefix() {
    let (_temp, store) = store();
    let made = mic(&store, "Find me", "2026-11-05T10:00:00.000+01:00");
    let name = made
        .folder
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let id = made.outcome.manifest.recording_id.clone();

    let (dir, manifest) = read_recording(store.root(), &name).unwrap();
    assert_eq!(dir, made.folder);
    assert_eq!(manifest.recording_id, id);
    assert_eq!(
        read_recording(store.root(), &id[..8]).unwrap().0,
        made.folder
    );
    assert_eq!(read_recording(store.root(), &id).unwrap().0, made.folder);

    for refused in [&id[..5], "", "nothing-like-it", "ffffffff"] {
        let error = read_recording(store.root(), refused).unwrap_err();
        assert_eq!(error.code(), "recording_invalid_input", "{refused:?}");
    }
}

#[test]
fn an_id_prefix_that_matches_two_recordings_is_refused() {
    let (_temp, store) = store();
    let made = mic(&store, "Twin", "2026-11-05T10:00:00.000+01:00");
    copy_dir(&made.folder, &store.root().join("copy of twin"));
    let id = &made.outcome.manifest.recording_id;
    let error = read_recording(store.root(), &id[..8]).unwrap_err();
    assert_eq!(error.code(), "recording_invalid_input");
    assert!(error.to_string().contains("more than one"));
}

#[test]
fn names_that_leave_the_recordings_folder_are_refused() {
    let (temp, store) = store();
    let made = mic(&store, "Inside", "2026-11-05T10:00:00.000+01:00");
    // A genuine recording next to the recordings folder.
    copy_dir(&made.folder, &temp.path().join("Sibling"));
    let name = made
        .folder
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    for bad in [
        "../Sibling".to_string(),
        "..".to_string(),
        ".".to_string(),
        format!("{name}/.."),
        "a/b".to_string(),
        "..\\Sibling".to_string(),
        temp.path().join("Sibling").to_string_lossy().into_owned(),
    ] {
        assert_eq!(
            read_recording(store.root(), &bad).unwrap_err().code(),
            "recording_invalid_input",
            "{bad:?}"
        );
        assert_eq!(
            delete_recording_folder(store.root(), &bad)
                .unwrap_err()
                .code(),
            "recording_invalid_input",
            "{bad:?}"
        );
    }
    assert!(temp.path().join("Sibling").exists());
}

#[test]
fn delete_removes_the_whole_folder() {
    let (_temp, store) = store();
    let one = mic(&store, "One", "2026-11-05T10:00:00.000+01:00");
    let two = mic(&store, "Two", "2026-11-05T11:00:00.000+01:00");
    let name = one
        .folder
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    delete_recording_folder(store.root(), &name).unwrap();
    assert!(!one.folder.exists());

    let id = two.outcome.manifest.recording_id.clone();
    delete_recording_folder(store.root(), &id[..8]).unwrap();
    assert!(!two.folder.exists());
    assert!(store.root().exists(), "only the recording goes");
    assert_eq!(
        delete_recording_folder(store.root(), &name)
            .unwrap_err()
            .code(),
        "recording_invalid_input",
        "it is already gone"
    );
}

#[test]
fn delete_refuses_a_recording_in_progress() {
    let (_temp, store) = store();
    let writer = store
        .create(recording("Live", vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    let name = writer.folder_name().to_string();
    let error = delete_recording_folder(store.root(), &name).unwrap_err();
    assert_eq!(error.code(), "recording_locked");
    assert!(writer.folder().exists());
}

/// A made recording's folder name.
fn folder_name(made: &Made) -> String {
    made.folder
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[test]
fn delete_fails_closed_when_the_lock_cannot_be_taken() {
    let (_temp, store) = store();
    let made = mic(&store, "One", "2026-11-05T10:00:00.000+01:00");
    let name = folder_name(&made);
    // Whatever stands where the lock file belongs (here a directory, on a
    // real system a permission error) is not "no lock": ownership is unknown.
    let lock = made.folder.join(crate::recording::store::LOCK_FILE);
    let _ = std::fs::remove_file(&lock);
    std::fs::create_dir(&lock).unwrap();
    let error = delete_recording_folder(store.root(), &name).unwrap_err();
    assert_ne!(error.code(), "recording_locked");
    assert!(made.folder.join("recording.json").is_file(), "{error}");
    assert!(made.folder.exists());
}

#[test]
fn delete_of_a_recording_without_a_lock_file_takes_the_lock_itself() {
    let (_temp, store) = store();
    let made = mic(&store, "One", "2026-11-05T10:00:00.000+01:00");
    let name = folder_name(&made);
    let _ = std::fs::remove_file(made.folder.join(crate::recording::store::LOCK_FILE));
    delete_recording_folder(store.root(), &name).unwrap();
    assert!(!made.folder.exists());
}

#[test]
fn a_removal_lock_keeps_out_deletion_recovery_and_a_second_lock() {
    let (_temp, store) = store();
    let made = mic(&store, "One", "2026-11-05T10:00:00.000+01:00");
    let name = folder_name(&made);
    let guard = lock_recording_for_removal(store.root(), &name).unwrap();
    assert_eq!(
        delete_recording_folder(store.root(), &name)
            .unwrap_err()
            .code(),
        "recording_locked"
    );
    assert_eq!(
        lock_recording_for_removal(store.root(), &name)
            .err()
            .map(|error| error.code()),
        Some("recording_locked")
    );
    // Recovery of the folder is refused while the removal holds the lock.
    let reports = store.recover_pending().unwrap();
    assert!(
        reports.iter().all(|report| !matches!(
            report.outcome,
            crate::recording::RecoveryOutcome::Recovered(_)
        )),
        "{reports:?}"
    );
    drop(guard);
    delete_recording_folder(store.root(), &name).unwrap();
}

#[test]
fn a_removal_lock_is_refused_for_a_recording_in_progress_and_for_a_bad_name() {
    let (_temp, store) = store();
    let writer = store
        .create(recording("Live", vec![track("mic", 48_000, 1, 24)]))
        .unwrap();
    let name = writer.folder_name().to_string();
    assert_eq!(
        lock_recording_for_removal(store.root(), &name)
            .err()
            .map(|error| error.code()),
        Some("recording_locked")
    );
    assert_eq!(
        lock_recording_for_removal(store.root(), "../x")
            .err()
            .map(|error| error.code()),
        Some("recording_invalid_input")
    );
}

#[test]
fn delete_refuses_a_folder_that_is_not_a_recording() {
    let (_temp, store) = store();
    let other = store.root().join("Photos");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("keep.txt"), b"mine").unwrap();
    let error = delete_recording_folder(store.root(), "Photos").unwrap_err();
    assert_eq!(error.code(), "recording_invalid_input");
    assert!(other.join("keep.txt").exists());

    // A file, not a folder.
    std::fs::write(store.root().join("note.txt"), b"x").unwrap();
    assert!(delete_recording_folder(store.root(), "note.txt").is_err());
    assert!(store.root().join("note.txt").exists());
}

#[test]
fn the_track_path_is_the_flac_file_of_a_known_track() {
    let (_temp, store) = store();
    let made = mic(&store, "Path", "2026-11-05T10:00:00.000+01:00");
    let path = audio_track_path(&made.folder, &made.outcome.manifest, "mic").unwrap();
    assert_eq!(path, made.folder.join("mic.flac"));
    assert_eq!(
        audio_track_path(&made.folder, &made.outcome.manifest, "system")
            .unwrap_err()
            .code(),
        "recording_unknown_track"
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        audio_track_path(&made.folder, &made.outcome.manifest, "mic")
            .unwrap_err()
            .code(),
        "recording_io"
    );
}

#[test]
fn rfc3339_times_become_epoch_seconds() {
    assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(epoch_seconds("1970-01-01T01:00:00+01:00"), Some(0));
    assert_eq!(
        epoch_seconds("2026-11-05T14:02:11.482+01:00"),
        Some(1_793_883_731)
    );
    assert_eq!(epoch_seconds("2026-11-05T13:02:11Z"), Some(1_793_883_731));
    assert_eq!(
        epoch_seconds("2000-03-01T00:00:00-05:30"),
        Some(951_888_600)
    );
    for bad in [
        "",
        "yesterday",
        "2026-11-05",
        "2026-13-05T00:00:00Z",
        "2026-11-05T14:02:11",
    ] {
        assert_eq!(epoch_seconds(bad), None, "{bad:?}");
    }
}
