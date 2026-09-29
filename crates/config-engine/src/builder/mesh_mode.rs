//! Compile both selected-exit policies into one immutable core configuration.
//!
//! The old builder remains the oracle for each policy. We admit dynamic switching only when
//! every non-route/DNS difference is the raw selector's default (or the dashboard's exit),
//! and every route/DNS difference can be guarded by `clash_mode`. Failure leaves the original
//! configuration untouched, so the existing restart path remains authoritative.

use std::collections::{BTreeMap, BTreeSet};

use crate::builder::endpoint_routes::mesh_node_carries_full_tunnel;
use crate::singbox::{ClashApi, DnsRule, DnsServer, Outbound, RouteRule, SingBoxConfig};
use crate::user_config::app_config::UserConfig;
use crate::user_config::dns_constants::PROXY_SELECTOR_TAG;
use crate::user_config::server_config::{Protocol, ServerConfig};

pub const NORMAL: &str = "normal";
pub const MESH_DIRECT: &str = "mesh-direct";
pub const DASHBOARD_SELECTOR: &str = "polaris-dashboard-exit";

fn is_no_exit_ts(server: &ServerConfig) -> bool {
    server.protocol == Protocol::Tailscale && !mesh_node_carries_full_tunnel(server)
}

/// One TS without an exit node and at least one real public exit. Multiple Tailscale endpoints
/// are deliberately excluded: selection can change the MagicDNS endpoint binding.
pub fn mode_candidates(config: &UserConfig) -> Option<(String, String)> {
    let ts: Vec<_> = config
        .servers
        .iter()
        .filter(|s| s.protocol == Protocol::Tailscale)
        .collect();
    if ts.len() != 1 || !is_no_exit_ts(ts[0]) {
        return None;
    }
    let normal = config
        .servers
        .iter()
        .find(|s| {
            Some(s.id.as_str()) == config.selected_server_id.as_deref()
                && s.protocol != Protocol::Tailscale
                && (!crate::user_config::server_config::is_mesh_node(s)
                    || mesh_node_carries_full_tunnel(s))
        })
        .or_else(|| {
            config.servers.iter().find(|s| {
                s.protocol != Protocol::Tailscale
                    && (!crate::user_config::server_config::is_mesh_node(s)
                        || mesh_node_carries_full_tunnel(s))
            })
        })?;
    Some((normal.id.clone(), ts[0].id.clone()))
}

pub fn selected_mode(config: &UserConfig) -> &'static str {
    if config
        .servers
        .iter()
        .find(|s| Some(s.id.as_str()) == config.selected_server_id.as_deref())
        .is_some_and(is_no_exit_ts)
    {
        MESH_DIRECT
    } else {
        NORMAL
    }
}

/// A mode transition may only select one of the public exits represented by the compiled
/// normal policy, or the unique split-only Tailscale endpoint represented by mesh-direct.
pub fn selected_is_mode_candidate(config: &UserConfig, mesh_id: &str) -> bool {
    let Some(selected) = config.selected_server_id.as_deref() else {
        return false;
    };
    selected == mesh_id
        || config.servers.iter().any(|server| {
            server.id == selected
                && server.protocol != Protocol::Tailscale
                && (!crate::user_config::server_config::is_mesh_node(server)
                    || mesh_node_carries_full_tunnel(server))
        })
}

fn normalize_non_policy(config: &SingBoxConfig) -> SingBoxConfig {
    let mut copy = config.clone();
    copy.route = None;
    copy.dns = None;
    if let Some(selector) = copy
        .outbounds
        .iter_mut()
        .find(|o| o.tag == PROXY_SELECTOR_TAG)
    {
        selector.default = None;
    }
    if let Some(services) = copy.services.as_mut() {
        for service in services {
            if let Some(dashboard) = service.dashboard.as_mut() {
                if let Some(client) = dashboard.http_client.as_mut() {
                    client.detour.clear();
                }
            }
        }
    }
    copy
}

fn guarded_route(mut rule: RouteRule, mode: &str) -> Option<RouteRule> {
    if rule.type_field.is_some() || rule.clash_mode.is_some() {
        return None;
    }
    rule.clash_mode = Some(mode.to_owned());
    Some(rule)
}

fn guarded_dns(mut rule: DnsRule, mode: &str) -> Option<DnsRule> {
    if rule.type_field.is_some() || rule.clash_mode.is_some() {
        return None;
    }
    rule.clash_mode = Some(mode.to_owned());
    Some(rule)
}

/// Merge two ordered rule lists by longest common subsequence. Shared rules retain their exact
/// position; each differing rule is mode-guarded. A logical/custom mode-bearing difference is
/// unrepresentable here and rejects the whole compilation.
fn merge_ordered<T: Clone + PartialEq>(
    normal: &[T],
    mesh: &[T],
    guard: impl Fn(T, &str) -> Option<T>,
) -> Option<Vec<T>> {
    let (n, m) = (normal.len(), mesh.len());
    if n.checked_mul(m).is_none_or(|cells| cells > 1_000_000) {
        return None;
    }
    let mut lengths = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lengths[i][j] = if normal[i] == mesh[j] {
                lengths[i + 1][j + 1] + 1
            } else {
                lengths[i + 1][j].max(lengths[i][j + 1])
            };
        }
    }
    let mut merged = Vec::with_capacity(n + m);
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && normal[i] == mesh[j] {
            merged.push(normal[i].clone());
            i += 1;
            j += 1;
        } else if i < n && (j == m || lengths[i + 1][j] >= lengths[i][j + 1]) {
            merged.push(guard(normal[i].clone(), NORMAL)?);
            i += 1;
        } else {
            merged.push(guard(mesh[j].clone(), MESH_DIRECT)?);
            j += 1;
        }
    }
    Some(merged)
}

fn dns_server_mapping(
    normal: &[DnsServer],
    mesh: &[DnsServer],
) -> Option<(Vec<DnsServer>, BTreeMap<String, String>)> {
    if normal.len() != mesh.len() {
        return None;
    }
    let existing: BTreeSet<_> = normal.iter().map(|s| s.tag.as_str()).collect();
    let mut servers = normal.to_vec();
    let mut changed = BTreeMap::new();
    for (a, b) in normal.iter().zip(mesh) {
        if a.tag != b.tag {
            return None;
        }
        if a == b {
            continue;
        }
        let mut comparable = b.clone();
        comparable.detour = a.detour.clone();
        if comparable != *a {
            return None;
        }
        let replacement = format!("{}-mesh-direct", b.tag);
        if existing.contains(replacement.as_str()) {
            return None;
        }
        changed.insert(b.tag.clone(), replacement);
    }
    // DNS transports can themselves bootstrap through another DNS transport. Clone every
    // dependent transitively, otherwise a mesh transport could still resolve via normal's
    // proxy detour even though its own `detour` field is unchanged.
    loop {
        let mut added = false;
        for server in mesh {
            if changed.contains_key(&server.tag) {
                continue;
            }
            let depends = server
                .domain_resolver
                .as_ref()
                .is_some_and(|tag| changed.contains_key(tag))
                || server
                    .address_resolver
                    .as_ref()
                    .is_some_and(|tag| changed.contains_key(tag));
            if depends {
                let replacement = format!("{}-mesh-direct", server.tag);
                if existing.contains(replacement.as_str()) {
                    return None;
                }
                changed.insert(server.tag.clone(), replacement);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    for b in mesh {
        if let Some(new_tag) = changed.get(&b.tag) {
            let mut extra = b.clone();
            extra.tag = new_tag.clone();
            if let Some(resolver) = extra.domain_resolver.as_mut() {
                if let Some(mapped) = changed.get(resolver) {
                    *resolver = mapped.clone();
                }
            }
            if let Some(resolver) = extra.address_resolver.as_mut() {
                if let Some(mapped) = changed.get(resolver) {
                    *resolver = mapped.clone();
                }
            }
            servers.push(extra);
        }
    }
    Some((servers, changed))
}

fn static_resolver_uses_changed(
    value: &serde_json::Value,
    changed: &BTreeMap<String, String>,
) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, child)| {
            if key == "domain_resolver" || key == "address_resolver" {
                match child {
                    serde_json::Value::String(tag) => changed.contains_key(tag),
                    serde_json::Value::Object(fields) => fields
                        .get("server")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|tag| changed.contains_key(tag)),
                    _ => false,
                }
            } else {
                static_resolver_uses_changed(child, changed)
            }
        }),
        serde_json::Value::Array(items) => items
            .iter()
            .any(|v| static_resolver_uses_changed(v, changed)),
        _ => false,
    }
}

fn remap_tag(tag: &mut Option<String>, mapping: &BTreeMap<String, String>) {
    if let Some(value) = tag.as_mut() {
        if let Some(mapped) = mapping.get(value) {
            *value = mapped.clone();
        }
    }
}

/// Returns true only after the entire two-mode configuration has been compiled and installed.
pub fn try_compile(
    actual: &mut SingBoxConfig,
    normal: &SingBoxConfig,
    mesh: &SingBoxConfig,
    selected_mode: &str,
) -> bool {
    if normalize_non_policy(actual) != normalize_non_policy(normal)
        || normalize_non_policy(normal) != normalize_non_policy(mesh)
    {
        return false;
    }
    let (Some(normal_route), Some(mesh_route), Some(normal_dns), Some(mesh_dns)) =
        (&normal.route, &mesh.route, &normal.dns, &mesh.dns)
    else {
        return false;
    };

    // The native mode API exposes every clash_mode from route and DNS rules. A user-supplied
    // third mode would remain activatable while our two policy catchalls do not cover it.
    if normal_route
        .rules
        .iter()
        .chain(&mesh_route.rules)
        .any(|r| r.clash_mode.is_some())
        || normal_dns
            .rules
            .as_deref()
            .unwrap_or_default()
            .iter()
            .chain(mesh_dns.rules.as_deref().unwrap_or_default())
            .any(|r| r.clash_mode.is_some())
    {
        return false;
    }

    let mut route_common_a = normal_route.clone();
    let mut route_common_b = mesh_route.clone();
    route_common_a.rules.clear();
    route_common_b.rules.clear();
    route_common_a.final_outbound = None;
    route_common_b.final_outbound = None;
    if route_common_a != route_common_b {
        return false;
    }

    let mut dns_common_a = normal_dns.clone();
    let mut dns_common_b = mesh_dns.clone();
    dns_common_a.servers.clear();
    dns_common_b.servers.clear();
    dns_common_a.rules = None;
    dns_common_b.rules = None;
    dns_common_a.final_server = None;
    dns_common_b.final_server = None;
    if dns_common_a != dns_common_b {
        return false;
    }

    let Some((servers, mapping)) = dns_server_mapping(&normal_dns.servers, &mesh_dns.servers)
    else {
        return false;
    };
    if normal_route
        .default_domain_resolver
        .as_ref()
        .is_some_and(|tag| mapping.contains_key(tag))
        || static_resolver_uses_changed(
            &serde_json::to_value(&normal.outbounds).unwrap_or_default(),
            &mapping,
        )
        || static_resolver_uses_changed(
            &serde_json::to_value(&normal.endpoints).unwrap_or_default(),
            &mapping,
        )
    {
        return false;
    }
    let mut mesh_route_rules = mesh_route.rules.clone();
    for rule in &mut mesh_route_rules {
        remap_tag(&mut rule.server, &mapping);
        remap_tag(&mut rule.domain_resolver, &mapping);
        if rule
            .dns_server_address
            .as_ref()
            .is_some_and(|refs| refs.keys().any(|tag| mapping.contains_key(tag)))
        {
            return false;
        }
    }
    let Some(mut route_rules) =
        merge_ordered(&normal_route.rules, &mesh_route_rules, guarded_route)
    else {
        return false;
    };
    // A mode-specific catch-all expresses both `route.final` values without mutating the core.
    for (mode, tag) in [
        (NORMAL, &normal_route.final_outbound),
        (MESH_DIRECT, &mesh_route.final_outbound),
    ] {
        let Some(tag) = tag else {
            return false;
        };
        route_rules.push(RouteRule {
            clash_mode: Some(mode.to_owned()),
            action: Some("route".into()),
            outbound: Some(tag.clone()),
            ..Default::default()
        });
    }
    let mut mesh_dns_rules = mesh_dns.rules.clone().unwrap_or_default();
    for rule in &mut mesh_dns_rules {
        remap_tag(&mut rule.server, &mapping);
        if rule
            .dns_server_address
            .as_ref()
            .is_some_and(|refs| refs.keys().any(|tag| mapping.contains_key(tag)))
        {
            return false;
        }
    }
    let Some(mut dns_rules) = merge_ordered(
        normal_dns.rules.as_deref().unwrap_or_default(),
        &mesh_dns_rules,
        guarded_dns,
    ) else {
        return false;
    };
    for (mode, tag) in [
        (NORMAL, &normal_dns.final_server),
        (MESH_DIRECT, &mesh_dns.final_server),
    ] {
        let Some(mut tag) = tag.clone() else {
            return false;
        };
        if mode == MESH_DIRECT {
            if let Some(mapped) = mapping.get(&tag) {
                tag = mapped.clone();
            }
        }
        dns_rules.push(DnsRule {
            clash_mode: Some(mode.to_owned()),
            server: Some(tag),
            ..Default::default()
        });
    }

    let Some(actual_route) = actual.route.as_ref() else {
        return false;
    };
    let Some(actual_dns) = actual.dns.as_ref() else {
        return false;
    };
    // A third ordinary node may have selected-dependent route or DNS effects. In that case do
    // not claim that the canonical pair represents the running projection.
    let actual_normal = actual_route == normal_route && actual_dns == normal_dns;
    let actual_mesh = actual_route == mesh_route && actual_dns == mesh_dns;
    if (selected_mode == NORMAL && !actual_normal) || (selected_mode == MESH_DIRECT && !actual_mesh)
    {
        return false;
    }

    let dashboard_present = actual
        .services
        .as_ref()
        .is_some_and(|services| services.iter().any(|s| s.dashboard.is_some()));
    if dashboard_present && actual.outbounds.iter().any(|o| o.tag == DASHBOARD_SELECTOR) {
        return false;
    }
    if actual.experimental.is_none() {
        return false;
    }
    let dashboard_selector = if dashboard_present {
        let selector = serde_json::from_value::<Outbound>(serde_json::json!({
            "type": "selector", "tag": DASHBOARD_SELECTOR,
            "outbounds": ["direct", PROXY_SELECTOR_TAG],
            "default": if selected_mode == MESH_DIRECT { "direct" } else { PROXY_SELECTOR_TAG }
        }))
        .ok();
        let Some(selector) = selector else {
            return false;
        };
        Some(selector)
    } else {
        None
    };
    // No partial mutation: every fallible comparison and conversion above has completed.
    let mut staged = actual.clone();
    let mut compiled_route = normal_route.clone();
    compiled_route.rules = route_rules;
    compiled_route.final_outbound = Some("direct".into());
    let mut compiled_dns = normal_dns.clone();
    compiled_dns.servers = servers;
    compiled_dns.rules = Some(dns_rules);
    compiled_dns.final_server = normal_dns.final_server.clone();
    staged.route = Some(compiled_route);
    staged.dns = Some(compiled_dns);
    let mode = selected_mode;
    if let Some(experimental) = staged.experimental.as_mut() {
        experimental.clash_api = Some(ClashApi {
            default_mode: mode.into(),
        });
    }
    if let Some(selector) = dashboard_selector {
        staged.outbounds.push(selector);
        if let Some(services) = staged.services.as_mut() {
            for service in services {
                if let Some(dashboard) = service.dashboard.as_mut() {
                    if let Some(client) = dashboard.http_client.as_mut() {
                        client.detour = DASHBOARD_SELECTOR.into();
                    }
                }
            }
        }
    }
    *actual = staged;
    true
}
