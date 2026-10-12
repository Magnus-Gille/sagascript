//! Poison-tolerant locking (docs/adr/0005-mutex-poison-policy.md, #359).
//!
//! A `std::sync::Mutex` is poisoned when a thread panics while holding its
//! guard. Propagating that with `lock().unwrap()` turns one panic into a panic
//! in every later caller (and an abort when the caller is a `Drop` running
//! during unwinding). Sagascript recovers instead: the data is taken from the
//! poisoned lock, the poison flag is cleared so each incident is reported once,
//! and an `error!` names the lock and the panic that poisoned it.
//!
//! Recovering is only correct when the protected data is still consistent or
//! the caller re-validates it. Modules whose invariants a panic could break
//! restore a safe state first via [`lock_or_recover_with`] or
//! [`recover_poisoned`].

use std::any::Any;
use std::panic::PanicHookInfo;
use std::sync::{Mutex, MutexGuard, Once, PoisonError};

/// Most recent panic seen by the hook from [`install_panic_recorder`].
static LAST_PANIC: Mutex<Option<String>> = Mutex::new(None);

/// Bounds the recorded panic text so one huge message cannot bloat every
/// recovery log line.
const MAX_PANIC_TEXT_CHARS: usize = 512;

/// Lock `mutex`, recovering its data if an earlier panic poisoned it.
///
/// `name` identifies the lock in the recovery log (for example
/// `"app_controller"`). Use this only where the protected data stays
/// consistent even if a panic interrupted an update; otherwise use
/// [`lock_or_recover_with`].
pub fn lock_or_recover<'a, T: ?Sized>(
    mutex: &'a Mutex<T>,
    name: &'static str,
) -> MutexGuard<'a, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| recover_poisoned(mutex, name, poisoned))
}

/// Like [`lock_or_recover`], but runs `restore` on the data before returning
/// it when (and only when) the lock was poisoned, so the caller never observes
/// state a panic may have left half-updated.
pub fn lock_or_recover_with<'a, T: ?Sized>(
    mutex: &'a Mutex<T>,
    name: &'static str,
    restore: impl FnOnce(&mut T),
) -> MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        let mut guard = recover_poisoned(mutex, name, poisoned);
        restore(&mut guard);
        guard
    })
}

/// Take the guard (or a guard-carrying value such as a `Condvar` wait result)
/// out of `poisoned`, clear `mutex`'s poison flag and log the recovery.
///
/// The building block for locks acquired through `try_lock` or `Condvar`
/// waits; prefer [`lock_or_recover`] for plain `lock()` calls.
pub fn recover_poisoned<T: ?Sized, G>(
    mutex: &Mutex<T>,
    name: &'static str,
    poisoned: PoisonError<G>,
) -> G {
    mutex.clear_poison();
    let cause = last_panic();
    tracing::error!(
        lock = name,
        panic = cause
            .as_deref()
            .unwrap_or("unknown (no panic recorder installed)"),
        "lock was poisoned by an earlier panic; recovered its data and continuing"
    );
    poisoned.into_inner()
}

/// Install a panic hook (once per process) that remembers the latest panic
/// message for [`recover_poisoned`]'s log line, then runs the previously
/// installed hook, so the default stderr report is unchanged.
pub fn install_panic_recorder() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record_panic(info);
            previous(info);
        }));
    });
}

/// The latest panic seen by the recorder hook, if it is installed and a panic
/// has happened.
pub fn last_panic() -> Option<String> {
    LAST_PANIC
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The message of a panic payload (from `catch_unwind` or a panic hook).
pub fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

fn record_panic(info: &PanicHookInfo<'_>) {
    let message = panic_message(info.payload());
    let text = match info.location() {
        Some(location) => format!("panicked at {location}: {message}"),
        None => format!("panicked: {message}"),
    };
    let text = if text.chars().count() > MAX_PANIC_TEXT_CHARS {
        let mut bounded: String = text.chars().take(MAX_PANIC_TEXT_CHARS).collect();
        bounded.push('…');
        bounded
    } else {
        text
    };
    // Never panic (or log) inside the hook: a panic here would abort.
    *LAST_PANIC.lock().unwrap_or_else(PoisonError::into_inner) = Some(text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn poison<T: Send>(mutex: &Mutex<T>) {
        std::thread::scope(|scope| {
            let panicked = scope
                .spawn(|| {
                    let _guard = mutex.lock().unwrap();
                    panic!("injected panic while holding the lock (#359)");
                })
                .join();
            assert!(panicked.is_err());
        });
        assert!(mutex.is_poisoned());
    }

    #[test]
    fn lock_or_recover_returns_the_data_of_a_poisoned_lock_and_clears_the_flag() {
        let counter = Mutex::new(41);
        poison(&counter);

        *lock_or_recover(&counter, "test.counter") += 1;

        assert!(!counter.is_poisoned(), "an incident is reported once");
        assert_eq!(*lock_or_recover(&counter, "test.counter"), 42);
    }

    #[test]
    fn restore_runs_only_when_the_lock_was_poisoned() {
        let state = Mutex::new(vec![1, 2, 3]);
        assert_eq!(
            *lock_or_recover_with(&state, "test.state", Vec::clear),
            [1, 2, 3]
        );

        poison(&state);
        assert!(lock_or_recover_with(&state, "test.state", Vec::clear).is_empty());
        assert!(!state.is_poisoned());
    }

    #[test]
    fn panic_message_reads_str_and_string_payloads() {
        let literal = std::panic::catch_unwind(|| panic!("literal")).unwrap_err();
        assert_eq!(panic_message(&*literal), "literal");
        let formatted = std::panic::catch_unwind(|| panic!("formatted {}", 7)).unwrap_err();
        assert_eq!(panic_message(&*formatted), "formatted 7");
        let other = std::panic::catch_unwind(|| std::panic::panic_any(7_u8)).unwrap_err();
        assert_eq!(panic_message(&*other), "non-string panic payload");
    }

    #[test]
    fn recover_poisoned_unwraps_try_lock_and_condvar_results() {
        let flag = Mutex::new(false);
        poison(&flag);
        let mut guard = match flag.try_lock() {
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                recover_poisoned(&flag, "test.flag", poisoned)
            }
            _ => panic!("expected a poisoned try_lock"),
        };
        *guard = true;
        drop(guard);

        let cv = std::sync::Condvar::new();
        poison(&flag);
        let guard = flag.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = cv
            .wait_timeout(guard, std::time::Duration::from_millis(1))
            .unwrap_or_else(|poisoned| recover_poisoned(&flag, "test.flag", poisoned));
        assert!(*guard);
        drop(guard);
        assert!(!flag.is_poisoned());
    }
}
