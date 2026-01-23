import Foundation

/// WhisperKit model variants
/// Smaller models are faster but less accurate; larger models are more accurate but slower
enum WhisperModel: String, CaseIterable, Identifiable, Codable {
    // Standard OpenAI Whisper models (via argmaxinc/whisperkit-coreml)
    case tinyEn = "tiny.en"     // 39M params, English only, fastest
    case tiny = "tiny"          // 39M params, multilingual
    case baseEn = "base.en"     // 74M params, English only
    case base = "base"          // 74M params, multilingual

    // KB-Whisper Swedish-optimized models (loaded from local path)
    // These models are fine-tuned on 50,000+ hours of Swedish speech data
    // and achieve 4x better WER on Swedish compared to standard Whisper
    // Models must be converted to CoreML and placed in ~/Library/Application Support/FlowDictate/Models/
    case kbWhisperTiny = "kb-whisper-tiny"     // 57.7M params, Swedish optimized, 13.2% WER
    case kbWhisperBase = "kb-whisper-base"     // 99.1M params, Swedish optimized, 9.1% WER
    case kbWhisperSmall = "kb-whisper-small"   // 0.3B params, Swedish optimized, 7.3% WER

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .tinyEn: return "Tiny (English)"
        case .tiny: return "Tiny (Multilingual)"
        case .baseEn: return "Base (English)"
        case .base: return "Base (Multilingual)"
        case .kbWhisperTiny: return "KB-Whisper Tiny (Swedish)"
        case .kbWhisperBase: return "KB-Whisper Base (Swedish)"
        case .kbWhisperSmall: return "KB-Whisper Small (Swedish)"
        }
    }

    var description: String {
        switch self {
        case .tinyEn: return "Fastest, English only (~0.3s for 5s audio)"
        case .tiny: return "Fast, supports all languages"
        case .baseEn: return "Balanced, English only"
        case .base: return "Balanced, supports all languages"
        case .kbWhisperTiny: return "Fast, Swedish optimized (13% WER)"
        case .kbWhisperBase: return "Balanced, Swedish optimized (9% WER)"
        case .kbWhisperSmall: return "Accurate, Swedish optimized (7% WER)"
        }
    }

    /// Whether this model only supports English
    var isEnglishOnly: Bool {
        switch self {
        case .tinyEn, .baseEn: return true
        case .tiny, .base, .kbWhisperTiny, .kbWhisperBase, .kbWhisperSmall: return false
        }
    }

    /// Whether this model is optimized for Swedish
    var isSwedishOptimized: Bool {
        switch self {
        case .kbWhisperTiny, .kbWhisperBase, .kbWhisperSmall: return true
        default: return false
        }
    }

    /// Model size in millions of parameters
    var parameterCount: Int {
        switch self {
        case .tinyEn, .tiny: return 39
        case .baseEn, .base: return 74
        case .kbWhisperTiny: return 58
        case .kbWhisperBase: return 99
        case .kbWhisperSmall: return 300
        }
    }

    /// Whether this model requires a local path (not available via WhisperKit's default repos)
    var requiresLocalPath: Bool {
        isSwedishOptimized
    }

    /// The model folder name for WhisperKit (used for standard models downloaded from HuggingFace)
    var modelName: String {
        switch self {
        case .tinyEn: return "openai_whisper-tiny.en"
        case .tiny: return "openai_whisper-tiny"
        case .baseEn: return "openai_whisper-base.en"
        case .base: return "openai_whisper-base"
        case .kbWhisperTiny: return "kblab_kb-whisper-tiny"
        case .kbWhisperBase: return "kblab_kb-whisper-base"
        case .kbWhisperSmall: return "kblab_kb-whisper-small"
        }
    }

    /// Local directory where custom models are stored
    /// ~/Library/Application Support/FlowDictate/Models/
    static var localModelsDirectory: URL {
        let appSupport = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first!
        return appSupport.appendingPathComponent("FlowDictate/Models", isDirectory: true)
    }

    /// Full local path to this model's folder (for KB-Whisper models)
    var localModelPath: URL? {
        guard requiresLocalPath else { return nil }
        return Self.localModelsDirectory.appendingPathComponent(modelName, isDirectory: true)
    }

    /// Check if this model is available locally
    var isLocallyAvailable: Bool {
        guard requiresLocalPath else { return true } // Standard models are always "available" (downloaded on demand)
        let ggmlPath = Self.localModelsDirectory.appendingPathComponent(ggmlFilename)
        return FileManager.default.fileExists(atPath: ggmlPath.path)
    }

    /// GGML model filename for whisper.cpp (Swedish models only)
    var ggmlFilename: String {
        switch self {
        case .kbWhisperTiny: return "kb-whisper-tiny-q5_0.bin"
        case .kbWhisperBase: return "kb-whisper-base-q5_0.bin"
        case .kbWhisperSmall: return "kb-whisper-small-q5_0.bin"
        default: return ""
        }
    }

    /// Download URL for GGML model (quantized q5_0 for smaller size)
    var ggmlDownloadURL: URL? {
        switch self {
        case .kbWhisperTiny:
            return URL(string: "https://huggingface.co/KBLab/kb-whisper-tiny/resolve/main/ggml-model-q5_0.bin")
        case .kbWhisperBase:
            return URL(string: "https://huggingface.co/KBLab/kb-whisper-base/resolve/main/ggml-model-q5_0.bin")
        case .kbWhisperSmall:
            return URL(string: "https://huggingface.co/KBLab/kb-whisper-small/resolve/main/ggml-model-q5_0.bin")
        default:
            return nil
        }
    }

    /// Approximate download size in MB (quantized q5_0 versions)
    var ggmlSizeMB: Int {
        switch self {
        case .kbWhisperTiny: return 40
        case .kbWhisperBase: return 60
        case .kbWhisperSmall: return 190
        default: return 0
        }
    }

    /// Standard models that work for any language
    static var standardModels: [WhisperModel] {
        [.tinyEn, .tiny, .baseEn, .base]
    }

    /// Swedish-optimized models
    static var swedishModels: [WhisperModel] {
        [.kbWhisperTiny, .kbWhisperBase, .kbWhisperSmall]
    }

    /// Recommended model for a given language
    static func recommendedModel(for language: Language) -> WhisperModel {
        switch language {
        case .english:
            return .baseEn  // English-only model for best English performance
        case .swedish:
            return .kbWhisperBase  // Swedish-optimized model (9.1% WER)
        case .auto:
            return .base  // Multilingual for auto-detect
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
