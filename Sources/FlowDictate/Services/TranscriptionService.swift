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
/// Routes to appropriate backend based on model type:
/// - Standard models (tiny, base, etc.) → WhisperKit (CoreML/ANE)
/// - KB-Whisper Swedish models → whisper.cpp (GGML)
/// - Remote → OpenAI API
final class TranscriptionService {
    // MARK: - Private State

    private let whisperKitBackend: WhisperKitBackend
    private let whisperCppBackend: WhisperCppBackend
    private let openAIBackend: OpenAIBackend
    private let logger = Logger(subsystem: "com.flowdictate", category: "Transcription")
    private let loggingService = LoggingService.shared

    /// Currently loaded model (for routing transcription calls)
    private var currentModel: WhisperModel?

    // MARK: - Initialization

    init() {
        self.whisperKitBackend = WhisperKitBackend()
        self.whisperCppBackend = WhisperCppBackend()
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

        // Determine actual backend based on model type for local transcription
        let actualBackend: String
        if backend == .local, let model = currentModel {
            actualBackend = model.isSwedishOptimized ? "whisper.cpp" : "WhisperKit"
        } else {
            actualBackend = backend.rawValue
        }

        loggingService.info(.Transcription, LogEvent.Transcription.started, data: [
            "backend": AnyCodable(actualBackend),
            "language": AnyCodable(language.rawValue),
            "audioSamples": AnyCodable(audio.count),
            "model": AnyCodable(currentModel?.rawValue ?? "unknown")
        ])

        do {
            let result: String
            switch backend {
            case .local:
                // Route to appropriate local backend based on model type
                if let model = currentModel, model.isSwedishOptimized {
                    result = try await whisperCppBackend.transcribe(audio: audio, language: language)
                } else {
                    result = try await whisperKitBackend.transcribe(audio: audio, language: language)
                }
            case .remote:
                result = try await openAIBackend.transcribe(audio: audio, language: language)
            }

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            logger.info("Transcription completed in \(elapsed, format: .fixed(precision: 2))s using \(actualBackend)")

            loggingService.info(.Transcription, LogEvent.Transcription.completed, data: [
                "backend": AnyCodable(actualBackend),
                "durationMs": AnyCodable(Int(elapsed * 1000)),
                "resultLength": AnyCodable(result.count)
            ])

            return result
        } catch {
            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            loggingService.error(.Transcription, LogEvent.Transcription.failed, data: [
                "backend": AnyCodable(actualBackend),
                "durationMs": AnyCodable(Int(elapsed * 1000)),
                "error": AnyCodable(error.localizedDescription)
            ])
            throw error
        }
    }

    /// Warm up the local transcription backend with the specified model
    /// Routes to WhisperKit for standard models, whisper.cpp for KB-Whisper
    /// - Parameter model: The WhisperModel to load
    func warmUp(model: WhisperModel = .base) async throws {
        currentModel = model

        if model.isSwedishOptimized {
            // Use whisper.cpp for KB-Whisper Swedish models
            try await whisperCppBackend.warmUp(model: model)
        } else {
            // Use WhisperKit for standard models
            try await whisperKitBackend.warmUp(model: model)
        }
    }

    /// Check if the local backend is ready (checks the appropriate backend for current model)
    var isLocalReady: Bool {
        get async {
            if let model = currentModel, model.isSwedishOptimized {
                return await whisperCppBackend.isReady
            }
            return await whisperKitBackend.isReady
        }
    }
}
