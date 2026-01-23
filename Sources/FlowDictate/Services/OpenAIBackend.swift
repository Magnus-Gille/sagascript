import Foundation
import os.log

/// Remote transcription backend using OpenAI Audio Transcription API
/// Sends audio to OpenAI for transcription (requires API key)
final class OpenAIBackend: TranscriptionBackendProtocol {
    // MARK: - Constants

    private let apiURL = URL(string: "https://api.openai.com/v1/audio/transcriptions")!
    private let model = "whisper-1" // Can also use gpt-4o-transcribe when available

    // MARK: - Private State

    private let keychainService: KeychainService
    private let logger = Logger(subsystem: "com.flowdictate", category: "OpenAI")

    // MARK: - Initialization

    init(keychainService: KeychainService = KeychainService.shared) {
        self.keychainService = keychainService
    }

    // MARK: - TranscriptionBackendProtocol

    var isReady: Bool {
        get async {
            keychainService.getAPIKey() != nil
        }
    }

    func warmUp() async throws {
        // No warm-up needed for remote backend
        // Just verify API key exists
        guard keychainService.getAPIKey() != nil else {
            throw DictationError.apiKeyMissing
        }
    }

    func transcribe(audio: [Float], language: Language) async throws -> String {
        guard let apiKey = keychainService.getAPIKey() else {
            throw DictationError.apiKeyMissing
        }

        guard !audio.isEmpty else {
            throw DictationError.noAudioCaptured
        }

        logger.info("Starting remote transcription of \(audio.count) samples")
        let startTime = CFAbsoluteTimeGetCurrent()

        // Convert Float samples to WAV data
        let wavData = createWAVData(from: audio)

        // Create multipart form data request
        let boundary = UUID().uuidString
        var request = URLRequest(url: apiURL)
        request.httpMethod = "POST"
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")

        var body = Data()

        // Add model field
        body.append("--\(boundary)\r\n".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"model\"\r\n\r\n".data(using: .utf8)!)
        body.append("\(model)\r\n".data(using: .utf8)!)

        // Add language field (if not auto)
        if let langCode = language.whisperCode {
            body.append("--\(boundary)\r\n".data(using: .utf8)!)
            body.append("Content-Disposition: form-data; name=\"language\"\r\n\r\n".data(using: .utf8)!)
            body.append("\(langCode)\r\n".data(using: .utf8)!)
        }

        // Add audio file
        body.append("--\(boundary)\r\n".data(using: .utf8)!)
        body.append("Content-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n".data(using: .utf8)!)
        body.append("Content-Type: audio/wav\r\n\r\n".data(using: .utf8)!)
        body.append(wavData)
        body.append("\r\n".data(using: .utf8)!)

        // Close boundary
        body.append("--\(boundary)--\r\n".data(using: .utf8)!)

        request.httpBody = body

        // Send request
        do {
            let (data, response) = try await URLSession.shared.data(for: request)

            guard let httpResponse = response as? HTTPURLResponse else {
                throw DictationError.networkError("Invalid response")
            }

            guard httpResponse.statusCode == 200 else {
                let errorMessage = String(data: data, encoding: .utf8) ?? "Unknown error"
                logger.error("API error: \(httpResponse.statusCode) - \(errorMessage)")

                if httpResponse.statusCode == 401 {
                    throw DictationError.apiKeyMissing
                }
                throw DictationError.networkError("API error: \(httpResponse.statusCode)")
            }

            // Parse response
            let json = try JSONDecoder().decode(TranscriptionResponse.self, from: data)

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            logger.info("Remote transcription completed in \(elapsed, format: .fixed(precision: 2))s")

            return json.text.trimmingCharacters(in: .whitespaces)
        } catch let error as DictationError {
            throw error
        } catch {
            logger.error("Network error: \(error.localizedDescription)")
            throw DictationError.networkError(error.localizedDescription)
        }
    }

    // MARK: - Private Methods

    /// Create WAV file data from Float samples at 16kHz mono
    private func createWAVData(from samples: [Float]) -> Data {
        let sampleRate: UInt32 = 16000
        let channels: UInt16 = 1
        let bitsPerSample: UInt16 = 16

        // Convert Float samples to Int16
        let int16Samples = samples.map { sample -> Int16 in
            let clamped = max(-1.0, min(1.0, sample))
            return Int16(clamped * Float(Int16.max))
        }

        let dataSize = UInt32(int16Samples.count * 2)
        let fileSize = dataSize + 36

        var header = Data()

        // RIFF header
        header.append("RIFF".data(using: .ascii)!)
        header.append(withUnsafeBytes(of: fileSize.littleEndian) { Data($0) })
        header.append("WAVE".data(using: .ascii)!)

        // fmt chunk
        header.append("fmt ".data(using: .ascii)!)
        header.append(withUnsafeBytes(of: UInt32(16).littleEndian) { Data($0) }) // chunk size
        header.append(withUnsafeBytes(of: UInt16(1).littleEndian) { Data($0) }) // audio format (PCM)
        header.append(withUnsafeBytes(of: channels.littleEndian) { Data($0) })
        header.append(withUnsafeBytes(of: sampleRate.littleEndian) { Data($0) })
        let byteRate = sampleRate * UInt32(channels) * UInt32(bitsPerSample / 8)
        header.append(withUnsafeBytes(of: byteRate.littleEndian) { Data($0) })
        let blockAlign = channels * (bitsPerSample / 8)
        header.append(withUnsafeBytes(of: blockAlign.littleEndian) { Data($0) })
        header.append(withUnsafeBytes(of: bitsPerSample.littleEndian) { Data($0) })

        // data chunk
        header.append("data".data(using: .ascii)!)
        header.append(withUnsafeBytes(of: dataSize.littleEndian) { Data($0) })

        // Audio data
        var audioData = Data(capacity: int16Samples.count * 2)
        for sample in int16Samples {
            withUnsafeBytes(of: sample.littleEndian) { audioData.append(contentsOf: $0) }
        }

        return header + audioData
    }
}

// MARK: - Response Types

private struct TranscriptionResponse: Codable {
    let text: String
}
