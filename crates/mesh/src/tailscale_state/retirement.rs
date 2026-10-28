//! Shared parsing and field validation for the original Go TS StateStore export.
//!
//! These data and checks do not attest producer identity, native disposal, state
//! ownership, or permission to project authentication files. Platform adapters
//! retain their original private custody and receipt constructors.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum TailscaleWriterState {
    Unknown,
    SealedDrained,
    NoStoreConstruction,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum TailscaleStateFileState {
    Unknown,
    Missing,
    Regular,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
pub enum TailscaleProfileState {
    Unknown,
    None,
    Bound,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TailscaleStoreRetirementNode {
    pub tag: String,
    pub state_directory: String,
    pub state_file: String,
    pub writer_state: TailscaleWriterState,
    pub state_file_state: TailscaleStateFileState,
    pub state_file_revision: String,
    pub profile_state: TailscaleProfileState,
    pub profile_fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TailscaleStoreRetirementInstance {
    pub run_nonce: String,
    pub config_digest: String,
    pub terminal: TailscaleWriterState,
    pub census_complete: bool,
    pub nodes: Vec<TailscaleStoreRetirementNode>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TailscaleStoreExport {
    contract_version: String,
    global_cleanup_evidence: String,
    instances: Vec<TailscaleStoreRetirementInstance>,
}

/// Independently observed original native run metadata. These public data do
/// not establish platform custody or confer authority to modify state.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedTailscaleStoreRun {
    pub run_nonce: String,
    pub config_digest: String,
    pub scopes: Vec<ObservedTailscaleStoreScope>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObservedTailscaleStoreScope {
    pub tag: String,
    pub state_directory: String,
    pub state_file: String,
}

fn hex64(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl TailscaleStoreExport {
    /// Borrow the original parsed instance list without minting a native receipt.
    pub fn instances(&self) -> &[TailscaleStoreRetirementInstance] {
        &self.instances
    }

    /// Check the original single-digest grammar and fields, not their provenance.
    pub fn validate(&self, digest: &str, terminal: bool) -> Result<(), String> {
        if !hex64(digest) {
            return Err(
                "CleanupUnknown: Original TS Store export header/scope is incomplete".into(),
            );
        }
        self.validate_binding(terminal, |instance| instance.config_digest == digest)
    }

    /// Validate the whole export against independently captured same-host run
    /// metadata. Callers retain the provenance and authority of those captures.
    pub fn validate_observed_runs(
        &self,
        observed: &[ObservedTailscaleStoreRun],
        terminal: bool,
    ) -> Result<(), String> {
        let differs = || "CleanupUnknown: Original observed run census differs".to_owned();
        let mut originals = std::collections::BTreeMap::new();
        for run in observed {
            if !hex64(&run.run_nonce)
                || !hex64(&run.config_digest)
                || originals.insert(run.run_nonce.as_str(), run).is_some()
            {
                return Err(differs());
            }
        }
        if observed.is_empty() || observed.len() != self.instances.len() {
            return Err(differs());
        }
        self.validate_binding(terminal, |instance| {
            originals
                .get(instance.run_nonce.as_str())
                .is_some_and(|run| run.config_digest == instance.config_digest)
        })?;
        for instance in &self.instances {
            let run = originals
                .get(instance.run_nonce.as_str())
                .ok_or_else(differs)?;
            let scopes: std::collections::BTreeSet<_> = run.scopes.iter().collect();
            let nodes: std::collections::BTreeSet<_> = instance
                .nodes
                .iter()
                .map(|node| ObservedTailscaleStoreScope {
                    tag: node.tag.clone(),
                    state_directory: node.state_directory.clone(),
                    state_file: node.state_file.clone(),
                })
                .collect();
            if scopes.len() != run.scopes.len()
                || nodes.len() != instance.nodes.len()
                || scopes.len() != nodes.len()
                || !nodes.iter().all(|node| scopes.contains(node))
            {
                return Err(differs());
            }
        }
        Ok(())
    }

    fn validate_binding(
        &self,
        terminal: bool,
        binding: impl Fn(&TailscaleStoreRetirementInstance) -> bool,
    ) -> Result<(), String> {
        if self.contract_version != "polaris-ts-auth-writer-retirement-v1"
            || self.global_cleanup_evidence != "CleanupUnknown"
            || self.instances.is_empty()
        {
            return Err(
                "CleanupUnknown: Original TS Store export header/scope is incomplete".into(),
            );
        }
        let mut nonces = std::collections::BTreeSet::new();
        for instance in &self.instances {
            if !hex64(&instance.run_nonce)
                || !nonces.insert(&instance.run_nonce)
                || !binding(instance)
                || (terminal
                    && (!instance.census_complete
                        || instance.terminal == TailscaleWriterState::Unknown))
            {
                return Err(
                    "CleanupUnknown: Original instance census or config binding differs".into(),
                );
            }
            let mut tags = std::collections::BTreeSet::new();
            let mut directories = std::collections::BTreeSet::new();
            for node in &instance.nodes {
                let directory = &node.state_directory;
                if node.tag.is_empty()
                    || !tags.insert(&node.tag)
                    || !directories.insert(directory)
                    || !directory.starts_with('/')
                    || directory == "/"
                    || directory.contains('\0')
                    || directory
                        .split('/')
                        .skip(1)
                        .any(|part| part.is_empty() || part == "." || part == "..")
                    || node.state_file != format!("{directory}/tailscaled.state")
                    || (node.state_file_state == TailscaleStateFileState::Regular
                        && !hex64(&node.state_file_revision))
                    || (node.state_file_state != TailscaleStateFileState::Regular
                        && !node.state_file_revision.is_empty())
                    || (node.profile_state == TailscaleProfileState::Bound
                        && (!hex64(&node.profile_fingerprint)
                            || node.state_file_state != TailscaleStateFileState::Regular))
                    || (node.profile_state != TailscaleProfileState::Bound
                        && !node.profile_fingerprint.is_empty())
                    || (terminal && node.writer_state == TailscaleWriterState::Unknown)
                {
                    return Err("CleanupUnknown: Original TS node canonical scope or writer/file/profile facts differ".into());
                }
            }
            if terminal {
                let expected = if instance
                    .nodes
                    .iter()
                    .any(|node| node.writer_state == TailscaleWriterState::SealedDrained)
                {
                    TailscaleWriterState::SealedDrained
                } else {
                    TailscaleWriterState::NoStoreConstruction
                };
                if instance.terminal != expected {
                    return Err(
                        "CleanupUnknown: Instance terminal does not cover its complete nodes"
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}
