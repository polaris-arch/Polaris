//! COEX object classification from supplied facts only (governance rules 5.6/6.2).
//!
//! Coverage declarations require a complete witness for the same interface, family and scope;
//! they do not claim every covered address as a resource. Additional resource routes and actual
//! interface addresses still participate in collision detection. This module performs no I/O
//! and does not change the existing tunnel-conflict input, probe or advisory behavior.

#![forbid(unsafe_code)]

use super::tunnel_conflict::ConflictCriteria;
use crate::user_config::cidr::normalize_cidr;
use polaris_helper_proto::Platform;
use std::collections::BTreeSet;
use std::net::IpAddr;

/// An empty successful enumeration is `Known(Vec::new())`, never `Unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact<T> {
    Known(T),
    /// Missing, failed, unsupported or unattributed evidence, with its source/reason.
    Unknown(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservationPhase {
    BeforePolarisTun,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteScope {
    Global,
    InterfaceScoped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteRole {
    CoverageDeclaration,
    ResourceClaim,
}

/// Host bits are deliberately retained for the exact IPv4 proxy signatures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceAddress {
    pub address: IpAddr,
    pub prefix_len: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFact {
    pub prefix: String,
    /// `Known(None)` means that the platform has no table-number concept, not a missing read.
    pub table: Fact<Option<u32>>,
    pub scope: Fact<RouteScope>,
    /// Supplied evidence must distinguish a coverage member from an additional resource claim.
    pub role: Fact<RouteRole>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySelectorScope {
    /// Verified whole-family applicability of from/to/fwmark AND every other rule filter,
    /// including lookup suppressions. A bare `from all` alone is not this witness.
    Global,
    /// A proved restriction; partial applicability does not establish global entry loss.
    Limited(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressFamily {
    V4,
    V6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyRuleFact {
    pub priority: u32,
    pub lookup_table: Fact<Option<u32>>,
    pub address_family: Fact<AddressFamily>,
    /// Independent of object association. Unknown selectors must never default to Global.
    pub selector_scope: Fact<PolicySelectorScope>,
    /// Includes selector applicability AND the rule -> table -> this object's interface link.
    /// It proves applicability to some traffic, NOT whole-family selector coverage.
    /// A collector unable to prove either must supply `Unknown`, not guess by priority/name.
    pub applies_to_object: Fact<bool>,
}

/// Routes/addresses in this record must all belong to `interface`; objects are never merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectFacts {
    pub interface: String,
    pub tunnel: Fact<bool>,
    pub virtualization: Fact<bool>,
    pub addresses: Fact<Vec<InterfaceAddress>>,
    pub routes: Fact<Vec<RouteFact>>,
    pub policy_rules: Fact<Vec<PolicyRuleFact>>,
    /// `Known(None)` means there is no verifiable stable identity, not that history is empty.
    pub stable_identity: Fact<Option<String>>,
}

pub struct ClassificationInput<'a> {
    pub platform: Platform,
    /// Phase of the supplied policy-rule evidence, not a guess from the current app status.
    pub observation_phase: &'a Fact<ObservationPhase>,
    /// Must include both the session TUN and the app's own mesh interfaces.
    pub own_interfaces: &'a Fact<Vec<String>>,
    pub objects: &'a [ObjectFacts],
    /// Already-observed repair records, keyed by verified stable identity; never product names.
    pub forced_repair_identities: &'a Fact<Vec<String>>,
    pub criteria: &'a ConflictCriteria,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectShape {
    Global,
    Scoped,
    Exclusive,
    Bystander,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    Follow,
    Exclusive,
    Bystander,
}

/// Order is the governance table's first-match order, not a configurable policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    OtherTunProxy,
    AddressCollision,
    ForcedRouteRepairHistory,
    EntryCannotBePreserved,
    GlobalCoverage,
    TunnelInterface,
    KnownVirtualization,
    NoObject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    pub shape: ObjectShape,
    pub relation: Relation,
    pub rule: Rule,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryAssessment {
    Matched,
    NotMatched,
    /// No history query can be made for this object; does NOT assert no stored record exists.
    NoVerifiableIdentity,
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    /// Global-scope coverage only. Interface-scoped defaults cannot establish these facts.
    pub ipv4: Fact<bool>,
    pub ipv6: Fact<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassificationReport {
    pub interface: String,
    /// `Known(None)` is either an excluded own interface or the final "do not build" row.
    pub decision: Fact<Option<Classification>>,
    pub excluded_self: bool,
    pub coverage: Coverage,
    pub history: HistoryAssessment,
    /// Every evaluated row, including known facts following an unresolved earlier row.
    pub predicates: Vec<(Rule, Fact<bool>)>,
    /// Malformed prefixes remain visible even when independent earlier evidence is decisive.
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Network {
    family: AddressFamily,
    start: u128,
    end: u128,
    bits: u8,
}

fn network(address: IpAddr, bits: u8) -> Result<Network, String> {
    let (family, width, value) = match address {
        IpAddr::V4(ip) => (AddressFamily::V4, 32, u128::from(u32::from(ip))),
        IpAddr::V6(ip) => (AddressFamily::V6, 128, u128::from(ip)),
    };
    if bits > width {
        return Err(format!("invalid address prefix: {address}/{bits}"));
    }
    let hosts = width - bits;
    let tail = if hosts == 128 {
        u128::MAX
    } else {
        (1u128 << hosts) - 1
    };
    let start = value & !tail;
    Ok(Network {
        family,
        start,
        end: start | tail,
        bits,
    })
}

fn parse_network(raw: &str) -> Result<Network, String> {
    // Reject signed/empty/multiple lengths before the existing strict address normalizer.
    if let Some((_, bits)) = raw.trim().split_once('/') {
        if bits.is_empty() || !bits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("invalid CIDR: {raw:?}"));
        }
    }
    let normalized = normalize_cidr(raw).ok_or_else(|| format!("invalid CIDR: {raw:?}"))?;
    let (address, bits) = normalized
        .split_once('/')
        .ok_or_else(|| format!("invalid CIDR: {raw:?}"))?;
    let address = address
        .parse()
        .map_err(|_| format!("invalid CIDR: {raw:?}"))?;
    let bits = bits.parse().map_err(|_| format!("invalid CIDR: {raw:?}"))?;
    network(address, bits)
}

fn and(a: Fact<bool>, b: Fact<bool>) -> Fact<bool> {
    match (a, b) {
        (Fact::Known(false), _) | (_, Fact::Known(false)) => Fact::Known(false),
        (Fact::Known(true), b) | (b, Fact::Known(true)) => b,
        (Fact::Unknown(a), Fact::Unknown(b)) => Fact::Unknown(format!("{a}; {b}")),
    }
}

fn or(a: Fact<bool>, b: Fact<bool>) -> Fact<bool> {
    match (a, b) {
        (Fact::Known(true), _) | (_, Fact::Known(true)) => Fact::Known(true),
        (Fact::Known(false), b) | (b, Fact::Known(false)) => b,
        (Fact::Unknown(a), Fact::Unknown(b)) => Fact::Unknown(format!("{a}; {b}")),
    }
}

fn not(a: &Fact<bool>) -> Fact<bool> {
    match a {
        Fact::Known(value) => Fact::Known(!value),
        Fact::Unknown(reason) => Fact::Unknown(reason.clone()),
    }
}

fn any(values: impl Iterator<Item = Fact<bool>>) -> Fact<bool> {
    values.fold(Fact::Known(false), or)
}

fn overlaps(a: Network, b: Network) -> bool {
    a.family == b.family && a.start <= b.end && b.start <= a.end
}

fn complete(mut intervals: Vec<Network>, family: AddressFamily) -> bool {
    intervals.sort_by_key(|n| (n.start, n.end));
    let max = match family {
        AddressFamily::V4 => u128::from(u32::MAX),
        AddressFamily::V6 => u128::MAX,
    };
    let mut next = 0;
    for n in intervals {
        if n.start > next {
            return false;
        }
        if n.end == max {
            return true;
        }
        next = next.max(n.end + 1);
    }
    false
}

/// Table and split filters keep Linux entry witnesses within the relevant lookup path.
fn cover(
    object: &ObjectFacts,
    parsed: &[Result<Network, String>],
    family: AddressFamily,
    scope: RouteScope,
    required_table: Option<u32>,
    split_only: bool,
) -> Fact<bool> {
    let routes = match &object.routes {
        Fact::Unknown(reason) => return Fact::Unknown(format!("routes: {reason}")),
        Fact::Known(routes) => routes,
    };
    let mut intervals = Vec::new();
    let mut unresolved = Vec::new();
    for (route, parsed) in routes.iter().zip(parsed) {
        let n = match parsed {
            Ok(n) => *n,
            Err(reason) => {
                unresolved.push(reason.clone());
                continue;
            }
        };
        if n.family != family || (split_only && n.bits == 0) {
            continue;
        }
        let role = match &route.role {
            Fact::Known(role) => Fact::Known(*role == RouteRole::CoverageDeclaration),
            Fact::Unknown(reason) => Fact::Unknown(format!("route role: {reason}")),
        };
        let scoped = match &route.scope {
            Fact::Known(value) => Fact::Known(*value == scope),
            Fact::Unknown(reason) => Fact::Unknown(format!("route scope: {reason}")),
        };
        let table = if let Some(required) = required_table {
            match &route.table {
                Fact::Known(Some(value)) => Fact::Known(*value == required),
                Fact::Known(None) => Fact::Unknown("Linux route table not established".into()),
                Fact::Unknown(reason) => Fact::Unknown(format!("route table: {reason}")),
            }
        } else {
            Fact::Known(true)
        };
        match and(and(role, scoped), table) {
            Fact::Known(true) => intervals.push(n),
            Fact::Known(false) => {}
            Fact::Unknown(reason) => unresolved.push(reason),
        }
    }
    if complete(intervals, family) {
        Fact::Known(true)
    } else if unresolved.is_empty() {
        Fact::Known(false)
    } else {
        Fact::Unknown(unresolved.join("; "))
    }
}

fn policy_any(
    object: &ObjectFacts,
    predicate: impl Fn(&PolicyRuleFact) -> Fact<bool>,
) -> Fact<bool> {
    match &object.policy_rules {
        Fact::Unknown(reason) => Fact::Unknown(format!("policy rules: {reason}")),
        Fact::Known(rules) => any(rules
            .iter()
            .map(|rule| and(rule.applies_to_object.clone(), predicate(rule)))),
    }
}

fn early_policy_coverage(
    object: &ObjectFacts,
    parsed: &[Result<Network, String>],
    rule: &PolicyRuleFact,
) -> Fact<bool> {
    let selectors = match &rule.selector_scope {
        Fact::Known(PolicySelectorScope::Global) => Fact::Known(true),
        Fact::Known(PolicySelectorScope::Limited(_)) => Fact::Known(false),
        Fact::Unknown(reason) => Fact::Unknown(format!("policy selector scope: {reason}")),
    };
    let path = match (&rule.lookup_table, &rule.address_family) {
        (Fact::Known(Some(table)), Fact::Known(family)) => cover(
            object,
            parsed,
            *family,
            RouteScope::Global,
            Some(*table),
            false,
        ),
        (Fact::Unknown(reason), _) => Fact::Unknown(format!("lookup table: {reason}")),
        (_, Fact::Unknown(reason)) => Fact::Unknown(format!("policy address family: {reason}")),
        (Fact::Known(None), _) => Fact::Unknown("Linux lookup table not established".into()),
    };
    and(Fact::Known(rule.priority < 9000), and(selectors, path))
}

fn proxy_signature(
    input: &ClassificationInput<'_>,
    object: &ObjectFacts,
    parsed: &[Result<Network, String>],
) -> Fact<bool> {
    let address_hit = match &object.addresses {
        Fact::Unknown(reason) => Fact::Unknown(format!("interface addresses: {reason}")),
        Fact::Known(addresses) => any(addresses.iter().map(|address| {
            let n = match network(address.address, address.prefix_len) {
                Ok(n) => n,
                Err(reason) => return Fact::Unknown(reason),
            };
            Fact::Known(match address.address {
                IpAddr::V4(ip) => matches!(
                    (u32::from(ip), address.prefix_len),
                    (0xc6120001, 16) | (0xac120001, 30) | (0xac130001, 30)
                ),
                IpAddr::V6(_) => {
                    let base = u128::from_be_bytes([
                        0xfd, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                    ]);
                    // Compare the actual host, not its interface's potentially broader network.
                    let host = match address.address {
                        IpAddr::V6(ip) => u128::from(ip),
                        _ => n.start,
                    };
                    host >= base && host <= base + 3
                }
            })
        })),
    };
    let platform_hit = match input.platform {
        Platform::Linux => {
            let candidate = policy_any(object, |rule| {
                let table = match &rule.lookup_table {
                    Fact::Known(value) => Fact::Known(*value == Some(2022)),
                    Fact::Unknown(reason) => Fact::Unknown(format!("lookup table: {reason}")),
                };
                or(Fact::Known((9000..=9010).contains(&rule.priority)), table)
            });
            let before = match input.observation_phase {
                Fact::Known(ObservationPhase::BeforePolarisTun) => Fact::Known(true),
                Fact::Known(ObservationPhase::Runtime) => {
                    Fact::Unknown("policy rules not witnessed before Polaris TUN".into())
                }
                Fact::Unknown(reason) => Fact::Unknown(format!("observation phase: {reason}")),
            };
            and(candidate, before)
        }
        Platform::Mac => match &object.routes {
            Fact::Unknown(reason) => Fact::Unknown(format!("macOS routes: {reason}")),
            Fact::Known(_) => {
                let mut v4 = BTreeSet::new();
                let mut v6 = BTreeSet::new();
                let mut bad = Vec::new();
                for item in parsed {
                    match item {
                        Err(reason) => bad.push(reason.clone()),
                        Ok(n) => {
                            let width = match n.family {
                                AddressFamily::V4 => 32,
                                AddressFamily::V6 => 128,
                            };
                            if (1..=8).contains(&n.bits) && n.start == 1u128 << (width - n.bits) {
                                match n.family {
                                    AddressFamily::V4 => {
                                        v4.insert(n.bits);
                                    }
                                    AddressFamily::V6 => {
                                        v6.insert(n.bits);
                                    }
                                }
                            }
                        }
                    }
                }
                if v4.len() >= 4 || v6.len() >= 4 {
                    Fact::Known(true)
                } else if bad.is_empty() {
                    Fact::Known(false)
                } else {
                    Fact::Unknown(bad.join("; "))
                }
            }
        },
        Platform::Win => Fact::Known(false),
        Platform::Android => Fact::Known(false),
        Platform::Ios => Fact::Known(false),
        Platform::Other => Fact::Known(false),
    };
    or(address_hit, platform_hit)
}

fn collision(
    object: &ObjectFacts,
    parsed: &[Result<Network, String>],
    protected: &[Result<Network, String>],
) -> Fact<bool> {
    if let Some(Err(reason)) = protected.iter().find(|n| n.is_err()) {
        return Fact::Unknown(format!("emitted criteria: {reason}"));
    }
    let intersects = |n: Network| {
        protected
            .iter()
            .any(|p| p.as_ref().is_ok_and(|p| overlaps(n, *p)))
    };
    let address_hit = match &object.addresses {
        Fact::Unknown(reason) => Fact::Unknown(format!("interface addresses: {reason}")),
        Fact::Known(addresses) => {
            any(addresses
                .iter()
                .map(|a| match network(a.address, a.prefix_len) {
                    Ok(n) => Fact::Known(intersects(n)),
                    Err(reason) => Fact::Unknown(reason),
                }))
        }
    };
    let route_hit = match &object.routes {
        Fact::Unknown(reason) => Fact::Unknown(format!("routes: {reason}")),
        Fact::Known(routes) => any(routes.iter().zip(parsed).map(|(route, n)| {
            let n = match n {
                Ok(n) => *n,
                Err(reason) => return Fact::Unknown(reason.clone()),
            };
            if !intersects(n) {
                return Fact::Known(false);
            }
            match &route.role {
                Fact::Known(RouteRole::ResourceClaim) => Fact::Known(true),
                Fact::Unknown(reason) => Fact::Unknown(format!("resource/coverage role: {reason}")),
                Fact::Known(RouteRole::CoverageDeclaration) => {
                    let witness = match &route.scope {
                        Fact::Known(scope) => cover(object, parsed, n.family, *scope, None, false),
                        Fact::Unknown(reason) => Fact::Unknown(format!("coverage scope: {reason}")),
                    };
                    match witness {
                        Fact::Known(true) => Fact::Known(false),
                        Fact::Known(false) => Fact::Unknown(
                            "coverage declaration has no complete same-family/scope witness".into(),
                        ),
                        Fact::Unknown(reason) => Fact::Unknown(reason),
                    }
                }
            }
        })),
    };
    or(address_hit, route_hit)
}

fn history(
    input: &ClassificationInput<'_>,
    object: &ObjectFacts,
) -> (HistoryAssessment, Fact<bool>) {
    match &object.stable_identity {
        Fact::Known(None) => (HistoryAssessment::NoVerifiableIdentity, Fact::Known(false)),
        Fact::Unknown(reason) => (
            HistoryAssessment::Unknown(reason.clone()),
            Fact::Unknown(format!("stable identity: {reason}")),
        ),
        Fact::Known(Some(key)) if key.trim().is_empty() => {
            let reason = "stable identity is empty".to_string();
            (
                HistoryAssessment::Unknown(reason.clone()),
                Fact::Unknown(reason),
            )
        }
        Fact::Known(Some(key)) => match input.forced_repair_identities {
            Fact::Unknown(reason) => (
                HistoryAssessment::Unknown(reason.clone()),
                Fact::Unknown(format!("repair history: {reason}")),
            ),
            Fact::Known(keys) => {
                let matched = keys.iter().any(|known| known == key);
                (
                    if matched {
                        HistoryAssessment::Matched
                    } else {
                        HistoryAssessment::NotMatched
                    },
                    Fact::Known(matched),
                )
            }
        },
    }
}

/// Classify supplied object facts in input order. No collection, persistence or action occurs.
#[must_use]
pub fn classify(input: &ClassificationInput<'_>) -> Vec<ClassificationReport> {
    let protected: Vec<_> = input
        .criteria
        .fakeip_ranges
        .iter()
        .chain(&input.criteria.tun_addresses)
        .map(|p| parse_network(p))
        .collect();
    input.objects.iter().map(|object| {
        let parsed: Vec<_> = match &object.routes {
            Fact::Known(routes) => routes.iter().map(|route| parse_network(&route.prefix)).collect(),
            Fact::Unknown(_) => Vec::new(),
        };
        let coverage = Coverage {
            ipv4: cover(object, &parsed, AddressFamily::V4, RouteScope::Global, None, false),
            ipv6: cover(object, &parsed, AddressFamily::V6, RouteScope::Global, None, false),
        };
        let global = or(coverage.ipv4.clone(), coverage.ipv6.clone());
        let (history, history_match) = history(input, object);
        let entry = if input.platform == Platform::Linux && !input.criteria.tun_addresses.is_empty() {
            let main_split = or(
                cover(object, &parsed, AddressFamily::V4, RouteScope::Global, Some(254), true),
                cover(object, &parsed, AddressFamily::V6, RouteScope::Global, Some(254), true),
            );
            let early = policy_any(object, |rule| early_policy_coverage(object, &parsed, rule));
            and(global.clone(), or(main_split, early))
        } else { Fact::Known(false) };
        let foreign = match input.own_interfaces {
            Fact::Known(names) => Fact::Known(!names.contains(&object.interface)),
            Fact::Unknown(reason) => Fact::Unknown(format!("own interface attribution: {reason}")),
        };
        let predicates = vec![
            (Rule::OtherTunProxy, and(foreign, proxy_signature(input, object, &parsed))),
            (Rule::AddressCollision, collision(object, &parsed, &protected)),
            (Rule::ForcedRouteRepairHistory, history_match),
            (Rule::EntryCannotBePreserved, entry),
            (Rule::GlobalCoverage, global),
            (Rule::TunnelInterface, object.tunnel.clone()),
            (Rule::KnownVirtualization, and(not(&object.tunnel), object.virtualization.clone())),
            (Rule::NoObject, Fact::Known(true)),
        ];
        let mut diagnostics: Vec<_> = parsed.iter().chain(&protected).filter_map(|n| n.as_ref().err().cloned()).collect();
        if let Fact::Known(addresses) = &object.addresses {
            diagnostics.extend(addresses.iter().filter_map(|a| network(a.address, a.prefix_len).err()));
        }
        let excluded_self = matches!(input.own_interfaces, Fact::Known(names) if names.contains(&object.interface));
        let decision = if excluded_self { Fact::Known(None) }
        else if object.interface.trim().is_empty() { Fact::Unknown("interface identity is empty".into()) }
        else if let Fact::Unknown(reason) = input.own_interfaces { Fact::Unknown(format!("own interface attribution: {reason}")) }
        else {
            predicates.iter().find_map(|(rule, fact)| match fact {
                Fact::Known(false) => None,
                Fact::Unknown(reason) => Some(Fact::Unknown(format!("{rule:?}: {reason}"))),
                Fact::Known(true) => Some(Fact::Known(match rule {
                    Rule::NoObject => None,
                    _ => {
                        let (shape, relation) = match rule {
                            Rule::OtherTunProxy | Rule::AddressCollision | Rule::ForcedRouteRepairHistory | Rule::EntryCannotBePreserved => (ObjectShape::Exclusive, Relation::Exclusive),
                            Rule::GlobalCoverage => (ObjectShape::Global, Relation::Follow),
                            Rule::TunnelInterface => (ObjectShape::Scoped, Relation::Follow),
                            Rule::KnownVirtualization => (ObjectShape::Bystander, Relation::Bystander),
                            Rule::NoObject => unreachable!("handled above"),
                        };
                        Some(Classification { shape, relation, rule: *rule })
                    }
                })),
            }).unwrap_or(Fact::Known(None))
        };
        ClassificationReport { interface: object.interface.clone(), decision, excluded_self, coverage, history, predicates, diagnostics }
    }).collect()
}

#[cfg(test)]
mod tests;
