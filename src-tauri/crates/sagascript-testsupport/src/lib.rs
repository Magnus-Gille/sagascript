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
use std::path::Path;
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
