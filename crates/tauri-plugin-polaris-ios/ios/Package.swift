// swift-tools-version:5.3
import PackageDescription

let package = Package(
    name: "tauri-plugin-polaris-ios",
    platforms: [.iOS("17.0")],
    products: [.library(name: "tauri-plugin-polaris-ios", type: .static, targets: ["tauri-plugin-polaris-ios"])],
    dependencies: [.package(name: "Tauri", path: "../.tauri/tauri-api")],
    targets: [.target(name: "tauri-plugin-polaris-ios", dependencies: [.byName(name: "Tauri")], path: "Sources")]
)
