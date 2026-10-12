use super::*;
use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest};

fn fixture(version: &str) -> (tempfile::TempDir, InstalledCoreManifest) {
    let source = tempfile::tempdir().unwrap();
    private_mode(source.path(), 0o700).unwrap();
    let files: Vec<_> = [
        ("sing-box", format!("core-{version}").into_bytes()),
        ("libcronet.so", b"sidecar".to_vec()),
    ]
    .into_iter()
    .map(|(name, raw)| {
        fs::write(source.path().join(name), &raw).unwrap();
        CoreFile {
            name: name.into(),
            bytes: raw.len() as u64,
            sha256: crate::core_install::sha256_hex(&raw),
        }
    })
    .collect();
    let core = CoreManifest {
        platform: "linux".into(),
        version: version.into(),
        provider_fingerprint: "a".repeat(64),
        files,
        helper_sha256: Some("b".repeat(64)),
    };
    let id = core
        .id(crate::core_install::sha256_hex)
        .unwrap()
        .as_str()
        .to_owned();
    (
        source,
        InstalledCoreManifest {
            schema: "polaris-core-manifest-v1".into(),
            id,
            app_version: "1.0.0".into(),
            core,
        },
    )
}

fn store() -> (tempfile::TempDir, CoreStore, u32) {
    let root = tempfile::tempdir().unwrap();
    private_mode(root.path(), 0o700).unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    let store = CoreStore::initialize(root.path(), OwnerPolicy { uid: Some(uid) }).unwrap();
    (root, store, uid)
}

#[test]
fn runtime_open_is_read_only_and_does_not_invent_an_installation() {
    let root = tempfile::tempdir().unwrap();
    let uid = nix::unistd::Uid::effective().as_raw();
    assert!(CoreStore::open(root.path(), OwnerPolicy { uid: Some(uid) }).is_err());
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 0);
}

#[test]
fn mismatched_source_never_becomes_an_eligible_version() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    fs::write(source.path().join("libcronet.so"), b"changed").unwrap();
    assert!(store.install(&manifest, source.path(), Some(uid)).is_err());
    assert!(!root
        .path()
        .join("manifests")
        .join(format!("{}.json", manifest.id))
        .exists());
    assert!(!root.path().join("versions").join(&manifest.id).exists());
}

#[test]
fn same_id_is_immutable_and_every_start_rechecks_contents() {
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    assert_eq!(
        store.install(&manifest, source.path(), Some(uid)).unwrap(),
        InstallOutcome::Installed
    );
    assert_eq!(
        store.install(&manifest, source.path(), Some(uid)).unwrap(),
        InstallOutcome::AlreadyPresent
    );
    let id = CoreId::parse(&manifest.id).unwrap();
    let verified = store.verify_for_start(&id).unwrap();
    assert_eq!(verified.manifest.id, manifest.id);
    drop(verified);
    fs::write(
        root.path()
            .join("versions")
            .join(&manifest.id)
            .join("libcronet.so"),
        b"changed",
    )
    .unwrap();
    assert!(store.verify_for_start(&id).is_err());
    // Reinstall must not replace a manifest-backed id to hide a damaged payload.
    assert!(store.install(&manifest, source.path(), Some(uid)).is_err());
}

#[test]
fn busy_leases_allow_new_install_but_prevent_retirement() {
    let (root, store, uid) = store();
    let mut leases = Vec::new();
    for version in ["1.0.0", "1.0.1", "1.0.2", "1.0.3", "1.0.4"] {
        let (source, manifest) = fixture(version);
        store.install(&manifest, source.path(), Some(uid)).unwrap();
        leases.push(
            store
                .verify_for_start(&CoreId::parse(&manifest.id).unwrap())
                .unwrap(),
        );
    }
    assert!(store.retain_latest_three(&[]).unwrap().is_empty());
    assert_eq!(
        fs::read_dir(root.path().join("manifests")).unwrap().count(),
        5
    );
    drop(leases);
    assert_eq!(store.retain_latest_three(&[]).unwrap().len(), 2);
    assert_eq!(
        fs::read_dir(root.path().join("versions")).unwrap().count(),
        3
    );
}

#[test]
fn linked_payload_and_weakened_store_permissions_are_rejected() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let (root, store, uid) = store();
    let (source, manifest) = fixture("1.0.0");
    fs::remove_file(source.path().join("sing-box")).unwrap();
    symlink(
        source.path().join("libcronet.so"),
        source.path().join("sing-box"),
    )
    .unwrap();
    assert!(store.install(&manifest, source.path(), Some(uid)).is_err());
    fs::set_permissions(root.path(), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(CoreStore::open(root.path(), OwnerPolicy { uid: Some(uid) }).is_err());
}
