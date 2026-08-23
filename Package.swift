// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "ClipboardVault",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "clipboard-vault", targets: ["ClipboardVaultCLI"]),
        .executable(name: "ClipboardVault", targets: ["ClipboardVaultApp"]),
    ],
    targets: [
        .target(name: "VaultCore"),
        .executableTarget(name: "ClipboardVaultCLI", dependencies: ["VaultCore"]),
        .executableTarget(name: "ClipboardVaultApp", dependencies: ["VaultCore"]),
        .testTarget(name: "VaultCoreTests", dependencies: ["VaultCore"]),
    ]
)
