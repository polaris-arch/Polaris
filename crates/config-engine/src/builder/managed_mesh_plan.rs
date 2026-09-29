//! S3b's pure, bounded ownership settlement. No file, OS, core or credential
//! access occurs here. The same result must later drive route rules, rule-set
//! files, platform scope and the preview; it is not an Apply receipt by itself.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::builder::endpoint_routes::{
    TAILNET_CGNAT, TAILNET_MAGICDNS_V4, TAILNET_MAGICDNS_V6, TAILNET_ULA_V6,
};
use crate::singbox::RouteRule;
use crate::user_config::cidr::{cidr_contains, cidrs_overlap, normalize_cidr, subtract_cidrs};
use crate::user_config::mesh_route_state::{
    MeshBindingState, MeshDnsOwnerTarget, MeshDnsShortNamePolicy, MeshOwnerRef,
    MeshReservationOwner, MeshRoutePolicy, MeshRouteState, MeshTarget,
};

const MAX_PLAN_CIDRS: usize = 4096;
const MAX_PLAN_WORK: usize = 262_144;
const MAX_OVERRIDE_MATCHER_BYTES: usize = 2048;
const MAX_OVERRIDE_MATCHER_NODES: usize = 128;
const SERVICE_CIDRS: [&str; 2] = [TAILNET_MAGICDNS_V4, TAILNET_MAGICDNS_V6];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedMeshCandidate {
    pub owner_ref: MeshOwnerRef,
    /// Config-declared candidates, excluding observations already in state.
    pub configured_cidrs: Vec<String>,
    /// Exact generated endpoint tag. Missing endpoint never grants a route.
    pub endpoint_tag: Option<String>,
    /// Opaque or failed file evidence cannot be interpreted as an empty set.
    pub evidence_complete: bool,
}

#[derive(Debug, Clone)]
pub struct ManagedMeshPlanInput {
    pub plan_id: String,
    pub config_version: String,
    pub policy: MeshRoutePolicy,
    pub state: MeshRouteState,
    pub candidates: Vec<ManagedMeshCandidate>,
    /// Actual pure matchers from the same config snapshot. The plan carries
    /// them forward so final emission does not re-read a possibly newer rule.
    pub scopeable_rule_matchers: BTreeMap<String, RouteRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedOwnerRoute {
    pub cidr: String,
    pub owner_ref: MeshOwnerRef,
    pub endpoint_tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum ManagedPlanTarget {
    Owner {
        #[serde(rename = "ownerRef")]
        owner_ref: MeshOwnerRef,
        #[serde(rename = "endpointTag")]
        endpoint_tag: String,
    },
    Reject,
    Unmanaged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedScopedOverride {
    pub rule_id: String,
    pub scope_cidrs: Vec<String>,
    /// Final emitter must AND this matcher with scopeCidrs, then apply target.
    pub matcher: RouteRule,
    pub target: ManagedPlanTarget,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManagedMeshRoutePlan {
    pub schema_version: u32,
    pub plan_id: String,
    pub config_version: String,
    pub input_state_revision: String,
    pub identity_bindings: Vec<MeshOwnerRef>,
    /// Q: the canonical, disjoint destination set governed by this plan.
    pub protected_cidrs: Vec<String>,
    pub owner_routes: Vec<ManagedOwnerRoute>,
    /// Whole-Q fallback reject follows owner routes in final rule order.
    pub reject_cidrs: Vec<String>,
    /// Q minus effective owner fragments; useful for honest preview counts.
    pub unassigned_cidrs: Vec<String>,
    /// Explicit unconditional release; never inferred from an absent entry.
    pub released_cidrs: Vec<String>,
    pub overrides: Vec<ManagedScopedOverride>,
    /// DNS policy is an independent opt-in. D1 supplies its actual rule plan.
    pub dns_managed: bool,
}

#[derive(Default)]
struct Budget {
    work: usize,
}

impl Budget {
    fn charge(&mut self, count: usize) -> Result<(), String> {
        self.work = self.work.saturating_add(count);
        if self.work > MAX_PLAN_WORK {
            return Err("managed mesh plan work budget exceeded".into());
        }
        Ok(())
    }

    fn check_len(&self, count: usize) -> Result<(), String> {
        if count > MAX_PLAN_CIDRS {
            Err("managed mesh plan CIDR budget exceeded".into())
        } else {
            Ok(())
        }
    }
}

fn subtract_bounded(
    mut pieces: Vec<String>,
    carves: &[String],
    budget: &mut Budget,
) -> Result<Vec<String>, String> {
    budget.check_len(pieces.len())?;
    for carve in carves {
        let mut next = Vec::new();
        for piece in pieces {
            budget.charge(1)?;
            next.extend(subtract_cidrs(&[piece], std::slice::from_ref(carve)));
            budget.check_len(next.len())?;
        }
        pieces = next;
        if pieces.is_empty() {
            break;
        }
    }
    Ok(pieces)
}

fn add_disjoint(
    out: &mut Vec<String>,
    cidrs: impl IntoIterator<Item = String>,
    budget: &mut Budget,
) -> Result<(), String> {
    for cidr in cidrs {
        let remaining = subtract_bounded(vec![cidr], out, budget)?;
        out.extend(remaining);
        budget.check_len(out.len())?;
    }
    out.sort();
    Ok(())
}

fn normalized_without_service(raw: &str, budget: &mut Budget) -> Result<Vec<String>, String> {
    let normalized =
        normalize_cidr(raw).ok_or_else(|| format!("invalid managed mesh CIDR: {raw}"))?;
    if matches!(normalized.as_str(), "0.0.0.0/0" | "::/0") {
        return Err("catch-all CIDR cannot be owned by managed mesh".into());
    }
    let service = SERVICE_CIDRS.map(str::to_owned);
    subtract_bounded(vec![normalized], &service, budget)
}

fn add_scope(
    scopes: &mut Vec<String>,
    raw: &str,
    budget: &mut Budget,
) -> Result<Vec<String>, String> {
    let pieces = normalized_without_service(raw, budget)?;
    add_disjoint(scopes, pieces.clone(), budget)?;
    Ok(pieces)
}

fn prefix_len(cidr: &str) -> u16 {
    cidr.rsplit_once('/')
        .and_then(|(_, prefix)| prefix.parse().ok())
        .unwrap_or(0)
}

fn scope_intersection(a: &str, b: &str) -> Option<String> {
    if !cidrs_overlap(a, b) {
        None
    } else if cidr_contains(a, b) {
        Some(b.to_owned())
    } else {
        Some(a.to_owned())
    }
}

fn validate_pure_matcher(matcher: &RouteRule) -> Result<(), String> {
    let mut node_count = 0;
    validate_pure_matcher_node(matcher, 0, &mut node_count)
}

fn validate_pure_matcher_node(
    matcher: &RouteRule,
    depth: usize,
    node_count: &mut usize,
) -> Result<(), String> {
    if depth > 8 {
        return Err("managed override matcher nesting exceeded".into());
    }
    *node_count += 1;
    if *node_count > MAX_OVERRIDE_MATCHER_NODES {
        return Err("managed override matcher node budget exceeded".into());
    }
    // Bound the typed tree before serializing it. Serializing the root first
    // would recurse through attacker-sized `rules` before the depth check.
    if matcher.type_field.as_deref() == Some("logical") {
        let children = matcher
            .rules
            .as_ref()
            .ok_or("managed logical override has no children")?;
        if children.is_empty() || children.len() > 32 {
            return Err("managed logical override child budget exceeded".into());
        }
        for child in children {
            validate_pure_matcher_node(child, depth + 1, node_count)?;
        }
    }
    let value = serde_json::to_value(matcher).map_err(|error| error.to_string())?;
    if value.to_string().len() > MAX_OVERRIDE_MATCHER_BYTES {
        return Err("managed override matcher byte budget exceeded".into());
    }
    let fields = value
        .as_object()
        .ok_or("managed override matcher is not an object")?;
    if matcher.type_field.as_deref() == Some("logical") {
        if !matches!(matcher.mode.as_deref(), Some("and" | "or"))
            || fields
                .keys()
                .any(|key| !matches!(key.as_str(), "type" | "mode" | "rules"))
        {
            return Err("managed logical override is not a pure matcher".into());
        }
        return Ok(());
    }
    if matcher.type_field.is_some() || matcher.mode.is_some() || matcher.rules.is_some() {
        return Err("managed override has an unsupported logical shape".into());
    }
    const MATCH_FIELDS: &[&str] = &[
        "protocol",
        "network",
        "domain",
        "domain_suffix",
        "domain_keyword",
        "domain_regex",
        "ip_cidr",
        "source_ip_cidr",
        "port",
        "port_range",
        "source_port",
        "source_port_range",
        "source_mac_address",
        "source_hostname",
        "process_name",
        "process_path",
        "package_name",
        "process_name_not",
        "inbound",
    ];
    if fields.is_empty()
        || fields
            .keys()
            .any(|key| !MATCH_FIELDS.contains(&key.as_str()))
    {
        return Err("managed override contains an action, resource, or unsupported matcher".into());
    }
    Ok(())
}

fn resolved_owner(
    owner_ref: &MeshOwnerRef,
    state: &MeshRouteState,
    candidates: &BTreeMap<MeshOwnerRef, &ManagedMeshCandidate>,
) -> ManagedPlanTarget {
    let bound = state.identities.iter().any(|identity| {
        identity.server_id == owner_ref.server_id
            && identity.identity_epoch == owner_ref.identity_epoch
            && identity.binding_state == MeshBindingState::Bound
    });
    if !bound {
        return ManagedPlanTarget::Reject;
    }
    let Some(tag) = candidates
        .get(owner_ref)
        .and_then(|candidate| candidate.endpoint_tag.as_ref())
        .filter(|tag| !tag.is_empty())
    else {
        return ManagedPlanTarget::Reject;
    };
    ManagedPlanTarget::Owner {
        owner_ref: owner_ref.clone(),
        endpoint_tag: tag.clone(),
    }
}

fn resolve_target(
    target: &MeshTarget,
    state: &MeshRouteState,
    candidates: &BTreeMap<MeshOwnerRef, &ManagedMeshCandidate>,
) -> ManagedPlanTarget {
    match target {
        MeshTarget::Owner {
            server_id,
            identity_epoch,
        } => resolved_owner(
            &MeshOwnerRef {
                server_id: server_id.clone(),
                identity_epoch: identity_epoch.clone(),
            },
            state,
            candidates,
        ),
        MeshTarget::Reject => ManagedPlanTarget::Reject,
        MeshTarget::Unmanaged => ManagedPlanTarget::Unmanaged,
    }
}

fn claim(
    scope: Vec<String>,
    target: ManagedPlanTarget,
    claimed: &mut Vec<String>,
    owners: &mut Vec<ManagedOwnerRoute>,
    released: &mut Vec<String>,
    budget: &mut Budget,
) -> Result<(), String> {
    for cidr in scope {
        for fragment in subtract_bounded(vec![cidr], claimed, budget)? {
            add_disjoint(claimed, std::iter::once(fragment.clone()), budget)?;
            match &target {
                ManagedPlanTarget::Owner {
                    owner_ref,
                    endpoint_tag,
                } => owners.push(ManagedOwnerRoute {
                    cidr: fragment,
                    owner_ref: owner_ref.clone(),
                    endpoint_tag: endpoint_tag.clone(),
                }),
                ManagedPlanTarget::Unmanaged => released.push(fragment),
                ManagedPlanTarget::Reject => {}
            }
        }
    }
    budget.check_len(owners.len())?;
    budget.check_len(released.len())
}

/// Resolve one immutable input snapshot. Any missing evidence blocks claiming
/// an owner; rejected fragments remain in Q and never fall through to another
/// tailnet. Service addresses are excluded for D1's separate port-53 policy.
pub fn compile_managed_mesh_plan(
    input: ManagedMeshPlanInput,
) -> Result<ManagedMeshRoutePlan, String> {
    input.policy.validate()?;
    input.state.validate()?;
    if input.plan_id.is_empty() || input.config_version.is_empty() {
        return Err("managed plan identity is incomplete".into());
    }
    let mut budget = Budget::default();
    if input.candidates.len() > 256
        || input.policy.candidate_order.len() > 256
        || input.policy.assignments.len() > MAX_PLAN_CIDRS
        || input.policy.overrides.len() > MAX_PLAN_CIDRS
        || input.scopeable_rule_matchers.len() > MAX_PLAN_CIDRS
        || input.state.observations.len() > MAX_PLAN_CIDRS
        || input.state.reservations.len() > MAX_PLAN_CIDRS
    {
        return Err("managed mesh plan input budget exceeded".into());
    }
    let mut candidates = BTreeMap::new();
    let mut endpoint_tags = BTreeSet::new();
    for candidate in &input.candidates {
        if !candidate.evidence_complete {
            return Err("managed candidate evidence is incomplete".into());
        }
        if candidates
            .insert(candidate.owner_ref.clone(), candidate)
            .is_some()
        {
            return Err("duplicate managed candidate identity".into());
        }
        if let Some(tag) = &candidate.endpoint_tag {
            if tag.is_empty() || !endpoint_tags.insert(tag) {
                return Err("ambiguous managed endpoint tag".into());
            }
        }
        if !input
            .policy
            .candidate_order
            .contains(&candidate.owner_ref.server_id)
        {
            return Err("managed candidate missing from frozen order".into());
        }
    }

    let mut all_scopes = Vec::new();
    let mut claimed = Vec::new();
    let mut owner_routes = Vec::new();
    let mut released = Vec::new();
    let mut assignments: Vec<(String, &MeshTarget)> = Vec::new();
    let mut same_prefix = BTreeMap::<String, &MeshTarget>::new();
    for assignment in &input.policy.assignments {
        let pieces = add_scope(&mut all_scopes, &assignment.cidr, &mut budget)?;
        if pieces.is_empty() {
            return Err("MagicDNS service address requires the separate DNS policy".into());
        }
        for cidr in pieces {
            if let Some(previous) = same_prefix.insert(cidr.clone(), &assignment.target) {
                if previous != &assignment.target {
                    return Err("conflicting explicit mesh targets at the same CIDR".into());
                }
            }
            assignments.push((cidr, &assignment.target));
        }
    }
    assignments.sort_by(|a, b| prefix_len(&b.0).cmp(&prefix_len(&a.0)).then(a.0.cmp(&b.0)));
    for (cidr, target) in assignments {
        claim(
            vec![cidr],
            resolve_target(target, &input.state, &candidates),
            &mut claimed,
            &mut owner_routes,
            &mut released,
            &mut budget,
        )?;
    }

    // Historical reservations have precedence over fresh candidates. Retired
    // or missing owners still claim their old region, but only as reject.
    let mut reservations = BTreeMap::<String, &MeshReservationOwner>::new();
    for reservation in &input.state.reservations {
        for cidr in add_scope(&mut all_scopes, &reservation.cidr, &mut budget)? {
            if let Some(previous) = reservations.insert(cidr.clone(), &reservation.owner_ref) {
                if previous != &reservation.owner_ref {
                    return Err("conflicting mesh reservations at the same CIDR".into());
                }
            }
        }
    }
    let mut reservations: Vec<_> = reservations.into_iter().collect();
    reservations.sort_by(|a, b| prefix_len(&b.0).cmp(&prefix_len(&a.0)).then(a.0.cmp(&b.0)));
    for (cidr, reservation) in reservations {
        let target = match reservation {
            MeshReservationOwner::Owner {
                server_id,
                identity_epoch,
            } => resolved_owner(
                &MeshOwnerRef {
                    server_id: server_id.clone(),
                    identity_epoch: identity_epoch.clone(),
                },
                &input.state,
                &candidates,
            ),
            MeshReservationOwner::Deny => ManagedPlanTarget::Reject,
        };
        claim(
            vec![cidr],
            target,
            &mut claimed,
            &mut owner_routes,
            &mut released,
            &mut budget,
        )?;
    }

    // Observations from an identity absent from the current endpoint set must
    // be retained as reject; a different candidate cannot inherit them.
    for observation in &input.state.observations {
        let target = resolved_owner(&observation.owner_ref, &input.state, &candidates);
        if target != ManagedPlanTarget::Reject {
            continue;
        }
        for raw in observation
            .raw_hosts
            .iter()
            .chain(&observation.advertised_routes)
        {
            let pieces = add_scope(&mut all_scopes, raw, &mut budget)?;
            claim(
                pieces,
                ManagedPlanTarget::Reject,
                &mut claimed,
                &mut owner_routes,
                &mut released,
                &mut budget,
            )?;
        }
    }

    for server_id in &input.policy.candidate_order {
        let mut matching = input
            .candidates
            .iter()
            .filter(|candidate| &candidate.owner_ref.server_id == server_id);
        let Some(candidate) = matching.next() else {
            continue;
        };
        if matching.next().is_some() {
            return Err("multiple identity epochs for one managed candidate".into());
        }
        let target = resolved_owner(&candidate.owner_ref, &input.state, &candidates);
        for raw in candidate.configured_cidrs.iter().chain(
            input
                .state
                .observations
                .iter()
                .filter(|observation| observation.owner_ref == candidate.owner_ref)
                .flat_map(|observation| {
                    observation
                        .raw_hosts
                        .iter()
                        .chain(&observation.advertised_routes)
                }),
        ) {
            let normalized = normalize_cidr(raw)
                .ok_or_else(|| format!("invalid managed mesh candidate CIDR: {raw}"))?;
            if normalized == TAILNET_CGNAT || normalized == TAILNET_ULA_V6 {
                // The old generator's defaults are evidence of an unknown
                // pool, not evidence this candidate owns the whole pool.
                add_scope(&mut all_scopes, raw, &mut budget)?;
                continue;
            }
            if cidr_contains(&normalized, TAILNET_CGNAT)
                || cidr_contains(&normalized, TAILNET_ULA_V6)
            {
                return Err("implicit candidate route is broader than bootstrap pool".into());
            }
            let pieces = add_scope(&mut all_scopes, raw, &mut budget)?;
            claim(
                pieces,
                target.clone(),
                &mut claimed,
                &mut owner_routes,
                &mut released,
                &mut budget,
            )?;
        }
    }

    // The default upstream pools are unknown bootstrap scope, not permission
    // for the first Tailscale node to own an entire /10 or /48.
    for cidr in [TAILNET_CGNAT, TAILNET_ULA_V6] {
        add_scope(&mut all_scopes, cidr, &mut budget)?;
    }
    let protected_cidrs = subtract_bounded(all_scopes, &released, &mut budget)?;
    let mut unassigned_cidrs = protected_cidrs.clone();
    for route in &owner_routes {
        unassigned_cidrs = subtract_bounded(
            unassigned_cidrs,
            std::slice::from_ref(&route.cidr),
            &mut budget,
        )?;
    }
    let mut overrides = Vec::new();
    for override_rule in &input.policy.overrides {
        let matcher = input
            .scopeable_rule_matchers
            .get(&override_rule.rule_id)
            .ok_or_else(|| {
                format!(
                    "managed override rule is not scopeable: {}",
                    override_rule.rule_id
                )
            })?;
        validate_pure_matcher(matcher)?;
        let mut scope = Vec::new();
        for raw in &override_rule.scope_cidrs {
            for normalized in normalized_without_service(raw, &mut budget)? {
                for protected in &protected_cidrs {
                    if let Some(intersection) = scope_intersection(&normalized, protected) {
                        add_disjoint(&mut scope, std::iter::once(intersection), &mut budget)?;
                    }
                }
            }
        }
        overrides.push(ManagedScopedOverride {
            rule_id: override_rule.rule_id.clone(),
            scope_cidrs: scope,
            matcher: matcher.clone(),
            target: resolve_target(&override_rule.target, &input.state, &candidates),
        });
    }
    // Bind every referenced epoch, including an epoch currently compiled to
    // reject. Otherwise a pending plan could silently lose the old identity
    // from its receipt when its endpoint disappears.
    let mut identity_bindings: BTreeSet<MeshOwnerRef> = input
        .candidates
        .iter()
        .map(|candidate| candidate.owner_ref.clone())
        .chain(
            input
                .state
                .observations
                .iter()
                .map(|observation| observation.owner_ref.clone()),
        )
        .collect();
    for reservation in &input.state.reservations {
        if let MeshReservationOwner::Owner {
            server_id,
            identity_epoch,
        } = &reservation.owner_ref
        {
            identity_bindings.insert(MeshOwnerRef {
                server_id: server_id.clone(),
                identity_epoch: identity_epoch.clone(),
            });
        }
    }
    for target in input
        .policy
        .assignments
        .iter()
        .map(|assignment| &assignment.target)
        .chain(
            input
                .policy
                .overrides
                .iter()
                .map(|override_rule| &override_rule.target),
        )
    {
        if let MeshTarget::Owner {
            server_id,
            identity_epoch,
        } = target
        {
            identity_bindings.insert(MeshOwnerRef {
                server_id: server_id.clone(),
                identity_epoch: identity_epoch.clone(),
            });
        }
    }
    if let Some(dns_policy) = &input.policy.dns_policy {
        for target in dns_policy
            .suffix_assignments
            .iter()
            .map(|assignment| &assignment.target)
            .chain(std::iter::once(&dns_policy.service_owner))
        {
            if let MeshDnsOwnerTarget::Owner {
                server_id,
                identity_epoch,
            } = target
            {
                identity_bindings.insert(MeshOwnerRef {
                    server_id: server_id.clone(),
                    identity_epoch: identity_epoch.clone(),
                });
            }
        }
        if let MeshDnsShortNamePolicy::Owner {
            server_id,
            identity_epoch,
        } = &dns_policy.short_name_policy
        {
            identity_bindings.insert(MeshOwnerRef {
                server_id: server_id.clone(),
                identity_epoch: identity_epoch.clone(),
            });
        }
    }
    owner_routes.sort_by(|a, b| a.cidr.cmp(&b.cidr).then(a.owner_ref.cmp(&b.owner_ref)));
    unassigned_cidrs.sort();
    released.sort();
    Ok(ManagedMeshRoutePlan {
        schema_version: 1,
        plan_id: input.plan_id,
        config_version: input.config_version,
        input_state_revision: input.state.revision,
        identity_bindings: identity_bindings.into_iter().collect(),
        reject_cidrs: protected_cidrs.clone(),
        protected_cidrs,
        owner_routes,
        unassigned_cidrs,
        released_cidrs: released,
        overrides,
        dns_managed: input.policy.dns_policy.is_some(),
    })
}

#[cfg(test)]
mod tests;
