// swift-tools-version: 5.9
// The swift-tools-version declares the minimum version of Swift required to build this package.

import PackageDescription

let package = Package(
    name: "FlowDictate",
    platforms: [
        .macOS(.v14)
    ],
    products: [
        .executable(
            name: "FlowDictate",
            targets: ["FlowDictate"]
        )
    ],
    dependencies: [
        // WhisperKit for local transcription
        .package(url: "https://github.com/argmaxinc/WhisperKit.git", from: "0.9.0"),
        // HotKey for global keyboard shortcuts
        .package(url: "https://github.com/soffes/HotKey.git", from: "0.2.0"),
    ],
    targets: [
        .executableTarget(
            name: "FlowDictate",
            dependencies: [
                .product(name: "WhisperKit", package: "WhisperKit"),
                .product(name: "HotKey", package: "HotKey"),
            ],
            path: "Sources/FlowDictate",
            resources: [
                .process("Resources")
            ]
        ),
        .testTarget(
            name: "FlowDictateTests",
            dependencies: ["FlowDictate"],
            path: "Tests/FlowDictateTests"
        ),
    ]
)
