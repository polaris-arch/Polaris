use super::*;
use crate::test_support::TestDir;

#[test]
fn same_length_source_replaced_between_lstat_and_open_is_rejected() {
    let dir = TestDir::new("polaris-rule-source-race-");
    let source = dir.path().join("source.json");
    let replacement = dir.path().join("replacement.json");
    fs::write(&source, b"first").unwrap();
    fs::write(&replacement, b"other").unwrap();
    assert_eq!(
        read_trusted_source_after_lstat(&source, &[dir.path().to_path_buf()], || {
            fs::remove_file(&source).unwrap();
            fs::rename(&replacement, &source).unwrap();
        })
        .unwrap_err(),
        MaterializeError::SourceChanged
    );
}

fn config_with_source(path: &Path) -> SingBoxConfig {
    serde_json::from_value(serde_json::json!({
        "log":{"level":"info", "timestamp":true}, "inbounds":[], "outbounds":[],
        "route":{"rules":[], "rule_set":[{"type":"local", "format":"source", "tag":"owned-rule", "path":path}]}
    })).unwrap()
}

#[test]
fn captured_file_values_keep_identity_and_detect_replacement_before_relocation() {
    let dir = TestDir::new("polaris-owned-rule-snapshot-");
    let path = dir.join("source.json");
    let bytes = br#"{"version":1,"rules":[{"ip_cidr":["10.1.0.0/16"]}]}"#;
    fs::write(&path, bytes).unwrap();
    let config = config_with_source(&path);
    let owned = capture_local_rule_sets(&config, &[dir.path().to_path_buf()]).unwrap();
    assert_eq!(
        owned.bytes_at(path.to_str().unwrap()),
        Some(bytes.as_slice())
    );
    assert_eq!(owned.verify_sources_unchanged(), Ok(()));
    let replacement = dir.join("replacement.json");
    fs::write(&replacement, bytes).unwrap();
    fs::rename(&replacement, &path).unwrap();
    assert_eq!(
        owned.verify_sources_unchanged(),
        Err(MaterializeError::SourceChanged)
    );
    assert_eq!(
        owned.bytes_at(path.to_str().unwrap()),
        Some(bytes.as_slice())
    );
}

#[test]
fn captured_rule_tag_format_and_path_cannot_be_swapped() {
    let dir = TestDir::new("polaris-owned-rule-fields-");
    let path = dir.join("source.json");
    fs::write(&path, b"original file").unwrap();
    let config = config_with_source(&path);
    let owned = capture_local_rule_sets(&config, &[dir.path().to_path_buf()]).unwrap();
    let plan = polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan {
        schema_version: 1,
        plan_id: "pure-relocation".into(),
        config_version: "v1".into(),
        input_state_revision: "1".into(),
        identity_bindings: vec![],
        protected_cidrs: vec![],
        owner_routes: vec![],
        reject_cidrs: vec![],
        unassigned_cidrs: vec![],
        released_cidrs: vec![],
        overrides: vec![],
        dns_managed: false,
    };
    let (relocated, payloads) =
        relocate_owned_rule_sets(dir.path(), &plan, &config, &owned).unwrap();
    assert_eq!(payloads[0].bytes, b"original file");
    assert!(relocated.route.unwrap().rule_set.unwrap()[0]
        .path
        .as_ref()
        .unwrap()
        .ends_with("rules/rs-0000.json"));
    for field in ["tag", "format", "path"] {
        let mut changed = serde_json::to_value(&config).unwrap();
        changed["route"]["rule_set"][0][field] = serde_json::json!("foreign");
        let changed: SingBoxConfig = serde_json::from_value(changed).unwrap();
        assert_eq!(
            relocate_owned_rule_sets(dir.path(), &plan, &changed, &owned).unwrap_err(),
            MaterializeError::SnapshotMismatch
        );
    }
}
