//! Injected synthetic inputs only; legacy fixture-derived rows are marked incomplete.
//! No provider/FFI/device acceptance or host reads are claimed.
use super::*;
use crate::route_probe::{parse_get_netipaddress, parse_route_print_routes};
use std::path::Path;

fn unknown<T>() -> Fact<T> {
    Fact::Unknown("not supplied".into())
}
fn reference(alias: &str, luid: u64, index: u32) -> WindowsInterfaceRef {
    WindowsInterfaceRef {
        alias: Fact::Known(alias.into()),
        luid: Fact::Known(luid),
        if_index: Fact::Known(index),
    }
}
fn source<T>(rows: Vec<T>) -> Fact<ReadRows<T>> {
    Fact::Known(ReadRows {
        rows,
        complete: Fact::Known(true),
        compartment: Fact::Known(1),
        error: Fact::Known(None),
    })
}
fn empty() -> WindowsFactInput {
    WindowsFactInput {
        adapters: source(vec![]),
        addresses: source(vec![]),
        routes4: source(vec![]),
        routes6: source(vec![]),
        ras: source(vec![]),
    }
}
fn adapter(alias: &str, luid: u64, if_type: u32) -> WindowsAdapterObservation {
    WindowsAdapterObservation {
        interface: reference(alias, luid, 7),
        if_type: Fact::Known(if_type),
        description: unknown(),
    }
}
fn address(family: AddressFamily, ip: &str, prefix: u8) -> WindowsAddressObservation {
    WindowsAddressObservation {
        interface: reference("vpn0", 1, 7),
        family,
        address: ip.parse().unwrap(),
        prefix_len: Fact::Known(prefix),
        scope_id: unknown(),
    }
}
fn route(family: AddressFamily, prefix: &str) -> WindowsRouteObservation {
    WindowsRouteObservation {
        interface: reference("vpn0", 1, 7),
        family,
        prefix: prefix.into(),
        next_hop: unknown(),
        next_hop_scope_id: unknown(),
        route_metric: unknown(),
        interface_metric: unknown(),
    }
}
fn rows<T>(source: &Fact<ReadRows<T>>) -> &ReadRows<T> {
    match source {
        Fact::Known(rows) => rows,
        Fact::Unknown(reason) => panic!("unexpected Unknown: {reason}"),
    }
}
#[test]
fn independent_dual_family_facts_preserve_host_bits_and_route_metadata() {
    let mut input = empty();
    input.adapters = source(vec![adapter("vpn0", 1, 53)]);
    input.addresses = source(vec![
        address(AddressFamily::V4, "10.8.0.2", 24),
        address(AddressFamily::V6, "fd00::1234", 64),
    ]);
    let mut r = route(AddressFamily::V4, "198.18.42.9/24");
    r.next_hop = Fact::Known("10.8.0.1".parse().unwrap());
    r.route_metric = Fact::Known(10);
    r.interface_metric = Fact::Known(20);
    input.routes4 = source(vec![r]);
    input.routes6 = source(vec![route(AddressFamily::V6, "fd00::1234/64")]);
    let result = validate_windows_input(input);
    let a = &rows(&result.addresses).rows;
    assert_eq!(a[0].address.to_string(), "10.8.0.2");
    assert_eq!(a[0].prefix_len, Fact::Known(24));
    assert_eq!(a[1].address.to_string(), "fd00::1234");
    assert_eq!(a[1].prefix_len, Fact::Known(64));
    let r = &rows(&result.routes4).rows[0];
    assert_eq!(r.prefix, "198.18.42.0/24");
    assert_eq!(r.next_hop, Fact::Known("10.8.0.1".parse().unwrap()));
    assert_eq!(r.route_metric, Fact::Known(10));
    assert_eq!(r.interface_metric, Fact::Known(20));
    assert_eq!(rows(&result.routes6).rows[0].prefix, "fd00::/64");
}
#[test]
fn empty_is_authoritative_only_with_explicit_complete_scope() {
    for complete in [Fact::Known(false), unknown()] {
        let mut input = empty();
        let Fact::Known(ref mut s) = input.routes4 else {
            panic!()
        };
        s.complete = complete.clone();
        let result = validate_windows_input(input);
        let s = rows(&result.routes4);
        assert_eq!(s.complete, complete);
        assert!(matches!(s.proven_empty(), Fact::Unknown(_)));
    }
    for scope in [unknown(), Fact::Known(0)] {
        let mut input = empty();
        let Fact::Known(ref mut s) = input.routes6 else {
            panic!()
        };
        s.compartment = scope;
        let result = validate_windows_input(input);
        let s = rows(&result.routes6);
        assert!(matches!(s.complete, Fact::Unknown(_)));
        assert!(matches!(s.compartment, Fact::Unknown(_)));
        assert!(matches!(s.proven_empty(), Fact::Unknown(_)));
    }
    let result = validate_windows_input(empty());
    assert_eq!(rows(&result.ras).proven_empty(), Fact::Known(true));
}
#[test]
fn failure_and_partial_read_preserve_unrelated_observations_without_empty_success() {
    for reason in ["permission denied", "unsupported", "unavailable"] {
        let mut input = empty();
        input.adapters = Fact::Unknown(reason.into());
        input.addresses = source(vec![address(AddressFamily::V4, "10.8.0.2", 24)]);
        input.routes4 = source(vec![route(AddressFamily::V4, "0.0.0.0/0")]);
        if let Fact::Known(s) = &mut input.routes4 {
            s.complete = Fact::Known(false);
        }
        input.ras = Fact::Unknown(reason.into());
        let result = validate_windows_input(input);
        assert_eq!(result.adapters, Fact::Unknown(reason.into()));
        assert_eq!(result.ras, Fact::Unknown(reason.into()));
        assert_eq!(
            rows(&result.addresses).rows[0].address.to_string(),
            "10.8.0.2"
        );
        assert_eq!(rows(&result.routes4).rows.len(), 1);
        assert_eq!(rows(&result.routes4).complete, Fact::Known(false));
        assert_eq!(rows(&result.routes4).proven_empty(), Fact::Known(false));
    }
}
#[test]
fn missing_keys_or_compartment_do_not_upgrade_legacy_completeness_or_metrics() {
    let mut input = empty();
    let mut a = adapter("Wintun", 1, 53);
    a.interface.luid = unknown();
    a.interface.if_index = unknown();
    input.adapters = source(vec![a]);
    let mut r = route(AddressFamily::V4, "0.0.0.0/1");
    r.interface = WindowsInterfaceRef {
        alias: Fact::Known("Wintun".into()),
        luid: unknown(),
        if_index: unknown(),
    };
    input.routes4 = source(vec![r]);
    let result = validate_windows_input(input);
    assert!(matches!(rows(&result.adapters).complete, Fact::Unknown(_)));
    let s = rows(&result.routes4);
    assert!(matches!(s.complete, Fact::Unknown(_)));
    assert!(matches!(s.rows[0].route_metric, Fact::Unknown(_)));
    assert!(matches!(s.rows[0].interface_metric, Fact::Unknown(_)));
    assert!(matches!(s.rows[0].next_hop, Fact::Unknown(_)));
    assert_eq!(s.rows[0].prefix, "0.0.0.0/1");
}
#[test]
fn iftype_names_and_ras_do_not_become_identity_or_tunnel_facts() {
    let mut input = empty();
    input.adapters = source(vec![
        adapter("Wintun", 1, 53),
        adapter("TAP-Windows6", 2, 53),
        adapter("vEthernet (Default Switch)", 3, 6),
    ]);
    input.ras = source(vec![WindowsRasObservation {
        name: Fact::Known("PolarisProbeL2TP".into()),
        all_users: true,
        interface: unknown(),
    }]);
    let result = validate_windows_input(input);
    let a = &rows(&result.adapters).rows;
    assert_eq!(a.len(), 3);
    assert_eq!(a[2].if_type, Fact::Known(6));
    let r = rows(&result.ras);
    assert_eq!(r.rows[0].name, Fact::Known("PolarisProbeL2TP".into()));
    assert!(r.rows[0].all_users);
    assert!(matches!(r.rows[0].interface, Fact::Unknown(_)));
    assert!(matches!(r.complete, Fact::Unknown(_)));
    let mut input = empty();
    input.ras = source(vec![WindowsRasObservation {
        name: Fact::Known("RAS-only".into()),
        all_users: false,
        interface: Fact::Known(reference("RAS-only", 9, 35)),
    }]);
    let result = validate_windows_input(input);
    assert_eq!(rows(&result.ras).rows.len(), 1);
    assert_eq!(rows(&result.ras).complete, Fact::Known(true)); // only synthetic association evidence
}
#[test]
fn name_only_ras_reference_is_unknown_not_an_alias_join() {
    let mut input = empty();
    input.adapters = source(vec![adapter("VPN", 1, 131)]);
    input.ras = source(vec![WindowsRasObservation {
        name: Fact::Known("VPN".into()),
        all_users: false,
        interface: Fact::Known(WindowsInterfaceRef {
            alias: Fact::Known("VPN".into()),
            luid: unknown(),
            if_index: Fact::Known(7),
        }),
    }]);
    let result = validate_windows_input(input);
    let s = rows(&result.ras);
    assert_eq!(s.rows[0].name, Fact::Known("VPN".into()));
    assert!(matches!(s.rows[0].interface, Fact::Unknown(_)));
    assert!(matches!(s.complete, Fact::Unknown(_)));
}
#[test]
fn conflicting_keys_aliases_and_duplicate_adapter_roster_are_unknown() {
    for mut other in [
        adapter("other", 1, 53),
        adapter("vpn0", 2, 53),
        adapter("vpn0", 1, 53),
    ] {
        other.interface.if_index = Fact::Known(8);
        let mut input = empty();
        input.adapters = source(vec![adapter("vpn0", 1, 53), other]);
        let result = validate_windows_input(input);
        assert!(matches!(result.adapters, Fact::Unknown(_)));
        assert_eq!(rows(&result.routes4).proven_empty(), Fact::Known(true));
    }
    let mut input = empty();
    let mut second = route(AddressFamily::V4, "128.0.0.0/1");
    second.interface.if_index = Fact::Known(8);
    input.routes4 = source(vec![route(AddressFamily::V4, "0.0.0.0/1"), second]);
    assert!(matches!(
        validate_windows_input(input).routes4,
        Fact::Unknown(_)
    ));
}
#[test]
fn adapter_cross_source_contradictions_fail_only_affected_source_and_do_not_borrow_scope() {
    for compartment in [Fact::Known(1), Fact::Known(2), unknown()] {
        let mut input = empty();
        input.adapters = source(vec![adapter("vpn0", 1, 53)]);
        let mut r = route(AddressFamily::V4, "0.0.0.0/0");
        r.interface.alias = Fact::Known("other".into());
        input.routes4 = source(vec![r]);
        if let Fact::Known(s) = &mut input.routes4 {
            s.compartment = compartment.clone();
        }
        let result = validate_windows_input(input);
        assert_eq!(rows(&result.adapters).rows.len(), 1);
        if compartment == Fact::Known(1) {
            assert!(matches!(result.routes4, Fact::Unknown(_)));
        } else {
            assert_eq!(
                rows(&result.routes4).rows[0].interface.alias,
                Fact::Known("other".into())
            );
        }
    }
}
#[test]
fn family_mismatch_and_bad_cidr_fail_source_without_erasing_addresses() {
    for r in [
        route(AddressFamily::V6, "::/0"),
        route(AddressFamily::V4, "::/0"),
        route(AddressFamily::V4, "010.0.0.1/32"),
        route(AddressFamily::V4, "0.0.0.0/+1"),
    ] {
        let mut input = empty();
        input.routes4 = source(vec![r]);
        input.addresses = source(vec![address(AddressFamily::V4, "10.8.0.2", 24)]);
        let result = validate_windows_input(input);
        assert!(matches!(result.routes4, Fact::Unknown(_)));
        assert_eq!(rows(&result.addresses).rows.len(), 1);
    }
    let mut input = empty();
    input.addresses = source(vec![address(AddressFamily::V4, "::1", 24)]);
    assert!(matches!(
        validate_windows_input(input).addresses,
        Fact::Unknown(_)
    ));
}
#[test]
fn indices_are_family_specific_and_are_not_stable_or_borrowed_as_luid() {
    let mut input = empty();
    let mut v4 = address(AddressFamily::V4, "10.8.0.2", 24);
    v4.interface.luid = unknown();
    let mut v6 = address(AddressFamily::V6, "fd00::2", 64);
    v6.interface.luid = unknown();
    v6.interface.alias = Fact::Known("other".into());
    input.addresses = source(vec![v4, v6]);
    let result = validate_windows_input(input);
    let a = &rows(&result.addresses).rows;
    assert_eq!(a.len(), 2);
    assert!(a
        .iter()
        .all(|a| matches!(a.interface.luid, Fact::Unknown(_))));
    assert_eq!(a[0].interface.if_index, a[1].interface.if_index); // same number, different families
}
#[test]
fn invalid_metadata_is_unknown_while_valid_zero_metrics_and_host_bits_survive() {
    for (family, ip, max) in [
        (AddressFamily::V4, "10.8.0.2", 32),
        (AddressFamily::V6, "fd00::2", 128),
    ] {
        for prefix in [0, max + 1, 255] {
            let mut input = empty();
            input.addresses = source(vec![address(family, ip, prefix)]);
            let result = validate_windows_input(input);
            let a = &rows(&result.addresses).rows[0];
            assert_eq!(a.address.to_string(), ip);
            assert!(matches!(a.prefix_len, Fact::Unknown(_)));
        }
    }
    let mut input = empty();
    let mut r = route(AddressFamily::V4, "0.0.0.0/0");
    r.route_metric = Fact::Known(0);
    r.interface_metric = Fact::Known(u32::MAX);
    r.next_hop = Fact::Known("::1".parse().unwrap());
    input.routes4 = source(vec![r]);
    let result = validate_windows_input(input);
    let r = &rows(&result.routes4).rows[0];
    assert_eq!(r.route_metric, Fact::Known(0));
    assert!(matches!(r.interface_metric, Fact::Unknown(_)));
    assert!(matches!(r.next_hop, Fact::Unknown(_)));
}
#[test]
fn interface_and_ras_row_limits_are_inclusive_then_fail_whole_source() {
    for count in [MAX_INTERFACE_ROWS, MAX_INTERFACE_ROWS + 1] {
        let mut input = empty();
        input.adapters = source(
            (0..count)
                .map(|i| adapter(&format!("vpn{i}"), i as u64 + 1, 53))
                .collect(),
        );
        input.ras = source(
            (0..count)
                .map(|i| WindowsRasObservation {
                    name: Fact::Known(format!("RAS{i}")),
                    all_users: false,
                    interface: unknown(),
                })
                .collect(),
        );
        let result = validate_windows_input(input);
        if count == MAX_INTERFACE_ROWS {
            assert_eq!(rows(&result.adapters).rows.len(), count);
            assert_eq!(rows(&result.ras).rows.len(), count);
        } else {
            assert!(matches!(result.adapters, Fact::Unknown(_)));
            assert!(matches!(result.ras, Fact::Unknown(_)));
        }
    }
}
#[test]
fn route_limits_are_per_family_and_do_not_truncate() {
    for count in [MAX_ROUTE_ROWS, MAX_ROUTE_ROWS + 1] {
        let mut input = empty();
        input.routes4 = source(vec![route(AddressFamily::V4, "0.0.0.0/0"); count]);
        input.routes6 = source(vec![route(AddressFamily::V6, "::/0"); count]);
        let result = validate_windows_input(input);
        if count == MAX_ROUTE_ROWS {
            assert_eq!(rows(&result.routes4).rows.len(), count);
            assert_eq!(rows(&result.routes6).rows.len(), count);
        } else {
            assert!(matches!(result.routes4, Fact::Unknown(_)));
            assert!(matches!(result.routes6, Fact::Unknown(_)));
        }
    }
}
#[test]
fn address_caps_include_duplicate_rows_and_both_families_of_same_luid() {
    for count in [MAX_INTERFACE_ADDRESSES, MAX_INTERFACE_ADDRESSES + 1] {
        let mut input = empty();
        input.addresses = source(
            (0..count)
                .map(|i| {
                    if i % 2 == 0 {
                        address(AddressFamily::V4, "10.8.0.2", 24)
                    } else {
                        address(AddressFamily::V6, "fd00::2", 64)
                    }
                })
                .collect(),
        );
        let result = validate_windows_input(input);
        if count == MAX_INTERFACE_ADDRESSES {
            assert_eq!(rows(&result.addresses).rows.len(), count);
        } else {
            assert!(matches!(result.addresses, Fact::Unknown(_)));
        }
    }
    // Limit accounting may group a reported alias conservatively, without proving
    // a cross-family association or filling a missing LUID.
    let mut input = empty();
    input.addresses = source(
        (0..MAX_INTERFACE_ADDRESSES + 1)
            .map(|i| {
                let mut a = if i % 2 == 0 {
                    address(AddressFamily::V4, "10.8.0.2", 24)
                } else {
                    address(AddressFamily::V6, "fd00::2", 64)
                };
                a.interface.luid = unknown();
                a.interface.if_index = Fact::Known(if i % 2 == 0 { 7 } else { 8 });
                a
            })
            .collect(),
    );
    assert!(matches!(
        validate_windows_input(input).addresses,
        Fact::Unknown(_)
    ));
    for count in [
        MAX_INTERFACE_ROWS * MAX_INTERFACE_ADDRESSES,
        MAX_INTERFACE_ROWS * MAX_INTERFACE_ADDRESSES + 1,
    ] {
        let mut input = empty();
        let mut a = address(AddressFamily::V4, "10.8.0.2", 24);
        a.interface = WindowsInterfaceRef {
            alias: Fact::Unknown(String::new()),
            luid: Fact::Unknown(String::new()),
            if_index: Fact::Unknown(String::new()),
        };
        input.addresses = source(vec![a; count]);
        let result = validate_windows_input(input);
        if count == MAX_INTERFACE_ROWS * MAX_INTERFACE_ADDRESSES {
            assert_eq!(rows(&result.addresses).rows.len(), count);
            assert!(matches!(rows(&result.addresses).complete, Fact::Unknown(_)));
        } else {
            assert!(matches!(result.addresses, Fact::Unknown(_)));
        }
    }
}
#[test]
fn byte_cap_counts_utf8_unknown_diagnostics_and_all_five_sources() {
    let big = "界".repeat(MAX_SOURCE_BYTES / 3 + 1);
    let mut input = empty();
    let mut a = adapter("vpn0", 1, 53);
    a.description = Fact::Known(big.clone());
    input.adapters = source(vec![a]);
    let mut a = address(AddressFamily::V4, "10.8.0.2", 24);
    a.prefix_len = Fact::Unknown(big.clone());
    input.addresses = source(vec![a]);
    let mut r = route(AddressFamily::V4, "0.0.0.0/0");
    r.route_metric = Fact::Unknown(big.clone());
    input.routes4 = source(vec![r]);
    let mut r = route(AddressFamily::V6, "::/0");
    r.interface.alias = Fact::Known(big.clone());
    input.routes6 = source(vec![r]);
    input.ras = source(vec![WindowsRasObservation {
        name: Fact::Known(big),
        all_users: false,
        interface: unknown(),
    }]);
    let result = validate_windows_input(input);
    assert!(matches!(result.adapters, Fact::Unknown(_)));
    assert!(matches!(result.addresses, Fact::Unknown(_)));
    assert!(matches!(result.routes4, Fact::Unknown(_)));
    assert!(matches!(result.routes6, Fact::Unknown(_)));
    assert!(matches!(result.ras, Fact::Unknown(_)));
    let mut input = empty();
    input.adapters = Fact::Unknown("x".repeat(MAX_SOURCE_BYTES + 1));
    let result = validate_windows_input(input);
    let Fact::Unknown(reason) = result.adapters else {
        panic!()
    };
    assert!(reason.len() < MAX_SOURCE_BYTES);
}
#[test]
fn byte_cap_is_inclusive_and_is_rechecked_after_normalization() {
    for extra in [0, 1] {
        let mut input = empty();
        let mut a = adapter("vpn0", 1, 53);
        a.description = Fact::Known(String::new());
        let used = a.bytes();
        a.description = Fact::Known("x".repeat(MAX_SOURCE_BYTES - used + extra));
        input.adapters = source(vec![a]);
        let result = validate_windows_input(input);
        if extra == 0 {
            assert_eq!(rows(&result.adapters).rows.len(), 1);
        } else {
            assert!(matches!(result.adapters, Fact::Unknown(_)));
        }
    }
    let mut input = empty();
    let mut r = route(AddressFamily::V4, "10.8.0.2");
    r.next_hop = Fact::Unknown(String::new());
    r.route_metric = Fact::Known(1);
    r.interface_metric = Fact::Known(1);
    r.next_hop = Fact::Unknown("x".repeat(MAX_SOURCE_BYTES - r.bytes()));
    input.routes4 = source(vec![r]);
    assert!(matches!(
        validate_windows_input(input).routes4,
        Fact::Unknown(_)
    ));
}
const CAPTURES: [&str; 7] = [
    "windows-w207-routes-ts-off-2026-09-12.txt",
    "windows-w207-wintun-present-2026-09-12.txt",
    "windows-w207-ts-on-2026-09-12.txt",
    "windows-w207-ovpn-tun-connected-2026-09-13.txt",
    "windows-w207-ras-l2tp-connected-2026-09-13.txt",
    "windows-w207-hyperv-present-2026-09-13.txt",
    "windows-w207-virtualization-stack-2026-09-13.txt",
];
fn section(file: &str, id: &str) -> String {
    let raw = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/route_probe/tests/fixtures")
            .join(file),
    )
    .unwrap();
    let mut active = false;
    let mut found = false;
    let mut out = String::new();
    for line in raw.lines() {
        if line.starts_with("@@@") {
            if active {
                break;
            }
            active = line == format!("@@@{id}");
            found |= active;
        } else if active {
            out.push_str(line);
            out.push('\n');
        }
    }
    assert!(found, "{file}: {id}");
    out
}
#[test]
fn seven_existing_captures_inject_only_legacy_observations_never_complete_new_facts() {
    for file in CAPTURES {
        let names = parse_get_netipaddress(&section(file, "GET_NETIPADDRESS")).unwrap();
        let mut input = empty();
        input.adapters = unknown();
        input.addresses = unknown();
        input.ras = unknown();
        for (id, family) in [
            ("ROUTE_PRINT_4", AddressFamily::V4),
            ("ROUTE_PRINT_6", AddressFamily::V6),
        ] {
            let legacy = parse_route_print_routes(&section(file, id), &names).unwrap();
            assert!(!legacy.is_empty());
            let source = Fact::Known(ReadRows {
                rows: legacy
                    .into_iter()
                    .map(|r| WindowsRouteObservation {
                        interface: WindowsInterfaceRef {
                            alias: Fact::Known(r.interface),
                            luid: unknown(),
                            if_index: unknown(),
                        },
                        family,
                        prefix: r.prefix,
                        next_hop: unknown(),
                        next_hop_scope_id: unknown(),
                        route_metric: unknown(),
                        interface_metric: unknown(),
                    })
                    .collect(),
                complete: unknown(),
                compartment: unknown(),
                error: Fact::Known(None),
            });
            if family == AddressFamily::V4 {
                input.routes4 = source;
            } else {
                input.routes6 = source;
            }
        }
        let result = validate_windows_input(input);
        for source in [&result.routes4, &result.routes6] {
            let s = rows(source);
            assert!(!s.rows.is_empty());
            assert!(matches!(s.complete, Fact::Unknown(_)));
            assert!(matches!(s.compartment, Fact::Unknown(_)));
            assert!(s
                .rows
                .iter()
                .all(|r| matches!(r.route_metric, Fact::Unknown(_))
                    && matches!(r.interface_metric, Fact::Unknown(_))
                    && matches!(r.interface.luid, Fact::Unknown(_))));
        }
        assert!(matches!(result.addresses, Fact::Unknown(_)));
        assert!(matches!(result.ras, Fact::Unknown(_)));
    }
}

#[test]
fn source_error_never_proves_complete_or_authoritative_empty() {
    for error in [Fact::Known(Some("permission denied".into())), unknown()] {
        let mut input = empty();
        let Fact::Known(ref mut source) = input.routes4 else {
            panic!()
        };
        source.error = error;
        let result = validate_windows_input(input);
        let source = rows(&result.routes4);
        assert!(matches!(source.complete, Fact::Unknown(_)));
        assert!(matches!(source.proven_empty(), Fact::Unknown(_)));
    }
}
