use super::*;

#[test]
fn global_utls_change_creates_runtime_debt_and_missing_restores_follow_projection() {
    let config = UserConfig::default();
    let applied = serde_json::to_value(config).unwrap();
    let mut disk = applied.clone();
    disk["utlsPolicy"] = "random".into();
    assert_eq!(non_node_runtime_debt(&applied, &disk), Some(true));
    disk["utlsPolicy"] = "follow".into();
    assert_eq!(non_node_runtime_debt(&applied, &disk), Some(false));
    disk["utlsPolicy"] = "default".into();
    assert_eq!(non_node_runtime_debt(&applied, &disk), Some(true));
    disk.as_object_mut().unwrap().remove("utlsPolicy");
    assert_eq!(non_node_runtime_debt(&applied, &disk), Some(false));
}
