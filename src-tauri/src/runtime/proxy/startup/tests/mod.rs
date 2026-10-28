mod hot_rule_content;

#[test]
fn management_auth_guard_checks_only_actual_generated_api_services() {
    use super::*;
    let mut config: SingBoxConfig = serde_json::from_value(
        serde_json::json!({"log":{"level":"info","timestamp":false},"inbounds":[],"outbounds":[]}),
    )
    .unwrap();
    assert!(require_management_api_auth(&config).is_ok());
    config.services = Some(vec![polaris_config_engine::singbox::ApiService {
        type_field: "api".into(),
        listen: "127.0.0.1".into(),
        listen_port: 9090,
        secret: None,
        dashboard: None,
    }]);
    assert!(require_management_api_auth(&config).is_err());
    config.services.as_mut().unwrap()[0].secret = Some("   ".into());
    assert!(require_management_api_auth(&config).is_err());
    config.services.as_mut().unwrap()[0].secret = Some("persisted-secret".into());
    assert!(require_management_api_auth(&config).is_ok());
}
