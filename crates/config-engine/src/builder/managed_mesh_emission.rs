//! Pure final-config emitter for one immutable managed mesh plan. S4 owns the
//! transaction, files, core check and platform receipt before this can run.

use crate::builder::managed_mesh_plan::{
    compile_managed_mesh_plan, ManagedMeshPlanInput, ManagedMeshRoutePlan, ManagedPlanTarget,
};
use crate::builder::mesh_dns::{
    build_mesh_dns_overlay, MeshDnsBuild, MeshDnsEmittedEndpoint, MeshDnsTargetDecision,
    MAGIC_DNS_SERVICE_CIDRS, MAGIC_DNS_SERVICE_PORT, MAGIC_DNS_SERVICE_TRANSPORTS,
};
use crate::builder::subscription_guard::{
    subscription_update_route_rules, SUBSCRIPTION_UPDATE_INBOUND_TAG,
};
use crate::singbox::{OneOrMany, RouteRule, SingBoxConfig};
use crate::user_config::cidr::cidrs_overlap;

mod guard;

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedMeshEmission {
    pub config: SingBoxConfig,
    pub dns: MeshDnsBuild,
    /// Ordinary TCP/UDP 53 to Q still reaches the generic DNS hijack before
    /// owner/Q rules. S4 must disclose this scoped exception in preview/ACK.
    pub ordinary_port53_hijack_cidrs: Vec<String>,
    /// Internal probe/update inbounds have dedicated DNS resolution and a Q
    /// reject before their original pinned outbound. S4 must show these
    /// exceptions in the preview alongside ordinary port-53 hijack.
    pub internal_inbound_exceptions: Vec<String>,
}

/// The input, plan and already generated legacy config must come from one
/// snapshot. Recompiling verifies the plan's identity, scopes and endpoint
/// tags; the installed endpoint list is checked separately. This function
/// neither writes route-set files nor starts the core.
pub fn emit_managed_mesh_config(
    legacy: &SingBoxConfig,
    input: &ManagedMeshPlanInput,
    plan: &ManagedMeshRoutePlan,
) -> Result<ManagedMeshEmission, String> {
    let expected = compile_managed_mesh_plan(input.clone())?;
    if &expected != plan {
        return Err("managed mesh plan differs from its input snapshot".into());
    }
    let mut config = legacy.clone();
    let emitted = input
        .candidates
        .iter()
        .filter_map(|candidate| {
            candidate
                .endpoint_tag
                .as_ref()
                .map(|tag| MeshDnsEmittedEndpoint {
                    owner: candidate.owner_ref.clone(),
                    tag: tag.clone(),
                })
        })
        .collect::<Vec<_>>();
    let endpoints = config.endpoints.as_deref().unwrap_or_default();
    for candidate in &emitted {
        if endpoints
            .iter()
            .filter(|endpoint| endpoint.tag == candidate.tag)
            .count()
            != 1
            || !endpoints
                .iter()
                .any(|endpoint| endpoint.tag == candidate.tag && endpoint.type_field == "tailscale")
            || config
                .outbounds
                .iter()
                .any(|outbound| outbound.tag == candidate.tag)
        {
            return Err(format!(
                "managed endpoint is absent or ambiguous: {}",
                candidate.owner.server_id
            ));
        }
    }
    let legacy_dns = config
        .dns
        .as_ref()
        .ok_or("generated config has no DNS section")?;
    let dns = build_mesh_dns_overlay(
        legacy_dns,
        input.policy.dns_policy.as_ref(),
        Some(&input.state),
        &emitted,
    )
    .map_err(|error| format!("managed DNS compilation failed: {error:?}"))?;
    config.dns = Some(dns.dns.clone());
    let route = config
        .route
        .as_mut()
        .ok_or("generated config has no route section")?;

    // Move only exact built-in exceptions ahead of the managed Q settlement.
    // Unknown inbound or local override shapes cannot silently bypass it.
    let generic_hijack = RouteRule {
        port: Some(OneOrMany::Many(vec![53])),
        action: Some("hijack-dns".into()),
        ..Default::default()
    };
    let mut hijack_indices = route
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| (rule == &generic_hijack).then_some(index));
    let hijack_index = hijack_indices
        .next()
        .ok_or("generic DNS hijack rule is missing")?;
    if hijack_indices.next().is_some() {
        return Err("generic DNS hijack rule is ambiguous".into());
    }
    let bootstrap_index = hijack_index
        .checked_sub(1)
        .ok_or("bootstrap DNS direct rule is missing")?;
    let bootstrap = &route.rules[bootstrap_index];
    if bootstrap.action.as_deref() != Some("route")
        || bootstrap.outbound.as_deref() != Some("direct")
        || bootstrap.ip_cidr.is_none()
        || !has_port(bootstrap, 53)
    {
        return Err("bootstrap DNS direct rule changed shape".into());
    }
    if bootstrap.ip_cidr.as_ref().is_some_and(|cidrs| {
        cidrs.iter().any(|cidr| {
            plan.protected_cidrs
                .iter()
                .any(|scope| cidrs_overlap(cidr, scope))
        })
    }) {
        return Err("bootstrap DNS direct overlaps managed protection".into());
    }
    let core_index = route.rules[..bootstrap_index]
        .iter()
        .position(|rule| {
            rule.action.as_deref() == Some("route")
                && rule.outbound.as_deref() == Some("direct")
                && matches!(&rule.process_name,
                    Some(OneOrMany::Many(names)) if names.iter().any(|name| name == "sing-box"))
        })
        .ok_or("core process anti-loop rule is missing")?;
    let subscription_indices: Vec<_> = route
        .rules
        .iter()
        .enumerate()
        .filter_map(|(index, rule)| has_subscription_inbound(rule).then_some(index))
        .collect();
    let subscription = if subscription_indices.is_empty() {
        None
    } else {
        if subscription_indices.len() != 3
            || subscription_indices[1] != subscription_indices[0] + 1
            || subscription_indices[2] != subscription_indices[1] + 1
        {
            return Err("subscription update guard is incomplete".into());
        }
        let start = subscription_indices[0];
        let pin = &route.rules[start + 2];
        let outbound = pin
            .outbound
            .as_deref()
            .ok_or("subscription update pin has no outbound")?;
        let expected = subscription_update_route_rules(outbound);
        if route.rules[start..start + 3] != expected {
            return Err("subscription update guard changed shape".into());
        }
        Some(expected)
    };
    let bare_resolve = RouteRule {
        action: Some("resolve".into()),
        ..Default::default()
    };
    let mut legacy_resolve_index = None;
    let mut internal_pins = Vec::new();
    let mut local_override_index = None;
    for (index, rule) in route.rules.iter().enumerate() {
        if rule.inbound.is_some() && !subscription_indices.contains(&index) {
            if is_exact_userspace_local_route(rule, endpoints) {
                continue;
            }
            let Some((tag, resolver)) = internal_pin_resolver(rule) else {
                return Err("managed mesh cannot preserve an unknown inbound pin".into());
            };
            if internal_pins.iter().any(|(_, known, _)| known == &tag)
                || !dns.dns.servers.iter().any(|server| server.tag == resolver)
            {
                return Err("managed mesh internal pin has no unique DNS resolver".into());
            }
            internal_pins.push((index, tag, resolver));
        }
        if rule.action.as_deref() == Some("resolve")
            && !subscription_indices.contains(&index)
            && (rule != &bare_resolve || legacy_resolve_index.replace(index).is_some())
        {
            return Err("managed mesh cannot place a scoped legacy resolve after Q".into());
        }
        if rule.override_address.is_some()
            && (!is_exact_microdone_local_override(rule)
                || local_override_index.replace(index).is_some()
                || plan
                    .protected_cidrs
                    .iter()
                    .any(|q| cidrs_overlap(q, "127.0.0.1/32")))
        {
            return Err("managed mesh local override changed shape or overlaps Q".into());
        }
    }
    let core = route.rules[core_index].clone();
    let bootstrap = route.rules[bootstrap_index].clone();
    let hijack = route.rules[hijack_index].clone();
    let local_override = local_override_index.map(|index| route.rules[index].clone());
    let pinned_rules: Vec<_> = internal_pins
        .iter()
        .map(|(index, tag, resolver)| (route.rules[*index].clone(), tag.clone(), resolver.clone()))
        .collect();
    let mut removed = vec![core_index, bootstrap_index, hijack_index];
    removed.extend(subscription_indices);
    removed.extend(internal_pins.iter().map(|(index, _, _)| *index));
    if let Some(index) = local_override_index {
        removed.push(index);
    }
    if let Some(index) = legacy_resolve_index {
        removed.push(index);
    }
    removed.sort_unstable();
    removed.dedup();
    for index in removed.into_iter().rev() {
        route.rules.remove(index);
    }
    let mut managed = vec![core];
    if let Some(sub) = &subscription {
        managed.extend(sub[..2].iter().cloned());
    }
    for (_, tag, resolver) in &pinned_rules {
        managed.push(RouteRule {
            inbound: Some(OneOrMany::Many(vec![tag.clone()])),
            action: Some("resolve".into()),
            server: Some(resolver.clone()),
            ..Default::default()
        });
        let mut reject_q = RouteRule {
            inbound: Some(OneOrMany::Many(vec![tag.clone()])),
            ip_cidr: Some(plan.protected_cidrs.clone()),
            ..Default::default()
        };
        reject(&mut reject_q);
        managed.push(reject_q);
    }
    if let Some(service) = &dns.service {
        let mut rule = RouteRule {
            ip_cidr: Some(MAGIC_DNS_SERVICE_CIDRS.map(str::to_owned).to_vec()),
            network: Some(MAGIC_DNS_SERVICE_TRANSPORTS.map(str::to_owned).to_vec()),
            port: Some(OneOrMany::Many(vec![u32::from(MAGIC_DNS_SERVICE_PORT)])),
            ..Default::default()
        };
        set_target(&mut rule, service);
        managed.push(rule);
    }
    managed.extend(pinned_rules.iter().map(|(pin, _, _)| pin.clone()));
    if let Some(local_override) = local_override {
        managed.push(local_override);
    }
    managed.push(bootstrap);
    managed.push(hijack);
    let mut managed_resolve = bare_resolve;
    let mut exceptions: Vec<String> = pinned_rules.iter().map(|(_, tag, _)| tag.clone()).collect();
    if subscription.is_some() {
        exceptions.push(SUBSCRIPTION_UPDATE_INBOUND_TAG.into());
    }
    if !exceptions.is_empty() {
        managed_resolve.inbound = Some(OneOrMany::Many(exceptions.clone()));
        managed_resolve.invert = Some(true);
    }
    managed.push(managed_resolve);

    managed.extend(guard::guarded_rules(plan)?);
    if let Some(sub) = subscription {
        managed.push(sub[2].clone());
    }

    let sniff_end = route
        .rules
        .iter()
        .take_while(|rule| rule.action.as_deref() == Some("sniff"))
        .count();
    if sniff_end == 0 {
        return Err("leading route sniff rule is missing".into());
    }
    route.rules.splice(sniff_end..sniff_end, managed);
    guard::check_final_route_budget(&route.rules)?;
    Ok(ManagedMeshEmission {
        config,
        dns,
        ordinary_port53_hijack_cidrs: plan.protected_cidrs.clone(),
        internal_inbound_exceptions: exceptions,
    })
}

fn internal_pin_resolver(rule: &RouteRule) -> Option<(String, String)> {
    let tag = match &rule.inbound {
        Some(OneOrMany::Many(tags)) if tags.len() == 1 => tags[0].as_str(),
        _ => return None,
    };
    let outbound = rule.outbound.as_deref()?;
    let resolver = match tag {
        "probe-direct-in" if outbound == "direct" => "dns-bootstrap".to_owned(),
        crate::builder::helpers::PROBE_PROXY_INBOUND_TAG => "dns-probe-exit-proxy".to_owned(),
        "update-in" if outbound == "direct" => "dns-bootstrap".to_owned(),
        "update-in" => "dns-remote".to_owned(),
        _ => {
            let index = tag.strip_prefix("probe-in-")?;
            if index.is_empty()
                || index.parse::<usize>().ok()?.to_string() != index
                || outbound != format!("probe-selector-{index}")
            {
                return None;
            }
            format!("dns-probe-exit-{index}")
        }
    };
    let expected = RouteRule {
        inbound: Some(OneOrMany::Many(vec![tag.to_owned()])),
        action: Some("route".into()),
        outbound: Some(outbound.to_owned()),
        ..Default::default()
    };
    (rule == &expected).then_some((tag.to_owned(), resolver))
}

fn is_exact_microdone_local_override(rule: &RouteRule) -> bool {
    rule == &RouteRule {
        domain_suffix: Some(vec![".microdone.cn".into()]),
        action: Some("route".into()),
        outbound: Some("direct".into()),
        override_address: Some("127.0.0.1".into()),
        ..Default::default()
    }
}

fn is_exact_userspace_local_route(
    rule: &RouteRule,
    endpoints: &[crate::singbox::Endpoint],
) -> bool {
    let Some(OneOrMany::Many(tags)) = &rule.inbound else {
        return false;
    };
    if tags.len() != 1
        || !endpoints
            .iter()
            .any(|endpoint| endpoint.tag == tags[0] && endpoint.type_field == "tailscale")
    {
        return false;
    }
    rule == &RouteRule {
        inbound: Some(OneOrMany::Many(tags.clone())),
        ip_cidr: Some(vec!["127.0.0.1/32".into(), "::1/128".into()]),
        action: Some("route".into()),
        outbound: Some("direct".into()),
        ..Default::default()
    }
}

fn has_subscription_inbound(rule: &RouteRule) -> bool {
    match &rule.inbound {
        Some(OneOrMany::One(tag)) => tag == SUBSCRIPTION_UPDATE_INBOUND_TAG,
        Some(OneOrMany::Many(tags)) => tags
            .iter()
            .any(|tag| tag == SUBSCRIPTION_UPDATE_INBOUND_TAG),
        None => false,
    }
}

fn has_port(rule: &RouteRule, port: u32) -> bool {
    match &rule.port {
        Some(OneOrMany::One(value)) => *value == port,
        Some(OneOrMany::Many(values)) => values.contains(&port),
        None => false,
    }
}

fn reject(rule: &mut RouteRule) {
    rule.action = Some("reject".into());
    rule.no_drop = Some(true);
}

fn set_target(rule: &mut RouteRule, target: &MeshDnsTargetDecision) {
    match target {
        MeshDnsTargetDecision::Owner { endpoint_tag, .. } => {
            rule.action = Some("route".into());
            rule.outbound = Some(endpoint_tag.clone());
        }
        MeshDnsTargetDecision::Reject(_) => reject(rule),
    }
}

fn set_plan_target(rule: &mut RouteRule, target: &ManagedPlanTarget) -> Result<(), String> {
    match target {
        ManagedPlanTarget::Owner { endpoint_tag, .. } => {
            rule.action = Some("route".into());
            rule.outbound = Some(endpoint_tag.clone());
        }
        ManagedPlanTarget::Reject => reject(rule),
        ManagedPlanTarget::Unmanaged => {
            return Err("unmanaged scoped override needs an explicit continuation rule".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
