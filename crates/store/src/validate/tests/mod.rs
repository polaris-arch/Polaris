mod rule_identity_tests;
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
    // masque-client：地址走通用校验、Basic 可选、TLS 生成侧恒开 ⇒ 无协议级必填。
    assert!(protocol_requirement_ok(
        "masque-client",
        &serde_json::json!({"address":"mq.example.com","port":443})
    ));
    // tailcat：与生成侧同一个判据（坏 key / DERP 冲突下发即整核失败，落盘门不得更宽）。
    let tc = |settings: serde_json::Value| serde_json::json!({ "tailcatSettings": settings });
    let pk = "lPLDHP0YorENQouqgSUx1GHu+3OcDc/F71Z3roMTSy4=";
    assert!(protocol_requirement_ok(
        "tailcat",
        &tc(serde_json::json!({"serverPublicKey":pk,"serverDiscoKey":pk,"derpRegion":1}))
    ));
    for bad in [
        serde_json::json!({"serverPublicKey":pk,"derpRegion":1}),
        serde_json::json!({"serverPublicKey":pk,"serverDiscoKey":pk}),
        serde_json::json!({"serverPublicKey":pk,"serverDiscoKey":pk,"derpRegion":1,
            "derpServers":["d.example"]}),
        serde_json::json!({"serverPublicKey":pk,"serverDiscoKey":pk,"derpRegion":"1"}),
    ] {
        assert!(
            !protocol_requirement_ok("tailcat", &tc(bad.clone())),
            "{bad}"
        );
    }
    assert!(!protocol_requirement_ok("tailcat", &serde_json::json!({})));
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

// ── 网络场景写入校验（N2，spec §7「写入校验」）─────────────────────────────────

fn with_profiles(extra: serde_json::Value) -> serde_json::Value {
    let mut cfg = crate::store::default_config();
    for (k, v) in extra.as_object().expect("extra 必须是 object") {
        cfg[k] = v.clone();
    }
    cfg
}

/// 合法场景可写；CIDR / 搜索域逐值校验，任一坏值整次写入被拒并点名那个值。
/// 牙：删掉 `validate_for_save` 在 `canonicalize_for_save` 里的调用 → 三条 is_err 转红。
#[test]
fn save_rejects_bad_network_profile_values_and_accepts_good_ones() {
    let good = with_profiles(
        serde_json::json!({"networkProfiles": [{"id": "np", "name": "office",
        "enabled": true, "probe": "auto",
        "match": {"dnsServerCidrs": ["10.20.0.0/16", "192.168.10.1"], "searchDomains": ["Corp.Example."]}}]}),
    );
    let saved = crate::ConfigStore::canonicalize_for_save(&good).expect("合法场景必须可写");
    assert_eq!(
        saved["networkProfiles"][0]["match"]["searchDomains"],
        serde_json::json!(["corp.example"]),
        "写入前 sanitize 规范化搜索域"
    );
    for (field, bad) in [
        ("dnsServerCidrs", "10.20.0.0/40"),
        ("dnsServerCidrs", "not-an-ip"),
        ("searchDomains", "corp example"),
        ("searchDomains", "*.corp.example"),
    ] {
        let mut cfg = good.clone();
        cfg["networkProfiles"][0]["match"][field] = serde_json::json!([bad]);
        let err = crate::ConfigStore::canonicalize_for_save(&cfg)
            .expect_err(&format!("{field}={bad} 必须被拒"));
        assert!(format!("{err}").contains(bad), "错误信息应点名坏值：{err}");
    }
}

/// 保留 id `builtin-netenv-dhcp` 不得被用户 DNS 资源 / DNS 组 / 场景占用；load 路径不受影响（不能因此
/// 把整份配置回落默认）。牙：删掉保留 id 检查 → 三条 is_err 转红。
#[test]
fn save_rejects_reserved_netenv_id_but_load_validation_tolerates_it() {
    let squats = [
        (
            "dnsServers",
            serde_json::json!([{"id": "builtin-netenv-dhcp", "name": "x",
            "enabled": true, "type": "udp", "outbound": {"type": "direct"}}]),
        ),
        (
            "dnsServerGroups",
            serde_json::json!([{"id": "builtin-netenv-dhcp", "name": "g",
            "enabled": true, "mode": "race", "members": ["builtin-domestic"]}]),
        ),
        (
            "networkProfiles",
            serde_json::json!([{"id": "builtin-netenv-dhcp", "name": "p",
            "enabled": true, "probe": "auto", "match": {"dnsServerCidrs": ["10.0.0.0/8"]}}]),
        ),
    ];
    for (key, value) in squats {
        let mut cfg = crate::store::default_config();
        cfg[key] = value;
        let err = crate::ConfigStore::canonicalize_for_save(&cfg)
            .expect_err(&format!("{key} 占用保留 id 必须被拒"));
        assert!(format!("{err}").contains("builtin-netenv-dhcp"), "{err}");
        let mut loaded = cfg.clone();
        assert!(
            validate_config(&mut loaded).is_ok(),
            "load 也跑 validate_config：保留 id 冲突不得让整份配置回落默认（{key}）"
        );
    }
    let clean = crate::store::default_config();
    assert!(crate::ConfigStore::canonicalize_for_save(&clean).is_ok());
}

mod audit_regressions;

#[test]
fn canonical_save_rejects_removed_or_disabled_protected_dns_resources() {
    use polaris_config_engine::user_config::dns_policy::{
        builtin_dns_server_resources, BUILTIN_BOOTSTRAP_DNS_ID, BUILTIN_DOMESTIC_DNS_ID,
        BUILTIN_REMOTE_DNS_ID,
    };
    for id in [
        BUILTIN_DOMESTIC_DNS_ID,
        BUILTIN_REMOTE_DNS_ID,
        BUILTIN_BOOTSTRAP_DNS_ID,
    ] {
        for remove in [true, false] {
            let mut servers = serde_json::to_value(builtin_dns_server_resources()).unwrap();
            let list = servers.as_array_mut().unwrap();
            if remove {
                list.retain(|server| server["id"] != id);
            } else {
                list.iter_mut().find(|server| server["id"] == id).unwrap()["enabled"] =
                    false.into();
            }
            let mut raw = crate::default_config();
            raw["dnsServers"] = servers;
            let before = raw.clone();
            let error = crate::ConfigStore::canonicalize_for_save(&raw)
                .expect_err(&format!("{id}, remove={remove}"));
            assert_eq!(error.code(), Some("DNS_DEPENDENCY_INVALID"));
            assert_eq!(raw, before, "rejected save must not synthesize resources");
        }
    }
    // Absent collections keep legacy serde defaults; explicit valid resources work.
    let mut legacy = crate::default_config();
    legacy.as_object_mut().unwrap().remove("dnsServers");
    let before = legacy.clone();
    let canonical = crate::ConfigStore::canonicalize_for_save(&legacy)
        .expect("absent legacy resources share the dependency graph's typed defaults");
    assert_eq!(legacy, before, "validation must not mutate the caller");
    assert!(
        canonical.get("dnsServers").is_none(),
        "validation defaults must not leak into raw saves"
    );
    assert_eq!(canonical["dnsDefaults"], before["dnsDefaults"]);
    legacy["dnsServers"] = serde_json::to_value(builtin_dns_server_resources()).unwrap();
    assert!(crate::ConfigStore::canonicalize_for_save(&legacy).is_ok());
}

#[test]
fn absent_dns_resources_do_not_authorize_unknown_fallbacks_or_explicit_empty_lists() {
    for explicit_empty in [false, true] {
        for id in ["builtin-remote", "unknown-resolver"] {
            let mut raw = crate::default_config();
            if explicit_empty {
                raw["dnsServers"] = serde_json::json!([]);
            } else {
                raw.as_object_mut().unwrap().remove("dnsServers");
            }
            raw["dnsDefaults"]["realQueryFallbackAction"] =
                serde_json::json!({"type": "server", "serverId": id});
            let before = raw.clone();
            let result = crate::ConfigStore::canonicalize_for_save(&raw);
            assert_eq!(result.is_ok(), !explicit_empty && id == "builtin-remote");
            if let Err(error) = result {
                assert_eq!(error.code(), Some("DNS_DEPENDENCY_INVALID"));
            }
            assert_eq!(raw, before);
        }
    }
}

#[test]
fn malformed_explicit_dns_collections_cannot_become_legacy_defaults_on_save() {
    for collection in [
        serde_json::Value::Null,
        serde_json::json!({}),
        serde_json::json!("invalid"),
    ] {
        let mut raw = crate::default_config();
        raw["dnsServers"] = collection;
        let before = raw.clone();
        let error = crate::ConfigStore::canonicalize_for_save(&raw)
            .expect_err("an explicit malformed collection is not an absent legacy field");
        assert_eq!(error.code(), Some("DNS_DEPENDENCY_INVALID"));
        assert_eq!(raw, before);
        let path = std::path::Path::new("/data/config.json");
        let write_fs = crate::MockFs::default();
        assert!(crate::ConfigStore::save(&write_fs, path, &raw, "abcdef012345").is_err());
        assert!(
            write_fs.operations().is_empty(),
            "rejected save must not touch disk"
        );

        // Reading an existing legacy file keeps the established tolerant migration.
        let bytes = serde_json::to_string(&raw).unwrap();
        let read_fs = crate::MockFs::default().with(path, bytes.clone());
        let loaded = crate::ConfigStore::load(&read_fs, path);
        assert!(loaded.loaded_from_disk);
        assert!(loaded.error.is_none());
        assert_eq!(
            read_fs.snapshot(path),
            Some(bytes),
            "load must not rewrite the source"
        );
    }
}
