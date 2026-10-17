use super::*;
use sagascript_core::settings::Settings;

#[cfg(test)]
mod controller_poison_tests {
    use super::*;

    /// Poison `mutex` the way production would: another thread panics while
    /// holding the guard (#359).
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

    fn controller() -> SharedController {
        Mutex::new(AppController::new(Settings::default()))
    }

    /// A panic under the controller lock must not turn every later command
    /// into a panic: the next real command path gets the controller.
    #[test]
    fn commands_keep_working_after_a_panic_poisoned_the_controller() {
        let controller = controller();
        poison(&controller);

        let lease = acquire_hotkey_configuration(&controller)
            .expect("a poisoned controller must be recovered, not propagated");
        let refused = acquire_hotkey_configuration(&controller)
            .err()
            .expect("the recovered controller still enforces the single-change lease");
        assert_eq!(refused.code(), "busy");
        assert!(refused.retryable());
        drop(lease);
        assert!(acquire_hotkey_configuration(&controller).is_ok());
        assert!(!controller.is_poisoned(), "recovery clears the poison flag");
    }

    /// The lease's Drop runs during unwinding when a settings change panics.
    /// Re-panicking on the poisoned controller there would abort the process.
    #[test]
    fn hotkey_configuration_lease_drop_tolerates_a_poisoned_controller() {
        let controller = controller();
        let lease = acquire_hotkey_configuration(&controller).unwrap();
        poison(&controller);

        let dropped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(lease)));
        assert!(
            dropped.is_ok(),
            "Drop must not panic on a poisoned controller"
        );
        assert!(
            acquire_hotkey_configuration(&controller).is_ok(),
            "the lease was released despite the poison"
        );
    }
}
