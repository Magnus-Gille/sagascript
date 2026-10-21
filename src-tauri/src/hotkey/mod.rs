pub mod configuration;
pub mod health;
mod registration;
pub(crate) mod transition;

#[cfg(target_os = "macos")]
mod macos_bare;

pub use health::{HotkeyHealth, HotkeyStatus, HotkeyStatusChange, OperationalHotkey};
pub(crate) use registration::NativeRegistrar;
pub use registration::{register_shortcuts, unregister_shortcuts};

#[cfg(target_os = "macos")]
pub use macos_bare::{bare_function_key_monitor_installed, install_bare_function_key_monitor};

#[derive(Clone, Copy)]
pub enum BareHotkeyState {
    Pressed,
    Released,
}
