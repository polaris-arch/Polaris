use super::*;

#[test]
fn ip_cidr_reexport_is_strict() {
    // re-export 的 is_valid_ip_cidr 仍是 config-engine 的严格实现（sanitize/normalize 依赖此语义）。
    assert!(is_valid_ip_cidr("10.0.0.0/8"));
    assert!(!is_valid_ip_cidr("010.0.0.1")); // 前导零
    assert!(!is_valid_ip_cidr("10.0.0.0/40")); // 掩码超界
}

#[test]
fn normalize_tun_exclude() {
    assert_eq!(
        normalize_tun_exclude_cidr("10.0.0.0/8"),
        Some("10.0.0.0/8".into())
    );
    assert_eq!(
        normalize_tun_exclude_cidr("1.2.3.4"),
        Some("1.2.3.4/32".into())
    );
    assert_eq!(normalize_tun_exclude_cidr("::1"), Some("::1/128".into()));
    // 过宽（v4<8）
    assert_eq!(normalize_tun_exclude_cidr("0.0.0.0/0"), None);
    assert_eq!(normalize_tun_exclude_cidr("10.0.0.0/7"), None);
    // 非法
    assert_eq!(normalize_tun_exclude_cidr("not-a-cidr"), None);
    assert_eq!(normalize_tun_exclude_cidr(""), None);
}

#[test]
fn protocol_requirement_checks() {
    // trojan 缺 password → 不通过
    let bad =
        serde_json::json!({"id":"s","name":"S","protocol":"trojan","address":"1.2.3.4","port":443});
    assert!(!protocol_requirement_ok("trojan", &bad));
    let good = serde_json::json!({"id":"s","name":"S","protocol":"trojan","password":"pw","address":"1.2.3.4","port":443});
    assert!(protocol_requirement_ok("trojan", &good));
    // wireguard 缺 localAddress → 不通过
    let wg = serde_json::json!({"wireguardSettings":{"privateKey":"k","peerPublicKey":"p"}});
    assert!(!protocol_requirement_ok("wireguard", &wg));

    let hysteria =
        serde_json::json!({"hysteriaSettings":{"authStr":"a","upMbps":10,"downMbps":20}});
    assert!(protocol_requirement_ok("hysteria", &hysteria));
    assert!(protocol_requirement_ok("tor", &serde_json::json!({})));

    let oc = serde_json::json!({"openconnectSettings":{
        "server":"vpn.example.com:443","username":"u","password":"p","flavor":"anyconnect"
    }});
    assert!(protocol_requirement_ok("openconnect", &oc));
    let ovpn = serde_json::json!({"openvpnClientSettings":{
        "server":"vpn.example.com","server_port":1194,"username":"u","password":"p","tls":{}
    }});
    assert!(protocol_requirement_ok("openvpn-client", &ovpn));
    assert!(!protocol_requirement_ok(
        "openvpn-client",
        &serde_json::json!({"openvpnClientSettings":{"server":"x"}})
    ));
}

#[test]
fn validate_accepts_minimal_config() {
    let mut v = serde_json::json!({
        "proxyMode": "global",
        "proxyModeType": "systemProxy",
        "logLevel": "info",
        "mixedPort": 7890,
        "tunConfig": {"mtu": 1350, "autoRoute": true, "strictRoute": true}
    });
    assert!(validate_config(&mut v).is_ok());
}

#[test]
fn validate_rejects_bad_proxy_mode() {
    let mut v = serde_json::json!({
        "proxyMode": "invalid",
        "proxyModeType": "tun",
        "logLevel": "info",
        "tunConfig": {"mtu": 1350, "autoRoute": true, "strictRoute": true}
    });
    assert!(matches!(
        validate_config(&mut v),
        Err(crate::StoreError::Validation(_))
    ));
}

#[test]
fn control_port_defaults_to_9090_when_missing() {
    // 缺 controlPort → 填 9090（上游 `if (!controlPort) controlPort=9090`）。
    let mut v = serde_json::json!({
        "proxyMode": "global",
        "proxyModeType": "systemProxy",
        "logLevel": "info",
        "mixedPort": 7890,
        "tunConfig": {"mtu": 1350, "autoRoute": true, "strictRoute": true}
    });
    validate_config(&mut v).unwrap();
    assert_eq!(v["controlPort"], serde_json::json!(9090));
}

#[test]
fn control_port_collision_with_mixed_reassigns() {
    // controlPort == mixedPort（非 9090）→ 回退 9090（撞口自愈，否则 sing-box FATAL）。
    let mut v = serde_json::json!({
        "proxyMode": "global",
        "proxyModeType": "systemProxy",
        "logLevel": "info",
        "mixedPort": 7890,
        "controlPort": 7890,
        "tunConfig": {"mtu": 1350, "autoRoute": true, "strictRoute": true}
    });
    validate_config(&mut v).unwrap();
    assert_eq!(v["controlPort"], serde_json::json!(9090));
    assert_ne!(v["controlPort"], v["mixedPort"], "撞口必须被拆开");
}

#[test]
fn control_port_collision_at_9090_reassigns_to_9091() {
    // 两口都 9090 → controlPort 取 9091（上游 `mixedPort===9090 ? 9091 : 9090`）。
    let mut v = serde_json::json!({
        "proxyMode": "global",
        "proxyModeType": "systemProxy",
        "logLevel": "info",
        "mixedPort": 9090,
        "controlPort": 9090,
        "tunConfig": {"mtu": 1350, "autoRoute": true, "strictRoute": true}
    });
    validate_config(&mut v).unwrap();
    assert_eq!(v["controlPort"], serde_json::json!(9091));
    assert_ne!(v["controlPort"], v["mixedPort"]);
}

#[test]
fn control_port_non_colliding_preserved() {
    // 不撞口 → 原值保留（幂等，不乱改用户设的端口）。
    let mut v = serde_json::json!({
        "proxyMode": "global",
        "proxyModeType": "systemProxy",
        "logLevel": "info",
        "mixedPort": 8080,
        "controlPort": 9091,
        "tunConfig": {"mtu": 1350, "autoRoute": true, "strictRoute": true}
    });
    validate_config(&mut v).unwrap();
    assert_eq!(v["controlPort"], serde_json::json!(9091));
}

#[test]
fn validate_rejects_bad_mtu() {
    let mut v = serde_json::json!({
        "proxyMode": "direct",
        "proxyModeType": "manual",
        "logLevel": "info",
        "tunConfig": {"mtu": 100, "autoRoute": true, "strictRoute": true}
    });
    assert!(validate_config(&mut v).is_err());
}

/// 🔴 TUN stack 随上游弃用移除后，`validate_config` **不得**再拒收任何遗留 `stack` 值。
///
/// 这条卡的是 save 路径：`ConfigStore::save` / `canonicalize_for_save` 只跑 sanitize + validate、不跑迁移链，
/// 而备份导入正走这条（`backup_import_save_core` 先存、再 `load_full` 触发迁移）。若 validate 还拒收，
/// 一份带 `"stack":"system"`（旧版合法值）或手改坏的 `"stack":"bogus"` 的旧备份会整份导入失败。
/// 牙：恢复旧的 `TUN_STACK_VALUES` 校验 → 非法值与非字符串两格转红。
#[test]
fn validate_ignores_legacy_tun_stack_of_any_value() {
    for legacy in [
        serde_json::json!("auto"),
        serde_json::json!("system"),
        serde_json::json!("gvisor"),
        serde_json::json!("mixed"),
        serde_json::json!("go"),
        serde_json::json!("bogus"),
        serde_json::json!(""),
        serde_json::json!(42),
        serde_json::Value::Null,
    ] {
        let mut v = serde_json::json!({
            "proxyMode": "smart",
            "proxyModeType": "tun",
            "logLevel": "info",
            "mixedPort": 7890,
            "tunConfig": {"stack": legacy.clone(), "autoRoute": true, "strictRoute": true}
        });
        assert!(
            validate_config(&mut v).is_ok(),
            "遗留 tunConfig.stack={legacy} 不得导致校验失败"
        );
    }
}
