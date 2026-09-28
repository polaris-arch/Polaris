use super::*;
use serde_json::{json, Value};

fn fixture() -> (MeshDnsPolicy, MeshRouteState) {
    let raw: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "crates/config-engine/fixtures/mesh-dns-policy.json"
    ))
    .unwrap();
    (
        serde_json::from_value(raw["policy"].clone()).unwrap(),
        serde_json::from_value(raw["state"].clone()).unwrap(),
    )
}

fn legacy_dns() -> DnsConfig {
    serde_json::from_value(json!({
        "servers": [
            {"tag":"dns-mdns","type":"mdns"},
            {"tag":"dns-lan","type":"local"},
            {"tag":"dns-remote","type":"local"},
            {"tag":"dns-tailscale","type":"tailscale","endpoint":"alpha-tag","accept_search_domain":true,"accept_default_resolvers":true}
        ],
        "rules": [
            {"domain_suffix":["local","254.169.in-addr.arpa"],"server":"dns-mdns"},
            {"domain_suffix":["lan","home.arpa","internal"],"domain_regex":["^[^.]+\\.?$"],"server":"dns-lan"},
            {"preferred_by":["dns-tailscale"],"action":"route","server":"dns-tailscale"},
            {"query_type":["A","AAAA"],"server":"dns-remote"}
        ],
        "final":"dns-remote"
    }))
    .unwrap()
}

fn emitted() -> Vec<MeshDnsEmittedEndpoint> {
    vec![
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
    ]
}

fn build(policy: &MeshDnsPolicy, state: &MeshRouteState) -> MeshDnsBuild {
    build_mesh_dns_overlay(&legacy_dns(), Some(policy), Some(state), &emitted()).unwrap()
}

#[test]
fn absent_policy_is_exact_legacy_dns_even_without_ledger() {
    let original = legacy_dns();
    let built = build_mesh_dns_overlay(&original, None, None, &[]).unwrap();
    assert_eq!(built.mode, MeshDnsMode::LegacyUnmanaged);
    assert_eq!(built.dns, original);
    assert!(built.service.is_none());
    assert!(built.short_name.is_none());
    assert!(built.limitations.is_empty());
}

#[test]
fn managed_overlay_finds_legacy_anchor_after_probe_prefix() {
    let (policy, state) = fixture();
    let mut legacy = legacy_dns();
    let prefix: DnsRule = serde_json::from_value(json!({
        "inbound":["probe-proxy-in"],
        "query_type":["A","AAAA"],
        "action":"route",
        "server":"dns-remote",
        "disable_cache":true
    }))
    .unwrap();
    legacy.rules.as_mut().unwrap().insert(0, prefix.clone());
    let built = build_mesh_dns_overlay(&legacy, Some(&policy), Some(&state), &emitted()).unwrap();
    let rules = built.dns.rules.as_ref().unwrap();
    assert_eq!(rules[0], prefix);
    assert_eq!(rules[1].server.as_deref(), Some("dns-mdns"));
    assert_ne!(rules[2].server.as_deref(), Some("dns-lan"));
    assert!(rules
        .iter()
        .any(|rule| rule.server.as_deref() == Some("dns-lan")));

    let duplicate = legacy.rules.as_ref().unwrap()[1].clone();
    legacy.rules.as_mut().unwrap().push(duplicate);
    assert_eq!(
        build_mesh_dns_overlay(&legacy, Some(&policy), Some(&state), &emitted()).unwrap_err(),
        MeshDnsBuildError::LegacyDnsShapeChanged
    );

    let mut broad_prefix = legacy_dns();
    broad_prefix.rules.as_mut().unwrap().insert(
        0,
        serde_json::from_value(json!({"query_type":["A","AAAA"],"server":"dns-remote"})).unwrap(),
    );
    assert_eq!(
        build_mesh_dns_overlay(&broad_prefix, Some(&policy), Some(&state), &emitted()).unwrap_err(),
        MeshDnsBuildError::LegacyDnsShapeChanged
    );
}

#[test]
fn explicit_owners_emit_one_resolver_per_endpoint_without_global_preferred_by() {
    let (policy, state) = fixture();
    let built = build(&policy, &state);
    assert_eq!(built.mode, MeshDnsMode::Managed);
    let resolvers: Vec<_> = built
        .dns
        .servers
        .iter()
        .filter(|server| server.type_field.as_deref() == Some("tailscale"))
        .collect();
    assert_eq!(resolvers.len(), 2);
    let a = resolvers
        .iter()
        .find(|server| server.endpoint.as_deref() == Some("alpha-tag"))
        .unwrap();
    let b = resolvers
        .iter()
        .find(|server| server.endpoint.as_deref() == Some("beta-tag"))
        .unwrap();
    assert_eq!(a.accept_search_domain, Some(true));
    assert_eq!(b.accept_search_domain, Some(false));
    assert!(resolvers
        .iter()
        .all(|server| server.accept_default_resolvers == Some(false)));
    assert!(built.dns.rules.as_ref().unwrap().iter().all(|rule| {
        !rule
            .preferred_by
            .as_ref()
            .is_some_and(|tags| tags.iter().any(|tag| tag == "dns-tailscale"))
    }));
    let suffix_rule = built
        .dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .find(|rule| {
            rule.domain_suffix
                .as_ref()
                .is_some_and(|names| names == &["corp.ts.net"])
        })
        .unwrap();
    let short_rule = built
        .dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .find(|rule| {
            rule.domain_regex
                .as_ref()
                .is_some_and(|names| names == &[SHORT_NAME_REGEX])
        })
        .unwrap();
    assert_eq!(suffix_rule.server.as_deref(), Some(a.tag.as_str()));
    assert_eq!(short_rule.server.as_deref(), Some(a.tag.as_str()));
    assert_eq!(
        built.service,
        Some(MeshDnsTargetDecision::Owner {
            owner: MeshOwnerRef {
                server_id: "beta".into(),
                identity_epoch: "epoch-b".into()
            },
            endpoint_tag: "beta-tag".into(),
        })
    );
    assert_eq!(
        MAGIC_DNS_SERVICE_CIDRS,
        ["100.100.100.100/32", "fd7a:115c:a1e0::53/128"]
    );
    assert_eq!(MAGIC_DNS_SERVICE_PORT, 53);
    assert_eq!(MAGIC_DNS_SERVICE_TRANSPORTS, ["tcp", "udp"]);
    assert_eq!(
        built.limitations,
        vec![
            MeshDnsLimitation::SameIpNeedsFlowContext,
            MeshDnsLimitation::NativeUpstreamMayDialDirect
        ]
    );
}

#[test]
fn candidate_rejects_precede_broader_owner_and_mdns_precedes_them_all() {
    let (policy, state) = fixture();
    let built = build(&policy, &state);
    let rules = built.dns.rules.as_ref().unwrap();
    assert_eq!(rules[0].server.as_deref(), Some("dns-mdns"));
    let suffixes: Vec<_> = rules[1..5]
        .iter()
        .map(|rule| rule.domain_suffix.as_ref().unwrap()[0].as_str())
        .collect();
    assert_eq!(
        suffixes,
        [
            "nested.corp.ts.net",
            "orphan.corp.ts.net",
            "corp.ts.net",
            "lab.ts.net"
        ]
    );
    assert_eq!(rules[2].action.as_deref(), Some("reject"));
    assert_eq!(rules.last().unwrap().server.as_deref(), Some("dns-remote"));
    assert!(rules.iter().all(|rule| !rule
        .domain_suffix
        .as_ref()
        .is_some_and(|names| names.iter().any(|name| name == "public.net"))));
}

#[test]
fn retired_or_missing_owner_and_endpoint_reject_without_other_mesh_fallback() {
    let (policy, mut state) = fixture();
    state.identities[1].binding_state = MeshBindingState::Retired;
    let built = build(&policy, &state);
    assert!(built
        .suffixes
        .iter()
        .filter(|(suffix, _)| suffix == "lab.ts.net" || suffix == "nested.corp.ts.net")
        .all(|(_, decision)| *decision
            == MeshDnsTargetDecision::Reject(MeshDnsRejectReason::OwnerNotBound)));
    assert_eq!(
        built.service,
        Some(MeshDnsTargetDecision::Reject(
            MeshDnsRejectReason::OwnerNotBound
        ))
    );
    assert_eq!(
        built
            .dns
            .servers
            .iter()
            .filter(|server| server.type_field.as_deref() == Some("tailscale"))
            .count(),
        1
    );

    state.identities.remove(1);
    let missing = build(&policy, &state);
    assert_eq!(
        missing.service,
        Some(MeshDnsTargetDecision::Reject(
            MeshDnsRejectReason::OwnerMissing
        ))
    );

    let (_, state) = fixture();
    let mut endpoints = emitted();
    endpoints.retain(|endpoint| endpoint.tag != "beta-tag");
    let absent_endpoint =
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&state), &endpoints).unwrap();
    assert_eq!(
        absent_endpoint.service,
        Some(MeshDnsTargetDecision::Reject(
            MeshDnsRejectReason::EndpointMissing
        ))
    );
    assert!(absent_endpoint
        .dns
        .rules
        .as_ref()
        .unwrap()
        .iter()
        .any(|rule| rule
            .domain_suffix
            .as_ref()
            .is_some_and(|names| names == &["lab.ts.net"])
            && rule.action.as_deref() == Some("reject")));
}

#[test]
fn short_name_system_keeps_lan_chain_and_reject_is_explicit() {
    let (mut policy, state) = fixture();
    policy.short_name_policy = MeshDnsShortNamePolicy::System;
    policy.service_owner = MeshDnsOwnerTarget::Reject;
    let system = build(&policy, &state);
    assert_eq!(system.short_name, Some(MeshDnsShortNameDecision::System));
    assert_eq!(
        system.service,
        Some(MeshDnsTargetDecision::Reject(MeshDnsRejectReason::Explicit))
    );
    assert_eq!(
        system
            .dns
            .rules
            .as_ref()
            .unwrap()
            .iter()
            .filter(|rule| rule
                .domain_regex
                .as_ref()
                .is_some_and(|names| names == &[SHORT_NAME_REGEX]))
            .count(),
        1
    );
    policy.short_name_policy = MeshDnsShortNamePolicy::Reject;
    let rejected = build(&policy, &state);
    assert_eq!(
        rejected.dns.rules.as_ref().unwrap()[5].action.as_deref(),
        Some("reject")
    );
}

#[test]
fn unproven_binding_duplicate_canonical_suffix_and_local_override_fail_closed() {
    let (mut policy, state) = fixture();
    policy.suffix_assignments[0].target = MeshDnsOwnerTarget::Owner {
        server_id: "beta".into(),
        identity_epoch: "epoch-b".into(),
    };
    let unproven = build(&policy, &state);
    assert_eq!(
        unproven
            .suffixes
            .iter()
            .find(|(suffix, _)| suffix == "corp.ts.net")
            .unwrap()
            .1,
        MeshDnsTargetDecision::Reject(MeshDnsRejectReason::CandidateUnproven)
    );
    policy
        .suffix_assignments
        .push(policy.suffix_assignments[0].clone());
    policy.suffix_assignments.last_mut().unwrap().suffix = "corp.ts.net".into();
    let endpoints = emitted();
    assert_eq!(
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&state), &endpoints),
        Err(MeshDnsBuildError::DuplicateSuffix("corp.ts.net".into()))
    );
    policy.suffix_assignments.pop();
    policy.suffix_assignments[0].suffix = "printer.local".into();
    assert_eq!(
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&state), &endpoints),
        Err(MeshDnsBuildError::InvalidSuffix("printer.local".into()))
    );
    policy.suffix_assignments[0].suffix = "corp.ts.net..".into();
    assert_eq!(
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&state), &endpoints),
        Err(MeshDnsBuildError::InvalidSuffix("corp.ts.net..".into()))
    );
}

#[test]
fn shared_candidate_requires_explicit_owner_and_existing_second_resolver_is_rejected() {
    let (policy, mut state) = fixture();
    state.observations[1]
        .magic_dns_suffixes
        .push("corp.ts.net".into());
    let chosen = build(&policy, &state);
    let corp = chosen
        .suffixes
        .iter()
        .find(|(suffix, _)| suffix == "corp.ts.net")
        .unwrap();
    assert!(matches!(
        &corp.1,
        MeshDnsTargetDecision::Owner {
            owner,
            ..
        } if owner.server_id == "alpha"
    ));

    let endpoints = emitted();
    let mut legacy = legacy_dns();
    let mut conflicting = legacy
        .servers
        .iter()
        .find(|server| server.tag == "dns-tailscale")
        .unwrap()
        .clone();
    conflicting.tag = "other-tailnet-dns".into();
    legacy.servers.push(conflicting);
    assert_eq!(
        build_mesh_dns_overlay(&legacy, Some(&policy), Some(&state), &endpoints),
        Err(MeshDnsBuildError::ConflictingTailscaleResolver)
    );
}

#[test]
fn endpoint_from_new_epoch_cannot_satisfy_old_bound_owner() {
    let (policy, state) = fixture();
    let mut endpoints = emitted();
    endpoints[0].owner.identity_epoch = "new-epoch-a".into();
    let built =
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&state), &endpoints).unwrap();
    assert_eq!(
        built
            .suffixes
            .iter()
            .find(|(suffix, _)| suffix == "corp.ts.net")
            .unwrap()
            .1,
        MeshDnsTargetDecision::Reject(MeshDnsRejectReason::EndpointMissing)
    );
    assert_eq!(
        built.short_name,
        Some(MeshDnsShortNameDecision::Target(
            MeshDnsTargetDecision::Reject(MeshDnsRejectReason::EndpointMissing)
        ))
    );
    assert!(matches!(
        built.service,
        Some(MeshDnsTargetDecision::Owner { .. })
    ));

    endpoints.push(MeshDnsEmittedEndpoint {
        owner: MeshOwnerRef {
            server_id: "alpha".into(),
            identity_epoch: "epoch-a".into(),
        },
        tag: "other-alpha-tag".into(),
    });
    assert_eq!(
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&state), &endpoints),
        Err(MeshDnsBuildError::ConflictingEmittedEndpoint)
    );
}

#[test]
fn managed_policy_requires_ledger_and_does_not_fall_back() {
    let (policy, state) = fixture();
    let endpoints = emitted();
    assert_eq!(
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), None, &endpoints),
        Err(MeshDnsBuildError::MissingState)
    );
    let mut invalid = state;
    invalid.local_id.clear();
    assert!(matches!(
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&invalid), &endpoints),
        Err(MeshDnsBuildError::InvalidState(_))
    ));
}

#[test]
fn oversized_history_is_an_error_not_a_truncated_reject_scope() {
    let (policy, mut state) = fixture();
    state.observations[0].magic_dns_suffixes = (0..=MAX_SUFFIXES)
        .map(|index| format!("node{index}.corp.ts.net"))
        .collect();
    let endpoints = emitted();
    assert_eq!(
        build_mesh_dns_overlay(&legacy_dns(), Some(&policy), Some(&state), &endpoints),
        Err(MeshDnsBuildError::ResourceLimitExceeded)
    );
}

/// Opt-in local integration gate. The caller supplies the fixed b609 Linux
/// binary; no real account, network session, or existing state directory is read.
#[test]
fn fixed_core_accepts_emitted_managed_dns_when_binary_is_supplied() {
    let Ok(binary) = std::env::var("POLARIS_D1_SING_BOX_BINARY") else {
        return;
    };
    let (policy, state) = fixture();
    let built = build(&policy, &state);
    let temporary = tempfile::tempdir().unwrap();
    let config_path = temporary.path().join("managed-dns.json");
    let config = json!({
        "log": {"level":"error"},
        "dns": built.dns,
        "endpoints": [
            {"type":"tailscale","tag":"alpha-tag","state_directory":temporary.path().join("alpha")},
            {"type":"tailscale","tag":"beta-tag","state_directory":temporary.path().join("beta")}
        ],
        "outbounds": [{"type":"direct","tag":"direct"}],
        "route": {"final":"direct","default_domain_resolver":"dns-remote"}
    });
    std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let output = std::process::Command::new(binary)
        .arg("check")
        .arg("-c")
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "fixed-core config check rejected D1 DNS: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
