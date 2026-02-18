use tracing::info;

/// Hotkey management service
/// Uses tauri-plugin-global-shortcut for registration
/// Push-to-talk needs key-down + key-up events
pub struct HotkeyService {
    current_shortcut: Option<String>,
    suspended: bool,
}

impl HotkeyService {
    pub fn new() -> Self {
        Self {
            current_shortcut: None,
            suspended: false,
        }
    }

    pub fn current_shortcut(&self) -> Option<&str> {
        self.current_shortcut.as_deref()
    }

    pub fn is_suspended(&self) -> bool {
        self.suspended
    }

    /// Suspend hotkey (e.g. while recording a new shortcut)
    pub fn suspend(&mut self) {
        self.suspended = true;
        info!("Hotkey suspended");
    }

    /// Resume hotkey after suspension
    pub fn resume(&mut self) {
        self.suspended = false;
        info!("Hotkey resumed");
    }

    /// Set the current shortcut string (for state tracking)
    pub fn set_shortcut(&mut self, shortcut: &str) {
        self.current_shortcut = Some(shortcut.to_string());
        info!("Hotkey set to: {shortcut}");
    }
}
