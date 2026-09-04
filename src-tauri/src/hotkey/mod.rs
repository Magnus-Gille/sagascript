pub mod health;
mod registration;
pub mod service;

#[cfg(target_os = "macos")]
mod macos_bare;

pub use health::{HotkeyHealth, HotkeyStatus, OperationalHotkey};
pub use registration::{register_shortcuts, unregister_shortcuts};
pub use service::HotkeyService;

#[cfg(target_os = "macos")]
pub use macos_bare::{bare_function_key_monitor_installed, install_bare_function_key_monitor};

#[derive(Clone, Copy)]
pub enum BareHotkeyState {
    Pressed,
    Released,
}
