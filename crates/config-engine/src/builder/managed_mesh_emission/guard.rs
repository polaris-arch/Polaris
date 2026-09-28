//! Bounded rule expansion for multi-answer route decisions. In b609 an
//! ip_cidr matcher accepts *any* resolved address, while the dialer receives
//! the complete answer set. A terminal owner/override therefore needs a
//! preceding mixed-answer reject for every set on which it depends.

use std::collections::BTreeMap;

use super::*;
use crate::user_config::cidr::{cidr_contains, cidrs_overlap, subtract_cidrs};
use crate::user_config::mesh_route_state::MeshOwnerRef;

const MAX_WORK: usize = 262_144;
const MAX_INTERMEDIATE_CIDRS: usize = 4096;
const MAX_ROUTE_CIDRS: usize = 32_768;
const MAX_ROUTE_NODES: usize = 16_384;
const MAX_ROUTE_RULE_BYTES: usize = 4 * 1024 * 1024;
const MAX_MATCHER_BRANCHES: usize = 128;

#[derive(Default)]
struct Budget {
    work: usize,
}

impl Budget {
    fn charge(&mut self, count: usize) -> Result<(), String> {
        self.work = self.work.saturating_add(count);
        if self.work > MAX_WORK {
            return Err("managed route work budget exceeded".into());
        }
        Ok(())
    }

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
                self.charge(1)?;
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

    fn intersect(&mut self, left: &[String], right: &[String]) -> Result<Vec<String>, String> {
        let mut result = Vec::new();
        for a in left {
            for b in right {
                self.charge(1)?;
                if !cidrs_overlap(a, b) {
                    continue;
                }
                let narrower = if cidr_contains(a, b) { b } else { a };
                let remaining = self.subtract(vec![narrower.clone()], &result)?;
                result.extend(remaining);
                if result.len() > MAX_INTERMEDIATE_CIDRS {
                    return Err("managed route CIDR budget exceeded".into());
                }
            }
        }
        Ok(result)
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

#[derive(Clone)]
struct Branch {
    leaves: Vec<RouteRule>,
    /// Intersection of the destination IP atoms in this conjunction. None
    /// means this branch has no destination IP constraint.
    ip_set: Option<Vec<String>>,
}

fn branch_leaf(matcher: RouteRule) -> Branch {
    Branch {
        ip_set: matcher
            .ip_cidr
            .as_ref()
            .filter(|set| !set.is_empty())
            .cloned(),
        leaves: vec![matcher],
    }
}

fn matcher_branches(matcher: &RouteRule, budget: &mut Budget) -> Result<Vec<Branch>, String> {
    if matcher.type_field.as_deref() != Some("logical") {
        let has_ip = matcher
            .ip_cidr
            .as_ref()
            .is_some_and(|cidrs| !cidrs.is_empty());
        let has_domain = [
            matcher.domain.as_ref(),
            matcher.domain_suffix.as_ref(),
            matcher.domain_keyword.as_ref(),
            matcher.domain_regex.as_ref(),
        ]
        .iter()
        .any(|field| field.is_some_and(|values| !values.is_empty()));
        if has_ip && has_domain {
            // b609's destination group ORs domain and ip_cidr inside a
            // default rule. Keep all other (source/port/process) axes in both.
            let mut domain = matcher.clone();
            domain.ip_cidr = None;
            let mut ip = matcher.clone();
            ip.domain = None;
            ip.domain_suffix = None;
            ip.domain_keyword = None;
            ip.domain_regex = None;
            return Ok(vec![branch_leaf(domain), branch_leaf(ip)]);
        }
        return Ok(vec![branch_leaf(matcher.clone())]);
    }
    let children = matcher
        .rules
        .as_ref()
        .ok_or("managed logical override has no children")?;
    let mut branches = if matcher.mode.as_deref() == Some("and") {
        vec![Branch {
            leaves: Vec::new(),
            ip_set: None,
        }]
    } else if matcher.mode.as_deref() == Some("or") {
        Vec::new()
    } else {
        return Err("managed logical override has an unsupported mode".into());
    };
    for child in children {
        let next = matcher_branches(child, budget)?;
        if matcher.mode.as_deref() == Some("or") {
            budget.charge(next.len())?;
            branches.extend(next);
        } else {
            budget.charge(branches.len().saturating_mul(next.len()))?;
            let mut product = Vec::new();
            for left in &branches {
                for right in &next {
                    let ip_set = match (&left.ip_set, &right.ip_set) {
                        (Some(a), Some(b)) => Some(budget.intersect(a, b)?),
                        (Some(a), None) => Some(a.clone()),
                        (None, Some(b)) => Some(b.clone()),
                        (None, None) => None,
                    };
                    let mut leaves = left.leaves.clone();
                    leaves.extend(right.leaves.clone());
                    product.push(Branch { leaves, ip_set });
                    if product.len() > MAX_MATCHER_BRANCHES {
                        return Err("managed override branch budget exceeded".into());
                    }
                }
            }
            branches = product;
        }
        if branches.len() > MAX_MATCHER_BRANCHES {
            return Err("managed override branch budget exceeded".into());
        }
    }
    Ok(branches)
}

fn non_ip_activation(branch: &Branch) -> Option<RouteRule> {
    let mut leaves = Vec::new();
    for leaf in &branch.leaves {
        let mut without_ip = leaf.clone();
        without_ip.ip_cidr = None;
        if without_ip != RouteRule::default() {
            leaves.push(without_ip);
        }
    }
    match leaves.len() {
        0 => None,
        1 => leaves.pop(),
        _ => Some(RouteRule {
            type_field: Some("logical".into()),
            mode: Some("and".into()),
            rules: Some(leaves),
            ..Default::default()
        }),
    }
}

struct Region {
    cidr: String,
    covering_branches: Vec<usize>,
}

fn override_rejects(
    gate: &RouteRule,
    matcher: &RouteRule,
    scope: &[String],
    q: &[String],
    budget: &mut Budget,
) -> Result<Vec<RouteRule>, String> {
    let branches = matcher_branches(matcher, budget)?;
    let mut activations = Vec::new();
    // Even outside-scope regions are needed: b609's ip_cidr is any-match over
    // the answer set, so one in-scope answer can activate the terminal gate
    // while another answer outside the scope remains available to the dialer.
    let mut regions: Vec<_> = q
        .iter()
        .map(|cidr| Region {
            cidr: cidr.clone(),
            covering_branches: Vec::new(),
        })
        .collect();
    for (index, branch) in branches.iter().enumerate() {
        let allowed = match &branch.ip_set {
            Some(ip_set) => budget.intersect(scope, ip_set)?,
            None => scope.to_vec(),
        };
        activations.push(non_ip_activation(branch));
        if allowed.is_empty() {
            continue;
        }
        let mut next = Vec::new();
        for region in regions {
            let inside = budget.intersect(std::slice::from_ref(&region.cidr), &allowed)?;
            let outside = budget.subtract(vec![region.cidr.clone()], &allowed)?;
            for cidr in inside {
                let mut covering_branches = region.covering_branches.clone();
                covering_branches.push(index);
                next.push(Region {
                    cidr,
                    covering_branches,
                });
            }
            for cidr in outside {
                next.push(Region {
                    cidr,
                    covering_branches: region.covering_branches.clone(),
                });
            }
            if next.len() > MAX_INTERMEDIATE_CIDRS {
                return Err("managed override region budget exceeded".into());
            }
        }
        regions = next;
    }
    let mut rejects = Vec::new();
    for region in regions {
        // An unconditional branch that covers this region makes every answer
        // there safe. Other branches can only add permissions, not revoke it.
        if region
            .covering_branches
            .iter()
            .any(|index| activations[*index].is_none())
        {
            continue;
        }
        let mut children = vec![gate.clone(), ip_match(vec![region.cidr])];
        let active: Vec<_> = region
            .covering_branches
            .iter()
            .filter_map(|index| activations[*index].clone())
            .collect();
        if !active.is_empty() {
            children.push(RouteRule {
                type_field: Some("logical".into()),
                mode: Some("or".into()),
                rules: Some(active),
                invert: Some(true),
                ..Default::default()
            });
        }
        let mut reject_rule = RouteRule {
            type_field: Some("logical".into()),
            mode: Some("and".into()),
            rules: Some(children),
            ..Default::default()
        };
        reject(&mut reject_rule);
        rejects.push(reject_rule);
        if rejects.len() > MAX_ROUTE_NODES {
            return Err("managed route rule node budget exceeded".into());
        }
    }
    Ok(rejects)
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
        rules.extend(override_rejects(
            &gate,
            &override_rule.matcher,
            &override_rule.scope_cidrs,
            q,
            &mut budget,
        )?);
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
