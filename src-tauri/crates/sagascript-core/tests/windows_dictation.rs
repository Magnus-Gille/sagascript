#![cfg(target_os = "windows")]

//! Windows-only, opt-in integration coverage for the live dictation runtime.
//!
//! These tests are ignored because they need local Whisper model files and
//! speech recordings. They never download artifacts. The English test uses
//! the checked-in `test-audio/english-jfk.wav` by default (override it with
//! `SAGASCRIPT_WINDOWS_ENGLISH_AUDIO`); the bilingual test requires an
//! explicit local Swedish recording in `SAGASCRIPT_WINDOWS_SWEDISH_AUDIO`.
//!
//! Run the English gate with:
//!
//! ```text
//! cargo test -p sagascript-core --test windows_dictation english_fixture_reuses_loaded_runtime -- --ignored --nocapture
//! ```
//!
//! Run the bilingual gate after setting `SAGASCRIPT_WINDOWS_SWEDISH_AUDIO` to
//! a local WAV/MP3/etc. fixture and `SAGASCRIPT_WINDOWS_SWEDISH_EXPECTED_WORD`
//! to one word present in that recording.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use sagascript_core::audio::decoder::decode_audio_file;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::model;
use sagascript_core::transcription::whisper_backend::{
    DictationTimings, WhisperBackend,
};
use sagascript_core::transcription::TranscribeOptions;

const ENGLISH_AUDIO_ENV: &str = "SAGASCRIPT_WINDOWS_ENGLISH_AUDIO";
const SWEDISH_AUDIO_ENV: &str = "SAGASCRIPT_WINDOWS_SWEDISH_AUDIO";
const SWEDISH_EXPECTED_ENV: &str = "SAGASCRIPT_WINDOWS_SWEDISH_EXPECTED_WORD";

fn english_fixture() -> PathBuf {
    std::env::var_os(ENGLISH_AUDIO_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../test-audio/english-jfk.wav")
        })
}

fn explicit_fixture(env_name: &str) -> PathBuf {
    let value = std::env::var_os(env_name).unwrap_or_else(|| {
        panic!(
            "set {env_name} to an explicit local speech fixture before running this ignored test"
        )
    });
    PathBuf::from(value)
}

fn require_fixture(path: PathBuf) -> Vec<f32> {
    assert!(
        path.is_file(),
        "speech fixture does not exist: {}",
        path.display()
    );
    let audio = decode_audio_file(&path).unwrap_or_else(|error| {
        panic!("failed to decode speech fixture {}: {error}", path.display())
    });
    assert!(!audio.is_empty(), "speech fixture is empty: {}", path.display());
    audio
}

fn require_model(model: WhisperModel) {
    let path = model::model_path(model);
    assert!(
        model::is_model_downloaded(model),
        "model {} is not installed at {}; this integration test never downloads models",
        model.display_name(),
        path.display()
    );
}

fn assert_contains_expected_word(text: &str, expected: &str, fixture: &str) {
    let text = text.to_lowercase();
    let expected = expected.trim().to_lowercase();
    assert!(!expected.is_empty(), "expected word for {fixture} is empty");
    assert!(
        text.split_whitespace().any(|word| {
            word.trim_matches(|character: char| !character.is_alphanumeric()) == expected.as_str()
        }),
        "transcription for {fixture} did not contain expected word {expected:?}"
    );
}

#[test]
#[ignore = "requires the installed English model and local speech fixture"]
fn english_fixture_reuses_loaded_runtime() {
    let audio = require_fixture(english_fixture());
    require_model(WhisperModel::BaseEn);

    let backend = WhisperBackend::new();
    let options = TranscribeOptions::default();

    let mut first_timings = DictationTimings::default();
    let first = backend
        .transcribe_dictation(
            WhisperModel::BaseEn,
            &audio,
            Language::English,
            &options,
            &mut first_timings,
        )
        .expect("first English fixture transcription should succeed");
    assert_contains_expected_word(&first, "country", "English JFK fixture");
    assert!(
        !first_timings.model_cached,
        "the first selection on a fresh backend should load the model"
    );

    let mut second_timings = DictationTimings::default();
    let second = backend
        .transcribe_dictation(
            WhisperModel::BaseEn,
            &audio,
            Language::English,
            &options,
            &mut second_timings,
        )
        .expect("second English fixture transcription should succeed");
    assert_contains_expected_word(&second, "country", "English JFK fixture");
    assert!(
        second_timings.model_cached,
        "repeated English transcription should reuse the loaded runtime"
    );
    assert_eq!(backend.resident_models(), vec![WhisperModel::BaseEn]);
}

#[test]
#[ignore = "requires installed English/Swedish models and explicit local speech fixtures"]
fn concurrent_warmup_switching_keeps_bilingual_runtime_selection_atomic() {
    let english_audio = require_fixture(english_fixture());
    let swedish_audio = require_fixture(explicit_fixture(SWEDISH_AUDIO_ENV));
    let swedish_expected = std::env::var(SWEDISH_EXPECTED_ENV).unwrap_or_else(|_| {
        panic!(
            "set {SWEDISH_EXPECTED_ENV} to one expected word from the Swedish fixture"
        )
    });
    require_model(WhisperModel::BaseEn);
    require_model(WhisperModel::KbWhisperBase);

    let backend = Arc::new(WhisperBackend::new());
    let english_backend = Arc::clone(&backend);
    let english_warmup = thread::spawn(move || {
        english_backend.warmup_model(WhisperModel::BaseEn, Language::English)
    });
    let swedish_backend = Arc::clone(&backend);
    let swedish_warmup = thread::spawn(move || {
        swedish_backend.warmup_model(WhisperModel::KbWhisperBase, Language::Swedish)
    });

    english_warmup
        .join()
        .expect("English warmup thread should not panic")
        .expect("English warmup should succeed");
    swedish_warmup
        .join()
        .expect("Swedish warmup thread should not panic")
        .expect("Swedish warmup should succeed");

    let residents = backend.resident_models();
    assert_eq!(residents.len(), 2, "both bilingual runtimes should remain warm");
    assert!(residents.contains(&WhisperModel::BaseEn));
    assert!(residents.contains(&WhisperModel::KbWhisperBase));

    let options = TranscribeOptions::default();
    for repetition in 0..2 {
        let mut english_timings = DictationTimings::default();
        let english = backend
            .transcribe_dictation(
                WhisperModel::BaseEn,
                &english_audio,
                Language::English,
                &options,
                &mut english_timings,
            )
            .expect("English fixture transcription should succeed");
        assert_contains_expected_word(&english, "country", "English JFK fixture");
        assert!(
            english_timings.model_cached,
            "English repetition {repetition} should use an active or cached runtime"
        );

        let mut swedish_timings = DictationTimings::default();
        let swedish = backend
            .transcribe_dictation(
                WhisperModel::KbWhisperBase,
                &swedish_audio,
                Language::Swedish,
                &options,
                &mut swedish_timings,
            )
            .expect("Swedish fixture transcription should succeed");
        assert_contains_expected_word(&swedish, &swedish_expected, "Swedish fixture");
        assert!(
            swedish_timings.model_cached,
            "Swedish repetition {repetition} should use an active or cached runtime"
        );
    }
}
