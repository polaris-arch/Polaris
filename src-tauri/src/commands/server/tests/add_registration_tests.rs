use super::*;

fn warp_node() -> Value {
    serde_json::from_str(include_str!(
        "../../../../../crates/mesh/src/warp/tests/registration-server.json"
    ))
    .unwrap()
}

#[test]
fn add_registration_warp_renderer_payload_survives_disk_readback() {
    let dir = temp_dir("warp-contract");
    let node = warp_node();
    let registered: polaris_mesh::warp::WarpWireGuardDraft = serde_json::from_str(include_str!(
        "../../../../../crates/mesh/src/warp/tests/registration-draft.json"
    ))
    .unwrap();
    let ipc = serde_json::to_value(registered).unwrap();
    assert_eq!(node["wireguardSettings"]["privateKey"], ipc["privateKey"]);
    assert_eq!(
        node["wireguardSettings"]["peerPublicKey"],
        ipc["peerPublicKey"]
    );
    let mgr = ConfigManager::new(dir.clone());
    server_add_core(&mgr, node.clone()).unwrap();
    let from_disk = ConfigManager::new(dir.clone()).load_full().unwrap();
    assert_eq!(from_disk["servers"], json!([node]));
}

#[test]
fn add_registration_rejects_missing_protocol_fields_without_writing() {
    let dir = temp_dir("invalid-protocols");
    let mgr = ConfigManager::new(dir.clone());
    seed_switch_nodes(&mgr);
    let before = mgr.load_full().unwrap();
    for protocol in ["wireguard", "openconnect", "openvpn-client"] {
        let invalid = json!({
            "id": "invalid-new-node", "name": "Incomplete", "protocol": protocol,
            "address": "example.test", "port": 443
        });
        let error = server_add_core(&mgr, invalid).unwrap_err();
        assert!(error.contains("必填"));
        assert_eq!(ConfigManager::new(dir.clone()).load_full().unwrap(), before);
    }
    let mut invalid_cidr = warp_node();
    invalid_cidr["wireguardSettings"]["localAddress"] = json!(["not-a-cidr"]);
    assert!(server_add_core(&mgr, invalid_cidr).is_err());
    assert_eq!(ConfigManager::new(dir.clone()).load_full().unwrap(), before);
}

#[test]
fn add_registration_retry_is_idempotent_after_normalization_and_rejects_collision() {
    let dir = temp_dir("retry-id");
    let mgr = ConfigManager::new(dir.clone());
    let node = warp_node();
    server_add_core(&mgr, node.clone()).unwrap();
    let before = mgr.load_full().unwrap();
    let path = dir.join("config.json");
    let disk_before = std::fs::read(&path).unwrap();
    let mut same_normalized = node.clone();
    same_normalized["bindInterface"] = json!("  ");
    assert_eq!(server_add_core(&mgr, same_normalized).unwrap(), before);
    assert_eq!(std::fs::read(&path).unwrap(), disk_before);
    let mut collision = node;
    collision["name"] = json!("Different node");
    assert!(server_add_core(&mgr, collision)
        .unwrap_err()
        .contains("标识"));
    assert_eq!(std::fs::read(path).unwrap(), disk_before);
    assert_eq!(ConfigManager::new(dir.clone()).load_full().unwrap(), before);
}

#[test]
fn add_registration_keeps_distinct_tailscale_nodes() {
    let dir = temp_dir("store-multiple-tailscale");
    let mgr = ConfigManager::new(dir.clone());
    let first = json!({"id":"ts-existing", "name":"TS", "protocol":"tailscale"});
    server_add_core(&mgr, first).unwrap();
    let second = json!({"id":"ts-new", "name":"TS new", "protocol":"tailscale"});
    server_add_core(&mgr, second).unwrap();
    let persisted = ConfigManager::new(dir.clone()).load_full().unwrap();
    assert_eq!(persisted["servers"].as_array().unwrap().len(), 2);
    assert_eq!(persisted["servers"][0]["id"], "ts-existing");
    assert_eq!(persisted["servers"][1]["id"], "ts-new");
}
