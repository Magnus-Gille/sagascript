import Foundation
import SwiftWhisper
import os.log

/// Transcription backend using whisper.cpp via SwiftWhisper
/// Used for KB-Whisper Swedish-optimized models (GGML format)
actor WhisperCppBackend: TranscriptionBackendProtocol {
    // MARK: - Private State

    private var whisper: Whisper?
    private var currentModel: WhisperModel?
    private var isLoading = false
    private let logger = Logger(subsystem: "com.flowdictate", category: "WhisperCpp")
    private let loggingService = LoggingService.shared
    private let downloadService = ModelDownloadService.shared

    // MARK: - TranscriptionBackendProtocol

    var isReady: Bool {
        whisper != nil
    }

    /// Warm up with the specified KB-Whisper model
    /// Downloads the model if not present, then loads it
    func warmUp(model: WhisperModel = .kbWhisperBase) async throws {
        guard model.isSwedishOptimized else {
            throw DictationError.modelNotLoaded
        }

        // If same model is already loaded, skip
        if let currentModel = currentModel, currentModel == model, whisper != nil {
            print("[WhisperCpp] Model \(model.rawValue) already loaded, skipping warmup")
            return
        }

        // If loading in progress, wait
        guard !isLoading else {
            print("[WhisperCpp] Model loading in progress, skipping warmup")
            return
        }

        isLoading = true
        defer { isLoading = false }

        print("")
        print("┌────────────────────────────────────────────────────────────┐")
        print("│  Loading KB-Whisper Model (whisper.cpp)                    │")
        print("└────────────────────────────────────────────────────────────┘")
        print("")
        print("[WhisperCpp] Model: \(model.displayName)")
        print("[WhisperCpp] Using whisper.cpp backend for Swedish-optimized transcription")
        print("")

        logger.info("Loading whisper.cpp model: \(model.rawValue)")
        let startTime = CFAbsoluteTimeGetCurrent()

        loggingService.info(.Transcription, LogEvent.Transcription.modelLoading, data: [
            "model": AnyCodable(model.rawValue),
            "backend": AnyCodable("whisper.cpp"),
            "parameterCount": AnyCodable(model.parameterCount),
            "isSwedishOptimized": AnyCodable(true)
        ])

        do {
            // Ensure model is downloaded
            let modelPath = try await downloadService.ensureModelAvailable(model)
            print("[WhisperCpp] Model path: \(modelPath.path)")

            // Load the model
            print("[WhisperCpp] Loading model into memory...")
            whisper = Whisper(fromFileURL: modelPath)
            currentModel = model

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            print("")
            print("[WhisperCpp] ✓ Model loaded in \(String(format: "%.1f", elapsed))s")
            logger.info("whisper.cpp model \(model.rawValue) loaded in \(elapsed, format: .fixed(precision: 2))s")

            loggingService.info(.Transcription, LogEvent.Transcription.modelLoaded, data: [
                "model": AnyCodable(model.rawValue),
                "backend": AnyCodable("whisper.cpp"),
                "loadTimeMs": AnyCodable(Int(elapsed * 1000))
            ])
        } catch {
            print("[WhisperCpp] ✗ Failed to load model: \(error.localizedDescription)")
            logger.error("Failed to load whisper.cpp model: \(error.localizedDescription)")

            loggingService.error(.Transcription, LogEvent.Transcription.modelFailed, data: [
                "model": AnyCodable(model.rawValue),
                "backend": AnyCodable("whisper.cpp"),
                "error": AnyCodable(error.localizedDescription)
            ])

            throw DictationError.modelNotLoaded
        }
    }

    /// Legacy warmUp() for protocol conformance
    func warmUp() async throws {
        try await warmUp(model: .kbWhisperBase)
    }

    func transcribe(audio: [Float], language: Language) async throws -> String {
        guard let whisper = whisper else {
            print("[WhisperCpp] ✗ Model not loaded!")
            throw DictationError.modelNotLoaded
        }

        guard !audio.isEmpty else {
            print("[WhisperCpp] ✗ No audio data provided!")
            throw DictationError.noAudioCaptured
        }

        let audioDuration = Double(audio.count) / 16000.0
        print("[WhisperCpp] Processing \(audio.count) samples (~\(String(format: "%.1f", audioDuration))s)")
        print("[WhisperCpp] Running inference...")

        logger.info("Starting whisper.cpp transcription of \(audio.count) samples")
        let startTime = CFAbsoluteTimeGetCurrent()

        do {
            // SwiftWhisper expects audio frames as [Float] at 16kHz - matches our format
            let segments = try await whisper.transcribe(audioFrames: audio)

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime

            // Calculate Real-Time Factor (RTF)
            let rtf = elapsed / audioDuration
            let rtfStatus = rtf < 0.3 ? "excellent" : (rtf < 0.5 ? "good" : (rtf < 1.0 ? "acceptable" : "SLOW"))

            print("[WhisperCpp] ✓ Inference completed in \(String(format: "%.2f", elapsed))s")
            print("[WhisperCpp] RTF: \(String(format: "%.2fx", rtf)) realtime (\(rtfStatus))")
            logger.info("whisper.cpp transcription completed in \(elapsed, format: .fixed(precision: 2))s, RTF: \(rtf, format: .fixed(precision: 2))")

            // Combine all segments
            let text = segments.map { $0.text }.joined(separator: " ").trimmingCharacters(in: .whitespaces)

            if text.isEmpty {
                print("[WhisperCpp] ⚠️  Transcription returned empty text (silence or noise?)")
                logger.warning("whisper.cpp transcription returned empty text")
            }

            loggingService.info(.Transcription, LogEvent.Transcription.completed, data: [
                "model": AnyCodable(currentModel?.rawValue ?? "unknown"),
                "backend": AnyCodable("whisper.cpp"),
                "audioDurationMs": AnyCodable(Int(audioDuration * 1000)),
                "transcriptionTimeMs": AnyCodable(Int(elapsed * 1000)),
                "rtf": AnyCodable(String(format: "%.2f", rtf)),
                "textLength": AnyCodable(text.count)
            ])

            return text
        } catch {
            print("[WhisperCpp] ✗ Transcription failed: \(error.localizedDescription)")
            logger.error("whisper.cpp transcription failed: \(error.localizedDescription)")

            loggingService.error(.Transcription, LogEvent.Transcription.failed, data: [
                "model": AnyCodable(currentModel?.rawValue ?? "unknown"),
                "backend": AnyCodable("whisper.cpp"),
                "error": AnyCodable(error.localizedDescription)
            ])

            throw DictationError.transcriptionFailed(error.localizedDescription)
        }
    }
}
