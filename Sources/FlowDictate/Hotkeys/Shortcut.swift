import Foundation
import AppKit
import Carbon.HIToolbox

// MARK: - Shortcut Constants

/// Custom Fn modifier bit (Carbon doesn't have one, so we use a high bit)
/// This allows storing Fn state alongside Carbon modifiers without conflict.
let kModsFnBit = 1 << 16

/// Sentinel key code value meaning "modifiers-only shortcut" (no physical key, just modifiers)
let kKeyCodeModifiersOnly = -1

// MARK: - Modifier Conversion Functions

/// Convert NSEvent.ModifierFlags to our stored modifier mask (Carbon + Fn bit)
func modifierMask(from flags: NSEvent.ModifierFlags) -> Int {
    var mask = 0
    if flags.contains(.command) { mask |= Int(cmdKey) }
    if flags.contains(.option) { mask |= Int(optionKey) }
    if flags.contains(.control) { mask |= Int(controlKey) }
    if flags.contains(.shift) { mask |= Int(shiftKey) }
    if flags.contains(.function) { mask |= kModsFnBit }
    return mask
}

/// Convert CGEventFlags to our stored modifier mask (Carbon + Fn bit)
func modifierMask(from flags: CGEventFlags) -> Int {
    var mask = 0
    if flags.contains(.maskCommand) { mask |= Int(cmdKey) }
    if flags.contains(.maskAlternate) { mask |= Int(optionKey) }
    if flags.contains(.maskControl) { mask |= Int(controlKey) }
    if flags.contains(.maskShift) { mask |= Int(shiftKey) }
    if flags.contains(.maskSecondaryFn) { mask |= kModsFnBit }
    return mask
}

/// Extract Carbon-only modifiers from stored mask (strips Fn bit for use with HotKey package)
func carbonModifiers(from storedMask: Int) -> UInt32 {
    return UInt32(storedMask & ~kModsFnBit)
}

/// Convert stored modifier mask to NSEvent.ModifierFlags (for comparison)
func nsModifierFlags(from storedMask: Int) -> NSEvent.ModifierFlags {
    var flags: NSEvent.ModifierFlags = []
    if storedMask & Int(cmdKey) != 0 { flags.insert(.command) }
    if storedMask & Int(optionKey) != 0 { flags.insert(.option) }
    if storedMask & Int(controlKey) != 0 { flags.insert(.control) }
    if storedMask & Int(shiftKey) != 0 { flags.insert(.shift) }
    if storedMask & kModsFnBit != 0 { flags.insert(.function) }
    return flags
}

// MARK: - Shortcut Type Detection

/// Check if the shortcut requires CGEventTap backend (Fn involved or modifiers-only)
func requiresCGEventTapBackend(keyCode: Int, modifiers: Int) -> Bool {
    // Modifiers-only shortcuts can't be handled by Carbon
    if keyCode == kKeyCodeModifiersOnly {
        return true
    }
    // Fn modifier can't be handled by Carbon
    if modifiers & kModsFnBit != 0 {
        return true
    }
    return false
}

// MARK: - Modifier Key Detection

/// Check if a key code represents a modifier key
func isModifierKeyCode(_ keyCode: Int) -> Bool {
    switch keyCode {
    case kVK_Command, kVK_RightCommand,
         kVK_Shift, kVK_RightShift,
         kVK_Control, kVK_RightControl,
         kVK_Option, kVK_RightOption,
         kVK_Function:
        return true
    default:
        return false
    }
}

/// Map hardware key code to its corresponding modifier bit
func modifierBitForKeyCode(_ keyCode: Int) -> Int {
    switch keyCode {
    case kVK_Command, kVK_RightCommand:
        return Int(cmdKey)
    case kVK_Shift, kVK_RightShift:
        return Int(shiftKey)
    case kVK_Control, kVK_RightControl:
        return Int(controlKey)
    case kVK_Option, kVK_RightOption:
        return Int(optionKey)
    case kVK_Function:
        return kModsFnBit
    default:
        return 0
    }
}

// MARK: - Shortcut Description

/// Build a human-readable description of a shortcut
/// - Parameters:
///   - keyCode: The key code (-1 for modifiers-only)
///   - modifiers: The modifier mask (Carbon + Fn bit)
/// - Returns: String like "⌘⌥", "Fn+Z", "⌃⇧Space"
func describeShortcut(keyCode: Int, modifiers: Int) -> String {
    var parts: [String] = []

    // Build modifier symbols in standard macOS order
    if modifiers & Int(controlKey) != 0 { parts.append("⌃") }
    if modifiers & Int(optionKey) != 0 { parts.append("⌥") }
    if modifiers & Int(shiftKey) != 0 { parts.append("⇧") }
    if modifiers & Int(cmdKey) != 0 { parts.append("⌘") }
    if modifiers & kModsFnBit != 0 { parts.append("Fn") }

    // For modifiers-only shortcuts, just return the modifier symbols
    if keyCode == kKeyCodeModifiersOnly {
        return parts.joined()
    }

    // Add key name
    let keyName = keyCodeToKeyName(keyCode)

    // If we have Fn in the modifiers, use "+" separator for clarity
    if modifiers & kModsFnBit != 0 && !parts.isEmpty {
        return parts.joined() + "+" + keyName
    }

    parts.append(keyName)
    return parts.joined()
}

/// Convert a key code to a human-readable name
private func keyCodeToKeyName(_ keyCode: Int) -> String {
    switch keyCode {
    case kVK_Space: return "Space"
    case kVK_Return: return "Return"
    case kVK_Tab: return "Tab"
    case kVK_Delete: return "Delete"
    case kVK_ForwardDelete: return "⌦"
    case kVK_Escape: return "Esc"
    case kVK_Home: return "Home"
    case kVK_End: return "End"
    case kVK_PageUp: return "PageUp"
    case kVK_PageDown: return "PageDown"
    case kVK_LeftArrow: return "←"
    case kVK_RightArrow: return "→"
    case kVK_UpArrow: return "↑"
    case kVK_DownArrow: return "↓"
    case kVK_F1: return "F1"
    case kVK_F2: return "F2"
    case kVK_F3: return "F3"
    case kVK_F4: return "F4"
    case kVK_F5: return "F5"
    case kVK_F6: return "F6"
    case kVK_F7: return "F7"
    case kVK_F8: return "F8"
    case kVK_F9: return "F9"
    case kVK_F10: return "F10"
    case kVK_F11: return "F11"
    case kVK_F12: return "F12"
    case kVK_F13: return "F13"
    case kVK_F14: return "F14"
    case kVK_F15: return "F15"
    case kVK_F16: return "F16"
    case kVK_F17: return "F17"
    case kVK_F18: return "F18"
    case kVK_F19: return "F19"
    case kVK_F20: return "F20"
    case kVK_ANSI_A: return "A"
    case kVK_ANSI_B: return "B"
    case kVK_ANSI_C: return "C"
    case kVK_ANSI_D: return "D"
    case kVK_ANSI_E: return "E"
    case kVK_ANSI_F: return "F"
    case kVK_ANSI_G: return "G"
    case kVK_ANSI_H: return "H"
    case kVK_ANSI_I: return "I"
    case kVK_ANSI_J: return "J"
    case kVK_ANSI_K: return "K"
    case kVK_ANSI_L: return "L"
    case kVK_ANSI_M: return "M"
    case kVK_ANSI_N: return "N"
    case kVK_ANSI_O: return "O"
    case kVK_ANSI_P: return "P"
    case kVK_ANSI_Q: return "Q"
    case kVK_ANSI_R: return "R"
    case kVK_ANSI_S: return "S"
    case kVK_ANSI_T: return "T"
    case kVK_ANSI_U: return "U"
    case kVK_ANSI_V: return "V"
    case kVK_ANSI_W: return "W"
    case kVK_ANSI_X: return "X"
    case kVK_ANSI_Y: return "Y"
    case kVK_ANSI_Z: return "Z"
    case kVK_ANSI_0: return "0"
    case kVK_ANSI_1: return "1"
    case kVK_ANSI_2: return "2"
    case kVK_ANSI_3: return "3"
    case kVK_ANSI_4: return "4"
    case kVK_ANSI_5: return "5"
    case kVK_ANSI_6: return "6"
    case kVK_ANSI_7: return "7"
    case kVK_ANSI_8: return "8"
    case kVK_ANSI_9: return "9"
    case kVK_ANSI_Minus: return "-"
    case kVK_ANSI_Equal: return "="
    case kVK_ANSI_LeftBracket: return "["
    case kVK_ANSI_RightBracket: return "]"
    case kVK_ANSI_Backslash: return "\\"
    case kVK_ANSI_Semicolon: return ";"
    case kVK_ANSI_Quote: return "'"
    case kVK_ANSI_Comma: return ","
    case kVK_ANSI_Period: return "."
    case kVK_ANSI_Slash: return "/"
    case kVK_ANSI_Grave: return "`"
    // Modifier keys (for when they're used as the "key" in a modifier-key-as-hotkey scenario)
    case kVK_Command, kVK_RightCommand: return "⌘"
    case kVK_Shift, kVK_RightShift: return "⇧"
    case kVK_Control, kVK_RightControl: return "⌃"
    case kVK_Option, kVK_RightOption: return "⌥"
    case kVK_Function: return "Fn"
    default:
        return "Key\(keyCode)"
    }
}
