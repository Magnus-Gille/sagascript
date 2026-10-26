//! How the app starts and is started again: the launch modes, the `setup`
//! hook with its startup order, a second launch or a reopen of the running app,
//! the Settings window, and the settings checks made at launch.

mod lifecycle;
#[cfg(target_os = "macos")]
mod menu;
mod mode;
mod settings;
mod startup;
mod window;

pub(crate) use lifecycle::{handle_run_event, handle_second_instance};
#[cfg(target_os = "macos")]
pub(crate) use menu::{build_guarded_menu, handle_guarded_menu};
#[cfg(windows)]
pub(crate) use mode::{attach_parent_console, cli_args_request_console_output};
pub(crate) use mode::{gui_launch_mode, GuiLaunchMode};
pub(crate) use settings::load_settings_with_permission_gate;
pub(crate) use startup::setup_app;
pub(crate) use window::open_settings_window;
