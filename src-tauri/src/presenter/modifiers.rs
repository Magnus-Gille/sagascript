const SHIFT_FLAG: u64 = 0x0002_0000;
const CONTROL_FLAG: u64 = 0x0004_0000;
const ALTERNATE_FLAG: u64 = 0x0008_0000;
const COMMAND_FLAG: u64 = 0x0010_0000;
const FN_FLAG: u64 = 0x0080_0000;
const BLOCKING_MODIFIERS: u64 = SHIFT_FLAG | CONTROL_FLAG | ALTERNATE_FLAG | COMMAND_FLAG | FN_FLAG;

fn modifiers_released_from_flags(flags: u64) -> bool {
    flags & BLOCKING_MODIFIERS == 0
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
#[allow(dead_code)]
extern "C" {
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
}

/// Read the combined-session modifier state without synthesizing or mutating
/// any event. Caps Lock is intentionally not part of the blocking mask.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub(crate) fn modifiers_released() -> bool {
    const COMBINED_SESSION_STATE: i32 = 0;
    let flags = unsafe { CGEventSourceFlagsState(COMBINED_SESSION_STATE) };
    modifiers_released_from_flags(flags)
}

#[cfg(test)]
mod tests {
    use super::modifiers_released_from_flags;

    #[test]
    fn modifier_flags_block_readiness() {
        for flags in [
            0x0002_0000, // Shift
            0x0004_0000, // Control
            0x0008_0000, // Alternate
            0x0010_0000, // Command
            0x0080_0000, // Fn
        ] {
            assert!(!modifiers_released_from_flags(flags));
        }
    }

    #[test]
    fn caps_lock_is_not_a_blocking_modifier() {
        assert!(modifiers_released_from_flags(0x0001_0000));
        assert!(modifiers_released_from_flags(0));
    }
}
