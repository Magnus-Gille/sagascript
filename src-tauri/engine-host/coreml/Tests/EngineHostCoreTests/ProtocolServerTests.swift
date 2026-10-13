import CoreML
import Foundation
import Testing
@testable import SagascriptEngineHostCore

private final class RecordingOutput {
    private let condition = NSCondition()
    private(set) var values: [[String: Any]] = []

    /// Records what a client would read: the response encoded as the host writes it
    /// (`WireProtocol.encode`, as main.swift does) and parsed back.
    func append(_ value: [String: Any]) {
        let line = try! WireProtocol.encode(value)
        precondition(!line.contains(0x0a), "a response must be one line")
        let parsed = try! JSONSerialization.jsonObject(with: line) as! [String: Any]
        condition.lock()
        values.append(parsed)
        condition.broadcast()
        condition.unlock()
    }

    func waitFor(id: UInt64, timeout: TimeInterval = 2) -> [[String: Any]] {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock()
        while !values.contains(where: { ($0["id"] as? NSNumber)?.uint64Value == id }) {
            if !condition.wait(until: deadline) { break }
        }
        let result = values.filter { ($0["id"] as? NSNumber)?.uint64Value == id }
        condition.unlock()
        return result
    }

    func responses(for id: UInt64) -> [[String: Any]] {
        condition.lock(); defer { condition.unlock() }
        return values.filter { ($0["id"] as? NSNumber)?.uint64Value == id }
    }

    func snapshot() -> [[String: Any]] {
        condition.lock(); defer { condition.unlock() }
        return values
    }
}

private final class FakeEngine: EngineBackend {
    let capabilities: HostCapabilities

    init(maxInFlight: Int = 2) {
        capabilities = HostCapabilities(
            maxWindowSeconds: 1,
            preferredWindowSeconds: 1,
            preferredOverlapSeconds: 0.2,
            maxInFlight: maxInFlight
        )
    }
    private let lock = NSLock()
    private let gateCondition = NSCondition()
    private var loaded = false
    private var gateOpen = false
    private var started: [String] = []

    /// Paths containing "gate" block inside `transcribe` until `openGates()`,
    /// which lets a test hold the in-flight slots without any timing assumptions.
    var startedPaths: [String] {
        gateCondition.lock(); defer { gateCondition.unlock() }
        return started
    }

    /// Blocks until `count` requests have entered `transcribe` (10 s safety timeout).
    func waitUntilStarted(count: Int) -> Bool {
        let deadline = Date().addingTimeInterval(10)
        gateCondition.lock(); defer { gateCondition.unlock() }
        while started.count < count {
            if !gateCondition.wait(until: deadline) { return false }
        }
        return true
    }

    func openGates() {
        gateCondition.lock(); gateOpen = true; gateCondition.broadcast(); gateCondition.unlock()
    }

    var loadedModelID: String? { loaded ? "fake" : nil }
    var isLoading: Bool { false }

    func load(modelDirectory: String, modelID: String, computeUnits: String) throws -> LoadResult {
        guard computeUnits == "ane" else {
            throw EngineHostError(code: .badRequest, message: "fake only accepts ane")
        }
        loaded = true
        return LoadResult(
            modelID: modelID,
            loadMilliseconds: 1,
            compiled: false,
            windowSeconds: 1,
            frameSeconds: 0.08,
            vocabularySize: 2,
            blankID: 1
        )
    }

    func transcribe(_ request: WindowRequest, isCancelled: @escaping () -> Bool) throws -> WindowResult {
        guard loaded else { throw EngineHostError(code: .notLoaded, message: "fake not loaded") }
        gateCondition.lock()
        started.append(request.pcmPath)
        gateCondition.broadcast()
        if request.pcmPath.contains("gate") {
            while !gateOpen {
                if isCancelled() { gateCondition.unlock(); throw EngineHostError(code: .cancelled, message: "fake cancelled") }
                _ = gateCondition.wait(until: Date().addingTimeInterval(0.01))
            }
        }
        gateCondition.unlock()
        let delay = request.pcmPath.contains("slow") ? 160_000 : 5_000
        var waited = 0
        while waited < delay {
            if isCancelled() { throw EngineHostError(code: .cancelled, message: "fake cancelled") }
            usleep(5_000)
            waited += 5_000
        }
        return WindowResult(
            tokens: [TranscriptionToken(id: 0, text: "▁ok", start: 0, duration: 0.08, confidence: 1)],
            audioSeconds: Double(request.numSamples) / 16_000,
            preprocessMilliseconds: 1,
            encodeMilliseconds: 2,
            decodeMilliseconds: 3
        )
    }

    func unload() { loaded = false }
}

private func sendRequest(_ server: EngineHostServer, _ object: [String: Any]) {
    let data = try! JSONSerialization.data(withJSONObject: object)
    server.handleLine(String(decoding: data, as: UTF8.self))
}

private func sendHello(_ server: EngineHostServer, id: UInt64 = 1) {
    sendRequest(server, [
        "v": 1, "id": id, "op": "hello",
        "client": ["name": "test", "version": "1", "git_sha": String(repeating: "0", count: 40)],
    ])
}

private func sendWindow(_ server: EngineHostServer, id: UInt64, path: String, priority: String) {
    sendRequest(server, [
        "v": 1, "id": id, "op": "transcribe_window", "pcm_path": path,
        "offset_samples": 0, "num_samples": 1000, "sample_rate": 16_000,
        "format": "f32le", "priority": priority,
    ])
}

@Test func helloMustBeFirstAndMalformedLinesAreProtocolErrors() {
    let output = RecordingOutput()
    let server = EngineHostServer(engine: FakeEngine(), output: output.append)
    sendRequest(server, ["v": 1, "id": 1, "op": "ping"])
    #expect(output.waitFor(id: 1).first?["ok"] as? Bool == false)
    #expect((output.responses(for: 1).first?["error"] as? [String: Any])?["code"] as? String == "protocol")
    server.handleLine("not json")
    #expect(output.waitFor(id: 0).first?["ok"] as? Bool == false)
}

@Test func routingAndImmediatePingStatus() {
    let output = RecordingOutput()
    let fake = FakeEngine()
    let server = EngineHostServer(engine: fake, output: output.append)
    sendHello(server)
    #expect(output.waitFor(id: 1).first?["ok"] as? Bool == true)
    sendRequest(server, ["v": 1, "id": 2, "op": "load", "model_dir": "/tmp/model", "model_id": "fake", "compute_units": "ane"])
    #expect(output.waitFor(id: 2).first?["ok"] as? Bool == true)
    sendWindow(server, id: 3, path: "/tmp/slow", priority: "batch")
    sendRequest(server, ["v": 1, "id": 4, "op": "status"])
    sendRequest(server, ["v": 1, "id": 5, "op": "ping"])
    #expect(output.waitFor(id: 4).first?["ok"] as? Bool == true)
    #expect(output.waitFor(id: 5).first?["ok"] as? Bool == true)
    #expect((output.responses(for: 4).first?["in_flight"] as? NSNumber)?.intValue == 1)
    #expect((output.responses(for: 4).first?["state"] as? String) == "busy")
    #expect(output.waitFor(id: 3).first?["ok"] as? Bool == true)
}

@Test func outOfOrderResponsesAndInteractivePriority() {
    let output = RecordingOutput()
    let fake = FakeEngine()
    let server = EngineHostServer(engine: fake, output: output.append)
    sendHello(server)
    sendRequest(server, ["v": 1, "id": 2, "op": "load", "model_dir": "/tmp/model", "model_id": "fake", "compute_units": "ane"])
    _ = output.waitFor(id: 2)
    // Occupy both in-flight slots with gated requests, and only then queue the
    // contenders, so the scheduling decision cannot race with completions.
    sendWindow(server, id: 10, path: "/tmp/gate-1", priority: "batch")
    sendWindow(server, id: 11, path: "/tmp/gate-2", priority: "batch")
    #expect(fake.waitUntilStarted(count: 2))
    sendWindow(server, id: 12, path: "/tmp/fast-batch", priority: "batch")
    sendWindow(server, id: 13, path: "/tmp/fast-interactive", priority: "interactive")
    fake.openGates()
    _ = output.waitFor(id: 10); _ = output.waitFor(id: 11); _ = output.waitFor(id: 12); _ = output.waitFor(id: 13)
    // The interactive request was queued last but must be started first.
    let started = fake.startedPaths
    #expect(started.count == 4)
    let interactive = started.firstIndex(of: "/tmp/fast-interactive")
    let batch = started.firstIndex(of: "/tmp/fast-batch")
    #expect(interactive != nil && batch != nil)
    #expect(interactive! < batch!)
    for id: UInt64 in [10, 11, 12, 13] {
        #expect(output.responses(for: id).first?["ok"] as? Bool == true)
    }
}

/// The client caps batch windows at `max_in_flight - 1`. With that lane full and
/// gated, an interactive request must start at once, without any batch completing.
@Test func interactiveRequestStartsWhileTheBatchLaneIsFull() {
    let output = RecordingOutput()
    let fake = FakeEngine(maxInFlight: 3)
    let server = EngineHostServer(engine: fake, output: output.append)
    sendHello(server)
    sendRequest(server, ["v": 1, "id": 2, "op": "load", "model_dir": "/tmp/model", "model_id": "fake", "compute_units": "ane"])
    _ = output.waitFor(id: 2)
    sendWindow(server, id: 10, path: "/tmp/gate-1", priority: "batch")
    sendWindow(server, id: 11, path: "/tmp/gate-2", priority: "batch")
    #expect(fake.waitUntilStarted(count: 2))
    sendWindow(server, id: 12, path: "/tmp/fast-interactive", priority: "interactive")
    #expect(fake.waitUntilStarted(count: 3))
    #expect(output.waitFor(id: 12).first?["ok"] as? Bool == true)
    // Both batch windows are still held: the interactive one did not wait for them.
    #expect(output.responses(for: 10).isEmpty && output.responses(for: 11).isEmpty)
    fake.openGates()
    _ = output.waitFor(id: 10); _ = output.waitFor(id: 11)
}

@Test func hostReservesAnInteractiveSlotByDefault() {
    #expect(CoreMLEngine.configuredMaxInFlight(environment: [:]) == 4)
    #expect(CoreMLEngine.configuredMaxInFlight(environment: ["SAGASCRIPT_ENGINE_MAX_IN_FLIGHT": "2"]) == 2)
    #expect(CoreMLEngine.configuredMaxInFlight(environment: ["SAGASCRIPT_ENGINE_MAX_IN_FLIGHT": "99"]) == 8)
}

@Test func cancelProducesAckAndCancelledTerminalResponse() {
    let output = RecordingOutput()
    let server = EngineHostServer(engine: FakeEngine(), output: output.append)
    sendHello(server)
    sendRequest(server, ["v": 1, "id": 2, "op": "load", "model_dir": "/tmp/model", "model_id": "fake", "compute_units": "ane"])
    _ = output.waitFor(id: 2)
    sendWindow(server, id: 20, path: "/tmp/slow-cancel", priority: "interactive")
    sendRequest(server, ["v": 1, "id": 21, "op": "cancel", "target": 20])
    #expect((output.waitFor(id: 21).first?["cancelled"] as? Bool) == true)
    let target = output.waitFor(id: 20).first
    #expect(target?["ok"] as? Bool == false)
    #expect((target?["error"] as? [String: Any])?["code"] as? String == "cancelled")
}

@Test func unsupportedUnloadAndEOF() {
    let output = RecordingOutput()
    let server = EngineHostServer(engine: FakeEngine(), output: output.append)
    sendHello(server)
    sendRequest(server, ["v": 1, "id": 2, "op": "unsupported-op"])
    #expect((output.waitFor(id: 2).first?["error"] as? [String: Any])?["code"] as? String == "unsupported")
    sendRequest(server, ["v": 1, "id": 3, "op": "unload"])
    #expect(output.waitFor(id: 3).first?["ok"] as? Bool == true)
    server.receiveEOF()
    sendRequest(server, ["v": 1, "id": 4, "op": "ping"])
    #expect((output.waitFor(id: 4).first?["error"] as? [String: Any])?["code"] as? String == "protocol")
}

@Test func alignedArrayPadsInnermostDimensionToTile() throws {
    let array = try makeAlignedArray(shape: [1, 1024, 1], dataType: .float32)
    #expect(array.strides.map(\.intValue) == [16384, 16, 1])
    #expect(Int(bitPattern: array.dataPointer) % 64 == 0)
}

@Test func encoderFramesCopiesStridedColumn() throws {
    // [1, hidden=4, time=3]: time is innermost, so its stride is padded to 16.
    let array = try makeAlignedArray(shape: [1, 4, 3], dataType: .float32)
    let pointer = array.dataPointer.bindMemory(to: Float.self, capacity: array.count)
    let strides = array.strides.map(\.intValue)
    for hidden in 0..<4 { for time in 0..<3 { pointer[hidden * strides[1] + time * strides[2]] = Float(hidden * 10 + time) } }
    let frames = try EncoderFrames(array, validLength: 3, hiddenSize: 4)
    var destination = [Float](repeating: 0, count: 4)
    destination.withUnsafeMutableBufferPointer { frames.copyFrame(2, to: $0.baseAddress!, stride: 1) }
    #expect(destination == [2, 12, 22, 32])
}

// MARK: - Golden transcripts

/// The golden transcripts in `src-tauri/engine-host/test-vectors/protocol/`, replayed through
/// `EngineHostServer` with the scripted engine in `fake-engine.json`. The ONNX host replays the
/// same files and the Rust protocol crate decodes them strictly, so a wire field renamed or
/// retyped here fails this test (docs/engine-host-protocol.md, Golden transcripts).
private enum Golden {
    static var directory: URL {
        // .../engine-host/coreml/Tests/EngineHostCoreTests/<this file>
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<4 { url.deleteLastPathComponent() }
        return url.appendingPathComponent("test-vectors/protocol", isDirectory: true)
    }

    /// Every transcript the contract defines; the replay fails when one is missing.
    static let transcriptNames = [
        "cancel.jsonl", "context-biasing.jsonl", "eof.jsonl", "handshake.jsonl", "load.jsonl",
        "shutdown.jsonl", "transcribe.jsonl",
    ]

    static func json(_ data: Data) throws -> Any {
        try JSONSerialization.jsonObject(with: data)
    }

    static func transcripts() throws -> [(name: String, steps: [[String: Any]])] {
        let names = try FileManager.default.contentsOfDirectory(atPath: directory.path)
            .filter { $0.hasSuffix(".jsonl") }
            .sorted()
        return try names.map { name -> (name: String, steps: [[String: Any]]) in
            let text = try String(contentsOf: directory.appendingPathComponent(name), encoding: .utf8)
            let steps = try text.split(separator: "\n").map { line -> [String: Any] in
                guard let step = try json(Data(line.utf8)) as? [String: Any] else {
                    throw EngineHostError(code: .internalError, message: "\(name): a step must be an object")
                }
                return step
            }
            return (name, steps)
        }
    }

    static func isFloat(_ number: NSNumber) -> Bool { CFNumberIsFloatType(number as CFNumber) }

    /// Same rules as the Rust `golden::matches`: identical key sets, equal arrays, integers stay
    /// integers, fractions match within a relative 1e-6, and placeholders for host-specific values.
    static func difference(_ expected: Any, _ actual: Any, at path: String = "$") -> String? {
        let mismatch = "\(path): expected \(expected), got \(actual)"
        if let name = expected as? String, name.hasPrefix("<"), name.hasSuffix(">") {
            return placeholderDifference(name, actual, at: path)
        }
        if let expected = expected as? [String: Any] {
            guard let actual = actual as? [String: Any] else { return mismatch }
            let missing = Set(expected.keys).subtracting(actual.keys).sorted()
            let extra = Set(actual.keys).subtracting(expected.keys).sorted()
            if !missing.isEmpty || !extra.isEmpty {
                return "\(path): missing fields \(missing), unexpected fields \(extra)"
            }
            for key in expected.keys.sorted() {
                if let found = difference(expected[key]!, actual[key]!, at: "\(path).\(key)") { return found }
            }
            return nil
        }
        if let expected = expected as? [Any] {
            guard let actual = actual as? [Any], actual.count == expected.count else { return mismatch }
            for index in expected.indices {
                if let found = difference(expected[index], actual[index], at: "\(path)[\(index)]") { return found }
            }
            return nil
        }
        if expected is NSNull { return actual is NSNull ? nil : mismatch }
        if let expected = expected as? String { return (actual as? String) == expected ? nil : mismatch }
        guard let expected = expected as? NSNumber, let actual = actual as? NSNumber else { return mismatch }
        if WireProtocol.isBoolean(expected) || WireProtocol.isBoolean(actual) {
            let same = WireProtocol.isBoolean(expected) && WireProtocol.isBoolean(actual)
                && expected.boolValue == actual.boolValue
            return same ? nil : mismatch
        }
        if isFloat(expected) {
            let tolerance = 1e-6 * max(1, abs(expected.doubleValue))
            return abs(expected.doubleValue - actual.doubleValue) <= tolerance ? nil : mismatch
        }
        return !isFloat(actual) && expected.compare(actual) == .orderedSame ? nil : mismatch
    }

    static func placeholderDifference(_ name: String, _ actual: Any, at path: String) -> String? {
        let number = actual as? NSNumber
        let isNumber = number.map { !WireProtocol.isBoolean($0) } ?? false
        let ok: Bool
        switch name {
        case "<string>": ok = actual is String
        case "<sha40>":
            let text = actual as? String ?? ""
            ok = text.count == 40 && text.allSatisfy { "0123456789abcdef".contains($0) }
        case "<bool>": ok = number.map(WireProtocol.isBoolean) ?? false
        case "<uint>": ok = isNumber && !isFloat(number!) && number!.int64Value >= 0
        case "<number>": ok = isNumber && number!.doubleValue.isFinite && number!.doubleValue >= 0
        default: return "\(path): unknown placeholder \(name)"
        }
        return ok ? nil : "\(path): expected \(name), got \(actual)"
    }
}

/// The scripted engine of `fake-engine.json` (the ONNX host's tests implement the same rules).
private final class GoldenEngine: EngineBackend {
    let capabilities: HostCapabilities
    private let spec: [String: Any]
    private let lock = NSLock()
    private var model: String?

    init(spec: [String: Any]) {
        self.spec = spec
        let caps = spec["capabilities"] as! [String: Any]
        capabilities = HostCapabilities(
            sampleRate: (caps["sample_rate"] as! NSNumber).intValue,
            maxWindowSeconds: (caps["max_window_s"] as! NSNumber).doubleValue,
            preferredWindowSeconds: (caps["preferred_window_s"] as! NSNumber).doubleValue,
            preferredOverlapSeconds: (caps["preferred_overlap_s"] as! NSNumber).doubleValue,
            maxInFlight: (caps["max_in_flight"] as! NSNumber).intValue,
            tokenTimestamps: (caps["token_timestamps"] as! NSNumber).boolValue,
            languages: caps["languages"] as! [String],
            computeUnits: caps["compute_units"] as! [String],
            minMacOS: caps["min_macos"] as? String,
            contextBiasing: (caps["context_biasing"] as! NSNumber).boolValue
        )
    }

    var loadedModelID: String? {
        lock.lock(); defer { lock.unlock() }
        return model
    }

    var isLoading: Bool { false }

    private func failure(_ object: Any?) -> EngineHostError {
        let error = object as! [String: Any]
        return EngineHostError(
            code: WireErrorCode(rawValue: error["code"] as! String)!,
            message: error["message"] as! String,
            retryable: (error["retryable"] as! NSNumber).boolValue
        )
    }

    func load(modelDirectory: String, modelID: String, computeUnits: String) throws -> LoadResult {
        if let error = (spec["load_errors"] as! [String: Any])[modelDirectory] { throw failure(error) }
        let result = spec["load_result"] as! [String: Any]
        lock.lock(); model = modelID; lock.unlock()
        return LoadResult(
            modelID: modelID,
            loadMilliseconds: (result["load_ms"] as! NSNumber).doubleValue,
            compiled: (result["compiled"] as! NSNumber).boolValue,
            windowSeconds: (result["window_s"] as! NSNumber).doubleValue,
            frameSeconds: (result["frame_s"] as! NSNumber).doubleValue,
            vocabularySize: (result["vocab_size"] as! NSNumber).intValue,
            blankID: (result["blank_id"] as! NSNumber).intValue
        )
    }

    func transcribe(_ request: WindowRequest, isCancelled: @escaping () -> Bool) throws -> WindowResult {
        guard loadedModelID != nil else { throw failure(spec["not_loaded"]) }
        guard let window = (spec["windows"] as! [String: Any])[request.pcmPath] as? [String: Any] else {
            throw EngineHostError(code: .engine, message: "fake engine has no window \(request.pcmPath)")
        }
        let expected = window["expect_boost"] as? [String: Any]
        let boostMatches: Bool
        if let expected, let boost = request.boost {
            boostMatches = boost.terms == (expected["terms"] as! [String])
                && boost.weight == (expected["weight"] as! NSNumber).floatValue
        } else {
            boostMatches = expected == nil && request.boost == nil
        }
        guard boostMatches else {
            throw EngineHostError(code: .internalError, message: "fake engine got an unexpected dictionary for \(request.pcmPath)")
        }
        if (window["hold"] as? NSNumber)?.boolValue == true {
            let deadline = Date().addingTimeInterval(10)
            while !isCancelled() {
                if Date() > deadline { throw EngineHostError(code: .internalError, message: "held window never cancelled") }
                usleep(1_000)
            }
            throw EngineHostError(code: .cancelled, message: "Transcription was cancelled")
        }
        if let error = window["error"] { throw failure(error) }
        let result = window["result"] as! [String: Any]
        let timings = result["timings"] as! [String: Any]
        let tokens = (result["tokens"] as! [[String: Any]]).map { token in
            TranscriptionToken(
                id: (token["id"] as! NSNumber).intValue,
                text: token["text"] as! String,
                start: (token["start"] as! NSNumber).doubleValue,
                duration: (token["duration"] as! NSNumber).doubleValue,
                confidence: (token["confidence"] as! NSNumber).doubleValue
            )
        }
        return WindowResult(
            tokens: tokens,
            audioSeconds: (result["audio_s"] as! NSNumber).doubleValue,
            preprocessMilliseconds: (timings["preprocess_ms"] as! NSNumber).doubleValue,
            encodeMilliseconds: (timings["encode_ms"] as! NSNumber).doubleValue,
            decodeMilliseconds: (timings["decode_ms"] as! NSNumber).doubleValue,
            boostActive: (result["boost_active"] as? NSNumber)?.boolValue,
            boostReason: result["boost_reason"] as? String,
            boostMicroseconds: (timings["boost_us"] as? NSNumber)?.intValue ?? 0
        )
    }

    func unload() {
        lock.lock(); model = nil; lock.unlock()
    }
}

private final class ShutdownFlag {
    private let condition = NSCondition()
    private var done = false

    func set() {
        condition.lock(); done = true; condition.broadcast(); condition.unlock()
    }

    func wait(timeout: TimeInterval = 5) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        condition.lock(); defer { condition.unlock() }
        while !done {
            if !condition.wait(until: deadline) { return done }
        }
        return true
    }
}

private extension RecordingOutput {
    /// The first response with `id` that no `expect` has matched yet (waits up to 5 s).
    func take(id: UInt64, consumed: inout Set<Int>) -> [String: Any]? {
        let deadline = Date().addingTimeInterval(5)
        while true {
            let all = snapshot()
            if let index = all.indices.first(where: { index in
                !consumed.contains(index) && all[index]["event"] == nil
                    && (all[index]["id"] as? NSNumber)?.uint64Value == id
            }) {
                consumed.insert(index)
                return all[index]
            }
            if Date() > deadline { return nil }
            usleep(2_000)
        }
    }
}

/// Replays one transcript; returns the first difference, or nil.
private func replay(_ name: String, _ steps: [[String: Any]], spec: [String: Any]) -> String? {
    let output = RecordingOutput()
    let exited = ShutdownFlag()
    let server = EngineHostServer(engine: GoldenEngine(spec: spec), output: output.append, onShutdown: exited.set)
    var consumed = Set<Int>()
    for (index, step) in steps.enumerated() {
        let at = "\(name) step \(index)"
        if let request = step["send"] {
            let line = try! JSONSerialization.data(withJSONObject: request)
            server.handleLine(String(decoding: line, as: UTF8.self))
        } else if let raw = step["send_raw"] as? String {
            server.handleLine(raw)
        } else if let expected = step["expect"] as? [String: Any] {
            let id = (expected["id"] as! NSNumber).uint64Value
            guard let actual = output.take(id: id, consumed: &consumed) else {
                return "\(at): no response with id \(id); the host wrote \(output.snapshot())"
            }
            if let found = Golden.difference(expected, actual) {
                return "\(at): \(found)\n  expected \(expected)\n  actual   \(actual)"
            }
        } else if step["eof"] != nil {
            server.receiveEOF()
        } else if let status = step["exit"] {
            guard (status as? NSNumber)?.intValue == 0 else { return "\(at): hosts exit 0, the transcript expects \(status)" }
            guard exited.wait() else { return "\(at): the host did not shut down" }
        } else if step["note"] == nil {
            return "\(at): unknown step \(step)"
        }
    }
    for (index, line) in output.snapshot().enumerated() where !consumed.contains(index) && line["event"] == nil {
        return "\(name): response the transcript does not expect: \(line)"
    }
    return nil
}

@Test func goldenTranscriptsReplayThroughTheServer() throws {
    let spec = try Golden.json(Data(contentsOf: Golden.directory.appendingPathComponent("fake-engine.json"))) as! [String: Any]
    let transcripts = try Golden.transcripts()
    for required in Golden.transcriptNames {
        #expect(transcripts.contains { $0.name == required }, "missing \(required)")
    }
    for transcript in transcripts {
        let found = replay(transcript.name, transcript.steps, spec: spec)
        #expect(found == nil, "\(found ?? "")")
    }
}

@Test func everyWireErrorCodeIsInTheTranscripts() throws {
    var codes = Set<String>()
    for transcript in try Golden.transcripts() {
        for step in transcript.steps {
            if let error = (step["expect"] as? [String: Any])?["error"] as? [String: Any] {
                codes.insert(error["code"] as! String)
            }
        }
    }
    #expect(codes == Set(WireErrorCode.allCases.map(\.rawValue)))
}
