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
    dependencies: [
        .package(url: "https://github.com/migueldeicaza/SwiftTerm.git", from: "1.2.6"),
    ],
    targets: [
        .executableTarget(
            name: "AGTMUXDesktop",
            dependencies: [
                "SwiftTerm",
            ],
            path: "Sources"
        ),
        .testTarget(
            name: "AGTMUXDesktopTests",
            dependencies: ["AGTMUXDesktop"],
            path: "Tests"
        ),
    ]
)
