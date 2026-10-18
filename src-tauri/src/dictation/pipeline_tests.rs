use super::*;

#[test]
fn elapsed_ms_if_success_only_marks_successful_operations_complete() {
    let start = Instant::now();
    assert!(elapsed_ms_if_success::<(), ()>(&Ok(()), start).is_some());
    assert!(elapsed_ms_if_success::<(), ()>(&Err(()), start).is_none());
}

/// The texts the hotkey path puts in the `error` event and the controller's
/// last error. #360 changed them: a timeout and a join error lost their
/// "Transcription failed: " prefix, and a cancellation is "Transcription
/// cancelled." instead of the old "Pianissimo transcription cancelled" or
/// "Cancelled". Pinned so a later change is deliberate.
#[test]
fn the_hotkey_path_reports_a_failed_dictation_with_these_texts() {
    use sagascript_core::error::DictationError;

    assert_eq!(
        dictation_failure_text(&DictationError::Cancelled),
        "Transcription cancelled."
    );
    assert_eq!(
        dictation_failure_text(&dictation_timed_out()),
        "Transcription timed out after 60s (inference aborted)"
    );
    assert_eq!(
        dictation_failure_text(&dictation_task_failed("task 7 panicked")),
        "Task join error: task 7 panicked"
    );
    assert_eq!(
        dictation_failure_text(&DictationError::TranscriptionFailed("no model".into())),
        "Transcription failed: no model"
    );
}

/// The Swift host reports the model file by absolute path; the event the
/// overlay and Settings show must not carry the user's home directory.
#[test]
fn an_engine_failure_reaches_the_error_event_without_the_users_path() {
    use sagascript_core::error::DictationError;

    let engine = DictationError::engine(
        "engine_model_load_failed",
        false,
        "Pianissimo engine: Failed to load model at /Users/x/Library/Application Support/Sagascript/models/foo.mlmodelc: boom",
    );
    assert_eq!(
        dictation_failure_text(&engine),
        "Pianissimo engine: Failed to load model at <path>/foo.mlmodelc: boom"
    );
}

/// The `error` event carries `{ code, message, retryable }` (ADR 0006, #360
/// part 2b): the failure's own code and retry hint, and the text it always
/// carried, so the Settings window decides "Try again" and "Download the model
/// again" by code and shows what it showed before.
#[test]
fn the_error_event_carries_the_code_the_retry_hint_and_the_same_text() {
    use sagascript_core::error::DictationError;

    let engine_busy = DictationError::engine("engine_busy", true, "Pianissimo engine: queue full");
    let cases = [
        (DictationError::ModelBusy, "model_busy", true),
        (DictationError::NoAudioCaptured, "no_audio_captured", true),
        (
            DictationError::TranscriptionFailed("no model".into()),
            "transcription_failed",
            false,
        ),
        (dictation_timed_out(), "transcription_timed_out", false),
        (
            dictation_task_failed("task 7 panicked"),
            "task_failed",
            false,
        ),
        (engine_busy, "engine_busy", true),
    ];
    for (error, code, retryable) in cases {
        let event = dictation_failure_event(&error);
        assert_eq!(event.code(), code, "{error}");
        assert_eq!(event.retryable(), retryable, "{error}");
        assert_eq!(event.message(), dictation_failure_text(&error), "{code}");
        assert_eq!(
            crate::contract::to_value(&event),
            serde_json::json!({
                "code": code,
                "message": dictation_failure_text(&error),
                "retryable": retryable,
            })
        );
    }
}

/// The Swift host's failure arrives in the `error` event with its own code
/// (`backendErrorRecovery` offers the model download for it) and without the
/// user's home directory.
#[test]
fn an_engine_failure_event_keeps_its_code_and_loses_the_path() {
    use sagascript_core::error::DictationError;

    let engine = DictationError::engine(
        "engine_model_missing",
        false,
        "Pianissimo engine: Model directory does not exist: /Users/x/Library/Application Support/Sagascript/models/foo",
    );
    let wire = crate::contract::to_value(dictation_failure_event(&engine));
    assert_eq!(wire["code"], "engine_model_missing");
    assert_eq!(wire["retryable"], false);
    assert_eq!(
        wire["message"],
        "Pianissimo engine: Model directory does not exist: <path>/foo"
    );
    assert!(!wire.to_string().contains("/Users/x"), "{wire}");
}
