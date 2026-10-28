//! Existing sanitized captures and synthetic failures only; no device acceptance.
use super::*;
use std::path::Path;

const FIXTURES: [&str; 5] = [
    "macos-p101-netstat-rn-2026-09-08.txt",
    "macos-p101-routes-ts-off-2026-09-12.txt",
    "macos-p101-routes-ts-on-2026-09-12.txt",
    "macos-p101-route-get-ts-on-2026-09-13.txt",
    "macos-p101-parallels-running-2026-09-13.txt",
];
const HEADER: &str = "Destination Gateway Flags Netif Expire\n";
fn section(file: &str, id: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/route_probe/tests/fixtures")
        .join(file);
    let raw = std::fs::read_to_string(path).unwrap();
    let mut active = false;
    let mut found = false;
    let mut result = String::new();
    for line in raw.lines() {
        if line.starts_with("@@@") {
            if active {
                break;
            }
            active = line == format!("@@@{id}");
            found |= active;
        } else if active {
            result.push_str(line);
            result.push('\n');
        }
    }
    assert!(found, "{file}: missing {id}");
    result
}
fn known(output: String, family: AddressFamily) -> Vec<MacosRouteObservation> {
    let Fact::Known(rows) = parse_route_observations(Ok(output), family) else {
        panic!("expected known rows")
    };
    rows
}
fn capture_rows(file: &str, family: AddressFamily) -> Vec<MacosRouteObservation> {
    let suffix = if file.contains("route-get") || file.contains("parallels") {
        "_NETSTAT"
    } else {
        ""
    };
    let prefix = if family == AddressFamily::V4 {
        "V4"
    } else {
        "V6"
    };
    known(section(file, &format!("{prefix}{suffix}")), family)
}
#[test]
fn all_five_captures_preserve_scoped_defaults_and_raw_evidence_in_both_families() {
    let expected_rows = [[51, 130], [51, 132], [85, 134], [88, 130], [98, 146]];
    let expected_scoped_defaults = [[0, 8], [0, 8], [1, 9], [1, 9], [3, 9]];
    for (capture, file) in FIXTURES.into_iter().enumerate() {
        for (leg, family) in [AddressFamily::V4, AddressFamily::V6]
            .into_iter()
            .enumerate()
        {
            let rows = capture_rows(file, family);
            let default = if family == AddressFamily::V4 {
                "0.0.0.0/0"
            } else {
                "::/0"
            };
            assert!(
                rows.iter().any(|r| r.prefix == default
                    && r.interface == "en0"
                    && r.scope == Fact::Known(RouteScope::Global)),
                "{file} {family:?}"
            );
            assert_eq!(rows.len(), expected_rows[capture][leg], "{file} {family:?}");
            assert_eq!(
                rows.iter()
                    .filter(|r| r.prefix == default
                        && r.scope == Fact::Known(RouteScope::InterfaceScoped))
                    .count(),
                expected_scoped_defaults[capture][leg],
                "{file} {family:?}"
            );
            assert!(
                rows.iter().all(|r| r.scope
                    == Fact::Known(if r.flags.contains('I') {
                        RouteScope::InterfaceScoped
                    } else {
                        RouteScope::Global
                    })),
                "{file} {family:?}"
            );
            assert!(rows
                .iter()
                .all(|r| r.family == family && !r.gateway.is_empty() && !r.flags.is_empty()));
        }
    }
    let rows = capture_rows(FIXTURES[2], AddressFamily::V4);
    let scoped = rows
        .iter()
        .find(|r| r.interface == "utun11" && r.prefix == "0.0.0.0/0")
        .unwrap();
    assert_eq!(scoped.flags, "UCSIg");
    assert_eq!(scoped.gateway, "link#29");
    let physical = rows
        .iter()
        .find(|r| r.interface == "en0" && r.prefix == "0.0.0.0/0")
        .unwrap();
    assert_eq!(physical.flags, "UGScg");
    assert_eq!(physical.gateway, "192.168.10.1");
}
#[test]
fn connected_disconnected_and_virtualization_captures_have_independent_assertions() {
    let off = capture_rows(FIXTURES[1], AddressFamily::V4);
    let on = capture_rows(FIXTURES[2], AddressFamily::V4);
    assert!(!off
        .iter()
        .any(|r| r.interface == "utun11" && r.prefix == "32.0.0.1/32"));
    assert!(on
        .iter()
        .any(|r| r.interface == "utun11" && r.prefix == "32.0.0.1/32" && r.gateway == "link#29"));
    let parallels = capture_rows(FIXTURES[4], AddressFamily::V4);
    assert!(parallels.iter().any(|r| r.interface == "bridge100"));
    assert!(parallels.iter().any(|r| r.interface == "bridge101"));
    // Printed interfaces stay separate; bridge routes are not promoted to VPN identity.
}
#[test]
fn abbreviated_destinations_and_zone_suffixes_normalize_without_gateway_guesses() {
    let rows = known(
        format!("{HEADER}192.168.10 link#1 UC en0\n224.0.0/4 link#2 UmCS en0\n127 link#3 UC lo0\n"),
        AddressFamily::V4,
    );
    assert_eq!(
        rows.iter().map(|r| r.prefix.as_str()).collect::<Vec<_>>(),
        vec!["192.168.10.0/24", "224.0.0.0/4", "127.0.0.0/8"]
    );
    let rows = known(
        format!("{HEADER}fe80::%utun0/64 fe80::1%utun0 UCI utun0\ndefault link#4 UGci utun1\n"),
        AddressFamily::V6,
    );
    assert_eq!(rows[0].prefix, "fe80::/64");
    assert_eq!(rows[0].gateway, "fe80::1%utun0");
    assert_eq!(rows[1].prefix, "::/0");
    assert_eq!(rows[1].scope, Fact::Known(RouteScope::Global)); // i is not I
}
#[test]
fn header_positions_support_refs_use_and_do_not_merge_interfaces() {
    let rows = known("Destination Gateway Flags Refs Use Netif Expire\ndefault link#1 UCSIg 1 0 utun0\ndefault link#2 UCSIg 2 0 utun1\n".into(), AddressFamily::V4);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].interface, "utun0");
    assert_eq!(rows[1].interface, "utun1");
    assert_eq!(rows[0].gateway, "link#1");
    assert_eq!(rows[1].gateway, "link#2");
}
#[test]
fn unfamiliar_flags_keep_rows_and_raw_tokens_with_unknown_scope() {
    let rows = known(
        format!("{HEADER}default link#1 UZg utun0\ndefault link#2 UZI utun1\n"),
        AddressFamily::V4,
    );
    assert_eq!(rows[0].flags, "UZg");
    assert_eq!(rows[1].flags, "UZI");
    assert!(rows.iter().all(|r| matches!(r.scope, Fact::Unknown(_))));
}
#[test]
fn failures_and_malformed_sources_never_return_partial_or_empty_success() {
    for bad in [
        Err("permission denied".into()),
        Ok("".into()),
        Ok("Permission denied".into()),
        Ok("<<POLARIS-CAPTURE-UNAVAILABLE>>".into()),
        Ok("Destination Gateway Netif\ndefault link#1 utun0\n".into()),
        Ok("Destination Gateway Flags Flags Netif\n".into()),
        Ok(format!("{HEADER}default link#1 UCSIg utun0\ntruncated\n")),
        Ok(format!("{HEADER}bad-prefix link#1 UCSIg utun0\n")),
        Ok(format!("{HEADER}0.0.0.0/+1 link#1 UCSIg utun0\n")),
        Ok(format!("{HEADER}010.0.0.1 link#1 UCSIg utun0\n")),
        Ok(format!("{HEADER}default link#1 UCSIg 123\n")),
        Ok(format!(
            "{HEADER}default link#1 UCSIg utun0 extra unexpected\n"
        )),
        Ok(format!("{HEADER}{HEADER}")),
    ] {
        assert!(matches!(
            parse_route_observations(bad, AddressFamily::V4),
            Fact::Unknown(_)
        ));
    }
}
#[test]
fn family_provenance_rejects_wrong_destinations_banners_and_mixed_tables() {
    for (raw, family) in [
        (format!("{HEADER}fe80::%/64 link#1 UCI utun0\n"), AddressFamily::V6),
        (format!("{HEADER}fe80::%utun0%other/64 link#1 UCI utun0\n"), AddressFamily::V6),
        (format!("{HEADER}::/0 link#1 UCI utun0\n"), AddressFamily::V4),
        (format!("{HEADER}0.0.0.0/0 link#1 UCI utun0\n"), AddressFamily::V6),
        (format!("Internet6:\n{HEADER}default link#1 UCI utun0\n"), AddressFamily::V4),
        (format!("Internet:\n{HEADER}default link#1 UCI utun0\nInternet6:\n{HEADER}default link#2 UCI utun1\n"), AddressFamily::V4),
    ] {
        assert!(matches!(parse_route_observations(Ok(raw), family), Fact::Unknown(_)));
    }
}
#[test]
fn byte_limit_is_inclusive_and_overflow_is_unknown_without_truncation() {
    let mut raw = HEADER.to_string();
    raw.push_str(&" ".repeat(MAX_SOURCE_BYTES - raw.len()));
    assert_eq!(raw.len(), MAX_SOURCE_BYTES);
    assert_eq!(
        parse_route_observations(Ok(raw.clone()), AddressFamily::V4),
        Fact::Known(vec![])
    );
    raw.push(' ');
    assert!(matches!(
        parse_route_observations(Ok(raw), AddressFamily::V4),
        Fact::Unknown(_)
    ));
    // UTF-8 byte size, not character count.
    assert!(matches!(
        parse_route_observations(Ok("界".repeat(MAX_SOURCE_BYTES / 3 + 1)), AddressFamily::V6),
        Fact::Unknown(_)
    ));
}
#[test]
fn row_limit_is_inclusive_and_overflow_is_unknown_without_truncation() {
    let row = "default link#1 UCSIg utun0\n";
    let mut raw = format!("{HEADER}{}", row.repeat(MAX_ROUTE_ROWS));
    assert_eq!(known(raw.clone(), AddressFamily::V4).len(), MAX_ROUTE_ROWS);
    raw.push_str(row);
    assert!(matches!(
        parse_route_observations(Ok(raw), AddressFamily::V4),
        Fact::Unknown(_)
    ));
}
#[test]
fn explicit_zero_row_table_is_known_empty_and_diagnostics_name_the_source() {
    for family in [AddressFamily::V4, AddressFamily::V6] {
        assert_eq!(
            parse_route_observations(Ok(HEADER.into()), family),
            Fact::Known(vec![])
        );
        let Fact::Unknown(reason) = parse_route_observations(Err("denied".into()), family) else {
            panic!()
        };
        assert!(reason.contains("macOS netstat"));
        assert!(reason.contains("denied"));
    }
}

#[test]
fn printed_flag_width_cannot_prove_absence_of_late_interface_scope_bit() {
    for family in [AddressFamily::V4, AddressFamily::V6] {
        for (flags, expected) in [
            ("UGHRDMmdC", Some(RouteScope::Global)), // 9: not clipped
            ("UGHRDMmdCX", None),                    // 10: I may be beyond the print precision
            ("UGHRDMmdCXL", None), // longer unsupported layout cannot prove absence
            ("UGHRDMmdCI", Some(RouteScope::InterfaceScoped)), // observed I at width 10
            ("UCSIg", Some(RouteScope::InterfaceScoped)),
            ("UGci", Some(RouteScope::Global)), // i is not I
        ] {
            let rows = known(format!("{HEADER}default link#7 {flags} utun7\n"), family);
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].flags, flags);
            assert_eq!(rows[0].gateway, "link#7");
            assert_eq!(rows[0].interface, "utun7");
            assert_eq!(rows[0].family, family);
            assert_eq!(
                rows[0].prefix,
                if family == AddressFamily::V4 {
                    "0.0.0.0/0"
                } else {
                    "::/0"
                }
            );
            if let Some(scope) = expected {
                assert_eq!(rows[0].scope, Fact::Known(scope), "{family:?} {flags}");
            } else {
                assert!(
                    matches!(rows[0].scope, Fact::Unknown(_)),
                    "{family:?} {flags}: {:?}",
                    rows[0].scope
                );
            }
        }
    }
}

use crate::exec::{Command, CommandOutput, CommandRunner};
use std::sync::Mutex;

struct MacFixtureRunner {
    outputs: [Result<String, String>; 3],
    calls: Mutex<Vec<(Command, Duration)>>,
}
impl CommandRunner for MacFixtureRunner {
    fn run(&self, command: &Command, budget: Duration) -> Result<CommandOutput, String> {
        let mut calls = self.calls.lock().unwrap();
        let index = calls.len();
        calls.push((command.clone(), budget));
        self.outputs[index].clone().map(|stdout| CommandOutput {
            stdout,
            stderr: String::new(),
        })
    }
}
fn provider(outputs: [Result<String, String>; 3]) -> Fact<Vec<ObjectFacts>> {
    collect_macos(&MacFixtureRunner {
        outputs,
        calls: Mutex::default(),
    })
}
fn fixture_inputs(file: &str) -> [Result<String, String>; 3] {
    let suffix = if file.contains("route-get") || file.contains("parallels") {
        "_NETSTAT"
    } else {
        ""
    };
    [
        Ok(section(file, &format!("V4{suffix}"))),
        Ok(section(file, &format!("V6{suffix}"))),
        if file == FIXTURES[0] {
            Err("historical capture has no ifconfig source".into())
        } else {
            Ok(section(
                file,
                if suffix.is_empty() {
                    "IFCONFIG"
                } else {
                    "IFCONFIG_FLAGS"
                },
            ))
        },
    ]
}
fn object_rows(facts: Fact<Vec<ObjectFacts>>) -> Vec<ObjectFacts> {
    let Fact::Known(objects) = facts else {
        panic!("expected known roster: {facts:?}")
    };
    objects
}
fn empty_routes() -> [Result<String, String>; 3] {
    [
        Ok(HEADER.into()),
        Ok(HEADER.into()),
        Ok("vpn0: flags=8010<POINTOPOINT,MULTICAST> mtu 1280\n".into()),
    ]
}
#[test]
fn provider_uses_only_three_exact_readonly_queries_and_two_second_operation_budgets() {
    let runner = MacFixtureRunner {
        outputs: empty_routes(),
        calls: Mutex::default(),
    };
    let objects = object_rows(collect_macos(&runner));
    assert_eq!(
        runner.calls.into_inner().unwrap(),
        vec![
            (
                Command::new("netstat", ["-rn", "-f", "inet"]),
                Duration::from_secs(2)
            ),
            (
                Command::new("netstat", ["-rn", "-f", "inet6"]),
                Duration::from_secs(2)
            ),
            (Command::new("ifconfig", ["-a"]), Duration::from_secs(2)),
        ]
    );
    assert_eq!(objects.len(), 1);
    assert_eq!(objects[0].addresses, Fact::Known(vec![]));
    assert_eq!(objects[0].routes, Fact::Known(vec![]));
    assert!(matches!(objects[0].tunnel, Fact::Unknown(_)));
    assert!(matches!(objects[0].policy_rules, Fact::Unknown(_)));
}
#[test]
fn provider_reads_actual_existing_rosters_and_retains_all_routes_without_selection_claims() {
    assert!(matches!(
        provider(fixture_inputs(FIXTURES[0])),
        Fact::Unknown(_)
    ));
    for file in &FIXTURES[1..] {
        let objects = object_rows(provider(fixture_inputs(file)));
        let rows: Vec<_> = [AddressFamily::V4, AddressFamily::V6]
            .into_iter()
            .flat_map(|family| capture_rows(file, family))
            .collect();
        let mut route_count = 0;
        for object in &objects {
            assert!(
                matches!(object.tunnel, Fact::Unknown(_)),
                "{file} {}",
                object.interface
            );
            assert!(matches!(object.virtualization, Fact::Unknown(_)));
            assert!(matches!(object.policy_rules, Fact::Unknown(_)));
            assert!(matches!(object.stable_identity, Fact::Unknown(_)));
            let Fact::Known(routes) = &object.routes else {
                panic!("{file}: {} {:?}", object.interface, object.routes)
            };
            let expected: Vec<_> = rows
                .iter()
                .filter(|r| r.interface == object.interface)
                .collect();
            assert_eq!(routes.len(), expected.len(), "{file} {}", object.interface);
            for (actual, raw) in routes.iter().zip(expected) {
                assert_eq!(actual.prefix, raw.prefix);
                assert_eq!(actual.scope, raw.scope);
                assert_eq!(actual.table, Fact::Known(None));
                assert!(matches!(actual.role, Fact::Unknown(_)));
            }
            route_count += routes.len();
            assert!(
                matches!(object.addresses, Fact::Known(_)),
                "{file} {} {:?}",
                object.interface,
                object.addresses
            );
        }
        assert_eq!(route_count, rows.len());
        let physical = objects.iter().find(|o| o.interface == "en0").unwrap();
        let Fact::Known(addresses) = &physical.addresses else {
            unreachable!()
        };
        assert!(addresses.iter().any(
            |a| a.address == "192.168.10.142".parse::<IpAddr>().unwrap() && a.prefix_len == 24
        ));
        assert!(!objects.iter().any(|o| o.interface == "member"));
        let inactive = objects.iter().find(|o| o.interface == "stf0").unwrap();
        assert_eq!(inactive.addresses, Fact::Known(vec![]));
    }
}
// LINK variants are injected into an existing complete sanitized capture, not
// newly collected device evidence. Their IFF bits change only header validity;
// neither link flags nor the POINTOPOINT flag establish object classification.
fn capture_with_link_flag(flag: &str, bit: u32) -> [Result<String, String>; 3] {
    let mut inputs = fixture_inputs(FIXTURES[2]);
    let roster = inputs[2].as_mut().unwrap();
    let header = roster
        .lines()
        .find(|line| line.starts_with("en0:"))
        .unwrap();
    let token = header.split_whitespace().nth(1).unwrap();
    let (number, names) = token
        .strip_prefix("flags=")
        .unwrap()
        .split_once('<')
        .unwrap();
    let names = names.strip_suffix('>').unwrap();
    let bits = u32::from_str_radix(number, 16).unwrap() | bit;
    let replacement = header.replacen(token, &format!("flags={bits:x}<{names},{flag}>"), 1);
    *roster = roster.replacen(header, &replacement, 1);
    assert!(roster
        .lines()
        .find(|line| line.starts_with("en0:"))
        .unwrap()
        .contains(flag));
    inputs
}
#[test]
fn roster_accepts_ascii_digit_link_flags_without_classification_evidence() {
    let baseline = decode_interfaces(fixture_inputs(FIXTURES[2])[2].clone().unwrap()).unwrap();
    for (flag, bit) in [("LINK0", 0x1000), ("LINK1", 0x2000), ("LINK2", 0x4000)] {
        let roster =
            decode_interfaces(capture_with_link_flag(flag, bit)[2].clone().unwrap()).unwrap();
        assert_eq!(
            roster, baseline,
            "{flag}: independent roster/address facts changed"
        );
        assert!(matches!(roster["en0"].tunnel, Fact::Unknown(_)));
        assert!(matches!(roster["en0"].virtualization, Fact::Unknown(_)));
    }
}
#[test]
fn roster_flag_names_still_reject_non_ascii_lowercase_punctuation_and_empty_tokens() {
    for flag in [
        "Link0",
        "LINK-0",
        "LINK.0",
        "LINK+0",
        "LINK０",
        "UP,,LINK0",
        ",LINK0",
        "LINK0,",
    ] {
        assert!(
            decode_interfaces(format!("en0: flags=9863<{flag}> mtu 1500\n")).is_err(),
            "{flag}"
        );
    }
}
fn assert_provider_preserves_capture_with_link_flag(flag: &str, bit: u32) {
    let baseline = object_rows(provider(fixture_inputs(FIXTURES[2])));
    let observed = object_rows(provider(capture_with_link_flag(flag, bit)));
    assert_eq!(
        observed, baseline,
        "{flag}: complete provider facts changed"
    );
    assert_eq!(observed.len(), 29);
    let routes: usize = observed
        .iter()
        .map(|o| match &o.routes {
            Fact::Known(rows) => rows.len(),
            Fact::Unknown(reason) => panic!("{flag}: {} lost routes: {reason}", o.interface),
        })
        .sum();
    assert_eq!(routes, 219);
    let physical = observed.iter().find(|o| o.interface == "en0").unwrap();
    assert!(
        matches!(&physical.addresses, Fact::Known(rows) if rows.iter().any(|a|
        a.address == "192.168.10.142".parse::<IpAddr>().unwrap() && a.prefix_len == 24))
    );
    assert!(observed
        .iter()
        .all(|o| matches!(o.tunnel, Fact::Unknown(_))
            && matches!(o.virtualization, Fact::Unknown(_))));
}
#[test]
fn provider_link0_preserves_complete_capture_roster_addresses_and_routes() {
    assert_provider_preserves_capture_with_link_flag("LINK0", 0x1000);
}
#[test]
fn provider_link1_preserves_complete_capture_roster_addresses_and_routes() {
    assert_provider_preserves_capture_with_link_flag("LINK1", 0x2000);
}
#[test]
fn provider_link2_preserves_complete_capture_roster_addresses_and_routes() {
    assert_provider_preserves_capture_with_link_flag("LINK2", 0x4000);
}

#[test]
fn address_rows_keep_host_bits_peer_local_addresses_and_explicit_ipv6_zone() {
    let mut input = empty_routes();
    input[2] = Ok("vpn0: flags=8010<POINTOPOINT,MULTICAST> mtu 1280\n\tinet 10.77.2.9 --> 10.77.2.1 netmask 0xffffff00\n\tinet 192.168.4.99 netmask 255.255.255.0 broadcast 192.168.4.255\n\tinet6 fe80::abcd%vpn0 prefixlen 64 scopeid 0x9\n\tinet6 fd00::9 prefixlen 128\n".into());
    let objects = object_rows(provider(input));
    let Fact::Known(rows) = &objects[0].addresses else {
        panic!("{:?}", objects[0])
    };
    assert_eq!(
        rows.iter()
            .map(|a| (a.address.to_string(), a.prefix_len))
            .collect::<Vec<_>>(),
        vec![
            ("10.77.2.9".into(), 24),
            ("192.168.4.99".into(), 24),
            ("fe80::abcd".into(), 64),
            ("fd00::9".into(), 128)
        ]
    );
}
#[test]
fn incomplete_or_invalid_roster_is_unknown_never_a_partial_or_empty_object_list() {
    for raw in [
        "",
        "\tinet 10.8.0.2 netmask 0xffffff00\n",
        "vpn0: flags=bad mtu 1280\n",
        "vpn0: flags=0<> mtu 1280\nmalformed\n",
        "vpn0: flags=0<> mtu 1280\nvpn0: flags=0<> mtu 1280\n",
        "member: en1 flags=3<LEARNING,DISCOVER>\n",
    ] {
        let mut input = empty_routes();
        input[2] = Ok(raw.into());
        assert!(matches!(provider(input), Fact::Unknown(_)), "{raw}");
    }
    for error in ["permission denied", "operation timed out", "not spawned"] {
        let mut input = empty_routes();
        input[2] = Err(error.into());
        let Fact::Unknown(reason) = provider(input) else {
            panic!("error became known")
        };
        assert!(reason.contains(error));
    }
}
#[test]
fn malformed_address_fails_its_whole_field_without_erasing_other_interface_or_route_facts() {
    for row in [
        "inet 10.8.0.2",
        "inet 10.8.0.2 netmask 0xff00ff00",
        "inet 10.8.0.2 netmask 0xffff",
        "inet ::1 netmask 0xffffffff",
        "inet6 10.8.0.2 prefixlen 24",
        "inet6 ::1 prefixlen 129",
        "inet6 ::1 prefixlen 1 prefixlen 2",
        "inet6 fe80::1%other prefixlen 64",
        "inet6 fe80::1%vpn0%extra prefixlen 64",
        "inet 10.8.0.2 netmask 0xffffff00 netmask 0xffffffff",
    ] {
        let mut input = empty_routes();
        input[2] = Ok(format!("vpn0: flags=0<> mtu 1280\n\tinet 10.8.0.9 netmask 0xffffff00\n\t{row}\n\tinet 10.8.0.10 netmask 0xffffff00\nother: flags=0<> mtu 1280\n\tinet 10.9.0.3 netmask 0xffffff00\n"));
        let objects = object_rows(provider(input));
        assert!(
            matches!(
                objects
                    .iter()
                    .find(|o| o.interface == "vpn0")
                    .unwrap()
                    .addresses,
                Fact::Unknown(_)
            ),
            "{row}"
        );
        let other = objects.iter().find(|o| o.interface == "other").unwrap();
        assert!(matches!(other.addresses, Fact::Known(_)));
        assert_eq!(other.routes, Fact::Known(vec![]));
    }
}
#[test]
fn route_failure_or_unmatched_interface_never_borrows_other_family_or_object_evidence() {
    for bad in [
        Err("IPv6 permission denied".into()),
        Ok("garbled".into()),
        Ok(format!("{HEADER}::/0 link#2 UCSI missing0\n")),
    ] {
        let mut input = empty_routes();
        input[0] = Ok(format!(
            "{HEADER}0/1 link#2 UCS vpn0\n128/1 link#2 UCS vpn0\n"
        ));
        input[1] = bad;
        let objects = object_rows(provider(input));
        assert!(matches!(objects[0].routes, Fact::Unknown(_)));
        assert_eq!(objects[0].addresses, Fact::Known(vec![]));
    }
    let mut input = empty_routes();
    input[0] = Ok(format!("{HEADER}0/1 link#2 UCS vpn0\n"));
    input[1] = Ok(format!("{HEADER}::/0 link#3 UCSI other\n"));
    input[2] = Ok("vpn0: flags=0<> mtu 1280\nother: flags=0<> mtu 1280\n".into());
    let objects = object_rows(provider(input));
    assert_eq!(objects.len(), 2);
    assert!(objects
        .iter()
        .all(|o| matches!(&o.routes, Fact::Known(rows) if rows.len()==1)));
}
#[test]
fn provider_limits_fail_closed_without_truncating_roster_or_address_list() {
    let mut input = empty_routes();
    input[2] = Ok("x".repeat(MAX_SOURCE_BYTES + 1));
    assert!(matches!(provider(input), Fact::Unknown(_)));
    let mut input = empty_routes();
    input[2] = Ok((0..=MAX_INTERFACE_ROWS)
        .map(|i| format!("if{i}: flags=0<> mtu 1280\n"))
        .collect());
    assert!(matches!(provider(input), Fact::Unknown(_)));
    let mut input = empty_routes();
    input[2] = Ok(format!(
        "vpn0: flags=0<> mtu 1280\n{}",
        "\tinet 10.8.0.9 netmask 0xffffff00\n".repeat(MAX_INTERFACE_ADDRESSES + 1)
    ));
    assert!(matches!(
        object_rows(provider(input))[0].addresses,
        Fact::Unknown(_)
    ));
}
