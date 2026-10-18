use super::*;

/// The five evaluation enums are diarization errors with their own text after
/// the usual prefix.
#[test]
fn the_evaluation_errors_are_diarization_errors() {
    let rows: Vec<(DictationError, &str)> = vec![
        (
            ReferenceError::TooManyIntervals.into(),
            "Diarization error: reference interval count exceeds 100000",
        ),
        (
            EvaluationError::InvalidCollar.into(),
            "Diarization error: collar_seconds must be finite and non-negative",
        ),
        (
            QualificationError::Serialization.into(),
            "Diarization error: qualification JSON serialization failed",
        ),
        (
            QualificationError::Reference(ReferenceError::TooManySpeakers).into(),
            "Diarization error: reference validation failed: reference speaker count exceeds 64",
        ),
        (
            StrataError::TooManyUemRegions.into(),
            "Diarization error: UEM contains more than 100000 regions",
        ),
        (
            ReportError::Invalid("speaker").into(),
            "Diarization error: invalid diarization report speaker",
        ),
    ];
    for (error, text) in rows {
        assert_eq!(error.code(), "diarization_error", "{error:?}");
        assert_eq!(error.to_string(), text, "{error:?}");
    }
}
