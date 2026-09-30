//! Historical route-scope extraction from a digest-bound active plan. The
//! union is deliberately an upper bound: an override may have a narrower
//! matcher, but its whole declared CIDR scope remains reserved on retirement.
//! Empty scope never proves that the owner process or TS state has exited.

use super::artifact::VerifiedActivePlan;
use polaris_config_engine::builder::managed_mesh_plan::ManagedPlanTarget;
use polaris_config_engine::user_config::cidr::normalize_cidr;
use polaris_config_engine::user_config::mesh_route_state::{MeshActivePlan, MeshOwnerRef};
use std::collections::BTreeSet;

const MAX_OWNER_SCOPE_CIDRS: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OwnerScopeError {
    InvalidCidr,
    TooManyCidrs,
}

/// Only `verified_owner_scope` may construct this. It carries historical
/// route scope, not a no-owner receipt or permission to delete state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedOwnerScope {
    active: MeshActivePlan,
    owner_ref: MeshOwnerRef,
    cidrs: Vec<String>,
}

impl VerifiedOwnerScope {
    pub(crate) fn active(&self) -> &MeshActivePlan {
        &self.active
    }

    pub(crate) fn owner_ref(&self) -> &MeshOwnerRef {
        &self.owner_ref
    }

    pub(crate) fn cidrs(&self) -> &[String] {
        &self.cidrs
    }
}

pub(crate) fn verified_owner_scope(
    verified: &VerifiedActivePlan,
    owner_ref: &MeshOwnerRef,
) -> Result<VerifiedOwnerScope, OwnerScopeError> {
    let routes = verified
        .plan()
        .owner_routes
        .iter()
        .filter(|route| route.owner_ref == *owner_ref)
        .map(|route| route.cidr.as_str());
    let overrides = verified
        .plan()
        .overrides
        .iter()
        .filter(|entry| {
            matches!(&entry.target, ManagedPlanTarget::Owner { owner_ref: target, .. } if target == owner_ref)
        })
        .flat_map(|entry| entry.scope_cidrs.iter().map(String::as_str));
    let mut cidrs = BTreeSet::new();
    let mut count = 0usize;
    for cidr in routes.chain(overrides) {
        count = count.checked_add(1).ok_or(OwnerScopeError::TooManyCidrs)?;
        if count > MAX_OWNER_SCOPE_CIDRS {
            return Err(OwnerScopeError::TooManyCidrs);
        }
        let canonical = normalize_cidr(cidr).ok_or(OwnerScopeError::InvalidCidr)?;
        if canonical.ends_with("/0") {
            return Err(OwnerScopeError::InvalidCidr);
        }
        cidrs.insert(canonical);
    }
    Ok(VerifiedOwnerScope {
        active: verified.active().clone(),
        owner_ref: owner_ref.clone(),
        cidrs: cidrs.into_iter().collect(),
    })
}
