//! Presenter coordination. All target handles and native actions stay on the
//! main thread. Inference returns only through a generation-checked callback.
mod gate;
#[cfg(any(target_os = "macos", test))]
mod observation;
#[cfg(target_os = "macos")]
mod macos_target;
#[cfg(target_os = "macos")]
mod modifiers;
#[cfg(target_os = "macos")]
mod utf16;

use std::cell::RefCell;
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};
use sagascript_cli::presenter::PresenterRequest;
use sagascript_core::settings::HotkeyMode;
#[cfg(target_os = "macos")]
use sagascript_core::settings::PresenterFinishAction;
use tauri::{Emitter, Manager};
use crate::{app_controller::{AppState, HotkeyDownResult}, SharedController};
use gate::{Action, Gate, Phase};

struct Session {
    generation: u64,
    gate: Gate,
    text: Option<String>,
    #[cfg(target_os = "macos")]
    target: Option<macos_target::TargetGuard>,
    #[cfg(target_os = "macos")]
    expected: Option<(String, usize, usize)>,
    #[cfg(target_os = "macos")]
    deadline: Option<Instant>,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
}

#[cfg(target_os = "macos")]
fn action(value: PresenterFinishAction) -> Action {
    match value {
        PresenterFinishAction::InsertOnly => Action::InsertOnly,
        PresenterFinishAction::Return => Action::Return,
        PresenterFinishAction::CommandReturn => Action::CommandReturn,
    }
}

fn status(app: &tauri::AppHandle, phase: Phase) {
    let value = match phase {
        Phase::Listening => "listening",
        Phase::Transcribing => "transcribing",
        Phase::AwaitingInsertion => "verifying_insertion",
        Phase::Inserted => "inserted",
        Phase::Submitting => "submitting",
        Phase::Sent => "sent",
        Phase::Cancelled => "cancelled",
        Phase::Draft => "draft",
        Phase::Failed => "failed",
        Phase::NoSpeech => "no_speech",
        Phase::SubmitUncertain => "submit_uncertain",
    };
    let _ = app.emit("presenter-status", value);
}

/// Entry points are dispatched on the main thread by both hotkey and argv
/// transports. No target text is accepted from those transports.
pub fn handle_request(app: &tauri::AppHandle, request: PresenterRequest) {
    match request {
        PresenterRequest::Start { profile_id } => start(app, profile_id.as_deref()),
        PresenterRequest::Finish => finish(app),
        PresenterRequest::Cancel => cancel(app),
    }
}

fn start(app: &tauri::AppHandle, requested_profile: Option<&str>) {
    let controller: tauri::State<'_, SharedController> = app.state();
    let settings = {
        let c = controller.lock().unwrap();
        if c.state() != AppState::Idle || c.settings().hotkey_mode != HotkeyMode::Presenter {
            return;
        }
        c.settings().clone()
    };
    if settings.validate_shortcut_configuration().is_err() {
        let _ = app.emit(crate::events::event::ERROR, "Presenter shortcut configuration is invalid.");
        return;
    }
    let profiles = settings.resolved_hotkey_profiles();
    let profile = match requested_profile {
        Some(id) => profiles.into_iter().find(|p| p.id == id),
        None => profiles.iter().find(|p| p.id == "default").cloned()
            .or_else(|| profiles.into_iter().next()),
    };
    let Some(profile) = profile else {
        let _ = app.emit(crate::events::event::ERROR, "Presenter profile was not found.");
        return;
    };
    // Capture before audio-start feedback or any overlay/window operation.
    #[cfg(target_os = "macos")]
    let target = match macos_target::TargetGuard::capture() {
        Ok(target) => Some(target),
        Err(code) => {
            tracing::warn!(?code, "Presenter target cannot be verified; keeping a draft only");
            None
        }
    };
    #[cfg(target_os = "macos")]
    let selected_action = target.as_ref()
        .and_then(|t| settings.presenter.app_actions.get(t.app_id()).copied())
        .map(action).unwrap_or_default();
    #[cfg(not(target_os = "macos"))]
    let selected_action = Action::InsertOnly;
    #[cfg(target_os = "macos")]
    let target_known = target.is_some();
    #[cfg(not(target_os = "macos"))]
    let target_known = false;
    let mut c = controller.lock().unwrap();
    if c.settings().hotkey_mode != HotkeyMode::Presenter
        || c.settings().presenter != settings.presenter
        || c.settings().resolved_hotkey_profiles() != settings.resolved_hotkey_profiles() {
        return;
    }
    match c.handle_hotkey_down_for_profile(profile.clone()) {
        Ok(HotkeyDownResult::StartedRecording) => {
            let generation = c.recording_generation();
            SESSION.with(|slot| *slot.borrow_mut() = Some(Session {
                generation, gate: Gate::new(selected_action, target_known), text: None,
                #[cfg(target_os = "macos")]
                target,
                #[cfg(target_os = "macos")]
                expected: None,
                #[cfg(target_os = "macos")]
                deadline: None,
            }));
            drop(c);
            crate::select_profile_menu(app, &profile);
            let _ = app.emit(crate::events::event::ACTIVE_HOTKEY_PROFILE_CHANGED, profile);
            let _ = app.emit(crate::events::event::STATE_CHANGED, "recording");
            status(app, Phase::Listening);
            crate::update_tray_status(app, "recording");
            if settings.show_overlay { crate::overlay::show(app); }
        }
        Ok(_) => {}
        Err(error) => { let _ = app.emit(crate::events::event::ERROR, error.to_string()); }
    }
}

fn finish(app: &tauri::AppHandle) {
    let controller: tauri::State<'_, SharedController> = app.state();
    let c = controller.lock().unwrap();
    if !c.should_finish_presenter() { return; }
    let generation = c.recording_generation();
    let accepted = SESSION.with(|slot| slot.borrow_mut().as_mut()
        .filter(|s| s.generation == generation).is_some_and(|s| s.gate.finish()));
    drop(c);
    if accepted {
        status(app, Phase::Transcribing);
        crate::stop_recording_and_transcribe(app, &controller);
    }
}

pub fn cancel(app: &tauri::AppHandle) {
    let controller: tauri::State<'_, SharedController> = app.state();
    let mut c = controller.lock().unwrap();
    if !c.is_presenter_session() { return; }
    let generation = c.recording_generation();
    let recording = c.state() == AppState::Recording;
    SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(session) = slot.as_mut().filter(|s| s.generation == generation) {
            session.gate.cancel();
            if recording { *slot = None; }
        }
    });
    if recording { c.cancel_recording(); }
    drop(c);
    status(app, Phase::Cancelled);
    // During inference remain busy until its worker finishes. A new Start
    // cannot inherit a queued insertion or receive the old worker's result.
    if recording { idle_ui(app); }
}

fn idle_ui(app: &tauri::AppHandle) {
    crate::overlay::hide(app);
    crate::update_tray_status(app, "idle");
    let _ = app.emit(crate::events::event::STATE_CHANGED, "idle");
}

fn terminal(app: &tauri::AppHandle, session: Session, error: Option<String>) {
    let controller: tauri::State<'_, SharedController> = app.state();
    let mut c = controller.lock().unwrap();
    if c.recording_generation() != session.generation { return; }
    let phase = session.gate.phase();
    if phase == Phase::Cancelled {
        c.complete_cancelled_recording();
    } else if let Some(error) = error {
        c.on_transcription_error(&error);
        let _ = app.emit(crate::events::event::ERROR, error);
    } else if let Some(text) = &session.text {
        c.on_transcription_success(text);
        let _ = app.emit(crate::events::event::TRANSCRIPTION_RESULT, text);
        crate::update_tray_last_result(app, text);
    } else {
        c.on_no_speech_detected();
    }
    drop(c);
    idle_ui(app);
    status(app, phase);
    let message = match phase {
        Phase::Inserted => "Presenter: inserted; not submitted",
        Phase::Sent => "Presenter: Submit key sent; delivery not confirmed",
        Phase::Cancelled => "Presenter: cancelled",
        Phase::Draft => "Presenter: not sent; copy the draft from Dictate",
        Phase::Failed => "Presenter: failed; check Dictate",
        Phase::NoSpeech => "Presenter: no speech detected",
        Phase::SubmitUncertain => "Presenter: may have submitted; check the destination",
        _ => "Presenter: check Dictate",
    };
    if !app.state::<crate::hotkey::HotkeyHealth>().is_failed() {
        crate::set_status_menu_text(app, message);
        if let Some(tray) = app.tray_by_id("main") {
            let _ = tray.set_tooltip(Some(&format!("Sagascript\n{message}")));
        }
    }
    // Do not focus Settings: a native key may have been queued, or the user
    // may intentionally be editing elsewhere. The draft remains in Dictate.
}

/// Called on the main thread after the existing transcription pipeline. A
/// stale generation, cancellation or missing target can never reach paste.
pub fn complete(app: &tauri::AppHandle, generation: u64, result: Result<String, String>) {
    let session = SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|s| s.generation == generation) { slot.take() } else { None }
    });
    let Some(mut session) = session else { return; };
    let error = result.as_ref().err().cloned();
    session.gate.transcribed(result.is_ok(), result.as_ref().is_ok_and(|s| !s.trim().is_empty()));
    if session.gate.phase() != Phase::Cancelled {
        session.text = result.ok().filter(|s| !s.trim().is_empty());
    }
    if session.gate.phase() != Phase::AwaitingInsertion {
        terminal(app, session, error);
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let controller: tauri::State<'_, SharedController> = app.state();
        let auto_paste = controller.lock().unwrap().settings().auto_paste;
        let before = session.target.as_ref().and_then(|t| {
            if !auto_paste || !modifiers::modifiers_released()
                || t.snapshot_matches() != Ok(true) { return None; }
            t.observed_value_and_selection().ok()
        });
        let expected = before.and_then(|(original, location, length)| {
            let payload = crate::paste::service::paste_payload(session.text.as_deref()?);
            let value = utf16::replace_utf16_range(&original, location, length, &payload)?;
            Some((value, location.checked_add(payload.encode_utf16().count())?, 0))
        });
        if session.gate.may_insert(expected.is_some()) {
            let pasted = crate::paste::PasteService::new().paste_checked(
                session.text.as_deref().unwrap_or_default(),
                || session.target.as_ref().is_some_and(|t| t.snapshot_matches() == Ok(true))
                    && modifiers::modifiers_released(),
            );
            if pasted.is_ok() {
                session.expected = expected;
                session.deadline = Some(Instant::now() + Duration::from_secs(2));
                SESSION.with(|slot| *slot.borrow_mut() = Some(session));
                status(app, Phase::AwaitingInsertion);
                schedule_observation(app, generation);
                return;
            }
            session.gate.insertion_timed_out();
        }
    }
    #[cfg(not(target_os = "macos"))]
    session.gate.target_changed();
    session.gate.insertion_timed_out();
    terminal(app, session, None);
}

#[cfg(target_os = "macos")]
fn schedule_observation(app: &tauri::AppHandle, generation: u64) {
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        crate::dispatch_to_main(&handle, move |app| observe(app, generation));
    });
}

#[cfg(target_os = "macos")]
fn observe(app: &tauri::AppHandle, generation: u64) {
    let session = SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.as_ref().is_some_and(|s| s.generation == generation) { slot.take() } else { None }
    });
    let Some(mut session) = session else { return; };
    if session.gate.phase() == Phase::Cancelled {
        terminal(app, session, None);
        return;
    }
    if session.deadline.is_none_or(|deadline| Instant::now() >= deadline) {
        session.gate.insertion_timed_out();
        terminal(app, session, None);
        return;
    }
    let observation = session.target.as_ref().and_then(|t| t.observed_value_and_selection().ok());
    let decision = observation::decide(
        observation.is_some(), observation == session.expected,
        session.deadline.is_some_and(|deadline| Instant::now() < deadline),
        modifiers::modifiers_released(),
    );
    if decision == observation::Decision::Draft {
        session.gate.target_changed();
        session.gate.insertion_timed_out();
    } else if decision == observation::Decision::Proven {
        let target = session.target.as_ref().expect("observation requires target");
        let controller: tauri::State<'_, SharedController> = app.state();
        let c = controller.lock().unwrap();
        let still_allowed = c.settings().hotkey_mode == HotkeyMode::Presenter
            && c.settings().auto_paste
            && c.settings().presenter.app_actions.get(target.app_id()).copied().map(action)
                == Some(session.gate.action());
        drop(c);
        let same_target = target.unchanged() == Ok(true)
            && session.deadline.is_some_and(|deadline| Instant::now() < deadline);
        if let Some(submit) = session.gate.authorize_submit(true, same_target, still_allowed) {
            // Guard again inside the native action immediately before the key.
            let success = submit_return(submit, || target.unchanged() == Ok(true)
                && target.observed_value_and_selection().ok() == session.expected
                && session.deadline.is_some_and(|deadline| Instant::now() < deadline));
            session.gate.submit_completed(success);
        }
    } else if decision == observation::Decision::Wait {
        SESSION.with(|slot| *slot.borrow_mut() = Some(session));
        schedule_observation(app, generation);
        return;
    } else {
        session.gate.insertion_timed_out();
    }
    terminal(app, session, None);
}

#[cfg(target_os = "macos")]
fn submit_return(action: Action, verify: impl FnOnce() -> bool) -> bool {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};
    if !matches!(action, Action::Return | Action::CommandReturn)
        || !crate::platform::macos::is_accessibility_trusted() { return false; }
    let Ok(mut enigo) = Enigo::new(&Settings::default()) else { return false; };
    if !verify() || !modifiers::modifiers_released() { return false; }
    if action == Action::Return { return enigo.key(Key::Return, Direction::Click).is_ok(); }
    if enigo.key(Key::Meta, Direction::Press).is_err() {
        let _ = enigo.key(Key::Meta, Direction::Release);
        return false;
    }
    let clicked = enigo.key(Key::Return, Direction::Click).is_ok();
    let released = enigo.key(Key::Meta, Direction::Release).is_ok();
    clicked && released
}
