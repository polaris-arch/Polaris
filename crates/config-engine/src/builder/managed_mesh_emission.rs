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

#[derive(Debug, Clone, PartialEq)]
pub struct ManagedMeshEmission {
    pub config: SingBoxConfig,
    pub dns: MeshDnsBuild,
    /// Ordinary TCP/UDP 53 to Q still reaches the generic DNS hijack before
    /// owner/Q rules. S4 must disclose this scoped exception in preview/ACK.
    pub ordinary_port53_hijack_cidrs: Vec<String>,
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

    // Reorder only rules whose exact baseline shape is known. Internal
    // probe/update inbounds would otherwise either resolve before their pin
    // or bypass Q unresolved, so they block this managed emitter for now.
    let hijack_index = route
        .rules
        .iter()
        .position(|rule| rule.action.as_deref() == Some("hijack-dns") && has_port(rule, 53))
        .ok_or("generic DNS hijack rule is missing")?;
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
    for (index, rule) in route.rules.iter().enumerate() {
        if rule.inbound.is_some() && !subscription_indices.contains(&index) {
            return Err("managed mesh cannot preserve a probe or update inbound pin".into());
        }
        if rule.action.as_deref() == Some("resolve") && !subscription_indices.contains(&index) {
            if rule != &bare_resolve || legacy_resolve_index.replace(index).is_some() {
                return Err("managed mesh cannot place a scoped legacy resolve after Q".into());
            }
        }
        if rule.action.as_deref() == Some("route")
            && (rule.domain.is_some()
                || rule.domain_suffix.is_some()
                || rule.domain_keyword.is_some()
                || rule.domain_regex.is_some())
        {
            return Err(
                "managed mesh needs an explicit exception for a domain-pinned route".into(),
            );
        }
    }
    let core = route.rules[core_index].clone();
    let bootstrap = route.rules[bootstrap_index].clone();
    let hijack = route.rules[hijack_index].clone();
    let mut removed = vec![core_index, bootstrap_index, hijack_index];
    removed.extend(subscription_indices);
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
    managed.push(bootstrap);
    managed.push(hijack);
    let mut managed_resolve = bare_resolve;
    if subscription.is_some() {
        managed_resolve.inbound =
            Some(OneOrMany::Many(
                vec![SUBSCRIPTION_UPDATE_INBOUND_TAG.into()],
            ));
        managed_resolve.invert = Some(true);
    }
    managed.push(managed_resolve);

    for override_rule in &plan.overrides {
        if override_rule.scope_cidrs.is_empty() {
            continue;
        }
        let mut rule = RouteRule {
            type_field: Some("logical".into()),
            mode: Some("and".into()),
            rules: Some(vec![
                override_rule.matcher.clone(),
                RouteRule {
                    ip_cidr: Some(override_rule.scope_cidrs.clone()),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        set_plan_target(&mut rule, &override_rule.target)?;
        managed.push(rule);
    }
    for owner_route in &plan.owner_routes {
        let rule = RouteRule {
            ip_cidr: Some(vec![owner_route.cidr.clone()]),
            action: Some("route".into()),
            outbound: Some(owner_route.endpoint_tag.clone()),
            ..Default::default()
        };
        managed.push(rule);
    }
    for cidr in &plan.reject_cidrs {
        let mut rule = RouteRule {
            ip_cidr: Some(vec![cidr.clone()]),
            ..Default::default()
        };
        reject(&mut rule);
        managed.push(rule);
    }
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
    Ok(ManagedMeshEmission {
        config,
        dns,
        ordinary_port53_hijack_cidrs: plan.protected_cidrs.clone(),
    })
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
