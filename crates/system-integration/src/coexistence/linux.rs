use polaris_config_engine::builder::coexistence::{
    AddressFamily, Fact, InterfaceAddress, ObjectFacts, PolicyRuleFact, PolicySelectorScope,
    RouteFact, RouteRole, RouteScope,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

type Rows = Vec<Map<String, Value>>;

fn rows(source: &str, output: Result<String, String>) -> Result<Rows, String> {
    let raw = output.map_err(|e| format!("{source}: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("{source}: invalid JSON enumeration: {e}"))
}

fn text<'a>(row: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    row.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("missing/invalid {key}"))
}

fn number(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn table(row: &Map<String, Value>) -> Fact<Option<u32>> {
    match row.get("table") {
        Some(value) => match number(value).or_else(|| match value.as_str()? {
            "main" => Some(254),
            "local" => Some(255),
            "default" => Some(253),
            _ => None,
        }) {
            Some(v) if v > 0 => Fact::Known(Some(v)),
            _ => Fact::Unknown("unresolved table ID".into()),
        },
        None => Fact::Unknown("table ID missing despite detailed all-table query".into()),
    }
}

fn prefix(raw: &str, family: AddressFamily) -> Result<String, String> {
    if raw == "default" {
        return Ok(match family {
            AddressFamily::V4 => "0.0.0.0/0",
            AddressFamily::V6 => "::/0",
        }
        .into());
    }
    let (ip, bits) = raw
        .split_once('/')
        .map_or((raw, None), |(ip, bits)| (ip, Some(bits)));
    let ip: IpAddr = ip.parse().map_err(|_| format!("invalid prefix {raw}"))?;
    let width = match (ip, family) {
        (IpAddr::V4(_), AddressFamily::V4) => 32,
        (IpAddr::V6(_), AddressFamily::V6) => 128,
        _ => return Err(format!("wrong-family prefix {raw}")),
    };
    let bits = match bits {
        None => width,
        Some(b) if !b.is_empty() && b.bytes().all(|c| c.is_ascii_digit()) => b
            .parse::<u8>()
            .map_err(|_| format!("invalid prefix {raw}"))?,
        _ => return Err(format!("invalid prefix {raw}")),
    };
    if bits > width {
        return Err(format!("invalid prefix {raw}"));
    }
    Ok(format!("{ip}/{bits}"))
}

fn addresses(rows: &Rows, interface: &str) -> Result<Vec<InterfaceAddress>, String> {
    let mut found = false;
    let mut result = Vec::new();
    for row in rows {
        if text(row, "ifname")? != interface {
            continue;
        }
        found = true;
        let info = row
            .get("addr_info")
            .and_then(Value::as_array)
            .ok_or("addr_info missing/invalid")?;
        for item in info {
            let item = item.as_object().ok_or("invalid address row")?;
            let family = match text(item, "family")? {
                "inet" => AddressFamily::V4,
                "inet6" => AddressFamily::V6,
                _ => return Err("unsupported address family".into()),
            };
            let bits = item
                .get("prefixlen")
                .and_then(number)
                .ok_or("prefixlen missing/invalid")?;
            let normalized = prefix(&format!("{}/{}", text(item, "local")?, bits), family)?;
            let (ip, bits) = normalized.split_once('/').ok_or("invalid address")?;
            result.push(InterfaceAddress {
                address: ip.parse().map_err(|_| "invalid address")?,
                prefix_len: bits.parse().map_err(|_| "invalid prefixlen")?,
            });
        }
    }
    if !found {
        return Err(
            "interface absent from address enumeration (possibly changed during collection)".into(),
        );
    }
    Ok(result)
}

fn routes(rows: &Rows, interface: &str, family: AddressFamily) -> Result<Vec<RouteFact>, String> {
    let mut result = Vec::new();
    for row in rows {
        // Multipath/nexthop-ID routes cannot establish a unique object link. Never
        // attribute the first next hop to the entire route or silently drop evidence.
        if row.contains_key("nexthops") || row.contains_key("nhid") {
            return Err("multipath/nexthop-ID route attribution unsupported".into());
        }
        let kind = match row.get("type") {
            None => "unicast",
            Some(value) => value.as_str().ok_or("invalid route type")?,
        };
        if matches!(kind, "blackhole" | "unreachable" | "prohibit" | "throw") {
            continue;
        }
        if text(row, "dev")? != interface {
            continue;
        }
        if matches!(kind, "local" | "broadcast") {
            continue;
        }
        if kind != "unicast" {
            return Err("unsupported route type".into());
        }
        let dst = prefix(text(row, "dst")?, family)?;
        let dst = polaris_config_engine::user_config::cidr::normalize_cidr(&dst)
            .ok_or("invalid normalized route prefix")?;
        let mut scope = Fact::Known(RouteScope::Global);
        // Linux route scope link/host is reachability, not Darwin RTF_IFSCOPE.
        // Unknown input flags may encode route applicability we cannot establish.
        if row
            .get("flags")
            .is_some_and(|v| v.as_array().is_none_or(|v| !v.is_empty()))
            || row.contains_key("from")
            || row.contains_key("src")
            || row.contains_key("tos")
            || row.contains_key("encap")
        {
            scope = Fact::Unknown("route applicability/flags unsupported".into());
        }
        result.push(RouteFact {
            prefix: dst,
            table: table(row),
            scope,
            role: Fact::Known(RouteRole::ResourceClaim),
        });
    }
    // Build a complete witness within ONE table/family/interface. CIDR intervals
    // are nested or disjoint, so widest-first at each start excludes extra resource
    // claims from the witness. A /0 never makes its nested business routes harmless.
    let mut by_table: BTreeMap<u32, Vec<(u128, u128, String)>> = BTreeMap::new();
    let max = match family {
        AddressFamily::V4 => u128::from(u32::MAX),
        AddressFamily::V6 => u128::MAX,
    };
    for route in &result {
        if let (Fact::Known(Some(table)), Fact::Known(RouteScope::Global)) =
            (&route.table, &route.scope)
        {
            let (address, bits) = route
                .prefix
                .split_once('/')
                .ok_or("invalid normalized prefix")?;
            let address: IpAddr = address.parse().map_err(|_| "invalid normalized address")?;
            let (start, width) = match address {
                IpAddr::V4(ip) => (u128::from(u32::from(ip)), 32u8),
                IpAddr::V6(ip) => (u128::from(ip), 128u8),
            };
            let bits: u8 = bits
                .parse()
                .map_err(|_| "invalid normalized prefix length")?;
            let hosts = width - bits;
            let tail = if hosts == 128 {
                u128::MAX
            } else {
                (1u128 << hosts) - 1
            };
            by_table
                .entry(*table)
                .or_default()
                .push((start, start | tail, route.prefix.clone()));
        }
    }
    let mut witnesses = BTreeSet::new();
    for (table, mut intervals) in by_table {
        intervals.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
        let mut next = 0;
        let mut members = Vec::new();
        let mut complete = false;
        for (start, end, prefix) in intervals {
            if start > next {
                break;
            }
            if end < next {
                continue;
            }
            members.push((table, prefix));
            if end == max {
                complete = true;
                break;
            }
            next = end + 1;
        }
        if complete {
            witnesses.extend(members);
        }
    }
    for route in &mut result {
        let witnessed = matches!(&route.table, Fact::Known(Some(t)) if witnesses.contains(&(*t, route.prefix.clone())));
        if route.prefix.ends_with("/0") || witnessed {
            route.role = Fact::Known(RouteRole::CoverageDeclaration);
        } else if route.prefix.ends_with("/1") {
            route.role = Fact::Unknown("split coverage lacks same-table/family witness".into());
        }
    }
    Ok(result)
}

fn selectors(row: &Map<String, Value>, family: AddressFamily) -> Fact<PolicySelectorScope> {
    // Positive whitelist: new iproute2 selectors cannot silently become Global.
    let metadata = [
        "priority", "table", "protocol", "src", "dst", "srclen", "dstlen",
    ];
    let restrictions = [
        "fwmark",
        "fwmask",
        "iif",
        "oif",
        "uid_start",
        "uid_end",
        "ipproto",
        "sport",
        "sport_start",
        "sport_end",
        "sport_mask",
        "dport",
        "dport_start",
        "dport_end",
        "dport_mask",
        "tos",
        "not",
        "l3mdev",
        "suppress_prefixlen",
        "suppress_ifgroup",
        "dscp",
        "flowlabel",
        "tun_id",
    ];
    if row
        .keys()
        .any(|k| !metadata.contains(&k.as_str()) && !restrictions.contains(&k.as_str()))
    {
        return Fact::Unknown("unrecognized rule selector/action".into());
    }
    if row.keys().any(|k| restrictions.contains(&k.as_str())) {
        return Fact::Known(PolicySelectorScope::Limited(
            "rule filter/suppression present".into(),
        ));
    }
    if !row.contains_key("src") {
        return Fact::Unknown("rule source selector missing".into());
    }
    for (key, len) in [("src", "srclen"), ("dst", "dstlen")] {
        if let Some(value) = row.get(key) {
            let Some(raw) = value.as_str() else {
                return Fact::Unknown("invalid rule selector".into());
            };
            let global = (raw == "all" && row.get(len).is_none_or(|v| number(v) == Some(0)))
                || match family {
                    AddressFamily::V4 => {
                        raw == "0.0.0.0" && row.get(len).and_then(number) == Some(0)
                    }
                    AddressFamily::V6 => raw == "::" && row.get(len).and_then(number) == Some(0),
                };
            if !global {
                return Fact::Known(PolicySelectorScope::Limited(format!("{key} restricted")));
            }
        } else if row.contains_key(len) {
            return Fact::Unknown("selector length without selector".into());
        }
    }
    Fact::Known(PolicySelectorScope::Global)
}

fn rules(
    rows: &Rows,
    family: AddressFamily,
    routes: &Fact<Vec<RouteFact>>,
) -> Result<Vec<PolicyRuleFact>, String> {
    rows.iter()
        .map(|row| {
            let priority = row
                .get("priority")
                .and_then(number)
                .ok_or("priority missing/invalid")?;
            let lookup = table(row);
            let selector = selectors(row, family);
            let applies = match (&lookup, &selector, routes) {
                (
                    Fact::Known(Some(t)),
                    Fact::Known(PolicySelectorScope::Global),
                    Fact::Known(routes),
                ) => {
                    let mut uncertain = false;
                    let mut hit = false;
                    for route in routes {
                        // Explicitly preserve family: route.prefix has already been validated.
                        let is_v4 = route
                            .prefix
                            .split('/')
                            .next()
                            .and_then(|p| p.parse::<IpAddr>().ok())
                            .is_some_and(|p| p.is_ipv4());
                        if is_v4 != (family == AddressFamily::V4) {
                            continue;
                        }
                        match (&route.table, &route.scope) {
                            (Fact::Known(Some(table)), Fact::Known(RouteScope::Global))
                                if t == table =>
                            {
                                hit = true
                            }
                            (Fact::Unknown(_), _) | (_, Fact::Unknown(_)) => uncertain = true,
                            _ => {}
                        }
                    }
                    if hit {
                        Fact::Known(true)
                    } else if uncertain {
                        Fact::Unknown("route/table association unresolved".into())
                    } else {
                        Fact::Known(false)
                    }
                }
                _ => Fact::Unknown(
                    "selector applicability or rule-table-interface link unproved".into(),
                ),
            };
            Ok(PolicyRuleFact {
                priority,
                lookup_table: lookup,
                address_family: Fact::Known(family),
                selector_scope: selector,
                applies_to_object: applies,
            })
        })
        .collect()
}

fn fact<T>(value: Result<T, String>, source: &str) -> Fact<T> {
    match value {
        Ok(v) => Fact::Known(v),
        Err(e) => Fact::Unknown(format!("{source}: {e}")),
    }
}

pub(super) fn assemble(
    links: Result<String, String>,
    addr: Result<String, String>,
    r4: Result<String, String>,
    r6: Result<String, String>,
    p4: Result<String, String>,
    p6: Result<String, String>,
) -> Fact<Vec<ObjectFacts>> {
    let links = match rows("ip link", links) {
        Ok(v) => v,
        Err(e) => return Fact::Unknown(e),
    };
    let addr = rows("ip address", addr);
    let r4 = rows("ip -4 route table all", r4);
    let r6 = rows("ip -6 route table all", r6);
    let p4 = rows("ip -4 rule", p4);
    let p6 = rows("ip -6 rule", p6);
    let mut objects = BTreeMap::new();
    for link in links {
        let name = match text(&link, "ifname") {
            Ok(v) => v.to_string(),
            Err(e) => return Fact::Unknown(format!("ip link: {e}")),
        };
        if objects.contains_key(&name) {
            return Fact::Unknown("duplicate interface identity".into());
        }
        let kind = link
            .get("linkinfo")
            .and_then(|v| v.get("info_kind"))
            .and_then(Value::as_str);
        let (tunnel, virtualization) = match kind {
            Some("tun" | "wireguard" | "ovpn") => (Fact::Known(true), Fact::Known(false)),
            Some("bridge" | "veth" | "vlan" | "macvlan" | "ipvlan") => {
                (Fact::Known(false), Fact::Known(true))
            }
            _ => (
                Fact::Unknown("link kind does not prove tunnel absence".into()),
                Fact::Unknown("virtualization not established".into()),
            ),
        };
        let route_result = r4
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|r| routes(r, &name, AddressFamily::V4))
            .and_then(|mut result| {
                result.extend(
                    r6.as_ref()
                        .map_err(Clone::clone)
                        .and_then(|r| routes(r, &name, AddressFamily::V6))?,
                );
                Ok(result)
            });
        let route_facts = fact(route_result, "routes");
        let policy = p4
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|r| rules(r, AddressFamily::V4, &route_facts))
            .and_then(|mut result| {
                result.extend(
                    p6.as_ref()
                        .map_err(Clone::clone)
                        .and_then(|r| rules(r, AddressFamily::V6, &route_facts))?,
                );
                Ok(result)
            });
        objects.insert(
            name.clone(),
            ObjectFacts {
                interface: name.clone(),
                tunnel,
                virtualization,
                addresses: fact(
                    addr.as_ref()
                        .map_err(Clone::clone)
                        .and_then(|r| addresses(r, &name)),
                    "addresses",
                ),
                routes: route_facts,
                policy_rules: fact(policy, "policy rules"),
                stable_identity: Fact::Known(None),
            },
        );
    }
    Fact::Known(objects.into_values().collect())
}
