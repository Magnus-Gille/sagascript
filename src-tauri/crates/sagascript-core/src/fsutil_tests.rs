use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Arc, Barrier};

use sagascript_testsupport::TempDir;

use super::*;

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

/// The real filesystem with chosen operations failing.
#[derive(Default)]
struct FaultyFs {
    fail_link: bool,
    fail_remove: bool,
}

impl PublishFs for FaultyFs {
    fn hard_link(&self, from: &Path, to: &Path) -> io::Result<()> {
        if self.fail_link {
            return Err(io::Error::other("injected link failure"));
        }
        RealFs.hard_link(from, to)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        if self.fail_remove {
            return Err(io::Error::other("injected unlink failure"));
        }
        RealFs.remove_file(path)
    }

    fn sync_dir(&self, dir: &Path) -> io::Result<()> {
        RealFs.sync_dir(dir)
    }
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
        .publish_with(&FaultyFs {
            fail_link: true,
            ..FaultyFs::default()
        })
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
        .publish_with(&FaultyFs {
            fail_remove: true,
            ..FaultyFs::default()
        })
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
        .publish_with(&FaultyFs {
            fail_link: true,
            fail_remove: true,
        })
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
