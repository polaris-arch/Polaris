use super::*;
fn pair() -> (DirectoryMarker, DirectoryDeletionIntent) {
    let marker = DirectoryMarker {
        version: 1,
        server_id: "node".into(),
        incarnation: "a".repeat(32),
        directory_epoch: "e".repeat(32),
        root_device: 1,
        root_inode: 2,
        directory_device: 1,
        directory_inode: 3,
        phase: "Open".into(),
        delete_intent_nonce: String::new(),
    };
    let intent = DirectoryDeletionIntent {
        server_id: "node".into(),
        incarnation: marker.incarnation.clone(),
        directory_identity: marker.identity(),
        state_directory: "/state/tailscale/node".into(),
        delete_intent_nonce: "b".repeat(32),
    };
    (marker, intent)
}
#[test]
fn drained_history_selects_current_incarnation_without_old_epoch_authority() {
    use polaris_mesh::tailscale_state::directory_retirement::{
        DirectoryWriterState, TailscaleDirectoryInstance, TailscaleDirectoryNode,
    };
    let (marker, intent) = pair();
    let node = TailscaleDirectoryNode {
        tag: "ts".into(),
        state_directory: intent.state_directory.clone(),
        directory_identity: intent.directory_identity.clone(),
        incarnation: intent.incarnation.clone(),
        directory_epoch: marker.directory_epoch.clone(),
        writer_state: DirectoryWriterState::SealedDrained,
        census_version: 1,
        failure: String::new(),
    };
    let current = TailscaleDirectoryInstance {
        run_nonce: "1".repeat(64),
        config_digest: "2".repeat(64),
        terminal: DirectoryWriterState::SealedDrained,
        census_complete: true,
        nodes: vec![node],
    };
    let mut old = current.clone();
    old.run_nonce = "3".repeat(64);
    old.nodes[0].incarnation = "c".repeat(32);
    old.nodes[0].directory_epoch = "d".repeat(32);
    let epochs = terminal_epochs_for_intent(&[old.clone(), current.clone()], &intent).unwrap();
    assert_eq!(
        epochs,
        std::collections::BTreeSet::from([marker.directory_epoch])
    );
    assert!(terminal_epochs_for_intent(&[old.clone()], &intent).is_err());
    old.nodes[0].writer_state = DirectoryWriterState::Unknown;
    assert!(terminal_epochs_for_intent(&[old, current], &intent).is_err());
}
#[test]
fn old_intent_never_matches_recreated_directory() {
    let (mut marker, intent) = pair();
    assert!(marker.matches(&intent));
    marker.incarnation = "c".repeat(32);
    assert!(!marker.matches(&intent));
    marker.incarnation = intent.incarnation.clone();
    marker.directory_inode += 1;
    assert!(!marker.matches(&intent));
}
#[test]
fn retries_require_original_nonce() {
    let (mut marker, mut intent) = pair();
    marker.phase = "Retiring".into();
    marker.delete_intent_nonce = intent.delete_intent_nonce.clone();
    assert!(marker.matches(&intent));
    intent.delete_intent_nonce = "c".repeat(32);
    assert!(!marker.matches(&intent));
}
#[test]
fn unknown_phase_and_noncanonical_nonce_fail_closed() {
    let (mut marker, mut intent) = pair();
    marker.phase = "Unknown".into();
    assert!(!marker.matches(&intent));
    marker.phase = "Open".into();
    intent.delete_intent_nonce = "B".repeat(32);
    assert!(!marker.matches(&intent));
}
