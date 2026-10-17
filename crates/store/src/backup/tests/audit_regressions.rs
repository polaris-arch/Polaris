use super::*;
#[test]
fn partial_rule_restore_rejects_shared_profile_collision() {
    let current = serde_json::json!({"servers":[],"subscriptions":[],"networkProfiles":[{"id":"p","name":"current"}]});
    let backup = serde_json::json!({"servers":[],"subscriptions":[],"networkProfiles":[{"id":"p","name":"backup"}],"trafficRules":[{"id":"r","type":"domain","values":["example.com"],"action":"direct","enabled":true}],"dnsRules":[{"id":"d","type":"domain","values":["example.com"],"enabled":true,"effects":{"dns":{"enabled":true,"action":{"type":"server","serverId":"builtin-domestic"}}}}]});
    for selected in [BackupCategory::CustomRules, BackupCategory::DnsRules] {
        let result = merge_categories(&current, &backup, &[selected]);
        assert_eq!(result.config, current);
        assert!(result.conflicts.iter().any(|c| c == "networkProfiles:p"));
    }
    let mut empty_dns = backup.clone();
    empty_dns["dnsRules"] = serde_json::json!([]);
    let skipped = merge_categories(
        &current,
        &empty_dns,
        &[BackupCategory::CustomRules, BackupCategory::DnsRules],
    );
    assert_eq!(skipped.config, current);
    assert!(skipped.conflicts.iter().any(|c| c == "networkProfiles:p"));
    let mut empty_traffic = backup.clone();
    empty_traffic["trafficRules"] = serde_json::json!([]);
    let skipped = merge_categories(
        &current,
        &empty_traffic,
        &[BackupCategory::CustomRules, BackupCategory::DnsRules],
    );
    assert_eq!(skipped.config, current);
    assert!(skipped.conflicts.iter().any(|c| c == "networkProfiles:p"));
    let both = merge_categories(
        &current,
        &backup,
        &[BackupCategory::CustomRules, BackupCategory::DnsRules],
    );
    assert!(both.conflicts.is_empty());
    assert_eq!(both.config["networkProfiles"][0]["name"], "backup");
}
