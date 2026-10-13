use super::*;
#[test]
fn subscription_only_batch_cannot_remove_rows_or_selected_exit() {
    let mut config =
        serde_json::json!({"selectedServerId":"s", "servers":[{"id":"s","subscriptionId":"sub"}]});
    let before = config.clone();
    assert!(
        remove_batch_manual_nodes(&mut config, &std::collections::HashSet::from(["s"])).is_empty()
    );
    assert_eq!(config, before);
}
#[test]
fn mixed_batch_only_removes_manual_nodes() {
    let mut config = serde_json::json!({"selectedServerId":"s", "servers":[{"id":"s","subscriptionId":"sub"},{"id":"m"}]});
    let removed =
        remove_batch_manual_nodes(&mut config, &std::collections::HashSet::from(["s", "m"]));
    assert_eq!(removed, vec![serde_json::json!({"id":"m"})]);
    assert_eq!(
        config["servers"],
        serde_json::json!([{"id":"s","subscriptionId":"sub"}])
    );
    assert_eq!(config["selectedServerId"], "s");
}
