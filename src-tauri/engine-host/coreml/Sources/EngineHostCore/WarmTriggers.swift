// The decisions behind the host's background canaries (docs/engine-host-protocol.md, `warm`), as
// small pure types so they are unit-tested; main.swift only wires them to the OS (a memory-pressure
// dispatch source, its once-a-second parent check and a timer).
import Dispatch
import Foundation

/// A memory-pressure level as `DispatchSource.makeMemoryPressureSource` reports it.
public enum MemoryPressureLevel: Equatable, Sendable {
    case normal
    case warning
    case critical

    /// The most severe level in an event mask (a source can deliver several bits at once).
    public init(_ event: DispatchSource.MemoryPressureEvent) {
        if event.contains(.critical) { self = .critical }
        else if event.contains(.warning) { self = .warning }
        else { self = .normal }
    }
}

/// When to run a canary after a memory-pressure event: 10 s after a warning, 5 s after the level
/// returns to normal, never while critical. Every event supersedes a pending one (a generation
/// counter), so a warning followed by a critical level before its 10 s are up runs nothing.
/// Not thread-safe: use it from one serial queue.
public struct MemoryPressurePolicy {
    public static let warningDelay: TimeInterval = 10
    public static let normalDelay: TimeInterval = 5

    /// A canary to run `delay` seconds from now, unless `shouldRun` says it was superseded.
    public struct Plan: Equatable, Sendable {
        public let delay: TimeInterval
        let generation: UInt64
    }

    private var generation: UInt64 = 0

    public init() {}

    /// Records an event; returns the canary it schedules, nil for a critical level.
    public mutating func record(_ level: MemoryPressureLevel) -> Plan? {
        generation += 1
        switch level {
        case .critical: return nil
        case .warning: return Plan(delay: Self.warningDelay, generation: generation)
        case .normal: return Plan(delay: Self.normalDelay, generation: generation)
        }
    }

    /// False once a newer event (including a critical one) has arrived.
    public func shouldRun(_ plan: Plan) -> Bool {
        plan.generation == generation
    }
}

/// Notices that the machine slept, without IOKit power notifications (`IORegisterForSystemPower`
/// obliges the process to acknowledge sleep). Fed from a periodic tick with the two clocks of
/// `HostClock`: the continuous one keeps counting during sleep, the uptime one does not, so the
/// continuous clock running ahead of the uptime clock between two ticks is time spent asleep.
public struct SleepDetector {
    /// The lead, in seconds, that counts as a sleep; a loaded machine can delay a tick by a little.
    public static let threshold: TimeInterval = 5

    private var previous: (continuous: TimeInterval, uptime: TimeInterval)?

    public init() {}

    /// True when, since the previous tick, the continuous clock advanced by more than
    /// `threshold` seconds beyond the uptime clock. The first tick only records the baseline.
    public mutating func tick(continuous: TimeInterval, uptime: TimeInterval) -> Bool {
        defer { previous = (continuous, uptime) }
        guard let previous else { return false }
        return (continuous - previous.continuous) - (uptime - previous.uptime) > Self.threshold
    }
}

/// `SAGASCRIPT_ENGINE_KEEPALIVE_S`: a canary every that many seconds while a model is loaded. Off
/// unless set; at least 60 s so it never races the 30 s `recent` rule.
public enum KeepAliveSetting: Equatable, Sendable {
    case off
    case every(seconds: Int)
    /// Not an integer of at least `minimumSeconds`; ignored (the caller logs it once).
    case invalid(String)

    public static let environmentVariable = "SAGASCRIPT_ENGINE_KEEPALIVE_S"
    public static let minimumSeconds = 60

    public init(environmentValue: String?) {
        guard let value = environmentValue else {
            self = .off
            return
        }
        if let seconds = Int(value), seconds >= Self.minimumSeconds {
            self = .every(seconds: seconds)
        } else {
            self = .invalid(value)
        }
    }
}
