use super::*;
use std::sync::mpsc;
use std::thread;

/// Generous for a wait that is released by an event, never by the clock.
const PATIENT: Duration = Duration::from_secs(30);
/// What a bystander is willing to wait for a lock that should be free.
const SHORT: Duration = Duration::from_millis(30);

/// Freeing a native context (weights, GPU buffers) or state takes a while. This
/// payload stands in for one: its release announces that it has begun and then
/// stays inside `Drop` until it is let go.
struct Slow {
    gate: Option<Arc<Gate>>,
}

struct Gate {
    begun: Mutex<mpsc::Sender<()>>,
    resume: Mutex<mpsc::Receiver<()>>,
}

impl Drop for Slow {
    fn drop(&mut self) {
        if let Some(gate) = &self.gate {
            let _ = gate.begun.lock().unwrap().send(());
            // Bounded, so that a payload released at the wrong time fails a
            // test instead of hanging it.
            let _ = gate.resume.lock().unwrap().recv_timeout(PATIENT);
        }
    }
}

/// The test's end of a `Slow` payload's gate.
struct Release {
    begun: mpsc::Receiver<()>,
    resume: mpsc::Sender<()>,
}

fn quick() -> Slow {
    Slow { gate: None }
}

fn slow() -> (Slow, Release) {
    let (begun_tx, begun_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let payload = Slow {
        gate: Some(Arc::new(Gate {
            begun: Mutex::new(begun_tx),
            resume: Mutex::new(resume_rx),
        })),
    };
    let release = Release {
        begun: begun_rx,
        resume: resume_tx,
    };
    (payload, release)
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

fn norwegian() -> RuntimeKey {
    key(WhisperModel::NbWhisperBase)
}

fn medium() -> RuntimeKey {
    key(WhisperModel::KbWhisperMedium)
}

struct Rig {
    state: Mutex<Option<Slow>>,
    loaded: Mutex<Option<Loaded<Slow>>>,
    cached: Mutex<Option<CachedWhisperRuntime<Slow, Slow>>>,
}

impl Rig {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(None),
            loaded: Mutex::new(None),
            cached: Mutex::new(None),
        })
    }

    fn slots(&self) -> RuntimeSlots<'_, Slow, Slow> {
        RuntimeSlots {
            state: &self.state,
            loaded_runtime: &self.loaded,
            cached_runtime: &self.cached,
        }
    }

    fn publish(&self, key: RuntimeKey, context: Slow) {
        publish_runtime(&self.slots(), context, key, PATIENT).expect("publish");
    }
}

/// What a bystander (a transcription that takes only the state lock, a lookup
/// of the active model) sees while a swap is busy releasing a displaced
/// payload.
#[derive(Debug, PartialEq)]
struct Seen {
    state_lock_acquired: bool,
    runtime_lock_free: bool,
    cache_lock_free: bool,
    active: Option<RuntimeKey>,
}

/// What a bystander must see: every lock free, and the new runtime already
/// active, so nobody waits on the teardown of something nobody uses.
fn unobstructed(active: RuntimeKey) -> Seen {
    Seen {
        state_lock_acquired: true,
        runtime_lock_free: true,
        cache_lock_free: true,
        active: Some(active),
    }
}

/// Take and release the state lock the way a transcription that takes only
/// that lock does (the CLI's), with its bounded wait: `false` is the
/// `ModelBusy` such a caller would report. A caller that also needs
/// `load_lock` waits for it to be released as well.
fn can_take_state(state: &Mutex<Option<Slow>>) -> bool {
    lock_state_slot_bounded(state, SHORT).is_ok()
}

/// A free lock can be taken at once. (A helper, so that the probe's guard is
/// gone before the next probe: a temporary in a struct literal lives to the
/// end of the statement.)
fn is_free<T>(lock: &Mutex<T>) -> bool {
    lock.try_lock().is_ok()
}

fn active_key(loaded: &Mutex<Option<Loaded<Slow>>>) -> Option<RuntimeKey> {
    let loaded = loaded.try_lock().ok()?;
    loaded.as_ref().map(|runtime| runtime.key)
}

/// Run `swap` on a thread, wait until the slow payload's release has begun,
/// look at the locks from outside, then let the release finish. Nothing here
/// waits on a lock without a bound, so a swap that keeps its locks fails the
/// assertion instead of hanging the test.
fn seen_during_release(
    rig: &Arc<Rig>,
    release: Release,
    swap: impl FnOnce(&Rig) + Send + 'static,
) -> Seen {
    let worker = {
        let rig = Arc::clone(rig);
        thread::spawn(move || swap(&rig))
    };
    release
        .begun
        .recv_timeout(PATIENT)
        .expect("the release of the displaced payload never began");

    let seen = Seen {
        state_lock_acquired: can_take_state(&rig.state),
        runtime_lock_free: is_free(&rig.loaded),
        cache_lock_free: is_free(&rig.cached),
        active: active_key(&rig.loaded),
    };

    release.resume.send(()).expect("let the release finish");
    worker.join().expect("the swap should not panic");
    seen
}

/// A runtime that does not fit the cache budget next to the new one is
/// dropped. With the old code that happened under all the swap's locks, so a
/// caller that arrived meanwhile and needed the state lock waited up to the
/// grace budget and then reported `ModelBusy`.
#[test]
fn a_runtime_outside_the_cache_budget_is_released_after_the_locks_are_let_go() {
    let rig = Rig::new();
    let (context, release) = slow();
    rig.publish(medium(), context);

    let seen = seen_during_release(&rig, release, |rig| rig.publish(english(), quick()));

    assert_eq!(seen, unobstructed(english()));
    assert!(rig.cached.lock().unwrap().is_none());
}

#[test]
fn the_replaced_secondary_is_released_after_the_locks_are_let_go() {
    let rig = Rig::new();
    let (context, release) = slow();
    rig.publish(swedish(), context);
    rig.publish(english(), quick());
    assert_eq!(
        rig.cached.lock().unwrap().as_ref().map(|r| r.runtime.key),
        Some(swedish())
    );

    let seen = seen_during_release(&rig, release, |rig| rig.publish(norwegian(), quick()));

    assert_eq!(seen, unobstructed(norwegian()));
    assert_eq!(
        rig.cached.lock().unwrap().as_ref().map(|r| r.runtime.key),
        Some(english()),
        "the previous active runtime took the secondary's place"
    );
}

#[test]
fn an_orphan_warm_state_is_released_after_the_locks_are_let_go() {
    let rig = Rig::new();
    let (orphan, release) = slow();
    *rig.state.lock().unwrap() = Some(orphan);

    let seen = seen_during_release(&rig, release, |rig| rig.publish(swedish(), quick()));

    assert_eq!(seen, unobstructed(swedish()));
    assert!(rig.state.lock().unwrap().is_none());
}

/// `load_model_inner` empties the secondary slot before opening a new set of
/// weights, so that two resident runtimes never overlap a third.
#[test]
fn discarding_the_secondary_releases_it_after_its_lock_is_let_go() {
    let rig = Rig::new();
    let (context, release) = slow();
    rig.publish(swedish(), context);
    rig.publish(english(), quick());

    let seen = seen_during_release(&rig, release, |rig| discard_secondary(&rig.slots()));

    assert_eq!(seen, unobstructed(english()));
    assert!(rig.cached.lock().unwrap().is_none());
}

/// The warm state that moves into the secondary slot with its runtime is not
/// released at all: only what nothing can use again is.
#[test]
fn a_runtime_that_fits_the_cache_is_kept_whole_and_nothing_is_released() {
    let rig = Rig::new();
    let (context, release) = slow();
    let (state, state_release) = slow();
    // Nobody will let a release finish: were one to start, it ends at once
    // (the gate is disconnected) after announcing itself on `begun`.
    let Release {
        begun: context_began,
        resume,
    } = release;
    drop(resume);
    let Release {
        begun: state_began,
        resume,
    } = state_release;
    drop(resume);
    rig.publish(swedish(), context);
    *rig.state.lock().unwrap() = Some(state);

    rig.publish(english(), quick());

    let cached = rig.cached.lock().unwrap();
    let cached = cached.as_ref().expect("the previous runtime is kept");
    assert_eq!(cached.runtime.key, swedish());
    assert!(cached.state.is_some(), "with its warm state");
    assert!(
        context_began.try_recv().is_err(),
        "the context was released"
    );
    assert!(
        state_began.try_recv().is_err(),
        "the warm state was released"
    );
}
