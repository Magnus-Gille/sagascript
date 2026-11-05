import CoreML
import Dispatch
import Foundation
import Testing
@testable import SagascriptEngineHostCore

// `warm` (#397): the canary is a request of its own that never touches the window scheduler. The
// server tests use the gated FakeEngine of ProtocolServerTests.swift; the golden transcript warm.jsonl
// is replayed there too.

/// A clock the test moves by hand (`HostClock.continuousSeconds` in production).
private final class ManualClock {
    private let lock = NSLock()
    private var seconds: TimeInterval

    init(_ start: TimeInterval = 1_000) { seconds = start }

    func read() -> TimeInterval {
        lock.lock(); defer { lock.unlock() }
        return seconds
    }

    func advance(_ delta: TimeInterval) {
        lock.lock(); seconds += delta; lock.unlock()
    }
}

/// The stderr lines the coordinator wrote.
private final class LogLines {
    private let condition = NSCondition()
    private var lines: [String] = []

    func append(_ line: String) {
        condition.lock(); lines.append(line); condition.broadcast(); condition.unlock()
    }

    var all: [String] {
        condition.lock(); defer { condition.unlock() }
        return lines
    }

    /// Waits until `count` lines exist (5 s safety timeout) and returns all lines.
    func waitFor(count: Int) -> [String] {
        let deadline = Date().addingTimeInterval(5)
        condition.lock(); defer { condition.unlock() }
        while lines.count < count {
            if !condition.wait(until: deadline) { break }
        }
        return lines
    }
}

private struct CanaryBoom: Error, LocalizedError {
    var errorDescription: String? { "boom" }
}

/// A server over the gated FakeEngine with an injected clock and log, past `hello` (and `load`).
private final class WarmHarness {
    let output = RecordingOutput()
    let fake: FakeEngine
    let clock = ManualClock()
    let log = LogLines()
    let server: EngineHostServer

    init(maxInFlight: Int = 2, load: Bool = true) {
        let fake = FakeEngine(maxInFlight: maxInFlight)
        let output = self.output
        let clock = self.clock
        let log = self.log
        self.fake = fake
        server = EngineHostServer(engine: fake, output: output.append, now: clock.read, warmLog: log.append)
        sendHello(server)
        _ = output.waitFor(id: 1)
        if load { loadModel() }
    }

    func loadModel(id: UInt64 = 2) {
        sendRequest(server, ["v": 1, "id": id, "op": "load", "model_dir": "/tmp/model", "model_id": "fake", "compute_units": "ane"])
        _ = output.waitFor(id: id)
    }

    func warm(id: UInt64) {
        sendRequest(server, ["v": 1, "id": id, "op": "warm"])
    }

    /// The single response to request `id`.
    func response(_ id: UInt64) -> [String: Any] {
        output.waitFor(id: id).first ?? [:]
    }

    func skipped(_ id: UInt64) -> String? {
        response(id)["skipped"] as? String
    }
}

private func errorCode(_ response: [String: Any]) -> String? {
    (response["error"] as? [String: Any])?["code"] as? String
}

// MARK: - The canary next to windows

@Test func aHeldCanaryDoesNotDelayAnInteractiveWindow() {
    let h = WarmHarness()
    h.fake.holdCanaries()
    h.warm(id: 10)
    #expect(h.fake.waitUntilCanaryStarted(count: 1))
    // A gated batch window takes one of the two slots; the canary took none.
    sendWindow(h.server, id: 11, path: "/tmp/gate-batch", priority: "batch")
    #expect(h.fake.waitUntilStarted(count: 1))
    sendWindow(h.server, id: 12, path: "/tmp/fast-interactive", priority: "interactive")
    #expect(h.response(12)["ok"] as? Bool == true)
    // The interactive window was answered while the canary was still running.
    #expect(h.output.responses(for: 10).isEmpty)

    sendRequest(h.server, ["v": 1, "id": 13, "op": "status"])
    #expect((h.response(13)["in_flight"] as? NSNumber)?.intValue == 1)
    // cancel does not know the canary: it cannot be interrupted, and it is harmless.
    sendRequest(h.server, ["v": 1, "id": 14, "op": "cancel", "target": 10])
    #expect(h.response(14)["cancelled"] as? Bool == false)
    #expect(h.output.responses(for: 10).isEmpty)

    h.fake.openCanaryGate()
    let canary = h.response(10)
    #expect(canary["ok"] as? Bool == true)
    #expect(canary["ran"] as? Bool == true)
    h.fake.openGates()
    #expect(h.response(11)["ok"] as? Bool == true)
}

@Test func aSecondWarmWhileOneRunsIsSkippedAtOnce() {
    let h = WarmHarness()
    h.fake.holdCanaries()
    h.warm(id: 10)
    #expect(h.fake.waitUntilCanaryStarted(count: 1))
    h.warm(id: 11)
    // Answered at once, while the first canary is still held.
    #expect(h.response(11)["ran"] as? Bool == false)
    #expect(h.skipped(11) == "in_progress")
    #expect(h.output.responses(for: 10).isEmpty)
    #expect(h.fake.canaryCount == 1)
    // A background trigger sees the same rule and says so.
    h.server.backgroundWarm(.keepAlive)
    #expect(h.log.all == ["warm canary trigger=keep_alive skipped=in_progress"])

    h.fake.openCanaryGate()
    let first = h.response(10)
    #expect(first["ran"] as? Bool == true)
    #expect((first["canary_ms"] as? NSNumber)?.intValue == 7)
    #expect(first["probable_recompile"] as? Bool == false)
    #expect(h.fake.canaryCount == 1)
}

// MARK: - Skip rules

@Test func recentAppliesWithinTheIntervalAndTheCanaryRunsAgainAfterIt() {
    let h = WarmHarness()
    h.warm(id: 10)
    #expect(h.response(10)["ran"] as? Bool == true)

    h.clock.advance(29)
    h.warm(id: 11)
    #expect(h.skipped(11) == "recent")
    // A skipped canary is not activity: the interval counts from the canary that ran.
    h.clock.advance(1)
    h.warm(id: 12)
    #expect(h.response(12)["ran"] as? Bool == true)
    #expect(h.fake.canaryCount == 2)

    h.clock.advance(29)
    h.warm(id: 13)
    #expect(h.skipped(13) == "recent")
}

@Test func theRecentIntervalIsConfigurable() {
    let fake = FakeEngine()
    let output = RecordingOutput()
    let clock = ManualClock()
    let server = EngineHostServer(engine: fake, output: output.append, recentInterval: 5, now: clock.read, warmLog: { _ in })
    sendHello(server)
    sendRequest(server, ["v": 1, "id": 2, "op": "load", "model_dir": "/tmp/model", "model_id": "fake", "compute_units": "ane"])
    _ = output.waitFor(id: 2)
    sendRequest(server, ["v": 1, "id": 3, "op": "warm"])
    #expect(output.waitFor(id: 3).first?["ran"] as? Bool == true)
    clock.advance(5)
    sendRequest(server, ["v": 1, "id": 4, "op": "warm"])
    #expect(output.waitFor(id: 4).first?["ran"] as? Bool == true)
}

@Test func aFinishedWindowCountsAsActivity() {
    let h = WarmHarness()
    sendWindow(h.server, id: 10, path: "/tmp/fast", priority: "interactive")
    #expect(h.response(10)["ok"] as? Bool == true)
    h.warm(id: 11)
    #expect(h.skipped(11) == "recent")
    #expect(h.fake.canaryCount == 0)

    h.clock.advance(30)
    h.warm(id: 12)
    #expect(h.response(12)["ran"] as? Bool == true)
}

@Test func skipRulesAreCheckedInProtocolOrder() {
    let h = WarmHarness(load: false)
    h.warm(id: 10)
    #expect(h.skipped(10) == "not_loaded")
    h.loadModel()
    h.warm(id: 11)
    #expect(h.response(11)["ran"] as? Bool == true)

    // Recent now, but a window in flight is reported first.
    sendWindow(h.server, id: 12, path: "/tmp/gate-1", priority: "batch")
    #expect(h.fake.waitUntilStarted(count: 1))
    h.warm(id: 13)
    #expect(h.skipped(13) == "busy")
    h.fake.openGates()
    #expect(h.response(12)["ok"] as? Bool == true)
    h.warm(id: 14)
    #expect(h.skipped(14) == "recent")

    // Recent still, but nothing is loaded: reported first.
    sendRequest(h.server, ["v": 1, "id": 15, "op": "unload"])
    _ = h.response(15)
    h.warm(id: 16)
    #expect(h.skipped(16) == "not_loaded")
    #expect(h.fake.canaryCount == 1)
}

@Test func warmIgnoresUnknownFieldsAndNeedsHello() {
    let output = RecordingOutput()
    let server = EngineHostServer(engine: FakeEngine(), output: output.append, warmLog: { _ in })
    sendRequest(server, ["v": 1, "id": 1, "op": "warm"])
    #expect(errorCode(output.waitFor(id: 1).first ?? [:]) == "protocol")
    sendHello(server, id: 2)
    _ = output.waitFor(id: 2)
    sendRequest(server, ["v": 1, "id": 3, "op": "warm", "reason": "key_down", "n": [1, 2]])
    #expect(output.waitFor(id: 3).first?["skipped"] as? String == "not_loaded")
}

// MARK: - Failures

@Test func aThrowingCanaryIsAnsweredWithTheEnginesError() {
    let h = WarmHarness()
    h.fake.setCanaryOutcome(.failure(EngineHostError(code: .busy, message: "the engine said no", retryable: true)))
    h.warm(id: 10)
    let failure = h.response(10)
    #expect(failure["ok"] as? Bool == false)
    let error = failure["error"] as? [String: Any]
    #expect(error?["code"] as? String == "busy")
    #expect(error?["message"] as? String == "the engine said no")
    #expect(error?["retryable"] as? Bool == true)
    // The model was exercised, so a failed canary is activity too.
    h.warm(id: 11)
    #expect(h.skipped(11) == "recent")
}

@Test func aCanaryThatThrowsAnotherErrorIsAnInternalError() {
    let h = WarmHarness()
    h.fake.setCanaryOutcome(.failure(CanaryBoom()))
    h.warm(id: 10)
    let error = h.response(10)["error"] as? [String: Any]
    #expect(error?["code"] as? String == "internal")
    #expect(error?["message"] as? String == "boom")
    #expect(error?["retryable"] as? Bool == false)
}

// MARK: - Background canaries

@Test func backgroundWarmRunsTheCanaryAndLogsIt() {
    let h = WarmHarness()
    h.fake.setCanaryOutcome(.success(CanaryResult(milliseconds: 12.4, probableRecompile: false)))
    h.server.backgroundWarm(.memoryPressure)
    #expect(h.log.waitFor(count: 1) == ["warm canary trigger=memory_pressure ms=12 probable_recompile=false"])
    #expect(h.fake.canaryCount == 1)

    // A probable recompile is prefixed `info: ` (the client logs only those at INFO).
    h.clock.advance(60)
    h.fake.setCanaryOutcome(.success(CanaryResult(milliseconds: 13_700.4, probableRecompile: true)))
    h.server.backgroundWarm(.wake)
    #expect(h.log.waitFor(count: 2) == [
        "warm canary trigger=memory_pressure ms=12 probable_recompile=false",
        "info: warm canary trigger=wake ms=13700 probable_recompile=true",
    ])
}

@Test func aCanaryRequestLogsWhenItRanButNotWhenSkipped() {
    let h = WarmHarness()
    h.warm(id: 10)
    #expect(h.response(10)["ran"] as? Bool == true)
    #expect(h.log.all == ["warm canary trigger=request ms=7 probable_recompile=false"])
    h.warm(id: 11)
    #expect(h.skipped(11) == "recent")
    // The response is written after the log decision: nothing was logged for the skip.
    #expect(h.log.all.count == 1)
}

@Test func backgroundWarmSkipsWhileAWindowIsHeldAndLogsWhy() {
    let h = WarmHarness()
    sendWindow(h.server, id: 10, path: "/tmp/gate-1", priority: "batch")
    #expect(h.fake.waitUntilStarted(count: 1))
    h.server.backgroundWarm(.keepAlive)
    #expect(h.log.waitFor(count: 1) == ["warm canary trigger=keep_alive skipped=busy"])
    // A queued window keeps the model busy too.
    sendWindow(h.server, id: 11, path: "/tmp/gate-2", priority: "batch")
    sendWindow(h.server, id: 12, path: "/tmp/fast", priority: "batch")
    h.server.backgroundWarm(.wake)
    #expect(h.log.waitFor(count: 2).last == "warm canary trigger=wake skipped=busy")
    #expect(h.fake.canaryCount == 0)

    h.fake.openGates()
    for id: UInt64 in [10, 11, 12] { #expect(h.response(id)["ok"] as? Bool == true) }
    // The windows ran moments ago: recent, and still no canary.
    h.server.backgroundWarm(.keepAlive)
    #expect(h.log.waitFor(count: 3).last == "warm canary trigger=keep_alive skipped=recent")
    #expect(h.fake.canaryCount == 0)
}

@Test func backgroundWarmWithoutAModelLogsNotLoaded() {
    let h = WarmHarness(load: false)
    h.server.backgroundWarm(.keepAlive)
    #expect(h.log.waitFor(count: 1) == ["warm canary trigger=keep_alive skipped=not_loaded"])
}

@Test func aFailedBackgroundCanaryLogsTheErrorCodeOnly() {
    let h = WarmHarness()
    h.fake.setCanaryOutcome(.failure(EngineHostError(code: .engine, message: "/Users/someone/Model.mlmodelc failed")))
    h.server.backgroundWarm(.memoryPressure)
    #expect(h.log.waitFor(count: 1) == ["warm canary trigger=memory_pressure failed=engine"])
}

@Test func backgroundWarmDoesNothingOnceTheHostIsStopping() {
    let h = WarmHarness()
    h.server.receiveEOF()
    h.server.backgroundWarm(.wake)
    // Nothing to wait for, so a short grace period: a canary started by mistake would have
    // entered the engine and logged by now.
    Thread.sleep(forTimeInterval: 0.1)
    #expect(h.fake.canaryCount == 0)
    #expect(h.log.all.isEmpty)
}

// MARK: - Wire

@Test func warmIsAKnownOpWithoutParameters() {
    guard case let .request(id, op, request) = WireProtocol.parse(line: #"{"v":1,"id":5,"op":"warm","x":{"y":1}}"#) else {
        Issue.record("warm did not parse as a request")
        return
    }
    #expect(id == 5 && op == "warm")
    guard case .warm = request else {
        Issue.record("warm parsed as another request")
        return
    }
}

@Test func warmResultsEncodeIntegerMillisecondsAndTheSkipReason() throws {
    func line(_ outcome: WarmOutcome) throws -> String {
        String(decoding: try WireProtocol.encode(WireProtocol.warmResult(outcome)), as: UTF8.self)
    }
    #expect(try line(.ran(CanaryResult(milliseconds: 13_699.6, probableRecompile: true)))
        == #"{"canary_ms":13700,"probable_recompile":true,"ran":true}"#)
    #expect(try line(.ran(CanaryResult(milliseconds: 38, probableRecompile: false)))
        == #"{"canary_ms":38,"probable_recompile":false,"ran":true}"#)
    for (reason, name) in [
        (WarmSkipReason.inProgress, "in_progress"), (.notLoaded, "not_loaded"), (.busy, "busy"), (.recent, "recent"),
    ] {
        #expect(try line(.skipped(reason)) == #"{"ran":false,"skipped":"\#(name)"}"#)
    }
}

@Test func triggersSpellTheirStderrNames() {
    #expect(WarmTrigger.request.rawValue == "request")
    #expect(WarmTrigger.memoryPressure.rawValue == "memory_pressure")
    #expect(WarmTrigger.wake.rawValue == "wake")
    #expect(WarmTrigger.keepAlive.rawValue == "keep_alive")
}

// MARK: - Pure decisions

/// `#expect` cannot call a mutating member, so the tick is taken first.
private func tick(_ detector: inout SleepDetector, _ continuous: TimeInterval, _ uptime: TimeInterval) -> Bool {
    detector.tick(continuous: continuous, uptime: uptime)
}

@Test func sleepDetectorSeesTheContinuousClockRunningAheadOfUptime() {
    var detector = SleepDetector()
    // The first tick is only the baseline, whatever the clocks' origins.
    #expect(!tick(&detector, 100, 50))
    #expect(!tick(&detector, 101, 51))
    // A busy machine delaying a tick moves both clocks alike.
    #expect(!tick(&detector, 111, 61))
    // Exactly 5 s ahead is not enough, more is a sleep.
    #expect(!tick(&detector, 117, 62))
    #expect(tick(&detector, 140, 63))
    // One tick later the lead is already accounted for.
    #expect(!tick(&detector, 141, 64))

    var boundary = SleepDetector()
    _ = tick(&boundary, 0, 0)
    #expect(tick(&boundary, 5.5, 0))
    var exactly = SleepDetector()
    _ = tick(&exactly, 0, 0)
    #expect(!tick(&exactly, 5, 0))
}

@Test func memoryPressurePolicySchedulesAndSupersedes() {
    var policy = MemoryPressurePolicy()
    let warning = policy.record(.warning)
    #expect(warning?.delay == 10)
    #expect(warning.map(policy.shouldRun) == true)

    // Critical by the time the warning's 10 s are up: nothing runs, and critical schedules nothing.
    let critical = policy.record(.critical)
    #expect(critical == nil)
    #expect(warning.map(policy.shouldRun) == false)

    // Back to normal: 5 s later; it also stays valid until the next event.
    let normal = policy.record(.normal)
    #expect(normal?.delay == 5)
    #expect(normal.map(policy.shouldRun) == true)
    #expect(warning.map(policy.shouldRun) == false)

    // A newer event of any kind supersedes a pending one, also a repeated warning.
    let first = policy.record(.warning)
    let second = policy.record(.warning)
    #expect(first.map(policy.shouldRun) == false)
    #expect(second.map(policy.shouldRun) == true)
    _ = policy.record(.normal)
    #expect(second.map(policy.shouldRun) == false)
}

@Test func memoryPressureLevelTakesTheMostSevereBit() {
    #expect(MemoryPressureLevel([.normal]) == .normal)
    #expect(MemoryPressureLevel([.warning]) == .warning)
    #expect(MemoryPressureLevel([.critical]) == .critical)
    #expect(MemoryPressureLevel([.normal, .warning]) == .warning)
    #expect(MemoryPressureLevel([.warning, .critical]) == .critical)
}

@Test func keepAliveSettingIsOffUnlessAValidIntervalIsSet() {
    #expect(KeepAliveSetting(environmentValue: nil) == .off)
    #expect(KeepAliveSetting(environmentValue: "60") == .every(seconds: 60))
    #expect(KeepAliveSetting(environmentValue: "3600") == .every(seconds: 3600))
    for value in ["59", "0", "-60", "", "abc", "60.5", " 60", "1e2"] {
        #expect(KeepAliveSetting(environmentValue: value) == .invalid(value), "\(value)")
    }
}

@Test func onlyANeuralEngineEncoderCanProbablyHaveRecompiled() {
    let ane = MLComputeUnits.cpuAndNeuralEngine
    #expect(!CoreMLEngine.isProbableRecompile(encodeMilliseconds: 999.9, encoderComputeUnits: ane))
    #expect(CoreMLEngine.isProbableRecompile(encodeMilliseconds: 1000, encoderComputeUnits: ane))
    #expect(CoreMLEngine.isProbableRecompile(encodeMilliseconds: 16_700, encoderComputeUnits: .all))
    #expect(!CoreMLEngine.isProbableRecompile(encodeMilliseconds: 40, encoderComputeUnits: .all))
    #expect(!CoreMLEngine.isProbableRecompile(encodeMilliseconds: 16_700, encoderComputeUnits: .cpuAndGPU))
    #expect(!CoreMLEngine.isProbableRecompile(encodeMilliseconds: 16_700, encoderComputeUnits: .cpuOnly))
}

@Test func hostClocksAreMonotonicAndTheContinuousOneNeverTrailsUptime() {
    let first = HostClock.continuousSeconds()
    let uptime = HostClock.uptimeSeconds()
    #expect(first > 0 && uptime > 0)
    #expect(HostClock.continuousSeconds() >= first)
    // Continuous time includes the time asleep, so it can only be ahead of the uptime clock.
    #expect(HostClock.continuousSeconds() >= uptime)
}
