// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "sagascript-engine-host",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "SagascriptEngineHostCore", targets: ["SagascriptEngineHostCore"]),
        .executable(name: "sagascript-engine-host", targets: ["sagascript-engine-host"]),
    ],
    targets: [
        .target(
            name: "SagascriptEngineHostCore",
            path: "Sources/EngineHostCore"
        ),
        .executableTarget(
            name: "sagascript-engine-host",
            dependencies: ["SagascriptEngineHostCore"],
            path: "Sources/sagascript-engine-host"
        ),
        .testTarget(
            name: "EngineHostCoreTests",
            dependencies: ["SagascriptEngineHostCore"],
            path: "Tests/EngineHostCoreTests"
        ),
    ],
    swiftLanguageVersions: [.v5]
)
