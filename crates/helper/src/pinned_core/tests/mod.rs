use super::*;
use polaris_helper_proto::core_manifest::{CoreFile, CoreManifest, InstalledCoreManifest};
use std::os::unix::fs::PermissionsExt;
#[test]
fn status_distinguishes_uninstalled_id_from_missing_installed_bytes() {
    let root = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    for dir in [root.path(), source.path()] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let uid = nix::unistd::Uid::effective().as_raw();
    let owner = OwnerPolicy { uid: Some(uid) };
    let store = CoreStore::initialize(root.path(), owner).unwrap();
    let files = [
        ("sing-box", b"inert-core".as_slice()),
        ("libcronet.so", b"inert-sidecar".as_slice()),
    ]
    .into_iter()
    .map(|(name, raw)| {
        std::fs::write(source.path().join(name), raw).unwrap();
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
    assert_eq!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Absent { id: id.clone() }
    );
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    assert!(matches!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Present { .. }
    ));
    std::fs::remove_file(
        root.path()
            .join("versions")
            .join(id.as_str())
            .join("libcronet.so"),
    )
    .unwrap();
    assert!(matches!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Invalid { .. }
    ));
    std::fs::remove_file(
        root.path()
            .join("manifests")
            .join(format!("{}.json", id.as_str())),
    )
    .unwrap();
    assert!(matches!(
        status_with_owner(Some(root.path()), &id, owner),
        CoreStatus::Invalid { .. }
    ));
}
