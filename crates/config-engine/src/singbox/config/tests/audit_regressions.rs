use super::*;
#[test]
fn optional_windows_handshake_survives_typed_reserialization() {
    let plain = serde_json::json!({"log":{"level":"info","timestamp":true},"inbounds":[],"outbounds":[]});
    let config: SingBoxConfig = serde_json::from_value(plain.clone()).unwrap();
    assert!(serde_json::to_value(config).unwrap().get("polaris_windows_directory").is_none());
    let mut bound = plain;
    bound["polaris_windows_directory"] = serde_json::json!({"version":1,"root":"C:/state","run_nonce":"run","expected_source":"source"});
    let config: SingBoxConfig = serde_json::from_value(bound.clone()).unwrap();
    assert_eq!(serde_json::to_value(config).unwrap()["polaris_windows_directory"], bound["polaris_windows_directory"]);
}
