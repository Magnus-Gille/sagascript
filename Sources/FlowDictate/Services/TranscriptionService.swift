import Foundation
import os.log

/// Protocol for transcription backends
protocol TranscriptionBackendProtocol {
    /// Transcribe audio to text
    /// - Parameters:
    ///   - audio: Audio samples as Float32 at 16kHz mono
    ///   - language: Target language for transcription
    /// - Returns: Transcribed text
    func transcribe(audio: [Float], language: Language) async throws -> String

    /// Check if the backend is ready for transcription
    var isReady: Bool { get async }

    /// Warm up the backend (e.g., load models)
    func warmUp() async throws
}

/// Service that manages transcription backends
final class TranscriptionService {
    // MARK: - Private State

    private let whisperKitBackend: WhisperKitBackend
    private let openAIBackend: OpenAIBackend
    private let logger = Logger(subsystem: "com.flowdictate", category: "Transcription")

    // MARK: - Initialization

    init() {
        self.whisperKitBackend = WhisperKitBackend()
        self.openAIBackend = OpenAIBackend()
    }

    // MARK: - Public Methods

    /// Transcribe audio to text using the specified backend
    /// - Parameters:
    ///   - audio: Audio samples as Float32 at 16kHz mono
    ///   - language: Target language
    ///   - backend: Which backend to use
    /// - Returns: Transcribed text
    func transcribe(
        audio: [Float],
        language: Language,
        backend: TranscriptionBackend
    ) async throws -> String {
        let startTime = CFAbsoluteTimeGetCurrent()

        let result: String
        switch backend {
        case .local:
            result = try await whisperKitBackend.transcribe(audio: audio, language: language)
        case .remote:
            result = try await openAIBackend.transcribe(audio: audio, language: language)
        }

        let elapsed = CFAbsoluteTimeGetCurrent() - startTime
        logger.info("Transcription completed in \(elapsed, format: .fixed(precision: 2))s using \(backend.rawValue)")

        return result
    }

    /// Warm up the local transcription backend
    func warmUp() async throws {
        try await whisperKitBackend.warmUp()
    }

    /// Check if the local backend is ready
    var isLocalReady: Bool {
        get async { await whisperKitBackend.isReady }
    }
}
