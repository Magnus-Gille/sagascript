use super::{restore_in, set_temporary_in, TemporaryText, Transaction};

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
    super::NativeTransaction::open()
        .unwrap()
        .set_text(original)
        .unwrap();
    let saved = super::set_temporary_text(temporary).unwrap();
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(temporary)
    );
    assert!(super::restore_if_unchanged(saved).unwrap());
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(original)
    );
    let saved = super::set_temporary_text(temporary).unwrap();
    super::NativeTransaction::open()
        .unwrap()
        .set_text(foreign)
        .unwrap();
    assert!(!super::restore_if_unchanged(saved).unwrap());
    assert_eq!(
        super::NativeTransaction::open()
            .unwrap()
            .text()
            .unwrap()
            .as_deref(),
        Some(foreign)
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
