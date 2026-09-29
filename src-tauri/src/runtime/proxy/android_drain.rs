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
    pub process_nonce: String,
    pub fence_id: String,
    pub marker_proof: MarkerProof,
    pub sealed_revision: u64,
    pub revision: u64,
    pub captured: Vec<Entry>,
}

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
    if expected_process.is_empty() || receipt.process_nonce != expected_process {
        return Err("Android native drain process nonce mismatch");
    }
    if expected_fence.is_empty() || receipt.fence_id != expected_fence {
        return Err("Android native drain fence mismatch");
    }
    if receipt.marker_proof != MarkerProof::Absent {
        return Err("Android native drain cold marker was not proved absent");
    }
    if receipt.sealed_revision == 0 || receipt.revision < receipt.sealed_revision {
        return Err("Android native drain revision invalid");
    }
    let mut tickets = HashSet::new();
    let mut owners = HashSet::new();
    for entry in &receipt.captured {
        let ticket = &entry.ticket;
        if ticket.id.is_empty() || ticket.logical_id.is_empty() || !tickets.insert(&ticket.id) {
            return Err("Android native drain ticket absent or duplicated");
        }
        if ticket.kind.is_owner() && !owners.insert((ticket.kind, &ticket.logical_id)) {
            return Err("Android native drain owner identity duplicated");
        }
        match (ticket.kind.is_owner(), entry.state) {
            (true, State::CancelledBeforeBirth | State::ClosedExact)
            | (false, State::CancelledBeforeBirth | State::Completed) => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn receipt() -> Value {
        json!({"processNonce":"process-1","fenceId":"fence-1","markerProof":"Absent","sealedRevision":3,
            "revision":5,"captured":[{"ticket":{"id":"ticket-1","kind":"Main",
                "logicalId":"run-1"},"state":"ClosedExact"}]})
    }

    fn check(value: Value) -> Result<(), &'static str> {
        let parsed: AndroidDrainReceipt = serde_json::from_value(value).unwrap();
        verify(&parsed, "process-1", "fence-1")
    }

    #[test]
    fn exact_terminal_receipt_is_only_a_shape_proof() {
        assert!(check(receipt()).is_ok());
        assert!(verify(
            &serde_json::from_value(receipt()).unwrap(),
            "old-process",
            "fence-1"
        )
        .is_err());
        assert!(verify(
            &serde_json::from_value(receipt()).unwrap(),
            "process-1",
            "old-fence"
        )
        .is_err());
    }

    #[test]
    fn rejects_missing_revisions_duplicate_owner_and_ticket() {
        let mut value = receipt();
        value.as_object_mut().unwrap().remove("sealedRevision");
        assert!(serde_json::from_value::<AndroidDrainReceipt>(value).is_err());
        let mut value = receipt();
        value["sealedRevision"] = json!(7);
        assert!(check(value).is_err());
        let mut value = receipt();
        value["markerProof"] = json!("PresentOrUnknown");
        assert!(check(value).is_err());
        let mut value = receipt();
        let duplicate = value["captured"][0].clone();
        value["captured"].as_array_mut().unwrap().push(duplicate);
        assert!(check(value).is_err());
        let mut value = receipt();
        let mut duplicate = value["captured"][0].clone();
        duplicate["ticket"]["id"] = json!("ticket-2");
        value["captured"].as_array_mut().unwrap().push(duplicate);
        assert!(check(value).is_err());
    }

    #[test]
    fn unknown_unsettled_and_validation_cleanup_unknown_never_pass() {
        for state in [
            "Reserved",
            "BirthEntered",
            "Unknown",
            "ValidationCleanupUnknown",
            "Completed",
        ] {
            let mut value = receipt();
            value["captured"][0]["state"] = json!(state);
            assert!(check(value).is_err(), "state={state}");
        }
        let mut validation = receipt();
        validation["captured"][0]["ticket"]["kind"] = json!("CheckConfig");
        validation["captured"][0]["state"] = json!("ValidationCleanupUnknown");
        assert!(check(validation).is_err());
    }
}
