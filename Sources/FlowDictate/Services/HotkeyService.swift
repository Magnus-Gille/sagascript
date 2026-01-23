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
    private var isRegistered: Bool = false
    private let loggingService = LoggingService.shared

    // MARK: - Public Methods

    /// Register a global hotkey
    /// - Parameters:
    ///   - keyCode: Virtual key code (e.g., kVK_Space)
    ///   - modifiers: Modifier flags (e.g., optionKey)
    func register(keyCode: UInt32, modifiers: UInt32) {
        // Unregister existing hotkey first to prevent double-registration
        if isRegistered {
            print("[HotkeyService] Unregistering previous hotkey before re-registration")
            unregister()
        }

        print("[HotkeyService] Registering hotkey - keyCode: \(keyCode), modifiers: \(modifiers)")

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
            print("[HotkeyService] ✗ Invalid key code \(keyCode)")
            return
        }

        // Create the hotkey
        hotKey = HotKey(key: key, modifiers: nsModifiers, keyDownHandler: { [weak self] in
            print("[HotkeyService] ⬇️  Hotkey DOWN event fired")
            self?.loggingService.info(.Hotkey, LogEvent.Hotkey.keyDown, data: [:])
            self?.onKeyDown?()
        }, keyUpHandler: { [weak self] in
            print("[HotkeyService] ⬆️  Hotkey UP event fired")
            self?.loggingService.info(.Hotkey, LogEvent.Hotkey.keyUp, data: [:])
            self?.onKeyUp?()
        })

        isRegistered = true
        print("[HotkeyService] ✓ Registered hotkey: \(key) with modifiers \(nsModifiers)")

        loggingService.info(.Hotkey, LogEvent.Hotkey.registered, data: [
            "keyCode": AnyCodable(Int(keyCode)),
            "modifiers": AnyCodable(Int(modifiers)),
            "keyName": AnyCodable("\(key)")
        ])
    }

    /// Unregister the current hotkey
    func unregister() {
        guard isRegistered else {
            print("[HotkeyService] No hotkey registered, skipping unregister")
            return
        }
        hotKey = nil
        isRegistered = false
        print("[HotkeyService] ✓ Unregistered hotkey")
        loggingService.info(.Hotkey, LogEvent.Hotkey.unregistered, data: [:])
    }
}
