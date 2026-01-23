import Foundation

/// Supported transcription languages
enum Language: String, CaseIterable, Identifiable, Codable {
    case english = "en"
    case swedish = "sv"
    case auto = "auto"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .english: return "English"
        case .swedish: return "Swedish"
        case .auto: return "Auto-detect"
        }
    }

    /// WhisperKit language code (nil for auto-detect)
    var whisperCode: String? {
        switch self {
        case .english: return "en"
        case .swedish: return "sv"
        case .auto: return nil
        }
    }
}

/// Transcription backend options
enum TranscriptionBackend: String, CaseIterable, Identifiable, Codable {
    case local = "local"
    case remote = "remote"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .local: return "Local (WhisperKit)"
        case .remote: return "Remote (OpenAI)"
        }
    }

    var description: String {
        switch self {
        case .local: return "Transcription happens entirely on your device. No internet required."
        case .remote: return "Audio is sent to OpenAI for transcription. Requires API key."
        }
    }
}

/// Hotkey activation mode
enum HotkeyMode: String, CaseIterable, Identifiable, Codable {
    case pushToTalk = "push"
    case toggle = "toggle"

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .pushToTalk: return "Push-to-talk"
        case .toggle: return "Toggle"
        }
    }

    var description: String {
        switch self {
        case .pushToTalk: return "Hold the hotkey to record, release to transcribe"
        case .toggle: return "Press once to start, press again to stop"
        }
    }
}
