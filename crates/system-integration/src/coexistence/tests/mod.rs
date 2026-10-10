//! Synthetic iproute2 JSON fixtures only. No host query, network changes or device acceptance.
use super::*;
use crate::exec::CommandOutput;
use polaris_config_engine::builder::coexistence::{
    AddressFamily, PolicySelectorScope, RouteRole, Rule,
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
fn fixture(r4: Value, r6: Value, p4: Value, p6: Value) -> FixtureRunner {
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
fn full_coverage_is_not_resource_collision_but_additional_route_is() {
    let report = reports(&fixture(
        split(json!(254), json!(254)),
        json!([]),
        json!([]),
        json!([]),
    ))
    .remove(0);
    assert_eq!(
        predicate(&report, Rule::AddressCollision),
        &Fact::Known(false)
    );
    assert_eq!(
        predicate(&report, Rule::EntryCannotBePreserved),
        &Fact::Known(true)
    );
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
    assert_eq!(r[0].role, Fact::Known(RouteRole::CoverageDeclaration));
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
        assert_eq!(
            predicate(&report, Rule::EntryCannotBePreserved),
            &Fact::Known(table == 100 && !v6)
        );
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
fn ipv6_coverage_and_named_builtin_tables_are_supported() {
    let f = fixture(
        json!([]),
        json!([{"dst":"::/1","dev":"vpn0","table":"main"},
        {"dst":"8000::/1","dev":"vpn0","table":"main"}]),
        json!([]),
        json!([]),
    );
    let report = reports(&f).remove(0);
    assert_eq!(report.coverage.ipv6, Fact::Known(true));
    assert_eq!(
        predicate(&report, Rule::AddressCollision),
        &Fact::Known(false)
    );
    assert_eq!(
        predicate(&report, Rule::EntryCannotBePreserved),
        &Fact::Known(true)
    );
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
        assert_eq!(
            predicate(&reports(&f)[0], Rule::OtherTunProxy),
            &Fact::Known(table == 100)
        );
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
    assert_eq!(report.coverage.ipv4, Fact::Known(true));
    assert_eq!(
        predicate(&report, Rule::AddressCollision),
        &Fact::Known(false)
    );
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
    assert_eq!(
        predicate(&reports(&f)[0], Rule::AddressCollision),
        &Fact::Known(false)
    );
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
