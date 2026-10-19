//! Deterministic source fixtures for IP-01/03/04/09/T01. No core or network invocation.
use super::*;

fn emitted(server: &ServerConfig) -> Value {
    serde_json::to_value(build_proxy_outbound(server, "fixture", &DomainResolver::Tag("dns-bootstrap".into()), "x64", "linux").unwrap()).unwrap()
}
fn socks(tag: &str) -> Value {
    json!({"type":"socks", "tag":tag, "server":"front.example", "server_port":1080})
}
fn exit(detour: Value) -> Value {
    json!({"type":"vless", "tag":"exit", "server":"exit.example", "server_port":443,
        "uuid":"11111111-1111-1111-1111-111111111111", "detour":detour})
}

#[test]
fn import_chain_survives_generated_outbound_tags() {
    let r = parse_doc(&json!({"outbounds":[exit(json!("front")), socks("front")]}), ImportOrigin::RemoteSubscription);
    assert_eq!((r.servers.len(), r.failed), (2, 0));
    assert_eq!(by_name(&r, "exit").detour.as_deref(), Some(by_name(&r, "front").id.as_str()));
    let generated = generate_local_import(&r.servers);
    assert!(generated.invalid_nodes.is_empty());
    let config = serde_json::to_value(generated.config).unwrap();
    let outbounds = config["outbounds"].as_array().unwrap();
    let front = outbounds.iter().find(|v| v["type"] == "socks").unwrap();
    let exit = outbounds.iter().find(|v| v["type"] == "vless").unwrap();
    assert_eq!(exit["detour"], front["tag"]);
}

#[test]
fn import_chain_rejects_missing_ambiguous_cycles_and_bad_shapes() {
    for outbounds in [
        json!([exit(json!("missing"))]),
        json!([exit(json!("front")), socks("front"), socks("front")]),
        json!([exit(json!("exit"))]),
        json!([exit(json!("front")), {"type":"socks","tag":"front","server":"f","server_port":1080,"detour":"exit"}]),
        json!([exit(json!(false))]),
        json!([exit(json!(""))]),
        // An untagged fallback display name must never stand in for a rejected tagged source.
        json!([exit(json!("front.example:1080")), {"type":"socks","server":"front.example","server_port":1080}, {"type":"vless","tag":"front.example:1080"}]),
    ] {
        let r = parse_doc(&json!({"outbounds":outbounds}), ImportOrigin::LocalFile);
        assert!(r.servers.iter().all(|s| s.name != "exit"), "{:?}", r.servers);
        assert!(r.failed > 0);
        assert!(r.warnings.iter().any(|w| w.contains("IMPORT_DETOUR_INVALID")));
    }
}

#[test]
fn cross_array_chains_and_pin_rejection_are_fail_closed() {
    let mut doc = masque_tailcat_doc();
    doc["outbounds"].as_array_mut().unwrap().push(socks("front"));
    doc["endpoints"][0]["detour"] = json!("front");
    doc["outbounds"][0]["detour"] = json!("front");
    let r = parse_doc(&doc, ImportOrigin::LocalFile);
    let id = by_name(&r, "front").id.as_str();
    assert_eq!(by_name(&r, "MQ3").detour.as_deref(), Some(id));
    assert_eq!(by_name(&r, "TC-REGION").detour.as_deref(), Some(id));
    let generated = serde_json::to_value(generate_local_import(&r.servers).config).unwrap();
    let front = generated["outbounds"].as_array().unwrap().iter().find(|v| v["type"] == "socks").unwrap();
    let mq = generated["endpoints"].as_array().unwrap().iter().find(|v| v["server_port"] == 18443).unwrap();
    assert_eq!(mq["detour"], front["tag"]);
    let tc = generated["outbounds"].as_array().unwrap().iter().find(|v| v["derp_region"] == 900).unwrap();
    assert_eq!(tc["detour"], front["tag"]);
    assert_eq!(tc["http_client"]["detour"], front["tag"]);

    for pins in [json!(["not-a-pin"]), json!([PIN_HEX, "not-a-pin"])] {
        let mut doc = masque_tailcat_doc();
        doc["endpoints"][0]["tls"]["certificate_sha256"] = pins;
        let r = parse_doc(&doc, ImportOrigin::LocalFile);
        assert_eq!(r.failed, 1);
        assert!(r.servers.iter().all(|s| s.name != "MQ3"));
    }
    let r = parse_doc(&json!({"outbounds":[exit(json!("front")), {
        "type":"trojan", "tag":"front", "server":"f", "server_port":443, "password":"pw",
        "tls":{"enabled":true,"certificate_sha256":[PIN_HEX,"bad"]}
    }]}), ImportOrigin::RemoteSubscription);
    assert!(r.servers.is_empty());
    assert_eq!(r.failed, 2);
}

#[test]
fn unsupported_custom_and_endpoint_targets_are_rejected() {
    for field in ["outbounds", "endpoints"] {
        let doc = json!({field:[{"type":"future-protocol","tag":"custom","detour":"front"}]});
        let r = parse_doc(&doc, ImportOrigin::LocalFile);
        assert!(r.servers.is_empty());
        assert_eq!(r.failed, 1);
    }
    let mut doc = masque_tailcat_doc();
    doc["outbounds"].as_array_mut().unwrap().push(exit(json!("MQ3")));
    let r = parse_doc(&doc, ImportOrigin::LocalFile);
    assert!(r.servers.iter().all(|s| s.name != "exit"));
    assert_eq!(r.failed, 1);
    // Same external tag across arrays is ambiguous even when one would be unsupported.
    doc["outbounds"].as_array_mut().unwrap().push(socks("MQ3"));
    let r = parse_doc(&doc, ImportOrigin::LocalFile);
    assert!(r.servers.iter().all(|s| s.name != "exit"));
}

#[test]
fn supported_transport_fields_reach_generated_json() {
    let mut node = exit(json!("unused"));
    node.as_object_mut().unwrap().remove("detour");
    node["transport"] = json!({"type":"http","path":"/rpc","host":["front.example","other.example"],
        "method":"POST","headers":{"X-Auth":["one","two"],"X-Single":"token"}});
    let r = parse(json!({"outbounds":[node.clone()]}));
    let output = emitted(&r.servers[0]);
    assert_eq!(output["transport"]["host"], json!(["front.example","other.example"]));
    assert_eq!(output["transport"]["method"], "POST");
    assert_eq!(output["transport"]["headers"]["X-Auth"], json!(["one","two"]));
    assert_eq!(output["transport"]["headers"]["X-Single"], json!(["token"]));
    node["transport"] = json!({"type":"ws","path":"/ws","headers":{"Host":"front.example","X-Auth":["token"]},
        "max_early_data":2048,"early_data_header_name":"Sec-WebSocket-Protocol"});
    let r = parse(json!({"outbounds":[node.clone()]}));
    let output = emitted(&r.servers[0]);
    assert_eq!(output["transport"]["headers"]["X-Auth"], "token");
    assert_eq!(output["transport"]["max_early_data"], 2048);
    assert_eq!(output["transport"]["early_data_header_name"], "Sec-WebSocket-Protocol");
    node["transport"]["headers"]["X-Auth"] = json!(["one","two"]);
    let rejected = parse(json!({"outbounds":[node]}));
    assert_eq!(rejected.failed, 1);
    assert!(rejected.servers.is_empty(), "WS scalar model must not truncate arrays");
    let r = parse(json!({"outbounds":[{"type":"http","server":"proxy.example","server_port":8080,
        "path":"/connect","headers":{"X-Auth":["one","two"]}}]}));
    let output = emitted(&r.servers[0]);
    assert_eq!(output["path"], "/connect");
    assert_eq!(output["headers"]["X-Auth"], json!(["one","two"]));
    assert!(output.get("transport").is_none());
}

#[test]
fn hysteria2_optional_settings_are_emitted() {
    let r = parse(json!({"outbounds":[{"type":"hysteria2","server":"hy.example","server_port":443,"password":"pw",
        "up_mbps":100,"down_mbps":500,"network":"tcp","disable_chrome_parrot":true}]}));
    let output = emitted(&r.servers[0]);
    for (key, value) in [("up_mbps",json!(100)),("down_mbps",json!(500)),("network",json!("tcp")),("disable_chrome_parrot",json!(true))] {
        assert_eq!(output[key], value, "{key}");
    }
}

#[test]
fn wireguard_absent_and_zero_keepalive_remain_disabled() {
    for keepalive in [None, Some(json!(0)), Some(json!(25))] {
        let mut peer = json!({"address":"wg.example","port":51820,"public_key":"key","allowed_ips":["0.0.0.0/0"]});
        if let Some(value) = &keepalive { peer["persistent_keepalive_interval"] = value.clone(); }
        let r = parse_doc(&json!({"endpoints":[{"type":"wireguard","tag":"wg","private_key":"private","address":["10.0.0.2/32"],"peers":[peer]}]}), ImportOrigin::LocalFile);
        let ep = polaris_config_engine::builder::endpoints::build_wireguard_endpoint(&r.servers[0], "wg", None, "linux", None).unwrap();
        let output = serde_json::to_value(ep).unwrap();
        assert_eq!(output["peers"][0]["persistent_keepalive_interval"], keepalive.unwrap_or(json!(0)));
    }
}
