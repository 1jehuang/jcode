// swift-tools-version: 6.0

import PackageDescription

let package = Package(
    name: "JCodeKit",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(
            name: "JCodeKit",
            targets: ["JCodeKit"]
        ),
    ],
    targets: [
        .target(
            name: "JCodeKit",
            path: "Sources/JCodeKit"
        ),
        .executableTarget(
            name: "JCodeKitTests",
            dependencies: ["JCodeKit"],
            path: "Tests/JCodeKitTests"
        ),
    ]
)
