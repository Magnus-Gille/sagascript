import CoreML
import Foundation
import Testing
@testable import SagascriptEngineHostCore

private let names = ["Preprocessor", "Encoder", "Decoder", "JointDecisionv3"]

private func makeRoot() throws -> URL {
    let root = FileManager.default.temporaryDirectory
        .appendingPathComponent("engine-host-cache-test-\(UUID().uuidString)", isDirectory: true)
    try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
    return root
}

private func makePackages(in root: URL) throws -> [String: URL] {
    var result: [String: URL] = [:]
    for name in names {
        let package = root.appendingPathComponent("src/\(name).mlpackage", isDirectory: true)
        try FileManager.default.createDirectory(at: package, withIntermediateDirectories: true)
        result[name] = package
    }
    return result
}

/// A fake compiler that writes a whole `.mlmodelc` into a temp directory, like Core ML does.
private final class FakeCompiler {
    private let lock = NSLock()
    private(set) var compiled: [String] = []
    var failOn: String?

    func compile(_ package: URL) throws -> URL {
        let name = package.deletingPathExtension().lastPathComponent
        lock.lock(); compiled.append(name); lock.unlock()
        if name == failOn { throw EngineHostError(code: "x", message: "boom \(name)") }
        let output = FileManager.default.temporaryDirectory
            .appendingPathComponent("fake-compile-\(UUID().uuidString)/\(name).mlmodelc", isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        try Data([1, 2, 3]).write(to: output.appendingPathComponent("coremldata.bin"))
        return output
    }
}

@Test func cacheCompilesOnceThenReuses() throws {
    let root = try makeRoot(); defer { try? FileManager.default.removeItem(at: root) }
    let packages = try makePackages(in: root)
    let compiler = FakeCompiler()
    let cacheRoot = root.appendingPathComponent("cache")
    let cache = CompiledModelCache(root: cacheRoot, compile: compiler.compile)

    let first = try cache.prepare(key: "k", packages: packages, required: names)
    #expect(first.compiled)
    #expect(compiler.compiled.sorted() == names.sorted())
    #expect(cache.isComplete(key: "k"))
    for name in names { #expect(CompiledModelCache.isCompiledModel(cache.modelURL(key: "k", name: name))) }

    let second = try cache.prepare(key: "k", packages: packages, required: names)
    #expect(!second.compiled)
    #expect(compiler.compiled.count == 4)  // nothing recompiled
    #expect(second.directory == first.directory)
    // No scratch directories are left in the cache root.
    let entries = try FileManager.default.contentsOfDirectory(atPath: cacheRoot.path)
    #expect(entries == ["k"])
}

@Test func interruptedCompilationLeavesNoMarkerAndResumes() throws {
    let root = try makeRoot(); defer { try? FileManager.default.removeItem(at: root) }
    let packages = try makePackages(in: root)
    let compiler = FakeCompiler()
    compiler.failOn = "JointDecisionv3"
    let cache = CompiledModelCache(root: root.appendingPathComponent("cache"), compile: compiler.compile)

    #expect(throws: EngineHostError.self) { try cache.prepare(key: "k", packages: packages, required: names) }
    #expect(!cache.isComplete(key: "k"))
    #expect(!CompiledModelCache.isCompiledModel(cache.modelURL(key: "k", name: "JointDecisionv3")))
    let done = names.filter { CompiledModelCache.isCompiledModel(cache.modelURL(key: "k", name: $0)) }
    #expect(done.sorted() == ["Decoder", "Encoder"])  // models finished before the failure are kept
    let leftovers = try FileManager.default.contentsOfDirectory(atPath: root.appendingPathComponent("cache").path)
        .filter { $0.hasPrefix(CompiledModelCache.scratchPrefix) }
    #expect(leftovers.isEmpty)

    compiler.failOn = nil
    let before = compiler.compiled.count
    let retry = try cache.prepare(key: "k", packages: packages, required: names)
    #expect(retry.compiled)
    #expect(compiler.compiled.count - before == 2)  // only JointDecisionv3 and Preprocessor
    #expect(cache.isComplete(key: "k"))
}

@Test func legacyPartialDirectoryWithoutMarkerIsCompleted() throws {
    let root = try makeRoot(); defer { try? FileManager.default.removeItem(at: root) }
    let packages = try makePackages(in: root)
    let compiler = FakeCompiler()
    let cache = CompiledModelCache(root: root.appendingPathComponent("cache"), compile: compiler.compile)
    // Only Decoder present (as the old host left it), plus a torn Encoder without coremldata.bin.
    let decoder = cache.modelURL(key: "k", name: "Decoder")
    try FileManager.default.createDirectory(at: decoder, withIntermediateDirectories: true)
    try Data([9]).write(to: decoder.appendingPathComponent("coremldata.bin"))
    let torn = cache.modelURL(key: "k", name: "Encoder")
    try FileManager.default.createDirectory(at: torn, withIntermediateDirectories: true)

    let result = try cache.prepare(key: "k", packages: packages, required: names)
    #expect(result.compiled)
    #expect(compiler.compiled.sorted() == ["Encoder", "JointDecisionv3", "Preprocessor"])
    #expect(cache.isComplete(key: "k"))
}

@Test func markerWithMissingModelTriggersRecompile() throws {
    let root = try makeRoot(); defer { try? FileManager.default.removeItem(at: root) }
    let packages = try makePackages(in: root)
    let compiler = FakeCompiler()
    let cache = CompiledModelCache(root: root.appendingPathComponent("cache"), compile: compiler.compile)
    _ = try cache.prepare(key: "k", packages: packages, required: names)
    try FileManager.default.removeItem(at: cache.modelURL(key: "k", name: "Encoder"))
    let again = try cache.prepare(key: "k", packages: packages, required: names)
    #expect(again.compiled)
    #expect(compiler.compiled.count == 5)
}

@Test func staleScratchIsRemoved() throws {
    let root = try makeRoot(); defer { try? FileManager.default.removeItem(at: root) }
    let cacheRoot = root.appendingPathComponent("cache")
    let scratch = cacheRoot.appendingPathComponent(CompiledModelCache.scratchPrefix + "old", isDirectory: true)
    try FileManager.default.createDirectory(at: scratch, withIntermediateDirectories: true)
    let cache = CompiledModelCache(root: cacheRoot, compile: FakeCompiler().compile)
    cache.removeStaleScratch(olderThan: -1)
    #expect(!FileManager.default.fileExists(atPath: scratch.path))
}

@Test func halfConversionMatchesFloat16() {
    #if arch(arm64)
    let samples: [Float] = [0, -0.0, 1, -1, 0.1, 3.14159, 65504, 70000, 1e-5, 6e-8, 1e-9, -2.5e-6, .infinity, 0.33333334]
    for value in samples {
        let expected = Float16(value)
        #expect(floatToHalf(value) == expected.bitPattern)
        #expect(halfToFloat(expected.bitPattern) == Float(expected))
    }
    #endif
    #expect(halfToFloat(0x7C00) == .infinity)
    #expect(halfToFloat(0x3C00) == 1)
    #expect(floatToHalf(Float.nan) & 0x7C00 == 0x7C00)
}

@Test func encoderFramesReadsFloat16Output() throws {
    let array = try makeAlignedArray(shape: [1, 4, 3], dataType: .float16)
    let pointer = array.dataPointer.bindMemory(to: UInt16.self, capacity: array.count)
    let strides = array.strides.map(\.intValue)
    for hidden in 0..<4 { for time in 0..<3 { pointer[hidden * strides[1] + time * strides[2]] = floatToHalf(Float(hidden * 10 + time)) } }
    let frames = try EncoderFrames(array, validLength: 3, hiddenSize: 4)
    var destination = [Float](repeating: 0, count: 4)
    destination.withUnsafeMutableBufferPointer { frames.copyFrame(1, to: $0.baseAddress!, stride: 1) }
    #expect(destination == [1, 11, 21, 31])

    // ... and into a float16 destination (float16 joint input).
    let step = try makeAlignedArray(shape: [1, 4, 1], dataType: .float16)
    frames.copyFrame(2, to: FloatVectorDestination(step, precision: .float16, axis: 1))
    let stepPointer = step.dataPointer.bindMemory(to: UInt16.self, capacity: step.count)
    let stepStride = step.strides[1].intValue
    #expect((0..<4).map { halfToFloat(stepPointer[$0 * stepStride]) } == [2, 12, 22, 32])
}

@Test func encoderFramesErrorNamesFeatureShapeAndDtype() throws {
    let array = try MLMultiArray(shape: [1, 4, 3], dataType: .double)
    do {
        _ = try EncoderFrames(array, validLength: 3, hiddenSize: 4)
        Issue.record("expected an error")
    } catch let error as EngineHostError {
        #expect(error.message.contains("'encoder'"))
        #expect(error.message.contains("[1, 4, 3]"))
        #expect(error.message.contains("float64"))
    }
}

@Test func convertedCopiesBetweenPrecisions() throws {
    let source = try MLMultiArray(shape: [1, 2, 3], dataType: .float32)
    for index in 0..<6 { source[index] = NSNumber(value: Float(index) + 0.5) }
    let half = try converted(source, to: .float16, feature: "test")
    #expect(half.dataType == .float16)
    #expect((0..<6).map { half[$0].floatValue } == [0.5, 1.5, 2.5, 3.5, 4.5, 5.5])
    #expect(try converted(source, to: .float32, feature: "test") === source)
}
