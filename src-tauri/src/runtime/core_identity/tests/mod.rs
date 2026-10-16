use super::*;
#[test]
fn all_selected_desktop_rows_have_complete_content_identities() {
    for platform in ["linux", "win", "mac-x64", "mac-arm64"] {
        let manifest = selected_manifest(platform).expect("actual pinned row");
        assert_eq!(
            manifest.files.len(),
            if platform.starts_with("mac-") { 1 } else { 2 }
        );
        assert_eq!(
            selected_id(platform)
                .expect("framed identity")
                .as_str()
                .len(),
            64
        );
    }
    assert!(selected_manifest("android").is_err());
}

#[test]
fn validated_bundle_observation_detects_sidecar_replacement_without_rehashing() {
    let dir = std::env::temp_dir().join(format!(
        "polaris-bundle-observation-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir(&dir).unwrap();
    let mut manifest = selected_manifest("linux").unwrap();
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
