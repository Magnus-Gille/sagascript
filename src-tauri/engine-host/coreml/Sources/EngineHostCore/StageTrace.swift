// MEASUREMENT ONLY (SAGA-565). Per-window monotonic stage timestamps for finding which
// resource saturates at N lanes. Off unless SAGASCRIPT_ENGINE_STAGE_TRACE is set:
// "1" writes JSON Lines to stderr, any other value is a file path to append to (stderr if it
// cannot be opened). A failed write stops tracing; it never stops the host.
// With the variable unset every call site is a single `enabled` boolean check.
import Dispatch
import Foundation

enum StageTrace {
    private static let target: String? = {
        let value = ProcessInfo.processInfo.environment["SAGASCRIPT_ENGINE_STAGE_TRACE"]
        guard let value, !value.isEmpty, value != "0" else { return nil }
        return value
    }()
    static let enabled: Bool = target != nil

    private static let lock = NSLock()
    /// The trace file, or nil for stderr. An unopenable path falls back to stderr.
    private static let file: StageTraceFile? = {
        guard let target, target != "1" else { return nil }
        return StageTraceFile(path: target)
    }()
    /// Set after the first failed write; tracing then stops for the rest of the process.
    private static var failed = false // guarded by `lock`

    /// Monotonic nanoseconds, comparable across threads of this process.
    @inline(__always) static func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds }

    /// Which lane (worker thread) the caller runs on.
    static func thread() -> UInt64 {
        var id: UInt64 = 0
        pthread_threadid_np(nil, &id)
        return id
    }

    /// `window` is the request's offset_samples, which is unique per window of one file.
    static func emit(_ event: String, window: Int, _ fields: [(String, UInt64)] = []) {
        guard enabled else { return }
        var line = "{\"ev\":\"\(event)\",\"win\":\(window),\"thr\":\(thread()),\"t\":\(now())"
        for (key, value) in fields { line += ",\"\(key)\":\(value)" }
        line += "}\n"
        lock.lock(); defer { lock.unlock() }
        guard !failed else { return }
        let written: Bool
        if let file {
            written = file.write(line)
        } else {
            written = (try? FileHandle.standardError.write(contentsOf: Data(("STAGE_TRACE " + line).utf8))) != nil
        }
        if !written { failed = true }
    }
}

/// An append-only trace file. Opened with O_APPEND so several host processes can share one path
/// without overwriting each other's lines, and written with the throwing API so a full disk ends
/// tracing instead of raising an Objective-C exception that would kill the host (SAGA-565 review).
final class StageTraceFile {
    private let handle: FileHandle

    init?(path: String) {
        let descriptor = open(path, O_WRONLY | O_CREAT | O_APPEND | O_CLOEXEC, 0o644)
        guard descriptor >= 0 else { return nil }
        handle = FileHandle(fileDescriptor: descriptor, closeOnDealloc: true)
    }

    /// For tests: wrap a handle whose writes are known to fail.
    init(handle: FileHandle) {
        self.handle = handle
    }

    /// Writes one line; false when the write failed.
    func write(_ line: String) -> Bool {
        do {
            try handle.write(contentsOf: Data(line.utf8))
            return true
        } catch {
            return false
        }
    }
}
