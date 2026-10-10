//! Bounded decoding of existing `netstat -rn -f inet/inet6` captures.
//!
//! Retains rows that the legacy advisory probe drops (including I-scoped defaults).
//! Observations do not establish coverage, selected egress, resource role, tunnel
//! identity, addresses, or a complete ObjectFacts snapshot. A future assembly must
//! supply Unknown for roles without independent evidence. No OS command is run here.

use super::{MAX_ROUTE_ROWS, MAX_SOURCE_BYTES};
use crate::route_probe::expand_netstat_destination;
use polaris_config_engine::builder::coexistence::{AddressFamily, Fact, RouteScope};
use polaris_config_engine::user_config::cidr::normalize_cidr;
use std::net::IpAddr;

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
