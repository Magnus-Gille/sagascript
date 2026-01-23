import Foundation

/// WhisperKit model variants
/// Smaller models are faster but less accurate; larger models are more accurate but slower
enum WhisperModel: String, CaseIterable, Identifiable, Codable {
    case tinyEn = "tiny.en"     // 39M params, English only, fastest
    case tiny = "tiny"          // 39M params, multilingual
    case baseEn = "base.en"     // 74M params, English only
    case base = "base"          // 74M params, multilingual

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .tinyEn: return "Tiny (English)"
        case .tiny: return "Tiny (Multilingual)"
        case .baseEn: return "Base (English)"
        case .base: return "Base (Multilingual)"
        }
    }

    var description: String {
        switch self {
        case .tinyEn: return "Fastest, English only (~0.3s for 5s audio)"
        case .tiny: return "Fast, supports all languages"
        case .baseEn: return "Balanced, English only"
        case .base: return "Balanced, supports all languages"
        }
    }

    /// Whether this model only supports English
    var isEnglishOnly: Bool {
        switch self {
        case .tinyEn, .baseEn: return true
        case .tiny, .base: return false
        }
    }

    /// Model size in millions of parameters
    var parameterCount: Int {
        switch self {
        case .tinyEn, .tiny: return 39
        case .baseEn, .base: return 74
        }
    }
}

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
