import Foundation
import Testing
@testable import SagascriptEngineHostCore

// The stage trace (SAGA-565) is measurement only: it must never crash the host, and several host
// processes may share one trace file.

private func temporaryPath(_ name: String) -> String {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("stage-trace-\(UUID().uuidString)", isDirectory: true)
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    return directory.appendingPathComponent(name).path
}

@Test func traceFilesSharingAPathAppendInsteadOfOverwriting() throws {
    let path = temporaryPath("trace.jsonl")
    let first = try #require(StageTraceFile(path: path))
    let second = try #require(StageTraceFile(path: path))

    #expect(first.write("a\n"))
    #expect(first.write("b\n"))
    #expect(second.write("c\n"))
    #expect(first.write("d\n"))

    let contents = try String(contentsOfFile: path, encoding: .utf8)
    #expect(contents == "a\nb\nc\nd\n")
}

@Test func traceFileAppendsToAnExistingFile() throws {
    let path = temporaryPath("trace.jsonl")
    try "earlier\n".write(toFile: path, atomically: true, encoding: .utf8)
    let file = try #require(StageTraceFile(path: path))

    #expect(file.write("later\n"))

    #expect(try String(contentsOfFile: path, encoding: .utf8) == "earlier\nlater\n")
}

@Test func anUnopenableTracePathYieldsNoFileInsteadOfCrashing() {
    let directory = temporaryPath("is-a-directory")
    try? FileManager.default.createDirectory(atPath: directory, withIntermediateDirectories: true)

    #expect(StageTraceFile(path: directory) == nil)
    #expect(StageTraceFile(path: temporaryPath("missing-parent/trace.jsonl") + "/x") == nil)
}

@Test func aFailingTraceWriteReportsFailureInsteadOfRaising() throws {
    // A handle opened for reading makes every write fail, like a full disk would; the old
    // `FileHandle.write(_:)` raised an Objective-C exception here and killed the host.
    let path = temporaryPath("read-only.jsonl")
    try "".write(toFile: path, atomically: true, encoding: .utf8)
    let readOnly = try #require(FileHandle(forReadingAtPath: path))
    let file = StageTraceFile(handle: readOnly)

    #expect(!file.write("line\n"))
    #expect(!file.write("line\n"))
}
