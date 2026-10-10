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
