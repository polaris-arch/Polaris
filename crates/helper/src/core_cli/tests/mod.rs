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
            // A missing candidate carrier must not fall back to a formal id.
            let core = polaris_core_supervisor::selected_core::formal_manifest(platform).unwrap();
            let id = core.id(crate::core_hash::sha256_hex).unwrap();
            let manifest = InstalledCoreManifest {
                schema: "polaris-core-manifest-v1".into(),
                id: id.as_str().into(),
                app_version: "fixture".into(),
                core,
            };
            assert!(require_compiled_selection(&manifest, platform).is_err());
        }
    }
}
