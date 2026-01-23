import SwiftUI
import AppKit
import Carbon.HIToolbox

/// A view that captures keyboard shortcuts for hotkey configuration
/// Click to start recording, press desired key combination, press Escape to cancel
struct HotkeyRecorderView: View {
    @Binding var keyCode: Int
    @Binding var modifiers: Int
    var onHotkeyChanged: (() -> Void)?

    @State private var isRecording = false
    @State private var tempKeyCode: Int?
    @State private var tempModifiers: Int = 0

    var body: some View {
        HStack {
            Text("Hotkey:")

            Spacer()

            Button(action: {
                isRecording = true
                tempKeyCode = nil
                tempModifiers = 0
            }) {
                HStack(spacing: 4) {
                    if isRecording {
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
                .background(isRecording ? Color.accentColor.opacity(0.2) : Color.secondary.opacity(0.2))
                .cornerRadius(6)
                .overlay(
                    RoundedRectangle(cornerRadius: 6)
                        .stroke(isRecording ? Color.accentColor : Color.clear, lineWidth: 2)
                )
            }
            .buttonStyle(.plain)
            .background(
                HotkeyRecorderNSViewRepresentable(
                    isRecording: $isRecording,
                    onKeyEvent: handleKeyEvent
                )
                .frame(width: 0, height: 0)
            )
        }
    }

    private func handleKeyEvent(_ event: NSEvent) {
        guard isRecording else { return }

        // Escape cancels recording
        if event.keyCode == UInt16(kVK_Escape) {
            isRecording = false
            return
        }

        // Convert NSEvent modifiers to Carbon modifiers
        var carbonMods = 0
        if event.modifierFlags.contains(.command) { carbonMods |= Int(cmdKey) }
        if event.modifierFlags.contains(.shift) { carbonMods |= Int(shiftKey) }
        if event.modifierFlags.contains(.option) { carbonMods |= Int(optionKey) }
        if event.modifierFlags.contains(.control) { carbonMods |= Int(controlKey) }

        // Require at least one modifier for most keys
        let keyCodeInt = Int(event.keyCode)
        let isModifierOnlyKey = [kVK_Shift, kVK_RightShift, kVK_Command, kVK_RightCommand,
                                  kVK_Option, kVK_RightOption, kVK_Control, kVK_RightControl]
            .contains(keyCodeInt)

        // Skip if only modifier key pressed
        if isModifierOnlyKey {
            return
        }

        // Require at least one modifier for safety (except F-keys)
        let isFunctionKey = (kVK_F1...kVK_F12).contains(keyCodeInt) ||
                           [kVK_F13, kVK_F14, kVK_F15, kVK_F16, kVK_F17, kVK_F18, kVK_F19, kVK_F20].contains(keyCodeInt)
        if carbonMods == 0 && !isFunctionKey {
            // Need at least one modifier for non-function keys
            return
        }

        // Accept the hotkey
        keyCode = keyCodeInt
        modifiers = carbonMods
        isRecording = false
        onHotkeyChanged?()
    }
}

/// NSViewRepresentable to capture key events when recording
private struct HotkeyRecorderNSViewRepresentable: NSViewRepresentable {
    @Binding var isRecording: Bool
    var onKeyEvent: (NSEvent) -> Void

    func makeNSView(context: Context) -> HotkeyRecorderNSView {
        let view = HotkeyRecorderNSView()
        view.onKeyEvent = onKeyEvent
        view.isRecordingBinding = { isRecording }
        return view
    }

    func updateNSView(_ nsView: HotkeyRecorderNSView, context: Context) {
        nsView.onKeyEvent = onKeyEvent
        nsView.isRecordingBinding = { isRecording }

        if isRecording {
            DispatchQueue.main.async {
                nsView.window?.makeFirstResponder(nsView)
            }
        }
    }
}

/// Custom NSView that captures key events for hotkey recording
private class HotkeyRecorderNSView: NSView {
    var onKeyEvent: ((NSEvent) -> Void)?
    var isRecordingBinding: (() -> Bool)?

    override var acceptsFirstResponder: Bool { true }

    override func keyDown(with event: NSEvent) {
        if isRecordingBinding?() == true {
            onKeyEvent?(event)
        } else {
            super.keyDown(with: event)
        }
    }

    override func flagsChanged(with event: NSEvent) {
        // We don't handle modifier-only presses as complete hotkeys
        super.flagsChanged(with: event)
    }
}
