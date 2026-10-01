//! whisper.cpp log routing with one targeted downgrade.
//!
//! whisper.cpp built with Core ML always probes for `<model>-encoder.mlmodelc`
//! and logs an ERROR when it is absent. KB/NB fine-tunes have no Core ML
//! encoder by design, so for those models (and only those) the exact
//! "failed to load Core ML model" line is downgraded to DEBUG. Every other
//! native message keeps its level and the `whisper_rs::whisper_logging_hook`
//! target, so existing log filters behave as before.

use std::collections::HashSet;
use std::ffi::{c_char, c_void, CStr};
use std::sync::{Mutex, Once};

use whisper_rs::GGMLLogLevel;

/// Mirrors whisper-rs-sys's `ggml_log_level` (unsigned except on MSVC Windows).
#[cfg(any(not(windows), target_env = "gnu"))]
#[allow(non_camel_case_types)]
type ggml_log_level = u32;
#[cfg(all(windows, not(target_env = "gnu")))]
#[allow(non_camel_case_types)]
type ggml_log_level = i32;

const CORE_ML_LOAD_FAILURE: &str = "failed to load Core ML model from '";
const TARGET: &str = "whisper_rs::whisper_logging_hook";

/// Encoder directory names (e.g. `kb-whisper-large-encoder.mlmodelc`) whose
/// absence is expected.
static EXPECTED_MISSING: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static INSTALL: Once = Once::new();

/// Record that whisper.cpp will probe for `dirname` and that Sagascript knows
/// no such encoder exists for the model being loaded.
pub(crate) fn expect_missing_coreml_encoder(dirname: &str) {
    let mut guard = EXPECTED_MISSING.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get_or_insert_with(HashSet::new)
        .insert(dirname.to_string());
}

/// True only for the Core ML load-failure line naming an encoder in `expected`.
pub(crate) fn is_expected_coreml_miss(message: &str, expected: &HashSet<String>) -> bool {
    let Some(start) = message.find(CORE_ML_LOAD_FAILURE) else {
        return false;
    };
    let rest = &message[start + CORE_ML_LOAD_FAILURE.len()..];
    let Some(end) = rest.find('\'') else {
        return false;
    };
    let path = &rest[..end];
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    expected.contains(name)
}

fn expected_miss(message: &str) -> bool {
    let guard = EXPECTED_MISSING.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .as_ref()
        .is_some_and(|set| is_expected_coreml_miss(message, set))
}

/// Route whisper.cpp logs through tracing (replacing whisper-rs's own hook,
/// which cannot downgrade individual messages). Safe to call repeatedly.
pub(crate) fn install() {
    // whisper-rs installs the GGML hook (and its default whisper hook); ours
    // replaces the whisper one afterwards.
    whisper_rs::install_logging_hooks();
    INSTALL.call_once(|| unsafe {
        whisper_rs::set_log_callback(Some(trampoline), std::ptr::null_mut());
    });
}

unsafe extern "C" fn trampoline(level: ggml_log_level, text: *const c_char, _: *mut c_void) {
    if text.is_null() {
        return;
    }
    // SAFETY: whisper.cpp passes a valid NUL-terminated string.
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    let text = text.trim();
    match GGMLLogLevel::from(level) {
        GGMLLogLevel::Error if expected_miss(text) => {
            tracing::debug!(target: TARGET, "{text} (expected: this model has no Core ML encoder)")
        }
        GGMLLogLevel::Error => tracing::error!(target: TARGET, "{text}"),
        GGMLLogLevel::Warn => tracing::warn!(target: TARGET, "{text}"),
        GGMLLogLevel::Info => tracing::info!(target: TARGET, "{text}"),
        GGMLLogLevel::Debug => tracing::debug!(target: TARGET, "{text}"),
        GGMLLogLevel::None | GGMLLogLevel::Cont => tracing::trace!(target: TARGET, "{text}"),
        GGMLLogLevel::Unknown(l) => {
            tracing::warn!(target: TARGET, "unknown log level {l}: message: {text}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::WhisperModel;

    const BASE_MSG: &str = "whisper_init_state: failed to load Core ML model from '/m/ggml-base-encoder.mlmodelc'";

    fn expected_for(model: WhisperModel) -> HashSet<String> {
        model.unsupported_coreml_probe_dirname().into_iter().collect()
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn model_without_encoder_is_suppressed() {
        const KB_MSG: &str = "whisper_init_state: failed to load Core ML model from '/Users/x/Library/Application Support/Sagascript/Models/kb-whisper-large-encoder.mlmodelc'";
        let expected = expected_for(WhisperModel::KbWhisperLarge);
        assert_eq!(
            WhisperModel::KbWhisperLarge.unsupported_coreml_probe_dirname().as_deref(),
            Some("kb-whisper-large-encoder.mlmodelc")
        );
        assert!(is_expected_coreml_miss(KB_MSG, &expected));
    }

    #[test]
    fn model_with_encoder_is_not_suppressed() {
        let expected = expected_for(WhisperModel::Base);
        assert!(expected.is_empty());
        assert!(!is_expected_coreml_miss(BASE_MSG, &expected));
        // A KB expectation must not hide a Base encoder failure either.
        assert!(!is_expected_coreml_miss(BASE_MSG, &expected_for(WhisperModel::KbWhisperLarge)));
    }

    #[test]
    fn other_errors_are_never_suppressed() {
        let expected = expected_for(WhisperModel::KbWhisperLarge);
        assert!(!is_expected_coreml_miss(
            "whisper_init_state: failed to load whisper model from 'kb-whisper-large-encoder.mlmodelc'",
            &expected
        ));
        assert!(!is_expected_coreml_miss("ggml_metal_init: error", &expected));
    }

    #[test]
    fn pins_exact_whisper_cpp_message_text() {
        // whisper.cpp (whisper_init_state) wording; if an upgrade changes it,
        // suppression silently stops and this test must be revisited.
        assert_eq!(CORE_ML_LOAD_FAILURE, "failed to load Core ML model from '");
        let expected: HashSet<String> =
            ["kb-whisper-large-encoder.mlmodelc".to_string()].into_iter().collect();
        assert!(is_expected_coreml_miss(
            "whisper_init_state: failed to load Core ML model from '/m/kb-whisper-large-encoder.mlmodelc'\n",
            &expected
        ));
        assert!(!is_expected_coreml_miss(
            "whisper_init_state: failed to load Core ML model /m/kb-whisper-large-encoder.mlmodelc",
            &expected
        ));
    }
}
