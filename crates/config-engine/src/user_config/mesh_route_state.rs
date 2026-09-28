//! Shared S3 wire contract. These values are intent and local evidence, never credentials.
//! Managed documents are parsed strictly before the legacy sanitizer can discard fields.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

pub const MESH_ROUTE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshOwnerRef {
    pub server_id: String,
    pub identity_epoch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MeshTarget {
    Owner {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "identityEpoch")]
        identity_epoch: String,
    },
    Reject,
    Unmanaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshAssignment {
    pub cidr: String,
    pub target: MeshTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshOverride {
    pub rule_id: String,
    pub scope_cidrs: Vec<String>,
    pub target: MeshTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshMigration {
    /// First release never silently allocates the unknown public Tailscale pool.
    pub unknown_public_pool: UnknownPoolDisposition,
    pub builtin_exceptions_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UnknownPoolDisposition {
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshRoutePolicy {
    pub schema_version: u32,
    pub candidate_order: Vec<String>,
    pub assignments: Vec<MeshAssignment>,
    pub overrides: Vec<MeshOverride>,
    pub migration: MeshMigration,
    /// Absent preserves legacy DNS behavior. Explicit null is invalid.
    #[serde(
        default,
        deserialize_with = "strict_optional_dns",
        skip_serializing_if = "Option::is_none"
    )]
    pub dns_policy: Option<MeshDnsPolicy>,
}

fn strict_optional_dns<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<MeshDnsPolicy>, D::Error> {
    let raw = Value::deserialize(deserializer)?;
    if raw.is_null() {
        return Err(serde::de::Error::custom(
            "dnsPolicy:null is not legacy absence",
        ));
    }
    serde_json::from_value(raw)
        .map(Some)
        .map_err(serde::de::Error::custom)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MeshDnsOwnerTarget {
    Owner {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "identityEpoch")]
        identity_epoch: String,
    },
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MeshDnsShortNamePolicy {
    System,
    Owner {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "identityEpoch")]
        identity_epoch: String,
    },
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshDnsSuffixAssignment {
    pub suffix: String,
    pub target: MeshDnsOwnerTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshDnsPolicy {
    pub schema_version: u32,
    pub suffix_assignments: Vec<MeshDnsSuffixAssignment>,
    pub short_name_policy: MeshDnsShortNamePolicy,
    pub service_owner: MeshDnsOwnerTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshBindingState {
    Bound,
    Unbound,
    Retiring,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshIdentity {
    pub server_id: String,
    pub identity_epoch: String,
    pub control_authority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub self_stable_id: Option<String>,
    pub binding_state: MeshBindingState,
    pub evidence_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshObservation {
    pub owner_ref: MeshOwnerRef,
    pub raw_hosts: Vec<String>,
    pub advertised_routes: Vec<String>,
    /// Monotonic evidence per identity epoch; empty/absent STATUS never removes history.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub magic_dns_suffixes: Vec<String>,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_valid_evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MeshReservationOwner {
    Owner {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "identityEpoch")]
        identity_epoch: String,
    },
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshReservation {
    pub cidr: String,
    pub owner_ref: MeshReservationOwner,
    pub origin: String,
}

fn decimal_revision<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = String::deserialize(deserializer)?;
    if value.is_empty()
        || value.len() > 20
        || value.starts_with('0') && value != "0"
        || !value.bytes().all(|byte| byte.is_ascii_digit())
        || value.parse::<u64>().is_err()
    {
        return Err(serde::de::Error::custom(
            "revision must be a canonical decimal u64 string",
        ));
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshDesiredRun {
    Running,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshIntent {
    #[serde(deserialize_with = "decimal_revision")]
    pub revision: String,
    pub desired_run: MeshDesiredRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshActivePlan {
    pub plan_id: String,
    pub digest: String,
    pub config_version: String,
    #[serde(deserialize_with = "decimal_revision")]
    pub input_state_revision: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshTransactionPhase {
    Prepared,
    StopRequested,
    OldStopped,
    StartRequested,
    CoreReady,
    Committed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshTransaction {
    pub plan_id: String,
    pub plan_digest: String,
    pub input_config_version: String,
    #[serde(deserialize_with = "decimal_revision")]
    pub input_state_revision: String,
    pub identity_bindings: Vec<MeshOwnerRef>,
    #[serde(deserialize_with = "decimal_revision")]
    pub intent_revision: String,
    pub boot_id: String,
    pub lifecycle_generation: String,
    pub phase: MeshTransactionPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous_plan_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_manifest_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MeshIdentityEffectStatus {
    Pending,
    Done,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshIdentityEffect {
    pub effect_id: String,
    pub server_id: String,
    pub retired_epoch: String,
    pub kind: String,
    pub status: MeshIdentityEffectStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshRouteState {
    pub schema_version: u32,
    pub local_id: String,
    #[serde(deserialize_with = "decimal_revision")]
    pub revision: String,
    pub identities: Vec<MeshIdentity>,
    pub observations: Vec<MeshObservation>,
    pub reservations: Vec<MeshReservation>,
    pub intent: MeshIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_plan: Option<MeshActivePlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<MeshTransaction>,
    pub identity_effects: Vec<MeshIdentityEffect>,
}

fn unique_nonempty<'a>(items: impl Iterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for item in items {
        if item.trim().is_empty() || !seen.insert(item) {
            return Err("empty or duplicate mesh identifier".into());
        }
    }
    Ok(())
}

impl MeshRoutePolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != MESH_ROUTE_SCHEMA_VERSION {
            return Err("unsupported meshRoutePolicy schemaVersion".into());
        }
        unique_nonempty(self.candidate_order.iter().map(String::as_str))?;
        unique_nonempty(self.overrides.iter().map(|rule| rule.rule_id.as_str()))?;
        for target in self
            .assignments
            .iter()
            .map(|assignment| &assignment.target)
            .chain(self.overrides.iter().map(|rule| &rule.target))
        {
            if let MeshTarget::Owner {
                server_id,
                identity_epoch,
            } = target
            {
                if server_id.is_empty() || identity_epoch.is_empty() {
                    return Err("mesh owner reference is empty".into());
                }
            }
        }
        if let Some(dns) = &self.dns_policy {
            if dns.schema_version != 1 {
                return Err("unsupported dnsPolicy schemaVersion".into());
            }
            unique_nonempty(
                dns.suffix_assignments
                    .iter()
                    .map(|assignment| assignment.suffix.as_str()),
            )?;
            for target in dns
                .suffix_assignments
                .iter()
                .map(|assignment| &assignment.target)
                .chain(std::iter::once(&dns.service_owner))
            {
                if let MeshDnsOwnerTarget::Owner {
                    server_id,
                    identity_epoch,
                } = target
                {
                    if server_id.is_empty() || identity_epoch.is_empty() {
                        return Err("DNS owner reference is empty".into());
                    }
                }
            }
            if let MeshDnsShortNamePolicy::Owner {
                server_id,
                identity_epoch,
            } = &dns.short_name_policy
            {
                if server_id.is_empty() || identity_epoch.is_empty() {
                    return Err("short-name owner reference is empty".into());
                }
            }
        }
        Ok(())
    }
}

impl MeshRouteState {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != MESH_ROUTE_SCHEMA_VERSION || self.local_id.trim().is_empty() {
            return Err("unsupported or incomplete meshRouteState".into());
        }
        let mut identity_keys = BTreeSet::new();
        for identity in &self.identities {
            if identity.server_id.is_empty()
                || identity.identity_epoch.is_empty()
                || !identity_keys.insert((&identity.server_id, &identity.identity_epoch))
            {
                return Err("empty or duplicate mesh identity epoch".into());
            }
        }
        unique_nonempty(
            self.identity_effects
                .iter()
                .map(|effect| effect.effect_id.as_str()),
        )?;
        for identity in &self.identities {
            if identity.identity_epoch.is_empty()
                || identity.control_authority.is_empty()
                || identity.evidence_source.is_empty()
            {
                return Err("incomplete mesh identity evidence".into());
            }
        }
        Ok(())
    }
}
