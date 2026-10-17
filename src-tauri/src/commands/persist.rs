//! The one way a command writes settings (#370).
//!
//! A setter used to do two things by hand: write the file with
//! `store::update(|settings| …)` and then copy the result into the controller
//! through its mutable settings accessor. Forgetting the second half left the
//! controller stale until the settings watcher happened to reload the file.
//! Both halves live here now, with the writers serialized, and a setter picks
//! how the controller follows the file:
//!
//! - [`persist_and_sync`] / [`try_persist_and_mirror`]: the controller takes
//!   over **only what the setter changed**. Ten setters always did this (see
//!   the table in `persist_tests.rs`), and it is not a detail. After a hot
//!   reload that could not register the file's shortcuts, the settings watcher
//!   keeps the controller on the shortcuts that *are* registered and detects the
//!   next change by comparing file and controller; key presses are resolved
//!   against the controller. A setter of an unrelated field that replaced the
//!   whole copy would install shortcuts the OS never registered, the watcher
//!   would see no difference, and the working shortcut would be dead until a
//!   restart. The same goes for the startup demotion of a Pianissimo selection
//!   on a host that cannot run it, which lives in the controller's copy only.
//! - [`try_persist_and_replace`]: the controller's whole copy becomes what was
//!   written. Only for a setter that applies the settings it has just written
//!   itself: a language change (profile models it reset), a profile's model, a
//!   dictionary, shortcuts the OS has just registered. They replaced the whole
//!   copy before this module existed.
//!
//! The functions return the settings that were written, so a command can act
//! on them (rebuild the profiles menu, warm an engine) without reading the file
//! again. `AppController::settings_mut` exists in tests only; a command that
//! needs to change settings goes through here.

use std::sync::Mutex;

use sagascript_core::settings::{store, Settings};
use sagascript_core::sync::lock_or_recover;

use super::{lock_controller, SharedController};
use crate::command_error::CommandError;

/// Lets one writer at a time through the whole of write-file-then-sync. The
/// file lock is released before the controller lock is taken, so two setters
/// could otherwise interleave between the two, and the one that wrote the file
/// first could install its older copy last. Poison recovery is safe because the
/// mutex guards no data. Order: this lock, then the file lock, then the
/// controller lock; nothing waits for this lock while holding the controller.
static PERSIST: Mutex<()> = Mutex::new(());
const PERSIST_LOCK: &str = "settings.persist";

/// Apply `change` to the settings file and, the same way, to the controller's
/// copy: nothing else of the controller's copy moves, even if the file differs.
///
/// `change` runs twice, so it must not depend on the copy it is given beyond
/// the fields it assigns; the setters assign a value they were handed.
///
/// `Err` is the file's own error ([`CommandError::Unclassified`] until
/// #360 part 2 types the settings store); the controller is left as it was.
pub(crate) fn persist_and_sync(
    controller: &SharedController,
    change: impl Fn(&mut Settings),
) -> Result<Settings, CommandError> {
    persist(
        controller,
        |settings| {
            change(settings);
            Ok::<(), CommandError>(())
        },
        |memory, _written| {
            let mut next = memory.clone();
            change(&mut next);
            next
        },
    )
    .map(|(_, written)| written)
}

/// [`persist_and_sync`] for a change that can be refused and whose effect on
/// the controller is a copy of what the file now holds: `mirror` copies the
/// changed fields from the written settings into the controller's copy.
///
/// The refusal is checked against the latest file while the settings lock is
/// held, so a validation and the write it guards cannot race another writer. A
/// refusal reaches the caller as its own error (a typed one stays typed) and
/// writes nothing.
pub(crate) fn try_persist_and_mirror<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
    mirror: impl FnOnce(&mut Settings, &Settings),
) -> Result<Settings, CommandError> {
    persist(controller, change, |memory, written| {
        let mut next = memory.clone();
        mirror(&mut next, written);
        next
    })
    .map(|(_, written)| written)
}

/// Apply `change` to the file and make the controller's whole copy what was
/// written. Returns the copy it replaced and the written settings, for a caller
/// that compares them (the profile model change decides whether to warm an
/// engine that way). See the module documentation for who may use this.
pub(crate) fn try_persist_and_replace<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
) -> Result<(Settings, Settings), CommandError> {
    persist(controller, change, |_memory, written| written.clone())
}

/// The shared core: one writer at a time, the file first, then the controller.
/// `follow` computes the controller's new copy from its current one and the
/// written settings; it runs under the controller lock and does no I/O.
fn persist<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
    follow: impl FnOnce(&Settings, &Settings) -> Settings,
) -> Result<(Settings, Settings), CommandError> {
    let _one_writer = lock_or_recover(&PERSIST, PERSIST_LOCK);
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
            let mut ctrl = lock_controller(controller);
            let next = follow(ctrl.settings(), &written);
            let previous = ctrl.update_settings(next);
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

#[cfg(test)]
#[path = "persist_tests.rs"]
mod tests;
