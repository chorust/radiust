// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "RadiustDesktop",
    platforms: [
        .macOS(.v14),
    ],
    products: [
        .executable(name: "RadiustDesktop", targets: ["RadiustDesktop"]),
    ],
    targets: [
        .executableTarget(
            name: "RadiustDesktop",
            dependencies: ["RadiustFFI"],
            path: "Sources/RadiustDesktop",
            linkerSettings: [
                // Link the in-process static library built by cargo on the Mac host.
                .linkedLibrary("radiust_ffi"),
                .unsafeFlags([
                    "-L../../target/release",
                    "-L../../target/debug",
                ]),
            ]
        ),
        .systemLibrary(
            name: "RadiustFFI",
            path: "Sources/RadiustFFI"
        ),
    ]
)
