use super::*;
fn acquire_physical(config: &Path, id: &str, exclusive: bool) -> io::Result<DirectoryLease> {
    DirectoryLease::acquire_root(config, id, exclusive, "tailscale-g4-v1")
}
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("polaris-dir-lease-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(path.join("tailscale")).unwrap();
        std::fs::create_dir_all(path.join("tailscale-g4-v1")).unwrap();
        Self(path)
    }
    fn create(&self) {
        let lease = acquire_physical(&self.0, "node", true).unwrap();
        lease.create_new().unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn mapped_selection_preserves_legacy_and_never_creates_a_node_during_planning() {
    let fixture = Fixture::new();
    assert_eq!(
        DirectoryLease::actual_directory(&fixture.0, "new").unwrap(),
        fixture.0.join("tailscale-g4-v1/new")
    );
    assert!(!fixture.0.join("tailscale/new").exists());
    assert!(!fixture.0.join("tailscale-g4-v1/new").exists());
    std::fs::create_dir(fixture.0.join("tailscale/old")).unwrap();
    assert_eq!(
        DirectoryLease::actual_directory(&fixture.0, "old").unwrap(),
        fixture.0.join("tailscale/old")
    );
    let legacy = DirectoryLease::acquire_mapped(&fixture.0, "old", false).unwrap();
    assert_eq!(legacy.directory_path(), fixture.0.join("tailscale/old"));
    assert!(legacy.snapshot_intent().is_err());
    assert!(!fixture.0.join("tailscale-g4-v1/old").exists());
}
#[test]
fn physical_intent_is_not_logical_alias_and_poison_revokes_every_retry() {
    let fixture = Fixture::new();
    let lease = DirectoryLease::acquire_root(&fixture.0, "node", true, "tailscale-g4-v1").unwrap();
    lease.create_new().unwrap();
    let intent = lease.snapshot_intent().unwrap();
    let mut marker = lease.read_marker().unwrap();
    let epochs = std::collections::BTreeSet::from([marker.directory_epoch.clone()]);
    marker.phase = "Retiring".into();
    marker.delete_intent_nonce = intent.delete_intent_nonce.clone();
    lease.write_marker(&marker).unwrap();
    drop(lease);
    let mut forged = intent.clone();
    forged.state_directory = fixture
        .0
        .join("tailscale/node")
        .to_string_lossy()
        .into_owned();
    assert!(DirectoryLease::acquire_intent(&fixture.0, &forged).is_err());
    DirectoryLease::poison_domain(&fixture.0).unwrap();
    let lease = DirectoryLease::acquire_intent(&fixture.0, &intent).unwrap();
    assert!(lease.snapshot_intent().is_err());
    assert!(lease.delete_bound(&intent, &epochs).is_err());
    assert!(lease.resume_committed(&intent).is_err());
    assert!(fixture.0.join("tailscale-g4-v1/node").is_dir());
}
#[test]
fn unsafe_open_domain_preserves_connections_without_minting_snapshot() {
    let fixture = Fixture::new();
    let lease = DirectoryLease::acquire_root(&fixture.0, "node", true, "tailscale-g4-v1").unwrap();
    lease.create_new().unwrap();
    drop(lease);
    DirectoryLease::poison_domain(&fixture.0).unwrap();
    let connected = DirectoryLease::acquire_mapped(&fixture.0, "node", false).unwrap();
    assert_eq!(
        connected.directory_path(),
        fixture.0.join("tailscale-g4-v1/node")
    );
    assert!(connected.snapshot_intent().is_err());
}
#[test]
fn child_shared_owner() {
    let Some(path) = std::env::var_os("POLARIS_DIRECTORY_LEASE_CHILD") else {
        return;
    };
    let _lease = acquire_physical(Path::new(&path), "node", false).unwrap();
    println!("DIRECTORY_LEASE_READY");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}
#[test]
fn actual_child_process_lease_blocks_parent_until_release() {
    use std::io::BufRead;
    let fixture = Fixture::new();
    fixture.create();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::tailscale_directory::unix::tests::child_shared_owner",
            "--nocapture",
        ])
        .env("POLARIS_DIRECTORY_LEASE_CHILD", &fixture.0)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(output.read_line(&mut line).unwrap() > 0);
        if line.contains("DIRECTORY_LEASE_READY") {
            break;
        }
    }
    assert!(acquire_physical(&fixture.0, "node", true).is_err());
    child.stdin.take().unwrap().write_all(b"release\n").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(acquire_physical(&fixture.0, "node", true).is_ok());
}
#[test]
fn shared_owner_blocks_exclusive_retirement() {
    let fixture = Fixture::new();
    fixture.create();
    let shared = acquire_physical(&fixture.0, "node", false).unwrap();
    assert!(acquire_physical(&fixture.0, "node", true).is_err());
    drop(shared);
    assert!(acquire_physical(&fixture.0, "node", true).is_ok());
}
#[test]
fn new_scope_epoch_invalidates_old_receipt_even_after_its_lease_exits() {
    let fixture = Fixture::new();
    fixture.create();
    let lease = acquire_physical(&fixture.0, "node", true).unwrap();
    let intent = lease.snapshot_intent().unwrap();
    let mut marker = lease.read_marker().unwrap();
    let epochs = std::collections::BTreeSet::from([marker.directory_epoch.clone()]);
    marker.directory_epoch = uuid::Uuid::new_v4().simple().to_string();
    lease.write_marker(&marker).unwrap();
    assert!(lease.delete_bound(&intent, &epochs).is_err());
    assert!(fixture.0.join("tailscale-g4-v1/node").is_dir());
}
#[test]
fn marked_deletion_is_idempotent_but_recreated_object_is_not_deleted() {
    let fixture = Fixture::new();
    fixture.create();
    let lease = acquire_physical(&fixture.0, "node", true).unwrap();
    let intent = lease.snapshot_intent().unwrap();
    let epochs = std::collections::BTreeSet::from([lease.read_marker().unwrap().directory_epoch]);
    std::fs::write(
        fixture.0.join("tailscale-g4-v1/node/taildrop"),
        b"owned node data",
    )
    .unwrap();
    lease.delete_bound(&intent, &epochs).unwrap();
    lease.delete_bound(&intent, &epochs).unwrap();
    std::fs::create_dir(fixture.0.join("tailscale-g4-v1/node")).unwrap();
    assert!(lease.delete_bound(&intent, &epochs).is_err());
    assert!(fixture.0.join("tailscale-g4-v1/node").is_dir());
}
#[test]
fn missing_or_replaced_stamp_never_authorizes_an_open_directory() {
    let fixture = Fixture::new();
    fixture.create();
    let lease = acquire_physical(&fixture.0, "node", true).unwrap();
    let intent = lease.snapshot_intent().unwrap();
    let epochs = std::collections::BTreeSet::from([lease.read_marker().unwrap().directory_epoch]);
    std::fs::remove_file(fixture.0.join("tailscale-g4-v1/node").join(STAMP)).unwrap();
    assert!(lease.snapshot_intent().is_err());
    assert!(lease.delete_bound(&intent, &epochs).is_err());
    assert!(fixture.0.join("tailscale-g4-v1/node").is_dir());
}
#[test]
fn retiring_missing_stamp_never_moves_original_even_with_reused_inode() {
    let fixture = Fixture::new();
    fixture.create();
    let lease = acquire_physical(&fixture.0, "node", true).unwrap();
    let intent = lease.snapshot_intent().unwrap();
    let mut marker = lease.read_marker().unwrap();
    marker.phase = "Retiring".into();
    marker.delete_intent_nonce = intent.delete_intent_nonce.clone();
    lease.write_marker(&marker).unwrap();
    std::fs::remove_file(fixture.0.join("tailscale-g4-v1/node").join(STAMP)).unwrap();
    let quarantine = format!("{}.retired-{}", lease.stem, marker.incarnation);
    // Numeric identity intentionally stays identical: the stamp must decide.
    assert!(lease.resume_committed(&intent).is_err());
    assert!(fixture.0.join("tailscale-g4-v1/node").is_dir());
    assert!(absent(&lease.control, &quarantine).unwrap());
    drop(lease);
    // A reacquire selects legacy lock for the unstamped physical object;
    // that lock must never be treated as the original managed lifetime EX.
    let legacy = acquire_physical(&fixture.0, "node", true).unwrap();
    assert!(legacy.lifetime_name.starts_with("legacy-"));
    assert!(legacy.resume_committed(&intent).is_err());
    assert!(fixture.0.join("tailscale-g4-v1/node").is_dir());
}
#[test]
fn two_directory_batch_releases_admission_but_keeps_both_lifetimes_until_ack() {
    let fixture = Fixture::new();
    fixture.create();
    let second = acquire_physical(&fixture.0, "second", true).unwrap();
    second.create_new().unwrap();
    drop(second);
    let mut retained = Vec::new();
    for id in ["node", "second"] {
        let mut lease = acquire_physical(&fixture.0, id, true).unwrap();
        let intent = lease.snapshot_intent().unwrap();
        let epochs =
            std::collections::BTreeSet::from([lease.read_marker().unwrap().directory_epoch]);
        lease.delete_bound(&intent, &epochs).unwrap();
        lease.release_admission_after_commit();
        assert!(acquire_physical(&fixture.0, id, true).is_err());
        retained.push(lease);
    }
    assert_eq!(retained.len(), 2);
    // Failed journal acknowledgement keeps both deleted markers as recovery
    // authority; after dropping lifetime locks each exact intent can retry.
    drop(retained);
    assert!(acquire_physical(&fixture.0, "node", true).is_ok());
    assert!(acquire_physical(&fixture.0, "second", true).is_ok());
}
#[test]
fn final_unlink_crash_resumes_but_never_removes_new_content() {
    let fixture = Fixture::new();
    fixture.create();
    let lease = acquire_physical(&fixture.0, "node", true).unwrap();
    let intent = lease.snapshot_intent().unwrap();
    let mut marker = lease.read_marker().unwrap();
    marker.phase = "Removing".into();
    marker.delete_intent_nonce = intent.delete_intent_nonce.clone();
    let quarantine = format!("{}.retired-{}", lease.stem, marker.incarnation);
    fs::renameat(&lease.root, "node", &lease.control, &quarantine).unwrap();
    lease.write_marker(&marker).unwrap();
    let target = directory(&lease.control, &quarantine).unwrap();
    fs::unlinkat(&target, STAMP, AtFlags::empty()).unwrap();
    let newcomer = fs::openat(
        &target,
        "new-content",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL,
        Mode::RUSR | Mode::WUSR,
    )
    .unwrap();
    drop(newcomer);
    assert!(lease.resume_committed(&intent).is_err());
    assert!(fs::statat(&target, "new-content", AtFlags::SYMLINK_NOFOLLOW).is_ok());
    fs::unlinkat(&target, "new-content", AtFlags::empty()).unwrap();
    lease.resume_committed(&intent).unwrap();
}
#[test]
fn legacy_unmarked_directory_is_never_adopted() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.0.join("tailscale-g4-v1/node")).unwrap();
    let lease = acquire_physical(&fixture.0, "node", true).unwrap();
    assert!(lease.create_new().is_err());
    assert!(lease.snapshot_intent().is_err());
}
#[test]
fn target_symlink_is_rejected_but_valid_special_ids_still_connect() {
    let fixture = Fixture::new();
    for id in ["Node", "NÖDE", "no\u{0308}de", ".polaris-directory-control"] {
        let lease = acquire_physical(&fixture.0, id, true).unwrap();
        lease.create_new().unwrap();
        drop(lease);
        assert!(acquire_physical(&fixture.0, id, false).is_ok());
    }
    std::os::unix::fs::symlink(&fixture.0, fixture.0.join("tailscale-g4-v1/node-link")).unwrap();
    assert!(acquire_physical(&fixture.0, "node-link", false).is_err());
}
#[test]
fn filesystem_case_and_unicode_aliases_share_the_original_lifetime_lock() {
    let fixture = Fixture::new();
    for (original, alias) in [("Node", "node"), ("NÖDE", "NO\u{0308}DE")] {
        let lease = acquire_physical(&fixture.0, original, true).unwrap();
        lease.create_new().unwrap();
        drop(lease);
        let root = open_absolute(&fixture.0.join("tailscale-g4-v1")).unwrap();
        let original_stat = fs::statat(&root, original, AtFlags::SYMLINK_NOFOLLOW).unwrap();
        if let Ok(alias_stat) = fs::statat(&root, alias, AtFlags::SYMLINK_NOFOLLOW) {
            if same(
                &alias_stat,
                original_stat.st_dev as u64,
                original_stat.st_ino as u64,
            ) {
                let owner = acquire_physical(&fixture.0, original, false).unwrap();
                assert!(acquire_physical(&fixture.0, alias, true).is_err());
                drop(owner);
                let lease = acquire_physical(&fixture.0, original, true).unwrap();
                let intent = lease.snapshot_intent().unwrap();
                assert!(lease
                    .ensure_no_configured_alias(
                        &intent,
                        &serde_json::json!({"servers":[{"id":alias}]})
                    )
                    .is_err());
            }
        }
    }
}
#[test]
fn replaced_target_is_legacy_connectable_but_not_bound_to_stale_marker() {
    let fixture = Fixture::new();
    fixture.create();
    std::fs::rename(
        fixture.0.join("tailscale-g4-v1/node"),
        fixture.0.join("old-directory"),
    )
    .unwrap();
    std::fs::create_dir(fixture.0.join("tailscale-g4-v1/node")).unwrap();
    let lease = acquire_physical(&fixture.0, "node", false).unwrap();
    assert!(lease.snapshot_intent().is_err());
}
