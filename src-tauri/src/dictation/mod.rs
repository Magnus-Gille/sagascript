//! Live dictation: what a hotkey press or release starts and stops, the
//! recording-to-paste pipeline that follows, and the warm-up of the engines it
//! uses.
//!
//! The hotkey callbacks must stay short (ADR 0003): they change the controller
//! state and hand everything slower to a task. Auto-paste runs on the main
//! thread on macOS; keep it, and anything else that waits, off the lock of the
//! shared controller.

mod engine;
mod pipeline;
mod trigger;

pub(crate) use engine::warm_models_at_startup;
pub(crate) use trigger::handle_hotkey_event;
