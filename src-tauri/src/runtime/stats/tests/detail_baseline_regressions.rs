use super::*;
#[tokio::test]
async fn detail_subscription_edge_survives_coalesced_watch_off_on() {
    let lifecycle = ConnectionStreamLifecycle::default();
    let epoch = lifecycle.start_task(|| {});
    let gate = Arc::new(StreamGateState::new());
    let mut notifications = gate.epoch.subscribe();
    let mut registry = polaris_stats_engine::SubscriptionRegistry::new();
    registry.subscribe(Topic::Topology, "main");
    let detail = registry.subscribe(Topic::Detail, "main");
    let was_open = registry.should_stream(Topic::Detail);
    registry.unsubscribe(Topic::Detail, detail);
    gate.bump();
    registry.subscribe(Topic::Detail, "main");
    lifecycle.request_detail_baseline();
    gate.bump();
    notifications.changed().await.unwrap();
    notifications.borrow_and_update();
    assert!(!notifications.has_changed().unwrap());
    let open = registry.should_stream(Topic::Detail);
    assert!(open && was_open && registry.should_stream_connections());
    let baseline_due = lifecycle.consume_detail_baseline(epoch).unwrap();
    assert!(
        baseline_due,
        "off/on edge must survive unchanged demand level"
    );
    assert!(open != was_open || (open && baseline_due));
    assert_eq!(lifecycle.consume_detail_baseline(epoch), Some(false));
    lifecycle.request_detail_baseline();
    lifecycle.retire(epoch, || {});
    let next = lifecycle.start_task(|| {});
    assert_eq!(lifecycle.consume_detail_baseline(epoch), None);
    assert_eq!(lifecycle.consume_detail_baseline(next), Some(true));
}

#[test]
fn detail_edge_is_requested_only_after_subscription_admission_and_before_wake() {
    let source = module_code("runtime/stats");
    let subscribe = method_body(&source, "    pub fn subscribe(");
    let admit = subscribe
        .find("self.register_subscription(window_label, topic)")
        .unwrap();
    let request = subscribe
        .find("self.connection_lifecycle.request_detail_baseline();")
        .unwrap();
    let wake = subscribe.find("self.gate.bump();").unwrap();
    assert!(admit < request && request < wake);
    assert!(subscribe.contains("if topic == Topic::Detail {"));
    assert!(
        source.contains("detail_open != detail_was_open || (detail_open && detail_baseline_due)")
    );
}
