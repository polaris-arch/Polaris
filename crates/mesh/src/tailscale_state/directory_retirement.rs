//! Whole-node directory evidence. Parsing never confers native custody or deletion authority.
//! Authentication retirement remains a separate contract.
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
pub enum DirectoryWriterState { Unknown, SealedDrained, NoDirectoryConstruction }

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TailscaleDirectoryNode {
    pub tag: String,
    pub state_directory: String,
    pub directory_identity: String,
    pub incarnation: String,
    pub directory_epoch: String,
    pub writer_state: DirectoryWriterState,
    pub census_version: u32,
    pub failure: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TailscaleDirectoryInstance {
    pub run_nonce: String,
    pub config_digest: String,
    pub terminal: DirectoryWriterState,
    pub census_complete: bool,
    pub nodes: Vec<TailscaleDirectoryNode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TailscaleDirectoryExport {
    contract_version: String,
    global_cleanup_evidence: String,
    instances: Vec<TailscaleDirectoryInstance>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedTailscaleDirectoryScope {
    pub tag: String,
    pub state_directory: String,
    pub directory_identity: String,
    pub incarnation: String,
    pub directory_epoch: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedTailscaleDirectoryRun {
    pub run_nonce: String,
    pub config_digest: String,
    pub scopes: Vec<ObservedTailscaleDirectoryScope>,
}

pub fn lower_hex(value: &str, size: usize) -> bool {
    value.len() == size && value.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn identity_valid(value: &str) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 5 && parts[0] == "unix" && parts[1..].iter().all(|p|
        p.parse::<u64>().is_ok_and(|n| n.to_string() == *p))
}
fn path_valid(value: &str) -> bool {
    value.starts_with('/') && value.len() > 1 && !value.contains('\0') && !value.ends_with('/')
        && value[1..].split('/').all(|p| !p.is_empty() && p != "." && p != "..")
}
impl TailscaleDirectoryExport {
    pub fn instances(&self) -> &[TailscaleDirectoryInstance] { &self.instances }
    pub fn observed_runs(&self) -> Result<Vec<ObservedTailscaleDirectoryRun>, String> {
        let unknown = || "CleanupUnknown: invalid original directory scope".to_owned();
        if self.contract_version != "polaris-ts-directory-retirement-v1" || self.global_cleanup_evidence != "CleanupUnknown"
            || self.instances.is_empty() || self.instances.len() > 256 { return Err(unknown()); }
        let mut runs = std::collections::BTreeSet::new();
        let mut result = Vec::new();
        for run in &self.instances {
            if !run.census_complete || !lower_hex(&run.run_nonce, 64) || !lower_hex(&run.config_digest, 64)
                || !runs.insert(&run.run_nonce) || run.nodes.len() > 256 { return Err(unknown()); }
            let mut scopes = std::collections::BTreeSet::new();
            let mut tags = std::collections::BTreeSet::new();
            let mut paths = std::collections::BTreeSet::new();
            for node in &run.nodes {
                if !tags.insert(&node.tag) || !paths.insert(&node.state_directory) || node.tag.is_empty() || !identity_valid(&node.directory_identity) || !lower_hex(&node.incarnation, 32) || !lower_hex(&node.directory_epoch, 32)
                    || !path_valid(&node.state_directory) || node.census_version != 1 { return Err(unknown()); }
                if !scopes.insert(ObservedTailscaleDirectoryScope { tag: node.tag.clone(), state_directory: node.state_directory.clone(), directory_identity: node.directory_identity.clone(), incarnation: node.incarnation.clone(), directory_epoch: node.directory_epoch.clone() }) { return Err(unknown()); }
            }
            result.push(ObservedTailscaleDirectoryRun { run_nonce: run.run_nonce.clone(), config_digest: run.config_digest.clone(), scopes: scopes.into_iter().collect() });
        }
        Ok(result)
    }
    pub fn validate(&self, digest: &str, terminal: bool) -> Result<(), String> {
        let observed = self.observed_runs()?;
        if !lower_hex(digest, 64) || !observed.iter().any(|r| r.config_digest == digest) {
            return Err("CleanupUnknown: directory config binding differs".into());
        }
        if terminal { self.validate_observed_runs(&observed, true) } else { Ok(()) }
    }

    pub fn validate_observed_runs(&self, observed: &[ObservedTailscaleDirectoryRun], terminal: bool) -> Result<(), String> {
        let unknown = || "CleanupUnknown: original directory census differs".to_owned();
        if !terminal || self.contract_version != "polaris-ts-directory-retirement-v1"
            || self.global_cleanup_evidence != "CleanupUnknown" || observed.is_empty()
            || observed.len() > 256 || observed.len() != self.instances.len() { return Err(unknown()); }
        let originals: BTreeMap<_, _> = observed.iter().map(|r| (r.run_nonce.as_str(), r)).collect();
        if originals.len() != observed.len() { return Err(unknown()); }
        let mut seen = std::collections::BTreeSet::new();
        for instance in &self.instances {
            let original = originals.get(instance.run_nonce.as_str()).ok_or_else(unknown)?;
            if !seen.insert(&instance.run_nonce) || !lower_hex(&instance.run_nonce, 64)
                || !lower_hex(&instance.config_digest, 64) || instance.config_digest != original.config_digest
                || !instance.census_complete || instance.nodes.len() != original.scopes.len()
                || instance.nodes.len() > 256 { return Err(unknown()); }
            if instance.nodes.is_empty() {
                if instance.terminal != DirectoryWriterState::NoDirectoryConstruction { return Err(unknown()); }
                continue;
            }
            if instance.terminal != DirectoryWriterState::SealedDrained { return Err(unknown()); }
            let expected: std::collections::BTreeSet<_> = original.scopes.iter().cloned().collect();
            let mut actual = std::collections::BTreeSet::new();
            let mut tags = std::collections::BTreeSet::new();
            let mut paths = std::collections::BTreeSet::new();
            for node in &instance.nodes {
                if !tags.insert(&node.tag) || !paths.insert(&node.state_directory) || node.writer_state != DirectoryWriterState::SealedDrained || node.census_version != 1
                    || !node.failure.is_empty() || node.tag.is_empty() || !identity_valid(&node.directory_identity)
                    || !path_valid(&node.state_directory) || !lower_hex(&node.incarnation, 32) || !lower_hex(&node.directory_epoch, 32) { return Err(unknown()); }
                actual.insert(ObservedTailscaleDirectoryScope { tag: node.tag.clone(), state_directory: node.state_directory.clone(), directory_identity: node.directory_identity.clone(), incarnation: node.incarnation.clone(), directory_epoch: node.directory_epoch.clone() });
            }
            if expected.len() != original.scopes.len() || actual.len() != instance.nodes.len() || actual != expected { return Err(unknown()); }
        }
        Ok(())
    }
}

/// Durable deletion intent is bound before native Stop. No timestamps or PIDs confer authority.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DirectoryDeletionIntent {
    pub server_id: String,
    pub incarnation: String,
    pub directory_identity: String,
    pub state_directory: String,
    pub delete_intent_nonce: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    fn export() -> TailscaleDirectoryExport {
        serde_json::from_value(serde_json::json!({
            "contractVersion":"polaris-ts-directory-retirement-v1", "globalCleanupEvidence":"CleanupUnknown",
            "instances":[{"runNonce":"a".repeat(64),"configDigest":"b".repeat(64),"terminal":"SealedDrained","censusComplete":true,
            "nodes":[{"tag":"ts-node","stateDirectory":"/state/tailscale/node","directoryIdentity":"unix:1:2:1:3","incarnation":"c".repeat(32),"directoryEpoch":"e".repeat(32),"writerState":"SealedDrained","censusVersion":1,"failure":""}]}]
        })).unwrap()
    }
    #[test] fn reload_history_preserves_each_digest_with_a_current_anchor() {
        let mut evidence = export();
        let mut historical = evidence.instances[0].clone();
        historical.run_nonce = "f".repeat(64); historical.config_digest = "e".repeat(64);
        evidence.instances.insert(0, historical);
        let original = evidence.observed_runs().unwrap();
        assert!(evidence.validate(&"b".repeat(64), false).is_ok());
        assert!(evidence.validate_observed_runs(&original, true).is_ok());
        assert!(evidence.validate(&"d".repeat(64), false).is_err());
        evidence.instances[0].config_digest = "d".repeat(64);
        assert!(evidence.validate_observed_runs(&original, true).is_err());
    }
    #[test] fn original_census_is_exact() {
        let mut evidence = export(); let original = evidence.observed_runs().unwrap();
        assert!(evidence.validate_observed_runs(&original,true).is_ok());
        evidence.instances[0].nodes[0].incarnation = "d".repeat(32);
        assert!(evidence.validate_observed_runs(&original,true).is_err());
    }
    #[test] fn old_run_partial_unknown_and_wrong_abi_never_authorize() {
        let original = export().observed_runs().unwrap();
        for change in 0..5 {
            let mut evidence = export();
            match change { 0 => evidence.instances[0].run_nonce = "d".repeat(64), 1 => evidence.instances[0].census_complete = false,
                2 => evidence.instances[0].nodes[0].writer_state = DirectoryWriterState::Unknown,
                3 => evidence.instances[0].nodes[0].census_version = 2, _ => { let node = evidence.instances[0].nodes[0].clone(); evidence.instances[0].nodes.push(node); } }
            assert!(evidence.validate_observed_runs(&original,true).is_err());
        }
    }
    #[test] fn no_construction_is_not_a_directory_proof() {
        let mut evidence = export(); let original = evidence.observed_runs().unwrap();
        evidence.instances[0].nodes.clear(); evidence.instances[0].terminal = DirectoryWriterState::NoDirectoryConstruction;
        assert!(evidence.validate_observed_runs(&original,true).is_err());
        assert!(evidence.instances()[0].nodes.is_empty());
    }
    #[test] fn auth_contract_and_global_clean_claim_are_rejected() {
        let mut evidence = export(); let original = evidence.observed_runs().unwrap();
        evidence.contract_version = "polaris-ts-store-retirement-v1".into(); assert!(evidence.validate_observed_runs(&original,true).is_err());
        let mut evidence = export(); evidence.global_cleanup_evidence = "NoOwner".into(); assert!(evidence.validate_observed_runs(&original,true).is_err());
    }
}
