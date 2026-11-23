use super::*;
#[test]
fn administrative_arguments_are_exact_and_allow_a_root_sealed_manifest() {
    let args = |values: &[&str]| {
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    };
    #[cfg(unix)]
    let valid = args(&[
        "core-install",
        "--root",
        "/root/core",
        "--source",
        "/user/stage",
        "--manifest",
        "/user/stage/core-manifest.json",
    ]);
    #[cfg(windows)]
    let valid = args(&[
        "core-install",
        "--root",
        r"C:\core",
        "--source",
        r"C:\stage",
        "--manifest",
        r"C:\stage\core-manifest.json",
    ]);
    assert!(!InstallArgs::parse(&valid).unwrap().preserve_existing);
    let mut preserve = valid.clone();
    preserve.push("--preserve-existing".into());
    assert!(InstallArgs::parse(&preserve).unwrap().preserve_existing);
    preserve.push("--preserve-existing".into());
    assert!(InstallArgs::parse(&preserve).is_err());
    let mut valued = valid.clone();
    valued.push("--preserve-existing=true".into());
    assert!(InstallArgs::parse(&valued).is_err());
    let mut sealed = valid.clone();
    #[cfg(unix)]
    {
        sealed[6] = "/root/transaction/core-manifest.json".into();
    }
    #[cfg(windows)]
    {
        sealed[6] = r"C:\root-transaction\core-manifest.json".into();
    }
    assert!(InstallArgs::parse(&sealed).is_ok());
    let mut duplicate = valid.clone();
    duplicate[3] = "--root".into();
    assert!(InstallArgs::parse(&duplicate).is_err());
    let mut unrelated = valid.clone();
    unrelated[6] = valid[2].clone();
    assert!(InstallArgs::parse(&unrelated).is_err());
    let mut extra = valid;
    extra.push("--daemon".into());
    assert!(InstallArgs::parse(&extra).is_err());
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[test]
fn unsupported_installer_verbs_never_fall_through_to_daemon_mode() {
    for verb in [
        "sync-install-path",
        "validate-install-path",
        "snapshot-user",
        "restore-user",
    ] {
        assert_eq!(
            run(&[verb.to_owned()]),
            Some(std::process::ExitCode::FAILURE)
        );
    }
}

#[test]
fn linux_legacy_dispatch_is_before_daemon_and_cannot_fall_into_old_mac_cli() {
    let source = polaris_source_probe::crate_source!("core_cli.rs");
    assert!(
        source
            .find("crate::linux_legacy_maintenance::run(args)")
            .unwrap()
            < source
                .find("crate::mac_migration_guard::run(args)")
                .unwrap()
    );
    assert!(source.contains("verb.starts_with(\"linux-legacy-\")"));
    assert!(source.contains("Linux legacy maintenance requires Linux"));
    assert!(source.contains("pub(crate) fn install(args: &InstallArgs)"));
}

#[test]
#[cfg(not(target_os = "linux"))]
fn linux_legacy_administrative_verb_family_is_rejected_on_every_other_platform() {
    for verb in [
        "linux-legacy-classify",
        "linux-legacy-install",
        "linux-legacy-uninstall",
        "linux-legacy-unknown",
    ] {
        assert_eq!(run(&[verb.into()]), Some(std::process::ExitCode::FAILURE));
    }
}

#[test]
fn administrative_self_hash_cannot_select_a_foreign_core_id() {
    let Ok(platform) = current_platform() else {
        return;
    };
    let selected = polaris_core_supervisor::selected_core::selected_manifest(platform);
    match selected {
        Ok(core) => {
            let id = core.id(crate::core_hash::sha256_hex).unwrap();
            let mut manifest = InstalledCoreManifest {
                schema: "polaris-core-manifest-v1".into(),
                id: id.as_str().into(),
                app_version: "fixture".into(),
                core,
            };
            require_compiled_selection(&manifest, platform).unwrap();
            manifest.core.files[0].sha256 = "f".repeat(64);
            manifest.id = manifest
                .core
                .id(crate::core_hash::sha256_hex)
                .unwrap()
                .as_str()
                .into();
            assert!(require_compiled_selection(&manifest, platform).is_err());
        }
        Err(_) => {
            // Only the exact formal pre-install helper can use the content pin;
            // candidate/temporary failures still must not borrow a formal id.
            let core = polaris_core_supervisor::selected_core::formal_manifest(platform).unwrap();
            let id = core.id(crate::core_hash::sha256_hex).unwrap();
            let mut manifest = InstalledCoreManifest {
                schema: "polaris-core-manifest-v1".into(),
                id: id.as_str().into(),
                app_version: "fixture".into(),
                core,
            };
            let formal_bootstrap = polaris_core_supervisor::selected_core::selection_lane()
                == polaris_core_supervisor::selected_core::SelectionLane::Formal
                && polaris_core_supervisor::selected_core::COMPILED_INSTALLATION
                    == br#"{"schema":"polaris-formal-desktop-unavailable-v1"}"#;
            if formal_bootstrap {
                require_compiled_selection(&manifest, platform).unwrap();
                // A valid self-rehashed manifest is still a foreign selection.
                manifest.core.files[0].sha256 = "f".repeat(64);
                manifest.id = manifest
                    .core
                    .id(crate::core_hash::sha256_hex)
                    .unwrap()
                    .as_str()
                    .into();
            }
            assert!(require_compiled_selection(&manifest, platform).is_err());
            if formal_bootstrap {
                println!(
                    "POLARIS_TEST_FORMAL_PENDING_HELPER_SELECTION_ACCEPTED_FOREIGN_ID_REJECTED"
                );
            }
        }
    }
}

/// The IPC body installs a trusted owner's self-verifying manifest, and only
/// under the CoreId the client asked for.
#[cfg(target_os = "linux")]
#[test]
fn bundled_install_admits_only_the_requested_id_from_a_trusted_directory() {
    use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest};
    use std::os::unix::fs::PermissionsExt;
    let Some(_completion) = crate::core_store::exact_fixture_scope(
        module_path!(),
        "bundled_install_admits_only_the_requested_id_from_a_trusted_directory",
    ) else {
        return;
    };
    let root_dir = tempfile::tempdir().unwrap();
    let source_dir = tempfile::tempdir().unwrap();
    for dir in [root_dir.path(), source_dir.path()] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let root = root_dir.path().canonicalize().unwrap();
    let source = source_dir.path().canonicalize().unwrap();
    let owner = OwnerPolicy {
        uid: Some(nix::unistd::Uid::effective().as_raw()),
    };
    let files = [
        ("sing-box", b"inert-core".as_slice()),
        ("libcronet.so", b"inert-sidecar".as_slice()),
    ]
    .into_iter()
    .map(|(name, raw)| {
        std::fs::write(source.join(name), raw).unwrap();
        CoreFile {
            name: name.into(),
            bytes: raw.len() as u64,
            sha256: crate::core_hash::sha256_hex(raw),
        }
    })
    .collect();
    let core = CoreManifest {
        platform: "linux".into(),
        version: "1.15.0-alpha.12".into(),
        provider_fingerprint: "a".repeat(64),
        files,
        helper_sha256: None,
        cleaner_sha256: None,
    };
    let id = core.id(crate::core_hash::sha256_hex).unwrap();
    let manifest = InstalledCoreManifest {
        schema: "polaris-core-manifest-v1".into(),
        id: id.as_str().into(),
        app_version: "1.0.0".into(),
        core,
    };
    std::fs::write(
        source.join("core-manifest.json"),
        manifest.to_json().unwrap(),
    )
    .unwrap();
    for name in ["sing-box", "libcronet.so", "core-manifest.json"] {
        std::fs::set_permissions(source.join(name), std::fs::Permissions::from_mode(0o644))
            .unwrap();
    }
    let args = InstallArgs {
        root: root.clone(),
        source: source.clone(),
        manifest: source.join("core-manifest.json"),
        source_owner: None,
        preserve_existing: false,
    };
    let receipt = root.join("manifests").join(format!("{}.json", id.as_str()));
    let other = CoreId::parse(&"f".repeat(64)).unwrap();
    assert!(install_from(&args, owner, owner.uid, Some(&other)).is_err());
    assert!(!receipt.exists());
    assert!(install_from(&args, owner, owner.uid, Some(&id))
        .unwrap()
        .starts_with("core-installed "));
    assert!(receipt.exists());
}

/// Every ancestor of the trusted directory is checked. A link placed above the
/// group-writable level proves the walk stops there, not at a higher level.
#[cfg(unix)]
#[test]
fn bundled_install_rejects_a_group_writable_ancestor() {
    use std::os::unix::fs::PermissionsExt;
    let base = tempfile::tempdir().unwrap();
    let shared = base.path().join("real").join("shared");
    std::fs::create_dir_all(shared.join("core")).unwrap();
    std::os::unix::fs::symlink(base.path().join("real"), base.path().join("link")).unwrap();
    let source = base.path().join("link").join("shared").join("core");
    let owner = OwnerPolicy {
        uid: Some(nix::unistd::Uid::effective().as_raw()),
    };
    let id = CoreId::parse(&"a".repeat(64)).unwrap();
    let attempt = || {
        install_bundled(base.path(), source.to_str(), &id, owner)
            .unwrap_err()
            .to_string()
    };
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o775)).unwrap();
    assert_eq!(attempt(), "untrusted store owner or mode");
    std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(attempt(), "linked store object");
}
