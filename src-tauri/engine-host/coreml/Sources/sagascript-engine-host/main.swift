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
        let data = try WireProtocol.encode(object)
        FileHandle.standardOutput.write(data)
        FileHandle.standardOutput.write(Data([0x0a]))
    } catch {
        FileHandle.standardError.write(Data(("protocol serialization error: \(error.localizedDescription)\n").utf8))
    }
}, onShutdown: {
    exit(0)
})

// Background canaries (#397): they hide a Neural Engine recompile after macOS evicted the program.
// The decisions live in EngineHostCore; this is only the OS wiring.

// Memory pressure: a warning schedules a canary in 10 s, a return to normal in 5 s, a critical
// level none; a newer event supersedes a pending one. One serial queue owns the policy.
let pressureQueue = DispatchQueue(label: "ai.gille.sagascript.engine-host.memory-pressure", qos: .utility)
var pressurePolicy = MemoryPressurePolicy()
let pressureSource = DispatchSource.makeMemoryPressureSource(
    eventMask: [.normal, .warning, .critical],
    queue: pressureQueue
)
pressureSource.setEventHandler {
    guard let plan = pressurePolicy.record(MemoryPressureLevel(pressureSource.data)) else { return }
    pressureQueue.asyncAfter(deadline: .now() + plan.delay) {
        if pressurePolicy.shouldRun(plan) { server.backgroundWarm(.memoryPressure) }
    }
}
pressureSource.activate()

// Opt-in keep-alive: a canary every SAGASCRIPT_ENGINE_KEEPALIVE_S seconds (at least 60).
var keepAliveTimer: DispatchSourceTimer?
switch KeepAliveSetting(environmentValue: ProcessInfo.processInfo.environment[KeepAliveSetting.environmentVariable]) {
case .off:
    break
case let .every(seconds):
    let timer = DispatchSource.makeTimerSource(queue: DispatchQueue.global(qos: .utility))
    timer.schedule(deadline: .now() + .seconds(seconds), repeating: .seconds(seconds))
    timer.setEventHandler { server.backgroundWarm(.keepAlive) }
    timer.activate()
    keepAliveTimer = timer
case let .invalid(value):
    FileHandle.standardError.write(Data(
        "ignoring \(KeepAliveSetting.environmentVariable)=\(value.debugDescription): expected an integer of at least \(KeepAliveSetting.minimumSeconds)\n".utf8
    ))
}

// The once-a-second parent check also notices wake from sleep (the continuous clock runs ahead of
// the uptime clock) and then runs a canary 3 s later. Not IOKit power notifications: those oblige
// the process to acknowledge sleep.
let parentPID = getppid()
DispatchQueue.global(qos: .utility).async {
    var sleepDetector = SleepDetector()
    while true {
        sleep(1)
        if getppid() != parentPID {
            server.receiveEOF()
            exit(0)
        }
        if sleepDetector.tick(continuous: HostClock.continuousSeconds(), uptime: HostClock.uptimeSeconds()) {
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + 3) {
                server.backgroundWarm(.wake)
            }
        }
    }
}

while let line = readLine(strippingNewline: true) {
    server.handleLine(line)
}
server.receiveEOF()
exit(0)
