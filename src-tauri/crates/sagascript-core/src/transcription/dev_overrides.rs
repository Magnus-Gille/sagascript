//! Development-only environment overrides.
//!
//! `SAGASCRIPT_ENGINE_HOST` (which executable is spawned as the engine host) and
//! `SAGASCRIPT_PIANISSIMO_MODEL_DIR` (which model directory is trusted without
//! manifest verification) are honored only in development builds: debug builds
//! or builds with the `dev-overrides` cargo feature. Release and signed builds
//! ignore them and log one warning per variable, so a hostile environment
//! cannot swap the sidecar or the model.

use std::ffi::OsString;
use std::sync::Mutex;

/// True when this build honors the development overrides.
pub const ENABLED: bool = cfg!(any(debug_assertions, feature = "dev-overrides"));

static WARNED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Apply the policy: a non-empty `value` is returned only when `enabled`;
/// otherwise it is dropped with a one-time warning naming `name`.
pub(crate) fn resolve(enabled: bool, name: &str, value: Option<OsString>) -> Option<OsString> {
    let value = value.filter(|v| !v.is_empty())?;
    if enabled {
        return Some(value);
    }
    let mut warned = WARNED.lock().unwrap_or_else(|e| e.into_inner());
    if !warned.iter().any(|n| n == name) {
        warned.push(name.to_string());
        tracing::warn!(
            variable = name,
            "ignoring development override in a release build (rebuild with the `dev-overrides` feature to use it)"
        );
    }
    None
}

/// The value of environment variable `name` if this build honors overrides.
pub fn env_override(name: &str) -> Option<OsString> {
    resolve(ENABLED, name, std::env::var_os(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honored_only_when_enabled() {
        assert_eq!(
            resolve(true, "X_TEST_A", Some("/dev/host".into())),
            Some(OsString::from("/dev/host"))
        );
        assert_eq!(resolve(false, "X_TEST_B", Some("/tmp/rogue-host".into())), None);
        assert_eq!(WARNED.lock().unwrap().iter().filter(|n| *n == "X_TEST_B").count(), 1);
        // The warning is emitted once per variable.
        assert_eq!(resolve(false, "X_TEST_B", Some("/tmp/rogue-host".into())), None);
        assert_eq!(WARNED.lock().unwrap().iter().filter(|n| *n == "X_TEST_B").count(), 1);
    }

    #[test]
    fn empty_and_unset_are_never_overrides() {
        for enabled in [true, false] {
            assert_eq!(resolve(enabled, "X_TEST_C", None), None);
            assert_eq!(resolve(enabled, "X_TEST_C", Some("".into())), None);
        }
        assert!(!WARNED.lock().unwrap().iter().any(|n| n == "X_TEST_C"));
    }

    #[test]
    fn debug_and_feature_builds_enable_overrides() {
        assert_eq!(ENABLED, cfg!(debug_assertions) || cfg!(feature = "dev-overrides"));
    }

    /// Only compiled into release builds without the feature
    /// (`cargo test --release -p sagascript-core --lib dev_overrides`).
    #[cfg(not(any(debug_assertions, feature = "dev-overrides")))]
    #[test]
    fn release_build_ignores_the_real_environment() {
        std::env::set_var("SAGASCRIPT_ENGINE_HOST", "/tmp/rogue-host");
        std::env::set_var("SAGASCRIPT_PIANISSIMO_MODEL_DIR", "/tmp/rogue-model");
        assert_eq!(crate::transcription::pianissimo_backend::resolve_host(), None);
        assert_eq!(crate::transcription::pianissimo_model::override_dir(), None);
    }
}
