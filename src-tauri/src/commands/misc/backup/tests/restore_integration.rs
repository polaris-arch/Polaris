//! Host-only restore acceptance: synthetic data in disposable ConfigManager directories.
//! The Tauri picker/broadcast shell is not constructible here; this drives its real file gateway,
//! backup category functions and transactional save core, then opens a new manager from disk.

use super::*;
use crate::runtime::config::ConfigManager;
use crate::test_support::TestDir;

fn seeded_config(manager: &ConfigManager, tag: &str) -> Value {
    let mut config = manager.load_full().unwrap();
    let manual_id = format!("{tag}-manual");
    let mesh_id = format!("{tag}-mesh");
    let subscription_id = format!("{tag}-subscription");
    let subscribed_node_id = format!("{tag}-sub-node");
    let traffic_id = format!("{tag}-traffic");
    let dns_rule_id = format!("{tag}-dns-rule");
    let dns_server_id = format!("{tag}-dns-server");
    let app_id = format!("{tag}-app");
    config["servers"] = json!([
        {
            "id": manual_id, "name": manual_id, "protocol": "vless",
            "address": "node.example.invalid", "port": 443,
            "uuid": "00000000-0000-0000-0000-000000000001"
        },
        {"id": mesh_id, "name": mesh_id, "protocol": "tailscale"},
        {
            "id": subscribed_node_id, "name": subscribed_node_id, "protocol": "vless",
            "address": "sub.example.invalid", "port": 443,
            "uuid": "00000000-0000-0000-0000-000000000002",
            "subscriptionId": subscription_id
        }
    ]);
    config["selectedServerId"] = json!(manual_id);
    config["subscriptions"] = json!([{
        "id": subscription_id, "name": subscription_id,
        "url": "https://example.invalid/subscription"
    }]);
    let traffic = json!({
        "id": traffic_id, "type": "domain", "values": ["route.example.invalid"],
        "action": "direct", "enabled": true,
        "effects": {"route": {"enabled": true, "action": "direct"}}
    });
    config["trafficRules"] = json!([traffic]);
    config["policyRules"] = config["trafficRules"].clone();
    config["customRules"] = config["trafficRules"].clone();
    config["routeRuleOrder"] = json!([traffic_id]);
    config["dnsRules"] = json!([{
        "id": dns_rule_id, "type": "domain", "values": ["dns.example.invalid"],
        "action": "direct", "enabled": true,
        "effects": {"dns": {"enabled": true, "resolver": "direct", "answerMode": "real",
            "action": {"type": "server", "serverId": dns_server_id}}}
    }]);
    config["dnsRuleOrder"] = json!([dns_rule_id]);
    config["customRuleSets"] = json!([{"id": format!("{tag}-rule-set")}]);
    config["ruleResources"] = json!([{"id": format!("{tag}-rule-resource")}]);
    let mut dns_server = config["dnsServers"][0].clone();
    dns_server["id"] = json!(dns_server_id);
    dns_server["name"] = json!(format!("{tag} DNS"));
    config["dnsServers"]
        .as_array_mut()
        .unwrap()
        .push(dns_server);
    config["dnsDefaults"]["directServerId"] = json!(dns_server_id);
    config["appRules"] = json!([{
        "id": app_id, "appId": "synthetic.browser", "action": "direct", "enabled": true
    }]);
    config["appRulesSeeded"] = json!(true);
    config["customAppPresets"] = json!([{"id": format!("{tag}-preset")}]);
    config["logLevel"] = json!(if tag == "source" { "debug" } else { "warn" });
    // Synthetic markers only; never read or write the user's actual credentials or config.
    config["clashApiSecret"] = json!(format!("{tag}-local-secret"));
    config["privacyPasswordHash"] = json!(format!("{tag}-local-hash"));
    config["recentServerIds"] = json!([manual_id]);
    manager.save_full(&config).unwrap();
    manager.load_full().unwrap()
}

fn node_ids(config: &Value) -> Vec<&str> {
    config["servers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["id"].as_str().unwrap())
        .collect()
}

fn has_dns_server(config: &Value, id: &str) -> bool {
    config["dnsServers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|server| server["id"] == id)
}

#[test]
fn all_eight_categories_survive_export_preview_apply_and_reopen() {
    let source_dir = TestDir::new("polaris-backup-all-source");
    let target_dir = TestDir::new("polaris-backup-all-target");
    let source = seeded_config(&ConfigManager::new(source_dir.clone()), "source");
    let target_manager = ConfigManager::new(target_dir.clone());
    let target = seeded_config(&target_manager, "target");
    let backup_config = pick_categories(&source, &BACKUP_CATEGORIES);
    for excluded in [
        "clashApiSecret",
        "privacyPassword",
        "privacyPasswordHash",
        "recentServerIds",
    ] {
        assert!(
            backup_config.get(excluded).is_none(),
            "{excluded} must not travel"
        );
    }

    let backup_path = source_dir.path().join("synthetic.polaris-backup");
    let gateway = RecordingGateway::new(source_dir.path().join("unused-gateway"));
    let body = json!({
        "version": BACKUP_FILE_VERSION, "platform": "linux", "config": backup_config
    })
    .to_string();
    assert_eq!(
        finish_export(&gateway, Some(desktop_target(&backup_path)), &body)["success"],
        true
    );
    let ImportSource::Loaded { raw, file_path } =
        read_import_source(&gateway, Some(desktop_target(&backup_path)))
    else {
        panic!("exported document must be readable for preview");
    };
    let preview = parse_backup_content(&raw).unwrap();
    assert_eq!(detect_categories(&preview.config), BACKUP_CATEGORIES);
    for category in BACKUP_CATEGORIES {
        assert!(
            count_category(&preview.config, category) > 0,
            "{category:?}"
        );
    }
    let apply_raw = read_apply_source(&gateway, &file_path).unwrap();
    let parsed = parse_backup_content(&apply_raw).unwrap();
    let saved = crate::commands::config::backup_import_save_core(
        &target_manager,
        &parsed.config,
        &BACKUP_CATEGORIES,
        parsed.platform.as_deref(),
        "linux",
        None,
    )
    .unwrap();
    assert!(saved.skipped.is_empty());
    let reopened = ConfigManager::new(target_dir.clone()).load_full().unwrap();
    assert_eq!(
        node_ids(&reopened),
        ["source-manual", "source-mesh", "source-sub-node"]
    );
    assert_eq!(reopened["subscriptions"][0]["id"], "source-subscription");
    assert_eq!(reopened["trafficRules"][0]["id"], "source-traffic");
    assert_eq!(reopened["dnsRules"][0]["id"], "source-dns-rule");
    assert_eq!(reopened["customRuleSets"][0]["id"], "source-rule-set");
    assert_eq!(reopened["ruleResources"][0]["id"], "source-rule-resource");
    assert!(has_dns_server(&reopened, "source-dns-server"));
    assert_eq!(reopened["appRules"][0]["id"], "source-app");
    assert_eq!(reopened["logLevel"], "debug");
    assert_eq!(
        reopened["selectedServerId"],
        Value::Null,
        "stale local exit is cleared"
    );
    for excluded in ["clashApiSecret", "privacyPasswordHash", "recentServerIds"] {
        assert_eq!(
            reopened[excluded], target[excluded],
            "local {excluded} must survive"
        );
    }
}

#[test]
fn selective_restore_replaces_chosen_classes_and_preserves_unchosen_classes() {
    let source_dir = TestDir::new("polaris-backup-selected-source");
    let target_dir = TestDir::new("polaris-backup-selected-target");
    let source = seeded_config(&ConfigManager::new(source_dir.clone()), "source");
    let target_manager = ConfigManager::new(target_dir.clone());
    seeded_config(&target_manager, "target");

    let first = [
        BackupCategory::ManualNodes,
        BackupCategory::DnsRules,
        BackupCategory::AppRules,
    ];
    let first_backup = pick_categories(&source, &first);
    let saved = crate::commands::config::backup_import_save_core(
        &target_manager,
        &parse_backup_content(&json!({"version":"1.2", "config":first_backup}).to_string())
            .unwrap()
            .config,
        &first,
        None,
        "linux",
        None,
    )
    .unwrap();
    assert!(saved.skipped.is_empty());
    let after_first = ConfigManager::new(target_dir.clone()).load_full().unwrap();
    assert_eq!(
        node_ids(&after_first),
        ["source-manual", "target-mesh", "target-sub-node"]
    );
    assert_eq!(after_first["subscriptions"][0]["id"], "target-subscription");
    assert_eq!(after_first["trafficRules"][0]["id"], "target-traffic");
    assert_eq!(after_first["dnsRules"][0]["id"], "source-dns-rule");
    assert!(
        has_dns_server(&after_first, "source-dns-server"),
        "DNS rule dependency follows"
    );
    assert_eq!(after_first["appRules"][0]["id"], "source-app");
    assert_eq!(after_first["logLevel"], "warn");

    let second = [
        BackupCategory::MeshNodes,
        BackupCategory::Subscriptions,
        BackupCategory::CustomRules,
        BackupCategory::GeneralSettings,
    ];
    let second_backup = pick_categories(&source, &second);
    crate::commands::config::backup_import_save_core(
        &target_manager,
        &second_backup,
        &second,
        None,
        "linux",
        None,
    )
    .unwrap();
    let after_second = ConfigManager::new(target_dir.clone()).load_full().unwrap();
    assert_eq!(
        node_ids(&after_second),
        ["source-manual", "source-mesh", "source-sub-node"]
    );
    assert_eq!(
        after_second["subscriptions"][0]["id"],
        "source-subscription"
    );
    assert_eq!(after_second["trafficRules"][0]["id"], "source-traffic");
    assert_eq!(after_second["dnsRules"][0]["id"], "source-dns-rule");
    assert_eq!(after_second["logLevel"], "debug");
}

#[test]
fn legacy_envelopes_and_bare_config_restore_through_persistent_store() {
    let source_dir = TestDir::new("polaris-backup-legacy-source");
    let source = seeded_config(&ConfigManager::new(source_dir.clone()), "source");
    let selected = [BackupCategory::ManualNodes, BackupCategory::GeneralSettings];
    let backup = pick_categories(&source, &selected);
    let formats = [
        json!({"version":"1.0", "config":backup}).to_string(),
        json!({"version":"1.1", "config":backup}).to_string(),
        backup.to_string(),
    ];
    for (index, raw) in formats.iter().enumerate() {
        let target_dir = TestDir::new(&format!("polaris-backup-legacy-{index}"));
        let manager = ConfigManager::new(target_dir.clone());
        seeded_config(&manager, "target");
        let parsed = parse_backup_content(raw).unwrap();
        assert_eq!(parsed.platform, None);
        crate::commands::config::backup_import_save_core(
            &manager,
            &parsed.config,
            &selected,
            parsed.platform.as_deref(),
            "linux",
            None,
        )
        .unwrap();
        let reopened = ConfigManager::new(target_dir.clone()).load_full().unwrap();
        assert_eq!(
            node_ids(&reopened),
            ["source-manual", "target-mesh", "target-sub-node"]
        );
        assert_eq!(reopened["logLevel"], "debug");
    }
}

#[test]
fn legacy_shared_policy_preview_can_restore_only_dns_without_replacing_local_traffic() {
    let legacy = json!({
        "servers": [], "configSchemaVersion": 2,
        "policyRules": [{
            "id": "legacy-both", "type": "domain", "values": ["legacy.example.invalid"],
            "action": "direct", "enabled": true,
            "effects": {
                "route": {"enabled": true, "action": "direct"},
                "dns": {"enabled": true, "resolver": "direct", "answerMode": "real"}
            }
        }]
    });
    for (index, raw) in [
        json!({"version":"1.0", "config":legacy}).to_string(),
        json!({"version":"1.1", "config":legacy}).to_string(),
        legacy.to_string(),
    ]
    .iter()
    .enumerate()
    {
        let dir = TestDir::new(&format!("polaris-backup-legacy-shared-policy-{index}"));
        let manager = ConfigManager::new(dir.clone());
        let before = seeded_config(&manager, "target");
        let parsed = parse_backup_content(raw).unwrap();
        let available = detect_categories(&parsed.config);
        assert!(available.contains(&BackupCategory::DnsRules));
        let saved = crate::commands::config::backup_import_save_core(
            &manager,
            &parsed.config,
            &[BackupCategory::DnsRules],
            parsed.platform.as_deref(),
            "linux",
            None,
        )
        .unwrap();
        assert!(saved.skipped.is_empty());
        let reopened = ConfigManager::new(dir.clone()).load_full().unwrap();
        assert_eq!(reopened["trafficRules"], before["trafficRules"]);
        assert_eq!(reopened["dnsRules"][0]["id"], "legacy-both");
        assert!(has_dns_server(&reopened, "target-dns-server"));
    }
}

#[test]
fn malformed_or_semantically_invalid_import_leaves_memory_disk_and_reopen_unchanged() {
    let dir = TestDir::new("polaris-backup-invalid-target");
    let manager = ConfigManager::new(dir.clone());
    let before = seeded_config(&manager, "target");
    let disk_before = std::fs::read(manager.path()).unwrap();
    for raw in ["not json", "{\"hello\":\"world\"}"] {
        assert!(parse_backup_content(raw).is_err());
        assert_eq!(manager.current().unwrap(), before);
        assert_eq!(std::fs::read(manager.path()).unwrap(), disk_before);
    }

    // A valid envelope can still carry an invalid setting; atomic save must reject the entire
    // selected class rather than half-persisting it or changing the in-memory current config.
    let parsed = parse_backup_content(
        &json!({"version":"1.2", "config":{"logLevel":"not-a-level"}}).to_string(),
    )
    .unwrap();
    assert!(crate::commands::config::backup_import_save_core(
        &manager,
        &parsed.config,
        &[BackupCategory::GeneralSettings],
        None,
        "linux",
        None,
    )
    .is_err());
    assert_eq!(manager.current().unwrap(), before);
    assert_eq!(std::fs::read(manager.path()).unwrap(), disk_before);
    assert_eq!(ConfigManager::new(dir.clone()).load_full().unwrap(), before);
}

#[test]
fn empty_foreign_rule_class_does_not_disable_or_clear_local_rules_and_dependencies() {
    let dir = TestDir::new("polaris-backup-empty-foreign-rules");
    let manager = ConfigManager::new(dir.clone());
    let mut current = seeded_config(&manager, "target");
    current["trafficRules"][0]["type"] = json!("processName");
    current["trafficRules"][0]["values"] = json!(["synthetic.exe"]);
    current["policyRules"] = current["trafficRules"].clone();
    current["customRules"] = current["trafficRules"].clone();
    manager.save_full(&current).unwrap();
    let current = manager.load_full().unwrap();
    let backup = json!({
        "trafficRules": [], "customRuleSets": [], "ruleResources": [],
        "networkProfiles": [{"id":"foreign-profile"}]
    });
    let saved = crate::commands::config::backup_import_save_core(
        &manager,
        &backup,
        &[BackupCategory::CustomRules],
        Some("darwin"),
        "linux",
        None,
    )
    .unwrap();
    assert_eq!(saved.skipped, [BackupCategory::CustomRules]);
    assert_eq!(saved.cross_platform_disabled_rules, 0);
    let reopened = ConfigManager::new(dir.clone()).load_full().unwrap();
    assert_eq!(reopened["trafficRules"][0]["enabled"], true);
    assert_eq!(reopened["customRuleSets"], current["customRuleSets"]);
    assert_eq!(reopened["ruleResources"], current["ruleResources"]);
    assert_eq!(reopened["networkProfiles"], current["networkProfiles"]);
}
