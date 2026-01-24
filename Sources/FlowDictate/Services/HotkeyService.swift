import Foundation
import AppKit
import Carbon.HIToolbox
import HotKey

/// Unified hotkey service that selects the appropriate backend:
/// - Carbon (via HotKey package) for standard shortcuts (fast, no extra permissions)
/// - CGEventTap for Fn modifier or modifiers-only shortcuts (requires Input Monitoring)
final class HotkeyService {
    // MARK: - Callbacks

    var onKeyDown: (() -> Void)? {
        didSet {
            carbonBackend.onKeyDown = onKeyDown
            cgEventTapBackend.onKeyDown = onKeyDown
        }
    }

    var onKeyUp: (() -> Void)? {
        didSet {
            carbonBackend.onKeyUp = onKeyUp
            cgEventTapBackend.onKeyUp = onKeyUp
        }
    }

    // MARK: - Backends

    private let carbonBackend = CarbonHotkeyBackend()
    private let cgEventTapBackend = CGEventTapHotkeyService()

    // MARK: - State

    private var isRegistered: Bool = false
    private var currentKeyCode: Int = 0
    private var currentModifiers: Int = 0
    private var usingCGEventTap: Bool = false

    private let loggingService = LoggingService.shared

    // MARK: - Public Methods

    /// Register a global hotkey
    /// - Parameters:
    ///   - keyCode: Virtual key code (e.g., kVK_Space), or kKeyCodeModifiersOnly (-1) for modifiers-only
    ///   - modifiers: Modifier flags including kModsFnBit for Fn
    func register(keyCode: UInt32, modifiers: UInt32) {
        register(keyCode: Int(keyCode), modifiers: Int(modifiers))
    }

    /// Register a global hotkey (Int version)
    func register(keyCode: Int, modifiers: Int) {
        // Unregister existing hotkey first to prevent double-registration
        if isRegistered {
            print("[HotkeyService] Unregistering previous hotkey before re-registration")
            unregister()
        }

        currentKeyCode = keyCode
        currentModifiers = modifiers

        print("[HotkeyService] Registering hotkey - keyCode: \(keyCode), modifiers: \(modifiers)")

        // Decide which backend to use
        if requiresCGEventTapBackend(keyCode: keyCode, modifiers: modifiers) {
            print("[HotkeyService] Using CGEventTap backend (Fn or modifiers-only)")
            usingCGEventTap = true
            cgEventTapBackend.onKeyDown = onKeyDown
            cgEventTapBackend.onKeyUp = onKeyUp
            cgEventTapBackend.register(keyCode: keyCode, modifiers: modifiers)
        } else {
            print("[HotkeyService] Using Carbon backend (standard shortcut)")
            usingCGEventTap = false
            carbonBackend.onKeyDown = onKeyDown
            carbonBackend.onKeyUp = onKeyUp
            carbonBackend.register(keyCode: keyCode, modifiers: modifiers)
        }

        isRegistered = true
    }

    /// Unregister the current hotkey
    func unregister() {
        guard isRegistered else {
            print("[HotkeyService] No hotkey registered, skipping unregister")
            return
        }

        if usingCGEventTap {
            cgEventTapBackend.unregister()
        } else {
            carbonBackend.unregister()
        }

        isRegistered = false
        print("[HotkeyService] ✓ Unregistered hotkey")
    }

    /// Temporarily suspend the hotkey (used while recording a new shortcut)
    func suspend() {
        guard isRegistered else { return }
        if usingCGEventTap {
            cgEventTapBackend.suspend()
        } else {
            // Carbon backend doesn't have suspend, just unregister
            carbonBackend.unregister()
        }
    }

    /// Resume the hotkey after suspension
    func resume() {
        guard isRegistered else { return }
        if usingCGEventTap {
            cgEventTapBackend.resume()
        } else {
            // Re-register for Carbon backend
            carbonBackend.register(keyCode: currentKeyCode, modifiers: currentModifiers)
        }
    }
}

// MARK: - Carbon Backend (using HotKey package)

/// Internal backend using the HotKey package (wraps Carbon RegisterEventHotKey)
private final class CarbonHotkeyBackend {
    var onKeyDown: (() -> Void)?
    var onKeyUp: (() -> Void)?

    private var hotKey: HotKey?
    private let loggingService = LoggingService.shared

    func register(keyCode: Int, modifiers: Int) {
        // Strip Fn bit since Carbon can't handle it
        let carbonMods = carbonModifiers(from: modifiers)

        print("[CarbonHotkeyBackend] Registering - keyCode: \(keyCode), carbonMods: \(carbonMods)")

        // Convert Carbon modifiers to NSEvent modifiers for HotKey package
        var nsModifiers: NSEvent.ModifierFlags = []
        if carbonMods & UInt32(cmdKey) != 0 { nsModifiers.insert(.command) }
        if carbonMods & UInt32(shiftKey) != 0 { nsModifiers.insert(.shift) }
        if carbonMods & UInt32(optionKey) != 0 { nsModifiers.insert(.option) }
        if carbonMods & UInt32(controlKey) != 0 { nsModifiers.insert(.control) }

        // Convert key code to Key enum
        guard let key = Key(carbonKeyCode: UInt32(keyCode)) else {
            print("[CarbonHotkeyBackend] ✗ Invalid key code \(keyCode)")
            return
        }

        // Create the hotkey
        hotKey = HotKey(key: key, modifiers: nsModifiers, keyDownHandler: { [weak self] in
            print("[CarbonHotkeyBackend] ⬇️  Hotkey DOWN event fired")
            self?.loggingService.info(.Hotkey, LogEvent.Hotkey.keyDown, data: [:])
            self?.onKeyDown?()
        }, keyUpHandler: { [weak self] in
            print("[CarbonHotkeyBackend] ⬆️  Hotkey UP event fired")
            self?.loggingService.info(.Hotkey, LogEvent.Hotkey.keyUp, data: [:])
            self?.onKeyUp?()
        })

        print("[CarbonHotkeyBackend] ✓ Registered hotkey: \(key) with modifiers \(nsModifiers)")

        loggingService.info(.Hotkey, LogEvent.Hotkey.registered, data: [
            "keyCode": AnyCodable(keyCode),
            "modifiers": AnyCodable(Int(carbonMods)),
            "keyName": AnyCodable("\(key)"),
            "backend": AnyCodable("Carbon")
        ])
    }

    func unregister() {
        hotKey = nil
        loggingService.info(.Hotkey, LogEvent.Hotkey.unregistered, data: [
            "backend": AnyCodable("Carbon")
        ])
    }
}
