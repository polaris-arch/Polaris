//! Pure validation of injected Windows observations, separate from legacy netinfo.
//!
//! A safe provider seam supplies independent reads; no ObjectFacts assembly,
//! classifier, joins or selected
//! egress are supplied. Metadata references are not proof of association or persistent
//! identity. Completeness and compartment are explicit per source; legacy Ok(Vec)
//! results with skipped rows cannot become complete snapshots here. Consumers must
//! keep unproved association, route roles, identity and applicability Unknown.

use super::{MAX_INTERFACE_ADDRESSES, MAX_INTERFACE_ROWS, MAX_ROUTE_ROWS, MAX_SOURCE_BYTES};
use polaris_config_engine::builder::coexistence::{AddressFamily, Fact};
use polaris_config_engine::user_config::cidr::normalize_cidr;
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

/// Source-local observations; Known rows alone do not prove a complete enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadRows<T> {
    pub rows: Vec<T>,
    /// Full read with attribution preserved, only within the recorded query scope.
    pub complete: Fact<bool>,
    /// Read namespace, never defaulted to the current or default compartment.
    pub compartment: Fact<u32>,
    /// Source-local read error; no credentials or raw RAS entry names.
    pub error: Fact<Option<String>>,
}
impl<T> ReadRows<T> {
    /// Authoritative emptiness of THIS source in its scope, not absence of VPNs or
    /// objects in other sources/compartments. Partial/unknown empty is not false data.
    pub fn proven_empty(&self) -> Fact<bool> {
        if !self.rows.is_empty() {
            return Fact::Known(false);
        }
        match (&self.complete, &self.compartment, &self.error) {
            (Fact::Known(true), Fact::Known(id), Fact::Known(None)) if *id != 0 => {
                Fact::Known(true)
            }
            _ => Fact::Unknown("source emptiness lacks complete scoped evidence".into()),
        }
    }
}

/// Snapshot reference only. LUID/index/alias never become persistent repair identity.
/// if_index comparisons require a supplied family; adapters/RAS have no such family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsInterfaceRef {
    pub alias: Fact<String>,
    pub luid: Fact<u64>,
    pub if_index: Fact<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsAdapterObservation {
    pub interface: WindowsInterfaceRef,
    pub if_type: Fact<u32>,
    /// Diagnostics only; never a tunnel/virtualization/identity classifier.
    pub description: Fact<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsAddressObservation {
    pub interface: WindowsInterfaceRef,
    pub family: AddressFamily,
    /// Actual host address is not normalized to its network prefix.
    pub address: IpAddr,
    pub prefix_len: Fact<u8>,
    /// IPv6 zone/scope id, separate from the 128 address bits.
    pub scope_id: Fact<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsRouteObservation {
    pub interface: WindowsInterfaceRef,
    pub family: AddressFamily,
    pub prefix: String,
    pub next_hop: Fact<IpAddr>,
    /// Scope of the next-hop address, never a route/compartment scope proof.
    pub next_hop_scope_id: Fact<u32>,
    pub route_metric: Fact<u32>,
    pub interface_metric: Fact<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsRasObservation {
    pub name: Fact<String>,
    pub all_users: bool,
    /// A reported association needs a snapshot key; name equality is not evidence.
    pub interface: Fact<WindowsInterfaceRef>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsFactInput {
    pub adapters: Fact<ReadRows<WindowsAdapterObservation>>,
    pub addresses: Fact<ReadRows<WindowsAddressObservation>>,
    pub routes4: Fact<ReadRows<WindowsRouteObservation>>,
    pub routes6: Fact<ReadRows<WindowsRouteObservation>>,
    pub ras: Fact<ReadRows<WindowsRasObservation>>,
}

/// Fixed source seam; OS reads belong to a blocking worker, never the IPC waiter.
pub trait WindowsFactSource: Send + Sync {
    fn adapters_and_addresses(
        &self,
    ) -> (
        Fact<ReadRows<WindowsAdapterObservation>>,
        Fact<ReadRows<WindowsAddressObservation>>,
    );
    fn routes(&self, family: AddressFamily) -> Fact<ReadRows<WindowsRouteObservation>>;
    fn ras(&self) -> Fact<ReadRows<WindowsRasObservation>>;
}

/// Four sequential reads, five independent sources. No joins or classification.
pub fn collect_windows(source: &impl WindowsFactSource) -> WindowsFactInput {
    let (adapters, addresses) = source.adapters_and_addresses();
    let routes4 = source.routes(AddressFamily::V4);
    let routes6 = source.routes(AddressFamily::V6);
    let ras = source.ras();
    validate_windows_input(WindowsFactInput {
        adapters,
        addresses,
        routes4,
        routes6,
        ras,
    })
}

/// RAS may report a dot followed by a telephone number. Retain the row but
/// discard that string before it can enter a DTO, debug output or error message.
pub fn private_ras_name(raw: &str) -> Fact<String> {
    if raw.trim_start().starts_with('.') {
        Fact::Unknown("RAS entry name withheld for privacy".into())
    } else {
        Fact::Known(raw.to_string())
    }
}

/// Validate each source independently. Limits apply AFTER provider allocation and
/// count all UTF-8 observation/diagnostic strings per source, not serialized bytes.
/// Overflow/malformed/conflicting evidence makes the affected source Unknown, never
/// truncates it or erases independent sources. Missing metadata stays Unknown.
/// Unknown scope/key cannot preserve a supplied positive completeness assertion.
pub fn validate_windows_input(mut input: WindowsFactInput) -> WindowsFactInput {
    input.adapters = validate_source(input.adapters, None, "adapters");
    input.addresses = validate_source(input.addresses, None, "addresses");
    input.routes4 = validate_source(input.routes4, Some(AddressFamily::V4), "IPv4 routes");
    input.routes6 = validate_source(input.routes6, Some(AddressFamily::V6), "IPv6 routes");
    input.ras = validate_source(input.ras, None, "RAS");
    // Only reject positive contradictions against observed adapter LUID metadata.
    // Missing members, indices without a family, or another/unknown compartment
    // NEVER prove membership, absence or a join. No metadata is copied to a row.
    check_adapter_conflicts(&mut input.addresses, &input.adapters, "addresses");
    check_adapter_conflicts(&mut input.routes4, &input.adapters, "IPv4 routes");
    check_adapter_conflicts(&mut input.routes6, &input.adapters, "IPv6 routes");
    check_adapter_conflicts(&mut input.ras, &input.adapters, "RAS");
    input
}

fn fact_bytes<T>(fact: &Fact<T>, known: impl FnOnce(&T) -> usize) -> usize {
    match fact {
        Fact::Known(value) => known(value),
        Fact::Unknown(reason) => reason.len(),
    }
}
fn ref_bytes(r: &WindowsInterfaceRef) -> usize {
    fact_bytes(&r.alias, String::len)
        .saturating_add(fact_bytes(&r.luid, |_| 0))
        .saturating_add(fact_bytes(&r.if_index, |_| 0))
}
fn unprove(complete: &mut Fact<bool>, reason: &str) {
    if matches!(complete, Fact::Known(true)) {
        *complete = Fact::Unknown(reason.into());
    }
}
fn valid_family(family: AddressFamily, address: IpAddr) -> bool {
    matches!(
        (family, address),
        (AddressFamily::V4, IpAddr::V4(_)) | (AddressFamily::V6, IpAddr::V6(_))
    )
}
fn clean_ref(r: &mut WindowsInterfaceRef) {
    if matches!(&r.alias, Fact::Known(alias) if alias.trim().is_empty() || alias.chars().any(char::is_control))
    {
        r.alias = Fact::Unknown("invalid interface alias".into());
    }
    if r.luid == Fact::Known(0) {
        r.luid = Fact::Unknown("zero LUID is not a snapshot key".into());
    }
    if r.if_index == Fact::Known(0) {
        r.if_index = Fact::Unknown("zero index is not a snapshot key".into());
    }
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Luid(u64),
    Index(u8, u32),
}
fn key(r: &WindowsInterfaceRef, family: Option<AddressFamily>) -> Option<Key> {
    if let Fact::Known(luid) = r.luid {
        return (luid != 0).then_some(Key::Luid(luid));
    }
    if let (Some(family), Fact::Known(index)) = (family, &r.if_index) {
        return (*index != 0).then_some(Key::Index(family_tag(family), *index));
    }
    None
}
fn family_tag(family: AddressFamily) -> u8 {
    if family == AddressFamily::V4 {
        4
    } else {
        6
    }
}
fn consistent<K: Ord, V: Eq>(map: &mut BTreeMap<K, V>, key: K, value: V) -> Result<(), String> {
    if map.get(&key).is_some_and(|old| old != &value) {
        return Err("conflicting interface key/alias metadata".into());
    }
    map.insert(key, value);
    Ok(())
}
#[derive(Default)]
struct References {
    aliases: BTreeMap<u64, String>,
    luids: BTreeMap<String, u64>,
    indices: BTreeMap<(u8, u64), u32>,
    index_luids: BTreeMap<(u8, u32), u64>,
    index_aliases: BTreeMap<(u8, u32), String>,
    alias_indices: BTreeMap<(u8, String), u32>,
}
impl References {
    fn observe(
        &mut self,
        r: &WindowsInterfaceRef,
        family: Option<AddressFamily>,
    ) -> Result<(), String> {
        if let (Fact::Known(luid), Fact::Known(alias)) = (&r.luid, &r.alias) {
            consistent(&mut self.aliases, *luid, alias.clone())?;
            consistent(&mut self.luids, alias.clone(), *luid)?;
        }
        if let (Some(family), Fact::Known(index)) = (family, &r.if_index) {
            let family = family_tag(family);
            if let Fact::Known(luid) = r.luid {
                consistent(&mut self.indices, (family, luid), *index)?;
                consistent(&mut self.index_luids, (family, *index), luid)?;
            }
            if let Fact::Known(alias) = &r.alias {
                consistent(&mut self.index_aliases, (family, *index), alias.clone())?;
                consistent(&mut self.alias_indices, (family, alias.clone()), *index)?;
            }
        }
        Ok(())
    }
}
trait Observation {
    const LIMIT: usize;
    const UNIQUE: bool = false;
    const ADDRESS_LIMIT: bool = false;
    fn interface(&self) -> Option<&WindowsInterfaceRef>;
    fn family(&self) -> Option<AddressFamily> {
        None
    }
    fn bytes(&self) -> usize;
    fn normalize(&mut self) -> Result<(), String>;
}
impl Observation for WindowsAdapterObservation {
    const LIMIT: usize = MAX_INTERFACE_ROWS;
    const UNIQUE: bool = true;
    fn interface(&self) -> Option<&WindowsInterfaceRef> {
        Some(&self.interface)
    }
    fn bytes(&self) -> usize {
        ref_bytes(&self.interface)
            .saturating_add(fact_bytes(&self.if_type, |_| 0))
            .saturating_add(fact_bytes(&self.description, String::len))
    }
    fn normalize(&mut self) -> Result<(), String> {
        clean_ref(&mut self.interface);
        Ok(())
    }
}
impl Observation for WindowsAddressObservation {
    const LIMIT: usize = MAX_INTERFACE_ROWS * MAX_INTERFACE_ADDRESSES;
    const ADDRESS_LIMIT: bool = true;
    fn interface(&self) -> Option<&WindowsInterfaceRef> {
        Some(&self.interface)
    }
    fn family(&self) -> Option<AddressFamily> {
        Some(self.family)
    }
    fn bytes(&self) -> usize {
        ref_bytes(&self.interface)
            .saturating_add(fact_bytes(&self.prefix_len, |_| 0))
            .saturating_add(fact_bytes(&self.scope_id, |_| 0))
    }
    fn normalize(&mut self) -> Result<(), String> {
        clean_ref(&mut self.interface);
        if self.family == AddressFamily::V4 {
            self.scope_id = Fact::Unknown("IPv4 has no IPv6 scope id".into());
        }
        if !valid_family(self.family, self.address) {
            return Err("address family mismatch".into());
        }
        let max = if self.family == AddressFamily::V4 {
            32
        } else {
            128
        };
        if matches!(self.prefix_len,Fact::Known(bits) if bits == 0 || bits > max) {
            self.prefix_len = Fact::Unknown("unsupported/sentinel interface prefix length".into());
        }
        Ok(())
    }
}
impl Observation for WindowsRouteObservation {
    const LIMIT: usize = MAX_ROUTE_ROWS;
    fn interface(&self) -> Option<&WindowsInterfaceRef> {
        Some(&self.interface)
    }
    fn family(&self) -> Option<AddressFamily> {
        Some(self.family)
    }
    fn bytes(&self) -> usize {
        ref_bytes(&self.interface)
            .saturating_add(self.prefix.len())
            .saturating_add(fact_bytes(&self.next_hop, |_| 0))
            .saturating_add(fact_bytes(&self.next_hop_scope_id, |_| 0))
            .saturating_add(fact_bytes(&self.route_metric, |_| 0))
            .saturating_add(fact_bytes(&self.interface_metric, |_| 0))
    }
    fn normalize(&mut self) -> Result<(), String> {
        clean_ref(&mut self.interface);
        if self
            .prefix
            .split_once('/')
            .is_some_and(|(_, bits)| bits.is_empty() || !bits.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("invalid route prefix length".into());
        }
        self.prefix = normalize_cidr(&self.prefix).ok_or("invalid route prefix")?;
        let address: IpAddr = self
            .prefix
            .split_once('/')
            .ok_or("missing route prefix length")?
            .0
            .parse()
            .map_err(|_| "invalid route address")?;
        if !valid_family(self.family, address) {
            return Err("route address family mismatch".into());
        }
        if matches!(self.next_hop, Fact::Known(address) if !valid_family(self.family,address)) {
            self.next_hop = Fact::Unknown("next hop family differs from source".into());
        }
        if self.family == AddressFamily::V4 {
            self.next_hop_scope_id = Fact::Unknown("IPv4 has no IPv6 scope id".into());
        }
        for metric in [&mut self.route_metric, &mut self.interface_metric] {
            if *metric == Fact::Known(u32::MAX) {
                *metric = Fact::Unknown("unused metric sentinel".into());
            }
        }
        Ok(())
    }
}
impl Observation for WindowsRasObservation {
    const LIMIT: usize = MAX_INTERFACE_ROWS;
    fn interface(&self) -> Option<&WindowsInterfaceRef> {
        if let Fact::Known(r) = &self.interface {
            Some(r)
        } else {
            None
        }
    }
    fn bytes(&self) -> usize {
        fact_bytes(&self.name, String::len).saturating_add(fact_bytes(&self.interface, ref_bytes))
    }
    fn normalize(&mut self) -> Result<(), String> {
        self.name = match &self.name {
            Fact::Known(name) => private_ras_name(name),
            Fact::Unknown(_) => Fact::Unknown("RAS entry name not established".into()),
        };
        if let Fact::Known(name) = &self.name {
            if name.trim().is_empty() || name.chars().any(char::is_control) {
                return Err("invalid RAS name".into());
            }
        }
        if let Fact::Known(r) = &mut self.interface {
            clean_ref(r);
            if key(r, None).is_none() {
                self.interface = Fact::Unknown(format!(
                    "RAS association lacks a family-qualified snapshot key: {r:?}"
                ));
            }
        }
        Ok(())
    }
}
fn string_budget<T: Observation>(source: &ReadRows<T>) -> usize {
    source.rows.iter().fold(
        fact_bytes(&source.complete, |_| 0)
            .saturating_add(fact_bytes(&source.compartment, |_| 0))
            .saturating_add(fact_bytes(&source.error, |v| {
                v.as_ref().map_or(0, String::len)
            })),
        |sum, row| sum.saturating_add(row.bytes()),
    )
}
fn validate_source<T: Observation>(
    source: Fact<ReadRows<T>>,
    family: Option<AddressFamily>,
    name: &str,
) -> Fact<ReadRows<T>> {
    let mut source = match source {
        Fact::Known(source) => source,
        Fact::Unknown(reason) => {
            return Fact::Unknown(if reason.len() <= MAX_SOURCE_BYTES {
                reason
            } else {
                format!("Windows {name}: diagnostic exceeds byte limit")
            })
        }
    };
    let validate = || -> Result<ReadRows<T>, String> {
        if source.rows.len() > T::LIMIT {
            return Err("row limit exceeded".into());
        }
        if string_budget(&source) > MAX_SOURCE_BYTES {
            return Err("string-byte limit exceeded".into());
        }
        if source.compartment == Fact::Known(0) {
            source.compartment =
                Fact::Unknown("zero compartment is not a verified read scope".into());
        }
        if source.error != Fact::Known(None) {
            unprove(&mut source.complete, "source error not established absent");
        }
        if matches!(source.compartment, Fact::Unknown(_)) {
            unprove(&mut source.complete, "unknown read compartment");
        }
        let mut references = References::default();
        let mut counts = BTreeMap::<Key, usize>::new();
        let mut alias_counts = BTreeMap::<String, usize>::new();
        let mut seen = BTreeSet::new();
        for row in &mut source.rows {
            row.normalize()?;
            if family.is_some() && row.family() != family {
                return Err("query/row family mismatch".into());
            }
            let Some(r) = row.interface() else {
                unprove(&mut source.complete, "missing interface association");
                continue;
            };
            references.observe(r, row.family())?;
            if T::ADDRESS_LIMIT {
                // Budget grouping is conservative, not a cross-family join proof.
                if let Fact::Known(alias) = &r.alias {
                    let count = alias_counts.entry(alias.clone()).or_default();
                    *count += 1;
                    if *count > MAX_INTERFACE_ADDRESSES {
                        return Err("per-alias address limit exceeded".into());
                    }
                }
            }
            let Some(key) = key(r, row.family()) else {
                unprove(&mut source.complete, "unproved snapshot interface key");
                continue;
            };
            if T::UNIQUE && !seen.insert(key.clone()) {
                return Err("duplicate adapter snapshot key".into());
            }
            if T::ADDRESS_LIMIT {
                let count = counts.entry(key).or_default();
                *count += 1;
                if *count > MAX_INTERFACE_ADDRESSES {
                    return Err("per-interface address limit exceeded".into());
                }
            }
        }
        if string_budget(&source) > MAX_SOURCE_BYTES {
            return Err("normalized string-byte limit exceeded".into());
        }
        Ok(source)
    };
    match validate() {
        Ok(source) => Fact::Known(source),
        Err(reason) => Fact::Unknown(format!("Windows {name}: {reason}")),
    }
}
fn check_adapter_conflicts<T: Observation>(
    source: &mut Fact<ReadRows<T>>,
    adapters: &Fact<ReadRows<WindowsAdapterObservation>>,
    name: &str,
) {
    let (Fact::Known(rows), Fact::Known(adapters)) = (&*source, adapters) else {
        return;
    };
    let (Fact::Known(a), Fact::Known(b)) = (&rows.compartment, &adapters.compartment) else {
        return;
    };
    if a != b {
        return;
    } // no borrowing across namespaces
    let mut references = References::default();
    for row in &adapters.rows {
        if references.observe(&row.interface, None).is_err() {
            return;
        }
    }
    for row in &rows.rows {
        if let Some(r) = row.interface() {
            if let Err(reason) = references.observe(r, row.family()) {
                *source = Fact::Unknown(format!("Windows {name}: adapter {reason}"));
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests;
