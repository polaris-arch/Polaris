//! Synthetic iproute2 JSON fixtures only. No host query, network changes or device acceptance.
use super::*;
use crate::exec::CommandOutput;
use polaris_config_engine::builder::coexistence::{
    AddressFamily, PolicySelectorScope, RouteRole, RouteScope, Rule,
};
use serde_json::{json, Value};
use std::cell::RefCell;

struct FixtureRunner {
    outputs: Vec<Result<String, String>>,
    calls: RefCell<Vec<Command>>,
}
impl CommandRunner for FixtureRunner {
    fn run(&self, cmd: &Command, timeout: Duration) -> Result<CommandOutput, String> {
        assert_eq!(timeout, Duration::from_secs(2));
        let index = self.calls.borrow().len();
        self.calls.borrow_mut().push(cmd.clone());
        self.outputs[index].clone().map(|stdout| CommandOutput {
            stdout,
            stderr: String::new(),
        })
    }
}
fn fixture(mut r4: Value, mut r6: Value, p4: Value, p6: Value) -> FixtureRunner {
    // Model the collector's -j -d -N wire shape even for the older routing cases.
    // Explicit textual/malformed types in compatibility/negative cases stay intact.
    for family in [&mut r4, &mut r6] {
        if let Some(rows) = family.as_array_mut() {
            for row in rows.iter_mut().filter_map(Value::as_object_mut) {
                row.entry("type").or_insert(json!("1"));
                row.entry("flags").or_insert(json!([]));
                row.entry("scope").or_insert(json!("0"));
            }
        }
    }
    FixtureRunner {
        outputs: vec![
            Ok(json!([{"ifname":"vpn0","linkinfo":{"info_kind":"tun"}}]).to_string()),
            Ok(json!([{"ifname":"vpn0","addr_info":[{"family":"inet","local":"10.8.0.2","prefixlen":24}]}]).to_string()),
            Ok(r4.to_string()), Ok(r6.to_string()), Ok(p4.to_string()), Ok(p6.to_string()),
        ], calls: RefCell::new(vec![]),
    }
}
fn object(f: &FixtureRunner) -> ObjectFacts {
    let Fact::Known(mut objects) = collect_linux(f).objects else {
        panic!("unknown objects")
    };
    assert_eq!(objects.len(), 1);
    objects.remove(0)
}
fn reports(f: &FixtureRunner) -> Vec<ClassificationReport> {
    let criteria = ConflictCriteria {
        fakeip_ranges: vec!["198.18.0.0/15".into()],
        mesh_cidrs: vec![],
        tun_addresses: vec!["172.19.0.1/16".into()],
    };
    let Fact::Known(reports) = collect_linux(f).classify(
        &Fact::Known(ObservationPhase::BeforePolarisTun),
        &Fact::Known(vec![]),
        &Fact::Known(vec![]),
        &criteria,
    ) else {
        panic!("unknown reports")
    };
    reports
}
fn split(table1: Value, table2: Value) -> Value {
    json!([{"dst":"0.0.0.0/1","dev":"vpn0","table":table1},
        {"dst":"128.0.0.0/1","dev":"vpn0","table":table2}])
}
fn predicate(report: &ClassificationReport, rule: Rule) -> &Fact<bool> {
    &report
        .predicates
        .iter()
        .find(|(r, _)| *r == rule)
        .unwrap()
        .1
}

#[test]
fn collector_uses_exact_read_only_queries_and_preserves_actual_host_bits() {
    let f = fixture(json!([]), json!([]), json!([]), json!([]));
    let obj = object(&f);
    let Fact::Known(addresses) = obj.addresses else {
        panic!()
    };
    assert_eq!(addresses[0].address.to_string(), "10.8.0.2");
    assert_eq!(obj.stable_identity, Fact::Known(None));
    let expected = [
        vec!["-j", "-d", "link", "show"],
        vec!["-j", "address", "show"],
        vec!["-j", "-d", "-N", "-4", "route", "show", "table", "all"],
        vec!["-j", "-d", "-N", "-6", "route", "show", "table", "all"],
        vec!["-j", "-N", "-4", "rule", "show"],
        vec!["-j", "-N", "-6", "rule", "show"],
    ];
    assert_eq!(f.calls.borrow().len(), 6);
    for (actual, args) in f.calls.borrow().iter().zip(expected) {
        assert_eq!(actual, &Command::new("ip", args));
    }
}
#[test]
fn successful_empty_enumeration_is_known_empty() {
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[0] = Ok("[]".into());
    assert_eq!(collect_linux(&f).objects, Fact::Known(vec![]));
    let f = fixture(json!([]), json!([]), json!([]), json!([]));
    let obj = object(&f);
    assert_eq!(obj.routes, Fact::Known(vec![]));
    assert_eq!(obj.policy_rules, Fact::Known(vec![]));
}
#[test]
fn failed_or_invalid_link_enumeration_never_becomes_no_objects() {
    for bad in [
        Err("permission denied".into()),
        Ok("".into()),
        Ok("{}".into()),
        Ok("[{}]".into()),
        Ok("[null]".into()),
    ] {
        let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
        f.outputs[0] = bad;
        let facts = collect_linux(&f);
        assert!(matches!(facts.objects, Fact::Unknown(_)));
        assert!(matches!(
            facts.classify(
                &Fact::Known(ObservationPhase::Runtime),
                &Fact::Known(vec![]),
                &Fact::Known(vec![]),
                &ConflictCriteria {
                    fakeip_ranges: vec![],
                    mesh_cidrs: vec![],
                    tun_addresses: vec![]
                }
            ),
            Fact::Unknown(_)
        ));
    }
}
#[test]
fn per_source_failures_remain_unknown_without_erasing_independent_facts() {
    for index in 1..6 {
        for bad in [
            Err("timeout or permissions failure".into()),
            Ok("broken".into()),
        ] {
            let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
            f.outputs[index] = bad;
            let obj = object(&f);
            assert_eq!(obj.tunnel, Fact::Known(true));
            match index {
                1 => assert!(matches!(obj.addresses, Fact::Unknown(_))),
                2 | 3 => assert!(matches!(obj.routes, Fact::Unknown(_))),
                4 | 5 => assert!(matches!(obj.policy_rules, Fact::Unknown(_))),
                _ => unreachable!(),
            }
        }
    }
}
#[test]
fn incomplete_address_snapshot_is_not_known_empty() {
    for rows in [
        json!([]),
        json!([{"ifname":"vpn0"}]),
        json!([{"ifname":"vpn0","addr_info":[{"family":"inet","local":"::1","prefixlen":24}]}]),
        json!([{"ifname":"vpn0","addr_info":[{"family":"inet","local":"10.8.0.2","prefixlen":33}]}]),
    ] {
        let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
        f.outputs[1] = Ok(rows.to_string());
        assert!(matches!(object(&f).addresses, Fact::Unknown(_)));
    }
}
#[test]
fn candidate_coverage_is_unknown_but_additional_resource_claim_is_independent() {
    let report = reports(&fixture(
        split(json!(254), json!(254)),
        json!([]),
        json!([]),
        json!([]),
    ))
    .remove(0);
    assert!(matches!(
        predicate(&report, Rule::AddressCollision),
        Fact::Unknown(_)
    ));
    assert!(matches!(
        predicate(&report, Rule::EntryCannotBePreserved),
        Fact::Unknown(_)
    ));
    let mut r = split(json!(254), json!(254));
    r.as_array_mut()
        .unwrap()
        .push(json!({"dst":"198.18.42.0/24","dev":"vpn0","table":254}));
    let report = reports(&fixture(r, json!([]), json!([]), json!([]))).remove(0);
    assert_eq!(
        predicate(&report, Rule::AddressCollision),
        &Fact::Known(true)
    );
}
#[test]
fn split_halves_from_different_tables_never_form_coverage() {
    let report = reports(&fixture(
        split(json!(100), json!(200)),
        json!([]),
        json!([]),
        json!([]),
    ))
    .remove(0);
    assert_ne!(report.coverage.ipv4, Fact::Known(true));
    assert_ne!(
        predicate(&report, Rule::EntryCannotBePreserved),
        &Fact::Known(true)
    );
}
#[test]
fn split_halves_from_different_interfaces_never_form_coverage() {
    let mut r = split(json!(254), json!(254));
    r[1]["dev"] = json!("other0");
    let report = reports(&fixture(r, json!([]), json!([]), json!([]))).remove(0);
    assert_ne!(report.coverage.ipv4, Fact::Known(true));
}
#[test]
fn default_route_retains_table_unknown_and_does_not_guess_main() {
    let f = fixture(
        json!([{"dst":"default","dev":"vpn0"}]),
        json!([]),
        json!([]),
        json!([]),
    );
    let Fact::Known(r) = object(&f).routes else {
        panic!()
    };
    assert!(matches!(r[0].table, Fact::Unknown(_)));
    assert!(matches!(r[0].role, Fact::Unknown(_)));
}
#[test]
fn early_global_policy_requires_same_table_family_and_interface() {
    for (table, v6) in [(100, false), (200, false), (100, true)] {
        let policy = json!([{"priority":100,"src":"all","table":table}]);
        let (p4, p6) = if v6 {
            (json!([]), policy)
        } else {
            (policy, json!([]))
        };
        let report = reports(&fixture(split(json!(100), json!(100)), json!([]), p4, p6)).remove(0);
        if table == 100 && !v6 {
            assert!(matches!(
                predicate(&report, Rule::EntryCannotBePreserved),
                Fact::Unknown(_)
            ));
        } else {
            assert_eq!(
                predicate(&report, Rule::EntryCannotBePreserved),
                &Fact::Known(false)
            );
        }
    }
}
#[test]
fn restricted_and_unrecognized_selectors_never_become_global() {
    for (key, value) in [
        ("fwmark", json!("0x1")),
        ("suppress_prefixlen", json!(0)),
        ("uid_start", json!(1000)),
        ("not", Value::Null),
        ("iif", json!("eth0")),
        ("future_selector", json!(true)),
        ("goto", json!(200)),
    ] {
        let mut p = json!({"priority":100,"src":"all","table":100});
        p[key] = value;
        let f = fixture(
            split(json!(100), json!(100)),
            json!([]),
            json!([p]),
            json!([]),
        );
        let Fact::Known(rules) = object(&f).policy_rules else {
            panic!()
        };
        assert_ne!(
            rules[0].selector_scope,
            Fact::Known(PolicySelectorScope::Global)
        );
        assert!(matches!(rules[0].applies_to_object, Fact::Unknown(_)));
        // Fresh runner: no query result is reused across collector calls.
        let f = fixture(
            split(json!(100), json!(100)),
            json!([]),
            json!([p]),
            json!([]),
        );
        assert_ne!(
            predicate(&reports(&f)[0], Rule::EntryCannotBePreserved),
            &Fact::Known(true)
        );
    }
}
#[test]
fn selector_source_lengths_and_missing_sources_are_not_global() {
    for p in [
        json!({"priority":100,"src":"all","srclen":8,"table":100}),
        json!({"priority":100,"src":"10.0.0.0","srclen":8,"table":100}),
        json!({"priority":100,"table":100}),
        json!({"priority":100,"src":null,"table":100}),
    ] {
        let f = fixture(
            split(json!(100), json!(100)),
            json!([]),
            json!([p]),
            json!([]),
        );
        let Fact::Known(rules) = object(&f).policy_rules else {
            panic!()
        };
        assert_ne!(
            rules[0].selector_scope,
            Fact::Known(PolicySelectorScope::Global)
        );
    }
}
#[test]
fn rules_keep_query_family_and_do_not_join_other_family_routes() {
    let f = fixture(
        json!([{"dst":"default","dev":"vpn0","table":100}]),
        json!([]),
        json!([]),
        json!([{"priority":9000,"src":"all","table":100}]),
    );
    let Fact::Known(rules) = object(&f).policy_rules else {
        panic!()
    };
    assert_eq!(rules[0].address_family, Fact::Known(AddressFamily::V6));
    assert_eq!(rules[0].applies_to_object, Fact::Known(false));
}
#[test]
fn ecmp_nhid_malformed_and_wrong_family_routes_remain_unknown() {
    for r in [
        json!([{"dst":"default","nexthops":[{"dev":"vpn0"},{"dev":"other0"}],"table":100}]),
        json!([{"dst":"default","nhid":7,"table":100}]),
        json!([{"dst":"default","table":100}]),
        json!([{"dst":"::/0","dev":"vpn0","table":100}]),
        json!([{"dst":"0.0.0.0/+1","dev":"vpn0","table":100}]),
    ] {
        let f = fixture(r, json!([]), json!([]), json!([]));
        assert!(matches!(object(&f).routes, Fact::Unknown(_)));
    }
}
#[test]
fn ipv6_candidates_preserve_named_builtin_tables_without_selected_coverage() {
    let f = fixture(
        json!([]),
        json!([{"dst":"::/1","dev":"vpn0","table":"main"},
        {"dst":"8000::/1","dev":"vpn0","table":"main"}]),
        json!([]),
        json!([]),
    );
    let Fact::Known(routes) = object(&fixture(
        json!([]),
        json!([{"dst":"::/1","dev":"vpn0","table":"main"},
            {"dst":"8000::/1","dev":"vpn0","table":"main"}]),
        json!([]),
        json!([]),
    ))
    .routes
    else {
        panic!()
    };
    assert_eq!(
        routes.iter().map(|r| r.prefix.as_str()).collect::<Vec<_>>(),
        vec!["::/1", "8000::/1"]
    );
    assert!(routes.iter().all(|r| r.table == Fact::Known(Some(254))
        && r.scope == Fact::Known(RouteScope::Global)
        && matches!(r.role, Fact::Unknown(_))));
    let report = reports(&f).remove(0);
    assert!(matches!(report.coverage.ipv6, Fact::Unknown(_)));
    // IPv6 candidates do not overlap the fixture's IPv4-only protected ranges.
    assert_eq!(
        predicate(&report, Rule::AddressCollision),
        &Fact::Known(false)
    );
    assert!(matches!(
        predicate(&report, Rule::EntryCannotBePreserved),
        Fact::Unknown(_)
    ));
}
#[test]
fn own_attribution_and_phase_are_caller_evidence() {
    let f = fixture(json!([]), json!([]), json!([]), json!([]));
    let facts = collect_linux(&f);
    let criteria = ConflictCriteria {
        fakeip_ranges: vec![],
        mesh_cidrs: vec![],
        tun_addresses: vec![],
    };
    let Fact::Known(reports) = facts.classify(
        &Fact::Unknown("phase unavailable".into()),
        &Fact::Unknown("mesh attribution unavailable".into()),
        &Fact::Known(vec![]),
        &criteria,
    ) else {
        panic!()
    };
    assert!(matches!(reports[0].decision, Fact::Unknown(_)));
    let Fact::Known(reports) = facts.classify(
        &Fact::Known(ObservationPhase::Runtime),
        &Fact::Known(vec!["vpn0".into()]),
        &Fact::Known(vec![]),
        &criteria,
    ) else {
        panic!()
    };
    assert!(reports[0].excluded_self);
    assert_eq!(reports[0].decision, Fact::Known(None));
}
#[test]
fn before_tun_policy_signature_never_borrows_priority_without_object_link() {
    for table in [100, 2022] {
        let f = fixture(
            json!([{"dst":"default","dev":"vpn0","table":100}]),
            json!([]),
            json!([{"priority":9000,"src":"all","table":table}]),
            json!([]),
        );
        let report = reports(&f).remove(0);
        if table == 100 {
            assert!(matches!(
                predicate(&report, Rule::OtherTunProxy),
                Fact::Unknown(_)
            ));
        } else {
            assert_eq!(predicate(&report, Rule::OtherTunProxy), &Fact::Known(false));
        }
    }
}
#[test]
fn duplicate_links_and_unknown_kinds_do_not_fabricate_identity_or_physical_status() {
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[0] = Ok(json!([{"ifname":"vpn0"},{"ifname":"vpn0"}]).to_string());
    assert!(matches!(collect_linux(&f).objects, Fact::Unknown(_)));
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[0] = Ok(json!([{"ifname":"vpn0","link_type":"ether"}]).to_string());
    assert!(matches!(object(&f).tunnel, Fact::Unknown(_)));
}

#[test]
fn arbitrary_complete_split_keeps_nested_business_routes_as_resources() {
    let mut routes = json!([
        {"dst":"0.0.0.0/2","dev":"vpn0","table":254},
        {"dst":"64.0.0.0/2","dev":"vpn0","table":254},
        {"dst":"128.0.0.0/2","dev":"vpn0","table":254},
        {"dst":"192.0.0.0/2","dev":"vpn0","table":254}]);
    let f = fixture(routes.clone(), json!([]), json!([]), json!([]));
    let report = reports(&f).remove(0);
    assert!(matches!(report.coverage.ipv4, Fact::Unknown(_)));
    assert!(matches!(
        predicate(&report, Rule::AddressCollision),
        Fact::Unknown(_)
    ));
    routes
        .as_array_mut()
        .unwrap()
        .push(json!({"dst":"198.18.42.0/24","dev":"vpn0","table":254}));
    let f = fixture(routes, json!([]), json!([]), json!([]));
    assert_eq!(
        predicate(&reports(&f)[0], Rule::AddressCollision),
        &Fact::Known(true)
    );
}
#[test]
fn default_does_not_bless_nested_resource_route_or_merge_duplicate_halves() {
    let f = fixture(
        json!([
        {"dst":"default","dev":"vpn0","table":100},
        {"dst":"198.18.42.0/24","dev":"vpn0","table":100}]),
        json!([]),
        json!([]),
        json!([]),
    );
    assert_eq!(
        predicate(&reports(&f)[0], Rule::AddressCollision),
        &Fact::Known(true)
    );
    let mut r = split(json!(254), json!(254));
    r.as_array_mut()
        .unwrap()
        .push(json!({"dst":"128.0.0.0/1","dev":"vpn0","table":254}));
    let f = fixture(r, json!([]), json!([]), json!([]));
    assert!(matches!(
        predicate(&reports(&f)[0], Rule::AddressCollision),
        Fact::Unknown(_)
    ));
}

#[test]
fn unsupported_route_applicability_and_bad_types_are_not_known_coverage() {
    for extra in [
        json!({"src":"2001:db8::/32"}),
        json!({"flags":["linkdown"]}),
        json!({"type":42}),
        json!({"type":"future-type"}),
    ] {
        let mut route = json!({"dst":"default","dev":"vpn0","table":254});
        route
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let f = fixture(json!([route]), json!([]), json!([]), json!([]));
        let report = reports(&f).remove(0);
        assert!(matches!(report.coverage.ipv4, Fact::Unknown(_)));
        assert_ne!(
            predicate(&report, Rule::EntryCannotBePreserved),
            &Fact::Known(true)
        );
    }
}
#[test]
fn runtime_evidence_does_not_claim_a_before_tun_proxy_signature() {
    let f = fixture(
        json!([{"dst":"default","dev":"vpn0","table":2022}]),
        json!([]),
        json!([{"priority":9000,"src":"all","table":2022}]),
        json!([]),
    );
    let facts = collect_linux(&f);
    let Fact::Known(reports) = facts.classify(
        &Fact::Known(ObservationPhase::Runtime),
        &Fact::Known(vec![]),
        &Fact::Known(vec![]),
        &ConflictCriteria {
            fakeip_ranges: vec![],
            mesh_cidrs: vec![],
            tun_addresses: vec!["172.19.0.1/16".into()],
        },
    ) else {
        panic!()
    };
    assert!(matches!(
        predicate(&reports[0], Rule::OtherTunProxy),
        Fact::Unknown(_)
    ));
}

// Review regressions: synthetic rows shaped by the exact -j -d -N argv.
// Numeric strings are emitted by iproute2 rtm_map.c when numeric is enabled.
#[test]
fn detailed_numeric_unicast_routes_are_parsed_for_both_families() {
    let f = fixture(
        json!([{"type":"1","dst":"default","gateway":"10.8.0.1", "dev":"vpn0",
        "table":"100","protocol":"186","scope":"0","metric":100,"flags":[]}]),
        json!([{"type":"1","dst":"default","gateway":"2001:db8::1","dev":"vpn0",
        "table":"100","protocol":"186","scope":"0","metric":100,"flags":[]}]),
        json!([]),
        json!([]),
    );
    let obj = object(&f);
    let Fact::Known(routes) = obj.routes else {
        panic!("-N RTN_UNICAST strings must not make routes Unknown")
    };
    assert_eq!(routes.len(), 2);
    assert!(routes.iter().all(|r| r.table == Fact::Known(Some(100))
        && r.scope == Fact::Known(RouteScope::Global)
        && matches!(r.role, Fact::Unknown(_))));
}
#[test]
fn detailed_numeric_local_and_terminal_routes_match_textual_forms() {
    for (numeric, textual) in [
        ("1", "unicast"),
        ("2", "local"),
        ("3", "broadcast"),
        ("6", "blackhole"),
        ("7", "unreachable"),
        ("8", "prohibit"),
        ("9", "throw"),
    ] {
        let input = |kind| {
            json!([{"type":kind,"dst":"192.0.2.0/24","dev":"vpn0",
            "table":"100","protocol":"186","scope":"0","flags":[]}])
        };
        let a = object(&fixture(input(numeric), json!([]), json!([]), json!([]))).routes;
        let b = object(&fixture(input(textual), json!([]), json!([]), json!([]))).routes;
        assert_eq!(a, b, "numeric type {numeric} must agree with {textual}");
        assert!(matches!(a, Fact::Known(_)));
    }
}
fn competition(priority: u32) -> FixtureRunner {
    let mut f = fixture(
        json!([
            {"type":"unicast","dst":"default","dev":"eth0","table":100,"metric":10,"flags":[]},
            {"type":"unicast","dst":"default","dev":"vpn0","table":100,"metric":100,"flags":[]}
        ]),
        json!([]),
        json!([{"priority":priority,"src":"all","table":100}]),
        json!([]),
    );
    f.outputs[0] = Ok(json!([{"ifname":"eth0","link_type":"ether"},
        {"ifname":"vpn0","linkinfo":{"info_kind":"tun"}}])
    .to_string());
    f.outputs[1] = Ok(json!([
        {"ifname":"eth0","addr_info":[{"family":"inet","local":"192.0.2.10","prefixlen":24}]},
        {"ifname":"vpn0","addr_info":[{"family":"inet","local":"10.8.0.2","prefixlen":24}]}
    ])
    .to_string());
    f
}
fn assert_unproved_selection(priority: u32, rule: Rule) {
    let f = competition(priority);
    let Fact::Known(objects) = collect_linux(&f).objects else {
        panic!()
    };
    let vpn = objects.iter().find(|o| o.interface == "vpn0").unwrap();
    let Fact::Known(rules) = &vpn.policy_rules else {
        panic!()
    };
    let report = reports(&competition(priority))
        .into_iter()
        .find(|r| r.interface == "vpn0")
        .unwrap();
    println!("competing defaults eth0 metric10/vpn0 metric100: priority={priority}, applies={:?}, predicate={:?}, decision={:?}",
        rules[0].applies_to_object,predicate(&report,rule),report.decision);
    assert!(
        matches!(rules[0].applies_to_object, Fact::Unknown(_)),
        "table membership is not object route selection"
    );
    assert!(matches!(predicate(&report, rule), Fact::Unknown(_)));
    assert!(matches!(report.decision, Fact::Unknown(_)));
}
#[test]
fn competing_default_does_not_establish_proxy_signature() {
    assert_unproved_selection(9000, Rule::OtherTunProxy);
}
#[test]
fn competing_default_does_not_establish_early_policy_entry_loss() {
    assert_unproved_selection(100, Rule::EntryCannotBePreserved);
}
#[test]
fn enumeration_limits_fail_closed_without_truncation() {
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[0] = Ok(json!((0..129)
        .map(|n| json!({"ifname":format!("if{n}")}))
        .collect::<Vec<_>>())
    .to_string());
    assert!(
        matches!(collect_linux(&f).objects, Fact::Unknown(_)),
        "link row cap must reject whole enumeration"
    );
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[2] = Ok(format!("[]{}", " ".repeat(1024 * 1024)));
    assert!(
        matches!(object(&f).routes, Fact::Unknown(_)),
        "source byte cap must reject whole enumeration"
    );
    let routes = json!((0..4097)
        .map(|_| json!({"type":"blackhole","dst":"192.0.2.0/24"}))
        .collect::<Vec<_>>());
    assert!(
        matches!(
            object(&fixture(routes, json!([]), json!([]), json!([]))).routes,
            Fact::Unknown(_)
        ),
        "route row cap must reject whole enumeration"
    );
    let policies = json!((0..257)
        .map(|n| json!({"priority":n,"src":"all","table":100}))
        .collect::<Vec<_>>());
    assert!(
        matches!(
            object(&fixture(json!([]), json!([]), policies, json!([]))).policy_rules,
            Fact::Unknown(_)
        ),
        "rule row cap must reject whole enumeration"
    );
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[1] = Ok(json!([{"ifname":"vpn0","addr_info":(0..257).map(|_|json!({"family":"inet","local":"10.8.0.2","prefixlen":24})).collect::<Vec<_>>()}]).to_string());
    assert!(
        matches!(object(&f).addresses, Fact::Unknown(_)),
        "nested address cap must reject whole object addresses"
    );
}

#[test]
fn numeric_terminal_routes_without_dev_are_empty_but_unattributed_unicast_is_unknown() {
    for numeric in ["6", "7", "8", "9"] {
        let route = json!([{"type":numeric,"dst":"192.0.2.0/24","table":"100","protocol":"4","scope":"0","flags":[]}]);
        assert_eq!(
            object(&fixture(route, json!([]), json!([]), json!([]))).routes,
            Fact::Known(vec![])
        );
    }
    for numeric in ["1", "2", "3", "0", "4", "5", "10", "11", "999"] {
        let route = json!([{"type":numeric,"dst":"192.0.2.0/24","table":"100","protocol":"4","scope":"0","flags":[]}]);
        assert!(
            matches!(
                object(&fixture(route, json!([]), json!([]), json!([]))).routes,
                Fact::Unknown(_)
            ),
            "unattributed/unsupported RTN {numeric}"
        );
    }
    for numeric in ["0", "4", "5", "10", "11", "999"] {
        let route =
            json!([{"type":numeric,"dst":"192.0.2.0/24","dev":"vpn0","table":"100","flags":[]}]);
        assert!(matches!(
            object(&fixture(route, json!([]), json!([]), json!([]))).routes,
            Fact::Unknown(_)
        ));
    }
}
#[test]
fn numeric_route_competition_stays_unknown_after_type_parsing_succeeds() {
    let mut f = competition(9000);
    let mut routes: Value = serde_json::from_str(f.outputs[2].as_ref().unwrap()).unwrap();
    for row in routes.as_array_mut().unwrap() {
        row["type"] = json!("1");
        row["table"] = json!("100");
    }
    f.outputs[2] = Ok(routes.to_string());
    let report = reports(&f)
        .into_iter()
        .find(|r| r.interface == "vpn0")
        .unwrap();
    assert!(matches!(report.coverage.ipv4, Fact::Unknown(_)));
    assert!(matches!(
        predicate(&report, Rule::OtherTunProxy),
        Fact::Unknown(_)
    ));
    assert!(matches!(report.decision, Fact::Unknown(_)));
}
#[test]
fn source_byte_limit_applies_to_all_six_outputs() {
    for source in 0..6 {
        let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
        f.outputs[source] = Ok(format!("[]{}", " ".repeat(MAX_SOURCE_BYTES)));
        let facts = collect_linux(&f);
        if source == 0 {
            let Fact::Unknown(reason) = facts.objects else {
                panic!("oversize links accepted")
            };
            assert!(reason.contains("byte limit exceeded"));
        } else {
            let Fact::Known(objects) = facts.objects else {
                panic!()
            };
            let object = &objects[0];
            let reason = match source {
                1 => match &object.addresses {
                    Fact::Unknown(r) => r,
                    _ => panic!(),
                },
                2 | 3 => match &object.routes {
                    Fact::Unknown(r) => r,
                    _ => panic!(),
                },
                4 | 5 => match &object.policy_rules {
                    Fact::Unknown(r) => r,
                    _ => panic!(),
                },
                _ => unreachable!(),
            };
            assert!(reason.contains("byte limit exceeded"));
        }
    }
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[2] = Ok(format!("[]{}", " ".repeat(MAX_SOURCE_BYTES - 2)));
    assert_eq!(object(&f).routes, Fact::Known(vec![]));
}
#[test]
fn row_and_nested_address_limit_boundaries_are_inclusive() {
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[0] = Ok(json!((0..MAX_INTERFACE_ROWS)
        .map(|n| json!({"ifname":format!("if{n}")}))
        .collect::<Vec<_>>())
    .to_string());
    let Fact::Known(objects) = collect_linux(&f).objects else {
        panic!("interface boundary rejected")
    };
    assert_eq!(objects.len(), MAX_INTERFACE_ROWS);
    let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
    f.outputs[1] = Ok(json!((0..MAX_INTERFACE_ROWS + 1)
        .map(|n| json!({"ifname":format!("if{n}"),"addr_info":[]}))
        .collect::<Vec<_>>())
    .to_string());
    let Fact::Unknown(reason) = object(&f).addresses else {
        panic!("address rows above cap accepted")
    };
    assert!(reason.contains("row limit exceeded"));
    for (count, accepted) in [(MAX_ROUTE_ROWS, true), (MAX_ROUTE_ROWS + 1, false)] {
        let routes = json!((0..count)
            .map(|_| json!({"type":"6","dst":"192.0.2.0/24"}))
            .collect::<Vec<_>>());
        let obj = object(&fixture(routes, json!([]), json!([]), json!([])));
        assert_eq!(matches!(obj.routes, Fact::Known(_)), accepted);
    }
    for (count, accepted) in [(MAX_POLICY_ROWS, true), (MAX_POLICY_ROWS + 1, false)] {
        let rules = json!((0..count)
            .map(|n| json!({"priority":n,"src":"all","table":"100"}))
            .collect::<Vec<_>>());
        let obj = object(&fixture(json!([]), json!([]), json!([]), rules));
        assert_eq!(matches!(obj.policy_rules, Fact::Known(_)), accepted);
    }
    for (count, accepted) in [
        (MAX_INTERFACE_ADDRESSES, true),
        (MAX_INTERFACE_ADDRESSES + 1, false),
    ] {
        let mut f = fixture(json!([]), json!([]), json!([]), json!([]));
        f.outputs[1] = Ok(json!([{"ifname":"vpn0","addr_info":(0..count).map(|_|json!({"family":"inet","local":"10.8.0.2","prefixlen":24})).collect::<Vec<_>>()}]).to_string());
        assert_eq!(matches!(object(&f).addresses, Fact::Known(_)), accepted);
    }
}
fn main_split_competition(v6: bool, competing_table: u32) -> FixtureRunner {
    let prefixes = if v6 {
        ["::/1", "8000::/1"]
    } else {
        ["0.0.0.0/1", "128.0.0.0/1"]
    };
    let route_rows = json!([
        {"type":"1","dst":prefixes[0],"dev":"eth0","table":competing_table.to_string(),"metric":10,"scope":"0","flags":[]},
        {"type":"1","dst":prefixes[1],"dev":"eth0","table":competing_table.to_string(),"metric":10,"scope":"0","flags":[]},
        {"type":"1","dst":prefixes[0],"dev":"vpn0","table":"254","metric":100,"scope":"0","flags":[]},
        {"type":"1","dst":prefixes[1],"dev":"vpn0","table":"254","metric":100,"scope":"0","flags":[]}]);
    let policy = json!([{"priority":32766,"src":"all","table":"254"}]);
    let mut f = competition(32766);
    f.outputs[2] = Ok(if v6 { json!([]) } else { route_rows.clone() }.to_string());
    f.outputs[3] = Ok(if v6 { route_rows } else { json!([]) }.to_string());
    f.outputs[4] = Ok(if v6 { json!([]) } else { policy.clone() }.to_string());
    f.outputs[5] = Ok(if v6 { policy } else { json!([]) }.to_string());
    f
}
fn assert_main_split_competition_unknown(v6: bool) {
    let Fact::Known(objects) = collect_linux(&main_split_competition(v6, 254)).objects else {
        panic!()
    };
    let vpn = objects.iter().find(|o| o.interface == "vpn0").unwrap();
    let Fact::Known(routes) = &vpn.routes else {
        panic!()
    };
    assert_eq!(routes.len(), 2);
    assert!(routes.iter().all(|r| r.table == Fact::Known(Some(254))
        && r.scope == Fact::Known(RouteScope::Global)
        && matches!(r.role, Fact::Unknown(_))));
    let report = reports(&main_split_competition(v6, 254))
        .into_iter()
        .find(|r| r.interface == "vpn0")
        .unwrap();
    let coverage = if v6 {
        &report.coverage.ipv6
    } else {
        &report.coverage.ipv4
    };
    println!(
        "main split competition v6={v6}, coverage={coverage:?}, entry={:?}, decision={:?}",
        predicate(&report, Rule::EntryCannotBePreserved),
        report.decision
    );
    assert!(
        matches!(coverage, Fact::Unknown(_)),
        "per-interface coverage is not a selected-path witness"
    );
    assert!(matches!(
        predicate(&report, Rule::EntryCannotBePreserved),
        Fact::Unknown(_)
    ));
    assert!(matches!(report.decision, Fact::Unknown(_)));
}
#[test]
fn main_split_competing_v4_coverage_does_not_establish_entry_loss() {
    assert_main_split_competition_unknown(false);
}
#[test]
fn main_split_competing_v6_coverage_does_not_establish_entry_loss() {
    assert_main_split_competition_unknown(true);
}

#[test]
fn main_split_needs_reachability_proof_even_without_same_table_competition() {
    for v6 in [false, true] {
        let report = reports(&main_split_competition(v6, 100))
            .into_iter()
            .find(|r| r.interface == "vpn0")
            .unwrap();
        let coverage = if v6 {
            &report.coverage.ipv6
        } else {
            &report.coverage.ipv4
        };
        assert!(matches!(coverage, Fact::Unknown(_)));
        assert!(matches!(
            predicate(&report, Rule::EntryCannotBePreserved),
            Fact::Unknown(_)
        ));
    }
}
#[test]
fn main_split_needs_reachability_proof_even_without_same_family_competition() {
    let mut f = main_split_competition(false, 254);
    f.outputs[2] = Ok(json!([
        {"type":"1","dst":"0.0.0.0/1","dev":"vpn0","table":"254","metric":100,"scope":"0","flags":[]},
        {"type":"1","dst":"128.0.0.0/1","dev":"vpn0","table":"254","metric":100,"scope":"0","flags":[]}]).to_string());
    f.outputs[3] = Ok(json!([
        {"type":"1","dst":"::/1","dev":"eth0","table":"254","metric":10,"scope":"0","flags":[]},
        {"type":"1","dst":"8000::/1","dev":"eth0","table":"254","metric":10,"scope":"0","flags":[]}]).to_string());
    let report = reports(&f)
        .into_iter()
        .find(|r| r.interface == "vpn0")
        .unwrap();
    assert!(matches!(report.coverage.ipv4, Fact::Unknown(_)));
    assert!(matches!(
        predicate(&report, Rule::EntryCannotBePreserved),
        Fact::Unknown(_)
    ));
}
#[test]
fn same_table_terminal_path_or_unknown_table_path_leaves_coverage_unproved() {
    for extra in [
        json!({"type":"6","dst":"192.0.2.0/24","table":"254","metric":1,"flags":[]}),
        json!({"type":"1","dst":"default","dev":"eth0","table":"unresolved-name","metric":1,"flags":[]}),
    ] {
        let mut f = main_split_competition(false, 100);
        let mut rows: Value = serde_json::from_str(f.outputs[2].as_ref().unwrap()).unwrap();
        rows.as_array_mut().unwrap().push(extra);
        f.outputs[2] = Ok(rows.to_string());
        let report = reports(&f)
            .into_iter()
            .find(|r| r.interface == "vpn0")
            .unwrap();
        assert!(matches!(report.coverage.ipv4, Fact::Unknown(_)));
        assert!(matches!(
            predicate(&report, Rule::EntryCannotBePreserved),
            Fact::Unknown(_)
        ));
        assert!(matches!(report.decision, Fact::Unknown(_)));
    }
}
fn cross_table_shadow(v6: bool) -> FixtureRunner {
    let mut f = main_split_competition(v6, 100);
    let prefixes = if v6 {
        ["::/1", "8000::/1"]
    } else {
        ["0.0.0.0/1", "128.0.0.0/1"]
    };
    let rows = json!([
        {"type":"1","dst":"default","dev":"eth0","table":"100","metric":10,"scope":"0","flags":[]},
        {"type":"1","dst":prefixes[0],"dev":"vpn0","table":"254","metric":100,"scope":"0","flags":[]},
        {"type":"1","dst":prefixes[1],"dev":"vpn0","table":"254","metric":100,"scope":"0","flags":[]}]);
    let policy = json!([{"priority":100,"src":"all","table":"100"},
        {"priority":32766,"src":"all","table":"254"}]);
    f.outputs[if v6 { 3 } else { 2 }] = Ok(rows.to_string());
    f.outputs[if v6 { 5 } else { 4 }] = Ok(policy.to_string());
    f
}
fn assert_cross_table_shadow_unknown(v6: bool) {
    let report = reports(&cross_table_shadow(v6))
        .into_iter()
        .find(|r| r.interface == "vpn0")
        .unwrap();
    let coverage = if v6 {
        &report.coverage.ipv6
    } else {
        &report.coverage.ipv4
    };
    println!("RPDB table100 default shadows main254 vpn0 split: v6={v6}, coverage={coverage:?}, entry={:?}, decision={:?}",
        predicate(&report,Rule::EntryCannotBePreserved),report.decision);
    assert!(matches!(coverage, Fact::Unknown(_)));
    assert!(matches!(
        predicate(&report, Rule::EntryCannotBePreserved),
        Fact::Unknown(_)
    ));
    assert!(matches!(report.decision, Fact::Unknown(_)));
}
#[test]
fn rpdb_cross_table_v4_shadow_does_not_establish_selected_coverage() {
    assert_cross_table_shadow_unknown(false);
}
#[test]
fn rpdb_cross_table_v6_shadow_does_not_establish_selected_coverage() {
    assert_cross_table_shadow_unknown(true);
}

#[test]
fn coverage_candidates_stay_unknown_when_policy_is_unreadable_or_unrecognized() {
    for v6 in [false, true] {
        for policy in [
            Err("permission denied".into()),
            Ok("not json".into()),
            Ok(
                json!([{"priority":100,"src":"all","table":"100","future_selector":true}])
                    .to_string(),
            ),
        ] {
            let mut f = cross_table_shadow(v6);
            f.outputs[if v6 { 5 } else { 4 }] = policy;
            let report = reports(&f)
                .into_iter()
                .find(|r| r.interface == "vpn0")
                .unwrap();
            let coverage = if v6 {
                &report.coverage.ipv6
            } else {
                &report.coverage.ipv4
            };
            assert!(matches!(coverage, Fact::Unknown(_)));
            assert!(matches!(
                predicate(&report, Rule::EntryCannotBePreserved),
                Fact::Unknown(_)
            ));
            assert!(matches!(report.decision, Fact::Unknown(_)));
        }
    }
}
#[test]
fn uncontested_coverage_candidates_do_not_erase_addresses_or_resource_claims() {
    for routes in [
        split(json!(254), json!(254)),
        json!([{"dst":"default","dev":"vpn0","table":"254"}]),
    ] {
        let f = fixture(routes, json!([]), json!([]), json!([]));
        let obj = object(&f);
        let Fact::Known(routes) = &obj.routes else {
            panic!()
        };
        assert!(routes.iter().all(|r| r.table == Fact::Known(Some(254))
            && r.scope == Fact::Known(RouteScope::Global)
            && matches!(r.role, Fact::Unknown(_))));
        let Fact::Known(addresses) = &obj.addresses else {
            panic!()
        };
        assert_eq!(addresses[0].address.to_string(), "10.8.0.2");
        assert_eq!(addresses[0].prefix_len, 24);
    }
    let mut routes = split(json!(254), json!(254));
    routes
        .as_array_mut()
        .unwrap()
        .push(json!({"dst":"198.18.42.0/24","dev":"vpn0","table":"254"}));
    let obj = object(&fixture(routes.clone(), json!([]), json!([]), json!([])));
    let Fact::Known(facts) = obj.routes else {
        panic!()
    };
    assert_eq!(
        facts
            .iter()
            .find(|r| r.prefix == "198.18.42.0/24")
            .unwrap()
            .role,
        Fact::Known(RouteRole::ResourceClaim)
    );
    let report = reports(&fixture(routes, json!([]), json!([]), json!([]))).remove(0);
    assert_eq!(
        predicate(&report, Rule::AddressCollision),
        &Fact::Known(true)
    );
    assert!(matches!(report.decision, Fact::Known(Some(_))));
    // A supplied interface address is independently decisive, even with unknown coverage.
    let mut f = fixture(
        split(json!(254), json!(254)),
        json!([]),
        json!([]),
        json!([]),
    );
    f.outputs[1] = Ok(json!([{"ifname":"vpn0","addr_info":[{"family":"inet","local":"198.18.0.1","prefixlen":16}]}]).to_string());
    let report = reports(&f).remove(0);
    assert_eq!(predicate(&report, Rule::OtherTunProxy), &Fact::Known(true));
    assert_eq!(
        predicate(&report, Rule::AddressCollision),
        &Fact::Known(true)
    );
    assert!(matches!(report.decision, Fact::Known(Some(_))));
}
