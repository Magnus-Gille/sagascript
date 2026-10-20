//! Progress and cancellation of a meeting job (#343, #374): the control a caller who owns the job
//! hands to the pipeline and to reprocessing, and the phases they report.
//!
//! Cancellation is cooperative: the caller raises the flag and every long step looks at it at
//! its checkpoints, so a cancelled job stops at the next one and releases what it holds.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::DictationError;

/// A step of a meeting job, in the order a run reaches them. The wire name (`snake_case`) is what
/// the app puts in a job snapshot and its frontend labels.
#[derive(Clone, Copy, Debug, serde::Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeetingPhase {
    Preparing,
    Decoding,
    LoadingModel,
    Analyzing,
    Clustering,
    Finalizing,
}

/// What a caller who owns a meeting job hands the run: the flag that cancels it and where it
/// reports its phase. The caller owns the flag and must not clear it during a job.
pub struct MeetingControl<'a> {
    pub cancellation: &'a AtomicBool,
    pub progress: &'a (dyn Fn(MeetingPhase) + Sync),
}

impl MeetingControl<'_> {
    /// `Err(Cancelled)` once the caller raised the flag.
    pub fn check(&self) -> Result<(), DictationError> {
        if self.cancellation.load(Ordering::Acquire) {
            return Err(DictationError::Cancelled);
        }
        Ok(())
    }
}

/// Check for cancellation, report `phase`, check again. Nothing happens without a control.
pub fn meeting_checkpoint(
    control: Option<&MeetingControl<'_>>,
    phase: MeetingPhase,
) -> Result<(), DictationError> {
    if let Some(control) = control {
        control.check()?;
        (control.progress)(phase);
        control.check()?;
    }
    Ok(())
}

/// Check for cancellation; nothing happens without a control.
pub fn meeting_check(control: Option<&MeetingControl<'_>>) -> Result<(), DictationError> {
    if let Some(control) = control {
        control.check()?;
    }
    Ok(())
}
