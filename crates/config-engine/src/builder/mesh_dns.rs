//! D1 managed MagicDNS compilation. The caller must bind this result to the same
//! route plan and emitted Tailscale endpoints before applying it to a running core.
//! In particular, `service` is a route-plan decision, not an installed route.

use std::collections::{BTreeMap, BTreeSet};

use crate::singbox::{DnsConfig, DnsRule, DnsServer};
use crate::user_config::mesh_route_state::{
    MeshBindingState, MeshDnsOwnerTarget, MeshDnsPolicy, MeshDnsShortNamePolicy, MeshIdentity,
    MeshOwnerRef, MeshRouteState,
};

pub const MAGIC_DNS_SERVICE_CIDRS: [&str; 2] = ["100.100.100.100/32", "fd7a:115c:a1e0::53/128"];
pub const MAGIC_DNS_SERVICE_PORT: u16 = 53;
pub const MAGIC_DNS_SERVICE_TRANSPORTS: [&str; 2] = ["tcp", "udp"];
const LEGACY_TS_DNS_TAG: &str = "dns-tailscale";
const SHORT_NAME_REGEX: &str = r"^[^.]+\.?$";
const MAX_SUFFIXES: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshDnsBuildError {
    MissingState,
    InvalidState(String),
    UnsupportedPolicyVersion,
    InvalidSuffix(String),
    DuplicateSuffix(String),
    ResourceLimitExceeded,
    LegacyDnsShapeChanged,
    ConflictingTailscaleResolver,
    ConflictingEmittedEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshDnsRejectReason {
    Explicit,
    UnassignedCandidate,
    CandidateUnproven,
    OwnerMissing,
    OwnerNotBound,
    EndpointMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshDnsTargetDecision {
    Owner {
        owner: MeshOwnerRef,
        endpoint_tag: String,
    },
    Reject(MeshDnsRejectReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshDnsShortNameDecision {
    System,
    Target(MeshDnsTargetDecision),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshDnsMode {
    LegacyUnmanaged,
    Managed,
}

/// Exact owner-epoch binding from the *same* generated plan as the endpoint.
/// A serverId-to-display-tag map alone cannot distinguish an old epoch from a
/// replacement endpoint under the same serverId.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshDnsEmittedEndpoint {
    pub owner: MeshOwnerRef,
    pub tag: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshDnsLimitation {
    /// Correct name answers do not disambiguate two owners of the same IP.
    SameIpNeedsFlowContext,
    /// The native Tailscale DNS transport may dial upstreams directly.
    NativeUpstreamMayDialDirect,
}

/// `service` is one decision for both MagicDNS service IPs. It is `None` only in
/// legacy mode. DNS upstreams inside the native Tailscale transport may dial
/// directly; this plan only prevents fallback to another Tailscale resolver or
/// the application's public resolver for explicitly managed names.
#[derive(Debug, Clone, PartialEq)]
pub struct MeshDnsBuild {
    pub dns: DnsConfig,
    pub mode: MeshDnsMode,
    pub suffixes: Vec<(String, MeshDnsTargetDecision)>,
    pub short_name: Option<MeshDnsShortNameDecision>,
    pub service: Option<MeshDnsTargetDecision>,
    pub limitations: Vec<MeshDnsLimitation>,
}

/// Pure D1 overlay. `None` means the *entire* legacy DNS config remains byte
/// equivalent. Managed callers must pass the persisted identity/evidence ledger
/// and exact owner-epoch bindings of actual emitted Tailscale endpoints from
/// the same generation.
/// A compile error must abort managed Apply, never fall back to legacy DNS.
pub fn build_mesh_dns_overlay(
    legacy: &DnsConfig,
    policy: Option<&MeshDnsPolicy>,
    state: Option<&MeshRouteState>,
    emitted_endpoints: &[MeshDnsEmittedEndpoint],
) -> Result<MeshDnsBuild, MeshDnsBuildError> {
    let Some(policy) = policy else {
        return Ok(MeshDnsBuild {
            dns: legacy.clone(),
            mode: MeshDnsMode::LegacyUnmanaged,
            suffixes: Vec::new(),
            short_name: None,
            service: None,
            limitations: Vec::new(),
        });
    };
    if policy.schema_version != 1 {
        return Err(MeshDnsBuildError::UnsupportedPolicyVersion);
    }
    let state = state.ok_or(MeshDnsBuildError::MissingState)?;
    if state.identities.len() > MAX_SUFFIXES
        || state.observations.len() > MAX_SUFFIXES
        || policy.suffix_assignments.len() > MAX_SUFFIXES
        || emitted_endpoints.len() > MAX_SUFFIXES
    {
        return Err(MeshDnsBuildError::ResourceLimitExceeded);
    }
    state.validate().map_err(MeshDnsBuildError::InvalidState)?;

    let mut emitted = BTreeMap::<(&str, &str), &str>::new();
    let mut used_tags = BTreeSet::new();
    let mut used_server_ids = BTreeSet::new();
    for endpoint in emitted_endpoints {
        if endpoint.owner.server_id.is_empty()
            || endpoint.owner.identity_epoch.is_empty()
            || endpoint.tag.is_empty()
            || emitted
                .insert(
                    (&endpoint.owner.server_id, &endpoint.owner.identity_epoch),
                    &endpoint.tag,
                )
                .is_some()
            || !used_tags.insert(endpoint.tag.as_str())
            || !used_server_ids.insert(endpoint.owner.server_id.as_str())
        {
            return Err(MeshDnsBuildError::ConflictingEmittedEndpoint);
        }
    }

    let mut identities: BTreeMap<(&str, &str), &MeshIdentity> = BTreeMap::new();
    for identity in &state.identities {
        identities.insert((&identity.server_id, &identity.identity_epoch), identity);
    }
    let mut candidates: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
    let mut evidence_count = 0usize;
    for observation in &state.observations {
        for raw in &observation.magic_dns_suffixes {
            if evidence_count == MAX_SUFFIXES {
                return Err(MeshDnsBuildError::ResourceLimitExceeded);
            }
            evidence_count += 1;
            let suffix = normalize_suffix(raw)?;
            candidates.entry(suffix).or_default().insert((
                observation.owner_ref.server_id.clone(),
                observation.owner_ref.identity_epoch.clone(),
            ));
        }
    }
    if candidates
        .len()
        .saturating_add(policy.suffix_assignments.len())
        > MAX_SUFFIXES
    {
        return Err(MeshDnsBuildError::ResourceLimitExceeded);
    }

    let mut suffixes: BTreeMap<String, MeshDnsTargetDecision> = candidates
        .keys()
        .map(|suffix| {
            (
                suffix.clone(),
                MeshDnsTargetDecision::Reject(MeshDnsRejectReason::UnassignedCandidate),
            )
        })
        .collect();
    let mut explicit = BTreeSet::new();
    for assignment in &policy.suffix_assignments {
        let suffix = normalize_suffix(&assignment.suffix)?;
        if !explicit.insert(suffix.clone()) {
            return Err(MeshDnsBuildError::DuplicateSuffix(suffix));
        }
        let decision = match &assignment.target {
            MeshDnsOwnerTarget::Reject => {
                MeshDnsTargetDecision::Reject(MeshDnsRejectReason::Explicit)
            }
            MeshDnsOwnerTarget::Owner {
                server_id,
                identity_epoch,
            } => resolve_owner(
                server_id,
                identity_epoch,
                Some((&suffix, &candidates)),
                &identities,
                &emitted,
            ),
        };
        suffixes.insert(suffix, decision);
    }

    // A more specific known candidate is never silently captured by a broader
    // assignment; absent an exact explicit assignment it stays rejected.
    let mut suffixes: Vec<_> = suffixes.into_iter().collect();
    suffixes.sort_by(|(a, _), (b, _)| {
        b.split('.')
            .count()
            .cmp(&a.split('.').count())
            .then(a.cmp(b))
    });
    let short_name = match &policy.short_name_policy {
        MeshDnsShortNamePolicy::System => MeshDnsShortNameDecision::System,
        MeshDnsShortNamePolicy::Reject => MeshDnsShortNameDecision::Target(
            MeshDnsTargetDecision::Reject(MeshDnsRejectReason::Explicit),
        ),
        MeshDnsShortNamePolicy::Owner {
            server_id,
            identity_epoch,
        } => MeshDnsShortNameDecision::Target(resolve_owner(
            server_id,
            identity_epoch,
            None,
            &identities,
            &emitted,
        )),
    };
    let service = match &policy.service_owner {
        MeshDnsOwnerTarget::Reject => MeshDnsTargetDecision::Reject(MeshDnsRejectReason::Explicit),
        MeshDnsOwnerTarget::Owner {
            server_id,
            identity_epoch,
        } => resolve_owner(server_id, identity_epoch, None, &identities, &emitted),
    };

    let mut dns = legacy.clone();
    dns.servers.retain(|server| {
        !(server.tag == LEGACY_TS_DNS_TAG && server.type_field.as_deref() == Some("tailscale"))
    });
    if dns
        .servers
        .iter()
        .any(|server| server.type_field.as_deref() == Some("tailscale"))
    {
        return Err(MeshDnsBuildError::ConflictingTailscaleResolver);
    }
    let rules = dns
        .rules
        .as_mut()
        .ok_or(MeshDnsBuildError::LegacyDnsShapeChanged)?;
    rules.retain(|rule| {
        !(rule.server.as_deref() == Some(LEGACY_TS_DNS_TAG)
            && rule
                .preferred_by
                .as_ref()
                .is_some_and(|tags| tags.iter().any(|tag| tag == LEGACY_TS_DNS_TAG)))
    });
    let mdns_rule = rules
        .first()
        .ok_or(MeshDnsBuildError::LegacyDnsShapeChanged)?;
    let lan_rule = rules
        .get(1)
        .ok_or(MeshDnsBuildError::LegacyDnsShapeChanged)?;
    if mdns_rule.server.as_deref() != Some("dns-mdns")
        || !mdns_rule
            .domain_suffix
            .as_ref()
            .is_some_and(|names| names.iter().any(|name| name == "local"))
        || lan_rule.server.as_deref() != Some("dns-lan")
            && lan_rule.action.as_deref() != Some("predefined")
        || !lan_rule
            .domain_regex
            .as_ref()
            .is_some_and(|names| names.iter().any(|name| name == SHORT_NAME_REGEX))
    {
        return Err(MeshDnsBuildError::LegacyDnsShapeChanged);
    }

    let mut occupied: BTreeSet<String> = dns
        .servers
        .iter()
        .map(|server| server.tag.clone())
        .collect();
    let mut endpoint_resolvers = BTreeMap::<String, String>::new();
    let mut overlay_rules = Vec::new();
    for (suffix, decision) in &suffixes {
        let mut rule = DnsRule {
            domain_suffix: Some(vec![suffix.clone()]),
            ..Default::default()
        };
        assign_rule(
            &mut rule,
            decision,
            false,
            &mut dns.servers,
            &mut occupied,
            &mut endpoint_resolvers,
        );
        overlay_rules.push(rule);
    }
    if let MeshDnsShortNameDecision::Target(decision) = &short_name {
        let mut rule = DnsRule {
            domain_regex: Some(vec![SHORT_NAME_REGEX.into()]),
            ..Default::default()
        };
        assign_rule(
            &mut rule,
            decision,
            true,
            &mut dns.servers,
            &mut occupied,
            &mut endpoint_resolvers,
        );
        overlay_rules.push(rule);
    }
    rules.splice(1..1, overlay_rules);

    Ok(MeshDnsBuild {
        dns,
        mode: MeshDnsMode::Managed,
        suffixes,
        short_name: Some(short_name),
        service: Some(service),
        limitations: vec![
            MeshDnsLimitation::SameIpNeedsFlowContext,
            MeshDnsLimitation::NativeUpstreamMayDialDirect,
        ],
    })
}

fn normalize_suffix(raw: &str) -> Result<String, MeshDnsBuildError> {
    if raw.len() > 255 {
        return Err(MeshDnsBuildError::InvalidSuffix(
            raw.chars().take(64).collect(),
        ));
    }
    let trimmed = raw.trim();
    let suffix = trimmed
        .strip_suffix('.')
        .unwrap_or(trimmed)
        .to_ascii_lowercase();
    let valid = suffix.len() <= 253
        && suffix.parse::<std::net::IpAddr>().is_err()
        && suffix.split('.').count() >= 2
        && suffix.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        });
    // These domains have an existing, more privileged mDNS/link-local rule.
    // Refuse a misleading managed assignment instead of claiming its owner won.
    let reserved = [
        "local",
        "254.169.in-addr.arpa",
        "8.e.f.ip6.arpa",
        "9.e.f.ip6.arpa",
        "a.e.f.ip6.arpa",
        "b.e.f.ip6.arpa",
    ]
    .iter()
    .any(|domain| suffix == *domain || suffix.ends_with(&format!(".{domain}")));
    if !valid || reserved {
        return Err(MeshDnsBuildError::InvalidSuffix(raw.into()));
    }
    Ok(suffix)
}

fn resolve_owner(
    server_id: &str,
    identity_epoch: &str,
    candidate: Option<(&str, &BTreeMap<String, BTreeSet<(String, String)>>)>,
    identities: &BTreeMap<(&str, &str), &MeshIdentity>,
    emitted: &BTreeMap<(&str, &str), &str>,
) -> MeshDnsTargetDecision {
    let Some(identity) = identities.get(&(server_id, identity_epoch)) else {
        return MeshDnsTargetDecision::Reject(MeshDnsRejectReason::OwnerMissing);
    };
    if identity.binding_state != MeshBindingState::Bound {
        return MeshDnsTargetDecision::Reject(MeshDnsRejectReason::OwnerNotBound);
    }
    if let Some((suffix, candidates)) = candidate {
        if !candidates
            .get(suffix)
            .is_some_and(|owners| owners.contains(&(server_id.into(), identity_epoch.into())))
        {
            return MeshDnsTargetDecision::Reject(MeshDnsRejectReason::CandidateUnproven);
        }
    }
    let Some(endpoint_tag) = emitted.get(&(server_id, identity_epoch)) else {
        return MeshDnsTargetDecision::Reject(MeshDnsRejectReason::EndpointMissing);
    };
    MeshDnsTargetDecision::Owner {
        owner: MeshOwnerRef {
            server_id: server_id.into(),
            identity_epoch: identity_epoch.into(),
        },
        endpoint_tag: (*endpoint_tag).into(),
    }
}

fn assign_rule(
    rule: &mut DnsRule,
    decision: &MeshDnsTargetDecision,
    short_name: bool,
    servers: &mut Vec<DnsServer>,
    occupied: &mut BTreeSet<String>,
    endpoint_resolvers: &mut BTreeMap<String, String>,
) {
    match decision {
        MeshDnsTargetDecision::Reject(_) => rule.action = Some("reject".into()),
        MeshDnsTargetDecision::Owner { endpoint_tag, .. } => {
            let resolver_tag = if let Some(tag) = endpoint_resolvers.get(endpoint_tag) {
                tag.clone()
            } else {
                let mut index = endpoint_resolvers.len();
                let tag = loop {
                    let candidate = format!("dns-mesh-{index}");
                    if occupied.insert(candidate.clone()) {
                        break candidate;
                    }
                    index += 1;
                };
                servers.push(DnsServer {
                    tag: tag.clone(),
                    type_field: Some("tailscale".into()),
                    server: None,
                    server_port: None,
                    path: None,
                    predefined: None,
                    domain_resolver: None,
                    detour: None,
                    endpoint: Some(endpoint_tag.clone()),
                    accept_search_domain: Some(short_name),
                    accept_default_resolvers: Some(false),
                    neighbor_domain: None,
                    address: None,
                    address_resolver: None,
                    inet4_range: None,
                    inet6_range: None,
                });
                endpoint_resolvers.insert(endpoint_tag.clone(), tag.clone());
                tag
            };
            if short_name {
                if let Some(server) = servers.iter_mut().find(|server| server.tag == resolver_tag) {
                    server.accept_search_domain = Some(true);
                }
            }
            rule.action = Some("route".into());
            rule.server = Some(resolver_tag);
        }
    }
}

#[cfg(test)]
mod tests;
