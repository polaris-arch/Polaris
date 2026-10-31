// Shared build-time selection. Both consumers execute this exact source; no
// second version/provider table may authorize an execution.
use std::path::{Path, PathBuf};
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
fn app_manifest_dir() -> PathBuf {
    repository_root().join("src-tauri")
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

    embed_validation_core_source();
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

fn temporary_desktop_default() -> bool {
    let path = app_manifest_dir().as_path().join("core-manifest.json");
    println!("cargo:rerun-if-changed={}", path.display());
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).expect("committed core selection is readable"))
            .expect("committed core selection is JSON");
    desktop_platform().is_some() && manifest.get("desktopTemporary").is_some()
}

fn upstream_desktop_default() -> bool {
    if desktop_platform().is_none() {
        return false;
    }
    let root = repository_root();
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("src-tauri/core-manifest.json")).expect("committed selection"),
    )
    .expect("committed selection JSON");
    let Some(selection) = manifest.get("desktopRelease") else {
        return false;
    };
    assert!(
        manifest.get("desktopTemporary").is_none(),
        "exclusive desktop selection required"
    );
    let path = selection["contractPath"]
        .as_str()
        .expect("formal contract path");
    assert!(
        matches!(
            path,
            "scripts/desktop-core/release-contract.json"
                | "scripts/desktop-core/pending-release-contract.json"
        ),
        "reviewed formal contract path required"
    );
    println!("cargo:rerun-if-changed={}", root.join(path).display());
    let raw = std::fs::read(root.join(path)).expect("formal contract is readable");
    assert_eq!(
        selection["contractSha256"].as_str(),
        Some(sha256_hex(&raw).as_str()),
        "formal contract digest differs"
    );
    let contract: serde_json::Value = serde_json::from_slice(&raw).expect("formal contract JSON");
    if contract["schema"] != "polaris-upstream-native-release-consumer-v1" {
        return false;
    }
    // Runtime and Windows source identity embed this canonical committed file.
    assert_eq!(
        path, "scripts/desktop-core/release-contract.json",
        "upstream installation must use the compiled canonical contract"
    );
    true
}

// Analysis-only builds have no frozen carrier. The explicit sentinel is never a
// usable installation and must not silently select the historical formal core.
const TEMPORARY_UNAVAILABLE: &[u8] = br#"{"schema":"polaris-temporary-desktop-unavailable-v1"}"#;
const FORMAL_UNAVAILABLE: &[u8] = br#"{"schema":"polaris-formal-desktop-unavailable-v1"}"#;

fn temporary_installation_pending(temporary_default: bool) -> bool {
    if !temporary_default
        || std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_some()
        || std::env::var_os("POLARIS_CORE_IDENTITY_FILE").is_some()
    {
        return false;
    }
    let directory = app_manifest_dir()
        .as_path()
        .parent()
        .expect("repository root")
        .join("resources")
        .join(desktop_platform().expect("desktop target"));
    for name in ["core-manifest.json", "candidate-carrier.json"] {
        let path = directory.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        if std::fs::symlink_metadata(&path).is_ok() {
            return false;
        }
        // Only true absence admits an unavailable analysis image. Permission or
        // I/O errors, and dangling symlinks above, must enter strict verification.
        if path
            .try_exists()
            .expect("inspect selected installation path")
        {
            return false;
        }
    }
    true
}

/// Technical candidate source facts are compiled only after the existing strict
/// consumer verifies the actual payloads and exact manual App/run context. This
/// is paired with a separately verified installation identity; neither authorizes managed launch.
fn embed_validation_core_source() {
    println!("cargo:rerun-if-env-changed=POLARIS_NATIVE_DNS_CANDIDATE");
    println!("cargo:rerun-if-env-changed=CORE_CANDIDATE");
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_ID");
    println!("cargo:rerun-if-env-changed=GITHUB_RUN_ATTEMPT");
    for key in [
        "POLARIS_NO_KERNEL_RUN",
        "POLARIS_BUILD_ID",
        "CARGO_TARGET_DIR",
        "GITHUB_ACTIONS",
        "GITHUB_EVENT_NAME",
        "GITHUB_REPOSITORY",
        "GITHUB_REPOSITORY_ID",
        "GITHUB_REF",
        "GITHUB_SHA",
        "GITHUB_WORKFLOW_REF",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("Cargo OUT_DIR"))
        .join("validation-core-source.json");
    let temporary_default = temporary_desktop_default();
    let mode = std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE");
    let raw = if temporary_installation_pending(temporary_default) {
        TEMPORARY_UNAVAILABLE.to_vec()
    } else if mode.is_none() && !temporary_default {
        b"null".to_vec()
    } else {
        assert!(
            desktop_platform().is_some(),
            "desktop candidate on unsupported target"
        );
        let candidate = if let Some(mode) = mode {
            assert_eq!(mode, "1", "explicit native DNS validation mode required");
            Some(std::env::var("CORE_CANDIDATE").expect("exact candidate App SHA"))
        } else {
            None
        };
        let root = repository_root();
        for path in [
            "scripts/desktop-core/native-dns-candidate.mjs",
            "scripts/desktop-core/native-dns-candidate.json",
            "scripts/desktop-core/native-dns-source-binding.json",
            "resources",
            "scripts/desktop-core",
            "scripts/prepare-core-identity.mjs",
            "scripts/lib/core-identity.mjs",
        ] {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
        let mut command = std::process::Command::new("node");
        command
            .arg(root.join("scripts/desktop-core/native-dns-candidate.mjs"))
            .arg("source");
        if let Some(candidate) = &candidate {
            command.arg(format!("--candidate={candidate}"));
        }
        let verified = command
            .current_dir(&root)
            .output()
            .expect("strict candidate source verifier must execute");
        assert!(
            verified.status.success(),
            "candidate source verification failed: {}",
            String::from_utf8_lossy(&verified.stderr)
        );
        let source: serde_json::Value =
            serde_json::from_slice(&verified.stdout).expect("verified candidate source JSON");
        if let Some(candidate) = candidate {
            assert_eq!(source["schema"], "polaris-native-dns-compiled-source-v1");
            assert_eq!(source["validationOnly"], true);
            assert_eq!(source["context"]["candidate"], candidate);
        } else {
            assert_eq!(source["schema"], "polaris-native-dns-compiled-source-v2");
            assert_eq!(source["validationOnly"], false);
            assert_eq!(source["purpose"], "temporary-default");
            assert_eq!(source["context"]["mode"], "temporary-default");
            assert_eq!(
                source["context"]["candidate"],
                std::env::var("POLARIS_BUILD_ID").expect("exact build identity")
            );
        }
        assert_eq!(source["AppAccepted"], false);
        assert_eq!(source["publicationEligible"], false);
        verified.stdout
    };
    std::fs::write(output, raw).expect("write compiled validation source identity");
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
    let temporary_default = temporary_desktop_default();
    let formal_default = upstream_desktop_default();
    let root = repository_root();
    if temporary_installation_pending(temporary_default || formal_default) {
        std::fs::write(
            output,
            if formal_default {
                FORMAL_UNAVAILABLE
            } else {
                TEMPORARY_UNAVAILABLE
            },
        )
        .expect("write unavailable installation identity");
        return;
    }
    let selected_path = std::env::var_os("POLARIS_CORE_IDENTITY_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            (temporary_default || formal_default).then(|| {
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
            let os = std::env::var("CARGO_CFG_TARGET_OS").expect("target OS");
            let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("target architecture");
            let platform = match (os.as_str(), arch.as_str()) {
                ("linux", "x86_64") => "linux",
                ("windows", "x86_64") => "win",
                ("macos", "x86_64") => "mac-x64",
                ("macos", "aarch64") => "mac-arm64",
                _ => panic!("selected desktop identity on unsupported target"),
            };
            if !temporary_default && std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_none() {
                let candidate_fingerprint = sha256_hex(
                    &std::fs::read(
                        repository_root().join("scripts/desktop-core/native-dns-candidate.json"),
                    )
                    .expect("candidate contract"),
                );
                assert_ne!(
                    manifest.core.provider_fingerprint, candidate_fingerprint,
                    "candidate installation cannot be compiled as a formal/default identity"
                );
            }
            if temporary_default || std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_some() {
                let root = repository_root();
                let manual = std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_some();
                let directory = root
                    .join(if manual {
                        "resources/.native-dns-app"
                    } else {
                        "resources"
                    })
                    .join(platform);
                assert_eq!(
                    manifest.core.provider_fingerprint,
                    sha256_hex(
                        &std::fs::read(
                            repository_root()
                                .join("scripts/desktop-core/native-dns-candidate.json")
                        )
                        .expect("candidate contract")
                    ),
                    "selected temporary contract fingerprint differs"
                );
                assert_eq!(
                    path,
                    directory.join("core-manifest.json"),
                    "exact candidate identity path required"
                );
                let verified = std::process::Command::new("node")
                    .arg(root.join("scripts/prepare-core-identity.mjs"))
                    .arg(format!("--platform={platform}"))
                    .arg(format!("--directory={}", directory.display()))
                    .arg("--mode=verify")
                    .current_dir(&root)
                    .output()
                    .expect("strict candidate App carrier verifier must execute");
                assert!(
                    verified.status.success(),
                    "candidate App identity verification failed: {}",
                    String::from_utf8_lossy(&verified.stderr)
                );
                assert_eq!(
                    raw,
                    std::fs::read(&path).expect("verified identity remains readable"),
                    "candidate identity changed during verification"
                );
            }
            if formal_default && std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_none() {
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
        None => {
            assert!(
                std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").is_none(),
                "candidate App requires a frozen actual helper/core installation identity"
            );
            b"null".to_vec()
        }
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
    assert!(
        !cfg!(feature = "formal-selection"),
        "selection features are mutually exclusive"
    );
    assert_eq!(
        desktop_platform(),
        Some("linux"),
        "local provider is Linux-only"
    );
    for key in [
        "POLARIS_NATIVE_DNS_CANDIDATE",
        "CORE_CANDIDATE",
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
    std::fs::write(output.join("validation-core-source.json"), b"null").unwrap();
}
