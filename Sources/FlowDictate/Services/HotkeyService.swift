import Foundation
import AppKit
import Carbon.HIToolbox
import HotKey

/// Service for registering and handling global keyboard shortcuts
/// Uses HotKey package which wraps Carbon Events API
final class HotkeyService {
    // MARK: - Callbacks

    var onKeyDown: (() -> Void)?
    var onKeyUp: (() -> Void)?

    // MARK: - Private State

    private var hotKey: HotKey?

    // MARK: - Public Methods

    /// Register a global hotkey
    /// - Parameters:
    ///   - keyCode: Virtual key code (e.g., kVK_Space)
    ///   - modifiers: Modifier flags (e.g., optionKey)
    func register(keyCode: UInt32, modifiers: UInt32) {
        // Convert Carbon modifiers to HotKey modifiers
        var nsModifiers: NSEvent.ModifierFlags = []

        if modifiers & UInt32(cmdKey) != 0 {
            nsModifiers.insert(.command)
        }
        if modifiers & UInt32(shiftKey) != 0 {
            nsModifiers.insert(.shift)
        }
        if modifiers & UInt32(optionKey) != 0 {
            nsModifiers.insert(.option)
        }
        if modifiers & UInt32(controlKey) != 0 {
            nsModifiers.insert(.control)
        }

        // Convert key code to Key enum
        guard let key = Key(carbonKeyCode: keyCode) else {
            print("HotkeyService: Invalid key code \(keyCode)")
            return
        }

        // Create the hotkey
        hotKey = HotKey(key: key, modifiers: nsModifiers, keyDownHandler: { [weak self] in
            self?.onKeyDown?()
        }, keyUpHandler: { [weak self] in
            self?.onKeyUp?()
        })

        print("HotkeyService: Registered hotkey \(key) with modifiers \(nsModifiers)")
    }

    /// Unregister the current hotkey
    func unregister() {
        hotKey = nil
        print("HotkeyService: Unregistered hotkey")
    }
}
