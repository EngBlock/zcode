// swift-tools-version: 5.9
import PackageDescription

// A Swift app with its web front end in web/ (a Vite app), the way an iOS
// app ships a web view's pages.
let package = Package(
    name: "HelloWeb",
    targets: [
        .executableTarget(name: "HelloWeb"),
    ]
)
