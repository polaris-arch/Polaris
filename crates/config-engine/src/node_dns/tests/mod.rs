use super::*;
use crate::builder::dns::{build_dns_config, DnsConfigDeps};
use crate::user_config::dns_config::{CustomDnsUpstream, DnsConfig as UserDnsConfig};
use std::collections::BTreeMap;

fn generated(platform: &str, override_text: Option<&str>) -> DnsConfig {
    let config = UserConfig {
        dns_config: Some(UserDnsConfig {
            node_resolver_pool: Some(vec![
                "ali".into(),
                "dnspod".into(),
                "udp".into(),
                "system".into(),
            ]),
            node_resolver_custom: Some(vec![CustomDnsUpstream {
                id: "udp".into(),
                spec: "udp://9.9.9.9:5353".into(),
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    build_dns_config(
        &config,
        &BTreeMap::new(),
        &DnsConfigDeps {
            platform: platform.into(),
            node_dns_decoy_override: override_text.map(str::to_string),
            lan_resolver_for_dns: None,
            pending_endpoints: vec![],
            log: |_, _| {},
            selected_server_tag: "proxy-selector".into(),
            probe_pool_ports: vec![],
            probe_proxy_port: None,
            custom_rules_dir: "/absent/custom".into(),
            runtime_rules_dir: "/absent/runtime".into(),
            rule_resources_path: "/absent/resources".into(),
            is_valid_srs_fn: |_| false,
            exists_fn: |_| false,
            system_dns_takeover_active: false,
            netenv_dhcp_suppressed: false,
        },
    )
}

#[test]
fn frozen_wire_contract_and_tier_barriers() {
    let dns = generated("linux", None);
    let rules: Vec<_> = dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .filter(|r| r.resolver_policy.as_deref() == Some(POLICY))
        .collect();
    let evaluations: Vec<_> = rules
        .iter()
        .filter(|r| r.action.as_deref() == Some("evaluate"))
        .collect();
    assert_eq!(evaluations.len(), 4);
    assert_eq!(
        evaluations
            .iter()
            .map(|r| r.speculative)
            .collect::<Vec<_>>(),
        vec![None, Some(true), None, Some(true)]
    );
    for rule in &evaluations {
        assert_eq!(rule.timeout.as_deref(), Some(CANDIDATE_TIMEOUT));
        assert_eq!(rule.disable_cache, Some(true));
        assert_eq!(rule.disable_optimistic_cache, Some(true));
    }
    assert_eq!(evaluations[3].query_type, Some(vec!["A".into()]));
    for evaluation in evaluations {
        let responses: Vec<_> = rules
            .iter()
            .filter(|r| r.match_response == evaluation.tag.clone().map(serde_json::Value::String))
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].response_class.as_deref(), Some("hit"));
        assert!(responses[0]
            .response_reject_ip_cidr
            .as_ref()
            .unwrap()
            .contains(&"31.13.0.0/16".into()));
        assert_eq!(responses[1].response_class.as_deref(), Some("empty"));
        assert_eq!(responses[1].mode.as_deref(), Some("defer"));
        assert!(responses[1].response_reject_ip_cidr.is_none());
        assert!(responses.iter().all(|r| r.race == Some(true)));
    }
    let fallback = rules.last().unwrap();
    assert_eq!(fallback.mode.as_deref(), Some("fallback"));
    assert_eq!(fallback.fallback_rcode.as_deref(), Some("SERVFAIL"));
    assert!(fallback.match_response.is_none() && fallback.race.is_none());
    assert_eq!(
        rules
            .iter()
            .filter(|r| r.mode.as_deref() == Some("fallback"))
            .count(),
        1
    );
    assert!(rules
        .iter()
        .all(|r| r.inbound.is_none() && r.rule_set.is_none()));
    for server in dns
        .servers
        .iter()
        .filter(|s| s.tag.starts_with("dns-node-native-"))
    {
        assert!(server.detour.is_none() && server.domain_resolver.is_none());
        if let Some(ip) = &server.server {
            assert!(ip.parse::<std::net::IpAddr>().is_ok());
        }
    }
    assert_eq!(
        serde_json::to_value(resolver()).unwrap(),
        serde_json::json!({"mode":"rules", "policy":"node-bootstrap", "strategy":"prefer_ipv4", "timeout":"2s"})
    );
}

#[test]
fn effective_mobile_tun_has_no_system_candidate() {
    for platform in ["android", "ios"] {
        let dns = generated(platform, None);
        assert!(dns
            .servers
            .iter()
            .filter(|s| s.tag.starts_with("dns-node-native-"))
            .all(|s| s.type_field.as_deref() != Some("local")));
    }
}

#[test]
fn decoy_override_replaces_and_invalid_override_falls_back() {
    let reject = |dns: DnsConfig| {
        dns.rules
            .unwrap()
            .into_iter()
            .find_map(|r| r.response_reject_ip_cidr)
            .unwrap()
    };
    assert_eq!(
        reject(generated("linux", Some("192.0.2.0/24\nnot a CIDR"))),
        vec!["192.0.2.0/24"]
    );
    assert_eq!(
        reject(generated("linux", Some("# empty\ninvalid"))),
        reject(generated("linux", None))
    );
}

#[test]
fn isolated_policy_keeps_lan_rejection_before_public_candidates() {
    let dns = generated("linux", None);
    let ordinary: Vec<_> = dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .filter(|r| r.resolver_policy.is_none())
        .take(2)
        .collect();
    let scoped: Vec<_> = dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .filter(|r| r.resolver_policy.as_deref() == Some(POLICY))
        .collect();
    assert_eq!(scoped[0].server.as_deref(), Some("dns-mdns"));
    assert_eq!(scoped[1].action.as_deref(), Some("predefined"));
    assert_eq!(scoped[1].rcode.as_deref(), Some("SERVFAIL"));
    assert_eq!(scoped[0].domain_suffix, ordinary[0].domain_suffix);
    assert_eq!(scoped[1].domain_regex, ordinary[1].domain_regex);
    assert_eq!(scoped[2].action.as_deref(), Some("evaluate"));
}

#[test]
fn rebuilding_mesh_protections_never_duplicates_policy_or_changes_candidates() {
    let mut dns = generated("linux", None);
    let original: Vec<_> = dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .filter(|r| matches!(r.action.as_deref(), Some("evaluate" | "respond")))
        .cloned()
        .collect();
    let protections = vec![DnsRule {
        domain_suffix: Some(vec!["private.mesh.test".into()]),
        action: Some("reject".into()),
        ..Default::default()
    }];
    replace_protections(&mut dns, &protections);
    replace_protections(&mut dns, &protections);
    let scoped: Vec<_> = dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .filter(|r| r.resolver_policy.as_deref() == Some(POLICY))
        .collect();
    assert_eq!(scoped[0].domain_suffix, protections[0].domain_suffix);
    assert_eq!(scoped[0].action.as_deref(), Some("reject"));
    assert_eq!(scoped.len(), original.len() + 1);
    assert_eq!(
        scoped[1..]
            .iter()
            .map(|r| (**r).clone())
            .collect::<Vec<_>>(),
        original
    );
}

#[test]
fn scoped_resolver_round_trips_without_changing_server_forms() {
    let native = resolver();
    let encoded = serde_json::to_value(&native).unwrap();
    assert_eq!(
        serde_json::from_value::<DomainResolver>(encoded).unwrap(),
        native
    );
    let old = DomainResolver::Tag("dns-bootstrap".into());
    assert_eq!(
        serde_json::to_value(old).unwrap(),
        serde_json::json!("dns-bootstrap")
    );
}

thread_local! {
    static DECOY_LOGS: std::cell::RefCell<Vec<(LogLevel, String)>> = const { std::cell::RefCell::new(Vec::new()) };
}
fn capture_decoy_log(level: LogLevel, message: &str) {
    DECOY_LOGS.with(|logs| logs.borrow_mut().push((level, message.into())));
}
#[test]
fn decoy_diagnostics_are_bounded_and_never_echo_override_content() {
    for text in [
        None,
        Some(""),
        Some("192.0.2.0/24\n2001:db8::/32"),
        Some("private-secret\ninvalid2\ninvalid3\ninvalid4\ninvalid5\ninvalid6\n192.0.2.0/24"),
    ] {
        DECOY_LOGS.with(|logs| logs.borrow_mut().clear());
        let cidrs = decoy_policy_cidrs(text, capture_decoy_log);
        DECOY_LOGS.with(|logs| {
            let logs = logs.borrow();
            assert!(logs.len() <= 2);
            assert!(logs.iter().all(|(_, message)| message.len() < 200
                && !message.contains("private-secret")
                && !message.contains("192.0.2")));
            match text {
                None => assert!(logs[0].1.contains("unavailable")),
                Some("") => {
                    assert_eq!(logs[0].0, LogLevel::Warn);
                    assert!(logs[0].1.contains("no valid prefixes"));
                }
                Some(value) if value.starts_with("private-secret") => {
                    assert_eq!(logs[0].0, LogLevel::Warn);
                    assert!(logs[0].1.contains("6 invalid lines"));
                    assert!(logs[0].1.ends_with("1, 2, 3, 4, 5"));
                    assert!(logs[1].1.contains("1 IPv4, 0 IPv6"));
                    assert_eq!(cidrs, vec!["192.0.2.0/24"]);
                }
                Some(_) => assert!(logs[0].1.contains("1 IPv4, 1 IPv6")),
            }
        });
    }
}
