//! ADR 0005 (#359): a lock poisoned by a panic is recovered, and the recovery
//! can name the panic that caused it. A separate test binary, because the
//! panic recorder is a process-wide hook.

use sagascript_core::sync::{install_panic_recorder, last_panic, lock_or_recover};
use std::sync::Mutex;

#[test]
fn recovery_after_a_recorded_panic_succeeds_and_knows_the_cause() {
    install_panic_recorder();
    install_panic_recorder(); // idempotent

    let jobs = Mutex::new(vec!["first"]);
    std::thread::scope(|scope| {
        let panicked = scope
            .spawn(|| {
                let mut guard = jobs.lock().unwrap();
                guard.push("half-done");
                panic!("worker failed while holding the jobs lock (#359)");
            })
            .join();
        assert!(panicked.is_err());
    });
    assert!(jobs.is_poisoned());

    let recorded = last_panic().expect("the recorder saw the panic");
    assert!(
        recorded.contains("worker failed while holding the jobs lock (#359)"),
        "{recorded}"
    );
    assert!(recorded.contains("poison_recovery.rs"), "{recorded}");

    let mut guard = lock_or_recover(&jobs, "test.jobs");
    assert_eq!(*guard, ["first", "half-done"]);
    guard.push("next");
    drop(guard);
    assert!(!jobs.is_poisoned());
}
