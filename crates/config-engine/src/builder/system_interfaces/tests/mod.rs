use super::*;
use serde_json::json;

#[test]
fn system_requests_include_typed_extra_and_conflicting_fields() {
    for field in ["system", "system_interface"] {
        for (typed, extra) in [(true, false), (false, true), (true, true)] {
            let mut endpoint = Endpoint::default();
            if field == "system" {
                endpoint.system = Some(typed);
            } else {
                endpoint.system_interface = Some(typed);
            }
            endpoint.extra.insert(field.into(), json!(extra));
            assert!(endpoint_requests_system_interface(&endpoint));
        }
        let raw = json!({field: true, "type": "custom", "tag": "custom"});
        assert!(raw_endpoint_requests_system_interface(&raw));
        let decoded: Endpoint = serde_json::from_value(raw).unwrap();
        assert!(endpoint_requests_system_interface(&decoded));
    }
    let userspace: Endpoint = serde_json::from_value(json!({
        "type": "openconnect", "tag": "userspace", "system": false, "system_interface": false
    }))
    .unwrap();
    assert!(!endpoint_requests_system_interface(&userspace));
    assert!(!raw_endpoint_requests_system_interface(
        &json!({"system": "true"})
    ));
    assert!(ensure_managed_system_interfaces(&[userspace], false).is_ok());
}

#[test]
fn unmanaged_errors_identify_only_actual_system_endpoints() {
    let endpoints = [
        Endpoint {
            tag: "OC".into(),
            system: Some(true),
            ..Default::default()
        },
        Endpoint {
            tag: "OV".into(),
            extra: json!({"system":true}).as_object().unwrap().clone(),
            ..Default::default()
        },
        Endpoint {
            tag: "userspace".into(),
            system: Some(false),
            ..Default::default()
        },
    ];
    let error = ensure_managed_system_interfaces(&endpoints, false).unwrap_err();
    assert!(error.contains("[OC, OV]"));
    assert!(!error.contains("userspace"));
    assert!(ensure_managed_system_interfaces(&endpoints, true).is_ok());
}
