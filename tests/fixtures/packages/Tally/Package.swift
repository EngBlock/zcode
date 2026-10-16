// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Tally",
    products: [
        .library(name: "Tally", targets: ["Tally"]),
        .library(name: "TallyFormat", targets: ["TallyFormat"]),
    ],
    targets: [
        .target(name: "Tally"),
        .target(name: "TallyFormat", dependencies: ["Tally"]),
    ]
)
