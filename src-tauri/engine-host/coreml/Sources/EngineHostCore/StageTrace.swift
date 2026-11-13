// MEASUREMENT ONLY (SAGA-565). Per-window monotonic stage timestamps for finding which
// resource saturates at N lanes. Off unless SAGASCRIPT_ENGINE_STAGE_TRACE is set:
// "1" writes JSON Lines to stderr, any other value is a file path to append to.
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
    private static let handle: FileHandle? = {
        guard let target, target != "1" else { return nil }
        if !FileManager.default.fileExists(atPath: target) {
            FileManager.default.createFile(atPath: target, contents: nil)
        }
        guard let opened = FileHandle(forWritingAtPath: target) else { return nil }
        _ = try? opened.seekToEnd()
        return opened
    }()

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
        if let handle {
            handle.write(Data(line.utf8))
        } else {
            FileHandle.standardError.write(Data(("STAGE_TRACE " + line).utf8))
        }
    }
}
