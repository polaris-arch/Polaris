use super::*;
use std::net::Ipv4Addr;

fn claim(id: &str, leg: ForceRouteLeg, cidrs: Option<&[&str]>) -> MeshRouteCandidate {
    MeshRouteCandidate {
        server_id: id.into(),
        tag: format!("endpoint-{id}"),
        leg,
        generated: true,
        generation_reason: None,
        engaged_reason: "alwaysRouteSubnets".into(),
        configured_cidrs: vec![],
        observed_hosts: vec![],
        referenced_file: None,
        match_cidrs: cidrs.map(|xs| {
            xs.iter()
                .map(|x| SourcedCidr {
                    cidr: (*x).into(),
                    source: MeshRouteSource::Declared,
                })
                .collect()
        }),
        unknown_reasons: vec![],
    }
}

fn snapshot(candidates: Vec<MeshRouteCandidate>) -> MeshRouteSnapshot {
    MeshRouteSnapshot {
        scope: MeshRouteScope::Preview,
        config_source: MeshRouteConfigSource::Draft,
        config_version: Some("test-version".into()),
        run_generation: None,
        sampled_at_ms: 1,
        load_evidence: MeshRouteLoadEvidence::Unknown,
        snapshot_stale: false,
        dns_owner_server_id: None,
        preceding_exceptions: vec!["customRuleMayOverride".into()],
        candidates,
    }
}

// 独立 oracle：逐地址按原始规则顺序 first-match，不调用 CIDR 差集/包含实现。
fn first_match_v4(candidates: &[MeshRouteCandidate], addr: Ipv4Addr) -> Option<&str> {
    let ip = u32::from(addr);
    for candidate in candidates {
        if !candidate.generated {
            continue;
        }
        for entry in candidate.match_cidrs.as_ref()? {
            let (raw, bits) = entry.cidr.split_once('/')?;
            let bits: u32 = bits.parse().ok()?;
            let network = u32::from(raw.parse::<Ipv4Addr>().ok()?);
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            if ip & mask == network & mask {
                return Some(&candidate.server_id);
            }
        }
    }
    None
}

#[test]
fn nested_and_partial_overlap_match_independent_first_match() {
    let candidates = vec![
        claim("a", ForceRouteLeg::ExternalRuleSet, Some(&["10.20.0.1/16"])),
        claim(
            "b",
            ForceRouteLeg::Inline,
            Some(&["10.20.1.99/24", "10.21.0.0/16"]),
        ),
    ];
    let report = resolve_mesh_route_snapshot(snapshot(candidates.clone()));
    assert_eq!(report.results[0].requested[0].cidr, "10.20.0.0/16");
    assert_eq!(report.results[1].coverage, MeshRouteCoverage::Partial);
    assert_eq!(
        report.results[1].effective,
        Some(vec!["10.21.0.0/16".into()])
    );
    assert_eq!(report.results[1].blocked_by[0].server_id, "a");
    assert_eq!(report.results[1].blocked_by[0].cidr, "10.20.1.0/24");
    for (ip, expected) in [("10.20.1.5", "a"), ("10.20.9.1", "a"), ("10.21.1.1", "b")] {
        let ip = ip.parse().unwrap();
        assert_eq!(first_match_v4(&candidates, ip), Some(expected));
        let resolved_owner = report.results.iter().find(|r| {
            r.effective.as_ref().is_some_and(|xs| {
                xs.iter().any(|x| {
                    let (network, bits) = x.split_once('/').unwrap();
                    let bits: u32 = bits.parse().unwrap();
                    let mask = if bits == 0 {
                        0
                    } else {
                        u32::MAX << (32 - bits)
                    };
                    u32::from(ip) & mask == u32::from(network.parse::<Ipv4Addr>().unwrap()) & mask
                })
            })
        });
        assert_eq!(resolved_owner.map(|r| r.server_id.as_str()), Some(expected));
    }
}

#[test]
fn external_and_inline_have_identical_conflict_semantics() {
    for legs in [
        [ForceRouteLeg::ExternalRuleSet, ForceRouteLeg::Inline],
        [ForceRouteLeg::Inline, ForceRouteLeg::ExternalRuleSet],
        [
            ForceRouteLeg::ExternalRuleSet,
            ForceRouteLeg::ExternalRuleSet,
        ],
    ] {
        let report = resolve_mesh_route_snapshot(snapshot(vec![
            claim("a", legs[0], Some(&["fd7a:115c:a1e0:0::1/64"])),
            claim("b", legs[1], Some(&["fd7a:115c:a1e0::2/128"])),
        ]));
        assert_eq!(report.results[1].coverage, MeshRouteCoverage::None);
        assert_eq!(report.results[1].effective, Some(vec![]));
        assert_eq!(
            report.results[1].blocked_by[0].cidr,
            "fd7a:115c:a1e0::2/128"
        );
    }
}

#[test]
fn narrower_first_carves_later_wider_range_and_matches_every_small_address() {
    let candidates = vec![
        claim("a", ForceRouteLeg::Inline, Some(&["10.20.1.9/24"])),
        claim("b", ForceRouteLeg::ExternalRuleSet, Some(&["10.20.9.9/16"])),
    ];
    let report = resolve_mesh_route_snapshot(snapshot(candidates.clone()));
    assert_eq!(
        report.results[0].effective,
        Some(vec!["10.20.1.0/24".into()])
    );
    assert_eq!(report.results[1].coverage, MeshRouteCoverage::Partial);
    assert_eq!(report.results[1].blocked_by[0].cidr, "10.20.1.0/24");
    for third in 0..=255_u8 {
        for fourth in 0..=255_u8 {
            let ip = Ipv4Addr::new(10, 20, third, fourth);
            let expected = if third == 1 { "a" } else { "b" };
            assert_eq!(first_match_v4(&candidates, ip), Some(expected));
            let resolved = report.results.iter().find(|result| {
                result.effective.as_ref().is_some_and(|ranges| {
                    ranges.iter().any(|range| {
                        let (network, bits) = range.split_once('/').unwrap();
                        let bits: u32 = bits.parse().unwrap();
                        let mask = if bits == 0 {
                            0
                        } else {
                            u32::MAX << (32 - bits)
                        };
                        u32::from(ip) & mask
                            == u32::from(network.parse::<Ipv4Addr>().unwrap()) & mask
                    })
                })
            });
            assert_eq!(
                resolved.map(|r| r.server_id.as_str()),
                Some(expected),
                "{ip}"
            );
        }
    }
}

#[test]
fn equivalent_v4_v6_claims_and_cross_family_separation() {
    let report = resolve_mesh_route_snapshot(snapshot(vec![
        claim(
            "a",
            ForceRouteLeg::Inline,
            Some(&["10.20.1.9/24", "FD7A:115C:A1E0:0::9/48"]),
        ),
        claim(
            "b",
            ForceRouteLeg::ExternalRuleSet,
            Some(&["10.20.1.0/24", "fd7a:115c:a1e0::/48"]),
        ),
        claim("c", ForceRouteLeg::Inline, Some(&["::ffff:10.20.1.5/128"])),
    ]));
    assert_eq!(report.results[1].effective, Some(vec![]));
    assert_eq!(report.results[1].blocked_by.len(), 2);
    assert_eq!(report.results[2].coverage, MeshRouteCoverage::Full);
    assert_eq!(report.results[2].requested[0].cidr, "::ffff:a14:105/128");
}

#[test]
fn cumulative_work_budget_returns_unknown_instead_of_partial_answer() {
    let candidates = (0..160)
        .map(|i| {
            let cidrs: Vec<String> = (1..=10)
                .map(|j| format!("10.{}.{}.{}", i / 256, i % 256, j))
                .collect();
            let refs: Vec<&str> = cidrs.iter().map(String::as_str).collect();
            claim(&format!("s{i}"), ForceRouteLeg::Inline, Some(&refs))
        })
        .collect();
    let report = resolve_mesh_route_snapshot(snapshot(candidates));
    assert!(report.results.iter().any(|r| r
        .unknown_reasons
        .contains(&MeshRouteUnknownReason::ResourceLimitExceeded)));
    let last = report.results.last().unwrap();
    assert_eq!(last.effective, None);
    assert_eq!(last.coverage, MeshRouteCoverage::Unknown);
}

#[test]
fn candidate_limit_marks_whole_report_incomplete() {
    let candidates = (0..=MAX_MESH_ROUTE_REPORT_CANDIDATES)
        .map(|i| {
            claim(
                &format!("s{i}"),
                ForceRouteLeg::Inline,
                Some(&["10.0.0.0/8"]),
            )
        })
        .collect();
    let report = resolve_mesh_route_snapshot(snapshot(candidates));
    assert!(report.results.is_empty());
    assert!(report.snapshot.candidates.is_empty());
    assert_eq!(
        report.total_candidate_count,
        MAX_MESH_ROUTE_REPORT_CANDIDATES + 1
    );
    assert_eq!(
        report.unknown_reasons,
        vec![MeshRouteUnknownReason::ResourceLimitExceeded]
    );
}

#[test]
fn opaque_predecessor_keeps_later_effective_null_without_false_owner() {
    let report = resolve_mesh_route_snapshot(snapshot(vec![
        claim("unknown", ForceRouteLeg::PreferredBy, None),
        claim("b", ForceRouteLeg::Inline, Some(&["10.0.0.0/8"])),
        claim("c", ForceRouteLeg::ExternalRuleSet, Some(&["10.1.0.0/16"])),
    ]));
    assert_eq!(report.results[1].effective, None);
    assert_eq!(report.results[2].effective, None);
    assert!(report.results[2].blocked_by.is_empty());
}

#[test]
fn bad_input_and_catch_all_are_not_reported_as_full_coverage() {
    let report = resolve_mesh_route_snapshot(snapshot(vec![
        claim(
            "a",
            ForceRouteLeg::Inline,
            Some(&["0.0.0.1/0", "010.0.0.1/8", "10.0.0.0/8"]),
        ),
        claim("b", ForceRouteLeg::Inline, Some(&["192.168.0.0/16"])),
    ]));
    assert_eq!(report.results[0].excluded_catch_all, vec!["0.0.0.0/0"]);
    assert_eq!(report.results[0].invalid, vec!["010.0.0.1/8"]);
    assert_eq!(report.results[0].effective, None);
    assert_eq!(report.results[1].coverage, MeshRouteCoverage::Unknown);
}

#[test]
fn failed_generation_does_not_claim_or_poison_following_candidate() {
    let mut failed = claim("a", ForceRouteLeg::Inline, Some(&["10.0.0.0/8"]));
    failed.generated = false;
    failed.generation_reason = Some("endpointBuildFailed".into());
    let report = resolve_mesh_route_snapshot(snapshot(vec![
        failed,
        claim("b", ForceRouteLeg::Inline, Some(&["10.0.0.0/8"])),
    ]));
    assert_eq!(report.results[0].coverage, MeshRouteCoverage::Unknown);
    assert_eq!(report.results[1].coverage, MeshRouteCoverage::Full);
}

#[test]
fn applied_requires_running_generation_and_ack() {
    let mut input = snapshot(vec![claim(
        "a",
        ForceRouteLeg::Inline,
        Some(&["10.0.0.0/8"]),
    )]);
    input.scope = MeshRouteScope::Applied;
    input.config_source = MeshRouteConfigSource::Running;
    input.run_generation = Some(12);
    input.load_evidence = MeshRouteLoadEvidence::FileWrittenUnacknowledged;
    let report = resolve_mesh_route_snapshot(input);
    assert_eq!(report.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(report.results[0].requested[0].cidr, "10.0.0.0/8");
    assert_eq!(report.results[0].effective, None);
    assert!(report.results[0].blocked_by.is_empty());
}

#[test]
fn stale_or_unversioned_running_snapshot_never_confirms_owner() {
    let mut input = snapshot(vec![claim(
        "a",
        ForceRouteLeg::Inline,
        Some(&["10.0.0.0/8"]),
    )]);
    input.scope = MeshRouteScope::Applied;
    input.config_source = MeshRouteConfigSource::Running;
    input.run_generation = Some(12);
    input.load_evidence = MeshRouteLoadEvidence::StartupReady;
    input.snapshot_stale = true;
    let stale = resolve_mesh_route_snapshot(input.clone());
    assert_eq!(stale.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(stale.results[0].effective, None);
    assert_eq!(stale.results[0].coverage, MeshRouteCoverage::Unknown);
    input.snapshot_stale = false;
    input.config_version = None;
    let unversioned = resolve_mesh_route_snapshot(input);
    assert_eq!(unversioned.snapshot.scope, MeshRouteScope::PersistedUnknown);
    assert_eq!(unversioned.results[0].effective, None);
}

#[test]
fn acknowledged_running_snapshot_can_prove_mesh_layer_owner() {
    let mut input = snapshot(vec![claim(
        "a",
        ForceRouteLeg::Inline,
        Some(&["10.0.0.0/8"]),
    )]);
    input.scope = MeshRouteScope::Applied;
    input.config_source = MeshRouteConfigSource::Running;
    input.run_generation = Some(12);
    input.load_evidence = MeshRouteLoadEvidence::StartupReady;
    let report = resolve_mesh_route_snapshot(input);
    assert_eq!(report.snapshot.scope, MeshRouteScope::Applied);
    assert_eq!(report.results[0].effective, Some(vec!["10.0.0.0/8".into()]));
}

#[test]
fn cross_platform_wire_fixture_matches_resolver() {
    let fixture: MeshRouteReport = serde_json::from_str(&polaris_source_probe::repo_file!(
        "ui/src/contracts/mesh-route-report.fixture.json"
    ))
    .unwrap();
    assert_eq!(
        resolve_mesh_route_snapshot(fixture.snapshot.clone()),
        fixture
    );
}
