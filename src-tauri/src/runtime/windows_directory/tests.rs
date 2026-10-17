use super::*;
fn test_root() -> Identity {
    Identity {
        volume: 7,
        file_id: "a".repeat(32),
    }
}
fn marker() -> Marker {
    Marker {
        version: 1,
        platform: PLATFORM.into(),
        server_id: "node".into(),
        incarnation: "b".repeat(32),
        directory_epoch: "c".repeat(32),
        root_volume: 7,
        root_file_id: "a".repeat(32),
        directory_volume: 7,
        directory_file_id: "d".repeat(32),
        phase: "Open".into(),
        delete_intent_nonce: String::new(),
    }
}
fn line(marker: &Marker) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(marker).unwrap();
    bytes.push(b'\n');
    bytes
}
#[test]
fn journal_rejects_torn_duplicate_unknown_and_rollback() {
    let open = marker();
    let data = line(&open);
    assert_eq!(parse_journal(&data, &test_root()).unwrap(), open);
    assert!(parse_journal(&data[..data.len() - 1], &test_root()).is_err());
    for field in ["\"version\":1,", "\"future\":1,"] {
        let altered =
            String::from_utf8(data.clone())
                .unwrap()
                .replacen('{', &format!("{{{field}"), 1);
        assert!(parse_journal(altered.as_bytes(), &test_root()).is_err());
    }
    let mut deleting = open.clone();
    deleting.phase = "Deleting".into();
    deleting.delete_intent_nonce = "e".repeat(32);
    let mut history = data;
    history.extend(line(&deleting));
    assert_eq!(parse_journal(&history, &test_root()).unwrap(), deleting);
    history.extend(line(&open));
    assert!(parse_journal(&history, &test_root()).is_err());
}
#[test]
fn intent_generation_nonce_and_identity_are_not_rebound() {
    let open = marker();
    let mut deleting = open.clone();
    deleting.phase = "Deleting".into();
    deleting.delete_intent_nonce = "e".repeat(32);
    assert!(valid_transition(&open, &deleting));
    let mut deleted = deleting.clone();
    deleted.phase = "Deleted".into();
    assert!(valid_transition(&deleting, &deleted));
    deleted.delete_intent_nonce = "f".repeat(32);
    assert!(!valid_transition(&deleting, &deleted));
    assert!(!valid_transition(&deleted, &open));
    let mut fresh = open;
    fresh.incarnation = "1".repeat(32);
    fresh.directory_epoch = "2".repeat(32);
    assert!(valid_transition(&deleted, &fresh));
    fresh.root_volume += 1;
    assert!(parse_journal(&line(&fresh), &test_root()).is_err());
}
#[test]
fn native_path_policy_rejects_devices_ads_unc_and_traversal() {
    for value in [
        ".", "..", "x.", "x ", "x:y", "a/b", "a\\b", "CON", "com1.log", "LPT²",
    ] {
        assert!(!valid_component(value), "{value}");
    }
    for value in [
        r"\\server\share\node",
        r"\\?\C:\node",
        r"C:node",
        r"C:\a\..\b",
        r"C:/a/b",
    ] {
        assert!(path_parts(Path::new(value)).is_err());
    }
    assert!(path_parts(Path::new(r"C:\users\owner\root")).is_ok());
}
#[test]
fn retained_directory_blocks_rename() {
    let temp = crate::test_support::TestDir::new("polaris-win-pin-");
    let path = temp.join("original");
    std::fs::create_dir(&path).unwrap();
    let held = open_object(&path, true, false, false).unwrap();
    assert!(std::fs::rename(&path, temp.join("moved")).is_err());
    drop(held);
    std::fs::rename(&path, temp.join("moved")).unwrap();
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KernelTestArtifact {
    path: PathBuf,
    sha256: String,
    kernel_source: String,
    validation_mode: String,
    repository: String,
}
struct KernelWriter {
    child: std::process::Child,
    config: PathBuf,
    nonce: String,
}
impl Drop for KernelWriter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn await_receipt(child: &mut std::process::Child, path: &Path, nonce: &str) {
    await_receipt_after_probe(child, path, nonce, || {});
}
// Test-only scheduling barrier, never a product admission override.
fn await_receipt_after_probe(
    child: &mut std::process::Child,
    path: &Path,
    nonce: &str,
    mut after_probe: impl FnMut(),
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        match std::fs::read_to_string(path) {
            Ok(value) if value == nonce => return,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => panic!("receipt read: {e}"),
        }
        after_probe();
        if let Some(status) = child.try_wait().unwrap() {
            // Publication can race between the first probe and process exit.
            let value = std::fs::read_to_string(path).expect("exited without readable receipt");
            assert_eq!(value, nonce, "exited with stale/partial receipt");
            assert!(status.success(), "nonzero child exit despite receipt");
            return; // close still validates the cached exit status
        }
        assert!(
            std::time::Instant::now() < deadline,
            "child receipt timeout"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
impl KernelWriter {
    fn start(config: &Path, poison: bool) -> Self {
        let manifest = std::env::var_os("POLARIS_WINDOWS_KERNEL_TEST_ARTIFACT")
            .expect("fixed kernel artifact manifest required");
        let artifact: KernelTestArtifact =
            serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
        assert!(artifact.path.is_absolute());
        assert!(lower_hex(&artifact.sha256, 64));
        assert_eq!(artifact.repository, "polaris-arch/sing-box-polaris");
        let mode = std::env::var("POLARIS_WINDOWS_TEST_VALIDATION_MODE").unwrap();
        let expected = std::env::var("POLARIS_WINDOWS_TEST_EXPECTED_SOURCE").unwrap();
        assert!(lower_hex(&expected, 40));
        assert_eq!(artifact.validation_mode, mode);
        assert_eq!(artifact.kernel_source, expected);
        match mode.as_str() {
            "formal" => assert_eq!(
                configuration_envelope(config).unwrap()["expected_source"].as_str(),
                Some(expected.as_str())
            ),
            "candidate" => eprintln!("candidate source={expected}; not formal release acceptance"),
            _ => panic!("explicit validation mode required"),
        }
        assert_eq!(
            polaris_updater::sha256_hex(&std::fs::read(&artifact.path).unwrap()),
            artifact.sha256
        );
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        for name in ["interop-ready", "interop-close", "interop-closed"] {
            match std::fs::remove_file(config.join(name)) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => panic!("receipt cleanup: {e}"),
            }
        }
        let child = std::process::Command::new(&artifact.path)
            .args([
                "-test.run=^TestWindowsDirectoryInteropChild$",
                "-test.v",
                "-test.timeout=75s",
            ])
            .env("POLARIS_DIRECTORY_INTEROP_ROOT", config)
            .env("POLARIS_DIRECTORY_INTEROP_NONCE", &nonce)
            .env(
                "POLARIS_DIRECTORY_INTEROP_POISON",
                if poison { "1" } else { "0" },
            )
            .spawn()
            .unwrap();
        let mut writer = Self {
            child,
            config: config.to_owned(),
            nonce,
        };
        await_receipt(
            &mut writer.child,
            &config.join("interop-ready"),
            &writer.nonce,
        );
        writer
    }
    fn close(&mut self) {
        std::fs::write(self.config.join("interop-close"), &self.nonce).unwrap();
        await_receipt(
            &mut self.child,
            &self.config.join("interop-closed"),
            &self.nonce,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            assert!(std::time::Instant::now() < deadline, "child exit timeout");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    fn crash(&mut self) {
        self.child.kill().unwrap();
        assert!(!self.child.wait().unwrap().success());
    }
}
fn saved_intent(config: &Path) -> DirectoryDeletionIntent {
    DirectoryLease::acquire(config, "node", false)
        .unwrap()
        .snapshot_intent()
        .unwrap()
}
#[test]
#[ignore = "requires native NTFS Windows and a pinned real Go test producer"]
fn interop_writer_retirement_and_stale_intent() {
    let temp = crate::test_support::TestDir::new("polaris-win-interop-retire-");
    let config = temp.path();
    let mut writer = KernelWriter::start(config, false);
    let intent = saved_intent(config);
    assert!(DirectoryLease::acquire_intent(config, &intent).is_err());
    writer.close();
    {
        let mut lease = DirectoryLease::acquire_intent(config, &intent).unwrap();
        lease
            .commit(&intent, &serde_json::json!({"servers":[]}))
            .unwrap();
    }
    assert!(!Path::new(&intent.state_directory).exists());
    assert!(
        auth_lease(config, "node").is_err(),
        "deleted generation cannot authorize auth-state mutation"
    );
    let mut next = KernelWriter::start(config, false);
    let new_intent = saved_intent(config);
    assert_ne!(new_intent.incarnation, intent.incarnation);
    next.close();
    assert!(DirectoryLease::acquire_intent(config, &intent).is_err());
    assert!(Path::new(&new_intent.state_directory).exists());
}
#[test]
#[ignore = "requires native NTFS Windows and a pinned real Go test producer"]
fn interop_crash_reopen_and_revocation() {
    let temp = crate::test_support::TestDir::new("polaris-win-interop-crash-");
    let config = temp.path();
    let mut writer = KernelWriter::start(config, false);
    let before = saved_intent(config);
    writer.crash();
    let mut reopened = KernelWriter::start(config, true);
    let after = saved_intent(config);
    assert_eq!(before.directory_identity, after.directory_identity);
    assert_eq!(before.incarnation, after.incarnation);
    assert!(DirectoryLease::acquire_intent(config, &before).is_err());
    reopened.close();
    assert!(DirectoryLease::acquire_intent(config, &before).is_err());
    assert!(Path::new(&before.state_directory).exists());
}
#[test]
#[ignore = "requires native NTFS Windows and a pinned real Go test producer"]
fn interop_replaced_identity_is_retained() {
    let temp = crate::test_support::TestDir::new("polaris-win-interop-replace-");
    let config = temp.path();
    let mut writer = KernelWriter::start(config, false);
    let intent = saved_intent(config);
    writer.close();
    let target = Path::new(&intent.state_directory);
    let original = target.with_file_name("original-generation");
    std::fs::rename(target, &original).unwrap();
    std::fs::create_dir(target).unwrap();
    std::fs::write(target.join("preserve"), b"replacement").unwrap();
    assert!(DirectoryLease::acquire_intent(config, &intent).is_err());
    assert_eq!(
        std::fs::read(target.join("preserve")).unwrap(),
        b"replacement"
    );
    assert!(original.join(".polaris-directory-incarnation").exists());
}
#[test]
fn receipt_race_child() {
    let Some(root) = std::env::var_os("POLARIS_RECEIPT_RACE_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let nonce = std::env::var("POLARIS_RECEIPT_RACE_NONCE").unwrap();
    let outcome = std::env::var("POLARIS_RECEIPT_RACE_OUTCOME").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::read_to_string(root.join("release"))
        .ok()
        .as_deref()
        != Some(nonce.as_str())
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    if outcome != "missing" {
        std::fs::write(
            root.join("receipt.tmp"),
            if outcome == "wrong" {
                "wrong nonce"
            } else {
                &nonce
            },
        )
        .unwrap();
        std::fs::rename(root.join("receipt.tmp"), root.join("receipt")).unwrap();
    }
    if outcome == "failed" {
        std::process::exit(17);
    }
}
#[test]
fn terminal_receipt_reread_accepts_only_complete_nonce_and_success() {
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    for outcome in ["success", "missing", "wrong", "failed"] {
        let temp = crate::test_support::TestDir::new("polaris-win-receipt-race-");
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let receipt = temp.join("receipt");
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::windows_directory::tests::receipt_race_child",
                "--nocapture",
            ])
            .env("POLARIS_RECEIPT_RACE_ROOT", temp.path())
            .env("POLARIS_RECEIPT_RACE_NONCE", &nonce)
            .env("POLARIS_RECEIPT_RACE_OUTCOME", outcome)
            .spawn()
            .unwrap();
        let mut guard = KernelWriter {
            child,
            config: temp.path().to_path_buf(),
            nonce: nonce.clone(),
        };
        let process = guard.child.as_raw_handle();
        let mut barrier_hit = false;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            await_receipt_after_probe(&mut guard.child, &receipt, &nonce, || {
                assert!(!barrier_hit);
                barrier_hit = true;
                assert!(!receipt.exists());
                std::fs::write(temp.join("release"), &nonce).unwrap();
                // Actual original process termination occurs between first read
                // and try_wait; no sleeps are used to guess this ordering.
                assert_eq!(unsafe { WaitForSingleObject(process, 10_000) }, 0);
            });
        }));
        assert!(barrier_hit);
        assert_eq!(result.is_ok(), outcome == "success");
        if outcome == "success" {
            assert!(guard.child.wait().unwrap().success());
        }
    }
}
#[test]
#[ignore = "requires the explicit one-shot SYSTEM task runner and pinned artifacts"]
fn interop_system_producer_user_consumer() {
    let config = PathBuf::from(std::env::var_os("POLARIS_SYSTEM_INTEROP_ROOT").unwrap());
    let receipt_nonce = std::env::var("POLARIS_SYSTEM_INTEROP_NONCE").unwrap();
    assert!(nonce(&receipt_nonce));
    assert!(config.is_absolute());
    let source = std::env::var("POLARIS_SYSTEM_INTEROP_SOURCE").unwrap();
    assert!(lower_hex(&source, 40));
    match std::env::var("POLARIS_SYSTEM_INTEROP_MODE")
        .unwrap()
        .as_str()
    {
        "formal" => assert_eq!(
            configuration_envelope(&config).unwrap()["expected_source"].as_str(),
            Some(source.as_str())
        ),
        "candidate" => eprintln!("candidate SYSTEM bootstrap, not formal release acceptance"),
        _ => panic!("explicit mode required"),
    }
    let wait = |name: &str| {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            match std::fs::read_to_string(config.join(name)) {
                Ok(value) if value == receipt_nonce => return,
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => panic!("SYSTEM receipt: {e}"),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "SYSTEM receipt timeout: {name}"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    wait("interop-ready");
    let server = std::env::var("POLARIS_SYSTEM_INTEROP_SERVER").unwrap();
    assert!(matches!(server.as_str(), "node" | "system-node"));
    let intent = DirectoryLease::acquire(&config, &server, false)
        .unwrap()
        .snapshot_intent()
        .unwrap();
    assert!(DirectoryLease::acquire_intent(&config, &intent).is_err());
    std::fs::write(config.join("interop-close"), &receipt_nonce).unwrap();
    wait("interop-closed");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut lease = loop {
        match DirectoryLease::acquire_intent(&config, &intent) {
            Ok(lease) => break lease,
            Err(e) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "SYSTEM custody was not released: {e}"
                );
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    };
    lease
        .commit(&intent, &serde_json::json!({"servers":[]}))
        .unwrap();
    assert!(!Path::new(&intent.state_directory).exists());
    std::fs::write(config.join("interop-consumed"), &receipt_nonce).unwrap();
}

#[test]
fn legacy_selection_never_issues_physical_retirement_custody() {
    let temp = crate::test_support::TestDir::new("polaris-win-legacy-");
    let logical = temp.join("tailscale").join("node");
    std::fs::create_dir_all(&logical).unwrap();
    assert_eq!(actual_directory(temp.path(), "node").unwrap(), logical);
    assert!(auth_lease(temp.path(), "node").unwrap().is_none());
    assert!(DirectoryLease::acquire(temp.path(), "node", true).is_err());
    assert!(logical.is_dir());
}
