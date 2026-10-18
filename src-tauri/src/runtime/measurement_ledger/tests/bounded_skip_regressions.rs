use super::super::*;
#[test]
fn skip_only_rounds_are_bounded_and_deleted_nodes_are_pruned() {
    let ledger = MeasurementLedger::new();
    let skipped = (0..HARD_CAP * 2)
        .map(|i| (format!("node-{i}"), "unsupported"))
        .collect::<Vec<_>>();
    ledger.set_skipped(&[], &skipped);
    assert_eq!(ledger.lock().skipped.len(), HARD_CAP);
    ledger.evict(&EvictContext {
        nodes: &BTreeSet::new(),
        url_digest: "u",
        now_ms: 0,
        freshness_cap_ms: &|_, _| 0,
    });
    assert!(ledger.lock().skipped.is_empty());
}
