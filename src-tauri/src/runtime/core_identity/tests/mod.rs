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
    assert!(selected_source_commit("win").is_ok());
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
    assert!(verify_bundled(&carrier.join(name)).unwrap().is_current());
    assert!(selected.helper_sha256.is_some());
    assert_eq!(selected.cleaner_sha256.is_some(), platform == "win");
}

#[test]
fn validated_bundle_observation_detects_sidecar_replacement_without_rehashing() {
    let dir = std::env::temp_dir().join(format!(
        "polaris-bundle-observation-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    let mut manifest = formal_manifest("linux").unwrap();
    assert_eq!(manifest.files.len(), 2);
    for file in &mut manifest.files {
        let bytes = b"original";
        std::fs::write(dir.join(&file.name), bytes).unwrap();
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
    use std::os::unix::fs::{symlink, DirBuilderExt};
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
    assert!(
        core.is_file(),
        "the normal bundled core candidate passes its existing file predicate"
    );
    let sidecar = dir.join(&manifest.files[1].name);
    if symbolic_link {
        let target = dir.join("regular-target");
        std::fs::write(&target, b"original").unwrap();
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
