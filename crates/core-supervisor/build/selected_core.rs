// Shared build-time selection. Both consumers execute this exact source; no
// second version/provider table may authorize an execution.
use std::path::PathBuf;
// The build consumer verifies the two complete document pins. The runtime
// consumer of this same generated type validates every remaining field.
#[expect(
    dead_code,
    reason = "build validates selection and contract document pins; runtime validates remaining generated fields"
)]
#[path = "../src/selected_core/development_config.rs"]
mod development_configuration;
fn repository_root() -> PathBuf {
    let dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    if dir.file_name().is_some_and(|name| name == "src-tauri") {
        dir.parent().unwrap().to_path_buf()
    } else {
        dir.parent().unwrap().parent().unwrap().to_path_buf()
    }
}
pub fn emit() {
    if cfg!(feature = "local-development-selection") {
        emit_local_development();
        return;
    }
    let root = repository_root();
    assert!(
        !root.join("resources/.local-development").exists(),
        "FormalSelection cannot consume a local development slot"
    );
    for name in [
        "local-development-selection.json",
        "local-development-contract.json",
    ] {
        std::fs::write(
            PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join(name),
            b"null",
        )
        .unwrap();
    }

    embed_selected_core_identity();
    embed_recovery_inventory();
}
fn desktop_platform() -> Option<&'static str> {
    match (
        std::env::var("CARGO_CFG_TARGET_OS").as_deref(),
        std::env::var("CARGO_CFG_TARGET_ARCH").as_deref(),
    ) {
        (Ok("linux"), Ok("x86_64")) => Some("linux"),
        (Ok("windows"), Ok("x86_64")) => Some("win"),
        (Ok("macos"), Ok("x86_64")) => Some("mac-x64"),
        (Ok("macos"), Ok("aarch64")) => Some("mac-arm64"),
        _ => None,
    }
}

const CONTRACT_PATH: &str = "scripts/desktop-core/release-contract.json";

// The committed selection is the contract path and digest; the build verifies
// both before any installation identity can be compiled.
fn formal_desktop_selection() -> bool {
    if desktop_platform().is_none() {
        return false;
    }
    let root = repository_root();
    let manifest_path = root.join("src-tauri/core-manifest.json");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest_path).expect("committed selection"))
            .expect("committed selection JSON");
    let selection = &manifest["desktopRelease"];
    assert_eq!(
        selection["contractPath"].as_str(),
        Some(CONTRACT_PATH),
        "reviewed formal contract path required"
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join(CONTRACT_PATH).display()
    );
    let raw = std::fs::read(root.join(CONTRACT_PATH)).expect("formal contract is readable");
    assert_eq!(
        selection["contractSha256"].as_str(),
        Some(sha256_hex(&raw).as_str()),
        "formal contract digest differs"
    );
    true
}

// Analysis-only builds have no frozen carrier. The explicit sentinel is never a
// usable installation.
const FORMAL_UNAVAILABLE: &[u8] = br#"{"schema":"polaris-formal-desktop-unavailable-v1"}"#;

fn installation_pending(formal_default: bool) -> bool {
    if !formal_default || std::env::var_os("POLARIS_CORE_IDENTITY_FILE").is_some() {
        return false;
    }
    let path = repository_root()
        .join("resources")
        .join(desktop_platform().expect("desktop target"))
        .join("core-manifest.json");
    println!("cargo:rerun-if-changed={}", path.display());
    if std::fs::symlink_metadata(&path).is_ok() {
        return false;
    }
    // Only true absence admits an unavailable analysis image. Permission or
    // I/O errors, and dangling symlinks above, must enter strict verification.
    !path
        .try_exists()
        .expect("inspect selected installation path")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    use std::fmt::Write;
    let mut hex = String::with_capacity(64);
    for byte in sha2::Sha256::digest(bytes) {
        write!(&mut hex, "{byte:02x}").expect("write digest to String");
    }
    hex
}

fn embed_selected_core_identity() {
    println!("cargo:rerun-if-env-changed=POLARIS_CORE_IDENTITY_FILE");
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join("selected-core-manifest.json");
    let formal_default = formal_desktop_selection();
    let root = repository_root();
    if installation_pending(formal_default) {
        std::fs::write(output, FORMAL_UNAVAILABLE)
            .expect("write unavailable installation identity");
        return;
    }
    let selected_path = std::env::var_os("POLARIS_CORE_IDENTITY_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            formal_default.then(|| {
                root.join("resources")
                    .join(desktop_platform().expect("desktop target"))
                    .join("core-manifest.json")
            })
        });
    let raw = match selected_path {
        Some(path) => {
            println!("cargo:rerun-if-changed={}", path.display());
            let raw = std::fs::read(&path).expect("selected installation manifest is readable");
            let manifest =
                polaris_helper_proto::core_manifest::InstalledCoreManifest::parse(&raw, sha256_hex)
                    .expect("selected installation manifest has an exact content identity");
            let platform =
                desktop_platform().expect("selected desktop identity on unsupported target");
            if formal_default {
                let directory = root.join("resources").join(platform);
                assert_eq!(
                    path,
                    directory.join("core-manifest.json"),
                    "exact formal identity path required"
                );
                let verified = std::process::Command::new("node")
                    .arg(root.join("scripts/prepare-core-identity.mjs"))
                    .arg(format!("--platform={platform}"))
                    .arg(format!("--directory={}", directory.display()))
                    .arg("--mode=verify")
                    .current_dir(&root)
                    .output()
                    .expect("strict formal App carrier verifier must execute");
                assert!(
                    verified.status.success(),
                    "formal App identity verification failed: {}",
                    String::from_utf8_lossy(&verified.stderr)
                );
                assert_eq!(
                    raw,
                    std::fs::read(&path).expect("verified formal identity readable"),
                    "formal identity changed during verification"
                );
            }
            assert_eq!(
                manifest.core.platform, platform,
                "selected core target differs"
            );
            assert!(
                manifest.core.helper_sha256.is_some(),
                "actual helper hash required"
            );
            if platform == "win" {
                assert!(
                    manifest.core.cleaner_sha256.is_some(),
                    "actual cleaner hash required"
                );
            }
            raw
        }
        // Library tests/mobile/debug source compilation do not create a desktop
        // installer. Actual installers always supply this frozen input in package.yml.
        None => b"null".to_vec(),
    };
    std::fs::write(output, raw).expect("write compiled selected core identity");
}

fn embed_recovery_inventory() {
    let output = PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join("selected-core-recovery-inventory.json");
    let raw = if let Some(platform) = desktop_platform() {
        let path = repository_root()
            .join("resources/core-recovery")
            .join(platform)
            .join("inventory.json");
        println!("cargo:rerun-if-changed={}", path.display());
        match std::fs::read(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => b"null".to_vec(),
            Err(error) => panic!("compiled recovery inventory unavailable: {error}"),
        }
    } else {
        b"null".to_vec()
    };
    std::fs::write(output, raw).expect("write compiled recovery inventory");
}

fn emit_local_development() {
    const {
        assert!(
            !(cfg!(feature = "local-development-selection") && cfg!(feature = "formal-selection")),
            "selection features are mutually exclusive"
        );
    }
    assert_eq!(
        desktop_platform(),
        Some("linux"),
        "local provider is Linux-only"
    );
    for key in [
        "POLARIS_CORE_IDENTITY_FILE",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "GITHUB_ACTIONS",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
        assert!(
            std::env::var_os(key).is_none(),
            "local compilation refuses external selection/workflow inputs: {key}"
        );
    }
    let root = repository_root();
    if std::env::var("CARGO_PKG_NAME").as_deref() == Ok("polaris") {
        // This fixed compiler overlay follows the typed lane. External config
        // cannot select it, add a carrier, or choose an execution policy.
        assert!(
            std::env::var_os("TAURI_CONFIG").is_none(),
            "local compilation refuses an external resource overlay"
        );
        let config = serde_json::json!({"bundle":{"resources":[
            "../resources/.local-development/", "../resources/dashboard/", "../resources/data/",
            "../THIRD-PARTY-LICENSES.md", "../NOTICE", "../LICENSE"
        ]}});
        std::env::set_var("TAURI_CONFIG", config.to_string());
    }
    let installed = cfg!(feature = "installed-development-selection")
        || std::env::var("CARGO_PKG_NAME").as_deref() == Ok("polaris");
    let command = std::process::Command::new("node")
        .arg(root.join("scripts/desktop-core/local-development.mjs"))
        .arg(if installed {
            "verify-installation"
        } else {
            "verify"
        })
        .current_dir(&root)
        .output()
        .expect("local compiler verifier executes");
    assert!(
        command.status.success(),
        "local development inputs failed: {}",
        String::from_utf8_lossy(&command.stderr)
    );
    let directory = root.join("resources/.local-development/linux");
    let pin = development_configuration::COMPILED
        .as_ref()
        .expect("actual local selection compiler pins");
    let output = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    for (source, target) in [
        ("selection.json", "local-development-selection.json"),
        ("contract.json", "local-development-contract.json"),
        ("inventory.json", "selected-core-recovery-inventory.json"),
    ] {
        let path = directory.join(source);
        println!("cargo:rerun-if-changed={}", path.display());
        let data = std::fs::read(path).unwrap();
        if source == "selection.json" {
            assert_eq!(
                sha256_hex(&data),
                pin.selection_sha256,
                "immutable local selection pin differs"
            );
        }
        if source == "contract.json" {
            assert_eq!(
                sha256_hex(&data),
                pin.contract_sha256,
                "immutable local contract pin differs"
            );
        }
        std::fs::write(output.join(target), data).unwrap();
    }
    println!(
        "cargo:rerun-if-changed={}",
        root.join("scripts/desktop-core/local-development.mjs")
            .display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("scripts/desktop-core/local-development.py")
            .display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("scripts/desktop-core/local-development-source.json")
            .display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        root.join("crates/core-supervisor/src/selected_core/development_config.rs")
            .display()
    );
    let installation = if installed {
        let path = directory.join("core-manifest.json");
        println!("cargo:rerun-if-changed={}", path.display());
        std::fs::read(path).expect("actual final Helper installation identity")
    } else {
        b"null".to_vec()
    };
    // The pure Helper compilation never embeds its own future digest.
    std::fs::write(output.join("selected-core-manifest.json"), installation).unwrap();
}
