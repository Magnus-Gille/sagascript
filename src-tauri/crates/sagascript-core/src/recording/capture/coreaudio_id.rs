//! macOS only: the identity of the default input device.
//!
//! `cpal` 0.15 names devices but does not expose an identity, and two
//! microphones can have the same name (two identical USB microphones). Core
//! Audio gives every device an `AudioObjectID` while it is present; the
//! default input device's is one property read of the system object. The id is
//! stable for as long as the device stays connected, which is exactly the span
//! of a recording; it is not stable across a restart or a reconnect and is
//! never stored.
//!
//! The call declares the one Core Audio function it needs instead of adding a
//! crate for it; Core Audio is already linked by `cpal`.

use std::ffi::c_void;

#[repr(C)]
struct AudioObjectPropertyAddress {
    selector: u32,
    scope: u32,
    element: u32,
}

#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const AudioObjectPropertyAddress,
        qualifier_size: u32,
        qualifier: *const c_void,
        data_size: *mut u32,
        data: *mut c_void,
    ) -> i32;
}

/// `kAudioObjectSystemObject`.
const SYSTEM_OBJECT: u32 = 1;
/// `kAudioHardwarePropertyDefaultInputDevice` ('dIn ').
const DEFAULT_INPUT_DEVICE: u32 = u32::from_be_bytes(*b"dIn ");
/// `kAudioObjectPropertyScopeGlobal` ('glob').
const SCOPE_GLOBAL: u32 = u32::from_be_bytes(*b"glob");
/// `kAudioObjectPropertyElementMain`.
const ELEMENT_MAIN: u32 = 0;
/// `kAudioObjectUnknown`: no such device.
const UNKNOWN: u32 = 0;

/// The `AudioObjectID` of the default input device, if there is one.
pub(super) fn default_input_device_id() -> Option<u64> {
    let address = AudioObjectPropertyAddress {
        selector: DEFAULT_INPUT_DEVICE,
        scope: SCOPE_GLOBAL,
        element: ELEMENT_MAIN,
    };
    let mut id: u32 = UNKNOWN;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: `address`, `size` and `id` are valid for the duration of the
    // call, `size` is the size of the buffer `id` that `data` points to, and
    // no qualifier is passed (size 0, null).
    let status = unsafe {
        AudioObjectGetPropertyData(
            SYSTEM_OBJECT,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            (&mut id as *mut u32).cast(),
        )
    };
    (status == 0 && id != UNKNOWN).then_some(u64::from(id))
}

#[cfg(test)]
mod tests {
    /// Reads a property of the system object; opens no stream and captures no
    /// audio. Ignored so that no test run touches the audio hardware at all.
    #[test]
    #[ignore = "queries the audio hardware (no capture); run by hand"]
    fn the_default_input_device_has_an_id_on_a_mac_with_a_microphone() {
        let id = super::default_input_device_id();
        eprintln!("default input device id: {id:?}");
    }
}
