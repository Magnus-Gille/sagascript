import Darwin
import Foundation
import SagascriptEngineHostCore

func fail(_ message: String, code: Int32 = 2) -> Never {
    FileHandle.standardError.write(Data((message + "\n").utf8))
    exit(code)
}

let arguments = Array(CommandLine.arguments.dropFirst())
if arguments.contains("--version") {
    print("sagascript-engine-host \(BuildInfo.version) (\(BuildInfo.gitSHA), dirty=\(BuildInfo.dirty))")
    exit(0)
}

var protocolVersion: String?
var cacheDirectory: URL?
var index = 0
while index < arguments.count {
    switch arguments[index] {
    case "--protocol":
        index += 1
        guard index < arguments.count else { fail("--protocol requires a value") }
        protocolVersion = arguments[index]
    case "--cache-dir":
        index += 1
        guard index < arguments.count else { fail("--cache-dir requires a value") }
        cacheDirectory = URL(fileURLWithPath: arguments[index], isDirectory: true)
    default:
        fail("unknown argument: \(arguments[index])")
    }
    index += 1
}
guard protocolVersion == "1" else { fail("usage: sagascript-engine-host --protocol 1 [--cache-dir DIR]") }

if let cacheDirectory {
    let temporaryDirectory = cacheDirectory.appendingPathComponent("tmp", isDirectory: true)
    do {
        try FileManager.default.createDirectory(at: temporaryDirectory, withIntermediateDirectories: true)
        setenv("TMPDIR", temporaryDirectory.path, 1)
    } catch {
        fail("cannot create CoreML temporary directory: \(error.localizedDescription)")
    }
}

let engine = CoreMLEngine(cacheDirectory: cacheDirectory)
let server = EngineHostServer(engine: engine, output: { object in
    do {
        let data = try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0a]))
    } catch {
        FileHandle.standardError.write(Data(("protocol serialization error: \(error.localizedDescription)\n").utf8))
    }
}, onShutdown: {
    exit(0)
})

let parentPID = getppid()
DispatchQueue.global(qos: .utility).async {
    while true {
        sleep(1)
        if getppid() != parentPID {
            server.receiveEOF()
            exit(0)
        }
    }
}

while let line = readLine(strippingNewline: true) {
    server.handleLine(line)
}
server.receiveEOF()
exit(0)
