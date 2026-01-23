import Foundation
import AVFoundation
import os.log

/// Service for capturing audio from the microphone
/// Uses AVAudioEngine for real-time audio capture
final class AudioCaptureService {
    // MARK: - Types

    /// Audio format required by WhisperKit
    struct AudioFormat {
        static let sampleRate: Double = 16000
        static let channels: AVAudioChannelCount = 1
        static let commonFormat: AVAudioCommonFormat = .pcmFormatFloat32
    }

    /// Maximum buffer size: 15 minutes of audio at 16kHz (~1.8MB)
    /// Prevents unbounded memory growth during long recordings
    private static let maxBufferSize = Int(16000 * 60 * 15) // 15 minutes

    // MARK: - Private State

    private let audioEngine = AVAudioEngine()
    private var audioBuffer: [Float] = []
    private var bufferCount = 0
    private let bufferLock = NSLock()
    private let logger = Logger(subsystem: "com.flowdictate", category: "AudioCapture")
    private let loggingService = LoggingService.shared

    /// Retained audio from last capture for retry on transcription failure
    private(set) var lastCapturedAudio: [Float]?

    // MARK: - Public Methods

    /// Start capturing audio from the microphone
    /// - Throws: Error if microphone permission is denied or audio engine fails to start
    func startCapture() async throws {
        print("[Audio] Checking microphone permission...")

        // Check microphone permission
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            print("[Audio] ✓ Microphone permission granted")
            loggingService.info(.Audio, LogEvent.Audio.permissionGranted, data: [:])
        case .notDetermined:
            print("[Audio] Requesting microphone permission...")
            loggingService.info(.Audio, LogEvent.Audio.permissionRequested, data: [:])
            // Request permission asynchronously (non-blocking)
            let granted = await AVCaptureDevice.requestAccess(for: .audio)
            guard granted else {
                print("[Audio] ✗ Microphone permission denied by user")
                loggingService.error(.Audio, LogEvent.Audio.permissionDenied, data: [
                    "reason": AnyCodable("user_denied")
                ])
                throw DictationError.microphonePermissionDenied
            }
            print("[Audio] ✓ Microphone permission granted")
            loggingService.info(.Audio, LogEvent.Audio.permissionGranted, data: [:])
        case .denied, .restricted:
            print("[Audio] ✗ Microphone permission denied - please enable in System Settings")
            loggingService.error(.Audio, LogEvent.Audio.permissionDenied, data: [
                "reason": AnyCodable("system_denied")
            ])
            throw DictationError.microphonePermissionDenied
        @unknown default:
            print("[Audio] ✗ Unknown microphone permission status")
            loggingService.error(.Audio, LogEvent.Audio.permissionDenied, data: [
                "reason": AnyCodable("unknown_status")
            ])
            throw DictationError.microphonePermissionDenied
        }

        // Clear previous buffer
        bufferLock.lock()
        audioBuffer.removeAll()
        bufferCount = 0
        bufferLock.unlock()

        // Configure audio session
        let inputNode = audioEngine.inputNode
        let inputFormat = inputNode.outputFormat(forBus: 0)
        print("[Audio] Input format: \(inputFormat.sampleRate)Hz, \(inputFormat.channelCount) channel(s)")

        // Create the format we need (16kHz mono)
        guard let targetFormat = AVAudioFormat(
            commonFormat: AudioFormat.commonFormat,
            sampleRate: AudioFormat.sampleRate,
            channels: AudioFormat.channels,
            interleaved: false
        ) else {
            print("[Audio] ✗ Failed to create target audio format")
            throw DictationError.transcriptionFailed("Failed to create audio format")
        }
        print("[Audio] Target format: \(targetFormat.sampleRate)Hz, \(targetFormat.channelCount) channel(s)")

        // Create a converter if needed
        let converter = AVAudioConverter(from: inputFormat, to: targetFormat)
        if converter != nil {
            print("[Audio] ✓ Audio converter created for format conversion")
        }

        // Install tap on input node
        // Buffer size of 1024 at 16kHz = ~64ms chunks (smaller for faster first buffer)
        inputNode.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { [weak self] buffer, _ in
            self?.processAudioBuffer(buffer, converter: converter, targetFormat: targetFormat)
        }

        // Start the audio engine
        do {
            try audioEngine.start()
            print("[Audio] ✓ Audio engine started - capturing audio")
            logger.info("Audio capture started")
            loggingService.info(.Audio, LogEvent.Audio.captureStarted, data: [
                "sampleRate": AnyCodable(Int(AudioFormat.sampleRate)),
                "channels": AnyCodable(Int(AudioFormat.channels)),
                "format": AnyCodable("float32")
            ])
        } catch {
            print("[Audio] ✗ Failed to start audio engine: \(error.localizedDescription)")
            inputNode.removeTap(onBus: 0)
            throw DictationError.transcriptionFailed("Failed to start audio engine: \(error.localizedDescription)")
        }
    }

    /// Stop capturing audio and return the captured samples
    /// Audio is retained in `lastCapturedAudio` for retry on transcription failure
    /// - Returns: Array of audio samples as Float32 at 16kHz mono
    func stopCapture() -> [Float] {
        print("[Audio] Stopping audio engine...")
        audioEngine.stop()
        audioEngine.inputNode.removeTap(onBus: 0)

        bufferLock.lock()
        let samples = audioBuffer
        audioBuffer.removeAll()
        bufferLock.unlock()

        // Retain audio for potential retry on transcription failure
        lastCapturedAudio = samples

        let durationSeconds = Double(samples.count) / AudioFormat.sampleRate
        print("[Audio] ✓ Captured \(samples.count) samples (\(String(format: "%.2f", durationSeconds)) seconds)")
        logger.info("Audio capture stopped, captured \(samples.count) samples")

        loggingService.info(.Audio, LogEvent.Audio.captureStopped, data: [
            "samples": AnyCodable(samples.count),
            "durationSeconds": AnyCodable(durationSeconds)
        ])

        return samples
    }

    /// Clear the retained audio after successful transcription
    func clearLastCapturedAudio() {
        lastCapturedAudio = nil
    }

    // MARK: - Private Methods

    private func processAudioBuffer(
        _ buffer: AVAudioPCMBuffer,
        converter: AVAudioConverter?,
        targetFormat: AVAudioFormat
    ) {
        bufferCount += 1
        if bufferCount <= 3 || bufferCount % 10 == 0 {
            print("[Audio] 📦 Buffer #\(bufferCount): \(buffer.frameLength) frames at \(buffer.format.sampleRate)Hz")
        }

        // If formats match, directly copy
        if buffer.format.sampleRate == AudioFormat.sampleRate &&
           buffer.format.channelCount == AudioFormat.channels {
            appendSamples(from: buffer)
            return
        }

        // Convert to target format using simple resampling
        guard let converter = converter else {
            print("[Audio] ⚠️  No converter, using direct copy")
            appendSamples(from: buffer)
            return
        }

        // Calculate output frame capacity
        let ratio = targetFormat.sampleRate / buffer.format.sampleRate
        let outputFrameCapacity = AVAudioFrameCount(Double(buffer.frameLength) * ratio)

        guard let convertedBuffer = AVAudioPCMBuffer(
            pcmFormat: targetFormat,
            frameCapacity: outputFrameCapacity
        ) else {
            print("[Audio] ✗ Failed to create converted buffer")
            return
        }

        // Use simpler conversion approach
        var error: NSError?
        var inputBufferUsed = false
        let status = converter.convert(to: convertedBuffer, error: &error) { inNumPackets, outStatus in
            if inputBufferUsed {
                outStatus.pointee = .noDataNow
                return nil
            }
            inputBufferUsed = true
            outStatus.pointee = .haveData
            return buffer
        }

        if let error = error {
            print("[Audio] ✗ Conversion error: \(error.localizedDescription)")
            return
        }

        if status == .haveData || status == .endOfStream {
            appendSamples(from: convertedBuffer)
        } else if status == .error {
            print("[Audio] ✗ Converter returned error status")
        }
    }

    private func appendSamples(from buffer: AVAudioPCMBuffer) {
        guard let channelData = buffer.floatChannelData else {
            print("[Audio] ✗ No channel data in buffer")
            return
        }

        let frameCount = Int(buffer.frameLength)
        guard frameCount > 0 else {
            print("[Audio] ⚠️  Buffer has 0 frames")
            return
        }

        let samples = Array(UnsafeBufferPointer(
            start: channelData[0],
            count: frameCount
        ))

        bufferLock.lock()
        let previousCount = audioBuffer.count

        // Check buffer size limit before appending
        if previousCount + samples.count > Self.maxBufferSize {
            let minutesRecorded = Double(previousCount) / AudioFormat.sampleRate / 60.0
            logger.warning("Audio buffer limit reached (\(String(format: "%.1f", minutesRecorded)) minutes). Ignoring new samples.")
            if previousCount < Self.maxBufferSize {
                // Log warning only once when we first hit the limit
                loggingService.warning(.Audio, "audio.buffer_limit_reached", data: [
                    "bufferSize": AnyCodable(previousCount),
                    "maxSize": AnyCodable(Self.maxBufferSize),
                    "minutesRecorded": AnyCodable(String(format: "%.1f", minutesRecorded))
                ])
            }
            bufferLock.unlock()
            return
        }

        audioBuffer.append(contentsOf: samples)
        bufferLock.unlock()

        if previousCount == 0 {
            print("[Audio] ✓ First samples captured (\(samples.count) samples)")
        }
    }
}
