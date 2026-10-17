#[cfg(target_os = "macos")]
pub mod macos;
mod microphone;

pub use microphone::MicPermission;

/// The microphone permission of this host (status and request).
pub fn mic_permission() -> MicPermission {
    MicPermission
}
