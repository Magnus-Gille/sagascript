import Foundation

private final class CancellationFlag {
    private let lock = NSLock()
    private var value = false

    var isCancelled: Bool {
        lock.lock(); defer { lock.unlock() }
        return value
    }

    func cancel() {
        lock.lock(); value = true; lock.unlock()
    }
}

private final class ScheduledJob {
    let id: UInt64
    let priority: String
    let sequence: UInt64
    let flag = CancellationFlag()
    let work: (CancellationFlag) throws -> [String: Any]
    let completion: (Result<[String: Any], EngineHostError>) -> Void

    init(
        id: UInt64,
        priority: String,
        sequence: UInt64,
        work: @escaping (CancellationFlag) throws -> [String: Any],
        completion: @escaping (Result<[String: Any], EngineHostError>) -> Void
    ) {
        self.id = id
        self.priority = priority
        self.sequence = sequence
        self.work = work
        self.completion = completion
    }
}

private final class RequestScheduler {
    private let lock = NSLock()
    private let queue: DispatchQueue
    private let maximum: Int
    private var nextSequence: UInt64 = 0
    private var pending: [ScheduledJob] = []
    private var running: [UInt64: ScheduledJob] = [:]

    init(maximum: Int) {
        self.maximum = maximum
        self.queue = DispatchQueue(
            label: "ai.gille.sagascript.engine-host.scheduler",
            qos: .userInitiated,
            attributes: .concurrent
        )
    }

    var inFlight: Int {
        lock.lock(); defer { lock.unlock() }
        return running.count
    }

    func submit(
        id: UInt64,
        priority: String,
        work: @escaping (CancellationFlag) throws -> [String: Any],
        completion: @escaping (Result<[String: Any], EngineHostError>) -> Void
    ) {
        lock.lock()
        let job = ScheduledJob(
            id: id,
            priority: priority,
            sequence: nextSequence,
            work: work,
            completion: completion
        )
        nextSequence += 1
        pending.append(job)
        let jobs = pumpLocked()
        lock.unlock()
        start(jobs)
    }

    /// Returns true when a matching pending or running request existed.
    func cancel(id: UInt64) -> Bool {
        lock.lock()
        if let index = pending.firstIndex(where: { $0.id == id }) {
            let job = pending.remove(at: index)
            lock.unlock()
            job.completion(.failure(EngineHostError(code: "cancelled", message: "Transcription was cancelled")))
            return true
        }
        if let job = running[id] {
            job.flag.cancel()
            lock.unlock()
            return true
        }
        lock.unlock()
        return false
    }

    func cancelAll() {
        lock.lock()
        let jobs = pending
        pending.removeAll()
        for job in running.values { job.flag.cancel() }
        lock.unlock()
        for job in jobs {
            job.completion(.failure(EngineHostError(code: "cancelled", message: "Host is exiting")))
        }
    }

    private func pumpLocked() -> [ScheduledJob] {
        var result: [ScheduledJob] = []
        while running.count + result.count < maximum, !pending.isEmpty {
            let index = pending.indices.min { left, right in
                let leftPriority = pending[left].priority == "interactive" ? 0 : 1
                let rightPriority = pending[right].priority == "interactive" ? 0 : 1
                if leftPriority != rightPriority { return leftPriority < rightPriority }
                return pending[left].sequence < pending[right].sequence
            }!
            let job = pending.remove(at: index)
            running[job.id] = job
            result.append(job)
        }
        return result
    }

    private func start(_ jobs: [ScheduledJob]) {
        for job in jobs {
            queue.async { [weak self] in
                let result: Result<[String: Any], EngineHostError>
                do {
                    result = .success(try job.work(job.flag))
                } catch let error as EngineHostError {
                    result = .failure(error)
                } catch {
                    result = .failure(EngineHostError(code: "internal", message: error.localizedDescription))
                }
                job.completion(result)
                self?.finish(job.id)
            }
        }
    }

    private func finish(_ id: UInt64) {
        lock.lock()
        running.removeValue(forKey: id)
        let jobs = pumpLocked()
        lock.unlock()
        start(jobs)
    }
}

public final class EngineHostServer {
    private let engine: EngineBackend
    private let output: ([String: Any]) -> Void
    private let onShutdown: () -> Void
    private let outputLock = NSLock()
    private let stateLock = NSLock()
    private let controlQueue = DispatchQueue(label: "ai.gille.sagascript.engine-host.control", qos: .userInitiated)
    private let scheduler: RequestScheduler
    private let startedAt = Date()
    private var sawHello = false
    private var stopping = false

    public init(
        engine: EngineBackend,
        output: @escaping ([String: Any]) -> Void,
        onShutdown: @escaping () -> Void = {}
    ) {
        self.engine = engine
        self.output = output
        self.onShutdown = onShutdown
        self.scheduler = RequestScheduler(maximum: engine.capabilities.maxInFlight)
    }

    public func handleLine(_ line: String) {
        guard line.utf8.count <= 16 * 1024 * 1024 else {
            sendFailure(id: 0, error: EngineHostError(code: "protocol", message: "JSON line exceeds 16 MiB"))
            return
        }
        guard let data = line.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let request = object as? [String: Any] else {
            sendFailure(id: 0, error: EngineHostError(code: "protocol", message: "Malformed JSON request"))
            return
        }
        let id = requestID(request) ?? 0
        guard let version = request["v"] as? NSNumber, version.intValue == 1,
              let requestID = requestID(request),
              let op = request["op"] as? String, !op.isEmpty else {
            sendFailure(id: id, error: EngineHostError(code: "protocol", message: "Invalid request envelope"))
            return
        }
        stateLock.lock()
        let isStopping = stopping
        let hasHello = sawHello
        stateLock.unlock()
        if isStopping {
            sendFailure(id: requestID, error: EngineHostError(code: "protocol", message: "Host is shutting down"))
            return
        }
        if !hasHello {
            guard op == "hello" else {
                sendFailure(id: requestID, error: EngineHostError(code: "protocol", message: "hello must be the first operation"))
                return
            }
            handleHello(id: requestID, request: request)
            return
        }
        if op == "hello" {
            sendFailure(id: requestID, error: EngineHostError(code: "protocol", message: "hello may only be sent once"))
            return
        }

        switch op {
        case "ping": sendSuccess(id: requestID, fields: [:])
        case "status": handleStatus(id: requestID)
        case "cancel": handleCancel(id: requestID, request: request)
        case "load": handleLoad(id: requestID, request: request)
        case "transcribe_window": handleTranscribe(id: requestID, request: request)
        case "unload": handleUnload(id: requestID)
        case "shutdown": handleShutdown(id: requestID)
        default:
            sendFailure(id: requestID, error: EngineHostError(code: "unsupported", message: "Unknown operation: \(op)"))
        }
    }

    public func receiveEOF() {
        stateLock.lock()
        stopping = true
        stateLock.unlock()
        scheduler.cancelAll()
    }

    private func handleHello(id: UInt64, request: [String: Any]) {
        guard request["client"] is [String: Any] else {
            sendFailure(id: id, error: EngineHostError(code: "bad_request", message: "hello requires a client object"))
            return
        }
        stateLock.lock(); sawHello = true; stateLock.unlock()
        let host: [String: Any] = [
            "name": "sagascript-engine-host",
            "version": BuildInfo.version,
            "git_sha": BuildInfo.gitSHA,
            "build_dirty": BuildInfo.dirty,
            "engine": "coreml",
            "engine_version": "CoreML / \(ProcessInfo.processInfo.operatingSystemVersionString)",
        ]
        sendSuccess(id: id, fields: [
            "protocol": 1,
            "host": host,
            "capabilities": engine.capabilities.jsonObject(),
        ])
    }

    private func handleStatus(id: UInt64) {
        let inFlight = scheduler.inFlight
        let loading = engine.isLoading
        let modelID = engine.loadedModelID
        let state: String
        if loading { state = "loading" }
        else if inFlight > 0 { state = "busy" }
        else if modelID != nil { state = "ready" }
        else { state = "idle" }
        sendSuccess(id: id, fields: [
            "state": state,
            "model_id": modelID ?? NSNull(),
            "in_flight": inFlight,
            "rss_bytes": 0,
            "uptime_s": Date().timeIntervalSince(startedAt),
        ])
    }

    private func handleCancel(id: UInt64, request: [String: Any]) {
        guard let target = unsignedInteger(request["target"]) else {
            sendFailure(id: id, error: EngineHostError(code: "bad_request", message: "cancel requires target"))
            return
        }
        sendSuccess(id: id, fields: ["cancelled": scheduler.cancel(id: target)])
    }

    private func handleLoad(id: UInt64, request: [String: Any]) {
        guard let modelDirectory = request["model_dir"] as? String, modelDirectory.hasPrefix("/"),
              let modelID = request["model_id"] as? String, !modelID.isEmpty,
              let computeUnits = request["compute_units"] as? String else {
            sendFailure(id: id, error: EngineHostError(code: "bad_request", message: "load requires absolute model_dir, model_id, and compute_units"))
            return
        }
        controlQueue.async { [weak self] in
            guard let self else { return }
            do {
                let result = try self.engine.load(modelDirectory: modelDirectory, modelID: modelID, computeUnits: computeUnits)
                self.sendSuccess(id: id, fields: result.jsonObject())
            } catch let error as EngineHostError {
                self.sendFailure(id: id, error: error)
            } catch {
                self.sendFailure(id: id, error: EngineHostError(code: "model_load_failed", message: error.localizedDescription))
            }
        }
    }

    /// Validates `boost_terms` / `boost_weight` (presence, type and bounds, whether or not terms are
    /// given) with fixed error texts: dictionary terms are personal data and are never echoed.
    static func parseBoost(_ request: [String: Any]) -> Result<BoostConfig?, EngineHostError> {
        func bad(_ message: String) -> Result<BoostConfig?, EngineHostError> {
            .failure(EngineHostError(code: "bad_request", message: message))
        }
        var terms: [String] = []
        if let raw = request["boost_terms"] {
            guard let array = raw as? [Any], array.count <= BoostConfig.maxTerms else {
                return bad("boost_terms must be an array of at most 500 strings")
            }
            for item in array {
                guard let term = item as? String, term.unicodeScalars.count <= BoostConfig.maxTermLength else {
                    return bad("boost_terms must be an array of at most 500 strings of at most 64 characters")
                }
                terms.append(term)
            }
        }
        var weight: Float = 0
        if let raw = request["boost_weight"] {
            guard let number = raw as? NSNumber, String(cString: number.objCType) != "c",
                  number.floatValue.isFinite, number.floatValue >= 0, number.floatValue <= 20 else {
                return bad("boost_weight must be a number within 0...20")
            }
            weight = number.floatValue
        }
        return .success(!terms.isEmpty && weight > 0 ? BoostConfig(terms: terms, weight: weight) : nil)
    }

    private func handleTranscribe(id: UInt64, request: [String: Any]) {
        guard let pcmPath = request["pcm_path"] as? String, pcmPath.hasPrefix("/"),
              let offsetSamples = integer(request["offset_samples"]), offsetSamples >= 0,
              let numSamples = integer(request["num_samples"]), numSamples > 0,
              let sampleRate = integer(request["sample_rate"]),
              let format = request["format"] as? String else {
            sendFailure(id: id, error: EngineHostError(code: "bad_request", message: "transcribe_window has invalid parameters"))
            return
        }
        let priority = request["priority"] as? String ?? "batch"
        guard priority == "interactive" || priority == "batch" else {
            sendFailure(id: id, error: EngineHostError(code: "bad_request", message: "priority must be interactive or batch"))
            return
        }
        let maximumSamples = Int((engine.capabilities.maxWindowSeconds * Double(engine.capabilities.sampleRate)).rounded(.down))
        guard numSamples <= maximumSamples else {
            sendFailure(id: id, error: EngineHostError(code: "bad_request", message: "num_samples exceeds max_window_s", retryable: false))
            return
        }
        let boost: BoostConfig?
        switch Self.parseBoost(request) {
        case let .success(config): boost = config
        case let .failure(error): sendFailure(id: id, error: error); return
        }
        let window = WindowRequest(
            pcmPath: pcmPath,
            offsetSamples: offsetSamples,
            numSamples: numSamples,
            sampleRate: sampleRate,
            format: format,
            priority: priority,
            boost: boost
        )
        scheduler.submit(id: id, priority: priority, work: { [weak self] flag in
            guard let self else { throw EngineHostError(code: "internal", message: "Host deallocated") }
            let result = try self.engine.transcribe(window, isCancelled: { flag.isCancelled })
            return result.jsonObject()
        }, completion: { [weak self] result in
            guard let self else { return }
            switch result {
            case let .success(fields): self.sendSuccess(id: id, fields: fields)
            case let .failure(error): self.sendFailure(id: id, error: error)
            }
        })
    }

    private func handleUnload(id: UInt64) {
        controlQueue.async { [weak self] in
            guard let self else { return }
            self.engine.unload()
            self.sendSuccess(id: id, fields: [:])
        }
    }

    private func handleShutdown(id: UInt64) {
        stateLock.lock(); stopping = true; stateLock.unlock()
        sendSuccess(id: id, fields: [:])
        controlQueue.async { [weak self] in
            guard let self else { return }
            self.scheduler.cancelAll()
            self.onShutdown()
        }
    }

    private func sendSuccess(id: UInt64, fields: [String: Any]) {
        var response: [String: Any] = ["id": id, "ok": true]
        for (key, value) in fields { response[key] = value }
        send(response)
    }

    private func sendFailure(id: UInt64, error: EngineHostError) {
        send([
            "id": id,
            "ok": false,
            "error": ["code": error.code, "message": error.message, "retryable": error.retryable],
        ])
    }

    private func send(_ object: [String: Any]) {
        outputLock.lock(); defer { outputLock.unlock() }
        output(object)
    }

    private func requestID(_ request: [String: Any]) -> UInt64? { unsignedInteger(request["id"]) }

    private func unsignedInteger(_ value: Any?) -> UInt64? {
        guard let number = value as? NSNumber, number.doubleValue.rounded() == number.doubleValue,
              number.doubleValue >= 1, number.doubleValue <= Double(UInt64.max) else { return nil }
        return number.uint64Value
    }

    private func integer(_ value: Any?) -> Int? {
        guard let number = value as? NSNumber, number.doubleValue.rounded() == number.doubleValue,
              number.doubleValue >= Double(Int.min), number.doubleValue <= Double(Int.max) else { return nil }
        return number.intValue
    }
}
