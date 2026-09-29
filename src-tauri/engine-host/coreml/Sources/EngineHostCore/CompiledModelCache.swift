import Foundation

/// Compiled-model cache: `<root>/<key>/<Name>.mlmodelc` plus a `.complete` marker.
///
/// - Each `.mlpackage` is compiled at most once per key. The compiled model is
///   staged in a scratch directory inside the cache root and moved into place
///   with a single rename, so `<key>/<Name>.mlmodelc` is either absent or whole.
/// - The `.complete` marker is written only after every required model is
///   present. Models compiled by an interrupted run are reused by the next run.
/// - Callers load from the returned stable path: the operating system's Neural
///   Engine compilation cache is keyed on it, so a stable path is what makes the
///   second load fast.
struct CompiledModelCache {
    static let completionMarker = ".complete"
    static let scratchPrefix = ".tmp-"

    let root: URL
    /// Compiles a `.mlpackage` and returns the location of the resulting
    /// `.mlmodelc` (which the cache then takes ownership of).
    let compile: (URL) throws -> URL

    struct Result {
        let directory: URL
        /// True only when at least one model was actually compiled by this call.
        let compiled: Bool
    }

    func modelURL(key: String, name: String) -> URL {
        root.appendingPathComponent(key, isDirectory: true)
            .appendingPathComponent("\(name).mlmodelc", isDirectory: true)
    }

    func isComplete(key: String) -> Bool {
        FileManager.default.fileExists(
            atPath: root.appendingPathComponent(key, isDirectory: true)
                .appendingPathComponent(Self.completionMarker).path
        )
    }

    /// A compiled model directory always contains `coremldata.bin`.
    static func isCompiledModel(_ url: URL) -> Bool {
        var isDirectory: ObjCBool = false
        return FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory)
            && isDirectory.boolValue
            && FileManager.default.fileExists(atPath: url.appendingPathComponent("coremldata.bin").path)
    }

    /// Ensures every name in `packages` is compiled under `<root>/<key>`.
    /// `required` lists every model that the completed cache entry must hold.
    func prepare(key: String, packages: [String: URL], required: [String]) throws -> Result {
        let fileManager = FileManager.default
        let directory = root.appendingPathComponent(key, isDirectory: true)
        try fileManager.createDirectory(at: directory, withIntermediateDirectories: true)
        let marker = directory.appendingPathComponent(Self.completionMarker)

        if isComplete(key: key), required.allSatisfy({ Self.isCompiledModel(modelURL(key: key, name: $0)) }) {
            return Result(directory: directory, compiled: false)
        }
        // Not complete (or damaged): the marker must not vouch for it any more.
        try? fileManager.removeItem(at: marker)

        var didCompile = false
        for name in required.sorted() {
            let target = modelURL(key: key, name: name)
            if Self.isCompiledModel(target) { continue }
            guard let package = packages[name] else {
                throw EngineHostError(code: "model_load_failed", message: "No package to compile for missing \(name).mlmodelc")
            }
            // Anything at `target` is not a whole model (e.g. a legacy partial copy).
            try? fileManager.removeItem(at: target)
            try compileAndInstall(name: name, package: package, target: target)
            didCompile = true
        }

        guard required.allSatisfy({ Self.isCompiledModel(modelURL(key: key, name: $0)) }) else {
            throw EngineHostError(code: "model_load_failed", message: "Compiled model cache is incomplete at \(directory.path)")
        }
        do {
            try Data("ok\n".utf8).write(to: marker, options: .atomic)
        } catch {
            throw EngineHostError(code: "model_load_failed", message: "Cannot mark CoreML cache complete: \(error.localizedDescription)")
        }
        return Result(directory: directory, compiled: didCompile)
    }

    private func compileAndInstall(name: String, package: URL, target: URL) throws {
        let fileManager = FileManager.default
        let scratch = root.appendingPathComponent(Self.scratchPrefix + UUID().uuidString, isDirectory: true)
        try fileManager.createDirectory(at: scratch, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: scratch) }
        do {
            let compiled = try compile(package)
            let staged = scratch.appendingPathComponent("\(name).mlmodelc", isDirectory: true)
            // The compiler writes into the system temp directory (possibly another
            // volume), so stage inside the cache root first; `moveItem` then is a
            // same-volume rename.
            try fileManager.moveItem(at: compiled, to: staged)
            guard Self.isCompiledModel(staged) else {
                throw EngineHostError(code: "model_load_failed", message: "CoreML compilation of \(name) produced no model")
            }
            do {
                try fileManager.moveItem(at: staged, to: target)
            } catch {
                // A concurrent host may have installed the same model first.
                guard Self.isCompiledModel(target) else { throw error }
            }
        } catch let error as EngineHostError {
            throw error
        } catch {
            throw EngineHostError(code: "model_load_failed", message: "CoreML compilation failed for \(name): \(error.localizedDescription)")
        }
    }

    /// Removes scratch directories left behind by crashed hosts.
    func removeStaleScratch(olderThan age: TimeInterval = 3600) {
        let fileManager = FileManager.default
        guard let items = try? fileManager.contentsOfDirectory(
            at: root, includingPropertiesForKeys: [.contentModificationDateKey]
        ) else { return }
        for item in items where item.lastPathComponent.hasPrefix(Self.scratchPrefix) {
            let modified = (try? item.resourceValues(forKeys: [.contentModificationDateKey]))?.contentModificationDate
            if let modified, Date().timeIntervalSince(modified) > age { try? fileManager.removeItem(at: item) }
        }
    }
}
