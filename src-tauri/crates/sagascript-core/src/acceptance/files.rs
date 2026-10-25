//! Narrow file access helpers for acceptance flows.
//!
//! Path-component checks and post-open identity checks reduce symlink races,
//! but ordinary path APIs cannot make the whole lookup atomic against a
//! process that can mutate the same directories concurrently.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Component, Path};

const MAX_FIXTURE_BYTES: u64 = 1024 * 1024;
const ONBOARDING_KEY: &str = "has_completed_onboarding";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureError {
    Unavailable,
    Invalid,
    TooLarge,
}

/// Read just the onboarding boolean from a small JSON fixture.
///
/// Other keys are parsed and discarded. Their values are never returned or
/// included in errors or logs.
pub fn read_onboarding_fixture(path: &Path) -> Result<bool, FixtureError> {
    let before = checked_regular_file(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Avoid blocking if a checked regular file is raced into a FIFO.
        #[cfg(target_os = "linux")]
        const OPEN_FLAGS: i32 = 0x800; // O_NONBLOCK
        #[cfg(target_os = "macos")]
        const OPEN_FLAGS: i32 = 0x4; // O_NONBLOCK
        options.custom_flags(OPEN_FLAGS);
    }
    let file = options.open(path).map_err(|_| FixtureError::Unavailable)?;
    verify_open_file(path, &file, &before)?;

    let mut bytes = Vec::with_capacity(4096);
    file.take(MAX_FIXTURE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| FixtureError::Unavailable)?;
    if bytes.len() as u64 > MAX_FIXTURE_BYTES {
        return Err(FixtureError::TooLarge);
    }
    parse_onboarding(&bytes)
}

/// Publish receipt bytes as a complete, private file without replacing a name.
pub fn write_receipt_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_receipt_new_with(path, bytes, &crate::fsutil::RealFs)
}

fn write_receipt_new_with(
    path: &Path,
    bytes: &[u8],
    ops: &dyn crate::fsutil::PublishFs,
) -> io::Result<()> {
    check_receipt_destination(path)?;
    let staged = crate::fsutil::StagedFile::create_with(path, bytes, ops)
        .map_err(|_| io::Error::other("receipt staging failed"))?;
    staged
        .publish_with(ops)
        .map(|_| ())
        .map_err(|_| io::Error::other("receipt publication failed"))
}

fn check_receipt_destination(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    check_components(parent).map_err(|error| match error {
        FixtureError::Invalid => {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid receipt folder")
        }
        FixtureError::Unavailable | FixtureError::TooLarge => {
            io::Error::new(io::ErrorKind::NotFound, "receipt folder unavailable")
        }
    })?;
    match fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "receipt destination already exists",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(io::Error::other("receipt destination unavailable")),
    }
}

fn checked_regular_file(path: &Path) -> Result<fs::Metadata, FixtureError> {
    check_components(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| FixtureError::Unavailable)?;
    if metadata.file_type().is_symlink() {
        return Err(FixtureError::Invalid);
    }
    if !metadata.is_file() {
        return Err(FixtureError::Invalid);
    }
    Ok(metadata)
}

fn check_components(path: &Path) -> Result<(), FixtureError> {
    let mut current = Path::new("").to_path_buf();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => {
                // A Windows drive prefix is not a filesystem entry until its
                // root component is appended (notably canonical \\?\ paths).
                current.push(prefix.as_os_str());
                continue;
            }
            Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => continue,
            Component::ParentDir => current.push(component.as_os_str()),
            Component::Normal(name) => current.push(name),
        }
        let metadata = fs::symlink_metadata(&current).map_err(|_| FixtureError::Unavailable)?;
        if metadata.file_type().is_symlink() {
            return Err(FixtureError::Invalid);
        }
    }
    Ok(())
}

fn verify_open_file(path: &Path, file: &File, before: &fs::Metadata) -> Result<(), FixtureError> {
    let after_path = fs::symlink_metadata(path).map_err(|_| FixtureError::Unavailable)?;
    let opened = file.metadata().map_err(|_| FixtureError::Unavailable)?;
    if after_path.file_type().is_symlink()
        || !after_path.is_file()
        || !opened.is_file()
        || !same_file(before, &after_path)
        || !same_file(before, &opened)
    {
        return Err(FixtureError::Invalid);
    }
    Ok(())
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}

#[cfg(not(unix))]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}

fn parse_onboarding(bytes: &[u8]) -> Result<bool, FixtureError> {
    use serde::de::{MapAccess, Visitor};
    use serde_json::Deserializer;
    use std::fmt;

    struct FixtureVisitor;
    impl<'de> Visitor<'de> for FixtureVisitor {
        type Value = bool;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a JSON object containing the onboarding boolean")
        }

        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut seen = HashSet::new();
            let mut result = None;
            while let Some(key) = map.next_key::<String>()? {
                if !seen.insert(key.clone()) {
                    return Err(serde::de::Error::custom("duplicate key"));
                }
                if key == ONBOARDING_KEY {
                    let value = map.next_value::<bool>()?;
                    result = Some(value);
                } else {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
            result.ok_or_else(|| serde::de::Error::custom("missing onboarding key"))
        }
    }

    let mut deserializer = Deserializer::from_slice(bytes);
    let value = serde::de::Deserializer::deserialize_map(&mut deserializer, FixtureVisitor)
        .map_err(|_| FixtureError::Invalid)?;
    deserializer.end().map_err(|_| FixtureError::Invalid)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sagascript_testsupport::TempDir;

    #[test]
    fn reads_only_boolean_and_rejects_bad_json_shapes() {
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).unwrap();
        let path = root.join("fixture.json");
        for (json, expected) in [
            (
                br#"{"has_completed_onboarding":true,"other":{"private":"x"}}"#.as_slice(),
                Ok(true),
            ),
            (
                br#"{"has_completed_onboarding":false}"#.as_slice(),
                Ok(false),
            ),
            (br#"{}"#.as_slice(), Err(FixtureError::Invalid)),
            (
                br#"{"has_completed_onboarding":0}"#.as_slice(),
                Err(FixtureError::Invalid),
            ),
            (
                br#"{"has_completed_onboarding":true,"has_completed_onboarding":false}"#.as_slice(),
                Err(FixtureError::Invalid),
            ),
            (
                br#"{"other":1,"other":2,"has_completed_onboarding":true}"#.as_slice(),
                Err(FixtureError::Invalid),
            ),
            (
                br#"{"has_completed_onboarding":true} trailing"#.as_slice(),
                Err(FixtureError::Invalid),
            ),
        ] {
            fs::write(&path, json).unwrap();
            assert_eq!(read_onboarding_fixture(&path), expected);
        }
    }

    #[test]
    fn fixture_missing_directory_and_size_are_rejected_without_mutation() {
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).unwrap();
        let missing = root.join("missing.json");
        assert_eq!(
            read_onboarding_fixture(&missing),
            Err(FixtureError::Unavailable)
        );
        let folder = root.join("folder");
        fs::create_dir(&folder).unwrap();
        assert_eq!(read_onboarding_fixture(&folder), Err(FixtureError::Invalid));
        let too_big = root.join("large.json");
        fs::write(&too_big, vec![b' '; MAX_FIXTURE_BYTES as usize + 1]).unwrap();
        assert_eq!(
            read_onboarding_fixture(&too_big),
            Err(FixtureError::TooLarge)
        );
        let mut entries = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        entries.sort();
        assert_eq!(entries, ["folder", "large.json"]);
        assert!(
            !missing.exists(),
            "reading a missing fixture creates nothing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fixture_rejects_symlinks_and_receipt_publication_is_exclusive() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).unwrap();
        let target = root.join("target.json");
        fs::write(&target, br#"{"has_completed_onboarding":true}"#).unwrap();
        let link = root.join("link.json");
        symlink(&target, &link).unwrap();
        assert_eq!(read_onboarding_fixture(&link), Err(FixtureError::Invalid));
        let dangling = root.join("dangling.json");
        symlink(root.join("absent"), &dangling).unwrap();
        assert_eq!(
            read_onboarding_fixture(&dangling),
            Err(FixtureError::Invalid)
        );

        let receipt = root.join("receipt.bin");
        write_receipt_new(&receipt, b"receipt").unwrap();
        assert_eq!(fs::read(&receipt).unwrap(), b"receipt");
        assert_eq!(
            fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(write_receipt_new(&receipt, b"overwrite").is_err());
        assert_eq!(fs::read(&receipt).unwrap(), b"receipt");
    }

    #[cfg(unix)]
    #[test]
    fn receipt_rejects_symlink_destinations_and_symlink_parent_components() {
        use std::os::unix::fs::symlink;
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).unwrap();
        let target = root.join("target.bin");
        fs::write(&target, b"keep").unwrap();

        let regular_link = root.join("regular-link.bin");
        symlink(&target, &regular_link).unwrap();
        assert_eq!(
            write_receipt_new(&regular_link, b"receipt")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        let dangling_link = root.join("dangling-link.bin");
        symlink(root.join("absent"), &dangling_link).unwrap();
        assert_eq!(
            write_receipt_new(&dangling_link, b"receipt")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );

        let real_parent = root.join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let parent_link = root.join("parent-link");
        symlink(&real_parent, &parent_link).unwrap();
        assert_eq!(
            write_receipt_new(&parent_link.join("receipt.bin"), b"receipt")
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            write_receipt_new(&real_parent, b"receipt")
                .unwrap_err()
                .kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        assert!(!real_parent.join("receipt.bin").exists());
    }

    #[test]
    fn receipt_staging_sync_and_publish_failures_publish_nothing() {
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).unwrap();
        let cases = [
            (
                "create",
                crate::test_support::FaultyFs::new().failing_create(),
            ),
            (
                "sync",
                crate::test_support::FaultyFs::new().failing_sync_file(io::ErrorKind::StorageFull),
            ),
            (
                "publish",
                crate::test_support::FaultyFs::new().failing_link(1),
            ),
        ];
        for (name, ops) in cases {
            let destination = root.join(format!("{name}.receipt"));
            assert!(write_receipt_new_with(&destination, b"receipt", &ops).is_err());
            assert!(!destination.exists());
            assert!(crate::fsutil::staging_leftovers(&destination).is_empty());
        }
    }

    #[test]
    fn receipt_write_failure_does_not_create_destination() {
        let dir = TempDir::new();
        let root = fs::canonicalize(dir.path()).unwrap();
        let path = root.join("absent").join("receipt");
        assert!(write_receipt_new(&path, b"receipt").is_err());
        assert!(!path.exists());
    }
}
