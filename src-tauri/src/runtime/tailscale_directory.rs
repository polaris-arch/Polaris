//! Cross-process directory identity. Only whole-node deletion uses this module.
//! Stable lock files are outside targets and are never removed or replaced.
#![forbid(unsafe_code)]
use polaris_mesh::tailscale_state::directory_retirement::DirectoryDeletionIntent;
use serde::{Deserialize, Serialize};
use std::io;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DirectoryMarker {
    version: u32,
    server_id: String,
    incarnation: String,
    directory_epoch: String,
    root_device: u64,
    root_inode: u64,
    directory_device: u64,
    directory_inode: u64,
    phase: String,
    delete_intent_nonce: String,
}
impl DirectoryMarker {
    fn identity(&self) -> String {
        format!(
            "unix:{}:{}:{}:{}",
            self.root_device, self.root_inode, self.directory_device, self.directory_inode
        )
    }
    fn matches(&self, intent: &DirectoryDeletionIntent) -> bool {
        self.version == 1
            && self.server_id == intent.server_id
            && self.incarnation == intent.incarnation
            && self.identity() == intent.directory_identity
            && polaris_mesh::tailscale_state::directory_retirement::lower_hex(&self.incarnation, 32)
            && polaris_mesh::tailscale_state::directory_retirement::lower_hex(
                &self.directory_epoch,
                32,
            )
            && polaris_mesh::tailscale_state::directory_retirement::lower_hex(
                &intent.delete_intent_nonce,
                32,
            )
            && (self.phase == "Open" && self.delete_intent_nonce.is_empty()
                || matches!(self.phase.as_str(), "Retiring" | "Removing" | "Deleted")
                    && self.delete_intent_nonce == intent.delete_intent_nonce)
    }
}
fn unknown() -> io::Error {
    io::Error::new(
        io::ErrorKind::WouldBlock,
        "CleanupUnknown: directory identity or lease unavailable",
    )
}

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::DirectoryLease;

#[cfg(all(target_os = "ios", unix))]
pub(crate) fn resume_committed_directory(
    config: &std::path::Path,
    intent: &DirectoryDeletionIntent,
    current: &serde_json::Value,
) -> io::Result<DirectoryLease> {
    let lease = DirectoryLease::acquire_intent(config, intent)?;
    lease.ensure_no_configured_alias(intent, current)?;
    lease.resume_committed(intent)?;
    Ok(lease)
}

#[cfg(any(target_os = "ios", test))]
fn terminal_epochs_for_intent(
    instances: &[polaris_mesh::tailscale_state::directory_retirement::TailscaleDirectoryInstance],
    intent: &DirectoryDeletionIntent,
) -> io::Result<std::collections::BTreeSet<String>> {
    use polaris_mesh::tailscale_state::directory_retirement::DirectoryWriterState;
    let mut epochs = std::collections::BTreeSet::new();
    for run in instances {
        if !run.census_complete
            || !matches!(
                run.terminal,
                DirectoryWriterState::SealedDrained | DirectoryWriterState::NoDirectoryConstruction
            )
        {
            return Err(unknown());
        }
        for node in &run.nodes {
            // First retain the terminal guarantee for the complete original history.
            if node.writer_state != DirectoryWriterState::SealedDrained
                || node.census_version != 1
                || !node.failure.is_empty()
            {
                return Err(unknown());
            }
            // A drained former incarnation at this basename cannot veto the current
            // one, nor contribute an epoch that authorizes the current physical object.
            if node.state_directory == intent.state_directory
                && node.directory_identity == intent.directory_identity
                && node.incarnation == intent.incarnation
            {
                epochs.insert(node.directory_epoch.clone());
            }
        }
    }
    if epochs.is_empty() {
        return Err(unknown());
    }
    Ok(epochs)
}

/// Receipt capability cannot be reconstructed from journal JSON. The native
/// adapter issues it only for the original Stop and complete original census.
#[cfg(all(target_os = "ios", unix))]
pub(crate) fn commit_original_directory(
    config: &std::path::Path,
    intent: &DirectoryDeletionIntent,
    receipt: &tauri_plugin_polaris_ios::TailscaleDirectoryRetirementReceipt,
    current: &serde_json::Value,
) -> io::Result<DirectoryLease> {
    let epochs = terminal_epochs_for_intent(receipt.instances(), intent)?;
    let lease = DirectoryLease::acquire_intent(config, intent)?;
    lease.ensure_no_configured_alias(intent, current)?;
    lease.delete_bound(intent, &epochs)?;
    // Retain this until the caller has persisted journal acknowledgement.
    Ok(lease)
}

#[cfg(test)]
mod tests;

#[cfg(windows)]
pub(crate) use crate::runtime::windows_directory::DirectoryLease;
