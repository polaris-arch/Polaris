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
    let mut manifest = selected_manifest("linux").unwrap();
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
