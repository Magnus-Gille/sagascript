import Foundation

/// Structured logging service with JSONL file output
/// Writes to ~/Library/Logs/FlowDictate/ with automatic rotation
final class LoggingService {
    static let shared = LoggingService()

    // MARK: - Configuration

    private let maxFileSize: UInt64 = 5_000_000 // 5MB
    private let maxFiles: Int = 5
    private let flushInterval: TimeInterval = 1.0
    private let flushThreshold: Int = 50

    // MARK: - Private State

    private let writeQueue = DispatchQueue(label: "com.flowdictate.logging", qos: .utility)
    private var buffer: [LogEntry] = []
    private var fileHandle: FileHandle?
    private var currentFilePath: URL?
    private var flushTimer: DispatchSourceTimer?

    let appSessionId: String
    private(set) var currentDictationId: String?

    // MARK: - Initialization

    private init() {
        appSessionId = "app-\(UUID().uuidString.prefix(8).lowercased())"
        setupLogDirectory()
        setupFlushTimer()
    }

    deinit {
        flushTimer?.cancel()
        flush()
        fileHandle?.closeFile()
    }

    // MARK: - Public API

    /// Start a new dictation session
    func startDictationSession() -> String {
        let sessionId = "dict-\(UUID().uuidString.prefix(8).lowercased())"
        currentDictationId = sessionId
        return sessionId
    }

    /// End the current dictation session
    func endDictationSession() {
        currentDictationId = nil
    }

    /// Log an event
    func log(
        level: LogLevel,
        category: LogCategory,
        event: String,
        data: [String: AnyCodable] = [:]
    ) {
        let entry = LogEntry(
            level: level,
            appSession: appSessionId,
            dictationSession: currentDictationId,
            category: category,
            event: event,
            data: data
        )

        // Console output (immediate)
        printConsole(entry)

        // File output (buffered, async)
        writeQueue.async { [weak self] in
            self?.buffer.append(entry)
            if let count = self?.buffer.count, count >= self?.flushThreshold ?? 50 {
                self?.flush()
            }
        }
    }

    // MARK: - Convenience Methods

    func debug(_ category: LogCategory, _ event: String, data: [String: AnyCodable] = [:]) {
        log(level: .debug, category: category, event: event, data: data)
    }

    func info(_ category: LogCategory, _ event: String, data: [String: AnyCodable] = [:]) {
        log(level: .info, category: category, event: event, data: data)
    }

    func warning(_ category: LogCategory, _ event: String, data: [String: AnyCodable] = [:]) {
        log(level: .warning, category: category, event: event, data: data)
    }

    func error(_ category: LogCategory, _ event: String, data: [String: AnyCodable] = [:]) {
        log(level: .error, category: category, event: event, data: data)
    }

    // MARK: - Private Methods

    private func setupLogDirectory() {
        let logDir = getLogDirectory()
        do {
            // Create directory with owner-only permissions (0o700)
            try FileManager.default.createDirectory(
                at: logDir,
                withIntermediateDirectories: true,
                attributes: [.posixPermissions: 0o700]
            )
            currentFilePath = logDir.appendingPathComponent("flowdictate.log")
            openLogFile()
        } catch {
            print("[LoggingService] Failed to create log directory: \(error)")
        }
    }

    private func getLogDirectory() -> URL {
        let libraryPath = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask).first!
        return libraryPath.appendingPathComponent("Logs/FlowDictate")
    }

    private func openLogFile() {
        guard let path = currentFilePath else { return }

        // Create file if it doesn't exist with owner-only permissions (0o600)
        if !FileManager.default.fileExists(atPath: path.path) {
            FileManager.default.createFile(
                atPath: path.path,
                contents: nil,
                attributes: [.posixPermissions: 0o600]
            )
        }

        do {
            fileHandle = try FileHandle(forWritingTo: path)
            fileHandle?.seekToEndOfFile()
        } catch {
            print("[LoggingService] Failed to open log file: \(error)")
        }
    }

    private func setupFlushTimer() {
        flushTimer = DispatchSource.makeTimerSource(queue: writeQueue)
        flushTimer?.schedule(deadline: .now() + flushInterval, repeating: flushInterval)
        flushTimer?.setEventHandler { [weak self] in
            self?.flush()
        }
        flushTimer?.resume()
    }

    private func flush() {
        // Must be called on writeQueue
        guard !buffer.isEmpty else { return }

        let entries = buffer
        buffer.removeAll()

        rotateIfNeeded()

        let encoder = JSONEncoder()
        encoder.outputFormatting = [] // Compact JSON

        for entry in entries {
            do {
                let jsonData = try encoder.encode(entry)
                if let newline = "\n".data(using: .utf8) {
                    fileHandle?.write(jsonData)
                    fileHandle?.write(newline)
                }
            } catch {
                print("[LoggingService] Failed to encode log entry: \(error)")
            }
        }

        // Sync to disk
        fileHandle?.synchronizeFile()
    }

    private func rotateIfNeeded() {
        guard let path = currentFilePath else { return }

        do {
            let attributes = try FileManager.default.attributesOfItem(atPath: path.path)
            let fileSize = attributes[.size] as? UInt64 ?? 0

            guard fileSize >= maxFileSize else { return }

            fileHandle?.closeFile()
            fileHandle = nil

            let logDir = getLogDirectory()
            let fm = FileManager.default

            // Delete oldest file
            let oldestFile = logDir.appendingPathComponent("flowdictate.\(maxFiles).log")
            try? fm.removeItem(at: oldestFile)

            // Rotate files: 4 -> 5, 3 -> 4, etc.
            for i in stride(from: maxFiles - 1, through: 1, by: -1) {
                let from = logDir.appendingPathComponent("flowdictate.\(i).log")
                let to = logDir.appendingPathComponent("flowdictate.\(i + 1).log")
                try? fm.moveItem(at: from, to: to)
            }

            // Move current to .1
            let firstRotated = logDir.appendingPathComponent("flowdictate.1.log")
            try? fm.moveItem(at: path, to: firstRotated)

            // Create new file with owner-only permissions (0o600)
            fm.createFile(atPath: path.path, contents: nil, attributes: [.posixPermissions: 0o600])
            openLogFile()

        } catch {
            print("[LoggingService] Failed to check file size: \(error)")
        }
    }

    private func printConsole(_ entry: LogEntry) {
        let dateFormatter = DateFormatter()
        dateFormatter.dateFormat = "yyyy-MM-dd HH:mm:ss"
        let timestamp = dateFormatter.string(from: Date())

        let levelStr = entry.level.rawValue.uppercased()
        let category = entry.category.rawValue

        var message = "\(timestamp) [\(levelStr)] [\(category)] \(entry.event)"

        // Append data as key=value pairs
        if !entry.data.isEmpty {
            let pairs = entry.data.map { key, value in
                "\(key)=\(formatValue(value.value))"
            }.joined(separator: " ")
            message += " \(pairs)"
        }

        print(message)
    }

    private func formatValue(_ value: Any) -> String {
        switch value {
        case let str as String:
            return str
        case let num as Int:
            return "\(num)"
        case let num as Double:
            return String(format: "%.2f", num)
        case let bool as Bool:
            return bool ? "true" : "false"
        default:
            return String(describing: value)
        }
    }
}
