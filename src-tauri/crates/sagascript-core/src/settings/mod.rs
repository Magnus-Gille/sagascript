mod hotkey;
mod presenter;
pub mod manager;
pub mod store;

pub use hotkey::{canonical_hotkey, validate_hotkey};
pub use manager::*;
pub use presenter::{PresenterConfig, PresenterFinishAction};
