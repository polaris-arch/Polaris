use super::*;

#[test]
fn status_bridge_requires_and_preserves_callback_observation_time() {
    let json = r#"{"observedAtMs":1200,"memory":0,"goroutines":0,"connectionsIn":0,
        "connectionsOut":0,"trafficAvailable":true,"uplink":0,"downlink":0,
        "uplinkTotal":1048576,"downlinkTotal":0}"#;
    let decoded: BridgeStatus = serde_json::from_str(json).unwrap();
    let frame: super::super::StatusFrame = decoded.into();
    assert_eq!(frame.observed_at_ms, Some(1200));
    assert_eq!(frame.status.uplink_total, 1048576);
    let missing = json.replace("\"observedAtMs\":1200,", "");
    assert!(serde_json::from_str::<BridgeStatus>(&missing).is_err());
}
