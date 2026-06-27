// Linux-specific platform code
//
// These stubs exist for API parity with the macOS platform module. Linux has
// no macOS-style accessibility (TCC) permission gate or dock activation policy:
// global input simulation works without an explicit per-app grant. They are not
// currently called (commands.rs short-circuits the permission queries on
// non-macOS targets) but are kept for parity and future use.

/// Linux doesn't gate keyboard simulation behind an accessibility permission.
#[allow(dead_code)]
pub fn is_accessibility_trusted() -> bool {
    true
}

/// No-op on Linux — there is no accessibility permission to request.
#[allow(dead_code)]
pub fn request_accessibility_permission() {
    // Nothing to do
}

/// No-op on Linux — there is no dock activation policy to set (the app lives in
/// the system tray via libayatana-appindicator).
#[allow(dead_code)]
pub fn set_activation_policy_accessory() {
    // Nothing to do
}
