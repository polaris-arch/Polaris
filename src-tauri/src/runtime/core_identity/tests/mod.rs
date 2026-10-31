use super::*;

#[test]
fn upstream_installation_provider_binds_exact_compiled_contract_in_its_own_domain() {
    let raw = r#"{"schema":"polaris-upstream-native-release-consumer-v1"}"#;
    let contract: serde_json::Value = serde_json::from_str(raw).unwrap();
    let row = serde_json::json!({});
    let expected = polaris_updater::sha256_hex(
        [
            b"polaris-upstream-native-installation-v1\0".as_slice(),
            raw.as_bytes(),
        ]
        .concat()
        .as_slice(),
    );
    assert_eq!(
        formal_provider_fingerprint(raw, &contract, &row).unwrap(),
        expected
    );
    assert_eq!(
        expected,
        "01bb2a80eabb78c22d852abed88e74b60081cc1e145268ee8d1901e77974b978"
    );
    assert_ne!(expected, polaris_updater::sha256_hex(raw.as_bytes()));
    assert_ne!(
        formal_provider_fingerprint(&format!("{raw}\n"), &contract, &row).unwrap(),
        expected
    );
    for name in ["providerFingerprint", "providerIdentity"] {
        let mut bad = row.clone();
        bad[name] = "a".repeat(64).into();
        assert!(formal_provider_fingerprint(raw, &contract, &bad).is_err());
    }
    let historical = serde_json::json!({"schema":"polaris-box-release-consumer-contract-v1"});
    assert_eq!(
        formal_provider_fingerprint(
            "historical",
            &historical,
            &serde_json::json!({"providerFingerprint":"a".repeat(64)})
        )
        .unwrap(),
        "a".repeat(64)
    );
}

#[test]
fn selected_source_is_compiled_not_a_runtime_environment_override() {
    let formal: serde_json::Value = serde_json::from_str(RELEASE_CONTRACT).unwrap();
    assert_eq!(
        selected_source_from(b"null", "win").unwrap(),
        formal["platforms"]["win"]["sourceCommit"].as_str().unwrap()
    );
    let candidate = serde_json::json!({
        "schema": "polaris-native-dns-compiled-source-v1", "validationOnly": true,
        "AppAccepted": false, "publicationEligible": false,
        "platforms": ["linux", "win", "mac-x64", "mac-arm64"],
        "sourceCommit": "a".repeat(40)
    });
    let raw = serde_json::to_vec(&candidate).unwrap();
    assert_eq!(selected_source_from(&raw, "win").unwrap(), "a".repeat(40));
    assert!(selected_source_from(&raw, "android").is_err());
    for field in [
        "schema",
        "sourceCommit",
        "platforms",
        "validationOnly",
        "AppAccepted",
    ] {
        let mut invalid = candidate.clone();
        invalid[field] = serde_json::Value::Null;
        assert!(selected_source_from(&serde_json::to_vec(&invalid).unwrap(), "win").is_err());
    }
    let mut temporary = candidate.clone();
    temporary["schema"] = "polaris-native-dns-compiled-source-v2".into();
    temporary["validationOnly"] = false.into();
    temporary["purpose"] = "temporary-default".into();
    temporary["context"] = serde_json::json!({"mode": "temporary-default"});
    assert_eq!(
        selected_source_from(&serde_json::to_vec(&temporary).unwrap(), "win").unwrap(),
        "a".repeat(40)
    );
    for field in [
        "purpose",
        "context",
        "publicationEligible",
        "sourceCommit",
        "validationOnly",
    ] {
        let mut invalid = temporary.clone();
        invalid[field] = serde_json::Value::Null;
        assert!(selected_source_from(&serde_json::to_vec(&invalid).unwrap(), "win").is_err());
    }
    assert!(selected_source_from(
        br#"{"schema":"polaris-temporary-desktop-unavailable-v1"}"#,
        "win"
    )
    .is_err());
    if polaris_core_supervisor::selected_core::selection_lane()
        == polaris_core_supervisor::selected_core::SelectionLane::LocalDevelopment
    {
        assert!(
            selected_source_commit("win").is_err(),
            "local source selection is Linux-only"
        );
    } else {
        assert!(selected_source_commit("win").is_ok());
    }
}

#[test]
fn all_selected_desktop_rows_have_complete_content_identities() {
    for platform in ["linux", "win", "mac-x64", "mac-arm64"] {
        let formal = formal_manifest(platform).expect("actual formal pinned row");
        assert_eq!(
            formal.files.len(),
            if platform.starts_with("mac-") { 1 } else { 2 }
        );
        assert_eq!(
            formal
                .id(polaris_updater::sha256_hex)
                .unwrap()
                .as_str()
                .len(),
            64
        );
        if COMPILED_INSTALLATION == b"null" || platform == current_platform().unwrap() {
            let manifest = selected_manifest(platform).expect("actual selected native row");
            assert_eq!(manifest.files.len(), formal.files.len());
            assert_eq!(selected_id(platform).unwrap().as_str().len(), 64);
        } else {
            assert!(
                selected_manifest(platform).is_err(),
                "foreign compiled installation must reject"
            );
        }
    }
    assert!(selected_manifest("android").is_err());
}

#[test]
fn candidate_compiled_identity_verifies_actual_carrier_without_a_runtime_override() {
    if polaris_core_supervisor::selected_core::selection_lane()
        == polaris_core_supervisor::selected_core::SelectionLane::LocalDevelopment
    {
        assert_eq!(COMPILED_VALIDATION_SOURCE, b"null");
        assert_eq!(
            selected_payload_dirs(&["old-formal-path"]).unwrap(),
            vec![".local-development/linux"]
        );
        let core = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("resources/.local-development/linux/sing-box");
        assert!(
            verify_bundled(&core).is_ok(),
            "the actual compiled local carrier must verify without an environment override"
        );
        assert!(selected_manifest("win").is_err());
        assert!(selected_manifest("mac-arm64").is_err());
        return;
    }
    if COMPILED_VALIDATION_SOURCE == b"null" {
        assert_eq!(selected_payload_dirs(&["linux"]).unwrap(), vec!["linux"]);
        return;
    }
    let platform = current_platform().unwrap();
    let selected = selected_manifest(platform).unwrap();
    let source: serde_json::Value = serde_json::from_slice(COMPILED_VALIDATION_SOURCE).unwrap();
    assert_eq!(
        selected.provider_fingerprint,
        source["bindingSha256"].as_str().unwrap()
    );
    assert_ne!(
        selected.provider_fingerprint,
        formal_manifest(platform).unwrap().provider_fingerprint
    );
    let dirs = selected_payload_dirs(&["old-formal-path"]).unwrap();
    let expected_directory = if source["schema"] == "polaris-native-dns-compiled-source-v2" {
        platform.to_owned()
    } else {
        format!(".native-dns-app/{platform}")
    };
    assert_eq!(dirs, vec![expected_directory]);
    let carrier = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../resources")
        .join(&dirs[0]);
    let name = if platform == "win" {
        "sing-box.exe"
    } else {
        "sing-box"
    };
    assert!(verify_directory_observed(&carrier, &selected)
        .unwrap()
        .is_current());
    assert!(
        verify_bundled(&carrier.join(name)).is_err(),
        "old candidate bytes cannot claim the new loader capability"
    );
    assert!(selected.helper_sha256.is_some());
    assert_eq!(selected.cleaner_sha256.is_some(), platform == "win");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Actual compiled selection + actual carrier bytes, with every core
        // file absent from the maintenance source directory.
        let private = std::env::temp_dir().join(format!(
            "polaris-compiled-maintenance-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        let helper = private.join("polaris-helper");
        std::fs::copy(carrier.join("polaris-helper"), &helper).unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let utility = maintenance_utility_from(&helper, platform, &selected).unwrap();
        assert_eq!(Some(utility.helper_sha256), selected.helper_sha256);
        assert!(selected
            .files
            .iter()
            .all(|file| !private.join(&file.name).exists()));
        std::fs::write(&helper, b"substituted source").unwrap();
        assert!(maintenance_utility_from(&helper, platform, &selected).is_err());
        std::fs::remove_dir_all(private).unwrap();
    }
}

#[test]
fn validated_bundle_observation_detects_sidecar_replacement_without_rehashing() {
    let dir = std::env::temp_dir().join(format!(
        "polaris-bundle-observation-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let mut manifest = formal_manifest("linux").unwrap();
    assert_eq!(manifest.files.len(), 2);
    for file in &mut manifest.files {
        let bytes = b"original";
        std::fs::write(dir.join(&file.name), bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.join(&file.name), std::fs::Permissions::from_mode(0o600))
                .unwrap();
        }
        file.bytes = bytes.len() as u64;
        file.sha256 = polaris_updater::sha256_hex(bytes);
    }
    // This is the real existing content verifier, not a metadata-only admission.
    let observation = verify_directory_observed(&dir, &manifest).unwrap();
    for _ in 0..64 {
        assert!(observation.is_current());
    }
    let sidecar = dir.join(&manifest.files[1].name);
    let modified = std::fs::metadata(&sidecar).unwrap().modified().unwrap();
    let replacement = dir.join("replacement");
    std::fs::write(&replacement, b"replaced").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    std::fs::rename(&replacement, &sidecar).unwrap();
    assert!(!observation.is_current());
    // Reacquisition still rejects the changed payload with a full content check.
    assert!(verify_directory_observed(&dir, &manifest).is_err());
    drop(observation);
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "linux")]
fn linux_bundle_with_special_sidecar(symbolic_link: bool) -> (PathBuf, CoreManifest) {
    use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "polaris-bundle-special-sidecar-{}",
            uuid::Uuid::new_v4()
        ));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    let mut manifest = formal_manifest("linux").unwrap();
    assert_eq!(
        manifest.files.len(),
        2,
        "actual Linux bundled layout has a core and sidecar"
    );
    for file in &mut manifest.files {
        file.bytes = 8;
        file.sha256 = polaris_updater::sha256_hex(b"original");
    }
    let core = dir.join(&manifest.files[0].name);
    std::fs::write(&core, b"original").unwrap();
    std::fs::set_permissions(&core, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        core.is_file(),
        "the normal bundled core candidate passes its existing file predicate"
    );
    let sidecar = dir.join(&manifest.files[1].name);
    if symbolic_link {
        let target = dir.join("regular-target");
        std::fs::write(&target, b"original").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &sidecar).unwrap();
    } else {
        nix::unistd::mkfifo(
            &sidecar,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .unwrap();
        // Deliberately never create/open a writer. A plain blocking read-open
        // would wait forever before the original content verifier is reached.
    }
    (dir, manifest)
}

#[cfg(target_os = "linux")]
fn rejects_linux_special_sidecar_without_blocking(symbolic_link: bool) {
    let (dir, manifest) = linux_bundle_with_special_sidecar(symbolic_link);
    // The ordinary first member must pass custody before the special sidecar
    // is rejected; permissive umasks must not make this a masked negative.
    let payload = open_payload_directory(&dir).unwrap();
    let (core, bytes) = payload.open_regular(&manifest.files[0].name).unwrap();
    assert_eq!(bytes, 8);
    drop(core);
    drop(payload);
    let worker_dir = dir.clone();
    let sidecar = dir.join(&manifest.files[1].name);
    let (sent, received) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::spawn(move || {
        let result = verify_directory_observed(&worker_dir, &manifest);
        sent.send(result).unwrap();
    });
    // This bounded receive is a failure watchdog, not a sleep or a FIFO writer.
    let result = received
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("Linux bundle observation blocked before rejecting the special sidecar");
    worker.join().unwrap();
    let reason =
        result.expect_err("a FIFO or symbolic link cannot become an observed regular payload");
    assert!(
        reason.starts_with("CORE_BUNDLE_MISMATCH: cannot observe payload:"),
        "must reject at safe observation capture, before following/reading the sidecar: {reason}"
    );
    assert!(
        std::fs::symlink_metadata(&sidecar).is_ok(),
        "reject the sidecar; do not delete it to bypass validation"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bundled_fifo_sidecar_without_writer_is_rejected_without_blocking() {
    rejects_linux_special_sidecar_without_blocking(false);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_bundled_symlink_sidecar_is_rejected_before_capture_without_blocking() {
    rejects_linux_special_sidecar_without_blocking(true);
}

#[cfg(unix)]
#[test]
fn maintenance_uses_selected_helper_pin_without_core_files_and_rejects_before_executor() {
    use polaris_helper_client::{ClientError, Executor, HelperManager, MaintenanceOutcome, SysOps};
    use polaris_helper_proto::Platform;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct NoSystem;
    impl SysOps for NoSystem {
        fn exists(&self, _: &Path) -> bool {
            panic!("no system lookup")
        }
        fn start_service(&self, _: &str) -> Result<(), String> {
            panic!("no service")
        }
        fn stop_service(&self, _: &str) -> Result<(), String> {
            panic!("no service")
        }
        fn is_loaded(&self, _: &str) -> bool {
            panic!("no service")
        }
        fn service_exists(&self, _: &str) -> bool {
            panic!("no service")
        }
    }
    struct InertExecutor(AtomicUsize);
    impl Executor for InertExecutor {
        fn execute(&self, _: &[String]) -> Result<(String, i32), ClientError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok((String::new(), 0))
        }
    }
    let dir =
        std::env::temp_dir().join(format!("polaris-maintenance-pin-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let source = dir.join("polaris-helper");
    let trusted = b"inert bundled helper bytes";
    std::fs::write(&source, trusted).unwrap();
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut selected = formal_manifest("linux").unwrap();
    selected.helper_sha256 = Some(polaris_updater::sha256_hex(trusted));
    assert!(selected.files.iter().all(|f| !dir.join(&f.name).exists()));
    let executor = InertExecutor(AtomicUsize::new(0));
    let manager = HelperManager::new(Platform::Mac, dir.join("absent-token"), Box::new(NoSystem));
    let attempt = |path: &Path,
                   platform: &str,
                   manifest: &CoreManifest|
     -> Result<MaintenanceOutcome, String> {
        let utility = maintenance_utility_from(path, platform, manifest)?;
        manager
            .uninstall_with_maintenance_outcome(&dir, Some(&utility), &executor)
            .map_err(|e| format!("{e:?}"))
    };
    assert_eq!(
        attempt(&source, "linux", &selected).unwrap(),
        MaintenanceOutcome::RetiredDataPreserved
    );
    assert_eq!(executor.0.load(Ordering::SeqCst), 1);
    let reject = |path: &Path, platform: &str, manifest: &CoreManifest| {
        let before = executor.0.load(Ordering::SeqCst);
        assert!(attempt(path, platform, manifest)
            .unwrap_err()
            .starts_with("CORE_BUNDLE_MISMATCH:"));
        assert_eq!(
            executor.0.load(Ordering::SeqCst),
            before,
            "no elevation for rejected source"
        );
    };
    let mut missing = selected.clone();
    missing.helper_sha256 = None;
    reject(&source, "linux", &missing);
    reject(&source, "mac-arm64", &selected);
    reject(Path::new("polaris-helper"), "linux", &selected);
    let wrong_name = dir.join("wrong-helper");
    std::fs::write(&wrong_name, trusted).unwrap();
    std::fs::set_permissions(&wrong_name, std::fs::Permissions::from_mode(0o600)).unwrap();
    reject(&wrong_name, "linux", &selected);
    std::fs::write(&source, b"substituted helper").unwrap();
    reject(&source, "linux", &selected);
    std::fs::remove_file(&source).unwrap();
    symlink(&wrong_name, &source).unwrap();
    reject(&source, "linux", &selected);
    std::fs::remove_file(&source).unwrap();
    std::fs::hard_link(&wrong_name, &source).unwrap();
    reject(&source, "linux", &selected);
    std::fs::remove_file(&source).unwrap();
    let oversized = std::fs::File::create(&source).unwrap();
    oversized.set_len(MAX_MAINTENANCE_HELPER_BYTES + 1).unwrap();
    drop(oversized);
    std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o600)).unwrap();
    // Pin the sparse zero contents too: deleting the size fence would admit
    // this source and reach the executor, rather than merely fail its digest.
    let mut oversized_selection = selected.clone();
    oversized_selection.helper_sha256 =
        Some("da6ce8755151acd05195db67ebce3ee0fb5f4012e71e821cc5750f3304eaf41e".into());
    reject(&source, "linux", &oversized_selection);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn recovery_selection_classifies_io_busy_and_damage_without_claiming_unsupported_can_be_repaired() {
    use polaris_core_supervisor::admitted_payload::AdmissionErrorKind as K;
    for (name, kind) in [
        ("Changed", K::Changed),
        ("Busy", K::Busy),
        ("Io", K::Io),
        ("Unknown", K::Unknown),
    ] {
        let error = selection_error(
            format!("CORE_ADMISSION_{name}: selected recovery unavailable"),
            ExecPurpose::CheckConfig,
        );
        assert_eq!(error.kind, kind);
        assert_eq!(error.kind.is_payload_damage(), name == "Changed");
    }
    assert_eq!(
        selection_error(
            "CORE_RECOVERY_UNSUPPORTED: user ACL policy absent".into(),
            ExecPurpose::RunMain
        )
        .kind,
        K::UnsupportedExactObject
    );
}
