// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "ZKit",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "ZKit", targets: ["ZKit"]),
        .executable(name: "zhello", targets: ["zhello"]),
    ],
    targets: [
        .target(name: "ZKit"),
        .executableTarget(name: "zhello", dependencies: ["ZKit"]),
        .testTarget(name: "ZKitTests", dependencies: ["ZKit"]),
    ]
)
