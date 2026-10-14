//! Test-only helpers shared by the Sagascript workspace (#363).
//!
//! Core, the CLI and the app take this crate as a `[dev-dependency]`; no shipped
//! binary links it. It deliberately does not depend on `sagascript-core`:
//! core's own unit tests use these helpers, and a helper built on core types
//! would hand those tests a second, incompatible copy of each type. Fixtures
//! made of core types (`meeting_transcript()`) therefore live in
//! `sagascript_core::test_support`, compiled for core's unit tests and, through
//! core's `test-support` feature, for the tests of the crates above it.
//!
//! Rules these helpers encode (docs/adr/0004-test-strategy.md): fixtures never
//! touch real user state, and tests running in parallel never share a path.

// A helper that cannot set up its fixture fails the calling test by
// panicking, as test code does (src-tauri/clippy.toml allows it in tests; this
// crate is test code compiled as a library, so the clippy policy would count it).
#![allow(
    clippy::panic,
    reason = "test fixtures fail the calling test when they cannot be set up"
)]

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// A fresh, uniquely named directory under the system temporary directory,
/// removed with its contents when dropped, including when the test panics.
///
/// Each value is a new `sagascript-test-*` directory created atomically by
/// [`tempfile`], so tests running in parallel, in several worktrees or in
/// several `cargo test` processes at once never collide on a path (a fixed or
/// timestamp-derived name did, see docs/testing/integrated-pr-review.md).
/// Keep the value alive for as long as the test uses the directory: a
/// `let _ = TempDir::new();` or a `(_, path)` pattern drops it at once.
///
/// It dereferences to [`Path`], so `dir.join("name")` and `&dir` where a
/// `&Path` is expected work, and it implements `AsRef<Path>` for the
/// `std::fs` functions.
#[derive(Debug)]
#[must_use = "the directory is removed as soon as the TempDir is dropped"]
pub struct TempDir(tempfile::TempDir);

impl TempDir {
    /// Creates the directory, or panics if it cannot.
    pub fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("sagascript-test-")
            .tempdir()
            .unwrap_or_else(|error| panic!("create a temporary test directory: {error}"));
        Self(directory)
    }

    /// The directory's path. It exists until `self` is dropped.
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.path()
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

/// Lowercase hexadecimal SHA-256 of `bytes`: the form Sagascript records
/// source hashes in (`source_sha256`, diarization cache identities).
pub fn sha256_hex(bytes: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(bytes.as_ref()))
}

/// The variables [`isolated_cli_env`] passes through to a spawned CLI: the
/// ordinary platform paths the Windows loader, home and data directory lookup
/// (locally installed models) and temporary files need. Nothing else of the
/// test process's environment reaches the child, in particular no provider
/// or application credentials.
pub const ORDINARY_OS_ENV: [&str; 14] = [
    "HOME",
    "USER",
    "LOGNAME",
    "PATH",
    "LANG",
    "TMPDIR",
    "SystemRoot",
    "WINDIR",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_DATA_HOME",
];

/// Isolates a spawned `sagascript` CLI: clears the inherited environment
/// except [`ORDINARY_OS_ENV`] and points `SAGASCRIPT_SETTINGS_PATH` at
/// `settings`, so the child neither sees the test process's credentials nor
/// reads or writes the user's real settings (ADR 0004: the variable is set on
/// the `Command`, never with `std::env::set_var` in the test process).
pub fn isolated_cli_env<'a>(command: &'a mut Command, settings: &Path) -> &'a mut Command {
    command.env_clear();
    for name in ORDINARY_OS_ENV {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command.env("SAGASCRIPT_SETTINGS_PATH", settings)
}

/// The file name the app and the CLI give the settings file
/// (`SETTINGS_FILENAME` in `sagascript-core`'s `settings/store.rs`; a test
/// there keeps the two equal).
pub const SETTINGS_FILE_NAME: &str = "sagascript-settings.json";

/// A settings file of a test's own: [`SETTINGS_FILE_NAME`] in a fresh
/// [`TempDir`], not created yet.
///
/// The directory is the settings file's alone, so what the store keeps beside
/// the file (`glossary.txt`, `glossaries/`, the `.json.lock` file, `.bak`
/// recovery copies) stays in it too. In-process tests pass [`path`](Self::path)
/// to the store's path-taking functions (`load_from`, the `*_at` helpers); a
/// spawned CLI gets it through [`isolated_cli_env`]. Either way nothing reads or
/// writes the user's real settings (docs/adr/0004-test-strategy.md, rule 5).
pub fn fake_settings() -> FakeSettings {
    let dir = TempDir::new();
    let path = dir.join(SETTINGS_FILE_NAME);
    FakeSettings { dir, path }
}

/// A test's own settings file, from [`fake_settings`]. Dropping it removes the
/// directory and everything in it.
#[derive(Debug)]
#[must_use = "the settings directory is removed as soon as the FakeSettings is dropped"]
pub struct FakeSettings {
    dir: TempDir,
    path: PathBuf,
}

impl FakeSettings {
    /// The settings file, `<dir>/sagascript-settings.json`. It does not exist
    /// until something writes it.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The directory that holds the settings file and the files the store
    /// keeps beside it.
    pub fn dir(&self) -> &Path {
        self.dir.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};

    /// The variables `command` sets explicitly (`None` for a removed one).
    fn configured_env(command: &Command) -> HashMap<OsString, Option<OsString>> {
        command
            .get_envs()
            .map(|(name, value)| (name.to_os_string(), value.map(OsStr::to_os_string)))
            .collect()
    }

    #[test]
    fn isolated_cli_env_drops_credentials_and_keeps_ordinary_paths() {
        let settings = Path::new("isolated").join("settings.json");
        let mut command = Command::new("sagascript");
        command
            .env("OPENAI_API_KEY", "sk-test-not-a-real-key")
            .env("ANTHROPIC_API_KEY", "test-not-a-real-key")
            .env("SAGASCRIPT_SETTINGS_PATH", "real-user-settings.json");

        isolated_cli_env(&mut command, &settings);

        let env = configured_env(&command);
        for credential in ["OPENAI_API_KEY", "ANTHROPIC_API_KEY"] {
            assert!(
                !env.contains_key(OsStr::new(credential)),
                "{credential} reached the child"
            );
        }
        assert_eq!(
            env.get(OsStr::new("SAGASCRIPT_SETTINGS_PATH")),
            Some(&Some(settings.into_os_string()))
        );
        let path = std::env::var_os("PATH").expect("the test process has a PATH");
        assert_eq!(env.get(OsStr::new("PATH")), Some(&Some(path)));
        for name in env.keys() {
            assert!(
                name.as_os_str() == OsStr::new("SAGASCRIPT_SETTINGS_PATH")
                    || ORDINARY_OS_ENV
                        .iter()
                        .any(|ordinary| name.as_os_str() == OsStr::new(ordinary)),
                "{name:?} is not an ordinary OS variable"
            );
        }
    }

    #[test]
    fn sha256_hex_matches_the_standard_test_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn temp_dir_is_unique_and_removed_with_its_contents_on_drop() {
        let dir = TempDir::new();
        let other = TempDir::new();
        assert_ne!(dir.path(), other.path());
        let path = dir.to_path_buf();
        assert!(path.is_dir());
        std::fs::create_dir(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested").join("file.txt"), b"fixture").unwrap();

        drop(dir);

        assert!(!path.exists(), "{} survived the drop", path.display());
        assert!(other.is_dir(), "dropping one TempDir removed another");
    }

    #[test]
    fn temp_dir_is_removed_when_the_test_panics() {
        let mut created = None;
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let dir = TempDir::new();
            std::fs::write(dir.join("file.txt"), b"fixture").unwrap();
            created = Some(dir.to_path_buf());
            panic!("simulated test failure while the TempDir is alive");
        }));

        assert!(outcome.is_err());
        let path = created.expect("the closure created the directory");
        assert!(!path.exists(), "{} survived the panic", path.display());
    }

    #[test]
    fn fake_settings_is_an_unwritten_file_alone_in_its_own_directory() {
        let settings = fake_settings();
        let other = fake_settings();
        assert_ne!(settings.dir(), other.dir());
        assert_eq!(settings.path(), settings.dir().join(SETTINGS_FILE_NAME));
        assert!(settings.dir().is_dir());
        assert!(
            !settings.path().exists(),
            "the settings file is not created"
        );
        assert_eq!(std::fs::read_dir(settings.dir()).unwrap().count(), 0);

        let dir = settings.dir().to_path_buf();
        std::fs::write(settings.path(), b"{}").unwrap();
        std::fs::write(dir.join("glossary.txt"), b"term\n").unwrap();
        drop(settings);

        assert!(!dir.exists(), "{} survived the drop", dir.display());
        assert!(
            other.dir().is_dir(),
            "dropping one FakeSettings removed another"
        );
    }
}
