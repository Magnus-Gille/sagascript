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
    let priority: WindowPriority
    let sequence: UInt64
    let flag = CancellationFlag()
    let work: (CancellationFlag) throws -> [String: Any]
    let completion: (Result<[String: Any], EngineHostError>) -> Void

    init(
        id: UInt64,
        priority: WindowPriority,
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
        priority: WindowPriority,
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
            job.completion(.failure(EngineHostError(code: .cancelled, message: "Transcription was cancelled")))
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
            job.completion(.failure(EngineHostError(code: .cancelled, message: "Host is exiting")))
        }
    }

    private func pumpLocked() -> [ScheduledJob] {
        var result: [ScheduledJob] = []
        while running.count + result.count < maximum, !pending.isEmpty {
            let index = pending.indices.min { left, right in
                let leftPriority = pending[left].priority == .interactive ? 0 : 1
                let rightPriority = pending[right].priority == .interactive ? 0 : 1
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
                    result = .failure(EngineHostError(code: .internalError, message: error.localizedDescription))
                }
                // Free the slot before the response goes out (as the ONNX host does), so a `status`
                // sent after the client saw the response never still counts this window.
                self?.finish(job.id)
                job.completion(result)
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
        let parsed = WireProtocol.parse(line: line)
        let id: UInt64
        let op: String
        switch parsed {
        case let .invalidEnvelope(envelopeID, message):
            sendFailure(id: envelopeID, error: EngineHostError(code: .protocolError, message: message))
            return
        case let .unknownOp(requestID, name), let .badParams(requestID, name, _), let .request(requestID, name, _):
            id = requestID
            op = name
        }
        stateLock.lock()
        let isStopping = stopping
        let hasHello = sawHello
        stateLock.unlock()
        if isStopping {
            sendFailure(id: id, error: EngineHostError(code: .protocolError, message: "Host is shutting down"))
            return
        }
        if !hasHello {
            switch parsed {
            case .request(_, _, .hello):
                handleHello(id: id)
            case let .badParams(_, "hello", message):
                // An incomplete client identity is a bad request, not the handshake.
                sendFailure(id: id, error: EngineHostError(code: .badRequest, message: message))
            default:
                sendFailure(id: id, error: EngineHostError(code: .protocolError, message: "hello must be the first operation"))
            }
            return
        }
        if op == "hello" {
            sendFailure(id: id, error: EngineHostError(code: .protocolError, message: "hello may only be sent once"))
            return
        }
        switch parsed {
        case .invalidEnvelope:
            return
        case let .unknownOp(_, name):
            sendFailure(id: id, error: EngineHostError(code: .unsupported, message: "Unknown operation: \(name)"))
        case let .badParams(_, _, message):
            sendFailure(id: id, error: EngineHostError(code: .badRequest, message: message))
        case let .request(_, _, request):
            dispatch(id: id, request: request)
        }
    }

    public func receiveEOF() {
        stateLock.lock()
        stopping = true
        stateLock.unlock()
        scheduler.cancelAll()
    }

    private func dispatch(id: UInt64, request: WireRequest) {
        switch request {
        case .hello: return
        case .ping: sendSuccess(id: id, fields: [:])
        case .status: handleStatus(id: id)
        case let .cancel(target):
            sendSuccess(id: id, fields: WireProtocol.cancelResult(cancelled: scheduler.cancel(id: target)))
        case let .load(modelDirectory, modelID, computeUnits):
            handleLoad(id: id, modelDirectory: modelDirectory, modelID: modelID, computeUnits: computeUnits)
        case let .transcribeWindow(parameters): handleTranscribe(id: id, parameters: parameters)
        case .unload: handleUnload(id: id)
        case .shutdown: handleShutdown(id: id)
        }
    }

    private func handleHello(id: UInt64) {
        stateLock.lock(); sawHello = true; stateLock.unlock()
        let host = WireHostIdentity(
            version: BuildInfo.version,
            gitSHA: BuildInfo.gitSHA,
            dirty: BuildInfo.dirty,
            engine: "coreml",
            engineVersion: "CoreML / \(ProcessInfo.processInfo.operatingSystemVersionString)"
        )
        sendSuccess(id: id, fields: WireProtocol.helloResult(host: host, capabilities: engine.capabilities))
    }

    private func handleStatus(id: UInt64) {
        let inFlight = scheduler.inFlight
        let loading = engine.isLoading
        let modelID = engine.loadedModelID
        let state: WireHostState
        if loading { state = .loading }
        else if inFlight > 0 { state = .busy }
        else if modelID != nil { state = .ready }
        else { state = .idle }
        sendSuccess(id: id, fields: WireProtocol.statusResult(
            state: state,
            modelID: modelID,
            inFlight: inFlight,
            rssBytes: 0,
            uptimeSeconds: Date().timeIntervalSince(startedAt)
        ))
    }

    private func handleLoad(id: UInt64, modelDirectory: String, modelID: String, computeUnits: String) {
        guard modelDirectory.hasPrefix("/"), !modelID.isEmpty else {
            sendFailure(id: id, error: EngineHostError(code: .badRequest, message: "load requires absolute model_dir, model_id, and compute_units"))
            return
        }
        controlQueue.async { [weak self] in
            guard let self else { return }
            do {
                let result = try self.engine.load(modelDirectory: modelDirectory, modelID: modelID, computeUnits: computeUnits)
                self.sendSuccess(id: id, fields: WireProtocol.loadResult(result))
            } catch let error as EngineHostError {
                self.sendFailure(id: id, error: error)
            } catch {
                self.sendFailure(id: id, error: EngineHostError(code: .modelLoadFailed, message: error.localizedDescription))
            }
        }
    }

    /// Checks a window against this host's limits (the ONNX host applies the same checks).
    private func handleTranscribe(id: UInt64, parameters: WireWindowParameters) {
        let capabilities = engine.capabilities
        guard parameters.pcmPath.hasPrefix("/"), parameters.numSamples > 0, parameters.format == "f32le" else {
            sendFailure(id: id, error: EngineHostError(code: .badRequest, message: "transcribe_window has invalid parameters"))
            return
        }
        guard parameters.sampleRate == capabilities.sampleRate else {
            sendFailure(id: id, error: EngineHostError(code: .badRequest, message: "sample_rate must match the host sample rate"))
            return
        }
        let maximumSamples = Int((capabilities.maxWindowSeconds * Double(capabilities.sampleRate)).rounded(.down))
        guard parameters.numSamples <= maximumSamples else {
            sendFailure(id: id, error: EngineHostError(code: .badRequest, message: "num_samples exceeds max_window_s"))
            return
        }
        let window = WindowRequest(
            pcmPath: parameters.pcmPath,
            offsetSamples: parameters.offsetSamples,
            numSamples: parameters.numSamples,
            sampleRate: parameters.sampleRate,
            format: parameters.format,
            priority: parameters.priority,
            boost: parameters.boost
        )
        scheduler.submit(id: id, priority: parameters.priority, work: { [weak self] flag in
            guard let self else { throw EngineHostError(code: .internalError, message: "Host deallocated") }
            let result = try self.engine.transcribe(window, isCancelled: { flag.isCancelled })
            return WireProtocol.windowResult(result)
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
        send(WireProtocol.success(id: id, fields: fields))
    }

    private func sendFailure(id: UInt64, error: EngineHostError) {
        send(WireProtocol.failure(id: id, error: error))
    }

    private func send(_ object: [String: Any]) {
        outputLock.lock(); defer { outputLock.unlock() }
        output(object)
    }
}
