//! The one way a command writes settings (#370).
//!
//! A setter used to do two things by hand: write the file with
//! `store::update(|settings| …)` and then copy the written field into the
//! controller through its mutable settings accessor. Forgetting the second
//! half, or copying one field of several, left the controller stale until the
//! settings watcher happened to reload the file. Both halves live here now:
//!
//! 1. the change is applied to the latest file under the cross-process settings
//!    lock, so a field the CLI changed a moment ago is kept ([`store::update`]);
//! 2. the controller's copy is replaced by what was written, as a whole. The
//!    watcher replaces it as a whole too, and a partial mirror is how a setter
//!    ended up with a copy that matched the file in one field only.
//!
//! The functions return the settings that were written, so a command can act
//! on them (rebuild the profiles menu, warm an engine) without reading the file
//! again. [`AppController::settings_mut`] exists in tests only; a command that
//! needs to change settings goes through here.

use sagascript_core::settings::{store, Settings};

use super::{lock_controller, SharedController};
use crate::command_error::CommandError;

/// Apply `change` to the settings file and the controller's copy.
///
/// `Err` is the file's own error ([`CommandError::Unclassified`] until
/// #360 part 2 types the settings store); the controller is left as it was.
pub(crate) fn persist_and_sync(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings),
) -> Result<Settings, CommandError> {
    try_persist_and_sync(controller, |settings| {
        change(settings);
        Ok::<(), CommandError>(())
    })
}

/// [`persist_and_sync`] for a change that can be refused. The refusal is
/// checked against the latest file while the settings lock is held, so a
/// validation and the write it guards cannot race another writer. A refusal
/// reaches the caller as its own error (a typed one stays typed) and writes
/// nothing.
pub(crate) fn try_persist_and_sync<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
) -> Result<Settings, CommandError> {
    try_persist_and_sync_replacing(controller, change).map(|(_, written)| written)
}

/// [`try_persist_and_sync`] that also returns the controller's copy that the
/// write replaced, for a caller that compares before and after under the same
/// swap (the profile model change decides whether to warm an engine that way).
pub(crate) fn try_persist_and_sync_replacing<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
) -> Result<(Settings, Settings), CommandError> {
    // The store reports a refusal as text. The typed error is carried out of
    // the closure beside it, so the caller gets the error it produced.
    let mut refusal: Option<CommandError> = None;
    let written = store::try_update(|settings| {
        change(settings).map_err(|error| {
            let error: CommandError = error.into();
            let text = error.to_string();
            refusal = Some(error);
            text
        })
    });
    match written {
        Ok(written) => {
            let previous = lock_controller(controller).update_settings(written.clone());
            Ok((previous, written))
        }
        Err(text) => Err(refusal.unwrap_or(CommandError::Unclassified(text))),
    }
}

/// Correct the controller's copy without writing the file: after a refused
/// save, bring a field that the controller got ahead of or behind the file in
/// back in line with it. The one in-memory-only write; it never persists, and
/// it replaces the copy under one lock so a concurrent command sees the old
/// settings or the corrected ones.
pub(crate) fn correct_in_memory(
    controller: &SharedController,
    correct: impl FnOnce(&mut Settings),
) {
    let mut ctrl = lock_controller(controller);
    let mut settings = ctrl.settings().clone();
    correct(&mut settings);
    ctrl.update_settings(settings);
}
