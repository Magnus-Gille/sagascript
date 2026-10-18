use super::*;

/// Generous for a wait that is released by an event, never by the clock.
const PATIENT: Duration = Duration::from_secs(30);

fn is_held<T>(mutex: &Mutex<T>) -> bool {
    matches!(mutex.try_lock(), Err(TryLockError::WouldBlock))
}

/// Poll until `mutex` is held by someone else. The poll can take the lock for
/// an instant; the other thread then simply waits that long.
fn wait_until_held<T>(mutex: &Mutex<T>) {
    let failsafe = Instant::now() + PATIENT;
    while !is_held(mutex) {
        assert!(Instant::now() < failsafe, "the lock was never taken");
        assert!(!mutex.is_poisoned(), "lock is poisoned");
        std::thread::yield_now();
    }
}

/// Characterisation of the warm-state protocol with a fake state (#371): abort
/// during inference, the timeout path, a sticky cancel, a busy caller and a
/// panic, all through `with_state_slot`, the code `with_warm_state` runs on
/// the native `WhisperState`. The fake inference polls the same trampoline
/// whisper.cpp polls before every compute step.
#[cfg(test)]
mod warm_state_protocol_tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::thread;

    #[derive(Debug, PartialEq)]
    struct FakeState(u32);

    /// A backend, a state slot and a counter of how many fake states were built.
    struct Rig {
        backend: WhisperBackend,
        slot: Mutex<Option<FakeState>>,
        created: AtomicU32,
    }

    impl Rig {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                backend: WhisperBackend::new(),
                slot: Mutex::new(None),
                created: AtomicU32::new(0),
            })
        }

        fn create(&self) -> Result<FakeState, DictationError> {
            Ok(FakeState(self.created.fetch_add(1, Ordering::SeqCst) + 1))
        }

        fn created(&self) -> u32 {
            self.created.load(Ordering::SeqCst)
        }

        fn run<R>(
            &self,
            grace: Duration,
            f: impl FnOnce(&mut FakeState) -> Result<R, DictationError>,
        ) -> Result<R, DictationError> {
            self.backend
                .with_state_slot(&self.slot, grace, || self.create(), f)
        }

        fn state(&self) -> Option<u32> {
            self.slot.lock().unwrap().as_ref().map(|state| state.0)
        }

        fn abort_pending(&self) -> bool {
            self.backend.abort_flag.load(Ordering::SeqCst)
        }

        /// An inference that, like `full()`, runs until the abort callback says
        /// stop. Signals `started` once it is running.
        fn inference_until_aborted(
            &self,
            started: &mpsc::Sender<()>,
        ) -> Result<(), DictationError> {
            let flag = Arc::as_ptr(&self.backend.abort_flag) as *mut c_void;
            started.send(()).expect("signal inference started");
            let failsafe = Instant::now() + PATIENT;
            loop {
                // SAFETY: `flag` points at the backend's own flag, which outlives this call.
                if unsafe { WhisperBackend::abort_trampoline(flag) } {
                    return Err(DictationError::TranscriptionFailed(
                        "Whisper inference failed: aborted".to_string(),
                    ));
                }
                assert!(Instant::now() < failsafe, "the abort never arrived");
                thread::yield_now();
            }
        }

        /// Run `inference_until_aborted` on a background thread and return once
        /// it is running and holds the state lock.
        fn start_inference(self: &Arc<Self>) -> thread::JoinHandle<Result<(), DictationError>> {
            let rig = Arc::clone(self);
            let (started_tx, started_rx) = mpsc::channel();
            let handle = thread::spawn(move || {
                rig.run(PATIENT, |_state| rig.inference_until_aborted(&started_tx))
            });
            started_rx.recv().expect("inference should start");
            handle
        }
    }

    fn failed_inference(result: &Result<(), DictationError>) -> bool {
        matches!(result, Err(DictationError::TranscriptionFailed(message))
            if message.contains("Whisper inference failed"))
    }

    #[test]
    fn state_is_built_on_first_use_and_reused_by_later_calls() {
        let rig = Rig::new();

        let first = rig.run(PATIENT, |state| Ok(state.0)).unwrap();
        let second = rig.run(PATIENT, |state| Ok(state.0)).unwrap();

        assert_eq!((first, second), (1, 1));
        assert_eq!(rig.created(), 1);
        assert_eq!(rig.state(), Some(1));
    }

    #[test]
    fn a_failed_state_build_skips_the_closure_and_leaves_the_slot_empty() {
        let rig = Rig::new();
        let mut ran = false;

        let result = rig.backend.with_state_slot(
            &rig.slot,
            PATIENT,
            || Err::<FakeState, _>(DictationError::ModelNotLoaded),
            |_| {
                ran = true;
                Ok(())
            },
        );

        assert!(matches!(result, Err(DictationError::ModelNotLoaded)));
        assert!(!ran);
        assert_eq!(rig.state(), None);
    }

    #[test]
    fn an_abort_during_inference_ends_it_and_discards_the_warm_state() {
        let rig = Rig::new();
        let inference = rig.start_inference();

        rig.backend.request_abort();
        let result = inference.join().unwrap();

        assert!(failed_inference(&result), "got {result:?}");
        assert_eq!(
            rig.state(),
            None,
            "an aborted compute leaves state unusable"
        );
        assert!(!rig.abort_pending(), "the lock holder clears its own abort");
        // The next transcription starts from a clean slate.
        assert_eq!(rig.run(PATIENT, |state| Ok(state.0)).unwrap(), 2);
        assert_eq!(rig.created(), 2);
    }

    #[test]
    fn a_stale_abort_does_not_stop_the_next_inference_or_cost_it_its_state() {
        let rig = Rig::new();
        rig.run(PATIENT, |_| Ok(())).unwrap();
        rig.backend.request_abort();
        let flag = Arc::as_ptr(&rig.backend.abort_flag) as *mut c_void;

        let aborted_inside = rig
            .run(PATIENT, |_| {
                // SAFETY: `flag` points at the backend's own flag.
                Ok(unsafe { WhisperBackend::abort_trampoline(flag) })
            })
            .unwrap();

        assert!(
            !aborted_inside,
            "the previous call's abort is stale once the lock is owned"
        );
        assert_eq!(rig.state(), Some(1), "so the warm state is kept");
        assert_eq!(rig.created(), 1);
    }

    /// The timeout path (WP2b): a caller that gives up with `ModelBusy` must not
    /// disturb the stuck inference; the timeout handler's abort then frees the
    /// lock, and the next call runs on a rebuilt state with a clear flag.
    #[test]
    fn a_timeout_abort_unwedges_the_pipeline() {
        let rig = Rig::new();
        let stuck = rig.start_inference();

        let mut ran = false;
        let busy = rig.run(Duration::from_millis(30), |_| {
            ran = true;
            Ok(())
        });
        assert!(
            matches!(busy, Err(DictationError::ModelBusy)),
            "got {busy:?}"
        );
        assert!(!ran);
        assert!(!rig.abort_pending(), "a busy caller touches nothing");
        assert_eq!(rig.created(), 1, "nor does it build a second state");

        // What the timeout handler does.
        rig.backend.request_abort();
        let rebuilt = rig
            .run(PATIENT, |state| Ok((state.0, rig.abort_pending())))
            .unwrap();

        assert_eq!(rebuilt, (2, false), "fresh state, clear flag");
        assert!(failed_inference(&stuck.join().unwrap()));
    }

    #[test]
    fn a_cancel_during_inference_aborts_it_and_every_later_call_is_refused() {
        let rig = Rig::new();
        let inference = rig.start_inference();

        rig.backend.request_cancel();
        let result = inference.join().unwrap();

        assert!(failed_inference(&result), "got {result:?}");
        assert_eq!(rig.state(), None);
        let mut ran = false;
        let later = rig.run(PATIENT, |_| {
            ran = true;
            Ok(())
        });
        assert!(
            matches!(later, Err(DictationError::Cancelled)),
            "got {later:?}"
        );
        assert!(!ran);
        assert_eq!(rig.created(), 1, "nothing is rebuilt for a cancelled run");
        assert!(rig.backend.is_cancel_requested());
    }

    #[test]
    fn a_sticky_cancel_refuses_new_inference_but_keeps_the_warm_state() {
        let rig = Rig::new();
        rig.run(PATIENT, |_| Ok(())).unwrap();
        rig.backend.request_cancel();

        let later = rig.run(PATIENT, |_| Ok(()));

        assert!(
            matches!(later, Err(DictationError::Cancelled)),
            "got {later:?}"
        );
        assert_eq!(
            rig.state(),
            Some(1),
            "a refused call does not discard the state"
        );
        assert!(
            !rig.abort_pending(),
            "the lock owner still cleared the flag"
        );
        assert!(rig.backend.is_cancel_requested());
    }

    #[test]
    fn a_panic_during_inference_poisons_the_slot_and_the_next_call_gets_a_fresh_state() {
        let rig = Rig::new();
        let panicking = Arc::clone(&rig);
        let outcome = thread::spawn(move || {
            panicking.run(PATIENT, |_| -> Result<(), DictationError> {
                panic!("injected panic mid-inference (#359)")
            })
        })
        .join();
        assert!(outcome.is_err());
        assert!(rig.slot.is_poisoned());
        assert_eq!(rig.created(), 1);

        let next = rig.run(PATIENT, |state| Ok(state.0)).unwrap();

        assert_eq!(next, 2, "the interrupted state is not reused");
        assert!(!rig.slot.is_poisoned());
    }

    /// ARCHITECTURE.md lock order: state, then the runtime (identity and
    /// context are one lock). Creating the state takes the runtime lock while
    /// the state lock is held.
    #[test]
    fn the_backend_takes_the_context_while_holding_the_state_lock() {
        let backend = Arc::new(WhisperBackend::new());
        let context = backend.loaded_runtime.lock().unwrap();
        let worker = {
            let backend = Arc::clone(&backend);
            thread::spawn(move || backend.with_warm_state_grace(PATIENT, |_| Ok(())))
        };

        wait_until_held(&backend.state);
        drop(context);

        let result = worker.join().unwrap();
        assert!(
            matches!(result, Err(DictationError::ModelNotLoaded)),
            "got {result:?}"
        );
        assert!(backend.state.try_lock().is_ok(), "released afterwards");
    }
}

/// Characterisation of the runtime swap with fake contexts and states (#371):
/// what a model switch keeps, releases and refuses while a transcription runs,
/// and the order in which it takes the four locks.
#[cfg(test)]
mod runtime_swap_tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::mpsc;
    use std::thread;

    const SHORT: Duration = Duration::from_millis(30);

    /// Counts how many fake contexts and states have been released.
    struct Probe {
        name: &'static str,
        released: Arc<AtomicU32>,
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn key(model: WhisperModel) -> RuntimeKey {
        RuntimeKey {
            model,
            profile: ContextProfile::FlashAttention,
        }
    }

    fn swedish() -> RuntimeKey {
        key(WhisperModel::KbWhisperBase)
    }

    fn english() -> RuntimeKey {
        key(WhisperModel::BaseEn)
    }

    struct Fakes {
        state: Mutex<Option<Probe>>,
        loaded: Mutex<Option<Loaded<Probe>>>,
        cached: Mutex<Option<CachedWhisperRuntime<Probe, Probe>>>,
        released: Arc<AtomicU32>,
    }

    /// What the four slots hold, by fake name.
    #[derive(Debug, PartialEq)]
    struct Snapshot {
        active: Option<RuntimeKey>,
        context: Option<&'static str>,
        state: Option<&'static str>,
        cached: Option<(RuntimeKey, &'static str, Option<&'static str>)>,
    }

    impl Fakes {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                state: Mutex::new(None),
                loaded: Mutex::new(None),
                cached: Mutex::new(None),
                released: Arc::new(AtomicU32::new(0)),
            })
        }

        fn probe(&self, name: &'static str) -> Probe {
            Probe {
                name,
                released: Arc::clone(&self.released),
            }
        }

        fn slots(&self) -> RuntimeSlots<'_, Probe, Probe> {
            RuntimeSlots {
                state: &self.state,
                loaded_runtime: &self.loaded,
                cached_runtime: &self.cached,
            }
        }

        fn publish(&self, key: RuntimeKey, context: &'static str) -> Result<(), DictationError> {
            publish_runtime(&self.slots(), self.probe(context), key, SHORT)
        }

        fn warm(&self, state: &'static str) {
            *self.state.lock().unwrap() = Some(self.probe(state));
        }

        fn released(&self) -> u32 {
            self.released.load(Ordering::SeqCst)
        }

        fn snapshot(&self) -> Snapshot {
            let loaded = self.loaded.lock().unwrap();
            Snapshot {
                active: loaded.as_ref().map(|runtime| runtime.key),
                context: loaded.as_ref().map(|runtime| runtime.ctx.name),
                state: self.state.lock().unwrap().as_ref().map(|p| p.name),
                cached: self.cached.lock().unwrap().as_ref().map(|runtime| {
                    (
                        runtime.runtime.key,
                        runtime.runtime.ctx.name,
                        runtime.state.as_ref().map(|p| p.name),
                    )
                }),
            }
        }
    }

    /// An inference in flight: holds the state lock until `release` fires.
    fn hold_state(fakes: &Arc<Fakes>) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
        let holder = Arc::clone(fakes);
        let (held_tx, held_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            let _inference = holder.state.lock().unwrap();
            held_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        held_rx.recv().unwrap();
        (release_tx, handle)
    }

    #[test]
    fn the_first_runtime_becomes_active_without_a_warm_state_or_a_secondary() {
        let fakes = Fakes::new();

        fakes.publish(swedish(), "swedish-ctx").unwrap();

        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(swedish()),
                context: Some("swedish-ctx"),
                state: None,
                cached: None,
            }
        );
        assert_eq!(fakes.released(), 0);
    }

    #[test]
    fn a_second_runtime_inside_the_budget_moves_the_first_and_its_warm_state_to_the_cache() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.warm("swedish-state");

        fakes.publish(english(), "english-ctx").unwrap();

        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(english()),
                context: Some("english-ctx"),
                state: None,
                cached: Some((swedish(), "swedish-ctx", Some("swedish-state"))),
            }
        );
        assert_eq!(
            fakes.released(),
            0,
            "nothing is released while it can be reused"
        );
    }

    #[test]
    fn a_leftover_warm_state_never_survives_into_a_newly_published_runtime() {
        let fakes = Fakes::new();
        fakes.warm("orphan-state");

        fakes.publish(swedish(), "swedish-ctx").unwrap();

        assert_eq!(fakes.snapshot().state, None);
        assert_eq!(fakes.released(), 1, "the orphan is released");
    }

    #[test]
    fn a_runtime_outside_the_budget_is_released_instead_of_cached() {
        let fakes = Fakes::new();
        fakes
            .publish(key(WhisperModel::KbWhisperMedium), "medium-ctx")
            .unwrap();
        fakes.warm("medium-state");

        fakes.publish(english(), "english-ctx").unwrap();

        assert_eq!(fakes.snapshot().cached, None);
        assert_eq!(fakes.released(), 2, "its context and its warm state");
    }

    #[test]
    fn publishing_the_same_runtime_again_releases_the_old_copy() {
        let fakes = Fakes::new();
        fakes.publish(english(), "first-ctx").unwrap();

        fakes.publish(english(), "second-ctx").unwrap();

        assert_eq!(fakes.snapshot().context, Some("second-ctx"));
        assert_eq!(
            fakes.snapshot().cached,
            None,
            "a runtime is never its own secondary"
        );
        assert_eq!(fakes.released(), 1);
    }

    #[test]
    fn a_third_runtime_replaces_the_secondary_with_the_previous_active_one() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.publish(english(), "english-ctx").unwrap();

        fakes
            .publish(key(WhisperModel::NbWhisperBase), "norwegian-ctx")
            .unwrap();

        let snapshot = fakes.snapshot();
        assert_eq!(snapshot.active, Some(key(WhisperModel::NbWhisperBase)));
        assert_eq!(snapshot.cached, Some((english(), "english-ctx", None)));
        assert_eq!(fakes.released(), 1, "the old secondary's context");
    }

    #[test]
    fn a_model_switch_during_inference_is_refused_and_changes_nothing() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        let (release, inference) = hold_state(&fakes);

        let result = fakes.publish(english(), "english-ctx");

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "got {result:?}"
        );
        release.send(()).unwrap();
        inference.join().unwrap();
        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(swedish()),
                context: Some("swedish-ctx"),
                state: None,
                cached: None,
            }
        );
        assert_eq!(
            fakes.released(),
            1,
            "only the freshly loaded context is dropped"
        );
    }

    #[test]
    fn activating_the_secondary_during_inference_is_refused_and_changes_nothing() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.publish(english(), "english-ctx").unwrap();
        let before = fakes.snapshot();
        let (release, inference) = hold_state(&fakes);

        let result = activate_cached_slots(&fakes.slots(), swedish(), SHORT);

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "got {result:?}"
        );
        release.send(()).unwrap();
        inference.join().unwrap();
        assert_eq!(fakes.snapshot(), before);
    }

    #[test]
    fn activating_the_secondary_swaps_the_runtimes_with_their_warm_states() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.publish(english(), "english-ctx").unwrap();
        fakes.warm("english-state");

        activate_cached_slots(&fakes.slots(), swedish(), SHORT).unwrap();

        assert_eq!(
            fakes.snapshot(),
            Snapshot {
                active: Some(swedish()),
                context: Some("swedish-ctx"),
                state: None,
                cached: Some((english(), "english-ctx", Some("english-state"))),
            }
        );
        assert_eq!(fakes.released(), 0);
    }

    #[test]
    fn activating_a_runtime_that_is_not_cached_fails_and_changes_nothing() {
        let fakes = Fakes::new();
        fakes.publish(swedish(), "swedish-ctx").unwrap();
        fakes.warm("swedish-state");
        let before = fakes.snapshot();

        let result = activate_cached_slots(&fakes.slots(), english(), SHORT);

        assert!(
            matches!(result, Err(DictationError::ModelNotLoaded)),
            "got {result:?}"
        );
        assert_eq!(fakes.snapshot(), before);
    }

    type Swap = fn(&RuntimeSlots<'_, Probe, Probe>) -> Result<(), DictationError>;

    /// Both transactions that take all four locks. `activate` finds nothing
    /// cached and reports `ModelNotLoaded` once it has the locks.
    fn each_swap() -> [(&'static str, Swap); 2] {
        [
            ("publish", |slots| {
                let ctx = Probe {
                    name: "new-ctx",
                    released: Arc::new(AtomicU32::new(0)),
                };
                publish_runtime(slots, ctx, english(), PATIENT)
            }),
            ("activate", |slots| {
                activate_cached_slots(slots, english(), PATIENT)
            }),
        ]
    }

    fn spawn_swap(fakes: &Arc<Fakes>, swap: Swap) -> thread::JoinHandle<()> {
        let swapper = Arc::clone(fakes);
        thread::spawn(move || {
            let _ = swap(&swapper.slots());
        })
    }

    // The documented lock order is state -> loaded_runtime (identity and context
    // together) -> cache. Seen from outside: while a swap waits on one lock it
    // holds every earlier lock and none of the later ones.

    #[test]
    fn a_swap_takes_the_state_lock_before_the_context() {
        for (name, swap) in each_swap() {
            let fakes = Fakes::new();
            let blocker = fakes.loaded.lock().unwrap();
            let worker = spawn_swap(&fakes, swap);

            wait_until_held(&fakes.state);
            assert!(
                !is_held(&fakes.cached),
                "{name}: cache taken before the context"
            );

            drop(blocker);
            worker.join().unwrap();
        }
    }

    #[test]
    fn a_swap_takes_the_runtime_identity_before_the_cache() {
        for (_name, swap) in each_swap() {
            let fakes = Fakes::new();
            let blocker = fakes.cached.lock().unwrap();
            let worker = spawn_swap(&fakes, swap);

            wait_until_held(&fakes.state);
            wait_until_held(&fakes.loaded);

            drop(blocker);
            worker.join().unwrap();
        }
    }
}
