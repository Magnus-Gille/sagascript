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
    @AppStorage("launchAtLogin") var launchAtLogin: Bool = false

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
    static func describeHotkey(keyCode: Int, modifiers: Int) -> String {
        var parts: [String] = []

        if modifiers & Int(controlKey) != 0 { parts.append("⌃") }
        if modifiers & Int(optionKey) != 0 { parts.append("⌥") }
        if modifiers & Int(shiftKey) != 0 { parts.append("⇧") }
        if modifiers & Int(cmdKey) != 0 { parts.append("⌘") }

        // Add key name using lookup table
        parts.append(keyCodeToName(keyCode))

        return parts.joined(separator: "")
    }

    /// Convert a key code to a human-readable name
    private static func keyCodeToName(_ keyCode: Int) -> String {
        switch keyCode {
        case kVK_Space: return "Space"
        case kVK_Return: return "Return"
        case kVK_Tab: return "Tab"
        case kVK_Delete: return "Delete"
        case kVK_Escape: return "Esc"
        case kVK_F1: return "F1"
        case kVK_F2: return "F2"
        case kVK_F3: return "F3"
        case kVK_F4: return "F4"
        case kVK_F5: return "F5"
        case kVK_F6: return "F6"
        case kVK_F7: return "F7"
        case kVK_F8: return "F8"
        case kVK_F9: return "F9"
        case kVK_F10: return "F10"
        case kVK_F11: return "F11"
        case kVK_F12: return "F12"
        case kVK_ANSI_A: return "A"
        case kVK_ANSI_B: return "B"
        case kVK_ANSI_C: return "C"
        case kVK_ANSI_D: return "D"
        case kVK_ANSI_E: return "E"
        case kVK_ANSI_F: return "F"
        case kVK_ANSI_G: return "G"
        case kVK_ANSI_H: return "H"
        case kVK_ANSI_I: return "I"
        case kVK_ANSI_J: return "J"
        case kVK_ANSI_K: return "K"
        case kVK_ANSI_L: return "L"
        case kVK_ANSI_M: return "M"
        case kVK_ANSI_N: return "N"
        case kVK_ANSI_O: return "O"
        case kVK_ANSI_P: return "P"
        case kVK_ANSI_Q: return "Q"
        case kVK_ANSI_R: return "R"
        case kVK_ANSI_S: return "S"
        case kVK_ANSI_T: return "T"
        case kVK_ANSI_U: return "U"
        case kVK_ANSI_V: return "V"
        case kVK_ANSI_W: return "W"
        case kVK_ANSI_X: return "X"
        case kVK_ANSI_Y: return "Y"
        case kVK_ANSI_Z: return "Z"
        case kVK_ANSI_0: return "0"
        case kVK_ANSI_1: return "1"
        case kVK_ANSI_2: return "2"
        case kVK_ANSI_3: return "3"
        case kVK_ANSI_4: return "4"
        case kVK_ANSI_5: return "5"
        case kVK_ANSI_6: return "6"
        case kVK_ANSI_7: return "7"
        case kVK_ANSI_8: return "8"
        case kVK_ANSI_9: return "9"
        default: return "Key\(keyCode)"
        }
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
