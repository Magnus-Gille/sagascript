import Foundation
import WhisperKit
import os.log

/// Local transcription backend using WhisperKit
/// Runs entirely on-device using Core ML and Neural Engine
actor WhisperKitBackend: TranscriptionBackendProtocol {
    // MARK: - Private State

    private var whisperKit: WhisperKit?
    private var isLoading = false
    private let logger = Logger(subsystem: "com.flowdictate", category: "WhisperKit")

    // MARK: - TranscriptionBackendProtocol

    var isReady: Bool {
        whisperKit != nil
    }

    func warmUp() async throws {
        guard whisperKit == nil && !isLoading else { return }

        isLoading = true
        defer { isLoading = false }

        logger.info("Loading WhisperKit model...")
        let startTime = CFAbsoluteTimeGetCurrent()

        do {
            // Use a small model for fast startup
            // Users can configure larger models later
            whisperKit = try await WhisperKit(
                model: "base",
                computeOptions: .init(
                    audioEncoderCompute: .cpuAndNeuralEngine,
                    textDecoderCompute: .cpuAndNeuralEngine
                )
            )

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            logger.info("WhisperKit model loaded in \(elapsed, format: .fixed(precision: 2))s")
        } catch {
            logger.error("Failed to load WhisperKit model: \(error.localizedDescription)")
            throw DictationError.modelNotLoaded
        }
    }

    func transcribe(audio: [Float], language: Language) async throws -> String {
        guard let whisperKit = whisperKit else {
            throw DictationError.modelNotLoaded
        }

        guard !audio.isEmpty else {
            throw DictationError.noAudioCaptured
        }

        logger.info("Starting transcription of \(audio.count) samples")
        let startTime = CFAbsoluteTimeGetCurrent()

        do {
            // Configure decoding options
            var options = DecodingOptions(
                task: .transcribe,
                usePrefillPrompt: true,
                skipSpecialTokens: true,
                withoutTimestamps: true
            )
            options.language = language.whisperCode

            // Run transcription
            let results = try await whisperKit.transcribe(
                audioArray: audio,
                decodeOptions: options
            )

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            logger.info("Transcription completed in \(elapsed, format: .fixed(precision: 2))s")

            // Combine all segments
            let text = results.map { $0.text }.joined(separator: " ").trimmingCharacters(in: CharacterSet.whitespaces)

            if text.isEmpty {
                logger.warning("Transcription returned empty text")
            }

            return text
        } catch {
            logger.error("Transcription failed: \(error.localizedDescription)")
            throw DictationError.transcriptionFailed(error.localizedDescription)
        }
    }
}
