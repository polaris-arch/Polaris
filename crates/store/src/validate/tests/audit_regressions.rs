use super::*;
#[test]
fn malformed_lan_cidr_is_not_silently_persisted() {
    for cidr in ["999.1.1.1/24", "10.0.0.1/99", "2001:db8::/129"] {
        assert!(validate_for_save(&serde_json::json!({"bypassLANList":[cidr]})).is_err());
    }
    assert!(validate_for_save(&serde_json::json!({"bypassLANList":["*.example.com", "10.0.0.0/8", "2001:db8::/32"]})).is_ok());
}
