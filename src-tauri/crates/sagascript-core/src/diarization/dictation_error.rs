//! The diarization evaluation error enums as [`DictationError`]s (ADR 0006,
//! #360 part 2): all of them are [`DictationError::DiarizationError`], with the
//! enum's own text. The CLI's `diarization` commands keep their own wording and
//! do not use these conversions.

use crate::diarization::eval::evaluation::EvaluationError;
use crate::diarization::eval::qualification::QualificationError;
use crate::diarization::eval::reference::ReferenceError;
use crate::diarization::eval::strata::StrataError;
use crate::diarization::report::ReportError;
use crate::error::DictationError;

macro_rules! diarization_error {
    ($($ty:ty),+ $(,)?) => {$(
        impl From<$ty> for DictationError {
            fn from(error: $ty) -> Self {
                DictationError::DiarizationError(error.to_string())
            }
        }
    )+};
}

diarization_error!(
    ReferenceError,
    EvaluationError,
    QualificationError,
    StrataError,
    ReportError,
);

#[cfg(test)]
#[path = "dictation_error_tests.rs"]
mod tests;
