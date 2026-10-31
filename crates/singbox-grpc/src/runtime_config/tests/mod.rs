use super::*;
fn fixture() -> Vec<u8> {
    let source = polaris_source_probe::crate_source!("runtime_status_handoff.hex");
    let text = source.trim();
    text.as_bytes()
        .chunks_exact(2)
        .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn actual_go_status_wire_preserves_presence_repetition_boundaries_and_unknowns() {
    let wire = fixture();
    let raw = decode_raw_status(wire.clone()).unwrap();
    assert_eq!(raw.wire, wire);
    assert_eq!(raw.status.control_revision, u64::MAX);
    let active = raw.status.active.as_ref().unwrap();
    assert_eq!(active.incarnation, "incarnation-A");
    assert_eq!(active.applied_revision, u64::MAX);
    let op = raw.status.operation.as_ref().unwrap();
    assert_eq!(op.state, 4);
    assert_eq!(op.before.as_ref().unwrap().applied_revision, u64::MAX - 1);
    assert_eq!(op.impacts.len(), 2);
    assert_eq!(op.impacts[0].affected_session_classes, ["tcp", "udp"]);
    assert_eq!(op.impacts[1], daemon::RuntimeImpact::default());
    assert_eq!(op.errors.len(), 2);
    assert_eq!(op.errors[0].message, "诊断\nsafe");
    assert_eq!(op.errors[1], daemon::RuntimePlanError::default());
    assert!(
        op.durable && op.owner_reload_acknowledged && raw.status.durable_owner_reload_acknowledged
    );
    assert_eq!(op.desired_state_revision, "D:9");
    assert_eq!(raw.status.durable_desired_state_revision, "r:\n9");
    // Prost legally discards unknown fields during re-encoding. Proof input must
    // retain the actual received bytes instead, including an explicit default.
    let mut extended = wire.clone();
    extended.extend_from_slice(&[0xa0, 0x06, 0x01, 0x2a, 0x00]);
    let unknown = decode_raw_status(extended.clone()).unwrap();
    assert_eq!(unknown.wire, extended);
    assert_eq!(unknown.status.durable_applied_digest, ""); // later explicit default wins
    assert_ne!(prost::Message::encode_to_vec(&unknown.status), unknown.wire);
    let presence = decode_raw_status(vec![0x12, 0, 0x1a, 0]).unwrap();
    assert!(presence.status.active.is_some() && presence.status.operation.is_some());
    assert!(decode_raw_status(vec![0x1a, 0x7f]).is_err());
}
