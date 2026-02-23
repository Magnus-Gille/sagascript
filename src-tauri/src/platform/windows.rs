// Windows-specific platform code
//
// These stubs exist for API parity with the macOS platform module.
// They are not currently called because commands.rs inlines the
// Windows logic, but they are kept for future use when the platform
// abstraction is unified.

/// Windows doesn't have macOS-style accessibility permission gates.
/// Input simulation via SendInput works without explicit user grants.
#[allow(dead_code)]
pub fn is_accessibility_trusted() -> bool {
    true
}

/// No-op on Windows — accessibility permissions aren't needed.
#[allow(dead_code)]
pub fn request_accessibility_permission() {
    // Nothing to do
}
