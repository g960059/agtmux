// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "AGTMUXDesktop",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "AGTMUXDesktop", targets: ["AGTMUXDesktop"]),
    ],
    targets: [
        .executableTarget(
            name: "AGTMUXDesktop",
            path: "Sources"
        ),
        .testTarget(
            name: "AGTMUXDesktopTests",
            dependencies: ["AGTMUXDesktop"],
            path: "Tests"
        ),
    ]
)
