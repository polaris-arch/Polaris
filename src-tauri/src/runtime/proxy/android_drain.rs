//! Strict, read-only validation of Android's process-wide native drain ledger.
//! A valid shape is not a NoOldCore receipt and changes no runtime ownership.

use std::collections::HashSet;

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub(super) enum Kind {
    Main,
    Login,
    Speedtest,
    CheckConfig,
    TargetlessStop,
    TargetlessReload,
}

impl Kind {
    fn is_owner(self) -> bool {
        matches!(self, Self::Main | Self::Login | Self::Speedtest)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(super) enum State {
    Reserved,
    BirthEntered,
    CancelledBeforeBirth,
    ClosedExact,
    Completed,
    Unknown,
    ValidationCleanupUnknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Ticket {
    pub id: String,
    pub kind: Kind,
    pub logical_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub ticket: Ticket,
    pub state: State,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AndroidDrainReceipt {
    pub protocol_version: u32,
    pub covered_producers: Vec<String>,
    pub coverage_complete: bool,
    pub process_nonce: String,
    pub fence_id: String,
    pub marker_proof: MarkerProof,
    pub sealed_revision: u64,
    pub revision: u64,
    pub captured_count: usize,
    pub captured: Vec<Entry>,
}

const REQUIRED_PRODUCERS: &[&str] = &[
    "main.bridge",
    "main.system",
    "main.close",
    "main.reload",
    "login.start",
    "login.close",
    "speedtest.start",
    "speedtest.close",
    "validation.checkConfig",
    "control.targetlessStop",
    "control.targetlessReload",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(super) enum MarkerProof {
    Absent,
    PresentOrUnknown,
}

/// Success only proves the frozen captured set is internally consistent and terminal.
/// Callers must separately prove the producer coverage and native close semantics.
pub(super) fn verify(
    receipt: &AndroidDrainReceipt,
    expected_process: &str,
    expected_fence: &str,
) -> Result<(), &'static str> {
    if receipt.protocol_version != 1 || !receipt.coverage_complete {
        return Err("Android native drain producer coverage incomplete");
    }
    let reported: HashSet<&str> = receipt
        .covered_producers
        .iter()
        .map(String::as_str)
        .collect();
    let required: HashSet<&str> = REQUIRED_PRODUCERS.iter().copied().collect();
    if reported != required || reported.len() != receipt.covered_producers.len() {
        return Err("Android native drain producer manifest mismatch");
    }
    if !valid_id(expected_process, 128)
        || !valid_id(&receipt.process_nonce, 128)
        || receipt.process_nonce != expected_process
    {
        return Err("Android native drain process nonce mismatch");
    }
    if !valid_id(expected_fence, 128)
        || !valid_id(&receipt.fence_id, 128)
        || receipt.fence_id != expected_fence
    {
        return Err("Android native drain fence mismatch");
    }
    if receipt.marker_proof != MarkerProof::Absent {
        return Err("Android native drain cold marker was not proved absent");
    }
    if receipt.sealed_revision == 0 || receipt.revision < receipt.sealed_revision {
        return Err("Android native drain revision invalid");
    }
    if receipt.captured_count != receipt.captured.len() {
        return Err("Android native drain frozen ticket count mismatch");
    }
    let mut tickets = HashSet::new();
    let mut owners = HashSet::new();
    for entry in &receipt.captured {
        let ticket = &entry.ticket;
        if !valid_id(&ticket.id, 128)
            || !valid_id(&ticket.logical_id, 256)
            || !tickets.insert(&ticket.id)
        {
            return Err("Android native drain ticket absent or duplicated");
        }
        if ticket.kind.is_owner() && !owners.insert((ticket.kind, &ticket.logical_id)) {
            return Err("Android native drain owner identity duplicated");
        }
        match (ticket.kind, entry.state) {
            (
                Kind::Main | Kind::Login | Kind::Speedtest,
                State::CancelledBeforeBirth | State::ClosedExact,
            )
            | (Kind::CheckConfig, State::CancelledBeforeBirth)
            | (
                Kind::TargetlessStop | Kind::TargetlessReload,
                State::CancelledBeforeBirth | State::Completed,
            ) => {}
            (_, State::Reserved | State::BirthEntered) => {
                return Err("Android native drain operation or owner unsettled");
            }
            (_, State::Unknown | State::ValidationCleanupUnknown) => {
                return Err("Android native drain cleanup unknown");
            }
            _ => return Err("Android native drain terminal kind mismatch"),
        }
    }
    Ok(())
}

fn valid_id(value: &str, max_utf16_units: usize) -> bool {
    !value.is_empty()
        && value.encode_utf16().count() <= max_utf16_units
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
}

#[cfg(test)]
mod tests;
