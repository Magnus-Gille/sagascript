// Engine-host protocol v1 on the wire: parsing requests and building responses. This file owns
// every wire field name of the Core ML host. The contract is docs/engine-host-protocol.md and the
// golden transcripts in src-tauri/engine-host/test-vectors/protocol/, which ProtocolServerTests
// replays; the Rust side (crates/sagascript-engine-protocol) decodes the same transcripts, so a
// renamed or retyped field fails a test on one side or the other.
//
// Parsing mirrors the Rust types: integers must be JSON integers (not floats, not booleans),
// strings must be strings, unknown fields are ignored, and a request whose parameters do not
// have the right types is a bad request.
import CoreFoundation
import Foundation

/// The `client` object of `hello`.
public struct WireClient: Equatable, Sendable {
    public let name: String
    public let version: String
    public let gitSHA: String
}

/// `transcribe_window` parameters, typed but not yet checked against the host's limits.
public struct WireWindowParameters: Sendable {
    public let pcmPath: String
    public let offsetSamples: Int
    public let numSamples: Int
    public let sampleRate: Int
    public let format: String
    public let priority: WindowPriority
    /// nil unless the request carries at least one term and a positive weight.
    public let boost: BoostConfig?
}

public enum WireRequest: Sendable {
    case hello(WireClient)
    case load(modelDirectory: String, modelID: String, computeUnits: String)
    case transcribeWindow(WireWindowParameters)
    case cancel(target: UInt64)
    case status
    case ping
    case unload
    case shutdown
}

/// One request line, classified like the Rust host (`decode_request` plus the envelope check).
public enum ParsedLine: Sendable {
    /// Answer `protocol` with this id (0 when the line has no usable id).
    case invalidEnvelope(id: UInt64, message: String)
    /// Answer `unsupported`.
    case unknownOp(id: UInt64, op: String)
    /// Answer `bad_request` (after the hello checks).
    case badParams(id: UInt64, op: String, message: String)
    case request(id: UInt64, op: String, request: WireRequest)
}

/// Build identity reported in `hello.host`.
public struct WireHostIdentity: Sendable {
    public let version: String
    public let gitSHA: String
    public let dirty: Bool
    public let engine: String
    public let engineVersion: String
}

/// `status.state`.
public enum WireHostState: String, Sendable {
    case idle
    case loading
    case ready
    case busy
}

public enum WireProtocol {
    public static let version = 1
    public static let maxLineBytes = 16 * 1024 * 1024
    public static let hostName = "sagascript-engine-host"
    public static let knownOps: Set<String> = [
        "hello", "load", "transcribe_window", "cancel", "status", "ping", "unload", "shutdown",
    ]
    /// Fixed `bad_request` texts for dictionaries: terms are personal data and never echoed.
    public static let boostTermsMessage =
        "boost_terms must be an array of at most 500 strings of at most 64 characters"
    public static let boostWeightMessage = "boost_weight must be a number within 0...20"
    public static let maxBoostWeight = 20.0

    // MARK: Parsing

    public static func parse(line: String) -> ParsedLine {
        guard line.utf8.count <= maxLineBytes else {
            return .invalidEnvelope(id: 0, message: "JSON line exceeds 16 MiB")
        }
        guard let data = line.data(using: .utf8),
              let parsed = try? JSONSerialization.jsonObject(with: data),
              let object = parsed as? [String: Any] else {
            return .invalidEnvelope(id: 0, message: "Malformed JSON request")
        }
        guard let id = unsignedInteger(object["id"]), id >= 1 else {
            return .invalidEnvelope(id: 0, message: "missing or invalid id")
        }
        guard let op = object["op"] as? String else {
            return .invalidEnvelope(id: id, message: "missing op")
        }
        guard unsignedInteger(object["v"]) == UInt64(version) else {
            return .invalidEnvelope(id: id, message: "Invalid request envelope")
        }
        guard knownOps.contains(op) else {
            return .unknownOp(id: id, op: op)
        }
        switch parseParameters(op: op, object) {
        case let .success(request): return .request(id: id, op: op, request: request)
        case let .failure(error): return .badParams(id: id, op: op, message: error.message)
        }
    }

    private static func parseParameters(op: String, _ object: [String: Any]) -> Result<WireRequest, EngineHostError> {
        func bad(_ message: String) -> Result<WireRequest, EngineHostError> {
            .failure(EngineHostError(code: .badRequest, message: message))
        }
        switch op {
        case "hello":
            guard let client = object["client"] as? [String: Any],
                  let name = client["name"] as? String,
                  let version = client["version"] as? String,
                  let gitSHA = client["git_sha"] as? String else {
                return bad("hello requires client.name, client.version and client.git_sha")
            }
            return .success(.hello(WireClient(name: name, version: version, gitSHA: gitSHA)))
        case "load":
            guard let modelDirectory = object["model_dir"] as? String,
                  let modelID = object["model_id"] as? String,
                  let computeUnits = object["compute_units"] as? String else {
                return bad("load requires model_dir, model_id and compute_units")
            }
            return .success(.load(modelDirectory: modelDirectory, modelID: modelID, computeUnits: computeUnits))
        case "transcribe_window":
            return parseWindow(object).map { WireRequest.transcribeWindow($0) }
        case "cancel":
            guard let target = unsignedInteger(object["target"]) else { return bad("cancel requires target") }
            return .success(.cancel(target: target))
        case "status": return .success(.status)
        case "ping": return .success(.ping)
        case "unload": return .success(.unload)
        case "shutdown": return .success(.shutdown)
        default: return bad("unknown operation")
        }
    }

    private static func parseWindow(_ object: [String: Any]) -> Result<WireWindowParameters, EngineHostError> {
        // The dictionary first, like the Rust decoder: its texts are fixed and never echo terms.
        let boost: BoostConfig?
        switch parseBoost(object) {
        case let .success(config): boost = config
        case let .failure(error): return .failure(error)
        }
        guard let pcmPath = object["pcm_path"] as? String,
              let offsetSamples = signedInteger(unsignedInteger(object["offset_samples"])),
              let numSamples = signedInteger(unsignedInteger(object["num_samples"])),
              let sampleRate = unsignedInteger(object["sample_rate"]), sampleRate <= UInt64(UInt32.max),
              let format = object["format"] as? String else {
            return .failure(EngineHostError(code: .badRequest, message: "transcribe_window has invalid parameters"))
        }
        guard let priorityName = object["priority"] as? String,
              let priority = WindowPriority(rawValue: priorityName) else {
            return .failure(EngineHostError(code: .badRequest, message: "priority must be interactive or batch"))
        }
        return .success(WireWindowParameters(
            pcmPath: pcmPath,
            offsetSamples: offsetSamples,
            numSamples: numSamples,
            sampleRate: Int(sampleRate),
            format: format,
            priority: priority,
            boost: boost
        ))
    }

    /// Validates `boost_terms` / `boost_weight` (type and bounds, each whenever present, even when
    /// the other is absent) with fixed texts. nil unless there is a term and a positive weight.
    public static func parseBoost(_ object: [String: Any]) -> Result<BoostConfig?, EngineHostError> {
        var terms: [String] = []
        if let raw = object["boost_terms"] {
            guard let array = raw as? [Any], array.count <= BoostConfig.maxTerms else {
                return .failure(EngineHostError(code: .badRequest, message: boostTermsMessage))
            }
            for item in array {
                guard let term = item as? String, term.unicodeScalars.count <= BoostConfig.maxTermLength else {
                    return .failure(EngineHostError(code: .badRequest, message: boostTermsMessage))
                }
                terms.append(term)
            }
        }
        var weight: Float = 0
        if let raw = object["boost_weight"] {
            guard let number = raw as? NSNumber, !isBoolean(number),
                  number.doubleValue.isFinite, number.doubleValue >= 0, number.doubleValue <= maxBoostWeight else {
                return .failure(EngineHostError(code: .badRequest, message: boostWeightMessage))
            }
            weight = number.floatValue
        }
        return .success(!terms.isEmpty && weight > 0 ? BoostConfig(terms: terms, weight: weight) : nil)
    }

    static func isBoolean(_ number: NSNumber) -> Bool {
        CFGetTypeID(number as CFTypeRef) == CFBooleanGetTypeID()
    }

    /// A JSON integer >= 0 (Rust `u64`): not a boolean, not a float such as `2.0`.
    static func unsignedInteger(_ value: Any?) -> UInt64? {
        guard let number = value as? NSNumber, !isBoolean(number),
              !CFNumberIsFloatType(number as CFNumber),
              number.compare(NSNumber(value: 0)) != .orderedAscending else { return nil }
        return number.uint64Value
    }

    private static func signedInteger(_ value: UInt64?) -> Int? {
        guard let value, value <= UInt64(Int.max) else { return nil }
        return Int(value)
    }

    // MARK: Building

    public static func success(id: UInt64, fields: [String: Any]) -> [String: Any] {
        var response: [String: Any] = ["id": id, "ok": true]
        for (key, value) in fields { response[key] = value }
        return response
    }

    public static func failure(id: UInt64, error: EngineHostError) -> [String: Any] {
        [
            "id": id,
            "ok": false,
            "error": ["code": error.code.rawValue, "message": error.message, "retryable": error.retryable],
        ]
    }

    public static func helloResult(host: WireHostIdentity, capabilities: HostCapabilities) -> [String: Any] {
        [
            "protocol": version,
            "host": [
                "name": hostName,
                "version": host.version,
                "git_sha": host.gitSHA,
                "build_dirty": host.dirty,
                "engine": host.engine,
                "engine_version": host.engineVersion,
            ] as [String: Any],
            "capabilities": capabilitiesObject(capabilities),
        ]
    }

    public static func capabilitiesObject(_ capabilities: HostCapabilities) -> [String: Any] {
        var object: [String: Any] = [
            "sample_rate": capabilities.sampleRate,
            "max_window_s": capabilities.maxWindowSeconds,
            "preferred_window_s": capabilities.preferredWindowSeconds,
            "preferred_overlap_s": capabilities.preferredOverlapSeconds,
            "max_in_flight": capabilities.maxInFlight,
            "token_timestamps": capabilities.tokenTimestamps,
            "languages": capabilities.languages,
            "compute_units": capabilities.computeUnits,
            "context_biasing": capabilities.contextBiasing,
        ]
        if let minMacOS = capabilities.minMacOS { object["min_macos"] = minMacOS }
        return object
    }

    public static func loadResult(_ result: LoadResult) -> [String: Any] {
        [
            "model_id": result.modelID,
            "load_ms": milliseconds(result.loadMilliseconds),
            "compiled": result.compiled,
            "window_s": result.windowSeconds,
            "frame_s": result.frameSeconds,
            "vocab_size": result.vocabularySize,
            "blank_id": result.blankID,
        ]
    }

    public static func windowResult(_ result: WindowResult) -> [String: Any] {
        var timings: [String: Any] = [
            "preprocess_ms": milliseconds(result.preprocessMilliseconds),
            "encode_ms": milliseconds(result.encodeMilliseconds),
            "decode_ms": milliseconds(result.decodeMilliseconds),
        ]
        var object: [String: Any] = [
            "tokens": result.tokens.map { token -> [String: Any] in
                [
                    "id": token.id,
                    "text": token.text,
                    "start": token.start,
                    "duration": token.duration,
                    "confidence": token.confidence,
                ]
            },
            "audio_s": result.audioSeconds,
        ]
        if let boostActive = result.boostActive {
            object["boost_active"] = boostActive
            if let boostReason = result.boostReason { object["boost_reason"] = boostReason }
            timings["boost_us"] = result.boostMicroseconds
        }
        object["timings"] = timings
        return object
    }

    public static func statusResult(
        state: WireHostState,
        modelID: String?,
        inFlight: Int,
        rssBytes: Int,
        uptimeSeconds: Double
    ) -> [String: Any] {
        [
            "state": state.rawValue,
            "model_id": modelID.map { $0 as Any } ?? NSNull(),
            "in_flight": inFlight,
            "rss_bytes": rssBytes,
            "uptime_s": uptimeSeconds,
        ]
    }

    public static func cancelResult(cancelled: Bool) -> [String: Any] {
        ["cancelled": cancelled]
    }

    /// One response as JSON (no trailing newline; the writer adds it). Sorted keys keep the output
    /// stable for logs; `[String: Any]` values must be JSON types.
    public static func encode(_ object: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
    }

    /// Milliseconds as a JSON integer (typed clients reject fractions such as 95520.45).
    static func milliseconds(_ value: Double) -> Int {
        guard value.isFinite, value > 0 else { return 0 }
        return Int(value.rounded())
    }
}
