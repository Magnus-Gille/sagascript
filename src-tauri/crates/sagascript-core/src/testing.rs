//! Seams for the tests of this crate and the crates above it (#372).
//!
//! A few tests must reach something that no production code should: the
//! startup load of an explicit settings file, a hook in the middle of an engine
//! request, one pass of the idle supervisor. They used to be public items (and
//! a public field) of the modules they belong to; they are `pub(crate)` there
//! and these functions are the only way in from outside. The module is
//! `#[doc(hidden)]` and not part of the API.
//!
//! Not to be confused with [`crate::test_support`], which holds fixtures made of
//! core types and exists only under `cfg(test)` or the `test-support` feature.
//! The integration tests of this crate compile it as a plain library without
//! that feature, so a seam they need has to be here, always compiled; each is a
//! one-line call into the module that owns the behaviour.

use std::path::{Path, PathBuf};

use crate::settings::{store, LineupEnv, Settings};
use crate::transcription::engine_host::client::TestHook;
use crate::transcription::engine_host::{EngineHostClient, EngineHostConfig};

/// The startup load of the settings file at `path` (legacy locations imported
/// from `legacy_sources`, every pending migration run) with fixed machine state
/// instead of this machine's: [`store::load_at`].
pub fn load_settings_at(
    path: &Path,
    legacy_sources: &[PathBuf],
    machine: &LineupEnv<'_>,
) -> Settings {
    store::load_at(path, legacy_sources, machine)
}

/// Make `config`'s client call `hook` with the name of a point of the request
/// path (`"after_ready"`), so a test can interleave lifecycle operations
/// deterministically. Production code never sets it.
pub fn set_engine_test_hook(
    config: &mut EngineHostConfig,
    hook: impl Fn(&str) + Send + Sync + 'static,
) {
    config.test_hook = Some(TestHook(std::sync::Arc::new(hook)));
}

/// Run one idle-supervision pass of `client` now (the supervisor thread does
/// this on a timer).
pub fn run_idle_check_now(client: &EngineHostClient) {
    client.run_idle_check_now();
}
