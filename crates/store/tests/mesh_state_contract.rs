use polaris_config_engine::user_config::mesh_route_state::{
    revise_semantic, MeshRoutePolicy, MeshRouteState,
};
use polaris_store::backup::{merge_categories, pick_categories, BackupCategory};
use polaris_store::mesh_guard::{reconcile_untrusted, validate_raw, POLICY_KEY, STATE_KEY};
use polaris_store::store::default_config;
use polaris_store::{ConfigStore, StdFs};
use serde_json::{json, Value};
use tempfile::TempDir;

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap()
}

fn managed_config() -> Value {
    let mut config = default_config();
    let wire = fixture();
    config[POLICY_KEY] = wire[POLICY_KEY].clone();
    config[STATE_KEY] = wire[STATE_KEY].clone();
    config
}

#[test]
fn shared_wire_fixture_is_strict_and_revision_is_decimal_string() {
    let fixture = fixture();
    let policy: MeshRoutePolicy = serde_json::from_value(fixture[POLICY_KEY].clone()).unwrap();
    let state: MeshRouteState = serde_json::from_value(fixture[STATE_KEY].clone()).unwrap();
    policy.validate().unwrap();
    state.validate().unwrap();
    assert_eq!(state.revision, "18446744073709551614");
    assert_eq!(serde_json::to_value(state).unwrap(), fixture[STATE_KEY]);
    assert_eq!(serde_json::to_value(policy).unwrap(), fixture[POLICY_KEY]);
}

#[test]
fn managed_disk_load_preserves_policy_and_ledger_exactly() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.json");
    let config = managed_config();
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    let loaded = ConfigStore::load(&StdFs, &path);
    assert!(loaded.protected_error.is_none());
    assert_eq!(loaded.config[POLICY_KEY], config[POLICY_KEY]);
    assert_eq!(loaded.config[STATE_KEY], config[STATE_KEY]);
}

#[test]
fn invalid_or_incomplete_managed_disk_never_falls_back_to_writable_default() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("config.json");
    for bad in [
        {
            let mut v = managed_config();
            v.as_object_mut().unwrap().remove(STATE_KEY);
            v
        },
        {
            let mut v = managed_config();
            v[STATE_KEY]["schemaVersion"] = json!(2);
            v
        },
        {
            let mut v = managed_config();
            v[STATE_KEY]["revision"] = json!(9007199254740993u64);
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["unexpected"] = json!(true);
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["migration"]["builtinExceptionsVersion"] = json!(0);
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["migration"]["builtinExceptionsVersion"] = json!(999);
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["assignments"][1]["target"]["unexpected"] = json!(true);
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["assignments"][1]["target"] =
                json!({"kind":"unmanaged","unexpected":true});
            v
        },
        {
            let mut v = managed_config();
            v[STATE_KEY]["reservations"][0]["ownerRef"] = json!({"kind":"deny","unexpected":true});
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["dnsPolicy"] = Value::Null;
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["dnsPolicy"] = json!({
                "schemaVersion":1,
                "suffixAssignments":[{"suffix":"tail.example.invalid","target":{"kind":"reject","unexpected":true}}],
                "shortNamePolicy":{"kind":"system"},
                "serviceOwner":{"kind":"reject"}
            });
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["dnsPolicy"] = json!({
                "schemaVersion":1,
                "suffixAssignments":[],
                "shortNamePolicy":{"kind":"system","unexpected":true},
                "serviceOwner":{"kind":"reject"}
            });
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["dnsPolicy"] = json!({
                "schemaVersion":1,
                "suffixAssignments":[],
                "shortNamePolicy":{"kind":"reject","unexpected":true},
                "serviceOwner":{"kind":"reject"}
            });
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["dnsPolicy"] = json!({
                "schemaVersion":1,
                "suffixAssignments":[],
                "shortNamePolicy":{"kind":"system"},
                "serviceOwner":{"kind":"reject","unexpected":true}
            });
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["dnsPolicy"] = json!({"schemaVersion":2,"suffixAssignments":[],"shortNamePolicy":{"kind":"system"},"serviceOwner":{"kind":"reject"}});
            v
        },
        {
            let mut v = managed_config();
            v[POLICY_KEY]["dnsPolicy"] = json!({"schemaVersion":1,"suffixAssignments":[],"shortNamePolicy":{"kind":"system"}});
            v
        },
        {
            let mut v = managed_config();
            v[STATE_KEY]["identities"][0]["identityEpoch"] = json!("");
            v
        },
        {
            let mut v = managed_config();
            v[STATE_KEY]["observations"][0]["rawHosts"] = json!(["100.80.1.2/24"]);
            v
        },
    ] {
        let bytes = serde_json::to_string(&bad).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let load = ConfigStore::load(&StdFs, &path);
        assert!(
            load.protected_error.is_some(),
            "bad managed disk must block: {bad}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        assert!(ConfigStore::canonicalize_for_save(&bad).is_err());
    }
}

#[test]
fn old_frontend_snapshot_preserves_exact_ledger_but_forged_edit_is_rejected() {
    let previous = managed_config();
    let mut old_snapshot = default_config();
    old_snapshot["mixedPort"] = json!(9000);
    reconcile_untrusted(&previous, &mut old_snapshot).unwrap();
    assert_eq!(old_snapshot[STATE_KEY], previous[STATE_KEY]);
    assert_eq!(old_snapshot[POLICY_KEY], previous[POLICY_KEY]);
    assert_eq!(old_snapshot["mixedPort"], json!(9000));

    old_snapshot[STATE_KEY]["revision"] = json!("18446744073709551615");
    assert!(reconcile_untrusted(&previous, &mut old_snapshot).is_err());
    assert!(validate_raw(&default_config()).is_ok());
    let mut forged = default_config();
    forged[POLICY_KEY] = previous[POLICY_KEY].clone();
    assert!(reconcile_untrusted(&default_config(), &mut forged).is_err());
}

#[test]
fn old_policy_snapshot_cannot_implicitly_disable_managed_dns() {
    let mut previous = managed_config();
    previous[POLICY_KEY]["dnsPolicy"] = json!({
        "schemaVersion": 1,
        "suffixAssignments": [{
            "suffix": "tail.example.invalid",
            "target": {"kind": "reject"}
        }],
        "shortNamePolicy": {"kind": "system"},
        "serviceOwner": {"kind": "reject"}
    });
    validate_raw(&previous).unwrap();

    let mut omitted_parent = default_config();
    reconcile_untrusted(&previous, &mut omitted_parent).unwrap();
    assert_eq!(omitted_parent[POLICY_KEY], previous[POLICY_KEY]);

    let mut old_parent = previous.clone();
    old_parent[POLICY_KEY]
        .as_object_mut()
        .unwrap()
        .remove("dnsPolicy");
    assert!(reconcile_untrusted(&previous, &mut old_parent).is_err());
    assert_eq!(
        previous[POLICY_KEY]["dnsPolicy"]["suffixAssignments"][0]["target"]["kind"],
        "reject"
    );
}

#[test]
fn legacy_backup_categories_never_export_or_import_mesh_evidence() {
    let current = managed_config();
    let selected = [BackupCategory::GeneralSettings, BackupCategory::MeshNodes];
    let backup = pick_categories(&current, &selected);
    assert!(backup.get(POLICY_KEY).is_none());
    assert!(backup.get(STATE_KEY).is_none());

    let mut foreign = backup;
    foreign[POLICY_KEY] = current[POLICY_KEY].clone();
    foreign[STATE_KEY] = current[STATE_KEY].clone();
    foreign[STATE_KEY]["localId"] = json!("foreign-device");
    let merged = merge_categories(&current, &foreign, &selected).config;
    assert_eq!(merged[POLICY_KEY], current[POLICY_KEY]);
    assert_eq!(merged[STATE_KEY], current[STATE_KEY]);
}

#[test]
fn semantic_revision_cas_ignores_clock_order_and_equivalent_ip_but_detects_new_scope() {
    let mut previous: MeshRouteState =
        serde_json::from_value(fixture()[STATE_KEY].clone()).unwrap();
    previous.revision = "4".into();
    previous.observations[0].raw_hosts = vec!["100.80.1.3/32".into(), "100.80.1.2".into()];
    previous.observations[0]
        .raw_hosts
        .push("fd7a:115c:a1e0:0:0:0:0:53/128".into());
    previous.observations[0].magic_dns_suffixes = vec!["Tail.Example.Invalid.".into()];
    let mut same = previous.clone();
    same.observations[0].raw_hosts.reverse();
    same.observations[0].raw_hosts[0] = "fd7a:115c:a1e0::53".into();
    same.observations[0].raw_hosts[1] = "100.80.1.2/32".into();
    same.observations[0].raw_hosts[2] = "100.80.1.3".into();
    same.observations[0].last_valid_evidence = Some("later-sample".into());
    same.observations[0].magic_dns_suffixes.clear();
    assert!(revise_semantic(&previous, "4", same).unwrap().is_none());

    let mut changed = previous.clone();
    changed.observations[0]
        .advertised_routes
        .push("100.81.0.42/16".into());
    let committed = revise_semantic(&previous, "4", changed).unwrap().unwrap();
    assert_eq!(committed.revision, "5");
    assert!(committed.observations[0]
        .advertised_routes
        .contains(&"100.81.0.0/16".into()));
    assert_eq!(
        committed.observations[0].magic_dns_suffixes,
        ["tail.example.invalid"]
    );
    assert!(revise_semantic(&committed, "4", committed.clone()).is_err());

    let mut exhausted = previous.clone();
    exhausted.revision = u64::MAX.to_string();
    let mut next = exhausted.clone();
    next.observations[0].raw_hosts.push("100.80.1.4".into());
    assert!(revise_semantic(&exhausted, &u64::MAX.to_string(), next).is_err());
}

#[test]
fn managed_ordinary_writes_block_identity_changes_without_scope_transaction() {
    let mut previous = managed_config();
    previous["servers"] = json!([{
        "id": "ts-a", "name": "Old display name", "protocol": "tailscale",
        "tailscaleSettings": {
            "controlUrl": "https://control.example.test/path",
            "sourceTag": "source-a", "authKey": "secret-a"
        }
    }]);
    let mut renamed = previous.clone();
    renamed["servers"][0]["name"] = json!("New display name");
    reconcile_untrusted(&previous, &mut renamed).unwrap();

    for mut changed in [
        {
            let mut next = previous.clone();
            next["servers"] = json!([]);
            next
        },
        {
            let mut next = previous.clone();
            next["servers"][0]["tailscaleSettings"]["controlUrl"] =
                json!("https://other.example.test/path");
            next
        },
        {
            let mut next = previous.clone();
            next["servers"][0]["protocol"] = json!("wireguard");
            next
        },
        {
            let mut next = previous.clone();
            next["servers"][0]["tailscaleSettings"]["sourceTag"] = json!("source-b");
            next
        },
        {
            let mut next = previous.clone();
            next["servers"][0]["tailscaleSettings"]["authKey"] = json!("secret-b");
            next
        },
    ] {
        let error = reconcile_untrusted(&previous, &mut changed).unwrap_err();
        assert!(error.to_string().contains("trusted retirement transaction"));
        assert!(!error.to_string().contains("secret-a"));
        assert!(!error.to_string().contains("secret-b"));
    }

    let mut legacy = previous.clone();
    legacy.as_object_mut().unwrap().remove(POLICY_KEY);
    legacy.as_object_mut().unwrap().remove(STATE_KEY);
    let mut deleted = legacy.clone();
    deleted["servers"] = json!([]);
    reconcile_untrusted(&legacy, &mut deleted).unwrap();
}
