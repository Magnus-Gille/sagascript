//! The one way a command writes settings (#370).
//!
//! A setter used to do two things by hand: write the file with
//! `machine::update(|settings| …)` and then copy the result into the controller
//! through its mutable settings accessor. Forgetting the second half left the
//! controller stale until the settings watcher happened to reload the file.
//! Both halves live here now, with the writers serialized.
//!
//! The controller's copy and the file differ on purpose in two ways, and a
//! setter must not erase either:
//!
//! - **Shortcuts the OS never registered.** After a hot reload that could not
//!   register the file's shortcuts, the settings watcher keeps the controller on
//!   the shortcuts that *are* registered and detects the next change by
//!   comparing file and controller; key presses are resolved against the
//!   controller. A setter that installed the file's shortcuts would make the
//!   watcher see no difference, and the working shortcut would be dead until a
//!   restart.
//! - **The demotion of Pianissimo.** `machine::load` demotes a selection of it on
//!   a host that cannot run it in memory only; the file keeps the user's choice.
//!   Every write here ends with the same demotion of the controller's copy, so
//!   the controller never holds a selection the host cannot run, whatever the
//!   file says and whichever way the setter followed the file.
//!
//! So a setter hands the controller **only what it wrote**, and the table in
//! `persist_tests.rs` states the fields of each:
//!
//! - [`persist_and_sync`]: the setter's change runs on the controller's copy as
//!   well as on the file.
//! - [`try_persist_and_mirror`]: for a change that can be refused, `mirror`
//!   copies the fields the change wrote from the written settings. A list of
//!   fields rather than "everything but the shortcuts": a third kind of
//!   deliberate difference would otherwise be erased by every setter until
//!   someone found it, as the two above were.
//! - [`try_persist_and_replace`]: the controller's whole copy becomes what was
//!   written. Only for the hotkey change, the one caller that has just
//!   registered the file's shortcuts with the OS: the controller adopting them
//!   is what makes them the operational ones.
//!
//! The functions return the settings that were written, and the controller's
//! copy before and after, so a command can act on them (rebuild the profiles
//! menu, warm an engine) without reading the file again.
//! `AppController::settings_mut` exists in tests only; a command that needs to
//! change settings goes through here.

use std::sync::Mutex;

use sagascript_core::machine;
use sagascript_core::settings::Settings;
use sagascript_core::sync::lock_or_recover;
use sagascript_core::transcription::pianissimo_backend::runtime_supported_on_this_os;

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

/// What a write left behind.
pub(crate) struct Persisted {
    /// The settings the file now holds. Not demoted: the file keeps the user's
    /// choice of an engine this host cannot run.
    pub(crate) written: Settings,
    /// The controller's copy before the write.
    pub(crate) previous: Settings,
    /// The controller's copy now, which the app runs on. `previous` and
    /// `current` are treated alike (both are demoted for this host), so a
    /// decision that compares them sees only the setter's change.
    pub(crate) current: Settings,
}

/// Apply `change` to the settings file and, the same way, to the controller's
/// copy: nothing else of the controller's copy moves, even if the file differs.
///
/// `change` runs twice, so it must not depend on the copy it is given beyond
/// the fields it assigns; the setters assign a value they were handed.
///
/// `Err` is the file's own error ([`CommandError::Settings`], with the code of
/// its `SettingsError`); the controller is left as it was.
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
    .map(|persisted| persisted.written)
}

/// [`persist_and_sync`] for a change that can be refused and whose effect on
/// the controller is a copy of what the file now holds: `mirror` copies the
/// fields the change wrote from the written settings into the controller's
/// copy, and only those. Which fields is the setter's rule; the table in
/// `persist_tests.rs` pins it.
///
/// The refusal is checked against the latest file while the settings lock is
/// held, so a validation and the write it guards cannot race another writer. A
/// refusal reaches the caller as its own error (a typed one stays typed) and
/// writes nothing.
pub(crate) fn try_persist_and_mirror<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
    mirror: impl FnOnce(&mut Settings, &Settings),
) -> Result<Persisted, CommandError> {
    persist(controller, change, |memory, written| {
        let mut next = memory.clone();
        mirror(&mut next, written);
        next
    })
}

/// Apply `change` to the file and make the controller's whole copy what was
/// written (demoted for this host, like every write here).
///
/// **Only for the hotkey change** (`persist_hotkey_change`): it is the one
/// caller that has registered the file's shortcuts with the OS, and a copy that
/// took them from anyone else would make the controller resolve key presses
/// against shortcuts nothing registered. Every other setter mirrors the fields
/// it writes; `only_the_hotkey_change_replaces_the_whole_copy` keeps the table
/// honest.
pub(crate) fn try_persist_and_replace<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
) -> Result<Persisted, CommandError> {
    persist(controller, change, |_memory, written| written.clone())
}

/// Whether this host can run Pianissimo: the gate the controller's copy is
/// demoted by, as `machine::load` demotes the startup copy, and the gate the
/// warm decision after a profile model change is made against.
pub(super) fn host_runs_pianissimo() -> bool {
    #[cfg(test)]
    if let Some(forced) = FORCED_PIANISSIMO_SUPPORT.with(std::cell::Cell::get) {
        return forced;
    }
    runtime_supported_on_this_os()
}

/// The shared core: one writer at a time, the file first, then the controller.
/// `follow` computes the controller's new copy from its current one and the
/// written settings; it runs under the controller lock and does no I/O. The
/// copy it returns is then demoted for this host: the file keeps the user's
/// choice of Pianissimo, the controller never holds it where it cannot run.
fn persist<E: Into<CommandError>>(
    controller: &SharedController,
    change: impl FnOnce(&mut Settings) -> Result<(), E>,
    follow: impl FnOnce(&Settings, &Settings) -> Settings,
) -> Result<Persisted, CommandError> {
    let _one_writer = lock_or_recover(&PERSIST, PERSIST_LOCK);
    // The store hands back the error type of `change` unchanged, and converts
    // its own failures (a settings file that cannot be written) into it.
    let written =
        machine::try_update::<CommandError, _>(|settings| change(settings).map_err(Into::into))?;
    // Asked before the controller lock is taken: the first answer may probe the
    // machine.
    let pianissimo_supported = host_runs_pianissimo();
    let mut ctrl = lock_controller(controller);
    let mut current = follow(ctrl.settings(), &written);
    // Quietly: the startup load warned about each demoted selection, and every
    // setter would say it again.
    let demoted = current.demote_unsupported_pianissimo_quietly(pianissimo_supported);
    let previous = ctrl.update_settings(current.clone());
    drop(ctrl);
    if demoted {
        tracing::debug!(
            "The written settings select Pianissimo, which this host cannot run; the controller keeps them demoted"
        );
    }
    Ok(Persisted {
        written,
        previous,
        current,
    })
}

#[cfg(test)]
thread_local! {
    static FORCED_PIANISSIMO_SUPPORT: std::cell::Cell<Option<bool>> =
        const { std::cell::Cell::new(None) };
}

/// Test seam: while the guard lives, this thread's setters treat the host as one
/// that cannot run Pianissimo, whatever machine the test runs on: the demotion of
/// the controller's copy and the profile model change's warm decision
/// (`persist_profile_model`) both ask [`host_runs_pianissimo`]. The tests that
/// use it run in a child process of their own (`isolated`) and call the setters
/// on the thread that holds the guard.
#[cfg(test)]
pub(super) struct HostWithoutPianissimo;

#[cfg(test)]
impl Drop for HostWithoutPianissimo {
    fn drop(&mut self) {
        FORCED_PIANISSIMO_SUPPORT.with(|forced| forced.set(None));
    }
}

#[cfg(test)]
pub(super) fn on_a_host_without_pianissimo() -> HostWithoutPianissimo {
    FORCED_PIANISSIMO_SUPPORT.with(|forced| forced.set(Some(false)));
    HostWithoutPianissimo
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
