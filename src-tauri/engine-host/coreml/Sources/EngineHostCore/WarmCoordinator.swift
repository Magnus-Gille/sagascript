// `warm` (docs/engine-host-protocol.md, #397): macOS can evict a loaded Core ML model's Neural
// Engine program, and the next prediction then recompiles it (11.5-16.7 s for the Pianissimo
// encoder, over 45 s on a loaded machine). There is no public API to pin a program, so a canary
// prediction on the loaded model moves that recompile out of a dictation window. The client asks
// for one when a dictation is about to start; the host also runs them on memory pressure, after
// wake from sleep and, if enabled, on a timer. This file decides whether a canary runs; the engine
// runs it.
import Darwin
import Foundation

/// What started a canary. The raw values are the strings of the stderr line.
public enum WarmTrigger: String, Sendable {
    case request
    case memoryPressure = "memory_pressure"
    case wake
    case keepAlive = "keep_alive"
}

/// Why a canary did not run, in the order the checks are made (`warm.skipped`).
public enum WarmSkipReason: String, Sendable {
    /// Another canary is running; it is not waited for.
    case inProgress = "in_progress"
    case notLoaded = "not_loaded"
    /// A window is queued or running and keeps the model resident itself.
    case busy
    /// A canary or window finished less than the recent interval ago.
    case recent
}

public enum WarmOutcome: Equatable, Sendable {
    case ran(CanaryResult)
    case skipped(WarmSkipReason)
}

/// Clocks for the warm logic. Both are in seconds from an arbitrary origin.
public enum HostClock {
    /// Keeps counting while the machine sleeps (`CLOCK_MONOTONIC_RAW` on Darwin). `systemUptime`
    /// does not count sleep, so it must not be used to age a canary.
    public static func continuousSeconds() -> TimeInterval {
        TimeInterval(clock_gettime_nsec_np(CLOCK_MONOTONIC_RAW)) / 1_000_000_000
    }

    /// Stops while the machine sleeps (`CLOCK_UPTIME_RAW`).
    public static func uptimeSeconds() -> TimeInterval {
        TimeInterval(clock_gettime_nsec_np(CLOCK_UPTIME_RAW)) / 1_000_000_000
    }
}

/// Runs canaries one at a time, off the window scheduler. A canary never takes a window slot, never
/// counts toward `status.in_flight`, and a window never waits for the coordinator.
public final class WarmCoordinator {
    public static let defaultRecentInterval: TimeInterval = 30

    /// The line goes to stderr, where the Rust client reads host diagnostics.
    public static func logToStandardError(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }

    private let engine: EngineBackend
    private let windowsInFlight: () -> Int
    private let now: () -> TimeInterval
    private let recentInterval: TimeInterval
    private let log: (String) -> Void
    // userInitiated: a lower QoS could slow the ANE compile that a real window may be waiting on.
    private let queue = DispatchQueue(label: "ai.gille.sagascript.engine-host.warm", qos: .userInitiated)
    private let lock = NSLock()
    private var inProgress = false
    private var lastActivity: TimeInterval?

    /// - Parameters:
    ///   - windowsInFlight: windows queued plus running in the scheduler.
    ///   - now: a clock that keeps counting during sleep.
    public init(
        engine: EngineBackend,
        windowsInFlight: @escaping () -> Int,
        now: @escaping () -> TimeInterval = HostClock.continuousSeconds,
        recentInterval: TimeInterval = WarmCoordinator.defaultRecentInterval,
        log: @escaping (String) -> Void = WarmCoordinator.logToStandardError
    ) {
        self.engine = engine
        self.windowsInFlight = windowsInFlight
        self.now = now
        self.recentInterval = recentInterval
        self.log = log
    }

    /// A window ran on the model (whatever its outcome): the model is resident, so a canary within
    /// `recentInterval` has nothing to do.
    public func noteActivity() {
        let time = now()
        lock.lock(); lastActivity = time; lock.unlock()
    }

    /// Runs a canary unless one of the skip rules applies. `completion` is called exactly once: at
    /// once, on the caller's thread, when another canary is running; otherwise on the warm queue
    /// after the in-progress flag is cleared.
    public func warm(trigger: WarmTrigger, completion: @escaping (Result<WarmOutcome, EngineHostError>) -> Void) {
        lock.lock()
        if inProgress {
            lock.unlock()
            let result = Result<WarmOutcome, EngineHostError>.success(.skipped(.inProgress))
            report(trigger: trigger, result: result)
            completion(result)
            return
        }
        inProgress = true
        lock.unlock()
        queue.async { [self] in
            let result = evaluate()
            report(trigger: trigger, result: result)
            lock.lock(); inProgress = false; lock.unlock()
            completion(result)
        }
    }

    /// The skip rules in the order of the protocol, then the canary itself. A canary that ran, and
    /// one that failed, both count as model activity.
    private func evaluate() -> Result<WarmOutcome, EngineHostError> {
        guard engine.loadedModelID != nil else { return .success(.skipped(.notLoaded)) }
        guard windowsInFlight() == 0 else { return .success(.skipped(.busy)) }
        lock.lock(); let last = lastActivity; lock.unlock()
        if let last, now() - last < recentInterval { return .success(.skipped(.recent)) }
        do {
            guard let canary = try engine.canary() else { return .success(.skipped(.notLoaded)) }
            noteActivity()
            return .success(.ran(canary))
        } catch let error as EngineHostError {
            noteActivity()
            return .failure(error)
        } catch {
            noteActivity()
            return .failure(EngineHostError(code: .internalError, message: error.localizedDescription))
        }
    }

    /// Every canary that ran writes one line; a probable recompile is prefixed `info: ` so the
    /// client logs it at INFO (other host stderr lines go to debug). Skipped background canaries
    /// log their reason, skipped requests (the client has the answer) log nothing. A failed
    /// background canary logs its error code only, since nobody receives the response.
    private func report(trigger: WarmTrigger, result: Result<WarmOutcome, EngineHostError>) {
        let prefix = "warm canary trigger=\(trigger.rawValue)"
        switch result {
        case let .success(.ran(canary)):
            let line = "\(prefix) ms=\(WireProtocol.milliseconds(canary.milliseconds)) probable_recompile=\(canary.probableRecompile)"
            log(canary.probableRecompile ? "info: " + line : line)
        case let .success(.skipped(reason)):
            if trigger != .request { log("\(prefix) skipped=\(reason.rawValue)") }
        case let .failure(error):
            if trigger != .request { log("\(prefix) failed=\(error.code.rawValue)") }
        }
    }
}
