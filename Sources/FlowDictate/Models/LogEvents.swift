import Foundation

// MARK: - Log Level

enum LogLevel: String, Codable {
    case debug
    case info
    case warning
    case error
}

// MARK: - Log Category

enum LogCategory: String, Codable {
    case App
    case Hotkey
    case Audio
    case Transcription
    case Paste
    case Settings
}

// MARK: - Log Entry

struct LogEntry: Codable {
    let ts: String
    let level: LogLevel
    let appSession: String
    let dictationSession: String?
    let category: LogCategory
    let event: String
    let data: [String: AnyCodable]

    init(
        level: LogLevel,
        appSession: String,
        dictationSession: String?,
        category: LogCategory,
        event: String,
        data: [String: AnyCodable] = [:]
    ) {
        self.ts = ISO8601DateFormatter.shared.string(from: Date())
        self.level = level
        self.appSession = appSession
        self.dictationSession = dictationSession
        self.category = category
        self.event = event
        self.data = data
    }
}

// MARK: - ISO8601 Formatter

extension ISO8601DateFormatter {
    static let shared: ISO8601DateFormatter = {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter
    }()
}

// MARK: - Event Names

enum LogEvent {
    // App events
    enum App {
        static let started = "app_started"
        static let ready = "app_ready"
        static let terminated = "app_terminated"
    }

    // Hotkey events
    enum Hotkey {
        static let registered = "hotkey_registered"
        static let unregistered = "hotkey_unregistered"
        static let keyDown = "key_down"
        static let keyUp = "key_up"
    }

    // Audio events
    enum Audio {
        static let captureStarted = "capture_started"
        static let captureStopped = "capture_stopped"
        static let permissionGranted = "permission_granted"
        static let permissionDenied = "permission_denied"
        static let permissionRequested = "permission_requested"
        static let bufferReceived = "buffer_received"
    }

    // Transcription events
    enum Transcription {
        static let started = "transcription_started"
        static let completed = "transcription_completed"
        static let failed = "transcription_failed"
        static let modelLoading = "model_loading"
        static let modelLoaded = "model_loaded"
        static let modelFailed = "model_failed"
    }

    // Paste events
    enum Paste {
        static let attempted = "paste_attempted"
        static let succeeded = "paste_succeeded"
        static let failed = "paste_failed"
        static let permissionDenied = "permission_denied"
    }

    // Settings events
    enum Settings {
        static let changed = "settings_changed"
    }

    // Session events
    enum Session {
        static let dictationStarted = "dictation_session_started"
        static let dictationComplete = "dictation_session_complete"
        static let stateChanged = "state_changed"
    }
}
