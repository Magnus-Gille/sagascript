import SwiftUI
import AppKit
import Carbon.HIToolbox

// MARK: - RecorderField (AppKit)

/// NSSearchField that eagerly becomes first responder and forwards key events.
/// Using a real text field mirrors the approach from KeyboardShortcuts and works reliably in Settings windows.
private final class RecorderField: NSSearchField {
    var onKeyEvent: ((UInt16, NSEvent.ModifierFlags) -> Void)?
    var onCancel: (() -> Void)?

    override var acceptsFirstResponder: Bool { true }

    override func becomeFirstResponder() -> Bool {
        let became = super.becomeFirstResponder()
        currentEditor()?.selectedRange = NSRange(location: 0, length: 0)
        return became
    }

    override func keyDown(with event: NSEvent) {
        // Swallow to prevent beeps and other default handling
    }

    override func flagsChanged(with event: NSEvent) {
        // Let the monitor track modifier changes
    }

    override func cancelOperation(_ sender: Any?) {
        onCancel?()
    }
}

// MARK: - RecorderFieldRepresentable

/// SwiftUI wrapper that focuses the field whenever recording starts.
private struct RecorderFieldRepresentable: NSViewRepresentable {
    let isRecording: Bool
    let onKeyEvent: (UInt16, NSEvent.ModifierFlags) -> Void
    let onCancel: () -> Void

    func makeNSView(context: Context) -> RecorderField {
        let field = RecorderField()
        field.onKeyEvent = onKeyEvent
        field.onCancel = onCancel
        field.focusRingType = .none
        field.isBordered = false
        field.isBezeled = false
        field.stringValue = ""
        return field
    }

    func updateNSView(_ nsView: RecorderField, context: Context) {
        nsView.onKeyEvent = onKeyEvent
        nsView.onCancel = onCancel

        guard isRecording else { return }

        DispatchQueue.main.async {
            nsView.window?.makeKeyAndOrderFront(nil)
            if nsView.window?.firstResponder !== nsView {
                nsView.window?.makeFirstResponder(nsView)
            }
        }
    }
}

// MARK: - HotkeyRecorderView

/// A view that captures keyboard shortcuts for hotkey configuration.
/// Supports:
/// - Normal shortcuts (⌘+Z, ⌥+Z, etc.) - accepted on keyDown
/// - Modifier-only shortcuts (⌘ alone, ⌥⌘, etc.) - accepted when all modifiers released
/// - Fn combinations (Fn+Z, Fn alone) - requires CGEventTap
///
/// Click to start recording, press desired key combination, press Escape to cancel.
struct HotkeyRecorderView: View {
    @Binding var keyCode: Int
    @Binding var modifiers: Int
    var onHotkeyChanged: (() -> Void)?

    @EnvironmentObject private var appController: AppController
    @StateObject private var recorder = HotkeyRecorder()

    var body: some View {
        HStack {
            Text("Hotkey:")

            Spacer()

            ZStack {
                // Hidden recorder field - real text input so it reliably receives key events
                RecorderFieldRepresentable(
                    isRecording: recorder.isRecording,
                    onKeyEvent: { _, _ in
                        // Events are now handled via the recorder's event tap/monitor
                    },
                    onCancel: {
                        recorder.cancelRecording()
                    }
                )
                .frame(width: 1, height: 1)
                .opacity(0.01)

                // Visible button for user interaction
                Button(action: {
                    if recorder.isRecording {
                        recorder.cancelRecording()
                    } else {
                        recorder.startRecording { newKeyCode, newModifiers in
                            self.keyCode = newKeyCode
                            self.modifiers = newModifiers
                            onHotkeyChanged?()
                        }
                    }
                }) {
                    HStack(spacing: 4) {
                        if recorder.isRecording {
                            Text("Press a key...")
                                .foregroundColor(.secondary)
                        } else {
                            Text(SettingsManager.describeHotkey(keyCode: keyCode, modifiers: modifiers))
                        }
                    }
                    .font(.system(.body, design: .monospaced))
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .frame(minWidth: 140)
                    .background(recorder.isRecording ? Color.accentColor.opacity(0.2) : Color.secondary.opacity(0.2))
                    .cornerRadius(6)
                    .overlay(
                        RoundedRectangle(cornerRadius: 6)
                            .stroke(recorder.isRecording ? Color.accentColor : Color.clear, lineWidth: 2)
                    )
                }
                .buttonStyle(.plain)
            }
        }
        .onAppear {
            recorder.onRecordingBegan = { [weak appController] in
                appController?.suspendHotkey()
            }
            recorder.onRecordingEnded = { [weak appController] in
                appController?.resumeHotkey()
            }
        }
        .onDisappear {
            recorder.cancelRecording()
        }
    }
}

// MARK: - Hotkey Recorder Class

/// Class-based coordinator for hotkey recording with improved algorithm:
/// - Normal keys (non-modifier): Accept immediately on keyDown
/// - Modifier-only: Accept when all modifiers released (to not break combos like ⌘+Z)
/// - Escape: Cancel recording
class HotkeyRecorder: ObservableObject {
    @Published var isRecording = false

    var onRecordingBegan: (() -> Void)?
    var onRecordingEnded: (() -> Void)?

    // Recording state
    private var collectedModsMask: Int = 0
    private var sawNonModifierKey: Bool = false
    private var completionHandler: ((Int, Int) -> Void)?

    // Event handling
    private var eventTap: RecordingEventTap?
    private var localMonitor: Any?

    func startRecording(onComplete: @escaping (Int, Int) -> Void) {
        guard !isRecording else { return }

        // Reset state
        collectedModsMask = 0
        sawNonModifierKey = false
        completionHandler = onComplete

        isRecording = true
        onRecordingBegan?()

        NSApp?.activate(ignoringOtherApps: true)

        // Start event tap for reliable Fn/modifier capture
        eventTap = RecordingEventTap()
        let tapStarted = eventTap?.start { [weak self] type, event in
            self?.handleCGEvent(type: type, event: event)
        } ?? false

        if !tapStarted {
            // Fall back to local NSEvent monitor
            print("[HotkeyRecorder] CGEventTap unavailable, using NSEvent monitor")
            localMonitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .keyUp, .flagsChanged]) { [weak self] event in
                self?.handleNSEvent(event)
                return nil // Swallow all events while recording
            }
        }
    }

    func cancelRecording() {
        guard isRecording else { return }
        stopRecordingInternal()
    }

    private func acceptShortcut(keyCode: Int, modifiers: Int) {
        guard isRecording else { return }

        // Defensive: If keyCode is a modifier key, treat as modifiers-only shortcut
        var finalKeyCode = keyCode
        if isModifierKeyCode(keyCode) {
            finalKeyCode = kKeyCodeModifiersOnly
        }

        let handler = completionHandler
        stopRecordingInternal()

        print("[HotkeyRecorder] Accepted shortcut: keyCode=\(finalKeyCode), modifiers=\(modifiers)")
        handler?(finalKeyCode, modifiers)
    }

    private func stopRecordingInternal() {
        eventTap?.stop()
        eventTap = nil

        if let monitor = localMonitor {
            NSEvent.removeMonitor(monitor)
            localMonitor = nil
        }

        collectedModsMask = 0
        sawNonModifierKey = false
        completionHandler = nil

        isRecording = false
        onRecordingEnded?()
    }

    // MARK: - CGEvent Handling

    private func handleCGEvent(type: CGEventType, event: CGEvent) {
        let keyCode = Int(event.getIntegerValueField(.keyboardEventKeycode))
        let mods = modifierMask(from: event.flags)

        switch type {
        case .keyDown:
            handleKeyDown(keyCode: keyCode, mods: mods)

        case .flagsChanged:
            handleFlagsChanged(keyCode: keyCode, mods: mods)

        case .keyUp:
            // keyUp is not used for acceptance in the new algorithm
            break

        default:
            break
        }
    }

    // MARK: - NSEvent Handling (fallback)

    private func handleNSEvent(_ event: NSEvent) {
        let keyCode = Int(event.keyCode)
        let flags = event.modifierFlags.intersection([.command, .shift, .control, .option, .function])
        let mods = modifierMask(from: flags)

        switch event.type {
        case .keyDown:
            handleKeyDown(keyCode: keyCode, mods: mods)

        case .flagsChanged:
            handleFlagsChanged(keyCode: keyCode, mods: mods)

        case .keyUp:
            break

        default:
            break
        }
    }

    // MARK: - Unified Event Handling

    private func handleKeyDown(keyCode: Int, mods: Int) {
        // Escape cancels recording
        if keyCode == kVK_Escape {
            print("[HotkeyRecorder] Cancelled by Escape")
            cancelRecording()
            return
        }

        // Check if this is a non-modifier key
        if !isModifierKeyCode(keyCode) {
            // Accept immediately on keyDown for normal keys
            sawNonModifierKey = true
            acceptShortcut(keyCode: keyCode, modifiers: mods)
        }
    }

    private func handleFlagsChanged(keyCode: Int, mods: Int) {
        // Track all modifiers pressed during this session
        if !sawNonModifierKey {
            collectedModsMask |= mods
        }

        // Detect "modifiers-only" completion:
        // When all modifiers are released (mods == 0) AND we collected some modifiers
        // AND no non-modifier key was pressed during this recording session
        if mods == 0 && collectedModsMask != 0 && !sawNonModifierKey {
            acceptShortcut(keyCode: kKeyCodeModifiersOnly, modifiers: collectedModsMask)
        }
    }

    func stopRecording() {
        cancelRecording()
    }

    deinit {
        stopRecordingInternal()
    }
}

// MARK: - Key Event Processing (Testable - Legacy Interface)

/// Result of processing a key event
enum KeyEventResult: Equatable {
    case ignored           // Not recording
    case cancelled         // Escape pressed
    case needsModifier     // Legacy: not used anymore
    case accepted(keyCode: Int, modifiers: Int)
}

/// Process a key event and determine what action to take
/// This is the legacy interface for backwards compatibility with tests
func processKeyEvent(
    keyCode: UInt16,
    modifierFlags: NSEvent.ModifierFlags,
    isRecording: Bool
) -> KeyEventResult {
    guard isRecording else { return .ignored }

    // Escape cancels recording
    if keyCode == UInt16(kVK_Escape) {
        return .cancelled
    }

    // Convert NSEvent modifiers to our stored format
    let mods = modifierMask(from: modifierFlags)
    let keyCodeInt = Int(keyCode)

    // If modifiers are empty and this is a modifier key, derive the modifier from the key code
    var effectiveMods = mods
    if effectiveMods == 0 {
        effectiveMods = modifierBitForKeyCode(keyCodeInt)
    }

    return .accepted(keyCode: keyCodeInt, modifiers: effectiveMods)
}

// MARK: - CGEvent Tap Helper for Recording

/// CGEvent tap manager specifically for the recording session.
/// Separate from the global hotkey tap to allow independent start/stop.
private final class RecordingEventTap {
    private var tap: CFMachPort?
    private var runLoopSource: CFRunLoopSource?
    private var eventHandler: ((CGEventType, CGEvent) -> Void)?

    func start(onEvent: @escaping (CGEventType, CGEvent) -> Void) -> Bool {
        stop()
        self.eventHandler = onEvent

        // Check permission first
        if !CGPreflightListenEventAccess() {
            // Try requesting it
            if !CGRequestListenEventAccess() {
                return false
            }
        }

        let mask: CGEventMask =
            (1 << CGEventType.keyDown.rawValue)
            | (1 << CGEventType.keyUp.rawValue)
            | (1 << CGEventType.flagsChanged.rawValue)

        let refcon = UnsafeMutableRawPointer(Unmanaged.passUnretained(self).toOpaque())

        guard let tap = CGEvent.tapCreate(
            tap: .cgSessionEventTap,
            place: .headInsertEventTap,
            options: .listenOnly,
            eventsOfInterest: mask,
            callback: { _, type, event, userInfo in
                guard let userInfo = userInfo else { return Unmanaged.passUnretained(event) }

                // Handle tap disabled
                if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
                    return Unmanaged.passUnretained(event)
                }

                let capture = Unmanaged<RecordingEventTap>.fromOpaque(userInfo).takeUnretainedValue()
                capture.eventHandler?(type, event)
                return Unmanaged.passUnretained(event)
            },
            userInfo: refcon
        ) else {
            eventHandler = nil
            return false
        }

        self.tap = tap
        runLoopSource = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0)
        if let source = runLoopSource {
            CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
        }
        CGEvent.tapEnable(tap: tap, enable: true)
        return true
    }

    func stop() {
        if let tap {
            CGEvent.tapEnable(tap: tap, enable: false)
        }
        if let source = runLoopSource {
            CFRunLoopRemoveSource(CFRunLoopGetMain(), source, .commonModes)
        }
        tap = nil
        runLoopSource = nil
        eventHandler = nil
    }

    deinit {
        stop()
    }
}
