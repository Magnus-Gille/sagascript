pub mod service;

pub use service::PasteService;

/// Whether an automatic paste may start now. The stored preference is the user's.
/// On macOS, Accessibility approval is checked at the moment of use and never
/// written back into the preference (SAGA-518). The paste service checks it again
/// before it sends a keystroke.
pub fn auto_paste_allowed_now(enabled: bool) -> bool {
    #[cfg(target_os = "macos")]
    {
        if enabled && !crate::platform::macos::is_accessibility_trusted() {
            tracing::warn!("Auto-paste waits for Accessibility approval; skipping this paste");
            return false;
        }
    }
    enabled
}
