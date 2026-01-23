import Foundation
import SwiftUI
import Carbon.HIToolbox

/// Manages user preferences with persistence via UserDefaults
@MainActor
final class SettingsManager: ObservableObject {
    static let shared = SettingsManager()

    // MARK: - Published Settings

    @AppStorage("language") var language: Language = .english
    @AppStorage("backend") var backend: TranscriptionBackend = .local
    @AppStorage("hotkeyMode") var hotkeyMode: HotkeyMode = .pushToTalk
    @AppStorage("showOverlay") var showOverlay: Bool = true

    // Hotkey settings (stored as separate components)
    @AppStorage("hotkeyKeyCode") var hotkeyKeyCode: Int = kVK_Space
    @AppStorage("hotkeyModifiers") var hotkeyModifiers: Int = Int(optionKey)

    // MARK: - Computed Properties

    /// Human-readable hotkey description
    var hotkeyDescription: String {
        var parts: [String] = []

        if hotkeyModifiers & Int(cmdKey) != 0 { parts.append("⌘") }
        if hotkeyModifiers & Int(shiftKey) != 0 { parts.append("⇧") }
        if hotkeyModifiers & Int(optionKey) != 0 { parts.append("⌥") }
        if hotkeyModifiers & Int(controlKey) != 0 { parts.append("⌃") }

        // Add key name
        if hotkeyKeyCode == kVK_Space {
            parts.append("Space")
        } else {
            // For other keys, we'd need a lookup table
            parts.append("Key \(hotkeyKeyCode)")
        }

        return parts.joined(separator: "")
    }

    // MARK: - Initialization

    private init() {}

    // MARK: - Methods

    /// Reset all settings to defaults
    func resetToDefaults() {
        language = .english
        backend = .local
        hotkeyMode = .pushToTalk
        showOverlay = true
        hotkeyKeyCode = kVK_Space
        hotkeyModifiers = Int(optionKey)
    }
}
