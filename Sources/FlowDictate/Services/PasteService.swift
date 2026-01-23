import Foundation
import AppKit
import ApplicationServices
import os.log

/// Service for pasting transcribed text into the active application
/// Uses clipboard + simulated Cmd+V for universal compatibility
final class PasteService {
    // MARK: - Private State

    private let logger = Logger(subsystem: "com.flowdictate", category: "Paste")

    // MARK: - Public Methods

    /// Paste text into the currently active application
    /// - Parameter text: The text to paste
    /// - Throws: DictationError if paste fails
    func paste(text: String) async throws {
        guard !text.isEmpty else { return }

        // Copy to clipboard
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)

        logger.info("Text copied to clipboard (\(text.count) characters)")

        // Check accessibility permission
        let trusted = AXIsProcessTrusted()
        if !trusted {
            logger.warning("Accessibility permission not granted. Text copied to clipboard only.")

            // Request permission (shows system dialog)
            let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
            AXIsProcessTrustedWithOptions(options)

            throw DictationError.accessibilityPermissionDenied
        }

        // Simulate Cmd+V
        try simulatePaste()

        logger.info("Paste simulated successfully")
    }

    // MARK: - Private Methods

    /// Simulate Cmd+V keystroke
    private func simulatePaste() throws {
        // Create key down event for 'V' with Cmd modifier
        guard let keyDownEvent = CGEvent(keyboardEventSource: nil, virtualKey: 0x09, keyDown: true) else {
            throw DictationError.transcriptionFailed("Failed to create key event")
        }
        keyDownEvent.flags = .maskCommand

        // Create key up event
        guard let keyUpEvent = CGEvent(keyboardEventSource: nil, virtualKey: 0x09, keyDown: false) else {
            throw DictationError.transcriptionFailed("Failed to create key event")
        }
        keyUpEvent.flags = .maskCommand

        // Post events
        keyDownEvent.post(tap: .cghidEventTap)
        keyUpEvent.post(tap: .cghidEventTap)
    }
}
