//! Shared S3 wire contract. These values are intent and local evidence, never credentials.
//! Managed documents are parsed strictly before the legacy sanitizer can discard fields.

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub const MESH_ROUTE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeshOwnerRef {
    pub server_id: String,
    pub identity_epoch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
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

fn tagged_fields<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<(String, Map<String, Value>), D::Error> {
    let Value::Object(mut fields) = Value::deserialize(deserializer)? else {
        return Err(serde::de::Error::custom("mesh target must be an object"));
    };
    let Some(Value::String(kind)) = fields.remove("kind") else {
        return Err(serde::de::Error::custom("mesh target kind is missing"));
    };
    Ok((kind, fields))
}

fn unit_fields<E: serde::de::Error>(fields: Map<String, Value>) -> Result<(), E> {
    if fields.is_empty() {
        Ok(())
    } else {
        Err(E::custom("unit mesh target has unexpected fields"))
    }
}

fn owner_fields<E: serde::de::Error>(
    mut fields: Map<String, Value>,
) -> Result<(String, String), E> {
    if fields.len() != 2 {
        return Err(E::custom(
            "mesh owner target fields are incomplete or unknown",
        ));
    }
    let Some(Value::String(server_id)) = fields.remove("serverId") else {
        return Err(E::custom("mesh owner serverId is missing"));
    };
    let Some(Value::String(identity_epoch)) = fields.remove("identityEpoch") else {
        return Err(E::custom("mesh owner identityEpoch is missing"));
    };
    Ok((server_id, identity_epoch))
}

impl<'de> Deserialize<'de> for MeshTarget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (kind, fields) = tagged_fields(deserializer)?;
        match kind.as_str() {
            "owner" => {
                let (server_id, identity_epoch) = owner_fields(fields)?;
                Ok(Self::Owner {
                    server_id,
                    identity_epoch,
                })
            }
            "reject" => {
                unit_fields(fields)?;
                Ok(Self::Reject)
            }
            "unmanaged" => {
                unit_fields(fields)?;
                Ok(Self::Unmanaged)
            }
            _ => Err(serde::de::Error::custom("unknown mesh target kind")),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeshDnsOwnerTarget {
    Owner {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "identityEpoch")]
        identity_epoch: String,
    },
    Reject,
}

impl<'de> Deserialize<'de> for MeshDnsOwnerTarget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (kind, fields) = tagged_fields(deserializer)?;
        match kind.as_str() {
            "owner" => {
                let (server_id, identity_epoch) = owner_fields(fields)?;
                Ok(Self::Owner {
                    server_id,
                    identity_epoch,
                })
            }
            "reject" => {
                unit_fields(fields)?;
                Ok(Self::Reject)
            }
            _ => Err(serde::de::Error::custom("unknown DNS owner target kind")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
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

impl<'de> Deserialize<'de> for MeshDnsShortNamePolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (kind, fields) = tagged_fields(deserializer)?;
        match kind.as_str() {
            "owner" => {
                let (server_id, identity_epoch) = owner_fields(fields)?;
                Ok(Self::Owner {
                    server_id,
                    identity_epoch,
                })
            }
            "system" => {
                unit_fields(fields)?;
                Ok(Self::System)
            }
            "reject" => {
                unit_fields(fields)?;
                Ok(Self::Reject)
            }
            _ => Err(serde::de::Error::custom("unknown DNS short-name kind")),
        }
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MeshReservationOwner {
    Owner {
        #[serde(rename = "serverId")]
        server_id: String,
        #[serde(rename = "identityEpoch")]
        identity_epoch: String,
    },
    Deny,
}

impl<'de> Deserialize<'de> for MeshReservationOwner {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (kind, fields) = tagged_fields(deserializer)?;
        match kind.as_str() {
            "owner" => {
                let (server_id, identity_epoch) = owner_fields(fields)?;
                Ok(Self::Owner {
                    server_id,
                    identity_epoch,
                })
            }
            "deny" => {
                unit_fields(fields)?;
                Ok(Self::Deny)
            }
            _ => Err(serde::de::Error::custom("unknown reservation owner kind")),
        }
    }
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

fn canonical_cidr(raw: &str) -> Result<String, String> {
    let (address, prefix) = raw
        .split_once('/')
        .map_or((raw, None), |(address, prefix)| (address, Some(prefix)));
    let ip: IpAddr = address.parse().map_err(|_| "invalid mesh IP address")?;
    match ip {
        IpAddr::V4(ip) => {
            let prefix = prefix
                .map_or(Ok(32), str::parse::<u8>)
                .map_err(|_| "invalid IPv4 prefix")?;
            if prefix > 32 {
                return Err("invalid IPv4 prefix".into());
            }
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            Ok(format!(
                "{}/{}",
                Ipv4Addr::from(u32::from(ip) & mask),
                prefix
            ))
        }
        IpAddr::V6(ip) => {
            let prefix = prefix
                .map_or(Ok(128), str::parse::<u8>)
                .map_err(|_| "invalid IPv6 prefix")?;
            if prefix > 128 {
                return Err("invalid IPv6 prefix".into());
            }
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            Ok(format!(
                "{}/{}",
                Ipv6Addr::from(u128::from(ip) & mask),
                prefix
            ))
        }
    }
}

fn canonical_cidr_set(values: &mut Vec<String>) -> Result<(), String> {
    let normalized: BTreeSet<String> = values
        .iter()
        .map(|value| canonical_cidr(value))
        .collect::<Result<_, _>>()?;
    *values = normalized.into_iter().collect();
    Ok(())
}

fn canonical_host(raw: &str) -> Result<String, String> {
    let address = raw.split_once('/').map_or(raw, |(address, _)| address);
    let ip: IpAddr = address.parse().map_err(|_| "invalid mesh host address")?;
    let canonical = canonical_cidr(raw)?;
    let host_prefix = match ip {
        IpAddr::V4(_) => "/32",
        IpAddr::V6(_) => "/128",
    };
    if !canonical.ends_with(host_prefix) {
        return Err("mesh raw host must be a single IP".into());
    }
    Ok(canonical)
}

fn canonical_host_set(values: &mut Vec<String>) -> Result<(), String> {
    let normalized: BTreeSet<String> = values
        .iter()
        .map(|value| canonical_host(value))
        .collect::<Result<_, _>>()?;
    *values = normalized.into_iter().collect();
    Ok(())
}

/// One compare-and-swap step for semantic ledger changes. Sampling timestamps are
/// excluded from the comparison; reordered/equivalent IP evidence cannot invalidate
/// a preview. A no-op does not write or advance the independent state revision.
pub fn revise_semantic(
    previous: &MeshRouteState,
    expected_revision: &str,
    mut next: MeshRouteState,
) -> Result<Option<MeshRouteState>, String> {
    previous.validate()?;
    next.validate()?;
    if previous.revision != expected_revision || next.revision != previous.revision {
        return Err("mesh state revision conflict".into());
    }
    // Historical MagicDNS scope survives missing/empty frames and retirement.
    // The resolver may only gain this evidence; a later release is a separate
    // explicit policy transaction, never an observation overwrite.
    for prior in &previous.observations {
        if let Some(current) = next.observations.iter_mut().find(|observation| {
            observation.owner_ref == prior.owner_ref && observation.source == prior.source
        }) {
            current
                .magic_dns_suffixes
                .extend(prior.magic_dns_suffixes.iter().cloned());
            if current.raw_hosts.is_empty() && current.advertised_routes.is_empty() {
                current.raw_hosts = prior.raw_hosts.clone();
                current.advertised_routes = prior.advertised_routes.clone();
            }
        } else {
            next.observations.push(prior.clone());
        }
    }
    let mut old = previous.clone();
    old.normalize_semantic()?;
    next.normalize_semantic()?;
    let mut new_compare = next.clone();
    old.revision.clear();
    new_compare.revision.clear();
    for observation in &mut old.observations {
        observation.last_valid_evidence = None;
    }
    for observation in &mut new_compare.observations {
        observation.last_valid_evidence = None;
    }
    if old == new_compare {
        return Ok(None);
    }
    let revision = previous
        .revision
        .parse::<u64>()
        .map_err(|_| "invalid mesh state revision")?
        .checked_add(1)
        .ok_or("mesh state revision exhausted")?;
    next.revision = revision.to_string();
    Ok(Some(next))
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
        if self.migration.builtin_exceptions_version != 1 {
            return Err("unsupported builtinExceptionsVersion".into());
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
    fn normalize_semantic(&mut self) -> Result<(), String> {
        for observation in &mut self.observations {
            canonical_host_set(&mut observation.raw_hosts)?;
            canonical_cidr_set(&mut observation.advertised_routes)?;
            let suffixes: BTreeSet<String> = observation
                .magic_dns_suffixes
                .iter()
                .map(|suffix| {
                    if suffix.trim() != suffix {
                        return Err("invalid MagicDNS suffix".into());
                    }
                    let normalized = suffix.trim_end_matches('.').to_ascii_lowercase();
                    if normalized.is_empty() {
                        return Err("invalid MagicDNS suffix".into());
                    }
                    Ok(normalized)
                })
                .collect::<Result<_, String>>()?;
            observation.magic_dns_suffixes = suffixes.into_iter().collect();
        }
        self.identities.sort_by(|a, b| {
            (&a.server_id, &a.identity_epoch).cmp(&(&b.server_id, &b.identity_epoch))
        });
        self.observations.sort_by(|a, b| {
            (
                &a.owner_ref.server_id,
                &a.owner_ref.identity_epoch,
                &a.source,
                &a.raw_hosts,
                &a.advertised_routes,
                &a.magic_dns_suffixes,
            )
                .cmp(&(
                    &b.owner_ref.server_id,
                    &b.owner_ref.identity_epoch,
                    &b.source,
                    &b.raw_hosts,
                    &b.advertised_routes,
                    &b.magic_dns_suffixes,
                ))
        });
        for reservation in &mut self.reservations {
            reservation.cidr = canonical_cidr(&reservation.cidr)?;
        }
        self.reservations.sort_by(|a, b| {
            (&a.cidr, &a.origin, format!("{:?}", a.owner_ref)).cmp(&(
                &b.cidr,
                &b.origin,
                format!("{:?}", b.owner_ref),
            ))
        });
        self.identity_effects
            .sort_by(|a, b| a.effect_id.cmp(&b.effect_id));
        if let Some(transaction) = &mut self.transaction {
            transaction.identity_bindings.sort_by(|a, b| {
                (&a.server_id, &a.identity_epoch).cmp(&(&b.server_id, &b.identity_epoch))
            });
        }
        Ok(())
    }

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
        for observation in &self.observations {
            if observation.owner_ref.server_id.is_empty()
                || observation.owner_ref.identity_epoch.is_empty()
            {
                return Err("incomplete mesh observation owner".into());
            }
            for raw_host in &observation.raw_hosts {
                canonical_host(raw_host)?;
            }
            for route in &observation.advertised_routes {
                canonical_cidr(route)?;
            }
            unique_nonempty(observation.magic_dns_suffixes.iter().map(String::as_str))?;
        }
        for reservation in &self.reservations {
            canonical_cidr(&reservation.cidr)?;
            if let MeshReservationOwner::Owner {
                server_id,
                identity_epoch,
            } = &reservation.owner_ref
            {
                if server_id.is_empty() || identity_epoch.is_empty() {
                    return Err("incomplete mesh reservation owner".into());
                }
            }
        }
        Ok(())
    }
}
