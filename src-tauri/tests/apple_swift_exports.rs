//! The locked iOS source adapter is exercised on every CI host, without Swift.
#[path = "../../vendor/swift-rs/src-rs/ios_exports.rs"]
mod ios_exports;

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "polaris-swift-exports-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn package(&self, relative: &str, file: &str, symbols: &[&str]) -> PathBuf {
        let root = self.0.join(relative);
        let source = root.join(file);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(root.join("Package.swift"), "// frozen package manifest\n").unwrap();
        let body: String = symbols
            .iter()
            .map(|name| {
                format!("@_cdecl(\"{name}\")\nfunc entry_{name}() {{ retainedImplementation() }}\n")
            })
            .collect();
        fs::write(source, body).unwrap();
        root
    }
    fn out(&self) -> PathBuf {
        let out = self.0.join("cargo-out");
        fs::create_dir(&out).unwrap();
        out
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

const RUNTIME: &[&str] = &[
    "log_stdout",
    "register_plugin",
    "on_webview_created",
    "run_plugin_command",
];

#[test]
fn only_access_control_changes_and_ambiguous_or_missing_entries_refuse() {
    let source =
        "@_cdecl(\"bridge\")\nfunc entry() { realImplementation() }\nfunc unrelated() {}\n";
    let expected = source.replacen("\nfunc entry", "\npublic func entry", 1);
    assert_eq!(
        ios_exports::export_entries(source, &["bridge"]).unwrap(),
        expected
    );
    for bad in [
        source.replace("bridge", "different"),
        format!("{source}{source}"),
        expected,
    ] {
        assert!(ios_exports::export_entries(&bad, &["bridge"]).is_err());
    }
}

#[test]
fn all_locked_packages_keep_originals_and_relative_tauri_dependency() {
    for (name, version, file, symbols) in [
        ("Tauri", "2.11.5", "Sources/Tauri/Tauri.swift", RUNTIME),
        (
            "tauri-plugin-dialog",
            "2.7.2",
            "Sources/DialogPlugin.swift",
            &["init_plugin_dialog"][..],
        ),
        (
            "tauri-plugin-notification",
            "2.3.3",
            "Sources/NotificationPlugin.swift",
            &["init_plugin_notification"][..],
        ),
        (
            "tauri-plugin-shell",
            "2.3.5",
            "Sources/ShellPlugin.swift",
            &["init_plugin_shell"][..],
        ),
    ] {
        let fixture = Fixture::new();
        let source = fixture.package("crate/ios", file, symbols);
        let dependency = fixture.package(
            "crate/.tauri/tauri-api",
            "Sources/Tauri/Tauri.swift",
            RUNTIME,
        );
        let original = fs::read(source.join(file)).unwrap();
        let dependency_original = fs::read(dependency.join("Sources/Tauri/Tauri.swift")).unwrap();
        fs::write(source.join("Package.resolved"), "frozen resolver input").unwrap();
        let out = fixture.out();
        assert!(ios_exports::stage(name, "wrong-version", &source, &out).is_err());
        assert!(!out.join("polaris-c-exports").exists());
        let staged = ios_exports::stage(name, version, &source, &out).unwrap();
        assert_eq!(fs::read(source.join(file)).unwrap(), original);
        assert_eq!(
            fs::read(dependency.join("Sources/Tauri/Tauri.swift")).unwrap(),
            dependency_original
        );
        assert_eq!(
            fs::read(staged.join("Package.swift")).unwrap(),
            fs::read(source.join("Package.swift")).unwrap()
        );
        assert_eq!(
            fs::read(staged.join("Package.resolved")).unwrap(),
            b"frozen resolver input"
        );
        assert_eq!(
            fs::read(staged.join(file)).unwrap(),
            ios_exports::export_entries(std::str::from_utf8(&original).unwrap(), symbols)
                .unwrap()
                .as_bytes()
        );
        if name != "Tauri" {
            assert_eq!(
                fs::read(
                    staged
                        .parent()
                        .unwrap()
                        .join(".tauri/tauri-api/Sources/Tauri/Tauri.swift")
                )
                .unwrap(),
                ios_exports::export_entries(
                    std::str::from_utf8(&dependency_original).unwrap(),
                    RUNTIME
                )
                .unwrap()
                .as_bytes()
            );
        }
        fs::write(staged.join("Sources/stale.swift"), "stale").unwrap();
        ios_exports::stage(name, version, &source, &out).unwrap();
        assert!(!staged.join("Sources/stale.swift").exists());
    }
}

#[test]
fn unrelated_swift_package_remains_exact_and_large_inputs_refuse_before_staging() {
    let fixture = Fixture::new();
    let source = fixture.package("crate/ios", "Sources/Tauri/Tauri.swift", RUNTIME);
    let out = fixture.out();
    assert_eq!(
        ios_exports::stage("unrelated", "1", &source, &out).unwrap(),
        source
    );
    assert!(!out.join("polaris-c-exports").exists());
    fs::write(
        source.join("Sources/oversized.swift"),
        vec![0; 4 * 1024 * 1024],
    )
    .unwrap();
    assert!(ios_exports::stage("Tauri", "2.11.5", &source, &out).is_err());
    assert!(!out.join("polaris-c-exports").exists());
}

#[cfg(unix)]
#[test]
fn source_and_retained_output_symlinks_refuse_without_touching_target() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let source = fixture.package("crate/ios", "Sources/Tauri/Tauri.swift", RUNTIME);
    let out = fixture.out();
    let guard = fixture.0.join("guard");
    fs::create_dir(&guard).unwrap();
    fs::write(guard.join("keep"), b"original").unwrap();
    symlink(&guard, out.join("polaris-c-exports")).unwrap();
    assert!(ios_exports::stage("Tauri", "2.11.5", &source, &out).is_err());
    assert_eq!(fs::read(guard.join("keep")).unwrap(), b"original");
    fs::remove_file(out.join("polaris-c-exports")).unwrap();
    symlink(guard.join("keep"), source.join("Sources/redirected.swift")).unwrap();
    assert!(ios_exports::stage("Tauri", "2.11.5", &source, &out).is_err());
    assert!(!out.join("polaris-c-exports").exists());
}
