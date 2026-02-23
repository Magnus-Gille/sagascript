use tracing::info;

/// Hotkey management service
/// Uses tauri-plugin-global-shortcut for registration
/// Push-to-talk needs key-down + key-up events
#[allow(dead_code)]
pub struct HotkeyService {
    current_shortcut: Option<String>,
    suspended: bool,
}

#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_has_no_shortcut() {
        let svc = HotkeyService::new();
        assert!(svc.current_shortcut().is_none());
    }

    #[test]
    fn new_is_not_suspended() {
        let svc = HotkeyService::new();
        assert!(!svc.is_suspended());
    }

    #[test]
    fn set_shortcut_stores_value() {
        let mut svc = HotkeyService::new();
        svc.set_shortcut("Control+Shift+Space");
        assert_eq!(svc.current_shortcut(), Some("Control+Shift+Space"));
    }

    #[test]
    fn set_shortcut_overwrites_previous() {
        let mut svc = HotkeyService::new();
        svc.set_shortcut("Control+Shift+Space");
        svc.set_shortcut("Alt+D");
        assert_eq!(svc.current_shortcut(), Some("Alt+D"));
    }

    #[test]
    fn suspend_and_resume() {
        let mut svc = HotkeyService::new();
        assert!(!svc.is_suspended());

        svc.suspend();
        assert!(svc.is_suspended());

        svc.resume();
        assert!(!svc.is_suspended());
    }

    #[test]
    fn suspend_is_idempotent() {
        let mut svc = HotkeyService::new();
        svc.suspend();
        svc.suspend();
        assert!(svc.is_suspended());
        svc.resume();
        assert!(!svc.is_suspended());
    }
}
