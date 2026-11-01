//! Temporarily opt the ORT host process out of Windows execution-speed throttling while
//! model work is active. Process scope is required because ORT executes on worker threads.

#[cfg(any(windows, test))]
use std::sync::{Mutex, MutexGuard};

#[cfg(windows)]
const EXECUTION_SPEED: u32 =
    windows_sys::Win32::System::Threading::PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
#[cfg(all(not(windows), test))]
const EXECUTION_SPEED: u32 = 1;
#[cfg(any(windows, test))]
const RESTORE_RETRIES: usize = 1;
#[cfg(any(windows, test))]
const RESTORE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(2);

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ThrottlingState {
    version: u32,
    control_mask: u32,
    state_mask: u32,
}

#[cfg(any(windows, test))]
impl ThrottlingState {
    fn high_qos(self) -> Self {
        Self {
            control_mask: self.control_mask | EXECUTION_SPEED,
            state_mask: self.state_mask & !EXECUTION_SPEED,
            ..self
        }
    }
}

#[cfg(any(windows, test))]
trait PowerApi: Send + Sync {
    fn get(&self) -> Result<ThrottlingState, String>;
    fn set(&self, state: ThrottlingState) -> Result<(), String>;
    fn report_failure(&self, operation: &'static str, error: &str);
}

#[cfg(any(windows, test))]
#[derive(Default)]
struct ScopeState {
    active_scopes: usize,
    original: Option<ThrottlingState>,
}

#[cfg(any(windows, test))]
struct QosManager<A> {
    api: A,
    state: Mutex<ScopeState>,
}

#[cfg(any(windows, test))]
impl<A: PowerApi> QosManager<A> {
    fn new(api: A) -> Self {
        Self {
            api,
            state: Mutex::new(ScopeState::default()),
        }
    }

    fn acquire(&self) -> QosScope<'_, A> {
        let mut state = lock(&self.state);
        if state.active_scopes > 0 {
            state.active_scopes += 1;
            return QosScope {
                manager: self,
                active: true,
            };
        }

        // A prior final-scope restore can fail transiently. Retain its exact baseline and
        // retry it before starting another scope so a later inference does not bless a
        // leaked HighQoS state as its new baseline.
        if let Some(original) = state.original {
            if let Err(error) = self.restore_original(original) {
                self.api.report_failure("restore previous state", &error);
                return QosScope {
                    manager: self,
                    active: false,
                };
            }
            state.original = None;
        }

        let original = match self.api.get() {
            Ok(original) => original,
            Err(error) => {
                self.api.report_failure("read current state", &error);
                return QosScope {
                    manager: self,
                    active: false,
                };
            }
        };
        if let Err(error) = self.api.set(original.high_qos()) {
            self.api.report_failure("enable HighQoS", &error);
            return QosScope {
                manager: self,
                active: false,
            };
        }

        state.original = Some(original);
        state.active_scopes = 1;
        QosScope {
            manager: self,
            active: true,
        }
    }

    fn release(&self) {
        let mut state = lock(&self.state);
        if state.active_scopes == 0 {
            self.api.report_failure(
                "restore previous state",
                "internal scope count was already zero",
            );
            return;
        }

        state.active_scopes -= 1;
        if state.active_scopes != 0 {
            return;
        }

        let Some(original) = state.original else {
            self.api.report_failure(
                "restore previous state",
                "original state was missing at final scope exit",
            );
            return;
        };
        match self.restore_original(original) {
            Ok(()) => state.original = None,
            Err(error) => self.api.report_failure("restore previous state", &error),
        }
    }

    fn restore_original(&self, original: ThrottlingState) -> Result<(), String> {
        let mut prior_error = None;
        for attempt in 0..=RESTORE_RETRIES {
            match self.api.set(original) {
                Ok(()) => {
                    if let Some(error) = prior_error {
                        self.api.report_failure(
                            "restore previous state",
                            &format!("first attempt failed but retry succeeded: {error}"),
                        );
                    }
                    return Ok(());
                }
                Err(error) if attempt < RESTORE_RETRIES => {
                    prior_error = Some(error);
                    std::thread::sleep(RESTORE_RETRY_DELAY);
                }
                Err(error) => {
                    return Err(format!(
                        "first attempt failed: {}; retry failed: {error}",
                        prior_error.as_deref().unwrap_or("not recorded")
                    ));
                }
            }
        }

        Err("restore attempt budget exhausted unexpectedly".into())
    }
}

#[cfg(any(windows, test))]
struct QosScope<'a, A: PowerApi> {
    manager: &'a QosManager<A>,
    active: bool,
}

#[cfg(any(windows, test))]
impl<A: PowerApi> Drop for QosScope<'_, A> {
    fn drop(&mut self) {
        if self.active {
            self.manager.release();
        }
    }
}

#[cfg(any(windows, test))]
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    use std::sync::atomic::{AtomicBool, Ordering};
    static POISON_WARNING: AtomicBool = AtomicBool::new(false);

    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            // The QoS manager changes its count and saved baseline only while holding this
            // mutex. Recover that state, clear poison so later scopes can proceed normally,
            // and report the unwind once instead of silently masking it.
            let guard = poisoned.into_inner();
            mutex.clear_poison();
            if POISON_WARNING
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                eprintln!(
                    "warning: recovered poisoned Windows process HighQoS scope state; continuing with the saved state"
                );
            }
            guard
        }
    }
}

#[cfg(windows)]
mod windows {
    use super::{PowerApi, QosManager, QosScope, ThrottlingState};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessInformation, ProcessPowerThrottling, SetProcessInformation,
        PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_STATE,
    };

    pub struct WindowsApi;

    impl PowerApi for WindowsApi {
        fn get(&self) -> Result<ThrottlingState, String> {
            let mut native = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: 0,
                StateMask: 0,
            };
            // SAFETY: the current-process pseudo-handle is valid, and `native` is the
            // correctly sized writable structure required by ProcessPowerThrottling.
            let success = unsafe {
                GetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&mut native as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
                    std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )
            };
            if success == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(ThrottlingState {
                version: native.Version,
                control_mask: native.ControlMask,
                state_mask: native.StateMask,
            })
        }

        fn set(&self, state: ThrottlingState) -> Result<(), String> {
            let native = PROCESS_POWER_THROTTLING_STATE {
                Version: state.version,
                ControlMask: state.control_mask,
                StateMask: state.state_mask,
            };
            // SAFETY: the current-process pseudo-handle is valid, and `native` is the
            // correctly sized readable structure required by ProcessPowerThrottling.
            let success = unsafe {
                SetProcessInformation(
                    GetCurrentProcess(),
                    ProcessPowerThrottling,
                    (&native as *const PROCESS_POWER_THROTTLING_STATE).cast(),
                    std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
                )
            };
            if success == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        }

        fn report_failure(&self, operation: &'static str, error: &str) {
            static READ_WARNING: AtomicBool = AtomicBool::new(false);
            static ENABLE_WARNING: AtomicBool = AtomicBool::new(false);
            static RESTORE_WARNING: AtomicBool = AtomicBool::new(false);
            let reported = match operation {
                "read current state" => &READ_WARNING,
                "enable HighQoS" => &ENABLE_WARNING,
                _ => &RESTORE_WARNING,
            };
            if reported
                .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                eprintln!(
                    "warning: Windows process HighQoS {operation} encountered an API failure; inference will continue: {error}"
                );
            }
        }
    }

    fn manager() -> &'static QosManager<WindowsApi> {
        static MANAGER: OnceLock<QosManager<WindowsApi>> = OnceLock::new();
        MANAGER.get_or_init(|| QosManager::new(WindowsApi))
    }

    pub struct HighQosScope {
        _scope: QosScope<'static, WindowsApi>,
    }

    pub fn enter() -> HighQosScope {
        HighQosScope {
            _scope: manager().acquire(),
        }
    }

    #[cfg(test)]
    pub fn process_api() -> WindowsApi {
        WindowsApi
    }
}

#[cfg(windows)]
pub(super) use windows::enter as high_qos_scope;

#[cfg(not(windows))]
pub(super) struct HighQosScope;

#[cfg(not(windows))]
pub(super) fn high_qos_scope() -> HighQosScope {
    HighQosScope
}

#[cfg(test)]
mod tests {
    use super::{lock, PowerApi, QosManager, ThrottlingState, EXECUTION_SPEED};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MockState {
        current: Option<ThrottlingState>,
        fail_get: bool,
        fail_set_calls: Vec<usize>,
        set_calls: usize,
        warnings: Vec<(&'static str, String)>,
    }

    #[derive(Clone, Default)]
    struct MockApi(Arc<Mutex<MockState>>);

    impl MockApi {
        fn with_state(state: ThrottlingState) -> Self {
            Self(Arc::new(Mutex::new(MockState {
                current: Some(state),
                ..MockState::default()
            })))
        }

        fn inspect(&self) -> (Option<ThrottlingState>, usize, Vec<(&'static str, String)>) {
            let state = lock(&self.0);
            (state.current, state.set_calls, state.warnings.clone())
        }

        fn fail_get(&self) {
            lock(&self.0).fail_get = true;
        }

        fn fail_set_calls(&self, calls: Vec<usize>) {
            lock(&self.0).fail_set_calls = calls;
        }
    }

    impl PowerApi for MockApi {
        fn get(&self) -> Result<ThrottlingState, String> {
            let state = lock(&self.0);
            if state.fail_get {
                Err("mock get failure".into())
            } else {
                state.current.ok_or_else(|| "mock has no state".into())
            }
        }

        fn set(&self, new_state: ThrottlingState) -> Result<(), String> {
            let mut state = lock(&self.0);
            state.set_calls += 1;
            if state.fail_set_calls.contains(&state.set_calls) {
                return Err(format!("mock set failure {}", state.set_calls));
            }
            state.current = Some(new_state);
            Ok(())
        }

        fn report_failure(&self, operation: &'static str, error: &str) {
            lock(&self.0).warnings.push((operation, error.to_string()));
        }
    }

    fn original_state() -> ThrottlingState {
        ThrottlingState {
            version: 9,
            control_mask: 0b1010,
            state_mask: 0b1111,
        }
    }

    #[test]
    fn high_qos_transitions_only_execution_speed_and_restores_exact_state() {
        let original = original_state();
        let expected = ThrottlingState {
            version: original.version,
            control_mask: original.control_mask | EXECUTION_SPEED,
            state_mask: original.state_mask & !EXECUTION_SPEED,
        };
        let api = MockApi::with_state(original);
        let manager = QosManager::new(api.clone());

        let scope = manager.acquire();
        assert_eq!(api.inspect().0, Some(expected));
        drop(scope);

        let (current, set_calls, warnings) = api.inspect();
        assert_eq!(current, Some(original));
        assert_eq!(set_calls, 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn overlapping_scopes_restore_only_after_the_final_scope_exits() {
        let original = original_state();
        let api = MockApi::with_state(original);
        let manager = QosManager::new(api.clone());

        let outer = manager.acquire();
        let inner = manager.acquire();
        assert_eq!(api.inspect().1, 1, "only the first scope enables HighQoS");

        drop(outer);
        assert_eq!(api.inspect().0, Some(original.high_qos()));
        assert_eq!(api.inspect().1, 1, "an overlapping scope is still active");

        drop(inner);
        let (current, set_calls, warnings) = api.inspect();
        assert_eq!(current, Some(original));
        assert_eq!(set_calls, 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn concurrent_scopes_share_one_process_transition() {
        let original = original_state();
        let api = MockApi::with_state(original);
        let manager = std::sync::Arc::new(QosManager::new(api.clone()));
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_manager = std::sync::Arc::clone(&manager);
        let worker = std::thread::spawn(move || {
            let scope = worker_manager.acquire();
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            drop(scope);
        });

        entered_rx.recv().unwrap();
        let second = manager.acquire();
        assert_eq!(api.inspect().0, Some(original.high_qos()));
        assert_eq!(api.inspect().1, 1);

        drop(second);
        assert_eq!(api.inspect().0, Some(original.high_qos()));
        release_tx.send(()).unwrap();
        worker.join().unwrap();

        let (current, set_calls, warnings) = api.inspect();
        assert_eq!(current, Some(original));
        assert_eq!(set_calls, 2);
        assert!(warnings.is_empty());
    }

    #[test]
    fn read_and_enable_failures_are_reported_and_do_not_claim_an_active_scope() {
        let original = original_state();
        let get_api = MockApi::with_state(original);
        get_api.fail_get();
        let get_manager = QosManager::new(get_api.clone());
        let scope = get_manager.acquire();
        drop(scope);
        let (current, set_calls, warnings) = get_api.inspect();
        assert_eq!(current, Some(original));
        assert_eq!(set_calls, 0);
        assert_eq!(warnings[0].0, "read current state");

        let set_api = MockApi::with_state(original);
        set_api.fail_set_calls(vec![1]);
        let set_manager = QosManager::new(set_api.clone());
        let scope = set_manager.acquire();
        drop(scope);
        let (current, set_calls, warnings) = set_api.inspect();
        assert_eq!(current, Some(original));
        assert_eq!(
            set_calls, 1,
            "a failed enable must not trigger a fake restore"
        );
        assert_eq!(warnings[0].0, "enable HighQoS");
    }

    #[test]
    fn transient_restore_failure_is_retried_before_scope_exit_returns() {
        let original = original_state();
        let api = MockApi::with_state(original);
        api.fail_set_calls(vec![2]);
        let manager = QosManager::new(api.clone());

        drop(manager.acquire());
        let (current, set_calls, warnings) = api.inspect();
        assert_eq!(current, Some(original));
        assert_eq!(set_calls, 3, "one bounded retry restores during drop");
        assert_eq!(warnings[0].0, "restore previous state");
        assert!(warnings[0].1.contains("retry succeeded"));

        let retry = manager.acquire();
        let (current, set_calls, _) = api.inspect();
        assert_eq!(current, Some(original.high_qos()));
        assert_eq!(set_calls, 4);
        drop(retry);
        let (current, set_calls, _) = api.inspect();
        assert_eq!(current, Some(original));
        assert_eq!(set_calls, 5);
    }

    #[test]
    fn persistent_restore_failure_keeps_baseline_for_later_recovery() {
        let original = original_state();
        let api = MockApi::with_state(original);
        api.fail_set_calls(vec![2, 3]);
        let manager = QosManager::new(api.clone());

        drop(manager.acquire());
        let (current, set_calls, warnings) = api.inspect();
        assert_eq!(current, Some(original.high_qos()));
        assert_eq!(set_calls, 3, "the final scope uses at most one retry");
        assert!(warnings[0].1.contains("retry failed"));

        let retry = manager.acquire();
        let (current, set_calls, _) = api.inspect();
        assert_eq!(current, Some(original.high_qos()));
        assert_eq!(set_calls, 5, "a later scope restores then enables");
        drop(retry);
        assert_eq!(api.inspect().0, Some(original));
    }

    #[test]
    fn poisoned_scope_state_is_cleared_and_active_scope_still_restores() {
        let original = original_state();
        let api = MockApi::with_state(original);
        let manager = QosManager::new(api.clone());
        let scope = manager.acquire();
        assert_eq!(api.inspect().0, Some(original.high_qos()));

        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _state = manager.state.lock().expect("state starts unpoisoned");
            panic!("simulate unwind while a QoS scope is active");
        }));
        assert!(unwind.is_err());
        assert!(manager.state.is_poisoned());

        drop(scope);
        assert!(!manager.state.is_poisoned());
        assert_eq!(api.inspect().0, Some(original));
    }

    #[cfg(windows)]
    #[test]
    fn windows_process_scope_restores_the_test_process_state() {
        use super::windows::process_api;

        let api = process_api();
        let original = api.get().expect("read test process power throttling state");
        {
            let _scope = super::high_qos_scope();
            let active = api.get().expect("read HighQoS test process state");
            assert_eq!(active.version, original.version);
            assert_eq!(active.control_mask, original.control_mask | EXECUTION_SPEED);
            assert_eq!(active.state_mask, original.state_mask & !EXECUTION_SPEED);
        }
        let restored = api.get().expect("read restored test process state");
        assert_eq!(restored, original);
    }
}
