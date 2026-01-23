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

    // MARK: - Private State

    private let audioEngine = AVAudioEngine()
    private var audioBuffer: [Float] = []
    private let bufferLock = NSLock()
    private let logger = Logger(subsystem: "com.flowdictate", category: "AudioCapture")

    // MARK: - Public Methods

    /// Start capturing audio from the microphone
    /// - Throws: Error if microphone permission is denied or audio engine fails to start
    func startCapture() throws {
        print("[Audio] Checking microphone permission...")

        // Check microphone permission
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized:
            print("[Audio] ✓ Microphone permission granted")
        case .notDetermined:
            print("[Audio] Requesting microphone permission...")
            // Request permission synchronously (blocking)
            let semaphore = DispatchSemaphore(value: 0)
            var granted = false
            AVCaptureDevice.requestAccess(for: .audio) { result in
                granted = result
                semaphore.signal()
            }
            semaphore.wait()
            guard granted else {
                print("[Audio] ✗ Microphone permission denied by user")
                throw DictationError.microphonePermissionDenied
            }
            print("[Audio] ✓ Microphone permission granted")
        case .denied, .restricted:
            print("[Audio] ✗ Microphone permission denied - please enable in System Settings")
            throw DictationError.microphonePermissionDenied
        @unknown default:
            print("[Audio] ✗ Unknown microphone permission status")
            throw DictationError.microphonePermissionDenied
        }

        // Clear previous buffer
        bufferLock.lock()
        audioBuffer.removeAll()
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
        // Buffer size of 4096 at 16kHz = ~256ms chunks
        inputNode.installTap(onBus: 0, bufferSize: 4096, format: inputFormat) { [weak self] buffer, _ in
            self?.processAudioBuffer(buffer, converter: converter, targetFormat: targetFormat)
        }

        // Start the audio engine
        do {
            try audioEngine.start()
            print("[Audio] ✓ Audio engine started - capturing audio")
            logger.info("Audio capture started")
        } catch {
            print("[Audio] ✗ Failed to start audio engine: \(error.localizedDescription)")
            inputNode.removeTap(onBus: 0)
            throw DictationError.transcriptionFailed("Failed to start audio engine: \(error.localizedDescription)")
        }
    }

    /// Stop capturing audio and return the captured samples
    /// - Returns: Array of audio samples as Float32 at 16kHz mono
    func stopCapture() -> [Float] {
        print("[Audio] Stopping audio engine...")
        audioEngine.stop()
        audioEngine.inputNode.removeTap(onBus: 0)

        bufferLock.lock()
        let samples = audioBuffer
        audioBuffer.removeAll()
        bufferLock.unlock()

        let durationSeconds = Double(samples.count) / AudioFormat.sampleRate
        print("[Audio] ✓ Captured \(samples.count) samples (\(String(format: "%.2f", durationSeconds)) seconds)")
        logger.info("Audio capture stopped, captured \(samples.count) samples")
        return samples
    }

    // MARK: - Private Methods

    private func processAudioBuffer(
        _ buffer: AVAudioPCMBuffer,
        converter: AVAudioConverter?,
        targetFormat: AVAudioFormat
    ) {
        // If formats match, directly copy
        if buffer.format.sampleRate == AudioFormat.sampleRate &&
           buffer.format.channelCount == AudioFormat.channels {
            appendSamples(from: buffer)
            return
        }

        // Convert to target format
        guard let converter = converter else {
            // No converter available, try direct copy
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
            return
        }

        var error: NSError?
        let status = converter.convert(to: convertedBuffer, error: &error) { inNumPackets, outStatus in
            outStatus.pointee = .haveData
            return buffer
        }

        if status == .haveData || status == .endOfStream {
            appendSamples(from: convertedBuffer)
        }
    }

    private func appendSamples(from buffer: AVAudioPCMBuffer) {
        guard let channelData = buffer.floatChannelData else { return }

        let samples = Array(UnsafeBufferPointer(
            start: channelData[0],
            count: Int(buffer.frameLength)
        ))

        bufferLock.lock()
        audioBuffer.append(contentsOf: samples)
        bufferLock.unlock()
    }
}
