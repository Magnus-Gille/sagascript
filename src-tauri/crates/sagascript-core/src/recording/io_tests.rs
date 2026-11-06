use std::io::{Read, Write};

use sagascript_testsupport::TempDir;

use super::*;

fn code(result: Result<File, RecordingError>) -> &'static str {
    match result {
        Ok(_) => "ok",
        Err(error) => error.code(),
    }
}

#[test]
fn a_regular_file_opens_in_every_mode_and_a_missing_one_is_created_only_when_asked() {
    let dir = TempDir::new();
    let path = dir.path().join("a.bin");
    // Missing: reported as an I/O failure (callers match NotFound) ...
    for mode in [Open::Read, Open::ReadWrite] {
        match open_member(&path, mode, "open a test file") {
            Err(RecordingError::Io { kind, .. }) => assert_eq!(kind, io::ErrorKind::NotFound),
            other => panic!("{mode:?}: {:?}", other.map(|_| ())),
        }
    }
    assert!(!path.exists());
    // ... unless creating is the point: private, and an existing file is kept.
    let mut file = open_member(&path, Open::ReadWriteCreate, "open a test file").unwrap();
    file.write_all(b"kept").unwrap();
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mut again = open_member(&path, Open::ReadWriteCreate, "open a test file").unwrap();
    let mut text = String::new();
    again.read_to_string(&mut text).unwrap();
    assert_eq!(text, "kept", "an existing file is not truncated");
    assert_eq!(code(open_member(&path, Open::Read, "x")), "ok");
    assert_eq!(code(open_member(&path, Open::ReadWrite, "x")), "ok");
}

#[test]
fn a_directory_is_not_a_file_of_the_recording() {
    let dir = TempDir::new();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    for mode in [Open::Read, Open::ReadWrite, Open::ReadWriteCreate] {
        assert_eq!(
            code(open_member(&sub, mode, "open a test file")),
            "recording_not_regular_file",
            "{mode:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_link_is_refused_whatever_it_points_at_and_nothing_behind_it_is_touched() {
    let dir = TempDir::new();
    let target = dir.path().join("target");
    std::fs::write(&target, b"precious").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let dangling = dir.path().join("dangling");
    let nowhere = dir.path().join("nowhere");
    std::os::unix::fs::symlink(&nowhere, &dangling).unwrap();
    let to_dir = dir.path().join("to-dir");
    std::os::unix::fs::symlink(dir.path(), &to_dir).unwrap();

    for path in [&link, &dangling, &to_dir] {
        for mode in [Open::Read, Open::ReadWrite, Open::ReadWriteCreate] {
            assert_eq!(
                code(open_member(path, mode, "open a test file")),
                "recording_not_regular_file",
                "{path:?} {mode:?}"
            );
        }
    }
    assert_eq!(std::fs::read(&target).unwrap(), b"precious");
    assert!(!nowhere.exists(), "creating did not go through the link");
}

#[cfg(unix)]
#[test]
fn a_pipe_is_refused_without_waiting_for_a_writer() {
    let dir = TempDir::new();
    let path = dir.path().join("pipe");
    let c_path = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    for mode in [Open::Read, Open::ReadWrite, Open::ReadWriteCreate] {
        assert_eq!(
            code(open_member(&path, mode, "open a test file")),
            "recording_not_regular_file",
            "{mode:?}"
        );
    }
}

/// The open itself does not follow a link either: a link that appears between
/// the look at the entry and the open (here: opened directly with the flag the
/// code uses) fails with the error `open_member` turns into a refusal.
#[cfg(unix)]
#[test]
fn the_open_flag_alone_stops_a_link_that_appears_after_the_look() {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = TempDir::new();
    let target = dir.path().join("target");
    std::fs::write(&target, b"x").unwrap();
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let error = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&link)
        .unwrap_err();
    assert!(is_link_refusal(&error), "{error:?}");
}
