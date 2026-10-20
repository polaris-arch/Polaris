//! Explicit, CAS-bound recovery of ordinary duplicate server identities.
use super::*;
use polaris_config_engine::user_config::UserConfig;
use std::collections::{HashMap, HashSet};

pub(crate) const IDENTITY_AMBIGUOUS: &str = "serverIdentityAmbiguous";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityConflictRecord {
    index: usize,
    name: String,
    protocol: String,
    address: Option<String>,
    port: Option<u64>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityConflictGroup {
    indices: Vec<usize>,
    repairable: bool,
    records: Vec<IdentityConflictRecord>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityConflicts {
    version: String,
    groups: Vec<IdentityConflictGroup>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityRepairChoice {
    pub indices: Vec<usize>,
    pub keeper_index: usize,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentityRepairRequest {
    pub version: String,
    pub choices: Vec<IdentityRepairChoice>,
}

fn duplicate_groups(raw: &Value) -> Vec<Vec<usize>> {
    let mut by_id: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, node) in raw
        .get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        if let Some(id) = node.get("id").and_then(Value::as_str) {
            by_id.entry(id).or_default().push(index);
        }
    }
    let mut groups: Vec<_> = by_id
        .into_values()
        .filter(|indices| indices.len() > 1)
        .collect();
    groups.sort_by_key(|indices| indices[0]);
    groups
}

fn ordinary_node(node: &Value) -> bool {
    node.get("protocol")
        .and_then(Value::as_str)
        .is_some_and(|protocol| !protocol.eq_ignore_ascii_case("tailscale"))
        && node.get("tailscaleSettings").is_none()
        && node.get("tailscaleCredentialIntent").is_none()
        && node
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|id| !id.trim().is_empty())
}

pub(crate) fn validate_server_identities(raw: &Value) -> Result<(), StoreError> {
    if duplicate_groups(raw).is_empty() {
        Ok(())
    } else {
        Err(StoreError::validation(IDENTITY_AMBIGUOUS))
    }
}

impl ConfigManager {
    /// Read only. No load migration, credential material, or node IDs are exposed.
    pub(crate) fn identity_conflicts(&self) -> Result<IdentityConflicts, StoreError> {
        deny_inside_projection("identity_conflicts");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let raw = self.raw_disk_for_mesh_under_write_lock()?;
        let groups = duplicate_groups(&raw)
            .into_iter()
            .map(|indices| {
                let repairable = indices
                    .iter()
                    .all(|index| ordinary_node(&raw["servers"][*index]));
                let records = indices
                    .iter()
                    .map(|index| {
                        let node = &raw["servers"][*index];
                        let text = |key| {
                            node.get(key)
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_owned()
                        };
                        IdentityConflictRecord {
                            index: *index,
                            name: text("name"),
                            protocol: text("protocol"),
                            address: node
                                .get("address")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                            port: node.get("port").and_then(Value::as_u64),
                        }
                    })
                    .collect();
                IdentityConflictGroup {
                    indices,
                    repairable,
                    records,
                }
            })
            .collect();
        Ok(IdentityConflicts {
            version: Self::raw_mesh_document_digest(&raw)?,
            groups,
        })
    }

    /// The existing config lock fences application writers, not arbitrary external file writers.
    /// This capability renames only proved ordinary records. It cannot edit credentials.
    pub(crate) fn repair_server_identities(
        &self,
        request: &IdentityRepairRequest,
        mut new_id: impl FnMut() -> String,
    ) -> Result<Value, StoreError> {
        deny_inside_projection("repair_server_identities");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let raw = self.raw_disk_for_mesh_under_write_lock()?;
        if Self::raw_mesh_document_digest(&raw)? != request.version {
            return Err(StoreError::validation("serverIdentityRevisionChanged"));
        }
        let groups = duplicate_groups(&raw);
        if groups.is_empty() || groups.len() != request.choices.len() {
            return Err(StoreError::validation("serverIdentityRepairInvalidChoice"));
        }
        for (indices, choice) in groups.iter().zip(&request.choices) {
            if *indices != choice.indices || !indices.contains(&choice.keeper_index) {
                return Err(StoreError::validation("serverIdentityRepairInvalidChoice"));
            }
            if !indices
                .iter()
                .all(|index| ordinary_node(&raw["servers"][*index]))
            {
                return Err(StoreError::validation("serverIdentityRepairUnsupported"));
            }
        }
        let mut incoming = raw.clone();
        let mut used: HashSet<String> = raw["servers"]
            .as_array()
            .ok_or_else(|| StoreError::validation("serverIdentityRepairUnsupported"))?
            .iter()
            .filter_map(|node| node.get("id").and_then(Value::as_str).map(str::to_owned))
            .collect();
        for choice in &request.choices {
            for index in &choice.indices {
                if *index == choice.keeper_index {
                    continue;
                }
                let id = new_id();
                // A collision is a failure, never an unbounded retry or silent overwrite.
                if id.trim().is_empty() || !used.insert(id.clone()) {
                    return Err(StoreError::validation("serverIdentityRepairInvalidChoice"));
                }
                incoming["servers"][*index]["id"] = Value::String(id);
            }
        }
        validate_server_identities(&incoming)?;
        // Only IDs of credential-free ordinary records differ. Rebase the credential
        // lookup onto that proved identity mapping; every TS owner and byte is unchanged.
        // No caller-supplied document or trusted credential target enters this path.
        let mut checked = incoming.clone();
        reconcile_tailscale_credentials(&incoming, &mut checked, None)?;
        mesh_guard::reconcile_untrusted(&raw, &mut checked)?;
        // Apply the existing legacy read defaults to the validation view only.
        // Missing old optional defaults must not block a narrow identity repair.
        polaris_store::finalize_config(&mut checked)?;
        let canonical = ConfigStore::canonicalize_for_save(&checked)?;
        // Canonicalization is validation only here: a legacy raw document may omit
        // modern defaults. Persisting its normalized form would edit unrelated data.
        // Every repaired record must nevertheless survive the normal store pipeline.
        let canonical_ids: HashSet<&str> = canonical["servers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|node| node.get("id").and_then(Value::as_str))
            .collect();
        if request
            .choices
            .iter()
            .flat_map(|choice| &choice.indices)
            .any(|index| {
                incoming["servers"][*index]["id"]
                    .as_str()
                    .is_none_or(|id| !canonical_ids.contains(id))
            })
        {
            return Err(StoreError::validation("serverIdentityRepairUnsupported"));
        }
        // Store canonicalization is tolerant Value validation, not typed runtime
        // admission. Parse the complete canonical view exactly as runtime consumers
        // do before publishing a successful repair. Do not serialize this typed view
        // back to disk: unknown fields and legacy omissions must remain unchanged.
        // Keep deserialization details private because errors may include field values.
        serde_json::from_value::<UserConfig>(canonical.clone())
            .map_err(|_| StoreError::validation("serverIdentityRepairUnsupported"))?;
        // This narrow recovery path has no caller-supplied content: incoming is the
        // locked disk snapshot with only the validated nonkeeper ID cells replaced.
        // Reuse the durable 0600 atomic writer; do not route through a generic raw-save API.
        #[cfg(test)]
        if self
            .test_fail_before_config_persist_once
            .swap(false, Ordering::SeqCst)
        {
            return Err(StoreError::Io("injected config persistence failure".into()));
        }
        let content = serde_json::to_string_pretty(&incoming).map_err(StoreError::from)?;
        let result = durable_atomic_write(&self.path, &content, &random_tmp_suffix());
        #[cfg(test)]
        let result = match result {
            Ok(_)
                if self
                    .test_uncertain_after_mesh_publish_once
                    .swap(false, Ordering::SeqCst) =>
            {
                Err(StoreError::CommitUncertain(
                    "injected after identity repair publish".into(),
                ))
            }
            other => other,
        };
        match result {
            Ok(_) => {
                // Readers see the usual normalized view, while the repair preserves
                // the disk's unrelated legacy fields byte-for-value. The next ordinary
                // config transaction retains its existing migration semantics.
                if let Ok(mut cache) = self.cache.write() {
                    *cache = Some(canonical);
                }
                Ok(incoming)
            }
            Err(error @ StoreError::CommitUncertain(_)) => {
                self.clear_cached_config();
                Err(error)
            }
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests;
