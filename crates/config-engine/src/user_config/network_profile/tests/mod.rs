//! U14：serde 往返与容错（`FIELD_NAMES` ≡ 投影由 `app_config/tests` 的
//! `field_names_equals_serde_projection` 钉，`fully_populated` 已带非空 `network_profiles`）。

use super::*;
use crate::user_config::app_config::UserConfig;
use crate::user_config::rule::Rule;
use serde_json::json;

#[test]
fn u14_missing_keys_take_defaults() {
    let p: NetworkProfile = serde_json::from_value(json!({"id": "np"})).unwrap();
    assert_eq!(p.id, "np");
    assert!(p.enabled, "enabled 缺省必须是 true");
    assert_eq!(p.probe, NetworkProbeSource::Auto);
    assert!(p.criteria.dns_server_cidrs.is_empty() && p.criteria.search_domains.is_empty());
}

#[test]
fn u14_bad_entries_are_dropped_without_failing_user_config() {
    let cfg: UserConfig = serde_json::from_value(json!({
        "servers": [],
        "selectedServerId": null,
        "networkProfiles": [
            {"id": "ok", "match": {"dnsServerCidrs": ["10.0.0.0/8"]}, "probe": "dhcp"},
            {"id": "bad-probe", "probe": "bogus"},
            {"id": 42},
            {"name": "no-id"},
            "not-an-object",
        ],
    }))
    .expect("坏条目不得炸掉整份 UserConfig");
    let ids: Vec<&str> = cfg.network_profiles.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids, ["ok"]);
    assert_eq!(cfg.network_profiles[0].probe, NetworkProbeSource::Dhcp);

    for junk in [json!("x"), json!(null), json!({"id": "np"})] {
        let cfg: UserConfig = serde_json::from_value(json!({
            "servers": [], "selectedServerId": null, "networkProfiles": junk,
        }))
        .unwrap_or_else(|e| panic!("networkProfiles={junk} 炸了整份配置：{e}"));
        assert!(cfg.network_profiles.is_empty());
    }
}

#[test]
fn u14_round_trip_keeps_wire_keys_and_skips_empty() {
    let cfg: UserConfig = serde_json::from_value(json!({
        "servers": [], "selectedServerId": null,
        "networkProfiles": [{"id": "np", "name": "公司", "enabled": false,
            "match": {"dnsServerCidrs": ["10.20.0.0/16"], "searchDomains": ["corp.example"]},
            "probe": "system"}],
        "trafficRules": [{"id": "r", "type": "domain", "values": ["a.example"],
            "action": "direct", "enabled": true, "networkProfileId": "np"}],
    }))
    .unwrap();
    let v = serde_json::to_value(&cfg).unwrap();
    assert_eq!(
        v["networkProfiles"],
        json!([{"id": "np", "name": "公司", "enabled": false,
            "match": {"dnsServerCidrs": ["10.20.0.0/16"], "searchDomains": ["corp.example"]},
            "probe": "system"}])
    );
    assert_eq!(v["trafficRules"][0]["networkProfileId"], "np");
    let back: UserConfig = serde_json::from_value(v).unwrap();
    assert_eq!(back.network_profiles, cfg.network_profiles);

    // 缺省不序列化：老配置投影不多一个键（config_generation_norm 不变）。
    let empty = serde_json::to_value(UserConfig::default()).unwrap();
    assert!(empty.get("networkProfiles").is_none());
    let rule = serde_json::to_value(Rule::default()).unwrap();
    assert!(rule.get("networkProfileId").is_none());
}

#[test]
fn normalize_search_domain_trims_dots_and_lowercases() {
    assert_eq!(
        normalize_search_domain(" .Corp.Example. ").as_deref(),
        Some("corp.example")
    );
    assert_eq!(normalize_search_domain(" . "), None);
}
