fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("ios") {
        tauri_plugin::Builder::new(&[]).ios_path("ios").build();
    }
}
