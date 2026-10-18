//! Native node bootstrap policy generation. This module owns only immutable
//! planning and serialization; core owns all sockets, scheduling and final cache.
pub mod decoy;
pub mod upstream;

use crate::singbox::{DnsConfig, DnsRule, DnsServer, DomainResolver, DomainStrategy, OneOrMany};
use crate::user_config::{LogLevel, UserConfig};
use polaris_helper_proto::Platform;
use upstream::{plan_upstreams, UpstreamKind};

pub const POLICY: &str = "node-bootstrap";
pub const CANDIDATE_TIMEOUT: &str = "1500ms";
pub const LOOKUP_TIMEOUT: &str = "2s";

pub fn enabled(config: &UserConfig) -> bool {
    config
        .dns_config
        .as_ref()
        .and_then(|dns| dns.resolve_node_domains_ahead)
        != Some(false)
}

pub fn resolver() -> DomainResolver {
    DomainResolver::Rules {
        mode: "rules".into(),
        policy: POLICY.into(),
        strategy: DomainStrategy::PreferIpv4,
        timeout: LOOKUP_TIMEOUT.into(),
    }
}

/// Rebuild just the explicit private/managed protection prefix. Called again
/// after managed mesh compilation, so isolated node lookups see the same owners
/// and rejection decisions as ordinary DNS without inheriting caller metadata.
pub fn replace_protections(dns: &mut DnsConfig, protections: &[DnsRule]) {
    let Some(rules) = dns.rules.as_mut() else {
        return;
    };
    let Some(start) = rules
        .iter()
        .position(|rule| rule.resolver_policy.as_deref() == Some(POLICY))
    else {
        return;
    };
    rules.retain(|rule| {
        rule.resolver_policy.as_deref() != Some(POLICY)
            || matches!(rule.action.as_deref(), Some("evaluate" | "respond"))
    });
    let scoped = protections.iter().cloned().map(|mut rule| {
        rule.resolver_policy = Some(POLICY.into());
        if rule.action.is_none() {
            rule.action = Some("route".into());
        }
        rule
    });
    rules.splice(start..start, scoped);
}

pub fn append_policy(
    config: &UserConfig,
    platform: &str,
    decoy_override: Option<&str>,
    dns: &mut DnsConfig,
    log: fn(LogLevel, &str),
) {
    let Some(plan) = plan_upstreams(
        config.dns_config.as_ref(),
        config
            .proxy_mode_type
            .effective_on(Platform::parse(platform)),
    ) else {
        return;
    };
    let decoys = decoy_policy_cidrs(decoy_override, log);
    // The caller puts these two canonical LAN rules at the front. Legacy
    // Tailscale preferred_by uses the DNS transport's current preferred-domain
    // state (including MagicDNS), never caller metadata. Core fingerprints it.
    let protections: Vec<DnsRule> = dns
        .rules
        .iter()
        .flatten()
        .take(2)
        .cloned()
        .chain(
            dns.rules
                .iter()
                .flatten()
                .filter(|rule| {
                    rule.server.as_deref() == Some("dns-tailscale") && rule.preferred_by.is_some()
                })
                .cloned(),
        )
        .collect();
    let rules = dns.rules.get_or_insert_with(Vec::new);
    for (tier, candidates) in [(1, &plan.tier1), (2, &plan.tier2)] {
        for (index, candidate) in candidates.iter().enumerate() {
            let server_tag = format!("dns-node-native-{tier}-{index}");
            let response_tag = format!("node-native-{tier}-{index}-response");
            dns.servers.push(DnsServer {
                tag: server_tag.clone(),
                type_field: Some(
                    match candidate.kind {
                        UpstreamKind::Doh => "https",
                        UpstreamKind::Udp => "udp",
                        UpstreamKind::System => "local",
                    }
                    .into(),
                ),
                server: candidate.ip.clone(),
                server_port: candidate.port,
                path: candidate.path.clone().map(OneOrMany::One),
                // IP-only upstreams have no recursive bootstrap and use the
                // core's native physical-interface selection, never a proxy.
                domain_resolver: None,
                detour: None,
                predefined: None,
                endpoint: None,
                accept_search_domain: None,
                accept_default_resolvers: None,
                neighbor_domain: None,
                address: None,
                address_resolver: None,
                inet4_range: None,
                inet6_range: None,
            });
            rules.push(DnsRule {
                resolver_policy: Some(POLICY.into()),
                action: Some("evaluate".into()),
                server: Some(server_tag),
                tag: Some(response_tag.clone()),
                timeout: Some(CANDIDATE_TIMEOUT.into()),
                disable_cache: Some(true),
                disable_optimistic_cache: Some(true),
                speculative: (index > 0).then_some(true),
                query_type: (candidate.kind == UpstreamKind::System).then(|| vec!["A".into()]),
                ..Default::default()
            });
            for class in ["hit", "empty"] {
                rules.push(DnsRule {
                    resolver_policy: Some(POLICY.into()),
                    action: Some("respond".into()),
                    match_response: Some(serde_json::Value::String(response_tag.clone())),
                    response_class: Some(class.into()),
                    race: Some(true),
                    mode: (class == "empty").then(|| "defer".into()),
                    response_reject_ip_cidr: (class == "hit").then(|| decoys.clone()),
                    ..Default::default()
                });
            }
        }
    }
    rules.push(DnsRule {
        resolver_policy: Some(POLICY.into()),
        action: Some("respond".into()),
        mode: Some("fallback".into()),
        fallback_rcode: Some("SERVFAIL".into()),
        ..Default::default()
    });
    replace_protections(dns, &protections);
}

/// Report only bounded line numbers/counts, never override text or file paths.
fn decoy_policy_cidrs(override_text: Option<&str>, log: fn(LogLevel, &str)) -> Vec<String> {
    let Some(text) = override_text else {
        log(
            LogLevel::Info,
            "Node DNS decoy override unavailable; using built-in prefixes",
        );
        return decoy::DecoySet::builtin().cidrs();
    };
    let parsed = decoy::DecoySet::parse(text);
    if !parsed.bad_lines.is_empty() {
        let lines = parsed
            .bad_lines
            .iter()
            .take(5)
            .map(|(number, _)| number.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        log(
            LogLevel::Warn,
            &format!(
                "Node DNS decoy override: {} invalid lines skipped; first line numbers: {lines}",
                parsed.bad_lines.len()
            ),
        );
    }
    let (v4, v6) = parsed.set.len();
    if parsed.fell_back {
        log(LogLevel::Warn, &format!("Node DNS decoy override has no valid prefixes; using built-in prefixes ({v4} IPv4, {v6} IPv6)"));
    } else {
        log(
            LogLevel::Info,
            &format!("Node DNS decoy override replaces built-in prefixes: {v4} IPv4, {v6} IPv6"),
        );
    }
    parsed.set.cidrs()
}

#[cfg(test)]
mod tests;
