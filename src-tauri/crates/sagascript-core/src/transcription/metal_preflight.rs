//! macOS Metal availability guard.
//!
//! whisper.cpp currently registers its Metal backend before it honours the
//! context's GPU setting. If `MTLCreateSystemDefaultDevice()` returns `nil`
//! (for example in a process sandbox without GPU access), that registration
//! dereferences the missing device name and terminates the process. Checking
//! the same system API first lets Sagascript return an actionable Rust error
//! instead of a SIGSEGV.

use std::{ffi::c_void, ptr::NonNull};

use crate::error::DictationError;

#[link(name = "Metal", kind = "framework")]
unsafe extern "C" {
    fn MTLCreateSystemDefaultDevice() -> *mut c_void;
}

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_release(value: *mut c_void);
}

const METAL_UNAVAILABLE: &str = "macOS did not provide a Metal GPU to this process. \
    Run Sagascript from a logged-in user session with GPU access; restricted test \
    sandboxes cannot run the Metal transcription backend";

/// Verify that whisper.cpp can register its mandatory macOS Metal backend.
pub(super) fn ensure_available() -> Result<(), DictationError> {
    ensure_available_with(
        || {
            // Safety: this is Apple's parameter-free Metal device factory. A
            // non-null result follows the Create rule and is released below.
            unsafe { MTLCreateSystemDefaultDevice() }
        },
        |device| {
            // Safety: `device` is the retained Objective-C object returned by
            // `MTLCreateSystemDefaultDevice` in the closure above.
            unsafe { objc_release(device) }
        },
    )
}

fn ensure_available_with(
    create_device: impl FnOnce() -> *mut c_void,
    release_device: impl FnOnce(*mut c_void),
) -> Result<(), DictationError> {
    let device = NonNull::new(create_device())
        .ok_or_else(|| DictationError::TranscriptionFailed(METAL_UNAVAILABLE.to_string()))?;

    release_device(device.as_ptr());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, ptr};

    #[test]
    fn missing_device_returns_actionable_error_without_releasing() {
        let release_called = Cell::new(false);

        let error = ensure_available_with(ptr::null_mut, |_| release_called.set(true)).unwrap_err();

        assert!(!release_called.get());
        let message = error.to_string();
        assert!(message.contains("Metal GPU"));
        assert!(message.contains("restricted test sandboxes"));
    }

    #[test]
    fn available_device_is_released_once() {
        let fake_device = NonNull::<c_void>::dangling().as_ptr();
        let released = Cell::new(ptr::null_mut());

        ensure_available_with(|| fake_device, |device| released.set(device)).unwrap();

        assert_eq!(released.get(), fake_device);
    }
}
