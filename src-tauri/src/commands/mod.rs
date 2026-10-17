//! The app's Tauri commands, one file per domain (#370).
//!
//! `main.rs` registers every command as `commands::name` in
//! `tauri::generate_handler!`, and the rest of the crate reaches the shared
//! helpers (`lock_controller`, `file_transcription_context`, ...) the same
//! way. The domain modules are private and re-exported here with a glob, so
//! those paths do not change when a command moves between files. A glob rather
//! than a list of names: `#[tauri::command]` also generates a
//! `__cmd__<name>` macro beside the function, and `generate_handler!` finds
//! it through the same path. The CommandMap generator
//! (`contract/command_map.rs`) reads the files below this one for the same
//! reason.
//!
//! | Module | Commands |
//! |---|---|
//! | `settings` | `get_settings` and the setters: language, models, auto-paste, overlay, decoder options, engine options, onboarding. |
//! | `hotkey` | Shortcut profiles and modes, retry, registration health. |
//! | `state` | Read-only dictation state. |
//! | `transcribe` | Live dictation and the Teach tab's recording and file runs. |
//! | `transcribe_file` | `transcribe_file` and its cancellation. |
//! | `files` | File-run context, clipboard and save-dialog export, supported formats. |
//! | `models` | Model pickers, download state and downloads, engine status. |
//! | `dictionary` | Personal dictionary and training suggestions. |
//! | `update_recovery` | Update-result guard, recovery drafts, update preparation. |
//! | `permissions` | Accessibility and microphone onboarding queries. |
//! | `system` | Build info and host platform. |

use std::sync::{Arc, Mutex, MutexGuard};

use crate::app_controller::AppController;
use sagascript_core::transcription::WhisperBackend;

mod dictionary;
mod files;
mod hotkey;
#[cfg(any(target_os = "macos", test))]
mod main_thread;
mod models;
mod permissions;
mod persist;
mod settings;
mod state;
mod system;
mod transcribe;
mod transcribe_file;
mod update_recovery;

#[cfg(test)]
mod test_fixtures;

pub use dictionary::*;
pub use files::*;
pub use hotkey::*;
pub use models::*;
pub use permissions::*;
pub(crate) use persist::*;
pub use settings::*;
pub use state::*;
pub use system::*;
pub use transcribe::*;
pub use transcribe_file::*;
pub use update_recovery::*;

/// Shared app state type — uses std::sync::Mutex (not tokio) because
/// cpal::Stream is !Send and we need sync access from Tauri commands
pub type SharedController = Mutex<AppController>;

/// Lock name for the shared controller in poison-recovery logs.
pub const CONTROLLER_LOCK: &str = "app_controller";

/// Lock the shared controller, recovering it if a panic poisoned the mutex
/// (ADR 0005, #359). Every command and hotkey path re-reads `state()` and the
/// settings it needs, and interrupted work is ended through the existing
/// stop/cancel/error paths, so continuing with the recovered controller is
/// safe; propagating the poison would panic every later command and hotkey
/// press until the app restarts. Never unwrap the plain `lock()` result.
pub fn lock_controller(controller: &SharedController) -> MutexGuard<'_, AppController> {
    sagascript_core::sync::lock_or_recover(controller, CONTROLLER_LOCK)
}

/// Shared whisper backend — separate from AppController to avoid holding
/// the controller lock during blocking transcription
pub type SharedWhisper = Arc<WhisperBackend>;

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
