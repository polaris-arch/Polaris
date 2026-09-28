//! Bounded rule expansion for multi-answer route decisions. In b609 an
//! ip_cidr matcher accepts *any* resolved address, while the dialer receives
//! the complete answer set. A terminal owner/override therefore needs a
//! preceding mixed-answer reject for every set on which it depends.

use std::collections::BTreeMap;

use super::*;
use crate::user_config::cidr::subtract_cidrs;
use crate::user_config::mesh_route_state::MeshOwnerRef;

const MAX_WORK: usize = 262_144;
const MAX_INTERMEDIATE_CIDRS: usize = 4096;
const MAX_ROUTE_CIDRS: usize = 32_768;
const MAX_ROUTE_NODES: usize = 16_384;
const MAX_ROUTE_RULE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct Budget {
    work: usize,
}

impl Budget {
    fn subtract(
        &mut self,
        mut pieces: Vec<String>,
        carve: &[String],
    ) -> Result<Vec<String>, String> {
        if pieces.len() > MAX_INTERMEDIATE_CIDRS || carve.len() > MAX_INTERMEDIATE_CIDRS {
            return Err("managed route CIDR budget exceeded".into());
        }
        for excluded in carve {
            let mut next = Vec::new();
            for piece in pieces {
                self.work = self.work.saturating_add(1);
                if self.work > MAX_WORK {
                    return Err("managed route work budget exceeded".into());
                }
                next.extend(subtract_cidrs(&[piece], std::slice::from_ref(excluded)));
                if next.len() > MAX_INTERMEDIATE_CIDRS {
                    return Err("managed route CIDR budget exceeded".into());
                }
            }
            pieces = next;
            if pieces.is_empty() {
                break;
            }
        }
        Ok(pieces)
    }
}

fn ip_match(cidrs: Vec<String>) -> RouteRule {
    RouteRule {
        ip_cidr: Some(cidrs),
        ..Default::default()
    }
}

fn mixed_reject(gate: Option<RouteRule>, allowed: Vec<String>, outside: Vec<String>) -> RouteRule {
    let mut children = Vec::with_capacity(3);
    if let Some(gate) = gate {
        children.push(gate);
    }
    children.push(ip_match(allowed));
    children.push(ip_match(outside));
    let mut rule = RouteRule {
        type_field: Some("logical".into()),
        mode: Some("and".into()),
        rules: Some(children),
        ..Default::default()
    };
    reject(&mut rule);
    rule
}

fn matcher_ip_atoms(matcher: &RouteRule, out: &mut Vec<Vec<String>>) {
    if let Some(cidrs) = &matcher.ip_cidr {
        out.push(cidrs.clone());
    }
    if let Some(children) = &matcher.rules {
        for child in children {
            matcher_ip_atoms(child, out);
        }
    }
}

pub(super) fn guarded_rules(plan: &ManagedMeshRoutePlan) -> Result<Vec<RouteRule>, String> {
    let mut budget = Budget::default();
    let q = &plan.protected_cidrs;
    let mut rules = Vec::new();
    let universe = vec!["0.0.0.0/0".to_owned(), "::/0".to_owned()];
    let outside_q = budget.subtract(universe, q)?;
    if !outside_q.is_empty() {
        rules.push(mixed_reject(None, q.clone(), outside_q));
    }

    for override_rule in &plan.overrides {
        if override_rule.scope_cidrs.is_empty() {
            continue;
        }
        let gate = RouteRule {
            type_field: Some("logical".into()),
            mode: Some("and".into()),
            rules: Some(vec![
                override_rule.matcher.clone(),
                ip_match(override_rule.scope_cidrs.clone()),
            ]),
            ..Default::default()
        };
        let mut atoms = vec![override_rule.scope_cidrs.clone()];
        matcher_ip_atoms(&override_rule.matcher, &mut atoms);
        for atom in atoms {
            let outside_atom = budget.subtract(q.clone(), &atom)?;
            if !outside_atom.is_empty() {
                rules.push(mixed_reject(Some(gate.clone()), atom, outside_atom));
            }
            if rules.len() > MAX_ROUTE_NODES {
                return Err("managed route rule node budget exceeded".into());
            }
        }
        let mut target = gate;
        set_plan_target(&mut target, &override_rule.target)?;
        rules.push(target);
    }

    // One answer in each of two fragments owned by the same endpoint is safe.
    // Group first, then compare the complete owner set against Q.
    let mut owners: BTreeMap<(MeshOwnerRef, String), Vec<String>> = BTreeMap::new();
    for owner_route in &plan.owner_routes {
        owners
            .entry((
                owner_route.owner_ref.clone(),
                owner_route.endpoint_tag.clone(),
            ))
            .or_default()
            .push(owner_route.cidr.clone());
    }
    for ((_owner_ref, endpoint_tag), owner_cidrs) in owners {
        let other_q = budget.subtract(q.clone(), &owner_cidrs)?;
        if !other_q.is_empty() {
            rules.push(mixed_reject(None, owner_cidrs.clone(), other_q));
        }
        rules.push(RouteRule {
            ip_cidr: Some(owner_cidrs),
            action: Some("route".into()),
            outbound: Some(endpoint_tag),
            ..Default::default()
        });
    }
    for cidr in &plan.reject_cidrs {
        let mut rule = ip_match(vec![cidr.clone()]);
        reject(&mut rule);
        rules.push(rule);
    }
    Ok(rules)
}

/// Count the entire emitted route tree, including retained legacy rules and
/// repeated nested matcher CIDRs. This prevents guard expansion from hiding
/// behind a small top-level rule count.
pub(super) fn check_final_route_budget(rules: &[RouteRule]) -> Result<(), String> {
    let mut cidrs = 0usize;
    let mut nodes = 0usize;
    let mut pending: Vec<_> = rules.iter().collect();
    while let Some(rule) = pending.pop() {
        nodes = nodes.saturating_add(1);
        if nodes > MAX_ROUTE_NODES {
            return Err("managed route rule node budget exceeded".into());
        }
        cidrs = cidrs
            .saturating_add(rule.ip_cidr.as_ref().map_or(0, Vec::len))
            .saturating_add(rule.source_ip_cidr.as_ref().map_or(0, Vec::len));
        if let Some(children) = &rule.rules {
            pending.extend(children);
        }
    }
    if cidrs > MAX_ROUTE_CIDRS {
        return Err("managed route CIDR budget exceeded".into());
    }
    let bytes = serde_json::to_vec(rules).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_ROUTE_RULE_BYTES {
        return Err("managed route rule byte budget exceeded".into());
    }
    Ok(())
}
