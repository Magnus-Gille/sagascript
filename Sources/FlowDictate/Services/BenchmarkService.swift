import Foundation
import os.log

/// Service for measuring and recording performance benchmarks
final class BenchmarkService {
    // MARK: - Singleton

    static let shared = BenchmarkService()

    // MARK: - Private State

    private let logger = Logger(subsystem: "com.flowdictate", category: "Benchmark")
    private var measurements: [String: [TimeInterval]] = [:]
    private let lock = NSLock()

    // MARK: - Initialization

    private init() {}

    // MARK: - Public Methods

    /// Measure the execution time of an async operation
    /// - Parameters:
    ///   - name: Name of the operation being measured
    ///   - operation: The async operation to measure
    /// - Returns: The result of the operation
    func measure<T>(_ name: String, operation: () async throws -> T) async rethrows -> T {
        let start = CFAbsoluteTimeGetCurrent()
        let result = try await operation()
        let elapsed = CFAbsoluteTimeGetCurrent() - start

        record(name: name, duration: elapsed)
        return result
    }

    /// Measure the execution time of a synchronous operation
    /// - Parameters:
    ///   - name: Name of the operation being measured
    ///   - operation: The operation to measure
    /// - Returns: The result of the operation
    func measureSync<T>(_ name: String, operation: () throws -> T) rethrows -> T {
        let start = CFAbsoluteTimeGetCurrent()
        let result = try operation()
        let elapsed = CFAbsoluteTimeGetCurrent() - start

        record(name: name, duration: elapsed)
        return result
    }

    /// Record a duration measurement
    /// - Parameters:
    ///   - name: Name of the measurement
    ///   - duration: Duration in seconds
    func record(name: String, duration: TimeInterval) {
        lock.lock()
        defer { lock.unlock() }

        if measurements[name] == nil {
            measurements[name] = []
        }
        measurements[name]?.append(duration)

        logger.debug("\(name): \(duration * 1000, format: .fixed(precision: 2))ms")
    }

    /// Get statistics for a measurement
    /// - Parameter name: Name of the measurement
    /// - Returns: Statistics or nil if no measurements exist
    func statistics(for name: String) -> MeasurementStats? {
        lock.lock()
        defer { lock.unlock() }

        guard let samples = measurements[name], !samples.isEmpty else {
            return nil
        }

        let sorted = samples.sorted()
        let count = samples.count
        let sum = samples.reduce(0, +)
        let avg = sum / Double(count)
        let minVal = sorted.first ?? 0
        let maxVal = sorted.last ?? 0
        let p50 = sorted[count / 2]
        let p95Index = Swift.min(Int(Double(count) * 0.95), count - 1)
        let p99Index = Swift.min(Int(Double(count) * 0.99), count - 1)
        let p95 = sorted[p95Index]
        let p99 = sorted[p99Index]

        return MeasurementStats(
            count: count,
            avg: avg,
            min: minVal,
            max: maxVal,
            p50: p50,
            p95: p95,
            p99: p99
        )
    }

    /// Get all measurement names
    var measurementNames: [String] {
        lock.lock()
        defer { lock.unlock() }
        return Array(measurements.keys).sorted()
    }

    /// Generate a markdown report of all measurements
    func generateReport() -> String {
        var report = "# FlowDictate Performance Benchmarks\n\n"
        report += "Generated: \(ISO8601DateFormatter().string(from: Date()))\n\n"
        report += "## Measurements\n\n"
        report += "| Metric | Count | Avg (ms) | Min (ms) | Max (ms) | P50 (ms) | P95 (ms) | P99 (ms) |\n"
        report += "|--------|-------|----------|----------|----------|----------|----------|----------|\n"

        for name in measurementNames {
            if let stats = statistics(for: name) {
                report += "| \(name) | \(stats.count) | \(String(format: "%.2f", stats.avg * 1000)) | \(String(format: "%.2f", stats.min * 1000)) | \(String(format: "%.2f", stats.max * 1000)) | \(String(format: "%.2f", stats.p50 * 1000)) | \(String(format: "%.2f", stats.p95 * 1000)) | \(String(format: "%.2f", stats.p99 * 1000)) |\n"
            }
        }

        return report
    }

    /// Clear all measurements
    func reset() {
        lock.lock()
        defer { lock.unlock() }
        measurements.removeAll()
    }
}

// MARK: - Supporting Types

struct MeasurementStats {
    let count: Int
    let avg: TimeInterval
    let min: TimeInterval
    let max: TimeInterval
    let p50: TimeInterval
    let p95: TimeInterval
    let p99: TimeInterval
}
