use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Barrier};

use sagascript_testsupport::TempDir;

use super::*;
use crate::test_support::FaultyFs;

/// Every entry of `dir`, hidden ones included, sorted.
fn names(dir: &Path) -> Vec<String> {
    let mut names = fs::read_dir(dir)
        .expect("list directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn publishes_the_exact_bytes_and_leaves_nothing_else() {
    let dir = TempDir::new();
    let destination = dir.join("export.json");

    let published = publish_new(&destination, b"complete content\n").expect("publish");

    assert!(published.leftover.is_none());
    assert_eq!(fs::read(&destination).expect("read"), b"complete content\n");
    assert_eq!(names(&dir), ["export.json"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&destination)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn an_empty_file_is_published_too() {
    let dir = TempDir::new();
    publish_new(&dir.join("empty"), b"").expect("publish");
    assert_eq!(fs::read(dir.join("empty")).expect("read"), b"");
}

#[test]
fn an_existing_destination_is_never_replaced() {
    let dir = TempDir::new();
    let file = dir.join("file.json");
    fs::write(&file, b"keep me").expect("file");
    let alias = dir.join("alias.json");
    fs::hard_link(&file, &alias).expect("hard link");
    let folder = dir.join("folder");
    fs::create_dir(&folder).expect("folder");

    for destination in [&file, &alias, &folder] {
        let error = publish_new(destination, b"replacement").expect_err("exists");
        assert!(
            matches!(error.kind, PublishErrorKind::AlreadyExists),
            "{error:?}"
        );
        assert!(error.leftover.is_none());
    }

    assert_eq!(fs::read(&file).expect("read"), b"keep me");
    assert_eq!(fs::read(&alias).expect("read"), b"keep me");
    assert_eq!(names(&dir), ["alias.json", "file.json", "folder"]);
}

#[cfg(unix)]
#[test]
fn a_symlink_destination_counts_as_existing_even_when_dangling() {
    let dir = TempDir::new();
    let target = dir.join("target.json");
    let link = dir.join("link.json");
    std::os::unix::fs::symlink(&target, &link).expect("dangling symlink");

    let error = publish_new(&link, b"replacement").expect_err("exists");

    assert!(matches!(error.kind, PublishErrorKind::AlreadyExists));
    assert!(!target.exists(), "the link's target must not be created");
    assert_eq!(names(&dir), ["link.json"]);
}

#[test]
fn an_unusable_destination_creates_nothing() {
    let dir = TempDir::new();
    let not_a_folder = dir.join("plain-file");
    fs::write(&not_a_folder, b"x").expect("file");

    let missing = publish_new(&dir.join("missing").join("out.json"), b"x").expect_err("missing");
    assert!(matches!(
        missing.kind,
        PublishErrorKind::ParentUnavailable(_)
    ));
    let beneath_file = publish_new(&not_a_folder.join("out.json"), b"x").expect_err("file parent");
    assert!(matches!(
        beneath_file.kind,
        PublishErrorKind::ParentNotDirectory | PublishErrorKind::ParentUnavailable(_)
    ));
    let nameless = publish_new(Path::new("/"), b"x").expect_err("no name");
    assert!(matches!(nameless.kind, PublishErrorKind::NoFileName));

    assert_eq!(names(&dir), ["plain-file"]);
    assert!(check_destination_folder(&dir.join("fine.json")).is_ok());
    assert!(check_destination_folder(&dir.join("missing").join("out.json")).is_err());
}

#[test]
fn a_staged_file_is_invisible_until_published_and_dropping_it_cleans_up() {
    let dir = TempDir::new();
    let destination = dir.join("out.json");

    let staged = StagedFile::create(&destination, b"bytes").expect("stage");
    assert!(!destination.exists(), "nothing at the destination yet");
    assert!(staged.is_staged());
    assert_eq!(fs::read(staged.staged_path()).expect("staged"), b"bytes");
    assert_eq!(staged.destination(), destination);
    assert_eq!(staging_leftovers(&destination), [staged.staged_path()]);

    drop(staged);
    assert_eq!(names(&dir), Vec::<String>::new());

    let staged = StagedFile::create(&destination, b"bytes").expect("stage again");
    staged.publish().expect("publish");
    assert_eq!(names(&dir), ["out.json"]);
}

#[test]
fn discard_removes_the_staging_file_and_reports_success() {
    let dir = TempDir::new();
    let staged = StagedFile::create(&dir.join("out.json"), b"bytes").expect("stage");
    staged.discard().expect("discard");
    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn of_racing_publishers_exactly_one_wins_and_the_file_is_never_torn() {
    let dir = TempDir::new();
    let destination = Arc::new(dir.join("contested.json"));
    let barrier = Arc::new(Barrier::new(8));
    let handles = (0..8)
        .map(|writer| {
            let destination = Arc::clone(&destination);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let bytes = format!("writer {writer}: {}", "x".repeat(64 * 1024));
                barrier.wait();
                publish_new(&destination, bytes.as_bytes()).map(|_| bytes)
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("publisher thread"))
        .collect::<Vec<_>>();

    let winners = results
        .iter()
        .filter_map(|result| result.as_ref().ok())
        .collect::<Vec<_>>();
    assert_eq!(winners.len(), 1, "exactly one publisher wins");
    for loser in results.iter().filter_map(|result| result.as_ref().err()) {
        assert!(
            matches!(loser.kind, PublishErrorKind::AlreadyExists),
            "{loser:?}"
        );
    }
    assert_eq!(
        fs::read(&*destination).expect("read"),
        winners[0].as_bytes(),
        "the file is the winner's complete content"
    );
    assert_eq!(names(&dir), ["contested.json"]);
}

#[test]
fn a_failed_commit_publishes_nothing_and_removes_the_staging_file() {
    let dir = TempDir::new();
    let destination = dir.join("out.json");
    let staged = StagedFile::create(&destination, b"bytes").expect("stage");

    let error = staged
        .publish_with(&FaultyFs::new().failing_link(1))
        .expect_err("the link fails");

    assert!(
        matches!(error.kind, PublishErrorKind::Commit(_)),
        "{error:?}"
    );
    assert!(error.leftover.is_none());
    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn a_staging_file_that_cannot_be_removed_after_a_commit_is_reported_not_failed() {
    let dir = TempDir::new();
    let destination = dir.join("out.json");
    let staged = StagedFile::create(&destination, b"bytes").expect("stage");
    let staging_path = staged.staged_path().to_path_buf();

    let published = staged
        .publish_with(&FaultyFs::new().failing_remove())
        .expect("the file is published");

    let leftover = published.leftover.expect("the leftover is reported");
    assert_eq!(leftover.path, staging_path);
    assert_eq!(fs::read(&destination).expect("read"), b"bytes");
    assert_eq!(staging_leftovers(&destination), [staging_path]);
}

#[test]
fn a_failed_commit_and_a_failed_removal_report_the_leftover() {
    let dir = TempDir::new();
    let destination = dir.join("out.json");
    let staged = StagedFile::create(&destination, b"bytes").expect("stage");

    let error = staged
        .publish_with(&FaultyFs::new().failing_link(1).failing_remove())
        .expect_err("the link fails");

    assert!(matches!(error.kind, PublishErrorKind::Commit(_)));
    assert!(error.leftover.is_some());
    assert!(!destination.exists());
}

#[test]
fn a_reserved_path_publishes_what_an_external_writer_created_there() {
    let dir = TempDir::new();
    let destination = dir.join("cache.json");
    let reserved = StagedFile::reserve(&destination).expect("reserve");
    assert!(!reserved.is_staged());

    // The pipeline writes its artifact atomically at the reserved path.
    publish_new(reserved.staged_path(), b"cache bytes").expect("external writer");
    assert!(reserved.is_staged());
    assert!(!destination.exists());

    reserved.publish().expect("publish");
    assert_eq!(fs::read(&destination).expect("read"), b"cache bytes");
    assert_eq!(names(&dir), ["cache.json"]);
}

#[test]
fn publishing_a_reserved_path_nobody_wrote_fails_cleanly() {
    let dir = TempDir::new();
    let reserved = StagedFile::reserve(&dir.join("cache.json")).expect("reserve");

    let error = reserved.publish().expect_err("nothing staged");

    assert!(
        matches!(error.kind, PublishErrorKind::NothingStaged),
        "{error:?}"
    );
    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn dropping_a_reserved_path_removes_what_was_written_there() {
    let dir = TempDir::new();
    let reserved = StagedFile::reserve(&dir.join("cache.json")).expect("reserve");
    publish_new(reserved.staged_path(), b"cache bytes").expect("external writer");

    drop(reserved);

    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn a_long_destination_name_still_gets_a_staging_name() {
    let dir = TempDir::new();
    let destination = dir.join(format!("{}.json", "n".repeat(150)));

    let staged = StagedFile::create(&destination, b"bytes").expect("stage");
    let staging_name = staged
        .staged_path()
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    assert!(staging_name.len() < 255, "{staging_name}");
    staged.publish().expect("publish");

    assert_eq!(fs::read(&destination).expect("read"), b"bytes");
}

#[test]
fn staging_leftovers_finds_what_a_killed_process_leaves_and_nothing_else() {
    let dir = TempDir::new();
    let destination = dir.join("proposal.json");
    let staged = StagedFile::create(&destination, b"bytes").expect("stage");
    let survivor = staged.staged_path().to_path_buf();
    // A kill never runs destructors: the staging file stays behind.
    std::mem::forget(staged);
    fs::write(dir.join("proposal.json.backup"), b"x").expect("unrelated");
    fs::write(dir.join(".proposal.json.not-a-uuid.tmp"), b"x").expect("unrelated");
    fs::write(
        dir.join(".other.json.2f6c5b0e-8c1f-4a8e-9f3a-6a0b8f1c2d3e.tmp"),
        b"x",
    )
    .expect("another destination's staging file");

    assert_eq!(staging_leftovers(&destination), [survivor]);
    assert!(staging_leftovers(&dir.join("missing").join("x.json")).is_empty());
}

// Review follow-ups (#342): names, leftovers and nested staging names.

#[test]
fn a_long_non_ascii_destination_name_still_gets_a_valid_staging_name() {
    let dir = TempDir::new();
    // 80 characters are 240 UTF-8 bytes; a staging name built from them is over
    // the 255-byte limit of Unix filesystems if the name is cut by characters.
    let destination = dir.join(format!("{}.json", "漢".repeat(80)));

    let staged = StagedFile::create(&destination, b"bytes").expect("stage");
    let staging_name = staged
        .staged_path()
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    // The limit is 255 bytes of UTF-8 on Unix and 255 UTF-16 units on Windows.
    #[cfg(unix)]
    let units = staging_name.len();
    #[cfg(not(unix))]
    let units = staging_name.encode_utf16().count();
    assert!(units <= 255, "{units} units");
    assert_eq!(staging_leftovers(&destination), [staged.staged_path()]);
    staged.publish().expect("publish");

    assert_eq!(fs::read(&destination).expect("read"), b"bytes");
    assert_eq!(names(&dir).len(), 1);
}

#[test]
fn a_staging_file_that_was_never_created_is_not_reported_as_a_leftover() {
    // The creation fails and so does the removal that follows (a name that is
    // too long, a read-only folder): nothing exists, so nothing is left over.
    let dir = TempDir::new();
    let ops = FaultyFs::new().failing_create().failing_remove();

    let error = StagedFile::create_with(&dir.join("out.json"), b"x", &ops).expect_err("no file");

    assert!(
        matches!(error.kind, PublishErrorKind::Stage(_)),
        "{error:?}"
    );
    assert!(error.leftover.is_none(), "{:?}", error.leftover);
    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn a_filesystem_that_cannot_fsync_still_publishes() {
    let dir = TempDir::new();
    let destination = dir.join("out.json");
    for kind in [io::ErrorKind::Unsupported, io::ErrorKind::InvalidInput] {
        let _ = fs::remove_file(&destination);
        let ops = FaultyFs::new().failing_sync_file(kind);

        let published = publish_new_with(&destination, b"bytes", &ops);

        // `InvalidInput` is the unsupported class (EINVAL) only on Unix.
        let tolerated = kind == io::ErrorKind::Unsupported || cfg!(unix);
        assert_eq!(published.is_ok(), tolerated, "{kind:?}: {published:?}");
        if tolerated {
            assert_eq!(fs::read(&destination).expect("read"), b"bytes");
        }
    }
}

#[test]
fn a_real_fsync_failure_publishes_nothing() {
    let dir = TempDir::new();
    let destination = dir.join("out.json");
    let ops = FaultyFs::new().failing_sync_file(io::ErrorKind::StorageFull);

    let error = publish_new_with(&destination, b"bytes", &ops).expect_err("disk full");

    assert!(
        matches!(error.kind, PublishErrorKind::Stage(_)),
        "{error:?}"
    );
    assert_eq!(names(&dir), Vec::<String>::new());
}

#[test]
fn a_kill_inside_a_reserved_writer_leaves_a_staging_name_that_is_found() {
    let dir = TempDir::new();
    let destination = dir.join("cache.json");
    let reserved = StagedFile::reserve(&destination).expect("reserve");
    // The writer of a reserved path publishes it with `publish_new`; a kill
    // between its staging and its link leaves a staging name of the staging name.
    let inner = StagedFile::create(reserved.staged_path(), b"partial cache").expect("inner");
    let inner_path = inner.staged_path().to_path_buf();
    std::mem::forget(inner);

    assert_eq!(staging_leftovers(&destination), [inner_path]);
    // The reserved name itself is listed once it exists.
    publish_new(reserved.staged_path(), b"cache bytes").expect("writer completes");
    assert_eq!(staging_leftovers(&destination).len(), 2);
}

#[test]
fn only_unsupported_syncs_are_tolerated() {
    use std::io::{Error, ErrorKind};
    assert!(sync_unsupported(&Error::from(ErrorKind::Unsupported)));
    assert!(!sync_unsupported(&Error::from(ErrorKind::StorageFull)));
    assert!(!sync_unsupported(&Error::other("I/O error")));
    #[cfg(unix)]
    {
        assert!(
            sync_unsupported(&Error::from(ErrorKind::InvalidInput)),
            "EINVAL"
        );
        assert!(sync_unsupported(&Error::from_raw_os_error(25)), "ENOTTY");
        assert!(!sync_unsupported(&Error::from_raw_os_error(5)), "EIO");
    }
    #[cfg(windows)]
    {
        assert!(
            sync_unsupported(&Error::from_raw_os_error(1)),
            "ERROR_INVALID_FUNCTION"
        );
        assert!(
            sync_unsupported(&Error::from_raw_os_error(50)),
            "ERROR_NOT_SUPPORTED"
        );
        assert!(
            !sync_unsupported(&Error::from_raw_os_error(87)),
            "ERROR_INVALID_PARAMETER is a real bug"
        );
        assert!(!sync_unsupported(&Error::from(ErrorKind::InvalidInput)));
        assert!(
            !sync_unsupported(&Error::from_raw_os_error(5)),
            "ERROR_ACCESS_DENIED"
        );
    }
}

#[test]
fn fallback_sync_runs_only_when_the_full_sync_is_unsupported() {
    use std::cell::Cell;
    use std::io::{Error, ErrorKind};
    let plain_ran = Cell::new(false);
    let plain = |result: std::io::Result<()>| {
        plain_ran.set(true);
        result
    };

    // The full sync works: nothing to fall back to.
    assert!(sync_with_fallback(|| Ok(()), || plain(Err(Error::other("unused")))).is_ok());
    assert!(!plain_ran.get());

    // A real failure is not retried with the weaker sync.
    let error = sync_with_fallback(
        || Err(Error::from(ErrorKind::StorageFull)),
        || plain(Ok(())),
    )
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::StorageFull);
    assert!(!plain_ran.get());

    // An unsupported full sync falls back, and the fallback's result counts.
    assert!(sync_with_fallback(
        || Err(Error::from(ErrorKind::Unsupported)),
        || plain(Ok(()))
    )
    .is_ok());
    assert!(plain_ran.replace(false));
    let error = sync_with_fallback(
        || Err(Error::from(ErrorKind::Unsupported)),
        || plain(Err(Error::other("I/O error"))),
    )
    .unwrap_err();
    assert_eq!(error.to_string(), "I/O error");
    assert!(plain_ran.get());
}

/// F_FULLFSYNC succeeds on APFS, so no download test reaches the `fsync(2)`
/// fallback; call it directly.
#[cfg(target_vendor = "apple")]
#[test]
fn plain_fsync_flushes_a_regular_file() {
    use std::io::Write;
    let dir = TempDir::new();
    let mut file = std::fs::File::create(dir.join("synced")).unwrap();
    file.write_all(b"data").unwrap();

    fsync(&file).unwrap();
}

/// A descriptor with no sync operation fails with an unsupported-class errno
/// (EINVAL, or ENOTSUP on XNU), which the caller turns into a warning rather
/// than a failed download.
#[cfg(target_vendor = "apple")]
#[test]
fn plain_fsync_reports_a_descriptor_that_cannot_sync() {
    use std::os::fd::OwnedFd;
    let (socket, _peer) = std::os::unix::net::UnixStream::pair().unwrap();
    let file = std::fs::File::from(OwnedFd::from(socket));

    let error = fsync(&file).unwrap_err();

    assert!(sync_unsupported(&error), "{error:?}");
}

#[cfg(unix)]
#[test]
fn unsupported_sync_is_tolerated_but_real_failures_are_not() {
    use std::io::{Error, ErrorKind};
    assert!(tolerate_unsupported(Ok(())).is_ok());
    assert!(tolerate_unsupported(Err(Error::from(ErrorKind::Unsupported))).is_ok());
    assert!(tolerate_unsupported(Err(Error::from_raw_os_error(25))).is_ok());
    assert!(tolerate_unsupported(Err(Error::from_raw_os_error(5))).is_err());
}

// Round 2 (#342): which staging files belong to which destination.

#[test]
fn destinations_sharing_a_long_prefix_do_not_share_staging_leftovers() {
    let dir = TempDir::new();
    // Longer than the part of a name copied into a staging name, and equal in it.
    let shared = "a".repeat(120);
    let one = dir.join(format!("{shared}-one.json"));
    let two = dir.join(format!("{shared}-two.json"));
    let staged_two = StagedFile::create(&two, b"bytes").expect("stage two");

    assert_eq!(staging_leftovers(&two), [staged_two.staged_path()]);
    assert_eq!(
        staging_leftovers(&one),
        Vec::<PathBuf>::new(),
        "another destination's staging file is not one's own"
    );
    let staged_one = StagedFile::create(&one, b"bytes").expect("stage one");
    assert_eq!(staging_leftovers(&one), [staged_one.staged_path()]);
    assert_eq!(staging_leftovers(&two), [staged_two.staged_path()]);
    staged_one.publish().expect("publish one");
    staged_two.publish().expect("publish two");
    assert_eq!(names(&dir).len(), 2);
}

#[test]
fn a_destination_shaped_like_a_staging_name_finds_its_own_staging_file_and_not_itself() {
    let dir = TempDir::new();
    let destination = dir.join(".report.2f6c5b0e-8c1f-4a8e-9f3a-6a0b8f1c2d3e.tmp");
    // The user's own file, named like a staging file.
    fs::write(&destination, b"the user's file").expect("user file");
    let staged = StagedFile::create(&destination, b"bytes").expect("stage");
    std::mem::forget(staged);

    let found = staging_leftovers(&destination);

    assert_eq!(found.len(), 1, "{found:?}");
    assert_ne!(
        found[0], destination,
        "the destination is not its own leftover"
    );
    assert_eq!(fs::read(&destination).expect("kept"), b"the user's file");
}

/// Everything `tracing` logs at debug level and above while `work` runs.
fn logged_while(work: impl FnOnce()) -> String {
    use std::sync::Mutex;
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().expect("log buffer").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::with_default(subscriber, work);
    let logged = buffer.0.lock().expect("log buffer").clone();
    String::from_utf8_lossy(&logged).into_owned()
}

#[test]
fn a_folder_that_cannot_sync_still_publishes_and_only_a_real_failure_warns() {
    let dir = TempDir::new();
    for (kind, warns) in [
        (io::ErrorKind::Unsupported, false),
        (io::ErrorKind::StorageFull, true),
    ] {
        let destination = dir.join(format!("{kind:?}.json"));
        let ops = FaultyFs::new().failing_sync_dir(kind);

        let logged = logged_while(|| {
            publish_new_with(&destination, b"bytes", &ops).expect("the file is published");
        });

        assert_eq!(fs::read(&destination).expect("read"), b"bytes", "{kind:?}");
        assert_eq!(logged.contains("WARN"), warns, "{kind:?}: {logged}");
        assert!(logged.contains("sync"), "{kind:?}: {logged}");
        assert!(
            !logged.contains(&dir.display().to_string()),
            "no path in the log: {logged}"
        );
    }
}
