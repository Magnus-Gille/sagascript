import Foundation
import AppKit
import ApplicationServices
import os.log

/// Service for pasting transcribed text into the active application
/// Uses clipboard + simulated Cmd+V for universal compatibility
/// Saves and restores previous clipboard contents after pasting
final class PasteService {
    // MARK: - Private State

    private let logger = Logger(subsystem: "com.flowdictate", category: "Paste")
    private let loggingService = LoggingService.shared

    /// Saved clipboard contents for restoration after paste
    private var savedClipboard: [NSPasteboard.PasteboardType: Data]?

    /// Delay before restoring clipboard (allows paste to complete)
    private let clipboardRestoreDelay: TimeInterval = 0.1

    // MARK: - Public Methods

    /// Paste text into the currently active application
    /// - Parameter text: The text to paste
    /// - Throws: DictationError if paste fails
    func paste(text: String) async throws {
        guard !text.isEmpty else { return }

        loggingService.info(.Paste, LogEvent.Paste.attempted, data: [
            "characters": AnyCodable(text.count)
        ])

        // Save current clipboard contents before overwriting
        let pasteboard = NSPasteboard.general
        saveClipboard(pasteboard)

        // Copy transcription to clipboard
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)

        logger.info("Text copied to clipboard (\(text.count) characters)")

        // Check accessibility permission
        let trusted = AXIsProcessTrusted()
        if !trusted {
            logger.warning("Accessibility permission not granted. Text copied to clipboard only.")

            loggingService.warning(.Paste, LogEvent.Paste.permissionDenied, data: [
                "reason": AnyCodable("accessibility_not_trusted")
            ])

            // Request permission (shows system dialog)
            let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
            AXIsProcessTrustedWithOptions(options)

            throw DictationError.accessibilityPermissionDenied
        }

        // Simulate Cmd+V
        do {
            try simulatePaste()
            logger.info("Paste simulated successfully")

            loggingService.info(.Paste, LogEvent.Paste.succeeded, data: [
                "characters": AnyCodable(text.count)
            ])

            // Restore clipboard after a short delay to allow paste to complete
            scheduleClipboardRestore()
        } catch {
            loggingService.error(.Paste, LogEvent.Paste.failed, data: [
                "error": AnyCodable(error.localizedDescription)
            ])
            // Still try to restore clipboard on failure
            scheduleClipboardRestore()
            throw error
        }
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

    /// Save all clipboard contents for later restoration
    private func saveClipboard(_ pasteboard: NSPasteboard) {
        var saved: [NSPasteboard.PasteboardType: Data] = [:]

        // Get all available types from the clipboard
        guard let types = pasteboard.types else {
            savedClipboard = nil
            return
        }

        // Save data for each type
        for type in types {
            if let data = pasteboard.data(forType: type) {
                saved[type] = data
            }
        }

        savedClipboard = saved.isEmpty ? nil : saved
        if savedClipboard != nil {
            logger.debug("Saved \(saved.count) clipboard types for restoration")
        }
    }

    /// Schedule clipboard restoration after a delay
    private func scheduleClipboardRestore() {
        guard savedClipboard != nil else { return }

        DispatchQueue.main.asyncAfter(deadline: .now() + clipboardRestoreDelay) { [weak self] in
            self?.restoreClipboard()
        }
    }

    /// Restore previously saved clipboard contents
    private func restoreClipboard() {
        guard let saved = savedClipboard else { return }

        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()

        // Restore all saved types
        for (type, data) in saved {
            pasteboard.setData(data, forType: type)
        }

        savedClipboard = nil
        logger.debug("Restored \(saved.count) clipboard types")
    }
}
