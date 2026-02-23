// Windows-specific platform code

/// Windows doesn't have macOS-style accessibility permission gates.
/// Input simulation via SendInput works without explicit user grants.
pub fn is_accessibility_trusted() -> bool {
    true
}

/// No-op on Windows — accessibility permissions aren't needed.
pub fn request_accessibility_permission() {
    // Nothing to do
}
