import Foundation

/// Application state machine
enum AppState: Equatable {
    case idle
    case recording
    case transcribing
    case error(String)

    var isRecording: Bool {
        if case .recording = self { return true }
        return false
    }

    var isTranscribing: Bool {
        if case .transcribing = self { return true }
        return false
    }

    var isBusy: Bool {
        isRecording || isTranscribing
    }
}

/// Errors that can occur during dictation
enum DictationError: LocalizedError {
    case microphonePermissionDenied
    case accessibilityPermissionDenied
    case modelNotLoaded
    case transcriptionFailed(String)
    case noAudioCaptured
    case apiKeyMissing
    case networkError(String)

    var errorDescription: String? {
        switch self {
        case .microphonePermissionDenied:
            return "Microphone permission is required. Please enable it in System Settings > Privacy & Security > Microphone."
        case .accessibilityPermissionDenied:
            return "Accessibility permission is required for automatic paste. Text has been copied to clipboard."
        case .modelNotLoaded:
            return "Transcription model is not loaded. Please wait for initialization."
        case .transcriptionFailed(let message):
            return "Transcription failed: \(message)"
        case .noAudioCaptured:
            return "No audio was captured. Please try again."
        case .apiKeyMissing:
            return "OpenAI API key is not configured. Please add it in Settings."
        case .networkError(let message):
            return "Network error: \(message)"
        }
    }
}
