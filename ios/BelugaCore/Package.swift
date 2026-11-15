// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "BelugaCore",
    platforms: [.iOS("26.0"), .macOS("15.0")],
    products: [
        .library(name: "BelugaCore", targets: ["BelugaCore"]),
    ],
    targets: [
        .target(name: "BelugaCore"),
        .testTarget(name: "BelugaCoreTests", dependencies: ["BelugaCore"], resources: [.copy("Fixtures")]),
    ]
)
