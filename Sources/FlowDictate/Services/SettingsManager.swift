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
    @AppStorage("whisperModel") var whisperModel: WhisperModel = .base
    @AppStorage("hotkeyMode") var hotkeyMode: HotkeyMode = .pushToTalk
    @AppStorage("showOverlay") var showOverlay: Bool = true
    @AppStorage("autoPaste") var autoPaste: Bool = true
    @AppStorage("autoSelectModel") var autoSelectModel: Bool = true  // Auto-select best model for language

    // Hotkey settings (stored as separate components)
    // Default: Control+Shift+Space (avoids conflicts with common shortcuts)
    @AppStorage("hotkeyKeyCode") var hotkeyKeyCode: Int = kVK_Space
    @AppStorage("hotkeyModifiers") var hotkeyModifiers: Int = Int(controlKey) | Int(shiftKey)

    // MARK: - Computed Properties

    /// Human-readable hotkey description
    var hotkeyDescription: String {
        return Self.describeHotkey(keyCode: hotkeyKeyCode, modifiers: hotkeyModifiers)
    }

    /// Static helper to describe a hotkey combination
    /// Supports Fn modifier and modifiers-only shortcuts (keyCode == kKeyCodeModifiersOnly)
    static func describeHotkey(keyCode: Int, modifiers: Int) -> String {
        return describeShortcut(keyCode: keyCode, modifiers: modifiers)
    }

    // MARK: - Initialization

    private init() {}

    // MARK: - Methods

    /// Reset all settings to defaults
    func resetToDefaults() {
        language = .english
        backend = .local
        whisperModel = .base
        hotkeyMode = .pushToTalk
        showOverlay = true
        autoPaste = true
        autoSelectModel = true
        hotkeyKeyCode = kVK_Space
        hotkeyModifiers = Int(controlKey) | Int(shiftKey)
    }

    /// Returns the effective model to use, considering auto-selection
    var effectiveModel: WhisperModel {
        if autoSelectModel {
            return WhisperModel.recommendedModel(for: language)
        }
        return whisperModel
    }

    /// Check if current model is appropriate for selected language
    var hasModelLanguageMismatch: Bool {
        // English-only model with non-English language
        if whisperModel.isEnglishOnly && language == .swedish {
            return true
        }
        // Swedish language with non-Swedish-optimized model (warn but allow)
        if language == .swedish && !whisperModel.isSwedishOptimized && !autoSelectModel {
            return true
        }
        return false
    }
}
