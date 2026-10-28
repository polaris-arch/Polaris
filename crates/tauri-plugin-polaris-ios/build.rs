#[path = "swift_package.rs"]
mod swift_package;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        println!("cargo:rerun-if-changed=ios/Package.swift");
        println!("cargo:rerun-if-changed=ios/Sources");
        let package = swift_package::stage(&manifest.join("ios"), &out)
            .expect("cannot stage the frozen iOS Swift package");
        println!("cargo:rerun-if-changed=ios/Package.resolved");
        tauri_plugin::Builder::new(&[]).ios_path(&package).build();
        swift_package::verify_resolution(&manifest.join("ios"), &package)
            .expect("SwiftPM dependency resolution differs from the frozen lock");
    }
}
