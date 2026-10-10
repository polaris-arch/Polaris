//! Read-only macOS fact collection and bounded decoding of existing captures.
//!
//! Retains rows that the legacy advisory probe drops (including I-scoped defaults).
//! Observations do not establish coverage, selected egress, resource role, tunnel
//! identity or route selection. The collector queries both families and an interface
//! roster; missing evidence remains Unknown. This is a manual diagnostics provider,
//! not runtime reprobe, classifier, notification or egress projection.

use super::{MAX_INTERFACE_ADDRESSES, MAX_INTERFACE_ROWS, MAX_ROUTE_ROWS, MAX_SOURCE_BYTES};
use crate::exec::{Command, CommandRunner};
use crate::route_probe::expand_netstat_destination;
use polaris_config_engine::builder::coexistence::{
    AddressFamily, Fact, InterfaceAddress, ObjectFacts, RouteFact, RouteScope,
};
use polaris_config_engine::user_config::cidr::normalize_cidr;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// Three sequential read-only queries through the caller's runner. Two seconds is
/// the requested operation budget PER command, not a total deadline or pipe-memory
/// cap. The snapshot IPC supplies its original observed-command custody runner.
/// No fallback, host mutation, own-interface inference or automatic retry.
///
/// `ifconfig -a` supplies the roster and actual host addresses. Route association
/// requires its exact printed interface. Even an observed point-to-point flag is
/// not a proof of VPN identity, coverage or selected egress.
pub fn collect_macos(runner: &impl CommandRunner) -> Fact<Vec<ObjectFacts>> {
    let read = |program: &str, args: &[&str]| {
        runner
            .run(
                &Command::new(program, args.iter().copied()),
                Duration::from_secs(2),
            )
            .map(|output| output.stdout)
    };
    let routes4 =
        parse_route_observations(read("netstat", &["-rn", "-f", "inet"]), AddressFamily::V4);
    let routes6 =
        parse_route_observations(read("netstat", &["-rn", "-f", "inet6"]), AddressFamily::V6);
    let interfaces = read("ifconfig", &["-a"]).and_then(decode_interfaces);
    let mut objects = match interfaces {
        Ok(objects) => objects,
        Err(reason) => return Fact::Unknown(format!("macOS ifconfig roster: {reason}")),
    };
    let routes = match (routes4, routes6) {
        (Fact::Known(mut v4), Fact::Known(v6)) => {
            v4.extend(v6);
            if v4.iter().any(|row| !objects.contains_key(&row.interface)) {
                Fact::Unknown(
                    "route interface absent from ifconfig roster; non-atomic observations".into(),
                )
            } else {
                Fact::Known(v4)
            }
        }
        (Fact::Unknown(reason), _) | (_, Fact::Unknown(reason)) => Fact::Unknown(reason),
    };
    for object in objects.values_mut() {
        object.routes = match &routes {
            Fact::Known(rows) => Fact::Known(
                rows.iter()
                    .filter(|row| row.interface == object.interface)
                    .map(|row| RouteFact {
                        prefix: row.prefix.clone(),
                        table: Fact::Known(None),
                        scope: row.scope.clone(),
                        role: Fact::Unknown(
                            "route role and selection not established by netstat".into(),
                        ),
                    })
                    .collect(),
            ),
            Fact::Unknown(reason) => Fact::Unknown(reason.clone()),
        };
    }
    Fact::Known(objects.into_values().collect())
}

fn valid_interface(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_alphabetic())
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
}

// A complete roster is never obtained from the legacy best-effort flags parser:
// malformed top-level rows, duplicates and missing headers must not be skipped.
// Nested bridge member lines stay in their parent block. Unknown address evidence
// fails only that interface's address field, never replaces it with a partial list.
fn decode_interfaces(raw: String) -> Result<BTreeMap<String, ObjectFacts>, String> {
    if raw.len() > MAX_SOURCE_BYTES {
        return Err("source exceeds byte limit".into());
    }
    let mut objects = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut address_count = 0usize;
    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            let (name, rest) = line
                .split_once(':')
                .ok_or_else(|| format!("malformed interface header at line {}", index + 1))?;
            let flag = rest
                .split_whitespace()
                .next()
                .and_then(|token| token.strip_prefix("flags="));
            let valid_flags =
                flag.and_then(|v| v.split_once('<'))
                    .is_some_and(|(number, letters)| {
                        !number.is_empty()
                            && number.bytes().all(|b| b.is_ascii_hexdigit())
                            && letters.strip_suffix('>').is_some_and(|v| {
                                v.is_empty()
                                    || v.split(',').all(|f| {
                                        !f.is_empty()
                                            && f.bytes().all(|b| {
                                                b.is_ascii_uppercase()
                                                    || b.is_ascii_digit()
                                                    || b == b'_'
                                            })
                                    })
                            })
                    });
            if !valid_interface(name) || !valid_flags {
                return Err(format!("malformed interface header at line {}", index + 1));
            }
            if objects.contains_key(name) {
                return Err("duplicate interface header".into());
            }
            if objects.len() == MAX_INTERFACE_ROWS {
                return Err("source exceeds interface limit".into());
            }
            current = Some(name.to_string());
            address_count = 0;
            objects.insert(
                name.to_string(),
                ObjectFacts {
                    interface: name.to_string(),
                    tunnel: Fact::Unknown("interface flags do not establish VPN identity".into()),
                    virtualization: Fact::Unknown("virtualization not established".into()),
                    addresses: Fact::Known(Vec::new()),
                    routes: Fact::Unknown("routes not assembled".into()),
                    policy_rules: Fact::Unknown("macOS policy source not observed".into()),
                    stable_identity: Fact::Unknown("stable repair identity not established".into()),
                },
            );
        } else {
            let name = current
                .as_ref()
                .ok_or("interface continuation before header")?;
            let tokens: Vec<_> = line.split_whitespace().collect();
            if !matches!(tokens.first(), Some(&"inet" | &"inet6")) {
                continue;
            }
            address_count += 1;
            let object = objects.get_mut(name).ok_or("interface block unavailable")?;
            if address_count > MAX_INTERFACE_ADDRESSES {
                object.addresses = Fact::Unknown("macOS interface exceeds address limit".into());
                continue;
            }
            match decode_address(&tokens, name) {
                Ok(address) => {
                    if let Fact::Known(rows) = &mut object.addresses {
                        rows.push(address);
                    }
                }
                Err(reason) => {
                    object.addresses = Fact::Unknown(format!(
                        "macOS {name} address at line {}: {reason}",
                        index + 1
                    ))
                }
            }
        }
    }
    if objects.is_empty() {
        return Err("missing interface headers".into());
    }
    Ok(objects)
}

fn decode_address(tokens: &[&str], interface: &str) -> Result<InterfaceAddress, String> {
    let token = tokens.get(1).ok_or("missing host address")?;
    let (address, zone) = token
        .split_once('%')
        .map_or((*token, None), |(ip, zone)| (ip, Some(zone)));
    if zone.is_some_and(|zone| zone != interface) {
        return Err("address zone differs from interface".into());
    }
    let address: IpAddr = address.parse().map_err(|_| "invalid host address")?;
    let field = |key| {
        let positions: Vec<_> = tokens
            .iter()
            .enumerate()
            .filter_map(|(i, v)| (*v == key).then_some(i))
            .collect();
        match positions.as_slice() {
            [i] => tokens.get(i + 1).copied().ok_or("missing prefix value"),
            _ => Err("missing or duplicate prefix field"),
        }
    };
    let prefix_len = match (tokens[0], address) {
        ("inet", IpAddr::V4(_)) if zone.is_none() => {
            let mask = field("netmask")?;
            let number = if let Some(hex) = mask.strip_prefix("0x") {
                if hex.len() != 8 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("invalid IPv4 mask".into());
                }
                u32::from_str_radix(hex, 16).map_err(|_| "invalid IPv4 mask")?
            } else {
                u32::from(mask.parse::<Ipv4Addr>().map_err(|_| "invalid IPv4 mask")?)
            };
            if number.leading_ones() + number.trailing_zeros() != 32 {
                return Err("non-contiguous IPv4 mask".into());
            }
            u8::try_from(number.leading_ones()).map_err(|_| "invalid IPv4 prefix")?
        }
        ("inet6", IpAddr::V6(_)) => {
            let bits = field("prefixlen")?;
            if bits.is_empty() || !bits.bytes().all(|b| b.is_ascii_digit()) {
                return Err("invalid IPv6 prefix".into());
            }
            bits.parse::<u8>()
                .ok()
                .filter(|bits| *bits <= 128)
                .ok_or("invalid IPv6 prefix")?
        }
        _ => return Err("host address family differs from source".into()),
    };
    Ok(InterfaceAddress {
        address,
        prefix_len,
    })
}

/// One observed row, attributed only to its printed interface and supplied family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacosRouteObservation {
    pub interface: String,
    pub family: AddressFamily,
    pub prefix: String,
    /// Raw token: it may be an IP, scoped IPv6 address, MAC, or link#N.
    pub gateway: String,
    /// Case-sensitive printed flags, retained even when their meaning is unknown.
    pub flags: String,
    /// Printed interface-scope flag, not a route-selection or reachability proof.
    pub scope: Fact<RouteScope>,
}

/// Decode one completed family-specific source. Family is query provenance, not a
/// gateway guess. Failure, mixed families, malformed input and limits fail the whole
/// affected source to Unknown; no partial list or truncation is returned. A valid
/// explicit header with zero rows is Known empty. Byte cap is post-source buffering.
pub fn parse_route_observations(
    output: Result<String, String>,
    family: AddressFamily,
) -> Fact<Vec<MacosRouteObservation>> {
    match decode(output, family) {
        Ok(rows) => Fact::Known(rows),
        Err(reason) => Fact::Unknown(format!("macOS netstat {family:?}: {reason}")),
    }
}

fn decode(
    output: Result<String, String>,
    family: AddressFamily,
) -> Result<Vec<MacosRouteObservation>, String> {
    let raw = output?;
    if raw.len() > MAX_SOURCE_BYTES {
        return Err("source exceeds byte limit".into());
    }
    let expected_banner = match family {
        AddressFamily::V4 => "Internet:",
        AddressFamily::V6 => "Internet6:",
    };
    let mut banner_seen = false;
    let mut columns: Option<(usize, usize, usize, usize)> = None;
    let mut rows = Vec::new();
    for (line, raw_line) in raw.lines().enumerate() {
        let text = raw_line.trim();
        if text.is_empty() {
            continue;
        }
        if text == "Routing tables" && columns.is_none() && !banner_seen {
            continue;
        }
        if matches!(text, "Internet:" | "Internet6:") {
            if text != expected_banner || banner_seen || columns.is_some() {
                return Err("mismatched or repeated family section".into());
            }
            banner_seen = true;
            continue;
        }
        let tokens: Vec<_> = text.split_whitespace().collect();
        if tokens.first() == Some(&"Destination") {
            if columns.is_some() {
                return Err("repeated column header".into());
            }
            let column = |name| {
                let positions: Vec<_> = tokens
                    .iter()
                    .enumerate()
                    .filter_map(|(i, token)| (*token == name).then_some(i))
                    .collect();
                match positions.as_slice() {
                    [index] => Ok(*index),
                    _ => Err(format!("missing or duplicate {name} column")),
                }
            };
            column("Destination")?;
            columns = Some((
                column("Gateway")?,
                column("Flags")?,
                column("Netif")?,
                tokens.len(),
            ));
            continue;
        }
        let Some((gateway, flags, netif, width)) = columns else {
            return Err(format!(
                "unrecognized source before header at line {}",
                line + 1
            ));
        };
        if tokens.len() <= gateway.max(flags).max(netif) || tokens.len() > width {
            return Err(format!("malformed row width at line {}", line + 1));
        }
        if rows.len() == MAX_ROUTE_ROWS {
            return Err("source exceeds row limit".into());
        }
        let interface = tokens[netif];
        if !interface.chars().any(|c| c.is_ascii_alphabetic())
            || !interface
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.'))
        {
            return Err(format!("invalid interface token at line {}", line + 1));
        }
        let prefix = if tokens[0] == "default" {
            match family {
                AddressFamily::V4 => "0.0.0.0/0".into(),
                AddressFamily::V6 => "::/0".into(),
            }
        } else {
            let address = tokens[0].split('/').next().ok_or("missing destination")?;
            if !address.contains(':')
                && address
                    .split('.')
                    .any(|part| part.len() > 1 && part.starts_with('0'))
            {
                return Err(format!("ambiguous IPv4 destination at line {}", line + 1));
            }
            if let Some((ip, zone)) = address.split_once('%') {
                if !ip.contains(':')
                    || zone.is_empty()
                    || !zone
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
                {
                    return Err(format!("invalid destination zone at line {}", line + 1));
                }
            }
            if tokens[0].split_once('/').is_some_and(|(_, bits)| {
                bits.is_empty() || !bits.bytes().all(|b| b.is_ascii_digit())
            }) {
                return Err(format!("invalid prefix length at line {}", line + 1));
            }
            expand_netstat_destination(tokens[0])
                .and_then(|prefix| normalize_cidr(&prefix))
                .ok_or_else(|| format!("invalid destination at line {}", line + 1))?
        };
        let address: IpAddr = prefix
            .split_once('/')
            .ok_or("missing prefix length")?
            .0
            .parse()
            .map_err(|_| "invalid address")?;
        if !matches!(
            (family, address),
            (AddressFamily::V4, IpAddr::V4(_)) | (AddressFamily::V6, IpAddr::V6(_))
        ) {
            return Err(format!("wrong-family destination at line {}", line + 1));
        }
        rows.push(MacosRouteObservation {
            interface: interface.into(),
            family,
            prefix,
            gateway: tokens[gateway].into(),
            flags: tokens[flags].into(),
            scope: scope(tokens[flags]),
        });
    }
    if columns.is_none() {
        return Err("missing column header".into());
    }
    Ok(rows)
}

fn scope(flags: &str) -> Fact<RouteScope> {
    // Apple's network_cmds/netstat.tproj/route.c bits[] defines printed letters.
    // I = RTF_IFSCOPE; lower-case i is RTF_IFREF and must not be conflated.
    // https://github.com/apple-oss-distributions/network_cmds/blob/main/netstat.tproj/route.c
    const KNOWN: &str = "UGHRDMmdCXLS12Wc3BbIiYrg";
    if flags.is_empty() || flags.len() > KNOWN.len() || !flags.chars().all(|c| KNOWN.contains(c)) {
        Fact::Unknown("unrecognized printed route flags".into())
    } else if flags.contains('I') {
        Fact::Known(RouteScope::InterfaceScoped)
    } else if flags.len() >= 10 {
        // p_flags(..., "%-10.10s ") clips late letters. Observed I is positive
        // evidence above; absence at the print precision cannot prove Global.
        Fact::Unknown("printed flags may truncate the interface-scope bit".into())
    } else {
        Fact::Known(RouteScope::Global)
    }
}

#[cfg(test)]
mod tests;
