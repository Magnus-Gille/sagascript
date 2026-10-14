import Foundation

/// Error codes of engine-host protocol v1. The raw values are the wire strings; this enum is the
/// only place they are spelled out (docs/engine-host-protocol.md, Envelope).
public enum WireErrorCode: String, CaseIterable, Sendable {
    /// Envelope or sequencing error (bad JSON, missing id/op, wrong `v`, op before `hello`).
    case protocolError = "protocol"
    case badRequest = "bad_request"
    case unsupported = "unsupported"
    case notLoaded = "not_loaded"
    case modelMissing = "model_missing"
    case modelLoadFailed = "model_load_failed"
    /// The only retryable code.
    case busy = "busy"
    case cancelled = "cancelled"
    case engine = "engine"
    case internalError = "internal"
}

public struct EngineHostError: Error, Equatable, Sendable {
    public let code: WireErrorCode
    public let message: String
    public let retryable: Bool

    public init(code: WireErrorCode, message: String, retryable: Bool = false) {
        self.code = code
        self.message = message
        self.retryable = retryable
    }
}

/// What `hello` announces (`capabilities`). The defaults are the Core ML host's.
public struct HostCapabilities: Sendable {
    public let sampleRate: Int
    public let maxWindowSeconds: Double
    public let preferredWindowSeconds: Double
    public let preferredOverlapSeconds: Double
    public let maxInFlight: Int
    public let tokenTimestamps: Bool
    public let languages: [String]
    public let computeUnits: [String]
    /// Omitted from `hello` when nil.
    public let minMacOS: String?
    public let contextBiasing: Bool

    public init(
        sampleRate: Int = 16_000,
        maxWindowSeconds: Double = 30,
        preferredWindowSeconds: Double = 15,
        preferredOverlapSeconds: Double = 2,
        maxInFlight: Int = 1,
        tokenTimestamps: Bool = true,
        languages: [String] = ["sv"],
        computeUnits: [String] = ["ane", "gpu", "cpu", "all"],
        minMacOS: String? = "14.0",
        contextBiasing: Bool = true
    ) {
        self.sampleRate = sampleRate
        self.maxWindowSeconds = maxWindowSeconds
        self.preferredWindowSeconds = preferredWindowSeconds
        self.preferredOverlapSeconds = preferredOverlapSeconds
        self.maxInFlight = maxInFlight
        self.tokenTimestamps = tokenTimestamps
        self.languages = languages
        self.computeUnits = computeUnits
        self.minMacOS = minMacOS
        self.contextBiasing = contextBiasing
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
}

/// `transcribe_window` priority. A request without one is a `bad_request`, as in the Rust types.
public enum WindowPriority: String, Sendable {
    case interactive
    case batch
}

public struct WindowRequest: Sendable {
    public let pcmPath: String
    public let offsetSamples: Int
    public let numSamples: Int
    public let sampleRate: Int
    public let format: String
    public let priority: WindowPriority
    public let boost: BoostConfig?

    public init(
        pcmPath: String,
        offsetSamples: Int,
        numSamples: Int,
        sampleRate: Int,
        format: String,
        priority: WindowPriority,
        boost: BoostConfig? = nil
    ) {
        self.pcmPath = pcmPath
        self.offsetSamples = offsetSamples
        self.numSamples = numSamples
        self.sampleRate = sampleRate
        self.format = format
        self.priority = priority
        self.boost = boost
    }
}

public struct TranscriptionToken: Sendable {
    public let id: Int
    public let text: String
    public let start: Double
    public let duration: Double
    public let confidence: Double
}

public struct WindowResult: Sendable {
    public let tokens: [TranscriptionToken]
    public let audioSeconds: Double
    public let preprocessMilliseconds: Double
    public let encodeMilliseconds: Double
    public let decodeMilliseconds: Double
    /// nil when no dictionary was sent; otherwise whether biasing was in effect for this window.
    public var boostActive: Bool? = nil
    /// Why biasing was not applied (`no_usable_terms`, `no_top_k_outputs`); nil when active or not requested.
    public var boostReason: String? = nil
    /// Trie build time in microseconds (0 when cached or not requested).
    public var boostMicroseconds: Int = 0
}

public protocol EngineBackend: AnyObject {
    var capabilities: HostCapabilities { get }
    var loadedModelID: String? { get }
    var isLoading: Bool { get }

    func load(modelDirectory: String, modelID: String, computeUnits: String) throws -> LoadResult
    func transcribe(_ request: WindowRequest, isCancelled: @escaping () -> Bool) throws -> WindowResult
    func unload()
}
