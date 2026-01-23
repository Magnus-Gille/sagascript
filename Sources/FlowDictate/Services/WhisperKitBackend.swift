import Foundation
import WhisperKit
import os.log

/// Local transcription backend using WhisperKit
/// Runs entirely on-device using Core ML and Neural Engine
///
/// Performance optimizations applied:
/// - Model prewarming (prewarm: true) - specializes CoreML models for ANE
/// - Full compute options for all pipeline stages
/// - Greedy decoding (temperature=0) for fastest inference
/// - Quality check thresholds disabled for speed
/// - Configurable model size (default: tinyEn for lowest latency)
actor WhisperKitBackend: TranscriptionBackendProtocol {
    // MARK: - Private State

    private var whisperKit: WhisperKit?
    private var currentModel: WhisperModel?
    private var isLoading = false
    private let logger = Logger(subsystem: "com.flowdictate", category: "WhisperKit")
    private let loggingService = LoggingService.shared

    // MARK: - TranscriptionBackendProtocol

    var isReady: Bool {
        whisperKit != nil
    }

    /// Warm up the model with the specified model variant
    /// - Parameter model: The WhisperModel to load (defaults to base for compatibility)
    ///
    /// Model loading strategy:
    /// - Standard models: Downloaded from argmaxinc/whisperkit-coreml (WhisperKit default)
    /// - KB-Whisper models: Loaded from local path ~/Library/Application Support/FlowDictate/Models/
    func warmUp(model: WhisperModel = .base) async throws {
        // If same model is already loaded, skip
        if let currentModel = currentModel, currentModel == model, whisperKit != nil {
            print("[WhisperKit] Model \(model.rawValue) already loaded, skipping warmup")
            return
        }

        // If loading in progress, wait
        guard !isLoading else {
            print("[WhisperKit] Model loading in progress, skipping warmup")
            return
        }

        isLoading = true
        defer { isLoading = false }

        // Check if KB-Whisper model is available locally
        if model.requiresLocalPath {
            guard model.isLocallyAvailable, let localPath = model.localModelPath else {
                let modelsDir = WhisperModel.localModelsDirectory.path
                print("")
                print("┌────────────────────────────────────────────────────────────┐")
                print("│  KB-Whisper Model Not Found                                │")
                print("└────────────────────────────────────────────────────────────┘")
                print("")
                print("[WhisperKit] ✗ Swedish model '\(model.modelName)' not found.")
                print("[WhisperKit] Expected location: \(modelsDir)/\(model.modelName)/")
                print("")
                print("[WhisperKit] To install KB-Whisper models:")
                print("  1. Convert model: pip install whisperkittools")
                print("  2. Run: whisperkit-generate-model --model-version KBLab/\(model.rawValue) --output-dir \"\(modelsDir)\"")
                print("")

                loggingService.error(.Transcription, LogEvent.Transcription.modelFailed, data: [
                    "model": AnyCodable(model.rawValue),
                    "error": AnyCodable("Model not found at \(modelsDir)/\(model.modelName)")
                ])

                throw DictationError.modelNotLoaded
            }

            print("")
            print("┌────────────────────────────────────────────────────────────┐")
            print("│  Loading KB-Whisper Model (Swedish-optimized)              │")
            print("└────────────────────────────────────────────────────────────┘")
            print("")
            print("[WhisperKit] Loading from: \(localPath.path)")
        } else {
            print("")
            print("┌────────────────────────────────────────────────────────────┐")
            print("│           Loading WhisperKit Model (\(model.rawValue.padding(toLength: 16, withPad: " ", startingAt: 0)))       │")
            print("└────────────────────────────────────────────────────────────┘")
            print("")
            print("[WhisperKit] Loading standard model '\(model.rawValue)'...")
            print("[WhisperKit] - If not cached: downloading from Hugging Face (slow)")
        }

        print("[WhisperKit] - Prewarming: compiling for Neural Engine (one-time)")
        print("")

        let sourceInfo = model.requiresLocalPath ? "local" : "HuggingFace"
        logger.info("Loading WhisperKit model: \(model.rawValue) from \(sourceInfo)")
        let startTime = CFAbsoluteTimeGetCurrent()

        loggingService.info(.Transcription, LogEvent.Transcription.modelLoading, data: [
            "model": AnyCodable(model.rawValue),
            "modelName": AnyCodable(model.modelName),
            "source": AnyCodable(sourceInfo),
            "backend": AnyCodable("local"),
            "parameterCount": AnyCodable(model.parameterCount),
            "isSwedishOptimized": AnyCodable(model.isSwedishOptimized)
        ])

        // Create a timer to show progress while loading
        let progressTask = Task {
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 2_000_000_000) // 2 seconds
                let elapsed = CFAbsoluteTimeGetCurrent() - startTime
                print("[WhisperKit] Still loading... (\(String(format: "%.0f", elapsed))s elapsed)")
            }
        }

        do {
            print("[WhisperKit] Initializing WhisperKit with optimized settings...")

            // Performance-optimized compute options:
            // - melCompute: GPU for mel spectrogram (fast parallel FFT)
            // - audioEncoderCompute: ANE for audio encoding (optimized for transformers)
            // - textDecoderCompute: ANE for text decoding (optimized for transformers)
            // - prefillCompute: CPU for prefill (small operation, avoids ANE context switch)
            let computeOptions = ModelComputeOptions(
                melCompute: .cpuAndGPU,
                audioEncoderCompute: .cpuAndNeuralEngine,
                textDecoderCompute: .cpuAndNeuralEngine,
                prefillCompute: .cpuOnly
            )

            // Load model from local path (KB-Whisper) or default HuggingFace repo (standard)
            if let localPath = model.localModelPath {
                // KB-Whisper: load from local directory
                whisperKit = try await WhisperKit(
                    modelFolder: localPath.path,
                    computeOptions: computeOptions,
                    prewarm: true
                )
            } else {
                // Standard models: download from WhisperKit's default repo
                whisperKit = try await WhisperKit(
                    model: model.modelName,
                    computeOptions: computeOptions,
                    prewarm: true
                )
            }

            currentModel = model

            progressTask.cancel()

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            print("")
            print("[WhisperKit] ✓ Model loaded and prewarmed in \(String(format: "%.1f", elapsed))s")
            logger.info("WhisperKit model \(model.rawValue) loaded in \(elapsed, format: .fixed(precision: 2))s")

            loggingService.info(.Transcription, LogEvent.Transcription.modelLoaded, data: [
                "model": AnyCodable(model.rawValue),
                "loadTimeMs": AnyCodable(Int(elapsed * 1000)),
                "prewarmed": AnyCodable(true),
                "isSwedishOptimized": AnyCodable(model.isSwedishOptimized)
            ])
        } catch {
            progressTask.cancel()
            print("[WhisperKit] ✗ Failed to load model: \(error.localizedDescription)")
            logger.error("Failed to load WhisperKit model: \(error.localizedDescription)")

            loggingService.error(.Transcription, LogEvent.Transcription.modelFailed, data: [
                "model": AnyCodable(model.rawValue),
                "error": AnyCodable(error.localizedDescription)
            ])

            throw DictationError.modelNotLoaded
        }
    }

    /// Legacy warmUp() for protocol conformance - uses default model
    func warmUp() async throws {
        try await warmUp(model: .base)
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

        let audioDuration = Double(audio.count) / 16000.0
        print("[WhisperKit] Processing \(audio.count) samples (~\(String(format: "%.1f", audioDuration))s)")
        print("[WhisperKit] Running inference on Neural Engine...")

        logger.info("Starting transcription of \(audio.count) samples")
        let startTime = CFAbsoluteTimeGetCurrent()

        do {
            // Performance-optimized decoding options:
            // - temperature=0: Greedy decoding (fastest, no sampling)
            // - temperatureFallbackCount=0: No retries with higher temperature
            // - usePrefillPrompt/Cache: Reuse previous context for faster decoding
            // - skipSpecialTokens: Don't output <|en|>, <|transcribe|>, etc.
            // - withoutTimestamps: Skip timestamp tokens for faster decoding
            // - All quality thresholds disabled: Skip compression/logprob checks
            // - concurrentWorkerCount=16: Max parallel workers on macOS
            var options = DecodingOptions(
                task: .transcribe,
                temperature: 0.0,                    // Greedy decoding (deterministic, fastest)
                temperatureFallbackCount: 0,         // No retries with higher temperature
                usePrefillPrompt: true,              // Use prompt context
                usePrefillCache: true,               // Cache prefill computation
                skipSpecialTokens: true,             // Don't output special tokens
                withoutTimestamps: true,             // Skip timestamp computation
                compressionRatioThreshold: nil,      // Disable compression ratio check
                logProbThreshold: nil,               // Disable log probability check
                firstTokenLogProbThreshold: nil,     // Disable first token check
                noSpeechThreshold: nil,              // Disable silence detection threshold
                concurrentWorkerCount: 16            // Max parallel workers
            )
            options.language = language.whisperCode

            // Run transcription
            let results = try await whisperKit.transcribe(
                audioArray: audio,
                decodeOptions: options
            )

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime

            // Calculate Real-Time Factor (RTF)
            // RTF < 1.0 means faster than realtime, RTF > 1.0 means slower than realtime
            let rtf = elapsed / audioDuration
            let rtfStatus = rtf < 0.3 ? "excellent" : (rtf < 0.5 ? "good" : (rtf < 1.0 ? "acceptable" : "SLOW"))

            print("[WhisperKit] ✓ Inference completed in \(String(format: "%.2f", elapsed))s")
            print("[WhisperKit] RTF: \(String(format: "%.2fx", rtf)) realtime (\(rtfStatus))")
            logger.info("Transcription completed in \(elapsed, format: .fixed(precision: 2))s, RTF: \(rtf, format: .fixed(precision: 2))")

            // Combine all segments
            let text = results.map { $0.text }.joined(separator: " ").trimmingCharacters(in: CharacterSet.whitespaces)

            if text.isEmpty {
                print("[WhisperKit] ⚠️  Transcription returned empty text (silence or noise?)")
                logger.warning("Transcription returned empty text")
            }

            loggingService.info(.Transcription, LogEvent.Transcription.completed, data: [
                "model": AnyCodable(currentModel?.rawValue ?? "unknown"),
                "audioDurationMs": AnyCodable(Int(audioDuration * 1000)),
                "transcriptionTimeMs": AnyCodable(Int(elapsed * 1000)),
                "rtf": AnyCodable(String(format: "%.2f", rtf)),
                "textLength": AnyCodable(text.count)
            ])

            return text
        } catch {
            print("[WhisperKit] ✗ Transcription failed: \(error.localizedDescription)")
            logger.error("Transcription failed: \(error.localizedDescription)")

            loggingService.error(.Transcription, LogEvent.Transcription.failed, data: [
                "model": AnyCodable(currentModel?.rawValue ?? "unknown"),
                "error": AnyCodable(error.localizedDescription)
            ])

            throw DictationError.transcriptionFailed(error.localizedDescription)
        }
    }
}
