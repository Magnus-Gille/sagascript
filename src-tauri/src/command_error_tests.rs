use super::*;
use crate::contract;
use sagascript_core::error::DictationError;
use sagascript_core::recording::RecordingError;
use sagascript_core::settings::SettingsError;
use serde_json::{json, Map, Value};

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
        wire(DictationError::MicrophonePermissionPending),
        json!({
            "code": "microphone_permission_pending",
            "message": "Microphone permission is awaiting your response. Choose Allow in the macOS dialog, then start recording again.",
            "retryable": true,
        })
    );
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
    let timed_out = wire(DictationError::TranscriptionTimedOut(
        "Transcription timed out after 60s (inference aborted)".into(),
    ));
    assert_eq!(timed_out["code"], "transcription_timed_out");
    assert_eq!(
        timed_out["retryable"], false,
        "a timeout is not offered as a retry: the same input times out again"
    );
}

#[test]
fn a_settings_error_keeps_its_code_and_its_text_on_the_wire() {
    assert_eq!(
        wire(SettingsError::DictionaryBlocksLanguageChange {
            profile_id: "default".into()
        }),
        json!({
            "code": "dictionary_blocks_language_change",
            "message": "Profile 'default' has a personal dictionary; clear it before changing the profile language",
            "retryable": false,
        })
    );
    assert_eq!(
        wire(SettingsError::Storage("Failed to lock settings: x".into()))["code"],
        "settings_storage_failed"
    );
    // The CLI's wrapper of the same error says "Settings error: …"; the webview
    // gets the text the command always showed, without the prefix.
    assert_eq!(
        wire(SettingsError::UnknownProfile { id: "p".into() })["message"],
        "Unknown profile 'p'"
    );
}

#[test]
fn the_meeting_and_evaluation_errors_keep_their_codes_and_texts() {
    use sagascript_core::diarization::eval::reference::ReferenceError;
    use sagascript_core::meeting::media::MediaError;
    use sagascript_core::meeting::reprocess::plan::ReprocessingPlanError;
    use sagascript_core::meeting::reprocess::proposal::ProposalError;
    use sagascript_core::meeting::review::MeetingReviewError;
    use sagascript_core::meeting::MeetingError;

    assert_eq!(
        wire(MeetingReviewError::ExpectedRevisionMismatch),
        json!({
            "code": "meeting_stale",
            "message": "correction expected revision does not match this review",
            "retryable": false,
        })
    );
    assert_eq!(
        wire(MeetingError::UnknownSpeaker)["code"],
        "meeting_invalid"
    );
    assert_eq!(
        wire(ReprocessingPlanError::StaleContext)["code"],
        "meeting_stale"
    );
    assert_eq!(
        wire(ProposalError::UnresolvedCorrections)["code"],
        "meeting_invalid"
    );
    assert_eq!(wire(MediaError::Changed)["code"], "meeting_stale");
    assert_eq!(
        wire(ReferenceError::TooManySpeakers)["code"],
        "diarization_error"
    );
}

/// One of every [`RecordingError`] variant, each with the text it would carry
/// in the app (a path in the encoder's message, to prove it is scrubbed). The
/// match fails to compile when a variant is added: add its sample here, and a
/// row follows in `command_errors.json`.
fn recording_errors() -> Vec<RecordingError> {
    let all = vec![
        RecordingError::Io {
            action: "create the recording folder",
            kind: std::io::ErrorKind::NotFound,
            os_code: Some(2),
        },
        RecordingError::DiskFull {
            action: "start a recording",
        },
        RecordingError::PermissionDenied {
            action: "open the microphone",
        },
        RecordingError::NameUnavailable,
        RecordingError::Locked,
        RecordingError::NotRecording,
        RecordingError::ManifestInvalid {
            reason: "the manifest is not JSON",
        },
        RecordingError::ManifestTooNew {
            required: 9,
            supported: 1,
        },
        RecordingError::UnsupportedFormat {
            reason: "a source delivers one or two channels at a real sample rate",
        },
        RecordingError::InvalidInput {
            reason: "a recording is named by one plain folder name",
        },
        RecordingError::UnknownTrack,
        RecordingError::TrackFinished,
        RecordingError::CorruptTrack {
            reason: "not a FLAC file",
        },
        RecordingError::SourceUnavailable {
            reason: "no input device",
        },
        RecordingError::SessionFailed {
            reason: "the recording writer stopped unexpectedly",
        },
        RecordingError::DeviceChanged {
            reason: "device_changed",
        },
        RecordingError::RecoveryNeeded,
        RecordingError::NotRegularFile {
            action: "open a track file",
        },
        RecordingError::JournalTooLarge,
        RecordingError::Encoder {
            message: "failed at /Users/x/Music/Sagascript/a/mic.flac".into(),
        },
    ];
    for error in &all {
        match error {
            RecordingError::Io { .. }
            | RecordingError::DiskFull { .. }
            | RecordingError::PermissionDenied { .. }
            | RecordingError::NameUnavailable
            | RecordingError::Locked
            | RecordingError::NotRecording
            | RecordingError::ManifestInvalid { .. }
            | RecordingError::ManifestTooNew { .. }
            | RecordingError::UnsupportedFormat { .. }
            | RecordingError::InvalidInput { .. }
            | RecordingError::UnknownTrack
            | RecordingError::TrackFinished
            | RecordingError::CorruptTrack { .. }
            | RecordingError::SourceUnavailable { .. }
            | RecordingError::SessionFailed { .. }
            | RecordingError::DeviceChanged { .. }
            | RecordingError::RecoveryNeeded
            | RecordingError::NotRegularFile { .. }
            | RecordingError::JournalTooLarge
            | RecordingError::Encoder { .. } => {}
        }
    }
    all
}

/// The `command_errors.json` rows of the recorder: one per code, keyed by it.
fn recording_rows() -> Map<String, Value> {
    recording_errors()
        .into_iter()
        .map(|error| (error.code().to_owned(), wire(error)))
        .collect()
}

#[test]
fn a_recording_error_keeps_its_code_retry_hint_and_text_on_the_wire() {
    assert_eq!(
        wire(RecordingError::PermissionDenied {
            action: "open the microphone"
        }),
        json!({
            "code": "recording_permission_denied",
            "message": "permission was denied; could not open the microphone",
            "retryable": false,
        })
    );
    assert_eq!(
        wire(RecordingError::DiskFull {
            action: "start a recording"
        }),
        json!({
            "code": "recording_disk_full",
            "message": "the disk is full; could not start a recording",
            "retryable": true,
        })
    );
    assert_eq!(wire(RecordingError::Locked)["retryable"], true);
    assert_eq!(wire(RecordingError::RecoveryNeeded)["retryable"], false);
}

#[test]
fn every_recording_error_has_a_distinct_code_a_row_and_no_path() {
    let rows = recording_rows();
    let errors = recording_errors();
    assert_eq!(rows.len(), errors.len(), "two samples share a code");
    for error in errors {
        let code = error.code();
        let row = &rows[code];
        assert_eq!(row["code"], code);
        assert_eq!(row["retryable"], error.retryable(), "{code}");
        assert!(code.starts_with("recording_"), "{code}");
        let message = row["message"].as_str().expect("message is a string");
        assert!(!message.contains("/Users/x"), "{code}: {message}");
        // The same error through the dictation error the CLI uses has one
        // representation on the wire, with the app's text.
        let through_dictation = CommandError::from(DictationError::Recording(error.clone()));
        assert!(matches!(through_dictation, CommandError::Recording(_)));
        assert_eq!(wire(through_dictation), *row, "{code}");
    }
}

#[test]
fn every_recording_code_has_a_row_in_the_shared_fixture() {
    if contract::blessing() {
        return; // the fixture is being rewritten; the next run reads it
    }
    let fixture = contract::read_fixture("command_errors.json");
    for (code, row) in recording_rows() {
        assert_eq!(
            fixture.get(&code),
            Some(&row),
            "contracts/transport/command_errors.json has no up-to-date row for {code}"
        );
    }
    // The codes the recorder window branches on (RECORDING_ERROR_CODES in
    // src/lib/record-types.ts) are all rows.
    for code in [
        "recording_permission_denied",
        "recording_source_unavailable",
        "recording_disk_full",
        "recording_device_changed",
        "recording_recovery_needed",
    ] {
        assert!(fixture.get(code).is_some(), "{code} has no row");
    }
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
/// so a path that entered through an error's text (a recovery draft's file, a
/// system error) never reaches the webview either.
#[test]
fn the_wire_message_never_carries_an_absolute_path() {
    for error in [
        CommandError::Io(format!("Could not save: {SWIFT_MESSAGE}")),
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
        SettingsError::NoProfiles.into(),
        RecordingError::DiskFull { action: "x" }.into(),
        CommandError::InvalidInput("x".into()),
        CommandError::NotFound("x".into()),
        CommandError::Unavailable("x".into()),
        CommandError::Io("x".into()),
        CommandError::Clipboard("x".into()),
        CommandError::ExportExists("x".into()),
        CommandError::LaunchFailed("x".into()),
        CommandError::DownloadFailed("x".into()),
        CommandError::AccessibilityPermissionDenied("x".into()),
        CommandError::PasteFailed("x".into()),
        CommandError::InvalidInput("x".into()).because("y"),
        DictationError::TranscriptionFailed("x".into()).into(),
        DictationError::engine("engine_busy", true, "x").into(),
        DictationError::MicrophonePermissionPending.into(),
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
fn a_fixed_message_keeps_its_cause_for_the_log_and_never_sends_it() {
    let error =
        CommandError::InvalidInput("Could not read a valid, bounded meeting proposal".into())
            .because("Failed to open /Users/x/Documents/a.json: No such file or directory");
    assert_eq!(error.code(), "invalid_input");
    assert_eq!(
        error.to_string(),
        "Could not read a valid, bounded meeting proposal"
    );
    let cause = std::error::Error::source(&error)
        .expect("the cause is kept as the error's source")
        .to_string();
    assert!(cause.contains("No such file or directory"), "{cause}");
    assert!(
        !cause.contains("/Users/x"),
        "the kept cause is scrubbed like the log: {cause}"
    );
    assert!(!format!("{error:?}").contains("/Users/x"), "{error:?}");
    // The webview gets the code and the fixed text, not the cause.
    assert_eq!(
        wire(error),
        json!({
            "code": "invalid_input",
            "message": "Could not read a valid, bounded meeting proposal",
            "retryable": false,
        })
    );
    let busy = CommandError::Busy("busy".into()).because("worker held the lock");
    assert!(busy.retryable(), "a cause does not change the retry hint");
    assert_eq!(busy.code(), "busy");
}

/// The shared fixture the frontend parses (src/test/transport.test.ts): real
/// serializations of the errors the GUI shows, including the Swift-style engine
/// failure, so the TypeScript side proves it can display them without the path.
#[test]
fn command_error_fixture_matches_what_rust_sends() {
    let mut actual = json!({
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
        "engine_model_missing": wire(DictationError::engine(
            "engine_model_missing",
            false,
            "Pianissimo engine: Model directory does not exist: /Users/x/Library/Application Support/Sagascript/models/foo",
        )),
        "microphone_permission_pending": wire(DictationError::MicrophonePermissionPending),
        "no_audio_captured": wire(DictationError::NoAudioCaptured),
        "paste_error": wire(CommandError::PasteFailed("Accessibility permission denied".into())),
        "dictionary_blocks_language_change": wire(SettingsError::DictionaryBlocksLanguageChange {
            profile_id: "default".into()
        }),
        "unknown_profile": wire(SettingsError::UnknownDictationProfile { id: "x".into() }),
        "invalid_input": wire(CommandError::InvalidInput("Invalid transcription result ID.".into())),
        "export_exists": wire(CommandError::ExportExists(
            "That file already exists. Choose a new name; no existing file was changed.".into()
        )),
        "meeting_stale": wire(sagascript_core::meeting::review::MeetingReviewError::ExpectedRevisionMismatch),
    });
    // Every code the recorder can send, one row each.
    actual
        .as_object_mut()
        .expect("the fixture is an object")
        .extend(recording_rows());
    contract::assert_fixture("command_errors.json", &actual);
}
