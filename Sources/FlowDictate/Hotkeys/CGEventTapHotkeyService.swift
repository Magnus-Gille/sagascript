import Foundation
import AppKit
import Carbon.HIToolbox

/// A hotkey service using CGEventTap for shortcuts that Carbon can't handle:
/// - Fn modifier combinations (Fn+Z, Fn alone)
/// - Modifiers-only shortcuts (⌘ alone, ⌥⌘ chord)
///
/// Uses .cgSessionEventTap with .listenOnly for minimal privilege requirements.
/// Requires Input Monitoring permission in System Settings.
final class CGEventTapHotkeyService {
    // MARK: - Callbacks

    var onKeyDown: (() -> Void)?
    var onKeyUp: (() -> Void)?

    // MARK: - Configuration

    private var storedKeyCode: Int = 0
    private var storedModifiers: Int = 0
    private var isModifiersOnly: Bool = false

    // MARK: - State

    private var tap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?
    private var isRegistered: Bool = false
    private var isSuspended: Bool = false

    // State for modifiers-only detection (tap-only semantics)
    private var candidateActive: Bool = false
    private var candidateCancelledByKey: Bool = false

    // For normal shortcuts: track if we've triggered to avoid double-fire
    private var hasTriggeredKeyDown: Bool = false

    private let loggingService = LoggingService.shared

    // MARK: - Public Methods

    /// Register a hotkey using CGEventTap
    /// - Parameters:
    ///   - keyCode: Virtual key code, or kKeyCodeModifiersOnly (-1) for modifiers-only
    ///   - modifiers: Modifier mask including kModsFnBit for Fn
    func register(keyCode: Int, modifiers: Int) {
        if isRegistered {
            print("[CGEventTapHotkeyService] Unregistering previous before re-registration")
            unregister()
        }

        storedKeyCode = keyCode
        storedModifiers = modifiers
        isModifiersOnly = (keyCode == kKeyCodeModifiersOnly)

        print("[CGEventTapHotkeyService] Registering - keyCode: \(keyCode), modifiers: \(modifiers), isModifiersOnly: \(isModifiersOnly)")

        // Check Input Monitoring permission
        if !checkAndRequestPermission() {
            print("[CGEventTapHotkeyService] ✗ Input Monitoring permission not granted")
            return
        }

        // Create the event tap
        guard createEventTap() else {
            print("[CGEventTapHotkeyService] ✗ Failed to create event tap")
            return
        }

        isRegistered = true
        isSuspended = false
        print("[CGEventTapHotkeyService] ✓ Registered with CGEventTap")

        loggingService.info(.Hotkey, LogEvent.Hotkey.registered, data: [
            "keyCode": AnyCodable(keyCode),
            "modifiers": AnyCodable(modifiers),
            "backend": AnyCodable("CGEventTap"),
            "isModifiersOnly": AnyCodable(isModifiersOnly)
        ])
    }

    /// Unregister the hotkey
    func unregister() {
        guard isRegistered else { return }

        destroyEventTap()
        isRegistered = false
        isSuspended = false
        resetState()

        print("[CGEventTapHotkeyService] ✓ Unregistered")
        loggingService.info(.Hotkey, LogEvent.Hotkey.unregistered, data: [
            "backend": AnyCodable("CGEventTap")
        ])
    }

    /// Temporarily suspend the hotkey (disable the tap)
    func suspend() {
        guard isRegistered, !isSuspended, let tap = tap else { return }
        CGEvent.tapEnable(tap: tap, enable: false)
        isSuspended = true
        resetState()
        print("[CGEventTapHotkeyService] Suspended")
    }

    /// Resume after suspension
    func resume() {
        guard isRegistered, isSuspended, let tap = tap else { return }
        CGEvent.tapEnable(tap: tap, enable: true)
        isSuspended = false
        resetState()
        print("[CGEventTapHotkeyService] Resumed")
    }

    // MARK: - Permission Handling

    private func checkAndRequestPermission() -> Bool {
        // CGPreflightListenEventAccess returns true if permission is granted
        if CGPreflightListenEventAccess() {
            return true
        }

        // Request permission - this shows system prompt on first call
        let granted = CGRequestListenEventAccess()
        if !granted {
            // Show guidance to user
            DispatchQueue.main.async {
                self.showPermissionAlert()
            }
        }
        return granted
    }

    private func showPermissionAlert() {
        let alert = NSAlert()
        alert.messageText = "Input Monitoring Permission Required"
        alert.informativeText = """
        FlowDictate needs Input Monitoring permission to use this hotkey type (Fn key or modifier-only shortcuts).

        Please enable it in:
        System Settings → Privacy & Security → Input Monitoring

        Then restart FlowDictate.
        """
        alert.alertStyle = .warning
        alert.addButton(withTitle: "Open System Settings")
        alert.addButton(withTitle: "Cancel")

        if alert.runModal() == .alertFirstButtonReturn {
            // Open System Settings to Input Monitoring
            if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent") {
                NSWorkspace.shared.open(url)
            }
        }
    }

    // MARK: - Event Tap Management

    private func createEventTap() -> Bool {
        let eventMask: CGEventMask =
            (1 << CGEventType.keyDown.rawValue) |
            (1 << CGEventType.keyUp.rawValue) |
            (1 << CGEventType.flagsChanged.rawValue)

        // Use Unmanaged to pass self to the callback
        let refcon = UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque())

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .listenOnly,
            eventsOfInterest: eventMask,
            callback: { proxy, type, event, userInfo -> Unmanaged<CGEvent>? in
                guard let userInfo = userInfo else {
                    return Unmanaged.passUnretained(event)
                }
                let service = Unmanaged<CGEventTapHotkeyService>.fromOpaque(userInfo).takeUnretainedValue()
                service.handleEvent(type: type, event: event)
                return Unmanaged.passUnretained(event)
            },
            userInfo: refcon
        ) else {
            return false
        }

        self.tap = tap

        // Create run loop source
        runLoopSource = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        if let source = runLoopSource {
            CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        }

        // Enable the tap
        CGEvent.tapEnable(tap: tap, enable: true)

        return true
    }

    private func destroyEventTap() {
        if let tap = tap {
            CGEvent.tapEnable(tap: tap, enable: false)
        }
        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes)
        }
        tap = nil
        runLoopSource = nil
    }

    // MARK: - Event Handling

    private func handleEvent(type: CGEventType, event: CGEvent) {
        // Handle tap disabled events
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            if let tap = tap {
                print("[CGEventTapHotkeyService] Tap was disabled, re-enabling...")
                CGEvent.tapEnable(tap: tap, enable: true)
            }
            return
        }

        let keyCode = Int(event.getIntegerValueField(.keyboardEventKeycode))
        let mods = modifierMask(from: event.flags)

        // Ignore auto-repeat key events
        let isAutoRepeat = event.getIntegerValueField(.keyboardEventAutorepeat) != 0
        if isAutoRepeat && type == .keyDown {
            return
        }

        if isModifiersOnly {
            handleModifiersOnlyEvent(type: type, keyCode: keyCode, mods: mods)
        } else {
            handleNormalShortcutEvent(type: type, keyCode: keyCode, mods: mods)
        }
    }

    /// Handle normal shortcut (keyCode + modifiers, possibly including Fn)
    private func handleNormalShortcutEvent(type: CGEventType, keyCode: Int, mods: Int) {
        switch type {
        case .keyDown:
            // Check if this matches our registered shortcut
            if keyCode == storedKeyCode && mods == storedModifiers {
                if !hasTriggeredKeyDown {
                    hasTriggeredKeyDown = true
                    print("[CGEventTapHotkeyService] ⬇️  Key DOWN matched")
                    loggingService.info(.Hotkey, LogEvent.Hotkey.keyDown, data: [:])
                    onKeyDown?()
                }
            }

        case .keyUp:
            // Check if this matches our registered shortcut
            if keyCode == storedKeyCode && hasTriggeredKeyDown {
                hasTriggeredKeyDown = false
                print("[CGEventTapHotkeyService] ⬆️  Key UP matched")
                loggingService.info(.Hotkey, LogEvent.Hotkey.keyUp, data: [:])
                onKeyUp?()
            }

        case .flagsChanged:
            // For normal shortcuts, flagsChanged doesn't trigger unless the key is released
            // If modifiers are released while key is held, treat as key up
            if hasTriggeredKeyDown && mods != storedModifiers {
                // Modifiers changed while we had triggered - act like key up
                hasTriggeredKeyDown = false
                print("[CGEventTapHotkeyService] ⬆️  Modifiers released (treated as key up)")
                loggingService.info(.Hotkey, LogEvent.Hotkey.keyUp, data: [:])
                onKeyUp?()
            }

        default:
            break
        }
    }

    /// Handle modifiers-only shortcut with "tap-only" semantics.
    /// Triggers only when all modifiers are released AND no non-modifier key was pressed.
    /// This prevents triggering when the user does ⌘+C (they pressed ⌘, then C).
    private func handleModifiersOnlyEvent(type: CGEventType, keyCode: Int, mods: Int) {
        switch type {
        case .flagsChanged:
            if mods == storedModifiers && !candidateActive {
                // User pressed the exact modifier combination we want
                candidateActive = true
                candidateCancelledByKey = false
                print("[CGEventTapHotkeyService] Modifiers-only candidate active: \(mods)")
            } else if candidateActive && mods != storedModifiers {
                // Modifiers changed from our target state
                if mods == 0 {
                    // All modifiers released
                    if !candidateCancelledByKey {
                        // Trigger! User pressed and released modifiers without any other key
                        print("[CGEventTapHotkeyService] ⬇️⬆️  Modifiers-only triggered")
                        loggingService.info(.Hotkey, LogEvent.Hotkey.keyDown, data: [
                            "type": AnyCodable("modifiersOnly")
                        ])
                        onKeyDown?()
                        // For modifiers-only, keyUp follows immediately
                        loggingService.info(.Hotkey, LogEvent.Hotkey.keyUp, data: [
                            "type": AnyCodable("modifiersOnly")
                        ])
                        onKeyUp?()
                    } else {
                        print("[CGEventTapHotkeyService] Modifiers-only cancelled (key was pressed)")
                    }
                    candidateActive = false
                } else {
                    // Modifiers changed to something else (subset or superset)
                    // Keep candidate active if they're adding more modifiers
                    // Cancel if they're releasing some but not all
                    if (mods & storedModifiers) != storedModifiers {
                        // Some of our required modifiers were released
                        candidateActive = false
                        print("[CGEventTapHotkeyService] Modifiers-only candidate cancelled (partial release)")
                    }
                }
            }

        case .keyDown:
            // A non-modifier key was pressed while candidate is active - cancel the trigger
            if candidateActive && !isModifierKeyCode(keyCode) {
                candidateCancelledByKey = true
                print("[CGEventTapHotkeyService] Modifiers-only cancelled by keyDown: \(keyCode)")
            }

        case .keyUp:
            // Key releases don't affect the cancelled state
            break

        default:
            break
        }
    }

    private func resetState() {
        candidateActive = false
        candidateCancelledByKey = false
        hasTriggeredKeyDown = false
    }

    deinit {
        unregister()
    }
}
