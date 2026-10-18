use super::*;

fn explicit_fakeip_config(mode: ProxyMode, fallback: Option<DnsPolicyAction>) -> UserConfig {
    let mut config = base_config();
    config.config_schema_version = Some(4);
    config.proxy_mode = mode;
    config.dns_defaults = Some(DnsPolicyDefaults {
        unmatched_action: Some(DnsPolicyAction::FakeIp),
        real_query_fallback_action: fallback,
        ..Default::default()
    });
    config
}

fn server_action(id: &str) -> DnsPolicyAction {
    DnsPolicyAction::Server {
        server_id: id.into(),
    }
}

fn default_tail(built: &DnsConfig) -> &[DnsRule] {
    let rules = built.rules.as_ref().unwrap();
    let start = rules
        .iter()
        .position(|rule| {
            rule.server.as_deref() == Some("fakeip")
                && rule.domain.is_none()
                && rule.domain_suffix.is_none()
                && rule.rules.is_none()
        })
        .unwrap();
    let end = rules
        .iter()
        .position(|rule| rule.resolver_policy.is_some())
        .unwrap_or(rules.len());
    &rules[start..end]
}

// Deliberately only replay the unconditional default tail. Explicit/domain/LAN
// priority is checked separately; this is not a substitute for native core tests.
fn select_tail<'a>(tail: &'a [DnsRule], qtype: &str, allow_fakeip: bool) -> &'a str {
    select_tail_region(tail, qtype, allow_fakeip, false)
}

fn select_tail_region<'a>(
    tail: &'a [DnsRule],
    qtype: &str,
    allow_fakeip: bool,
    local: bool,
) -> &'a str {
    for rule in tail {
        assert!(rule.domain.is_none() && rule.domain_suffix.is_none());
        assert!(rule.inbound.is_none());
        if rule.rule_set.is_some() && !local {
            continue;
        }
        if rule
            .query_type
            .as_ref()
            .is_some_and(|types| !types.iter().any(|t| t == qtype))
        {
            continue;
        }
        if !allow_fakeip && rule.server.as_deref() == Some("fakeip") {
            continue;
        }
        return rule.server.as_deref().or(rule.action.as_deref()).unwrap();
    }
    panic!("default tail must terminate {qtype}");
}

#[test]
fn real_query_fallback_matrix_preserves_address_and_internal_lookup_paths() {
    for mode in [ProxyMode::Direct, ProxyMode::Smart, ProxyMode::Global] {
        for ipv6 in [false, true] {
            for (fallback, expected) in [
                (Some(server_action(BUILTIN_REMOTE_DNS_ID)), "dns-remote"),
                (Some(server_action(BUILTIN_DOMESTIC_DNS_ID)), "dns-domestic"),
                (None, "dns-domestic"), // old direct config-engine callers
                (
                    Some(DnsPolicyAction::Reject {
                        method: DnsRejectMethod::Default,
                    }),
                    "reject",
                ),
            ] {
                let mut config = explicit_fakeip_config(mode, fallback);
                config.enable_ipv6 = Some(ipv6);
                let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
                let tail = default_tail(&built);
                assert_eq!(tail.len(), 3);
                assert_eq!(tail[1].query_type, Some(vec!["A".into(), "AAAA".into()]));
                assert_eq!(tail[1].server.as_deref(), Some("dns-remote"));
                for qtype in ["A", "AAAA"] {
                    assert_eq!(select_tail(tail, qtype, true), "fakeip");
                    assert_eq!(select_tail(tail, qtype, false), "dns-remote");
                }
                for qtype in ["HTTPS", "SVCB", "TXT", "MX", "SRV", "CAA", "NS"] {
                    assert_eq!(select_tail(tail, qtype, true), expected);
                    assert_eq!(select_tail(tail, qtype, false), expected);
                }
            }
        }
    }
}

#[test]
fn fakeip_real_query_tail_matches_reviewed_snapshot() {
    let built = build_dns_config(
        &explicit_fakeip_config(ProxyMode::Smart, Some(server_action(BUILTIN_REMOTE_DNS_ID))),
        &BTreeMap::new(),
        &deps_false(),
    );
    let expected: serde_json::Value = serde_json::from_str(&polaris_source_probe::crate_file!(
        "fixtures/fakeip-real-query-default-tail.json"
    ))
    .unwrap();
    assert_eq!(
        serde_json::to_value(default_tail(&built)).unwrap(),
        expected
    );
}

#[test]
fn invalid_real_query_fallback_rejects_instead_of_reaching_domestic_final() {
    for fallback in [
        server_action("missing-resource"),
        server_action("builtin-netenv-dhcp"), // unavailable on the Linux test platform
        DnsPolicyAction::FakeIp,
        DnsPolicyAction::FollowRouteDefault,
        DnsPolicyAction::Group {
            group_id: "missing-group".into(),
        },
    ] {
        let config = explicit_fakeip_config(ProxyMode::Global, Some(fallback));
        let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
        let tail = default_tail(&built);
        assert_eq!(select_tail(tail, "HTTPS", true), "reject");
        assert_eq!(tail.last().unwrap().no_drop, Some(true));
        assert_eq!(select_tail(tail, "A", true), "fakeip");
    }
    let mut config = explicit_fakeip_config(ProxyMode::Smart, Some(server_action("disabled")));
    config.dns_servers.push(DnsServerResource {
        id: "disabled".into(),
        enabled: false,
        ..Default::default()
    });
    let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    assert_eq!(select_tail(default_tail(&built), "TXT", true), "reject");
    let target = config
        .dns_servers
        .iter_mut()
        .find(|server| server.id == "disabled")
        .unwrap();
    target.enabled = true;
    target.kind = DnsServerKind::Hosts;
    let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    assert_eq!(select_tail(default_tail(&built), "TXT", true), "reject");
}

#[test]
fn explicit_dns_actions_and_protected_exceptions_precede_new_default() {
    for action in [
        server_action(BUILTIN_REMOTE_DNS_ID),
        DnsPolicyAction::Reject {
            method: DnsRejectMethod::Default,
        },
    ] {
        let mut config = explicit_fakeip_config(
            ProxyMode::Global,
            Some(server_action(BUILTIN_DOMESTIC_DNS_ID)),
        );
        let mut rule = dns_effect_rule(
            "explicit",
            RuleType::Domain,
            &["explicit.example"],
            RuleDnsResolver::Proxy,
            RuleDnsAnswerMode::Real,
        );
        rule.effects.as_mut().unwrap().dns.as_mut().unwrap().action = Some(action);
        config.dns_rules = Some(vec![rule]);
        let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
        let rules = built.rules.as_ref().unwrap();
        let default_start = rules
            .iter()
            .take_while(|rule| rule.resolver_policy.is_none())
            .count()
            - default_tail(&built).len();
        let explicit_index = rules
            .iter()
            .position(|rule| {
                rule.domain
                    .as_ref()
                    .is_some_and(|names| names.iter().any(|name| name == "explicit.example"))
            })
            .unwrap();
        assert!(explicit_index < default_start);
        assert!(rules[explicit_index].query_type.is_none());
        assert!(rules[..default_start]
            .iter()
            .any(|rule| rule.server.as_deref() == Some("dns-bootstrap")));
        assert!(rules[..default_start]
            .iter()
            .any(|rule| rule.server.as_deref() == Some("dns-mdns")));
        // Existing ordinary filter behavior is intentionally retained, not silently
        // upgraded to the new default resolver or moved behind explicit rules.
        assert!(rules[..explicit_index].iter().any(|rule| rule
            .domain_keyword
            .as_ref()
            .is_some_and(|words| words.iter().any(|word| word == "stun"))));
    }
}

#[test]
fn fallback_is_inactive_for_real_dns_defaults_and_unmigrated_legacy() {
    let mut config = explicit_fakeip_config(
        ProxyMode::Global,
        Some(DnsPolicyAction::Reject {
            method: DnsRejectMethod::Default,
        }),
    );
    config.dns_defaults.as_mut().unwrap().unmatched_action =
        Some(server_action(BUILTIN_REMOTE_DNS_ID));
    let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    assert_eq!(
        built
            .rules
            .as_ref()
            .unwrap()
            .iter()
            .rev()
            .find(|rule| rule.resolver_policy.is_none())
            .unwrap()
            .server
            .as_deref(),
        Some("dns-remote")
    );
    config.config_schema_version = None;
    config.dns_defaults = None;
    let legacy = build_dns_config(&config, &BTreeMap::new(), &deps_false());
    let tail = default_tail(&legacy);
    assert_eq!(tail.len(), 2);
    assert_eq!(
        tail.last().unwrap().query_type,
        Some(vec!["A".into(), "AAAA".into()])
    );
    assert_eq!(legacy.final_server.as_deref(), Some("dns-domestic"));
}

#[test]
fn available_regional_shadow_and_reverse_keep_internal_address_split() {
    for mode in [ProxyMode::Direct, ProxyMode::Smart, ProxyMode::Global] {
        for reverse in [false, true] {
            let mut config = explicit_fakeip_config(
                mode,
                Some(DnsPolicyAction::Reject {
                    method: DnsRejectMethod::Default,
                }),
            );
            config.region_routing = Some(RegionRoutingConfig {
                enabled: true,
                region: "cn".into(),
                reverse,
            });
            let mut deps = deps_false();
            deps.is_valid_srs_fn = |_| true;
            let built = build_dns_config(&config, &BTreeMap::new(), &deps);
            let tail = default_tail(&built);
            assert_eq!(tail.len(), 4);
            assert!(tail[1].rule_set.is_some());
            assert!(tail[2].rule_set.is_none());
            assert_eq!(tail[2].query_type, Some(vec!["A".into(), "AAAA".into()]));
            for local in [false, true] {
                let address_server = if local != reverse {
                    "dns-domestic"
                } else {
                    "dns-remote"
                };
                for qtype in ["A", "AAAA"] {
                    assert_eq!(select_tail_region(tail, qtype, true, local), "fakeip");
                    assert_eq!(
                        select_tail_region(tail, qtype, false, local),
                        address_server
                    );
                }
                for qtype in ["HTTPS", "SVCB", "TXT"] {
                    assert_eq!(select_tail_region(tail, qtype, true, local), "reject");
                }
            }
        }
    }
}

#[test]
fn missing_or_disabled_builtin_fallback_cannot_use_legacy_transport() {
    for missing in [false, true] {
        let mut config =
            explicit_fakeip_config(ProxyMode::Smart, Some(server_action(BUILTIN_REMOTE_DNS_ID)));
        if missing {
            config
                .dns_servers
                .retain(|server| server.id != BUILTIN_REMOTE_DNS_ID);
        } else {
            config
                .dns_servers
                .iter_mut()
                .find(|server| server.id == BUILTIN_REMOTE_DNS_ID)
                .unwrap()
                .enabled = false;
        }
        let built = build_dns_config(&config, &BTreeMap::new(), &deps_false());
        assert_eq!(select_tail(default_tail(&built), "HTTPS", true), "reject");
    }
}

#[test]
fn managed_mesh_owner_and_reject_still_precede_real_query_fallback() {
    use crate::builder::mesh_dns::{build_mesh_dns_overlay, MeshDnsEmittedEndpoint};
    use crate::user_config::mesh_route_state::{MeshDnsPolicy, MeshOwnerRef, MeshRouteState};
    let fixture: serde_json::Value = serde_json::from_str(&polaris_source_probe::crate_file!(
        "fixtures/mesh-dns-policy.json"
    ))
    .unwrap();
    let policy: MeshDnsPolicy = serde_json::from_value(fixture["policy"].clone()).unwrap();
    let state: MeshRouteState = serde_json::from_value(fixture["state"].clone()).unwrap();
    let endpoints = vec![
        MeshDnsEmittedEndpoint {
            owner: MeshOwnerRef {
                server_id: "alpha".into(),
                identity_epoch: "epoch-a".into(),
            },
            tag: "alpha-tag".into(),
        },
        MeshDnsEmittedEndpoint {
            owner: MeshOwnerRef {
                server_id: "beta".into(),
                identity_epoch: "epoch-b".into(),
            },
            tag: "beta-tag".into(),
        },
    ];
    let mut deps = deps_false();
    deps.lan_resolver_for_dns = Some("192.168.1.1".into());
    let dns = build_dns_config(
        &explicit_fakeip_config(
            ProxyMode::Global,
            Some(server_action(BUILTIN_REMOTE_DNS_ID)),
        ),
        &BTreeMap::new(),
        &deps,
    );
    let overlay = build_mesh_dns_overlay(&dns, Some(&policy), Some(&state), &endpoints).unwrap();
    let rules = overlay.dns.rules.as_ref().unwrap();
    let default_start = rules
        .iter()
        .take_while(|rule| rule.resolver_policy.is_none())
        .count()
        - default_tail(&overlay.dns).len();
    let suffix_rule = |suffix: &str| {
        rules
            .iter()
            .position(|rule| {
                rule.domain_suffix.as_ref().is_some_and(|names| {
                    names
                        .iter()
                        .any(|name| name.trim_start_matches('.') == suffix)
                })
            })
            .unwrap()
    };
    let owner = suffix_rule("corp.ts.net");
    let rejected = suffix_rule("orphan.corp.ts.net");
    assert!(owner < default_start && rejected < default_start);
    assert!(rules[owner]
        .server
        .as_deref()
        .unwrap()
        .starts_with("dns-mesh-"));
    assert_eq!(rules[rejected].action.as_deref(), Some("reject"));
    assert!(rules[owner].query_type.is_none() && rules[rejected].query_type.is_none());
    assert_eq!(
        select_tail(default_tail(&overlay.dns), "TXT", true),
        "dns-remote"
    );
}
