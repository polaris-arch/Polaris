use super::*;
use serde_json::json;
fn parse_nodes(nodes: serde_json::Value) -> ClashParseResult {
    let yaml = serde_yaml::to_value(nodes).unwrap();
    parse_clash_proxies(&yaml, "sub", "now", &mut new_uuid())
}
fn emit(node: &ServerConfig) -> serde_json::Value {
    serde_json::to_value(
        polaris_config_engine::builder::outbound::build_proxy_outbound(
            node,
            "fixture",
            &polaris_config_engine::singbox::DomainResolver::Tag("dns".into()),
            "x64",
            "linux",
        )
        .unwrap(),
    )
    .unwrap()
}
#[test]
fn https_alias_always_enables_emitted_tls() {
    for kind in ["http", "https"] {
        for tls in [None, Some(false), Some(true)] {
            let mut node = json!({"name":"fixture","type":kind,"server":"proxy.example","port":443,"username":"u","password":"pw"});
            if let Some(tls) = tls {
                node["tls"] = json!(tls);
            }
            let result = parse_nodes(json!([node]));
            assert_eq!(result.servers.len(), 1);
            assert_eq!(
                emit(&result.servers[0]).get("tls").is_some(),
                kind == "https" || tls == Some(true)
            );
        }
    }
}
#[test]
fn clash_dialer_proxy_resolves_and_rejects_invalid_graphs() {
    let exit =
        json!({"type":"http","name":"exit","server":"e.example","port":80,"dialer-proxy":"front"});
    let front = json!({"type":"socks5","name":"front","server":"f.example","port":1080});
    let result = parse_nodes(json!([exit.clone(), front.clone()]));
    assert_eq!(
        result.servers[0].detour.as_deref(),
        Some(result.servers[1].id.as_str())
    );
    for nodes in [
        json!([exit.clone()]),
        json!([exit.clone(), front.clone(), front]),
        json!([
            exit, {"type":"socks5","name":"front","server":"f","port":1080,"dialer-proxy":"exit"}
        ]),
    ] {
        let result = parse_nodes(nodes);
        assert!(result.servers.iter().all(|s| s.name != "exit"));
        assert!(result.failed > 0);
    }
}
#[test]
fn clash_preserves_all_supported_headers_and_method() {
    let mut node = json!({"type":"vless","name":"fixture","server":"e.example","port":443,"uuid":"11111111-1111-1111-1111-111111111111",
        "network":"ws","ws-opts":{"path":"/ws","headers":{"Host":"front.example","X-Auth":"token"}}});
    let result = parse_nodes(json!([node.clone()]));
    assert_eq!(
        emit(&result.servers[0])["transport"]["headers"]["X-Auth"],
        "token"
    );
    node["network"] = json!("http");
    node["http-opts"] = json!({"path":"/rpc","method":"POST","headers":{"X-Auth":["one","two"]}});
    let result = parse_nodes(json!([node]));
    let output = emit(&result.servers[0]);
    assert_eq!(output["transport"]["method"], "POST");
    assert_eq!(
        output["transport"]["headers"]["X-Auth"],
        json!(["one", "two"])
    );
}
