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
    private let loggingService = LoggingService.shared

    // MARK: - TranscriptionBackendProtocol

    var isReady: Bool {
        whisperKit != nil
    }

    func warmUp() async throws {
        guard whisperKit == nil && !isLoading else {
            print("[WhisperKit] Model already loaded or loading, skipping warmup")
            return
        }

        isLoading = true
        defer { isLoading = false }

        print("")
        print("┌────────────────────────────────────────────────────────────┐")
        print("│           Loading WhisperKit Model (base)                  │")
        print("└────────────────────────────────────────────────────────────┘")
        print("")
        print("[WhisperKit] This may take a while on first run...")
        print("[WhisperKit] - Checking for cached model...")
        print("[WhisperKit] - If not cached, downloading from Hugging Face (~150MB)")
        print("[WhisperKit] - Then compiling for Neural Engine...")
        print("")

        logger.info("Loading WhisperKit model...")
        let startTime = CFAbsoluteTimeGetCurrent()

        loggingService.info(.Transcription, LogEvent.Transcription.modelLoading, data: [
            "model": AnyCodable("base"),
            "backend": AnyCodable("local")
        ])

        // Create a timer to show progress while loading
        let progressTask = Task {
            var dots = 0
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 2_000_000_000) // 2 seconds
                dots += 1
                let elapsed = CFAbsoluteTimeGetCurrent() - startTime
                print("[WhisperKit] Still loading... (\(String(format: "%.0f", elapsed))s elapsed)")
            }
        }

        do {
            // Use a small model for fast startup
            // Users can configure larger models later
            print("[WhisperKit] Initializing WhisperKit...")
            whisperKit = try await WhisperKit(
                model: "base",
                computeOptions: .init(
                    audioEncoderCompute: .cpuAndNeuralEngine,
                    textDecoderCompute: .cpuAndNeuralEngine
                )
            )

            progressTask.cancel()

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            print("")
            print("[WhisperKit] ✓ Model loaded successfully in \(String(format: "%.1f", elapsed))s")
            logger.info("WhisperKit model loaded in \(elapsed, format: .fixed(precision: 2))s")

            loggingService.info(.Transcription, LogEvent.Transcription.modelLoaded, data: [
                "model": AnyCodable("base"),
                "loadTimeMs": AnyCodable(Int(elapsed * 1000))
            ])
        } catch {
            progressTask.cancel()
            print("[WhisperKit] ✗ Failed to load model: \(error.localizedDescription)")
            logger.error("Failed to load WhisperKit model: \(error.localizedDescription)")

            loggingService.error(.Transcription, LogEvent.Transcription.modelFailed, data: [
                "model": AnyCodable("base"),
                "error": AnyCodable(error.localizedDescription)
            ])

            throw DictationError.modelNotLoaded
        }
    }

    func transcribe(audio: [Float], language: Language) async throws -> String {
        guard let whisperKit = whisperKit else {
            print("[WhisperKit] ✗ Model not loaded!")
            throw DictationError.modelNotLoaded
        }

        guard !audio.isEmpty else {
            print("[WhisperKit] ✗ No audio data provided!")
            throw DictationError.noAudioCaptured
        }

        let durationSeconds = Double(audio.count) / 16000.0
        print("[WhisperKit] Processing \(audio.count) samples (~\(String(format: "%.1f", durationSeconds))s)")
        print("[WhisperKit] Running inference on Neural Engine...")

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
            print("[WhisperKit] ✓ Inference completed in \(String(format: "%.2f", elapsed))s")
            logger.info("Transcription completed in \(elapsed, format: .fixed(precision: 2))s")

            // Combine all segments
            let text = results.map { $0.text }.joined(separator: " ").trimmingCharacters(in: CharacterSet.whitespaces)

            if text.isEmpty {
                print("[WhisperKit] ⚠️  Transcription returned empty text (silence or noise?)")
                logger.warning("Transcription returned empty text")
            }

            return text
        } catch {
            print("[WhisperKit] ✗ Transcription failed: \(error.localizedDescription)")
            logger.error("Transcription failed: \(error.localizedDescription)")
            throw DictationError.transcriptionFailed(error.localizedDescription)
        }
    }
}
