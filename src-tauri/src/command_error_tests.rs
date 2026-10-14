use super::*;
use crate::contract;
use sagascript_core::error::DictationError;
use serde_json::{json, Value};

/// What the Swift engine host sends when a model file cannot be loaded
/// (`CoreMLEngine.swift`): an absolute path under the user's home directory,
/// with a space in it.
const SWIFT_MESSAGE: &str = "Failed to load model at /Users/x/Library/Application Support/Sagascript/models/foo.mlmodelc: The file couldn't be opened.";

fn wire(error: impl Into<CommandError>) -> Value {
    contract::to_value(error.into())
}

#[test]
fn a_command_error_is_code_message_retryable_on_the_wire() {
    assert_eq!(
        wire(CommandError::NotRecording),
        json!({
            "code": "not_recording",
            "message": "No training recording is active",
            "retryable": false,
        })
    );
    assert_eq!(
        wire(CommandError::Busy("Sagascript is busy.".into())),
        json!({ "code": "busy", "message": "Sagascript is busy.", "retryable": true })
    );
    assert_eq!(
        wire(CommandError::DictionaryChangedElsewhere),
        json!({
            "code": "dictionary_changed_elsewhere",
            "message": "Dictionary changed elsewhere: the persisted source no longer matches the editor baseline",
            "retryable": false,
        })
    );
}

#[test]
fn dictation_errors_keep_their_code_and_retry_hint() {
    assert_eq!(
        wire(DictationError::ModelBusy)["code"],
        "model_busy",
        "a busy model is its own code"
    );
    assert_eq!(wire(DictationError::ModelBusy)["retryable"], true);
    assert_eq!(
        wire(DictationError::NoAudioCaptured)["code"],
        "no_audio_captured"
    );
    assert_eq!(
        wire(DictationError::FileNotFound(
            "No such file or directory (os error 2)".into()
        )),
        json!({
            "code": "file_not_found",
            "message": "File decode error: Failed to open file: No such file or directory (os error 2)",
            "retryable": false,
        })
    );
    assert_eq!(
        wire(DictationError::TranscriptionTimedOut(
            "Transcription timed out after 60s (inference aborted)".into()
        ))["code"],
        "transcription_timed_out"
    );
}

#[test]
fn cancellation_has_one_representation() {
    let from_dictation = CommandError::from(DictationError::Cancelled);
    assert!(matches!(from_dictation, CommandError::Cancelled));
    assert_eq!(
        wire(CommandError::Cancelled),
        json!({ "code": "cancelled", "message": "Transcription cancelled.", "retryable": false })
    );
}

#[test]
fn an_engine_failure_keeps_its_own_code_retry_hint_and_loses_the_path() {
    let error = CommandError::from(DictationError::engine(
        "engine_model_load_failed",
        false,
        SWIFT_MESSAGE,
    ));
    let value = wire(error);
    assert_eq!(value["code"], "engine_model_load_failed");
    assert_eq!(value["retryable"], false);
    let message = value["message"].as_str().expect("message is a string");
    assert!(!message.contains("/Users/x"), "{message}");
    assert!(message.contains("foo.mlmodelc"), "{message}");

    let busy = wire(DictationError::engine("engine_busy", true, "queue full"));
    assert_eq!(
        (busy["code"].as_str(), busy["retryable"].as_bool()),
        (Some("engine_busy"), Some(true))
    );
}

/// The text of a command error is scrubbed again where the app serializes it,
/// so a path that entered through a plain `String` error (not yet typed, #360
/// part 2) never reaches the webview either.
#[test]
fn the_wire_message_never_carries_an_absolute_path() {
    for error in [
        CommandError::Unclassified(format!("Could not save: {SWIFT_MESSAGE}")),
        CommandError::HotkeyChangeFailed(
            "Failed to register /Users/x/Library/Preferences/a.plist".into(),
        ),
        CommandError::Busy("busy at /home/x/.local/share/a".into()),
        CommandError::from(DictationError::FileDecodeError(
            "Failed to read C:\\Users\\x\\AppData\\a.wav".into(),
        )),
    ] {
        let message = wire(error)["message"].as_str().expect("string").to_owned();
        for private in ["/Users/x", "/home/x", "C:\\Users\\x", "Application Support"] {
            assert!(!message.contains(private), "{private} in {message}");
        }
    }
}

#[test]
fn every_code_is_stable_snake_case() {
    let errors: Vec<CommandError> = vec![
        CommandError::Busy("x".into()),
        CommandError::NotRecording,
        CommandError::Cancelled,
        CommandError::DictionaryChangedElsewhere,
        CommandError::HotkeyInvalid("x".into()),
        CommandError::HotkeyChangeFailed("x".into()),
        CommandError::Unclassified("x".into()),
        DictationError::TranscriptionFailed("x".into()).into(),
        DictationError::engine("engine_busy", true, "x").into(),
    ];
    for error in errors {
        let code = error.code().to_owned();
        assert!(
            !code.is_empty()
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'),
            "{code:?} is not snake_case"
        );
    }
}

#[test]
fn plain_strings_become_unclassified_until_their_command_is_typed() {
    let error = CommandError::from("An update is being applied.");
    assert_eq!(error.code(), "unclassified");
    assert_eq!(error.to_string(), "An update is being applied.");
    let error: CommandError = String::from("x").into();
    assert!(matches!(error, CommandError::Unclassified(_)));
}

/// The shared fixture the frontend parses (src/test/transport.test.ts): real
/// serializations of the errors the GUI shows, including the Swift-style engine
/// failure, so the TypeScript side proves it can display them without the path.
#[test]
fn command_error_fixture_matches_what_rust_sends() {
    let actual = json!({
        "busy": wire(CommandError::Busy(
            "Cannot start recording while Sagascript is busy. Stop the current recording or wait for transcription to finish."
                .into()
        )),
        "not_recording": wire(CommandError::NotRecording),
        "cancelled": wire(CommandError::Cancelled),
        "dictionary_changed_elsewhere": wire(CommandError::DictionaryChangedElsewhere),
        "file_not_found": wire(DictationError::FileNotFound(
            "No such file or directory (os error 2)".into()
        )),
        "model_busy": wire(DictationError::ModelBusy),
        "transcription_timed_out": wire(DictationError::TranscriptionTimedOut(
            "Transcription timed out after 60s (inference aborted)".into()
        )),
        "engine_model_load_failed": wire(DictationError::engine(
            "engine_model_load_failed",
            false,
            &format!("Pianissimo engine: {SWIFT_MESSAGE}"),
        )),
        "engine_busy": wire(DictationError::engine("engine_busy", true, "Pianissimo engine: queue full")),
        "unclassified": wire(CommandError::Unclassified("Unknown dictation profile 'x'".into())),
    });
    contract::assert_fixture("command_errors.json", &actual);
}
