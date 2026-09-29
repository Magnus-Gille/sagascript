import CoreML
import Foundation
import Testing
@testable import SagascriptEngineHostCore

private final class RecordingOutput {
    private let condition = NSCondition()
    private(set) var values: [[String: Any]] = []

    func append(_ value: [String: Any]) {
        condition.lock()
        values.append(value)
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
}

private final class FakeEngine: EngineBackend {
    let capabilities = HostCapabilities(
        maxWindowSeconds: 1,
        preferredWindowSeconds: 1,
        preferredOverlapSeconds: 0.2,
        maxInFlight: 2
    )
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
            throw EngineHostError(code: "bad_request", message: "fake only accepts ane")
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
        guard loaded else { throw EngineHostError(code: "not_loaded", message: "fake not loaded") }
        gateCondition.lock()
        started.append(request.pcmPath)
        gateCondition.broadcast()
        if request.pcmPath.contains("gate") {
            while !gateOpen {
                if isCancelled() { gateCondition.unlock(); throw EngineHostError(code: "cancelled", message: "fake cancelled") }
                _ = gateCondition.wait(until: Date().addingTimeInterval(0.01))
            }
        }
        gateCondition.unlock()
        let delay = request.pcmPath.contains("slow") ? 160_000 : 5_000
        var waited = 0
        while waited < delay {
            if isCancelled() { throw EngineHostError(code: "cancelled", message: "fake cancelled") }
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
    sendRequest(server, ["v": 1, "id": 1, "op": "hello", "client": ["name": "test"]])
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
    sendRequest(server, ["v": 1, "id": 1, "op": "hello", "client": [:]])
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

@Test func cancelProducesAckAndCancelledTerminalResponse() {
    let output = RecordingOutput()
    let server = EngineHostServer(engine: FakeEngine(), output: output.append)
    sendRequest(server, ["v": 1, "id": 1, "op": "hello", "client": [:]])
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
    sendRequest(server, ["v": 1, "id": 1, "op": "hello", "client": [:]])
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
