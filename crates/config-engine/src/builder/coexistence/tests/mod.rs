//! All inputs here are synthetic facts, not production collection or device receipts.

use super::*;
use std::net::Ipv6Addr;

fn known<T>(value: T) -> Fact<T> {
    Fact::Known(value)
}
fn missing<T>(source: &str) -> Fact<T> {
    Fact::Unknown(source.into())
}

fn object() -> ObjectFacts {
    ObjectFacts {
        interface: "foreign0".into(),
        tunnel: known(true),
        virtualization: known(false),
        addresses: known(vec![]),
        routes: known(vec![]),
        policy_rules: known(vec![]),
        stable_identity: known(None),
    }
}

fn address(ip: &str, prefix_len: u8) -> InterfaceAddress {
    InterfaceAddress {
        address: ip.parse().unwrap(),
        prefix_len,
    }
}

fn route(prefix: &str, role: RouteRole) -> RouteFact {
    RouteFact {
        prefix: prefix.into(),
        table: known(Some(254)),
        scope: known(RouteScope::Global),
        role: known(role),
    }
}

fn coverage(prefixes: &[&str]) -> Vec<RouteFact> {
    prefixes
        .iter()
        .map(|p| route(p, RouteRole::CoverageDeclaration))
        .collect()
}

fn policy(priority: u32, table: Option<u32>) -> PolicyRuleFact {
    PolicyRuleFact {
        priority,
        lookup_table: known(table),
        applies_to_object: known(true),
    }
}

fn criteria() -> ConflictCriteria {
    ConflictCriteria {
        fakeip_ranges: vec!["198.18.0.0/15".into()],
        mesh_cidrs: vec![],
        tun_addresses: vec!["172.19.0.1/16".into()],
    }
}

fn run_with(
    obj: &ObjectFacts,
    platform: Platform,
    phase: &Fact<ObservationPhase>,
    own: &Fact<Vec<String>>,
    memory: &Fact<Vec<String>>,
    criteria: &ConflictCriteria,
) -> ClassificationReport {
    classify(&ClassificationInput {
        platform,
        observation_phase: phase,
        own_interfaces: own,
        objects: std::slice::from_ref(obj),
        forced_repair_identities: memory,
        criteria,
    })
    .remove(0)
}

fn run(obj: &ObjectFacts, platform: Platform) -> ClassificationReport {
    run_with(
        obj,
        platform,
        &known(ObservationPhase::BeforePolarisTun),
        &known(vec![]),
        &known(vec![]),
        &criteria(),
    )
}

fn expect(report: &ClassificationReport, shape: ObjectShape, rule: Rule) {
    let relation = match shape {
        ObjectShape::Global | ObjectShape::Scoped => Relation::Follow,
        ObjectShape::Exclusive => Relation::Exclusive,
        ObjectShape::Bystander => Relation::Bystander,
    };
    assert_eq!(
        report.decision,
        known(Some(Classification {
            shape,
            relation,
            rule
        })),
        "{report:?}"
    );
}

fn predicate(report: &ClassificationReport, rule: Rule) -> &Fact<bool> {
    &report
        .predicates
        .iter()
        .find(|(r, _)| *r == rule)
        .unwrap()
        .1
}

fn expect_unknown(report: &ClassificationReport) {
    assert!(
        matches!(&report.decision, Fact::Unknown(reason) if !reason.is_empty()),
        "{report:?}"
    );
}

#[test]
fn governance_table_has_a_positive_example_for_every_row() {
    let mut proxy = object();
    proxy.addresses = known(vec![address("172.18.0.1", 30)]);
    expect(
        &run(&proxy, Platform::Win),
        ObjectShape::Exclusive,
        Rule::OtherTunProxy,
    );
    let mut collision = object();
    collision.routes = known(vec![route("198.18.42.0/24", RouteRole::ResourceClaim)]);
    expect(
        &run(&collision, Platform::Win),
        ObjectShape::Exclusive,
        Rule::AddressCollision,
    );
    let mut remembered = object();
    remembered.stable_identity = known(Some("vpn-config:one".into()));
    let report = run_with(
        &remembered,
        Platform::Win,
        &known(ObservationPhase::BeforePolarisTun),
        &known(vec![]),
        &known(vec!["vpn-config:one".into()]),
        &criteria(),
    );
    expect(
        &report,
        ObjectShape::Exclusive,
        Rule::ForcedRouteRepairHistory,
    );
    let mut full = object();
    full.routes = known(coverage(&["0.0.0.0/1", "128.0.0.0/1"]));
    expect(
        &run(&full, Platform::Linux),
        ObjectShape::Exclusive,
        Rule::EntryCannotBePreserved,
    );
    expect(
        &run(&full, Platform::Win),
        ObjectShape::Global,
        Rule::GlobalCoverage,
    );
    expect(
        &run(&object(), Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    let mut bystander = object();
    bystander.tunnel = known(false);
    bystander.virtualization = known(true);
    expect(
        &run(&bystander, Platform::Win),
        ObjectShape::Bystander,
        Rule::KnownVirtualization,
    );
    bystander.virtualization = known(false);
    assert_eq!(run(&bystander, Platform::Win).decision, known(None));
}

#[test]
fn every_row_has_a_one_condition_neighbor_that_does_not_match_it() {
    let mut obj = object();
    obj.addresses = known(vec![address("172.18.0.2", 30)]);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    obj.addresses = known(vec![]);
    obj.routes = known(vec![route("198.20.42.0/24", RouteRole::ResourceClaim)]);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    obj.stable_identity = known(Some("vpn-config:other".into()));
    let report = run_with(
        &obj,
        Platform::Win,
        &known(ObservationPhase::BeforePolarisTun),
        &known(vec![]),
        &known(vec!["vpn-config:one".into()]),
        &criteria(),
    );
    expect(&report, ObjectShape::Scoped, Rule::TunnelInterface);
    obj = object();
    obj.routes = known(coverage(&["0.0.0.0/1", "128.0.0.0/1"]));
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Global,
        Rule::GlobalCoverage,
    );
    obj.routes = known(coverage(&["10.0.0.0/8"]));
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    obj = object();
    obj.tunnel = known(false);
    assert_eq!(run(&obj, Platform::Win).decision, known(None));
    obj.virtualization = known(true);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Bystander,
        Rule::KnownVirtualization,
    );
    obj.virtualization = known(false);
    assert_eq!(run(&obj, Platform::Win).decision, known(None));
}

#[test]
fn first_true_rule_wins_and_an_earlier_unknown_blocks_later_certainty() {
    let mut obj = object();
    obj.routes = known(coverage(&["0.0.0.0/1", "128.0.0.0/1"]));
    obj.addresses = known(vec![address("172.19.0.1", 30)]);
    obj.stable_identity = known(Some("vpn".into()));
    let memory = known(vec!["vpn".into()]);
    let evaluate = |obj: &ObjectFacts| {
        run_with(
            obj,
            Platform::Linux,
            &known(ObservationPhase::BeforePolarisTun),
            &known(vec![]),
            &memory,
            &criteria(),
        )
    };
    expect(&evaluate(&obj), ObjectShape::Exclusive, Rule::OtherTunProxy);
    obj.addresses = known(vec![address("198.18.42.1", 24)]);
    expect(
        &evaluate(&obj),
        ObjectShape::Exclusive,
        Rule::AddressCollision,
    );
    obj.addresses = known(vec![]);
    expect(
        &evaluate(&obj),
        ObjectShape::Exclusive,
        Rule::ForcedRouteRepairHistory,
    );
    obj.stable_identity = known(None);
    expect(
        &evaluate(&obj),
        ObjectShape::Exclusive,
        Rule::EntryCannotBePreserved,
    );
    obj.addresses = missing("addresses were not collected");
    let report = evaluate(&obj);
    expect_unknown(&report);
    assert_eq!(
        predicate(&report, Rule::EntryCannotBePreserved),
        &known(true)
    );
    assert!(
        matches!(&report.decision, Fact::Unknown(reason) if reason.starts_with("OtherTunProxy:"))
    );
}

#[test]
fn own_tun_and_mesh_interfaces_are_excluded_and_missing_ownership_is_unknown() {
    let mut obj = object();
    obj.addresses = known(vec![address("172.19.0.1", 30)]);
    for name in ["own-tun", "own-mesh"] {
        obj.interface = name.into();
        let report = run_with(
            &obj,
            Platform::Win,
            &known(ObservationPhase::Runtime),
            &known(vec![name.into()]),
            &known(vec![]),
            &criteria(),
        );
        assert!(report.excluded_self);
        assert_eq!(report.decision, known(None));
        assert_eq!(predicate(&report, Rule::OtherTunProxy), &known(false));
    }
    let report = run_with(
        &obj,
        Platform::Win,
        &known(ObservationPhase::Runtime),
        &missing("own mesh list unavailable"),
        &known(vec![]),
        &criteria(),
    );
    expect_unknown(&report);
    assert!(!report.excluded_self);
    assert!(matches!(
        predicate(&report, Rule::OtherTunProxy),
        Fact::Unknown(_)
    ));
    obj.interface.clear();
    expect_unknown(&run(&obj, Platform::Win));
}

#[test]
fn ipv4_signatures_preserve_exact_host_and_prefix() {
    for (ip, bits) in [("198.18.0.1", 16), ("172.18.0.1", 30), ("172.19.0.1", 30)] {
        let mut obj = object();
        obj.addresses = known(vec![address(ip, bits)]);
        expect(
            &run(&obj, Platform::Win),
            ObjectShape::Exclusive,
            Rule::OtherTunProxy,
        );
        obj.addresses = known(vec![address(ip, bits - 1)]);
        assert_eq!(
            predicate(&run(&obj, Platform::Win), Rule::OtherTunProxy),
            &known(false)
        );
    }
    for ip in ["198.18.0.0", "198.18.0.2", "172.18.0.0", "172.19.0.2"] {
        let mut obj = object();
        obj.addresses = known(vec![address(ip, 30)]);
        assert_eq!(
            predicate(&run(&obj, Platform::Win), Rule::OtherTunProxy),
            &known(false)
        );
    }
}

#[test]
fn ipv6_signature_uses_actual_host_and_has_exact_range_boundaries() {
    for ip in [
        "fdfe:dcba:9876::",
        "fdfe:dcba:9876::1",
        "fdfe:dcba:9876::2",
        "fdfe:dcba:9876::3",
    ] {
        let mut obj = object();
        obj.addresses = known(vec![address(ip, 64)]);
        expect(
            &run(&obj, Platform::Win),
            ObjectShape::Exclusive,
            Rule::OtherTunProxy,
        );
    }
    let mut obj = object();
    obj.addresses = known(vec![address("fdfe:dcba:9876::4", 128)]);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    obj.addresses = known(vec![address("fdfe:dcba:9876::1", 129)]);
    let report = run(&obj, Platform::Win);
    expect_unknown(&report);
    assert!(!report.diagnostics.is_empty());
}

#[test]
fn linux_proxy_rules_require_before_tun_and_proven_object_applicability() {
    for (priority, table, hit) in [
        (8999, Some(100), false),
        (9000, Some(100), true),
        (9010, Some(100), true),
        (9011, Some(100), false),
        (32000, Some(2022), true),
        (32000, Some(2023), false),
    ] {
        let mut obj = object();
        obj.policy_rules = known(vec![policy(priority, table)]);
        assert_eq!(
            predicate(&run(&obj, Platform::Linux), Rule::OtherTunProxy),
            &known(hit)
        );
    }
    let mut obj = object();
    obj.policy_rules = known(vec![policy(9000, Some(2022))]);
    for phase in [known(ObservationPhase::Runtime), missing("phase absent")] {
        let report = run_with(
            &obj,
            Platform::Linux,
            &phase,
            &known(vec![]),
            &known(vec![]),
            &criteria(),
        );
        expect_unknown(&report);
        assert!(matches!(
            predicate(&report, Rule::OtherTunProxy),
            Fact::Unknown(_)
        ));
    }
    if let Fact::Known(rules) = &mut obj.policy_rules {
        rules[0].applies_to_object = known(false);
    }
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    if let Fact::Known(rules) = &mut obj.policy_rules {
        rules[0].applies_to_object = missing("unattributed table");
    }
    expect_unknown(&run(&obj, Platform::Linux));
    obj.policy_rules = missing("no platform rule source");
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
}

const MAC_V4: [&str; 8] = [
    "1.0.0.0/8",
    "2.0.0.0/7",
    "4.0.0.0/6",
    "8.0.0.0/5",
    "16.0.0.0/4",
    "32.0.0.0/3",
    "64.0.0.0/2",
    "128.0.0.0/1",
];
const MAC_V6: [&str; 8] = [
    "100::/8", "200::/7", "400::/6", "800::/5", "1000::/4", "2000::/3", "4000::/2", "8000::/1",
];

#[test]
fn mac_proxy_signature_counts_distinct_exact_segments_per_family() {
    for n in [0, 3, 4, 8] {
        let mut obj = object();
        obj.routes = known(
            MAC_V4[..n]
                .iter()
                .map(|p| route(p, RouteRole::ResourceClaim))
                .collect(),
        );
        assert_eq!(
            predicate(&run(&obj, Platform::Mac), Rule::OtherTunProxy),
            &known(n >= 4)
        );
    }
    let mut obj = object();
    obj.routes = known(
        MAC_V4[..3]
            .iter()
            .chain([&MAC_V4[0], &MAC_V4[1]])
            .map(|p| route(p, RouteRole::ResourceClaim))
            .collect(),
    );
    assert_eq!(
        predicate(&run(&obj, Platform::Mac), Rule::OtherTunProxy),
        &known(false)
    );
    obj.routes = known(
        MAC_V4[..3]
            .iter()
            .chain(MAC_V6[..3].iter())
            .map(|p| route(p, RouteRole::ResourceClaim))
            .collect(),
    );
    assert_eq!(
        predicate(&run(&obj, Platform::Mac), Rule::OtherTunProxy),
        &known(false)
    );
    obj.routes = known(
        MAC_V6[..4]
            .iter()
            .map(|p| route(p, RouteRole::ResourceClaim))
            .collect(),
    );
    expect(
        &run(&obj, Platform::Mac),
        ObjectShape::Exclusive,
        Rule::OtherTunProxy,
    );
    obj.routes = known(
        ["1.0.0.0/9", "2.0.0.0/7", "4.0.0.0/6", "8.0.0.0/5"]
            .iter()
            .map(|p| route(p, RouteRole::ResourceClaim))
            .collect(),
    );
    assert_eq!(
        predicate(&run(&obj, Platform::Mac), Rule::OtherTunProxy),
        &known(false)
    );
}

#[test]
fn full_global_coverage_is_proved_per_family_without_resources_false_positives() {
    for prefixes in [
        &["0.0.0.0/0"][..],
        &["0.0.0.0/1", "128.0.0.0/1"],
        &["0.0.0.0/2", "64.0.0.0/2", "128.0.0.0/2", "192.0.0.0/2"],
        &["128.0.0.0/1", "0.0.0.0/1", "128.0.0.0/1", "10.0.0.0/8"],
    ] {
        let mut obj = object();
        obj.routes = known(coverage(prefixes));
        let report = run(&obj, Platform::Win);
        expect(&report, ObjectShape::Global, Rule::GlobalCoverage);
        assert_eq!(
            report.coverage,
            Coverage {
                ipv4: known(true),
                ipv6: known(false)
            }
        );
        assert_eq!(predicate(&report, Rule::AddressCollision), &known(false));
    }
    for prefixes in [&["::/0"][..], &["::/1", "8000::/1"]] {
        let mut obj = object();
        obj.routes = known(coverage(prefixes));
        let report = run(&obj, Platform::Win);
        expect(&report, ObjectShape::Global, Rule::GlobalCoverage);
        assert_eq!(
            report.coverage,
            Coverage {
                ipv4: known(false),
                ipv6: known(true)
            }
        );
    }
}

#[test]
fn incomplete_or_scoped_coverage_cannot_impersonate_global_coverage() {
    for prefixes in [
        &["0.0.0.0/1"][..],
        &["0.0.0.0/1", "128.0.0.0/2"],
        &MAC_V4,
        &["0.0.0.0/1", "8000::/1"],
    ] {
        let mut obj = object();
        obj.routes = known(coverage(prefixes));
        let report = run(&obj, Platform::Win);
        assert_eq!(report.coverage.ipv4, known(false));
        assert_eq!(report.coverage.ipv6, known(false));
        assert_ne!(predicate(&report, Rule::GlobalCoverage), &known(true));
    }
    let mut obj = object();
    let mut scoped = route("0.0.0.0/0", RouteRole::CoverageDeclaration);
    scoped.scope = known(RouteScope::InterfaceScoped);
    obj.routes = known(vec![scoped]);
    let report = run(&obj, Platform::Win);
    expect(&report, ObjectShape::Scoped, Rule::TunnelInterface);
    assert_eq!(report.coverage.ipv4, known(false));
    if let Fact::Known(routes) = &mut obj.routes {
        routes[0].scope = missing("flags dropped");
    }
    expect_unknown(&run(&obj, Platform::Win));
}

#[test]
fn coverage_never_combines_two_object_records() {
    let mut a = object();
    a.routes = known(coverage(&["0.0.0.0/1"]));
    let mut b = object();
    b.interface = "foreign1".into();
    b.routes = known(coverage(&["128.0.0.0/1"]));
    let phase = known(ObservationPhase::BeforePolarisTun);
    let own = known(vec![]);
    let memory = known(vec![]);
    let criteria = criteria();
    let mut objects = [a, b];
    let reports = classify(&ClassificationInput {
        platform: Platform::Win,
        observation_phase: &phase,
        own_interfaces: &own,
        objects: &objects,
        forced_repair_identities: &memory,
        criteria: &criteria,
    });
    assert_eq!(reports.len(), 2);
    assert!(reports.iter().all(|r| r.coverage.ipv4 == known(false)));
    assert_eq!(reports[0].interface, "foreign0");
    assert_eq!(reports[1].interface, "foreign1");
    objects[0].routes = known(coverage(&MAC_V4[..3]));
    objects[1].routes = known(coverage(&MAC_V4[3..6]));
    let reports = classify(&ClassificationInput {
        platform: Platform::Mac,
        observation_phase: &phase,
        own_interfaces: &own,
        objects: &objects,
        forced_repair_identities: &memory,
        criteria: &criteria,
    });
    assert!(reports
        .iter()
        .all(|r| predicate(r, Rule::OtherTunProxy) == &known(false)));
}

#[test]
fn a_single_missing_ipv6_endpoint_is_a_real_hole_and_maximum_does_not_overflow() {
    // Independent decomposition of all IPv6 addresses except MAX; adding that host closes it.
    let mut prefixes = Vec::new();
    for bits in 1..=128u8 {
        let low = if bits == 1 {
            u128::MAX
        } else {
            (1u128 << (129 - bits)) - 1
        };
        prefixes.push(format!("{}/{bits}", Ipv6Addr::from(u128::MAX ^ low)));
    }
    let mut obj = object();
    obj.routes = known(
        prefixes
            .iter()
            .map(|p| route(p, RouteRole::CoverageDeclaration))
            .collect(),
    );
    assert_eq!(run(&obj, Platform::Win).coverage.ipv6, known(false));
    if let Fact::Known(routes) = &mut obj.routes {
        routes.push(route(
            "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128",
            RouteRole::CoverageDeclaration,
        ));
    }
    let report = run(&obj, Platform::Win);
    assert_eq!(report.coverage.ipv6, known(true));
    expect(&report, ObjectShape::Global, Rule::GlobalCoverage);
    obj.routes = known(coverage(&[
        "255.255.255.255/32",
        "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff/128",
    ]));
    assert_eq!(
        run(&obj, Platform::Win).coverage,
        Coverage {
            ipv4: known(false),
            ipv6: known(false)
        }
    );
}

#[test]
fn resource_routes_and_addresses_survive_complete_coverage_declarations() {
    let mut obj = object();
    obj.routes = known(coverage(&["0.0.0.0/1", "128.0.0.0/1"]));
    if let Fact::Known(routes) = &mut obj.routes {
        routes.push(route("198.18.42.0/24", RouteRole::ResourceClaim));
    }
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Exclusive,
        Rule::AddressCollision,
    );
    obj.routes = known(coverage(&["0.0.0.0/0"]));
    obj.addresses = known(vec![address("198.18.42.1", 24)]);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Exclusive,
        Rule::AddressCollision,
    );
    obj.addresses = known(vec![address("172.19.0.2", 30)]);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Exclusive,
        Rule::AddressCollision,
    );
    obj.addresses = known(vec![]);
    if let Fact::Known(routes) = &mut obj.routes {
        routes.push(RouteFact {
            role: missing("unresolved extra declaration"),
            ..route("198.18.42.0/24", RouteRole::ResourceClaim)
        });
    }
    let report = run(&obj, Platform::Win);
    expect_unknown(&report);
    assert_eq!(report.coverage.ipv4, known(true));
    obj.routes = known(coverage(&["128.0.0.0/1"]));
    expect_unknown(&run(&obj, Platform::Win));
}

#[test]
fn linux_entry_failure_uses_main_split_or_applicable_early_policy_with_full_coverage() {
    let mut obj = object();
    obj.routes = known(coverage(&["0.0.0.0/1", "128.0.0.0/1"]));
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Exclusive,
        Rule::EntryCannotBePreserved,
    );
    if let Fact::Known(routes) = &mut obj.routes {
        for r in routes {
            r.table = known(Some(100));
        }
    }
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Global,
        Rule::GlobalCoverage,
    );
    obj.policy_rules = known(vec![policy(8999, Some(100))]);
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Exclusive,
        Rule::EntryCannotBePreserved,
    );
    obj.policy_rules = known(vec![policy(9011, Some(100))]);
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Global,
        Rule::GlobalCoverage,
    );
    obj.policy_rules = known(vec![policy(8999, Some(100))]);
    if let Fact::Known(rules) = &mut obj.policy_rules {
        rules[0].applies_to_object = known(false);
    }
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Global,
        Rule::GlobalCoverage,
    );
    obj.routes = known(coverage(&["0.0.0.0/0"]));
    obj.policy_rules = known(vec![]);
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Global,
        Rule::GlobalCoverage,
    );
    obj.routes = known(coverage(&["10.0.0.0/8"]));
    obj.policy_rules = known(vec![policy(8999, Some(100))]);
    expect(
        &run(&obj, Platform::Linux),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    obj.routes = known(coverage(&["0.0.0.0/1", "128.0.0.0/1"]));
    obj.policy_rules = known(vec![]);
    if let Fact::Known(routes) = &mut obj.routes {
        for r in routes {
            r.table = missing("table number not retained");
        }
    }
    expect_unknown(&run(&obj, Platform::Linux));
    let mut no_tun = criteria();
    no_tun.tun_addresses.clear();
    expect(
        &run_with(
            &obj,
            Platform::Linux,
            &known(ObservationPhase::BeforePolarisTun),
            &known(vec![]),
            &known(vec![]),
            &no_tun,
        ),
        ObjectShape::Global,
        Rule::GlobalCoverage,
    );
}

#[test]
fn unknown_identity_history_is_distinct_from_no_verifiable_key_and_no_match() {
    let mut obj = object();
    let phase = known(ObservationPhase::BeforePolarisTun);
    let own = known(vec![]);
    let failed = missing("history read failed");
    let report = run_with(&obj, Platform::Win, &phase, &own, &failed, &criteria());
    expect(&report, ObjectShape::Scoped, Rule::TunnelInterface);
    assert_eq!(report.history, HistoryAssessment::NoVerifiableIdentity);
    obj.stable_identity = known(Some("stable:1".into()));
    let report = run_with(&obj, Platform::Win, &phase, &own, &failed, &criteria());
    expect_unknown(&report);
    assert!(matches!(report.history, HistoryAssessment::Unknown(_)));
    let report = run_with(
        &obj,
        Platform::Win,
        &phase,
        &own,
        &known(vec!["stable:1".into()]),
        &criteria(),
    );
    expect(
        &report,
        ObjectShape::Exclusive,
        Rule::ForcedRouteRepairHistory,
    );
    obj.stable_identity = known(Some("stable:2".into()));
    let report = run_with(
        &obj,
        Platform::Win,
        &phase,
        &own,
        &known(vec!["stable:1".into()]),
        &criteria(),
    );
    expect(&report, ObjectShape::Scoped, Rule::TunnelInterface);
    assert_eq!(report.history, HistoryAssessment::NotMatched);
    obj.stable_identity = missing("key unavailable");
    expect_unknown(&run(&obj, Platform::Win));
    obj.stable_identity = known(Some(" ".into()));
    expect_unknown(&run(&obj, Platform::Win));
}

#[test]
fn virtualization_requires_known_non_tunnel_and_proven_identity() {
    let mut obj = object();
    obj.virtualization = known(true);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    obj.tunnel = known(false);
    expect(
        &run(&obj, Platform::Win),
        ObjectShape::Bystander,
        Rule::KnownVirtualization,
    );
    obj.virtualization = known(false);
    assert_eq!(run(&obj, Platform::Win).decision, known(None));
    obj.virtualization = missing("type 6 does not identify virtualization");
    expect_unknown(&run(&obj, Platform::Win));
    obj.tunnel = missing("tunnel kind unavailable");
    obj.virtualization = known(true);
    expect_unknown(&run(&obj, Platform::Win));
}

#[test]
fn empty_success_and_failed_collection_are_not_the_same_fact() {
    let base = object();
    expect(
        &run(&base, Platform::Win),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
    for source in ["not probed", "unsupported", "probe failed"] {
        let mut obj = base.clone();
        obj.addresses = missing(source);
        expect_unknown(&run(&obj, Platform::Win));
        obj = base.clone();
        obj.routes = missing(source);
        expect_unknown(&run(&obj, Platform::Win));
        obj = base.clone();
        obj.policy_rules = missing(source);
        expect_unknown(&run(&obj, Platform::Linux));
    }
}

#[test]
fn malformed_cidrs_are_visible_and_never_silently_become_empty_routes() {
    for prefix in [
        "",
        " ",
        "bad",
        "010.0.0.1/8",
        "0.0.0.0/33",
        "::/129",
        "::/-1",
        "::/+1",
        "::/",
        "::/1/2",
    ] {
        let mut obj = object();
        let mut routes = coverage(&["0.0.0.0/0"]);
        routes.push(route(prefix, RouteRole::ResourceClaim));
        obj.routes = known(routes);
        let report = run(&obj, Platform::Win);
        expect_unknown(&report);
        assert!(!report.diagnostics.is_empty(), "{prefix:?}");
    }
    let mut bad_criteria = criteria();
    bad_criteria.fakeip_ranges.push("invalid".into());
    let report = run_with(
        &object(),
        Platform::Win,
        &known(ObservationPhase::BeforePolarisTun),
        &known(vec![]),
        &known(vec![]),
        &bad_criteria,
    );
    expect_unknown(&report);
    assert!(!report.diagnostics.is_empty());
    let mut obj = object();
    obj.addresses = known(vec![address("172.18.0.1", 30)]);
    obj.routes = known(vec![route("invalid", RouteRole::ResourceClaim)]);
    let report = run(&obj, Platform::Win);
    expect(&report, ObjectShape::Exclusive, Rule::OtherTunProxy);
    assert!(!report.diagnostics.is_empty());
}

#[test]
fn mesh_overlap_is_not_promoted_into_a_new_exclusive_category() {
    let mut obj = object();
    obj.routes = known(vec![route("100.64.0.0/10", RouteRole::ResourceClaim)]);
    let mut criteria = criteria();
    criteria.mesh_cidrs.push("100.64.0.0/10".into());
    expect(
        &run_with(
            &obj,
            Platform::Win,
            &known(ObservationPhase::BeforePolarisTun),
            &known(vec![]),
            &known(vec![]),
            &criteria,
        ),
        ObjectShape::Scoped,
        Rule::TunnelInterface,
    );
}
