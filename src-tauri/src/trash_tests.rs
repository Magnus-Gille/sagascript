use std::sync::Mutex;

use sagascript_testsupport::TempDir;

use super::*;

/// A recording folder as the recorder leaves it, as far as the checks look.
fn make_recording(root: &Path, name: &str) -> PathBuf {
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).expect("create the folder");
    std::fs::write(folder.join(MANIFEST_FILE), b"{}").expect("write the manifest");
    folder
}

fn code(result: Result<PathBuf, CommandError>) -> String {
    result.expect_err("the folder is refused").code().to_owned()
}

#[test]
fn a_direct_child_with_a_manifest_may_be_trashed() {
    let root = TempDir::new();
    let folder = make_recording(root.path(), "2026-11-05-standup-ab12cd");
    assert_eq!(
        recording_folder(root.path(), "2026-11-05-standup-ab12cd").expect("accepted"),
        folder
    );
}

#[test]
fn a_name_that_is_not_one_plain_component_is_refused() {
    let root = TempDir::new();
    make_recording(root.path(), "a");
    std::fs::create_dir_all(root.path().join("a").join("b")).expect("nested folder");
    make_recording(&root.path().join("a"), "b");
    for name in [
        "", ".", "..", "../a", "a/b", "a\\b", "/a", "a/", "./a", "..\\a",
    ] {
        assert_eq!(
            code(recording_folder(root.path(), name)),
            "invalid_input",
            "{name:?}"
        );
    }
}

#[test]
fn an_absolute_path_is_not_a_name() {
    let root = TempDir::new();
    let folder = make_recording(root.path(), "a");
    let absolute = folder.to_string_lossy().into_owned();
    assert_eq!(
        code(recording_folder(root.path(), &absolute)),
        "invalid_input"
    );
}

#[test]
fn a_missing_folder_is_not_found() {
    let root = TempDir::new();
    assert_eq!(code(recording_folder(root.path(), "gone")), "not_found");
}

#[test]
fn a_folder_without_recording_json_is_refused() {
    let root = TempDir::new();
    std::fs::create_dir_all(root.path().join("photos")).expect("folder");
    assert_eq!(
        code(recording_folder(root.path(), "photos")),
        "invalid_input"
    );
    // A directory called recording.json is not a manifest either.
    let odd = root.path().join("odd");
    std::fs::create_dir_all(odd.join(MANIFEST_FILE)).expect("folder");
    assert_eq!(code(recording_folder(root.path(), "odd")), "invalid_input");
}

#[test]
fn a_file_is_not_a_recording_folder() {
    let root = TempDir::new();
    std::fs::write(root.path().join("note.txt"), b"x").expect("file");
    assert_eq!(
        code(recording_folder(root.path(), "note.txt")),
        "invalid_input"
    );
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_refused_even_to_a_recording() {
    let root = TempDir::new();
    let outside = TempDir::new();
    let real = make_recording(outside.path(), "real");
    std::os::unix::fs::symlink(&real, root.path().join("link")).expect("symlink");
    assert_eq!(code(recording_folder(root.path(), "link")), "invalid_input");
}

#[test]
fn error_messages_carry_no_path() {
    let root = TempDir::new();
    let shown = root.path().to_string_lossy().into_owned();
    for name in ["gone", "../x", "."] {
        let message = recording_folder(root.path(), name)
            .expect_err("refused")
            .message();
        assert!(!message.contains(&shown), "{message}");
    }
}

/// The seam: a trash that remembers what it was given and removes nothing.
#[derive(Default)]
pub(crate) struct RecordingTheTrash(pub Mutex<Vec<PathBuf>>);

impl RecordingTrash for RecordingTheTrash {
    fn move_to_trash(&self, folder: &Path) -> Result<(), CommandError> {
        self.0.lock().expect("lock").push(folder.to_path_buf());
        Ok(())
    }
}

#[test]
fn the_seam_is_a_trait_object() {
    let trash: Box<dyn RecordingTrash> = Box::new(RecordingTheTrash::default());
    trash.move_to_trash(Path::new("x")).expect("accepted");
}

#[cfg(not(any(target_os = "macos", windows)))]
#[test]
fn elsewhere_the_system_trash_is_unavailable() {
    let error = move_to_trash(Path::new("x")).expect_err("no trash here");
    assert_eq!(error.code(), "unavailable");
}

/// A fixed disk with a normal bin and a small folder: the one accepted case.
fn good() -> BinFacts {
    BinFacts {
        drive_type: Some(3),
        bin_off: false,
        quota_bytes: Some(10 * 1024 * 1024 * 1024),
        volume_bytes: Some(500 * 1024 * 1024 * 1024),
        folder_bytes: 600 * 1024 * 1024,
        unreadable: false,
    }
}

#[test]
fn a_fixed_disk_with_room_in_its_bin_is_accepted() {
    assert_eq!(recycle_guaranteed(&good()), Ok(()));
}

/// The Windows drive types of `GetDriveTypeW`: only a fixed disk is trusted to
/// have a Recycle Bin (the shell deletes for good on the others).
#[test]
fn only_a_fixed_disk_is_expected_to_have_a_recycle_bin() {
    for other in [
        Some(0),
        Some(1),
        Some(2),
        Some(4),
        Some(5),
        Some(6),
        Some(99),
        None,
    ] {
        let facts = BinFacts {
            drive_type: other,
            ..good()
        };
        assert_eq!(
            recycle_guaranteed(&facts),
            Err(BinRefusal::NoBin),
            "{other:?}"
        );
    }
}

#[test]
fn a_switched_off_bin_is_refused() {
    let facts = BinFacts {
        bin_off: true,
        ..good()
    };
    assert_eq!(recycle_guaranteed(&facts), Err(BinRefusal::BinOff));
}

#[test]
fn a_folder_larger_than_the_quota_is_refused() {
    let facts = BinFacts {
        folder_bytes: 10 * 1024 * 1024 * 1024 + 1,
        ..good()
    };
    assert_eq!(recycle_guaranteed(&facts), Err(BinRefusal::TooLarge));
    // At the quota exactly it fits.
    let facts = BinFacts {
        folder_bytes: 10 * 1024 * 1024 * 1024,
        ..good()
    };
    assert_eq!(recycle_guaranteed(&facts), Ok(()));
}

#[test]
fn without_an_explicit_quota_a_low_estimate_of_the_default_is_used() {
    // 5 % of a 100 GB disk is 5 GB.
    let facts = BinFacts {
        quota_bytes: None,
        volume_bytes: Some(100 * 1024 * 1024 * 1024),
        folder_bytes: 6 * 1024 * 1024 * 1024,
        ..good()
    };
    assert_eq!(recycle_guaranteed(&facts), Err(BinRefusal::TooLarge));
    let facts = BinFacts {
        folder_bytes: 4 * 1024 * 1024 * 1024,
        ..facts
    };
    assert_eq!(recycle_guaranteed(&facts), Ok(()));
}

/// A registry value that cannot be read, a file of the folder whose size is
/// unknown, a volume key that cannot be named: none of them is "no setting" or
/// "0 bytes", whatever else the facts say.
#[test]
fn a_read_that_failed_is_refused_even_when_everything_else_looks_fine() {
    let facts = BinFacts {
        unreadable: true,
        ..good()
    };
    assert_eq!(recycle_guaranteed(&facts), Err(BinRefusal::Unreadable));
    // The drive type is judged first, a known switched-off bin before a read error.
    let facts = BinFacts {
        unreadable: true,
        drive_type: Some(4),
        ..good()
    };
    assert_eq!(recycle_guaranteed(&facts), Err(BinRefusal::NoBin));
}

#[test]
fn what_cannot_be_established_is_refused() {
    let facts = BinFacts {
        quota_bytes: None,
        volume_bytes: None,
        ..good()
    };
    assert_eq!(recycle_guaranteed(&facts), Err(BinRefusal::Unknown));
}

#[test]
fn every_refusal_says_the_recording_was_not_deleted_and_carries_no_path() {
    for refusal in [
        BinRefusal::NoBin,
        BinRefusal::BinOff,
        BinRefusal::TooLarge,
        BinRefusal::Unknown,
        BinRefusal::Unreadable,
    ] {
        let error = refusal_error(refusal);
        assert_eq!(error.code(), "unavailable");
        let message = error.message();
        assert!(message.contains("not deleted"), "{message}");
        assert!(!message.contains(['\\', '/']), "{message}");
    }
}
