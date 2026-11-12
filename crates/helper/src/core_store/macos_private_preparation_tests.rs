//! Storage-only self-mutation tests; no root execution or policy fixture.
use super::*;

#[test]
fn private_generation_root_change_refreshes_data_without_minting_admission() {
    let Some(_completion) = tests::exact_fixture_scope(
        module_path!(),
        "private_generation_root_change_refreshes_data_without_minting_admission",
    ) else {
        return;
    };
    let (_root, store, uid) = tests::store();
    let (source, manifest) = tests::fixture("private-preparation");
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    let id = CoreId::parse(&manifest.id).unwrap();
    let mutation = store.lock_store_mutation().unwrap();
    let data = store.verify_objects(&id, false).unwrap();
    assert!(data.data_is_current());
    fs::create_dir(store.root.join(".execution-owned-fixture")).unwrap();
    assert!(!data.data_is_current());
    let refreshed = store
        .snapshot_after_private_preparation(&id, data, &mutation)
        .unwrap();
    assert!(refreshed.data_is_current());
    assert!(refreshed.admitted.is_none());
    assert!(refreshed
        .prepare(polaris_core_supervisor::admitted_payload::ExecPurpose::RunMain)
        .is_err());
    let probe = store.lease_file(&id).unwrap();
    assert!(matches!(probe.try_lock(), Err(TryLockError::WouldBlock)));
    drop(refreshed);
    probe.try_lock().unwrap();
}

#[test]
fn private_root_refresh_cannot_hide_selected_member_changes() {
    let Some(_completion) = tests::exact_fixture_scope(
        module_path!(),
        "private_root_refresh_cannot_hide_selected_member_changes",
    ) else {
        return;
    };
    let (_root, store, uid) = tests::store();
    let (source, manifest) = tests::fixture("private-preparation-negative");
    store
        .install(&manifest, source.path(), Some(uid), None)
        .unwrap();
    let id = CoreId::parse(&manifest.id).unwrap();
    let mutation = store.lock_store_mutation().unwrap();
    let data = store.verify_objects(&id, false).unwrap();
    fs::create_dir(store.root.join(".execution-owned-fixture")).unwrap();
    fs::write(data.directory.join("sing-box"), b"changed").unwrap();
    assert!(store
        .snapshot_after_private_preparation(&id, data, &mutation)
        .is_err());
}
