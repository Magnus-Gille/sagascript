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
