use super::*;
use crate::test_support::TestDir;
use serde_json::json;

fn fixture() -> (TestDir, ConfigManager, Value) {
    let directory = TestDir::new("identity-repair-");
    let manager = ConfigManager::new(directory.clone());
    let mut raw = manager.current().unwrap();
    raw["servers"] = json!([
        {"id":"same","name":"A","protocol":"trojan","address":"a.invalid","port":443,"password":"fixture-a","unknown":{"preserve":true}},
        {"id":"same","name":"B","protocol":"trojan","address":"b.invalid","port":443,"password":"fixture-b"},
        {"id":"selected","name":"Selected","protocol":"trojan","address":"c.invalid","port":443,"password":"fixture-c"}
    ]);
    raw["selectedServerId"] = json!("same");
    raw["logLevel"] = json!("debug");
    let canonical = ConfigStore::canonicalize_for_save(&raw).unwrap();
    // Represents a legacy disk, not an ordinary writer allowed to create duplicates.
    std::fs::write(
        directory.join("config.json"),
        serde_json::to_vec(&canonical).unwrap(),
    )
    .unwrap();
    (directory, manager, canonical)
}

fn request(manager: &ConfigManager, keeper: usize) -> IdentityRepairRequest {
    let preview = manager.identity_conflicts().unwrap();
    IdentityRepairRequest {
        version: preview.version,
        choices: vec![IdentityRepairChoice {
            indices: vec![0, 1],
            keeper_index: keeper,
        }],
    }
}

#[test]
fn repair_preserves_every_field_and_reference_except_nonkeeper_id() {
    for keeper in [0, 1] {
        let (_directory, manager, raw) = fixture();
        let mut expected = raw.clone();
        expected["servers"][1 - keeper]["id"] = json!("new-identity");
        let saved = manager
            .repair_server_identities(&request(&manager, keeper), || "new-identity".into())
            .unwrap();
        assert_eq!(saved, expected);
        assert_eq!(manager.current().unwrap(), expected);
        assert_eq!(manager.identity_conflicts().unwrap().groups.len(), 0);
    }
}

#[test]
fn preview_excludes_ids_and_credentials_and_does_not_mutate_disk() {
    let (directory, manager, _) = fixture();
    let before = std::fs::read(directory.join("config.json")).unwrap();
    let preview = serde_json::to_value(manager.identity_conflicts().unwrap()).unwrap();
    let text = preview.to_string();
    assert!(!text.contains("fixture-a"));
    assert!(!text.contains("fixture-b"));
    assert!(!text.contains("\"id\""));
    assert_eq!(preview["groups"][0]["indices"], json!([0, 1]));
    assert_eq!(
        std::fs::read(directory.join("config.json")).unwrap(),
        before
    );
}

#[test]
fn stale_full_document_or_invalid_choice_never_writes() {
    let (directory, manager, mut raw) = fixture();
    let stale = request(&manager, 0);
    raw["logLevel"] = json!("warn");
    std::fs::write(
        directory.join("config.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let before = std::fs::read(directory.join("config.json")).unwrap();
    assert!(manager
        .repair_server_identities(&stale, || "new".into())
        .unwrap_err()
        .to_string()
        .contains("serverIdentityRevisionChanged"));
    for choices in [
        vec![],
        vec![IdentityRepairChoice {
            indices: vec![1, 0],
            keeper_index: 0,
        }],
        vec![IdentityRepairChoice {
            indices: vec![0, 1],
            keeper_index: 2,
        }],
    ] {
        let mut invalid = request(&manager, 0);
        invalid.choices = choices;
        assert!(manager
            .repair_server_identities(&invalid, || "new".into())
            .is_err());
    }
    assert!(manager
        .repair_server_identities(&request(&manager, 0), || "selected".into())
        .is_err());
    assert_eq!(
        std::fs::read(directory.join("config.json")).unwrap(),
        before
    );
}

#[test]
fn any_tailscale_related_duplicate_is_not_repairable() {
    for field in [
        json!({"protocol":"tailscale"}),
        json!({"tailscaleSettings":null}),
        json!({"tailscaleCredentialIntent":null}),
    ] {
        let (directory, manager, mut raw) = fixture();
        for (key, value) in field.as_object().unwrap() {
            raw["servers"][1][key] = value.clone();
        }
        std::fs::write(
            directory.join("config.json"),
            serde_json::to_vec(&raw).unwrap(),
        )
        .unwrap();
        let before = std::fs::read(directory.join("config.json")).unwrap();
        assert!(!manager.identity_conflicts().unwrap().groups[0].repairable);
        assert!(manager
            .repair_server_identities(&request(&manager, 0), || "new".into())
            .unwrap_err()
            .to_string()
            .contains("serverIdentityRepairUnsupported"));
        assert_eq!(
            std::fs::read(directory.join("config.json")).unwrap(),
            before
        );
    }
}

#[test]
fn ordinary_writers_cannot_create_duplicate_identity() {
    let (_directory, manager, mut raw) = fixture();
    raw["servers"][1]["id"] = json!("different");
    // A snapshot can remove ambiguity, but still cannot guess the original credential owner.
    assert!(manager.save_full(&raw).is_err());
    let directory = TestDir::new("identity-repair-new-");
    let clean = ConfigManager::new(directory.to_path_buf());
    clean.current().unwrap();
    raw["servers"][1]["id"] = json!("same");
    assert!(clean
        .save_full(&raw)
        .unwrap_err()
        .to_string()
        .contains(IDENTITY_AMBIGUOUS));
}

#[test]
fn repair_preserves_legacy_raw_defaults_and_unrelated_tailscale_fields() {
    let (directory, manager, mut raw) = fixture();
    raw.as_object_mut().unwrap().remove("logLevel");
    raw.as_object_mut().unwrap().remove("dnsPolicy");
    raw["legacyUnknown"] = json!({"keep":"unchanged"});
    raw["servers"].as_array_mut().unwrap().push(json!({
        "id":"ts-other","name":"TS","protocol":"tailscale",
        "tailscaleSettings":{"authKey":"ts-fixture-secret","unknown":{"preserve":true}}
    }));
    std::fs::write(
        directory.join("config.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let mut expected = raw.clone();
    expected["servers"][1]["id"] = json!("new");
    let saved = manager
        .repair_server_identities(&request(&manager, 0), || "new".into())
        .unwrap();
    assert_eq!(saved, expected);
    let disk: Value =
        serde_json::from_slice(&std::fs::read(directory.join("config.json")).unwrap()).unwrap();
    assert_eq!(disk, expected);
    assert_eq!(disk["servers"][3], raw["servers"][3]);
}

#[test]
fn persistence_failure_and_uncertain_commit_never_retry() {
    let (directory, manager, raw) = fixture();
    let choice = request(&manager, 0);
    manager
        .test_fail_before_config_persist_once
        .store(true, Ordering::SeqCst);
    assert!(manager
        .repair_server_identities(&choice, || "new".into())
        .is_err());
    let disk: Value =
        serde_json::from_slice(&std::fs::read(directory.join("config.json")).unwrap()).unwrap();
    assert_eq!(disk, raw);
    manager
        .test_uncertain_after_mesh_publish_once
        .store(true, Ordering::SeqCst);
    assert!(matches!(
        manager.repair_server_identities(&choice, || "new".into()),
        Err(StoreError::CommitUncertain(_))
    ));
    assert!(manager.cache.read().unwrap().is_none());
    assert!(manager
        .repair_server_identities(&choice, || "another".into())
        .unwrap_err()
        .to_string()
        .contains("serverIdentityRevisionChanged"));
}

#[test]
fn malformed_typed_fields_in_either_repaired_record_never_write() {
    for index in [0, 1] {
        for (field, invalid) in [("protocol", json!("TROJAN")), ("detour", json!(123))] {
            let (directory, manager, mut raw) = fixture();
            raw["servers"][index][field] = invalid;
            // Both counterexamples survive the tolerant store pipeline. The repair
            // must not claim success for nodes that runtime typed parsing rejects.
            let canonical = ConfigStore::canonicalize_for_save(&raw).unwrap();
            assert_eq!(canonical["servers"].as_array().unwrap().len(), 3);
            assert!(serde_json::from_value::<UserConfig>(canonical).is_err());
            std::fs::write(
                directory.join("config.json"),
                serde_json::to_vec(&raw).unwrap(),
            )
            .unwrap();
            let before = std::fs::read(directory.join("config.json")).unwrap();
            let cached_before = manager.cache.read().unwrap().clone();
            let error = manager
                .repair_server_identities(&request(&manager, 0), || "new".into())
                .unwrap_err();
            assert!(error
                .to_string()
                .contains("serverIdentityRepairUnsupported"));
            assert_eq!(
                std::fs::read(directory.join("config.json")).unwrap(),
                before
            );
            assert_eq!(*manager.cache.read().unwrap(), cached_before);
        }
    }
}

#[test]
fn malformed_unrelated_record_blocks_complete_runtime_view_without_write() {
    let (directory, manager, mut raw) = fixture();
    raw["servers"][2]["detour"] = json!(123);
    std::fs::write(
        directory.join("config.json"),
        serde_json::to_vec(&raw).unwrap(),
    )
    .unwrap();
    let before = std::fs::read(directory.join("config.json")).unwrap();
    assert!(manager
        .repair_server_identities(&request(&manager, 0), || "new".into())
        .unwrap_err()
        .to_string()
        .contains("serverIdentityRepairUnsupported"));
    assert_eq!(
        std::fs::read(directory.join("config.json")).unwrap(),
        before
    );
}
