//! Shared, side-effect-bounded hotkey registration transitions.
//!
//! This module knows the OS-call order and resulting operational state. Callers
//! retain their own locking, health diagnostics, settings, and event behavior.

use super::OperationalHotkey;

/// The OS operations needed by a hotkey transition.
pub(crate) trait Registrar {
    fn register(&mut self, shortcuts: &[String]) -> Result<(), String>;
    fn unregister(&mut self, shortcuts: &[String]) -> Result<(), String>;
}

/// Result of cleaning up a failed change and, when possible, restoring the old
/// operational shortcuts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RollbackOutcome {
    /// Cleanup succeeded and the previous operational state is again known.
    Restored(OperationalHotkey),
    /// The new or partial registration could not be removed; the old shortcuts
    /// are deliberately not registered because both sets may be active.
    CleanupFailed(String),
    /// Cleanup succeeded but restoring the previous registered shortcuts failed.
    RestoreFailed(String),
}

impl RollbackOutcome {
    pub(crate) fn operational(&self) -> OperationalHotkey {
        match self {
            Self::Restored(previous) => previous.clone(),
            Self::CleanupFailed(_) => OperationalHotkey::Unknown,
            Self::RestoreFailed(_) => OperationalHotkey::Inactive,
        }
    }
}

/// Why a registration transition failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TransitionFailureCause {
    PreviousUnknown,
    UnregisterOld {
        error: String,
    },
    RegisterNew {
        error: String,
        rollback: RollbackOutcome,
    },
}

/// A failed transition and the operational state callers must record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransitionFailure {
    pub(crate) cause: TransitionFailureCause,
    pub(crate) operational: OperationalHotkey,
}

/// Replace the current operational shortcuts with `new`.
///
/// An unknown starting state is rejected before touching the OS. A failed old
/// unregister makes the result unknown. A failed new registration invokes the
/// same rollback used after settings persistence failures.
pub(crate) fn apply(
    previous: &OperationalHotkey,
    new: &[String],
    registrar: &mut impl Registrar,
) -> Result<OperationalHotkey, TransitionFailure> {
    if matches!(previous, OperationalHotkey::Unknown) {
        return Err(TransitionFailure {
            cause: TransitionFailureCause::PreviousUnknown,
            operational: OperationalHotkey::Unknown,
        });
    }

    if let OperationalHotkey::Registered(old) = previous {
        if let Err(error) = registrar.unregister(old) {
            return Err(TransitionFailure {
                cause: TransitionFailureCause::UnregisterOld { error },
                operational: OperationalHotkey::Unknown,
            });
        }
    }

    match registrar.register(new) {
        Ok(()) => Ok(OperationalHotkey::registered_many(new)),
        Err(error) => {
            let rollback = rollback(previous, new, registrar);
            Err(TransitionFailure {
                operational: rollback.operational(),
                cause: TransitionFailureCause::RegisterNew { error, rollback },
            })
        }
    }
}

/// Remove new or partially registered shortcuts and restore the previous
/// shortcuts when that previous state was registered.
///
/// Cleanup failure is terminal for this rollback attempt: the old shortcuts
/// must not be registered on top of a possibly active partial new set.
/// An unknown previous state is refused without OS calls. Production callers
/// cannot reach this case: `apply` refuses it, and persistence rollback follows
/// only a successful `apply`.
pub(crate) fn rollback(
    previous: &OperationalHotkey,
    new: &[String],
    registrar: &mut impl Registrar,
) -> RollbackOutcome {
    if matches!(previous, OperationalHotkey::Unknown) {
        return RollbackOutcome::Restored(OperationalHotkey::Unknown);
    }

    if let Err(error) = registrar.unregister(new) {
        return RollbackOutcome::CleanupFailed(error);
    }

    match previous {
        OperationalHotkey::Registered(old) => match registrar.register(old) {
            Ok(()) => RollbackOutcome::Restored(previous.clone()),
            Err(error) => RollbackOutcome::RestoreFailed(error),
        },
        OperationalHotkey::Inactive | OperationalHotkey::Unknown => {
            RollbackOutcome::Restored(previous.clone())
        }
    }
}

#[cfg(test)]
#[path = "transition_tests.rs"]
mod tests;
