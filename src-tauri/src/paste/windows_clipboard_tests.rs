use super::{
    finalize_generation_in, restore_in, set_temporary_in, spawn_restore_worker, TemporaryText,
    Transaction,
};
#[cfg(target_os = "windows")]
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// Deliberately opt-in: the Windows candidate job owns an ephemeral runner's
/// clipboard. Never run native smoke coverage against a developer's clipboard.
#[cfg(target_os = "windows")]
#[test]
#[ignore = "requires an isolated GitHub Windows runner clipboard"]
fn native_runner_clipboard_transaction_smoke() {
    assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    let original = "Sagascript isolated CI original";
    let temporary = "Sagascript isolated CI temporary";
    let foreign = "Sagascript isolated CI newer copy";
    let write_foreign_bounded = |text: &str| {
        let text = text.to_owned();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let owner = super::ClipboardOwner::new()?;
                let mut transaction = super::NativeTransaction::open_for(owner.0.as_ptr())?;
                transaction.set_text(&text)?;
                Ok(())
            })();
            let _ = sender.send(result);
        });
        receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("foreign clipboard write timed out")
            .expect("foreign clipboard write failed");
    };
    let write_foreign_while_pumping = |text: &str| {
        let text = text.to_owned();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let owner = super::ClipboardOwner::new()?;
                let mut transaction = super::NativeTransaction::open_for(owner.0.as_ptr())?;
                transaction.set_text(&text)?;
                Ok(())
            })();
            let _ = sender.send(result);
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            super::pump_messages();
            match receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(result) => {
                    result.expect("foreign clipboard write failed");
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("foreign clipboard write timed out while pumping")
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    panic!("foreign clipboard writer exited without a result")
                }
            }
        }
    };
    let start_contended_foreign_writer = |text: &str| {
        let text = text.to_owned();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (write_tx, write_rx) = mpsc::channel();
        let (written_tx, written_rx) = mpsc::channel::<Result<(), String>>();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let setup = (|| -> Result<_, String> {
                let owner = super::ClipboardOwner::new()?;
                let transaction = super::NativeTransaction::open_for(owner.0.as_ptr())?;
                Ok((owner, transaction))
            })();
            let (owner, mut transaction) = match setup {
                Ok(value) => value,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            let result = (|| -> Result<(), String> {
                write_rx
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|error| format!("foreign write request failed: {error}"))?;
                transaction.set_text(&text)?;
                drop(transaction);
                drop(owner);
                written_tx
                    .send(Ok(()))
                    .map_err(|_| "foreign write result receiver dropped".to_owned())?;
                Ok(())
            })();
            let _ = done_tx.send(result);
        });
        ready_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("contended foreign writer setup timed out")
            .expect("contended foreign writer setup failed");
        (write_tx, written_rx, done_rx)
    };
    write_foreign_bounded(original);
    let saved = super::set_temporary_text_on_thread(temporary).unwrap();
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(temporary)
    );
    assert!(super::restore_if_unchanged_on_thread(saved).unwrap());
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(original)
    );
    let saved = super::set_temporary_text_on_thread(temporary).unwrap();
    write_foreign_while_pumping(foreign);
    assert!(!super::restore_if_unchanged_on_thread(saved).unwrap());
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(foreign)
    );

    // A foreign copy of the identical text still counts as a new user copy.
    write_foreign_bounded(original);
    let saved = super::set_temporary_text_on_thread(temporary).unwrap();
    write_foreign_while_pumping(temporary);
    assert!(!super::restore_if_unchanged_on_thread(saved).unwrap());

    // Exercise the initial close/reopen race with a real foreign owner, not
    // only a changed sequence in the delayed restore phase.
    write_foreign_bounded(original);
    {
        let owner = super::ClipboardOwner::new().unwrap();
        let saved = super::set_temporary_in(
            super::NativeTransaction::open_for(owner.0.as_ptr()).unwrap(),
            temporary,
        )
        .unwrap();
        write_foreign_while_pumping(temporary);
        let transaction = super::NativeTransaction::open_for(owner.0.as_ptr()).unwrap();
        let owner_matches = clipboard_win::get_owner() == Some(owner.0);
        assert!(!owner_matches);
        let saved = finalize_generation_in(transaction, saved, temporary, owner_matches).unwrap();
        assert_eq!(saved.generation, None);
    }

    // A foreign writer empties the clipboard while our worker is waiting for
    // the restore request. The owner must pump WM_DESTROYCLIPBOARD, and the
    // generation/owner check must preserve the foreign text when scheduled.
    write_foreign_bounded(original);
    let pending = super::set_temporary_text(temporary).unwrap();
    write_foreign_bounded(foreign);
    pending.schedule_restore();
    // The production worker waits 100 ms before attempting a checked restore;
    // do not let the assertion pass merely because the foreign write happened
    // before that delayed work ran.
    std::thread::sleep(Duration::from_millis(150));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref()
            == Some(foreign)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(foreign)
    );

    // A caller-side failure drops the pending handle instead of scheduling;
    // the worker still pumps until it observes disconnect and must not restore.
    write_foreign_bounded(original);
    let pending = super::set_temporary_text(temporary).unwrap();
    write_foreign_bounded(foreign);
    drop(pending);
    // Allow the worker to observe sender disconnect and drop its session
    // before checking that the newer clipboard contents remain untouched.
    std::thread::sleep(Duration::from_millis(150));
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref()
            == Some(foreign)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(foreign)
    );

    // Hold the clipboard open while the creator thread enters its checked
    // restore. The foreign owner only opens the clipboard before readiness; a
    // synchronized EmptyClipboard call is released after the production
    // open-retry pump signals. No test-side message pumping is allowed here.
    write_foreign_bounded(original);
    let saved = super::set_temporary_text_on_thread(temporary).unwrap();
    let (retry_signal_tx, retry_signal_rx) = mpsc::channel();
    let (retry_resume_tx, retry_resume_rx) = mpsc::channel();
    let (write_tx, written_rx, done_rx) = start_contended_foreign_writer(foreign);
    super::set_open_retry_signal(Some((retry_signal_tx, retry_resume_rx)));
    let coordinator = std::thread::spawn(move || -> Result<(), String> {
        retry_signal_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| format!("production clipboard open retry did not run: {error}"))?;
        write_tx
            .send(())
            .map_err(|_| "foreign writer request receiver dropped".to_owned())?;
        retry_resume_tx
            .send(())
            .map_err(|_| "production retry resume receiver dropped".to_owned())?;
        written_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| {
                format!("production retry pump did not release foreign EmptyClipboard: {error}")
            })?
            .map_err(|error| format!("contended foreign write failed: {error}"))?;
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|error| format!("contended foreign writer did not terminate: {error}"))?
            .map_err(|error| format!("contended foreign writer failed: {error}"))
    });
    let restored = super::restore_if_unchanged_on_thread(saved);
    coordinator
        .join()
        .expect("foreign coordinator thread panicked")
        .expect("foreign coordinator failed");
    assert!(!restored.expect("checked restore should open after foreign release"));

    // Exercise the public channel-only lifecycle. The worker owns and drops
    // its HWND on one thread while the caller only receives a Send handle.
    write_foreign_bounded(original);
    let pending = super::set_temporary_text(temporary).unwrap();
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(temporary)
    );
    pending.schedule_restore();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref()
            == Some(original)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(original)
    );
}
use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Rc;

#[derive(Debug)]
struct FakeState {
    events: Vec<String>,
    held: bool,
    text: Result<Option<String>, String>,
    generation: Option<NonZeroU32>,
    set_errors: Vec<String>,
}

impl FakeState {
    fn new(text: &str, generation: Option<NonZeroU32>) -> Self {
        Self {
            events: Vec::new(),
            held: false,
            text: Ok(Some(text.to_owned())),
            generation,
            set_errors: Vec::new(),
        }
    }
}

fn state_without_text(generation: Option<NonZeroU32>) -> Rc<RefCell<FakeState>> {
    let state = state("unused", generation);
    state.borrow_mut().text = Ok(None);
    state
}

struct FakeTransaction {
    state: Rc<RefCell<FakeState>>,
}

impl FakeTransaction {
    fn new(state: Rc<RefCell<FakeState>>) -> Self {
        state.borrow_mut().held = true;
        Self { state }
    }
}

impl Drop for FakeTransaction {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        assert!(state.held, "transaction must remain held until Drop");
        state.held = false;
        state.events.push("drop".to_owned());
    }
}

impl Transaction for FakeTransaction {
    fn text(&mut self) -> Result<Option<String>, String> {
        let mut state = self.state.borrow_mut();
        assert!(state.held, "text must run while the transaction is held");
        state.events.push("text".to_owned());
        state.text.clone()
    }

    fn set_text(&mut self, text: &str) -> Result<(), String> {
        let mut state = self.state.borrow_mut();
        assert!(
            state.held,
            "set_text must run while the transaction is held"
        );
        state.events.push(format!("set_text:{text}"));
        if state.set_errors.is_empty() {
            Ok(())
        } else {
            Err(state.set_errors.remove(0))
        }
    }

    fn generation(&mut self) -> Option<NonZeroU32> {
        let mut state = self.state.borrow_mut();
        assert!(
            state.held,
            "generation must run while the transaction is held"
        );
        state.events.push("generation".to_owned());
        state.generation
    }
}

fn state(text: &str, generation: Option<NonZeroU32>) -> Rc<RefCell<FakeState>> {
    Rc::new(RefCell::new(FakeState::new(text, generation)))
}

fn generation(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).expect("test generation must be nonzero")
}

fn assert_before_drop(state: &Rc<RefCell<FakeState>>, event: &str) {
    let state = state.borrow();
    let drop_index = state
        .events
        .iter()
        .position(|entry| entry == "drop")
        .expect("transaction must be dropped");
    let event_index = state
        .events
        .iter()
        .position(|entry| entry == event)
        .unwrap_or_else(|| panic!("missing event {event:?}: {:?}", state.events));
    assert!(event_index < drop_index, "{event:?} occurred after Drop");
}

fn assert_events(state: &Rc<RefCell<FakeState>>, expected: &[&str]) {
    let state = state.borrow();
    let actual: Vec<_> = state.events.iter().map(String::as_str).collect();
    assert_eq!(actual, expected);
}

#[test]
fn initial_read_write_and_generation_finish_before_transaction_drop() {
    let state = state("before", Some(generation(7)));

    let saved = set_temporary_in(FakeTransaction::new(Rc::clone(&state)), "after")
        .expect("initial transaction should succeed");

    assert_eq!(saved.saved_text.as_deref(), Some("before"));
    assert_eq!(saved.generation, Some(generation(7)));
    assert_before_drop(&state, "text");
    assert_before_drop(&state, "set_text:after");
    assert_before_drop(&state, "generation");
    assert_eq!(
        state.borrow().events.last().map(String::as_str),
        Some("drop")
    );
}

#[test]
fn restore_checks_generation_and_writes_before_transaction_drop() {
    let state = state("current", Some(generation(7)));
    let saved = TemporaryText {
        saved_text: Some("before".to_owned()),
        generation: Some(generation(7)),
    };

    assert!(
        restore_in(FakeTransaction::new(Rc::clone(&state)), saved).expect("restore should succeed")
    );

    assert_before_drop(&state, "generation");
    assert_before_drop(&state, "set_text:before");
    let state_ref = state.borrow();
    let events = &state_ref.events;
    assert!(
        events.iter().position(|entry| entry == "generation")
            < events.iter().position(|entry| entry == "set_text:before")
    );
    assert_eq!(events.last().map(String::as_str), Some("drop"));
}

#[test]
fn foreign_generation_mismatch_skips_restore_write() {
    let state = state("foreign", Some(generation(8)));
    let saved = TemporaryText {
        saved_text: Some("before".to_owned()),
        generation: Some(generation(7)),
    };

    assert!(!restore_in(FakeTransaction::new(Rc::clone(&state)), saved)
        .expect("generation mismatch is a clean skip"));

    let state_ref = state.borrow();
    let events = &state_ref.events;
    assert!(events.iter().any(|entry| entry == "generation"));
    assert!(!events.iter().any(|entry| entry == "set_text:before"));
    assert_eq!(events.last().map(String::as_str), Some("drop"));
}

#[test]
fn finalized_generation_uses_post_close_value_only_for_owned_text() {
    let state = state("temporary", Some(generation(9)));
    let saved = TemporaryText {
        saved_text: Some("before".into()),
        generation: Some(generation(7)),
    };
    let finalized = finalize_generation_in(
        FakeTransaction::new(Rc::clone(&state)),
        saved,
        "temporary",
        true,
    )
    .unwrap();
    assert_eq!(finalized.generation, Some(generation(9)));
    assert_eq!(finalized.saved_text.as_deref(), Some("before"));
    assert_events(&state, &["text", "generation", "drop"]);
}

#[test]
fn foreign_owner_in_close_reopen_gap_never_authorizes_restore_even_for_identical_text() {
    let state = state("temporary", Some(generation(9)));
    let saved = TemporaryText {
        saved_text: Some("before".into()),
        generation: Some(generation(7)),
    };
    let finalized = finalize_generation_in(
        FakeTransaction::new(Rc::clone(&state)),
        saved,
        "temporary",
        false,
    )
    .unwrap();
    assert_eq!(finalized.generation, None);
    assert_events(&state, &["drop"]);
    assert!(!restore_in(FakeTransaction::new(Rc::clone(&state)), finalized).unwrap());
    assert_events(&state, &["drop", "drop"]);
}

#[test]
fn changed_text_with_same_owner_never_authorizes_restore() {
    let state = state("newer", Some(generation(9)));
    let saved = TemporaryText {
        saved_text: Some("before".into()),
        generation: Some(generation(7)),
    };
    let finalized = finalize_generation_in(
        FakeTransaction::new(Rc::clone(&state)),
        saved,
        "temporary",
        true,
    )
    .unwrap();
    assert_eq!(finalized.generation, None);
    assert_events(&state, &["text", "drop"]);
}

#[test]
fn missing_generation_fails_closed_without_restore_write() {
    let state = state("current", None);
    let saved = TemporaryText {
        saved_text: Some("before".to_owned()),
        generation: Some(generation(7)),
    };

    assert!(!restore_in(FakeTransaction::new(Rc::clone(&state)), saved)
        .expect("missing generation is a clean skip"));

    let state_ref = state.borrow();
    let events = &state_ref.events;
    assert!(events.iter().any(|entry| entry == "generation"));
    assert!(!events.iter().any(|entry| entry == "set_text:before"));
    assert_eq!(events.last().map(String::as_str), Some("drop"));
}

#[test]
fn absent_clipboard_text_is_saved_as_none_and_skips_restore() {
    let capture_state = state_without_text(Some(generation(7)));
    let saved = set_temporary_in(FakeTransaction::new(Rc::clone(&capture_state)), "after")
        .expect("absent text is a successful empty snapshot");

    assert_eq!(saved.saved_text, None);
    assert_eq!(saved.generation, Some(generation(7)));

    let restore_state = state("current", Some(generation(7)));

    assert!(
        !restore_in(FakeTransaction::new(Rc::clone(&restore_state)), saved)
            .expect("missing saved text is a clean skip")
    );

    let events = &restore_state.borrow().events;
    assert!(!events.iter().any(|entry| entry == "generation"));
    assert!(!events.iter().any(|entry| entry == "set_text:before"));
    assert_eq!(events.last().map(String::as_str), Some("drop"));
}

#[test]
fn text_error_is_preserved_and_transaction_drops() {
    let state = state("before", Some(generation(7)));
    state.borrow_mut().text = Err("text read failed".to_owned());

    let error = match set_temporary_in(FakeTransaction::new(Rc::clone(&state)), "after") {
        Ok(_) => panic!("text failure should be returned"),
        Err(error) => error,
    };

    assert_eq!(error, "text read failed");
    assert_events(&state, &["text", "drop"]);
}

#[test]
fn restore_set_error_is_preserved_and_transaction_drops() {
    let state = state("current", Some(generation(7)));
    state
        .borrow_mut()
        .set_errors
        .push("restore failed".to_owned());
    let saved = TemporaryText {
        saved_text: Some("before".to_owned()),
        generation: Some(generation(7)),
    };

    let error = restore_in(FakeTransaction::new(Rc::clone(&state)), saved)
        .expect_err("restore failure should be returned");

    assert_eq!(error, "restore failed");
    assert_events(&state, &["generation", "set_text:before", "drop"]);
}

#[test]
fn initial_write_failure_attempts_best_effort_restore_under_same_transaction() {
    let state = state("before", Some(generation(7)));
    state
        .borrow_mut()
        .set_errors
        .push("original write failed".to_owned());

    let error = match set_temporary_in(FakeTransaction::new(Rc::clone(&state)), "after") {
        Ok(_) => panic!("original write failure should be returned"),
        Err(error) => error,
    };

    assert_eq!(error, "original write failed");
    assert_events(
        &state,
        &["text", "set_text:after", "set_text:before", "drop"],
    );
}

#[derive(Default)]
struct SyntheticLifecycleState {
    init_thread: Option<ThreadId>,
    restore_thread: Option<ThreadId>,
    drop_thread: Option<ThreadId>,
    restored: bool,
}

struct SyntheticSession {
    state: Arc<Mutex<SyntheticLifecycleState>>,
    // Deliberately model the native HWND/session payload: it must not be
    // `Send`; only the channel-only PendingRestore handle crosses threads.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl SyntheticSession {
    fn new(state: Arc<Mutex<SyntheticLifecycleState>>) -> Self {
        Self {
            state,
            _not_send: std::marker::PhantomData,
        }
    }
}

impl Drop for SyntheticSession {
    fn drop(&mut self) {
        self.state.lock().unwrap().drop_thread = Some(std::thread::current().id());
    }
}

fn wait_for_synthetic_drop(state: &Arc<Mutex<SyntheticLifecycleState>>) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if state.lock().unwrap().drop_thread.is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("synthetic clipboard worker did not terminate");
}

#[test]
fn pending_restore_handle_is_send_channel_only() {
    fn assert_send<T: Send>() {}

    assert_send::<super::PendingRestore>();
}

#[test]
fn retry_pumps_before_each_bounded_open_delay() {
    let mut attempts = 0;
    let mut events = Vec::new();
    let result = super::retry_with_pump(
        || {
            attempts += 1;
            if attempts < 3 {
                Err("clipboard busy".to_owned())
            } else {
                Ok("opened")
            }
        },
        || events.push("pump"),
    )
    .expect("retry should eventually open");

    assert_eq!(result, "opened");
    assert_eq!(attempts, 3);
    assert_eq!(events, ["pump", "pump"]);
}

#[test]
fn retry_exhaustion_is_bounded_and_pumps_before_each_retry() {
    let mut attempts = 0;
    let mut pumps = 0;
    let error = super::retry_with_pump(
        || {
            attempts += 1;
            Err::<(), _>("clipboard busy".to_owned())
        },
        || pumps += 1,
    )
    .expect_err("persistent clipboard contention must fail");

    assert_eq!(error, "clipboard busy");
    assert_eq!(attempts, 6);
    assert_eq!(pumps, 5);
}

#[test]
fn restore_worker_success_restores_and_drops_on_creation_thread() {
    let state = Arc::new(Mutex::new(SyntheticLifecycleState::default()));
    let init_state = Arc::clone(&state);
    let restore_state = Arc::clone(&state);
    let pending = spawn_restore_worker(
        move || {
            let thread = std::thread::current().id();
            init_state.lock().unwrap().init_thread = Some(thread);
            Ok(SyntheticSession::new(init_state))
        },
        move |session: SyntheticSession| {
            let mut state = restore_state.lock().unwrap();
            state.restore_thread = Some(std::thread::current().id());
            state.restored = true;
            drop(state);
            drop(session);
            Ok(())
        },
        || {},
    )
    .expect("synthetic worker should become ready");

    pending.schedule_restore();
    wait_for_synthetic_drop(&state);

    let state = state.lock().unwrap();
    assert!(state.restored);
    assert_eq!(state.init_thread, state.restore_thread);
    assert_eq!(state.init_thread, state.drop_thread);
}

#[test]
fn restore_worker_disconnect_drops_without_restore_on_creation_thread() {
    let state = Arc::new(Mutex::new(SyntheticLifecycleState::default()));
    let init_state = Arc::clone(&state);
    let restore_state = Arc::clone(&state);
    let pending = spawn_restore_worker(
        move || {
            let thread = std::thread::current().id();
            init_state.lock().unwrap().init_thread = Some(thread);
            Ok(SyntheticSession::new(init_state))
        },
        move |session: SyntheticSession| {
            restore_state.lock().unwrap().restore_thread = Some(std::thread::current().id());
            drop(session);
            Ok(())
        },
        || {},
    )
    .expect("synthetic worker should become ready");

    drop(pending);
    wait_for_synthetic_drop(&state);

    let state = state.lock().unwrap();
    assert!(!state.restored);
    assert_eq!(state.restore_thread, None);
    assert_eq!(state.init_thread, state.drop_thread);
}

#[test]
fn restore_worker_initialization_error_is_returned_to_caller() {
    let result = spawn_restore_worker(
        || Err::<SyntheticSession, String>("synthetic initialization failed".to_owned()),
        |_session: SyntheticSession| Ok(()),
        || {},
    );

    match result {
        Ok(_) => panic!("initialization failure must not produce a pending handle"),
        Err(error) => assert_eq!(error, "synthetic initialization failed"),
    }
}
