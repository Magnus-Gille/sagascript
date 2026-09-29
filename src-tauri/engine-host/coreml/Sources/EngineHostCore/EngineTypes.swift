import Foundation

public struct EngineHostError: Error, Equatable, Sendable {
    public let code: String
    public let message: String
    public let retryable: Bool

    public init(code: String, message: String, retryable: Bool = false) {
        self.code = code
        self.message = message
        self.retryable = retryable
    }
}

public struct HostCapabilities: Sendable {
    public let sampleRate: Int
    public let maxWindowSeconds: Double
    public let preferredWindowSeconds: Double
    public let preferredOverlapSeconds: Double
    public let maxInFlight: Int

    public init(
        sampleRate: Int = 16_000,
        maxWindowSeconds: Double = 30,
        preferredWindowSeconds: Double = 15,
        preferredOverlapSeconds: Double = 2,
        maxInFlight: Int = 1
    ) {
        self.sampleRate = sampleRate
        self.maxWindowSeconds = maxWindowSeconds
        self.preferredWindowSeconds = preferredWindowSeconds
        self.preferredOverlapSeconds = preferredOverlapSeconds
        self.maxInFlight = maxInFlight
    }

    public func jsonObject() -> [String: Any] {
        [
            "sample_rate": sampleRate,
            "max_window_s": maxWindowSeconds,
            "preferred_window_s": preferredWindowSeconds,
            "preferred_overlap_s": preferredOverlapSeconds,
            "max_in_flight": maxInFlight,
            "token_timestamps": true,
            "languages": ["sv"],
            "compute_units": ["ane", "gpu", "cpu", "all"],
            "min_macos": "14.0",
        ]
    }
}

public struct LoadResult: Sendable {
    public let modelID: String
    public let loadMilliseconds: Double
    public let compiled: Bool
    public let windowSeconds: Double
    public let frameSeconds: Double
    public let vocabularySize: Int
    public let blankID: Int

    public func jsonObject() -> [String: Any] {
        [
            "model_id": modelID,
            "load_ms": Int(loadMilliseconds.rounded()),
            "compiled": compiled,
            "window_s": windowSeconds,
            "frame_s": frameSeconds,
            "vocab_size": vocabularySize,
            "blank_id": blankID,
        ]
    }
}

public struct WindowRequest: Sendable {
    public let pcmPath: String
    public let offsetSamples: Int
    public let numSamples: Int
    public let sampleRate: Int
    public let format: String
    public let priority: String

    public init(
        pcmPath: String,
        offsetSamples: Int,
        numSamples: Int,
        sampleRate: Int,
        format: String,
        priority: String
    ) {
        self.pcmPath = pcmPath
        self.offsetSamples = offsetSamples
        self.numSamples = numSamples
        self.sampleRate = sampleRate
        self.format = format
        self.priority = priority
    }
}

public struct TranscriptionToken: Sendable {
    public let id: Int
    public let text: String
    public let start: Double
    public let duration: Double
    public let confidence: Double

    public func jsonObject() -> [String: Any] {
        [
            "id": id,
            "text": text,
            "start": start,
            "duration": duration,
            "confidence": confidence,
        ]
    }
}

public struct WindowResult: Sendable {
    public let tokens: [TranscriptionToken]
    public let audioSeconds: Double
    public let preprocessMilliseconds: Double
    public let encodeMilliseconds: Double
    public let decodeMilliseconds: Double

    public func jsonObject() -> [String: Any] {
        [
            "tokens": tokens.map { $0.jsonObject() },
            "audio_s": audioSeconds,
            "timings": [
                "preprocess_ms": Int(preprocessMilliseconds.rounded()),
                "encode_ms": Int(encodeMilliseconds.rounded()),
                "decode_ms": Int(decodeMilliseconds.rounded()),
            ],
        ]
    }
}

public protocol EngineBackend: AnyObject {
    var capabilities: HostCapabilities { get }
    var loadedModelID: String? { get }
    var isLoading: Bool { get }

    func load(modelDirectory: String, modelID: String, computeUnits: String) throws -> LoadResult
    func transcribe(_ request: WindowRequest, isCancelled: @escaping () -> Bool) throws -> WindowResult
    func unload()
}
