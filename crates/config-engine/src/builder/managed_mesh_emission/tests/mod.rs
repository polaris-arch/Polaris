use super::*;
use crate::builder::managed_mesh_plan::ManagedMeshCandidate;
use crate::user_config::cidr::cidr_contains;
use crate::user_config::mesh_route_state::{
    MeshOverride, MeshOwnerRef, MeshRoutePolicy, MeshRouteState, MeshTarget,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

fn owner(server_id: &str, epoch: &str) -> MeshOwnerRef {
    MeshOwnerRef {
        server_id: server_id.into(),
        identity_epoch: epoch.into(),
    }
}

fn input() -> ManagedMeshPlanInput {
    let mut wire: Value = serde_json::from_str(include_str!(
        "../../../../../../ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    wire["meshRoutePolicy"]["dnsPolicy"] = json!({
        "schemaVersion": 1,
        "suffixAssignments": [
            {"suffix":"corp-a.ts.net","target":{"kind":"owner","serverId":"ts-a","identityEpoch":"epoch-a"}},
            {"suffix":"corp-b.ts.net","target":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}}
        ],
        "shortNamePolicy":{"kind":"owner","serverId":"ts-a","identityEpoch":"epoch-a"},
        "serviceOwner":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}
    });
    wire["meshRoutePolicy"]["assignments"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "cidr":"100.81.0.0/16",
            "target":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}
        }));
    wire["meshRouteState"]["identities"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "serverId":"ts-b","identityEpoch":"epoch-b",
            "controlAuthority":"https://other.example.invalid/",
            "bindingState":"bound","evidenceSource":"status"
        }));
    wire["meshRouteState"]["observations"][0]["magicDnsSuffixes"] = json!(["corp-a.ts.net"]);
    wire["meshRouteState"]["observations"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "ownerRef":{"serverId":"ts-b","identityEpoch":"epoch-b"},
            "rawHosts":[],"advertisedRoutes":["100.81.0.0/16"],
            "magicDnsSuffixes":["corp-b.ts.net"],
            "source":"status","lastValidEvidence":"status-b"
        }));
    ManagedMeshPlanInput {
        plan_id: "d1-plan-1".into(),
        config_version: "config-1".into(),
        policy: serde_json::from_value::<MeshRoutePolicy>(wire["meshRoutePolicy"].clone()).unwrap(),
        state: serde_json::from_value::<MeshRouteState>(wire["meshRouteState"].clone()).unwrap(),
        candidates: vec![
            ManagedMeshCandidate {
                owner_ref: owner("ts-a", "epoch-a"),
                configured_cidrs: vec!["100.80.0.0/16".into()],
                endpoint_tag: Some("ep-a".into()),
                evidence_complete: true,
            },
            ManagedMeshCandidate {
                owner_ref: owner("ts-b", "epoch-b"),
                configured_cidrs: vec!["100.81.0.0/16".into()],
                endpoint_tag: Some("ep-b".into()),
                evidence_complete: true,
            },
        ],
        scopeable_rule_matchers: BTreeMap::new(),
    }
}

fn legacy() -> SingBoxConfig {
    serde_json::from_value(json!({
        "log":{"level":"error","timestamp":false,"disabled":true},
        "dns":{
            "servers":[
                {"tag":"dns-mdns","type":"mdns"},
                {"tag":"dns-lan","type":"local"},
                {"tag":"dns-remote","type":"local"},
                {"tag":"dns-tailscale","type":"tailscale","endpoint":"ep-a","accept_search_domain":true}
            ],
            "rules":[
                {"domain_suffix":["local"],"server":"dns-mdns"},
                {"domain_regex":["^[^.]+\\.?$"],"server":"dns-lan"},
                {"preferred_by":["dns-tailscale"],"action":"route","server":"dns-tailscale"},
                {"query_type":["A","AAAA"],"server":"dns-remote"}
            ],
            "final":"dns-remote"
        },
        "inbounds":[],
        "outbounds":[{"type":"direct","tag":"direct"}],
        "endpoints":[
            {"type":"tailscale","tag":"ep-a","auth_key":"tskey-auth-example","state_directory":"/tmp/d1-a","control_url":"https://127.0.0.1","hostname":"d1-a"},
            {"type":"tailscale","tag":"ep-b","auth_key":"tskey-auth-example","state_directory":"/tmp/d1-b","control_url":"https://127.0.0.1","hostname":"d1-b"}
        ],
        "route":{
            "rules":[
                {"action":"sniff"},
                {"process_name":["sing-box","sing-box.exe"],"action":"route","outbound":"direct"},
                {"ip_cidr":["223.5.5.5/32","100.100.100.100/32"],"port":[53,443],"action":"route","outbound":"direct"},
                {"port":[53],"action":"hijack-dns"},
                {"ip_cidr":["100.64.0.0/10"],"action":"route","outbound":"direct"}
            ],
            "default_domain_resolver":"dns-remote","final":"direct"
        }
    }))
    .unwrap()
}

fn first_match<'a>(rules: &'a [RouteRule], ip: &str, network: &str, port: u32) -> &'a RouteRule {
    rules
        .iter()
        .find(|rule| {
            if matches!(rule.action.as_deref(), Some("sniff" | "resolve"))
                || rule.process_name.is_some()
                || rule.inbound.is_some()
            {
                return false;
            }
            let ip_matches = rule
                .ip_cidr
                .as_ref()
                .is_none_or(|cidrs| cidrs.iter().any(|cidr| cidr_contains(cidr, ip)));
            let net_matches = rule
                .network
                .as_ref()
                .is_none_or(|networks| networks.iter().any(|item| item == network));
            ip_matches && net_matches && rule.port.as_ref().is_none_or(|_| has_port(rule, port))
        })
        .unwrap()
}

/// Independent first-match model for the fixed core's non-terminal resolve:
/// after resolve, ip_cidr sees DestinationAddresses; failure terminates.
fn trace_domain<'a>(
    rules: &'a [RouteRule],
    answers: &[&str],
    inbound: Option<&str>,
    port: u32,
) -> Result<(Option<&'a RouteRule>, usize), &'static str> {
    trace_destination(rules, answers, inbound, port, true)
}

fn trace_ip<'a>(
    rules: &'a [RouteRule],
    ip: &str,
    inbound: Option<&str>,
    port: u32,
) -> Result<(Option<&'a RouteRule>, usize), &'static str> {
    trace_destination(rules, &[ip], inbound, port, false)
}

fn trace_destination<'a>(
    rules: &'a [RouteRule],
    answers: &[&str],
    inbound: Option<&str>,
    port: u32,
    is_domain: bool,
) -> Result<(Option<&'a RouteRule>, usize), &'static str> {
    let mut resolved = !is_domain;
    let mut resolve_count = 0;
    for rule in rules {
        if rule.action.as_deref() == Some("sniff") || rule.process_name.is_some() {
            continue;
        }
        let inbound_matches = match &rule.inbound {
            Some(OneOrMany::One(tag)) => inbound == Some(tag.as_str()),
            Some(OneOrMany::Many(tags)) => {
                inbound.is_some_and(|value| tags.iter().any(|tag| tag == value))
            }
            None => true,
        };
        if inbound_matches == rule.invert.unwrap_or(false) {
            continue;
        }
        if rule.port.is_some() && !has_port(rule, port) {
            continue;
        }
        if rule.ip_cidr.as_ref().is_some_and(|cidrs| {
            !resolved
                || !answers
                    .iter()
                    .any(|ip| cidrs.iter().any(|cidr| cidr_contains(cidr, ip)))
        }) {
            continue;
        }
        match rule.action.as_deref() {
            Some("resolve") => {
                if is_domain {
                    resolve_count += 1;
                    if answers.is_empty() {
                        return Err("resolve failed");
                    }
                    resolved = true;
                }
            }
            Some("route" | "reject" | "hijack-dns") => return Ok((Some(rule), resolve_count)),
            _ => {}
        }
    }
    Ok((None, resolve_count))
}

#[test]
fn final_rules_protect_both_magic_dns_ips_before_bootstrap_and_hijack() {
    let input = input();
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let built = emit_managed_mesh_config(&legacy(), &input, &plan).unwrap();
    let rules = &built.config.route.as_ref().unwrap().rules;
    for service in MAGIC_DNS_SERVICE_CIDRS {
        for network in MAGIC_DNS_SERVICE_TRANSPORTS {
            let hit = first_match(rules, service, network, 53);
            assert_eq!(hit.outbound.as_deref(), Some("ep-b"));
        }
    }
    assert_eq!(
        first_match(rules, "100.80.2.3/32", "tcp", 443)
            .outbound
            .as_deref(),
        Some("ep-a")
    );
    assert_eq!(
        first_match(rules, "100.81.2.3/32", "tcp", 443)
            .outbound
            .as_deref(),
        Some("ep-b")
    );
    assert_eq!(
        first_match(rules, "100.90.2.3/32", "tcp", 443)
            .action
            .as_deref(),
        Some("reject")
    );
    for network in ["tcp", "udp"] {
        let ordinary_q_dns = first_match(rules, "100.80.2.3/32", network, 53);
        assert_eq!(ordinary_q_dns.action.as_deref(), Some("hijack-dns"));
        assert_ne!(ordinary_q_dns.outbound.as_deref(), Some("direct"));
    }
    assert!(built
        .ordinary_port53_hijack_cidrs
        .iter()
        .any(|cidr| cidr_contains(cidr, "100.80.2.3/32")));
    assert_eq!(
        first_match(rules, "8.8.8.8/32", "tcp", 53)
            .action
            .as_deref(),
        Some("hijack-dns")
    );
    assert_eq!(
        first_match(rules, "223.5.5.5/32", "udp", 53)
            .outbound
            .as_deref(),
        Some("direct")
    );
    assert!(
        rules
            .iter()
            .position(|rule| rule.outbound.as_deref() == Some("ep-b") && has_port(rule, 53))
            < rules
                .iter()
                .position(|rule| rule.action.as_deref() == Some("hijack-dns"))
    );
    assert_eq!(
        built
            .dns
            .dns
            .servers
            .iter()
            .filter(|server| server.type_field.as_deref() == Some("tailscale"))
            .count(),
        2
    );
}

#[test]
fn resolved_domain_ipv6_and_fakeip_name_reach_q_before_legacy_final() {
    let input = input();
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let mut config = legacy();
    // The old general resolve is later in route.rs; managed emission must
    // remove it rather than let a second DNS lookup alter the Q decision.
    config.route.as_mut().unwrap().rules.push(RouteRule {
        action: Some("resolve".into()),
        ..Default::default()
    });
    let built = emit_managed_mesh_config(&config, &input, &plan).unwrap();
    let rules = &built.config.route.as_ref().unwrap().rules;
    assert_eq!(
        rules
            .iter()
            .filter(|rule| rule.action.as_deref() == Some("resolve"))
            .count(),
        1
    );
    for (answers, target) in [
        (vec!["100.80.2.3/32"], Some("ep-a")),
        (vec!["100.81.2.3/32"], Some("ep-b")),
    ] {
        let (rule, count) = trace_domain(rules, &answers, None, 443).unwrap();
        assert_eq!(count, 1);
        assert_eq!(rule.unwrap().outbound.as_deref(), target);
    }
    // IPv6 and a domain restored from FakeIP with an unassigned Q answer reject.
    for answers in [vec!["fd7a:115c:a1e0:1::9/128"], vec!["100.90.2.3/32"]] {
        let (rule, count) = trace_domain(rules, &answers, None, 443).unwrap();
        assert_eq!(count, 1);
        assert_eq!(rule.unwrap().action.as_deref(), Some("reject"));
    }
    assert_eq!(trace_domain(rules, &[], None, 443), Err("resolve failed"));
}

#[test]
fn subscription_guard_precedes_q_and_pin_cannot_preempt_q() {
    let input = input();
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let mut config = legacy();
    config.route.as_mut().unwrap().rules.splice(
        1..1,
        crate::builder::subscription_guard::subscription_update_route_rules("direct"),
    );
    let built = emit_managed_mesh_config(&config, &input, &plan).unwrap();
    let rules = &built.config.route.as_ref().unwrap().rules;
    let sub: Vec<_> = rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| has_subscription_inbound(rule))
        .collect();
    assert_eq!(sub.len(), 4); // dedicated three plus inverted generic resolve
    assert_eq!(sub[0].1.action.as_deref(), Some("resolve"));
    assert_eq!(sub[1].1.action.as_deref(), Some("reject"));
    assert_eq!(sub[2].1.invert, Some(true));
    assert_eq!(sub[3].1.outbound.as_deref(), Some("direct"));
    let owner_at = rules
        .iter()
        .position(|rule| rule.outbound.as_deref() == Some("ep-a"))
        .unwrap();
    assert!(sub[0].0 < sub[1].0 && sub[1].0 < owner_at && owner_at < sub[3].0);
    let (private, private_resolves) = trace_domain(
        rules,
        &["100.80.2.3/32"],
        Some("subscription-update-in"),
        443,
    )
    .unwrap();
    assert_eq!(private_resolves, 1);
    assert_eq!(private.unwrap().action.as_deref(), Some("reject"));
    let (known_private, known_private_resolves) =
        trace_ip(rules, "100.80.2.3/32", Some("subscription-update-in"), 443).unwrap();
    assert_eq!(known_private_resolves, 0);
    assert_eq!(known_private.unwrap().action.as_deref(), Some("reject"));
    let (public, public_resolves) = trace_domain(
        rules,
        &["203.0.113.5/32"],
        Some("subscription-update-in"),
        443,
    )
    .unwrap();
    assert_eq!(public_resolves, 1);
    assert_eq!(public.unwrap().outbound.as_deref(), Some("direct"));
}

#[test]
fn unproved_legacy_resolution_or_internal_pin_blocks_managed_emission() {
    let input = input();
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let mut config = legacy();
    config.route.as_mut().unwrap().rules.insert(
        1,
        RouteRule {
            inbound: Some(OneOrMany::Many(vec!["probe-direct-in".into()])),
            action: Some("route".into()),
            outbound: Some("direct".into()),
            ..Default::default()
        },
    );
    assert!(emit_managed_mesh_config(&config, &input, &plan)
        .unwrap_err()
        .contains("inbound pin"));
    let mut config = legacy();
    config.route.as_mut().unwrap().rules.push(RouteRule {
        inbound: Some(OneOrMany::Many(vec!["custom-in".into()])),
        action: Some("resolve".into()),
        ..Default::default()
    });
    assert!(emit_managed_mesh_config(&config, &input, &plan).is_err());
    let mut config = legacy();
    config.route.as_mut().unwrap().rules.push(RouteRule {
        domain_suffix: Some(vec!["bank.example".into()]),
        action: Some("route".into()),
        outbound: Some("direct".into()),
        ..Default::default()
    });
    assert!(emit_managed_mesh_config(&config, &input, &plan)
        .unwrap_err()
        .contains("domain-pinned"));
}

#[test]
fn absent_dns_policy_preserves_legacy_dns_and_service_route() {
    let mut input = input();
    input.policy.dns_policy = None;
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let original = legacy();
    let built = emit_managed_mesh_config(&original, &input, &plan).unwrap();
    assert_eq!(built.config.dns, original.dns);
    assert!(!built.ordinary_port53_hijack_cidrs.is_empty());
    let rules = &built.config.route.as_ref().unwrap().rules;
    assert_eq!(
        first_match(rules, MAGIC_DNS_SERVICE_CIDRS[0], "udp", 53)
            .outbound
            .as_deref(),
        Some("direct")
    );
    assert_eq!(
        first_match(rules, "100.80.2.3/32", "udp", 53)
            .action
            .as_deref(),
        Some("hijack-dns")
    );
}

#[test]
fn missing_endpoint_and_stale_plan_cannot_fall_to_another_tailnet() {
    let mut input = input();
    input.candidates[1].endpoint_tag = None;
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let mut config = legacy();
    config
        .endpoints
        .as_mut()
        .unwrap()
        .retain(|endpoint| endpoint.tag != "ep-b");
    let built = emit_managed_mesh_config(&config, &input, &plan).unwrap();
    let rules = &built.config.route.as_ref().unwrap().rules;
    assert_eq!(
        first_match(rules, MAGIC_DNS_SERVICE_CIDRS[0], "udp", 53)
            .action
            .as_deref(),
        Some("reject")
    );
    assert_eq!(
        first_match(rules, "100.81.2.3/32", "tcp", 443)
            .action
            .as_deref(),
        Some("reject")
    );
    assert!(built
        .dns
        .dns
        .servers
        .iter()
        .all(|server| server.endpoint.as_deref() != Some("ep-b")));

    input.candidates[1].endpoint_tag = Some("ep-b".into());
    assert!(emit_managed_mesh_config(&config, &input, &plan).is_err());
    let current_plan = compile_managed_mesh_plan(input.clone()).unwrap();
    assert!(emit_managed_mesh_config(&config, &input, &current_plan).is_err());
}

#[test]
fn service_reject_and_bootstrap_overlap_fail_closed() {
    let mut input = input();
    input.policy.dns_policy.as_mut().unwrap().service_owner =
        crate::user_config::mesh_route_state::MeshDnsOwnerTarget::Reject;
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let built = emit_managed_mesh_config(&legacy(), &input, &plan).unwrap();
    assert_eq!(
        first_match(
            &built.config.route.as_ref().unwrap().rules,
            MAGIC_DNS_SERVICE_CIDRS[1],
            "tcp",
            53
        )
        .action
        .as_deref(),
        Some("reject")
    );

    let mut config = legacy();
    config.route.as_mut().unwrap().rules[2]
        .ip_cidr
        .as_mut()
        .unwrap()
        .push("100.80.2.3/32".into());
    assert!(emit_managed_mesh_config(&config, &input, &plan)
        .unwrap_err()
        .contains("overlaps"));
}

#[test]
fn scoped_override_is_and_of_original_matcher_and_protected_scope() {
    let mut input = input();
    input.policy.overrides.push(MeshOverride {
        rule_id: "corp-only".into(),
        scope_cidrs: vec!["100.80.2.0/24".into()],
        target: MeshTarget::Reject,
    });
    input.scopeable_rule_matchers.insert(
        "corp-only".into(),
        RouteRule {
            domain_suffix: Some(vec!["corp.example.invalid".into()]),
            ..Default::default()
        },
    );
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let built = emit_managed_mesh_config(&legacy(), &input, &plan).unwrap();
    let rules = &built.config.route.as_ref().unwrap().rules;
    let scoped = rules
        .iter()
        .find(|rule| rule.type_field.as_deref() == Some("logical"))
        .unwrap();
    assert_eq!(scoped.mode.as_deref(), Some("and"));
    assert_eq!(scoped.action.as_deref(), Some("reject"));
    assert_eq!(
        scoped.rules.as_ref().unwrap()[0].domain_suffix.as_deref(),
        Some(["corp.example.invalid".into()].as_slice())
    );
    assert_eq!(
        scoped.rules.as_ref().unwrap()[1].ip_cidr.as_deref(),
        Some(["100.80.2.0/24".into()].as_slice())
    );
    let first_owner = rules
        .iter()
        .position(|rule| rule.outbound.as_deref() == Some("ep-a"))
        .unwrap();
    let override_index = rules
        .iter()
        .position(|rule| rule.type_field.as_deref() == Some("logical"))
        .unwrap();
    assert!(override_index < first_owner);
}

#[test]
fn ambiguous_tag_and_unmanaged_scoped_override_block_emission() {
    let input = input();
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let mut duplicate = legacy();
    let extra = duplicate.endpoints.as_ref().unwrap()[0].clone();
    duplicate.endpoints.as_mut().unwrap().push(extra);
    assert!(emit_managed_mesh_config(&duplicate, &input, &plan)
        .unwrap_err()
        .contains("ambiguous"));

    let mut input = input;
    input.policy.overrides.push(MeshOverride {
        rule_id: "release-by-name".into(),
        scope_cidrs: vec!["100.80.2.0/24".into()],
        target: MeshTarget::Unmanaged,
    });
    input.scopeable_rule_matchers.insert(
        "release-by-name".into(),
        RouteRule {
            domain_suffix: Some(vec!["corp.example.invalid".into()]),
            ..Default::default()
        },
    );
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    assert!(emit_managed_mesh_config(&legacy(), &input, &plan)
        .unwrap_err()
        .contains("unmanaged scoped override"));
}

#[test]
fn fixed_b609_core_accepts_managed_dns_and_route_shape() {
    let repo_core =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/linux/sing-box");
    let core = std::env::var_os("POLARIS_TEST_CORE")
        .map(std::path::PathBuf::from)
        .unwrap_or(repo_core);
    if !core.is_file() {
        assert_ne!(
            std::env::var("POLARIS_REQUIRE_KERNEL_GATE").as_deref(),
            Ok("1")
        );
        eprintln!("fixed b609 core absent; set POLARIS_TEST_CORE or fetch package core");
        return;
    }
    let version = std::process::Command::new(&core)
        .arg("version")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&version.stdout)
        .contains("b609f959f57ce34416c51c7b87ce4a76f2e1df56"));
    let mut input = input();
    input.policy.overrides.push(MeshOverride {
        rule_id: "corp-only".into(),
        scope_cidrs: vec!["100.80.2.0/24".into()],
        target: MeshTarget::Reject,
    });
    input.scopeable_rule_matchers.insert(
        "corp-only".into(),
        RouteRule {
            domain_suffix: Some(vec!["corp.example.invalid".into()]),
            ..Default::default()
        },
    );
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let mut source = legacy();
    source.route.as_mut().unwrap().rules.splice(
        1..1,
        crate::builder::subscription_guard::subscription_update_route_rules("direct"),
    );
    let built = emit_managed_mesh_config(&source, &input, &plan).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("managed-dns.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&built.config).unwrap()).unwrap();
    let result = std::process::Command::new(&core)
        .args(["--disable-color", "check", "-c"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "fixed core rejected emitted config: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let formatted = std::process::Command::new(&core)
        .args(["format", "-c"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(formatted.status.success());
    let wire: Value = serde_json::from_slice(&formatted.stdout).unwrap();
    assert!(wire["route"]["rules"]
        .as_array()
        .unwrap()
        .iter()
        .any(|rule| {
            rule["action"] == "resolve"
                && rule["invert"] == true
                && rule["inbound"] == "subscription-update-in"
        }));
}
