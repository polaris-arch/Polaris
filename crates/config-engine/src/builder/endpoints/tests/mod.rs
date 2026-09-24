use super::*;
use crate::user_config::server_config::{Protocol, ServerConfig, WireGuardSettings};

#[test]
fn wg_endpoint_basic() {
    let mut s = ServerConfig {
        id: "w1".into(),
        name: "WG".into(),
        protocol: Protocol::Wireguard,
        address: "1.2.3.4".into(),
        port: 51820,
        ..Default::default()
    };
    s.wireguard_settings = Some(Box::new(WireGuardSettings {
        private_key: Some("priv".into()),
        peer_public_key: Some("pub".into()),
        local_address: vec!["10.0.0.2/32".into()],
        allow_internet: Some(true),
        ..Default::default()
    }));
    let dial = crate::builder::helpers::get_node_dial_domain_resolver("dns-bootstrap", false);
    let ep = build_wireguard_endpoint(&s, "tag-w1", Some(&dial), "linux", None).unwrap();
    assert_eq!(ep.type_field, "wireguard");
    assert_eq!(ep.mtu, Some(1408));
    assert_eq!(ep.peers.as_ref().unwrap()[0].address, "1.2.3.4");
    // allowInternet=on → allowed_ips 含 0/0。
    assert!(ep.peers.as_ref().unwrap()[0]
        .allowed_ips
        .contains(&"0.0.0.0/0".to_string()));
}

#[test]
fn wg_unroutable_errors() {
    let mut s = ServerConfig {
        id: "w1".into(),
        name: "WG".into(),
        protocol: Protocol::Wireguard,
        address: "1.2.3.4".into(),
        port: 51820,
        ..Default::default()
    };
    s.wireguard_settings = Some(Box::new(WireGuardSettings {
        private_key: Some("priv".into()),
        peer_public_key: Some("pub".into()),
        local_address: vec!["10.0.0.2/32".into()],
        allow_internet: Some(false), // 关外网
        allowed_ips: vec![],         // 无具体段
        ..Default::default()
    }));
    assert!(build_wireguard_endpoint(&s, "tag", None, "linux", None).is_err());
}

#[test]
fn ts_endpoint_exit_node_when_full_tunnel() {
    let mut s = ServerConfig {
        id: "t1".into(),
        name: "TS".into(),
        protocol: Protocol::Tailscale,
        ..Default::default()
    };
    s.tailscale_settings = Some(Box::new(
        crate::user_config::server_config::TailscaleSettings {
            exit_node: Some("exit-peer".into()),
            ..Default::default()
        },
    ));
    let ep = build_tailscale_endpoint(&s, "tag-t1", "/fake/ts/t1", "linux", None);
    // exit_node 设 → mesh_allows_internet=true → exit_node 下发。
    assert_eq!(ep.exit_node.as_deref(), Some("exit-peer"));
}

/// `taildrop_directory` **恒下发且恒绝对**（1.14.0-beta.15）。
///
/// 这条不是「多测一个字段」：金样快照里**一个 tailscale endpoint 都没有**，
/// 整套 golden/`sing-box check` 对拍对本字段的检出力恒为 0 —— 缺了这条断言，
/// 把它改回 `None`（= 回落到内核那个跟着 CWD 漂的相对默认值）不会红任何门。
#[test]
fn ts_endpoint_always_pins_taildrop_directory_under_state_dir() {
    let mut s = ServerConfig {
        id: "t1".into(),
        name: "TS".into(),
        protocol: Protocol::Tailscale,
        ..Default::default()
    };
    // 用户一个 tailscale 设置都没填的最小形态：本字段仍须下发。
    s.tailscale_settings = Some(Default::default());
    let ep = build_tailscale_endpoint(&s, "tag-t1", "/fake/ts/t1", "linux", None);
    let dir = ep
        .taildrop_directory
        .as_deref()
        .expect("taildrop_directory 必须下发，不得留给内核相对默认值");
    assert_eq!(dir, "/fake/ts/t1/Taildrop");
    // 绝对性是本字段存在的**唯一理由**：相对路径会被内核按核进程 CWD 解析。
    assert!(
        dir.starts_with('/') || dir.as_bytes().get(1) == Some(&b':'),
        "必须是绝对路径（unix `/…` 或 Windows `X:\\…`），实得 {dir}"
    );
    assert!(
        dir.starts_with("/fake/ts/t1"),
        "须落在该节点自己的 state_dir 之下，随节点一起清理，实得 {dir}"
    );
}

/// `listen_port`：填了才下发，`0` 与未填一律不下发。
///
/// 同 `taildrop_directory` 那条的理由 —— 金样里零个 tailscale endpoint，对拍抓不到这条接线；
/// 而 `Endpoint.listen_port` 是 WG 腿也在用的**共用字段**，接错了不会编译失败，只会静默不发。
#[test]
fn ts_endpoint_emits_listen_port_only_when_set_nonzero() {
    fn ep_with(port: Option<u16>) -> Endpoint {
        let mut s = ServerConfig {
            id: "t1".into(),
            name: "TS".into(),
            protocol: Protocol::Tailscale,
            ..Default::default()
        };
        s.tailscale_settings = Some(Box::new(
            crate::user_config::server_config::TailscaleSettings {
                listen_port: port,
                ..Default::default()
            },
        ));
        build_tailscale_endpoint(&s, "tag-t1", "/fake/ts/t1", "linux", None)
    }
    assert_eq!(ep_with(Some(41641)).listen_port, Some(41641));
    // 0 = 内核的「自动选端口」，等价未设 ⇒ 不写进配置。
    assert_eq!(ep_with(Some(0)).listen_port, None);
    assert_eq!(ep_with(None).listen_port, None);
}

/// WireGuard 腿**不得**下发 `taildrop_directory`（该键只属 tailscale endpoint）。
#[test]
fn wg_endpoint_never_sets_taildrop_directory() {
    let s = ServerConfig {
        id: "w1".into(),
        name: "WG".into(),
        protocol: Protocol::Wireguard,
        address: "1.2.3.4".into(),
        port: 51820,
        wireguard_settings: Some(Box::new(WireGuardSettings {
            private_key: Some("priv".into()),
            peer_public_key: Some("pub".into()),
            local_address: vec!["10.0.0.2/32".into()],
            ..Default::default()
        })),
        ..Default::default()
    };
    let ep = build_wireguard_endpoint(&s, "tag-w1", None, "linux", None).unwrap();
    assert_eq!(ep.taildrop_directory, None);
}

/// 落盘态直达生成器：`reverseMesh:true` 的 WARP 节点**不得**发出 `system:true` / 接口名。
///
/// 谓词单测（`endpoint_routes.rs`）只钉判据；这条钉的是**发射面** —— 判据与 `ep.system`
/// 之间的接线断了（比如有人把 `ep.system = Some(uses_system)` 改成 `Some(reverse_mesh)`），
/// 谓词测试照绿，而磁盘上的 WARP 节点照样 FATAL。
#[test]
fn warp_endpoint_policy_differs_from_plain_wireguard() {
    fn wg(address: &str) -> ServerConfig {
        ServerConfig {
            id: "w1".into(),
            name: "WARP".into(),
            protocol: Protocol::Wireguard,
            address: address.into(),
            port: 2408,
            wireguard_settings: Some(Box::new(WireGuardSettings {
                private_key: Some("priv".into()),
                peer_public_key: Some("pub".into()),
                local_address: vec!["172.16.0.2/32".into()],
                reverse_mesh: Some(true),
                ..Default::default()
            })),
            ..Default::default()
        }
    }

    // 非 darwin 才会下发接口名 → 用 linux 让「接口名也没漏出去」这条断言有意义。
    let warp = build_wireguard_endpoint(
        &wg("engage.cloudflareclient.com"),
        "tag",
        None,
        "linux",
        None,
    )
    .expect("WARP endpoint 应能构建");
    assert_eq!(warp.system, Some(false), "WARP 不得发 system:true");
    assert_eq!(warp.name, None, "WARP 不得占用内核接口名");
    assert_eq!(warp.mtu, Some(crate::warp::WARP_MTU));

    // 反向对照：同样的 reverseMesh:true，普通 WG 仍应发 system:true + 接口名。
    let plain = build_wireguard_endpoint(&wg("vpn.example.com"), "tag", None, "linux", None)
        .expect("普通 WG endpoint 应能构建");
    assert_eq!(plain.system, Some(true));
    assert_eq!(plain.name.as_deref(), Some(WG_SYSTEM_INTERFACE_NAME));
    assert_eq!(plain.mtu, Some(1408));

    // 用户显式设置始终优先于协议缺省值。
    let mut custom = wg("engage.cloudflareclient.com");
    let settings = custom.wireguard_settings.as_mut().unwrap();
    settings.mtu = Some(1360);
    settings.persistent_keepalive = Some(0);
    let custom = build_wireguard_endpoint(&custom, "tag", None, "linux", None)
        .expect("显式 MTU 的 WARP endpoint 应能构建");
    assert_eq!(custom.mtu, Some(1360));
    assert_eq!(
        custom.peers.unwrap()[0].persistent_keepalive_interval,
        Some(0),
        "显式 0 应关闭保活，不能被改写回 25 秒"
    );
}

// ── MASQUE 客户端 ─────────────────────────────────────────────────────────────

fn masque(
    settings: Option<crate::user_config::protocol_settings::MasqueClientSettings>,
) -> ServerConfig {
    ServerConfig {
        id: "m1".into(),
        name: "MQ".into(),
        protocol: Protocol::MasqueClient,
        address: "mq.example.com".into(),
        port: 443,
        masque_client_settings: settings.map(Box::new),
        ..Default::default()
    }
}

fn masque_bag(
    pairs: serde_json::Value,
) -> crate::user_config::protocol_settings::MasqueClientSettings {
    serde_json::from_value(pairs).expect("MASQUE 设置夹具无效")
}

fn no_log(_: crate::user_config::log_level::LogLevel, _: &str) {}

fn masque_ep(s: &ServerConfig) -> serde_json::Value {
    let ep = build_masque_endpoint(s, "MQ", None, None, no_log).expect("合法节点应能构造");
    serde_json::to_value(ep).expect("序列化")
}

/// 顶层字段映射：server/port/凭据取顶层；TLS 恒开、SNI 缺省回落节点地址、pin 转成内核要的 base64。
/// 缺设置结构也是完整节点（必需内容都在顶层）。
#[test]
fn masque_maps_top_level_fields_and_always_enables_tls() {
    use crate::user_config::protocol_settings::TlsSettings;
    const HEX: &str = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881";
    const B64: &str = "LXEWQrcmsEQBYnyp+6wy9chTD7GQPMTbAiWHF5IaSIE=";

    let bare = masque_ep(&masque(None));
    assert_eq!(bare["type"], "masque-client");
    assert_eq!(bare["server"], "mq.example.com");
    assert_eq!(bare["server_port"], 443);
    assert_eq!(
        bare["tls"]["enabled"], true,
        "TLS 必须恒开：v3 缺 TLS 整核失败"
    );
    assert_eq!(
        bare["tls"]["server_name"], "mq.example.com",
        "未填 SNI 时回落节点地址"
    );
    assert_eq!(bare["tls"]["insecure"], false);
    for k in ["username", "password", "version", "path", "detour"] {
        assert!(bare.get(k).is_none(), "未设置的 `{k}` 不应下发：{bare}");
    }

    let mut s = masque(None);
    s.username = Some("u".into());
    s.password = Some("p".into());
    s.tls_settings = Some(TlsSettings {
        server_name: Some("sni.example.com".into()),
        allow_insecure: Some(true),
        certificate_sha256: Some(HEX.into()),
        certificate_public_key_sha256: Some(B64.into()),
        alpn: Some(vec!["h2".into()]),
        ..Default::default()
    });
    let ep = masque_ep(&s);
    assert_eq!(ep["username"], "u");
    assert_eq!(ep["password"], "p");
    assert_eq!(ep["tls"]["server_name"], "sni.example.com");
    assert_eq!(ep["tls"]["insecure"], true);
    assert_eq!(ep["tls"]["certificate_sha256"], serde_json::json!([B64]));
    assert_eq!(
        ep["tls"]["certificate_public_key_sha256"],
        serde_json::json!([B64])
    );
    assert!(
        ep["tls"].get("alpn").is_none(),
        "ALPN 由 version 决定，用户手填的不下发"
    );
}

/// 透传袋里的 `system` / `name` / `advertise_routes` 一律发不出去；同一个袋里的无害键
/// `udp_timeout` 必须发出去（反向对照：证明合并确实发生过，不是整袋被丢）。
/// 袋里冒名顶替的生成侧键（server/tls/detour）同样被具名值压住。
#[test]
fn masque_strips_forbidden_keys_but_keeps_the_rest_of_the_bag() {
    let s = masque(Some(masque_bag(serde_json::json!({
        "system": true,
        "name": "mq0",
        "advertise_routes": ["10.0.0.0/8"],
        "udp_timeout": "5m",
        "server": "evil.example.com",
        "tls": {"enabled": false},
        "detour": "ghost",
    }))));
    let ep = masque_ep(&s);
    for k in ["system", "name", "advertise_routes", "detour"] {
        assert!(ep.get(k).is_none(), "`{k}` 从透传袋漏进了产物：{ep}");
    }
    assert_eq!(ep["udp_timeout"], "5m", "无害袋键必须原样下发");
    assert_eq!(ep["server"], "mq.example.com", "顶层地址是唯一真值");
    assert_eq!(ep["tls"]["enabled"], true);
}

/// 版本 × 键矩阵，逐格对着随包核实测表（缺省/3 两组都收；2 只收 H2；1 都不收）。
#[test]
fn masque_strips_keys_incompatible_with_the_http_version() {
    const H2: &[&str] = &[
        "idle_timeout",
        "keep_alive_period",
        "stream_receive_window",
        "connection_receive_window",
        "max_concurrent_streams",
    ];
    const QUIC: &[&str] = &["initial_packet_size", "disable_path_mtu_discovery"];
    let mut bag = serde_json::json!({
        "idle_timeout": "30s", "keep_alive_period": "10s", "stream_receive_window": 1048576,
        "connection_receive_window": 1048576, "max_concurrent_streams": 8,
        "initial_packet_size": 1280, "disable_path_mtu_discovery": true,
    });
    for (version, keep_h2, keep_quic) in [
        (None, true, true),
        (Some(3), true, true),
        (Some(2), true, false),
        (Some(1), false, false),
    ] {
        bag["version"] = serde_json::to_value(version).unwrap();
        let ep = masque_ep(&masque(Some(masque_bag(bag.clone()))));
        for k in H2 {
            assert_eq!(
                ep.get(*k).is_some(),
                keep_h2,
                "version={version:?} 下 H2 键 `{k}`"
            );
        }
        for k in QUIC {
            assert_eq!(
                ep.get(*k).is_some(),
                keep_quic,
                "version={version:?} 下 QUIC 键 `{k}`"
            );
        }
    }
}

/// 剥键要记 Warn（用户该知道自己的调优键没生效），不剥时不吵。
#[test]
fn masque_logs_a_warning_only_when_it_strips() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static WARNS: AtomicUsize = AtomicUsize::new(0);
    fn count(level: crate::user_config::log_level::LogLevel, msg: &str) {
        if level == crate::user_config::log_level::LogLevel::Warn
            && msg.contains("initial_packet_size")
        {
            WARNS.fetch_add(1, Ordering::SeqCst);
        }
    }
    let s = masque(Some(masque_bag(
        serde_json::json!({"version": 2, "initial_packet_size": 1280}),
    )));
    build_masque_endpoint(&s, "MQ", None, None, count).unwrap();
    assert_eq!(WARNS.load(Ordering::SeqCst), 1);
    let s = masque(Some(masque_bag(
        serde_json::json!({"initial_packet_size": 1280}),
    )));
    build_masque_endpoint(&s, "MQ", None, None, count).unwrap();
    assert_eq!(
        WARNS.load(Ordering::SeqCst),
        1,
        "缺省版本不剥键，不该记 Warn"
    );
}

/// fail-closed：path 非空且不以 `/` 开头、version 超出 0–3 ⇒ 内核整核失败 ⇒ 构造器拒绝并给 token。
/// 空 path 内核接受（等于缺省模板），不拒。
#[test]
fn masque_rejects_values_that_would_fail_the_whole_core() {
    let with = |bag: serde_json::Value| masque(Some(masque_bag(bag)));
    assert_eq!(
        build_masque_endpoint(
            &with(serde_json::json!({"path": "no-slash"})),
            "MQ",
            None,
            None,
            no_log
        )
        .unwrap_err(),
        INVALID_REASON_MASQUE_PATH
    );
    assert_eq!(
        build_masque_endpoint(
            &with(serde_json::json!({"path": " /x"})),
            "MQ",
            None,
            None,
            no_log
        )
        .unwrap_err(),
        INVALID_REASON_MASQUE_PATH,
        "前导空格实测同样被内核拒"
    );
    assert_eq!(
        build_masque_endpoint(
            &with(serde_json::json!({"version": 4})),
            "MQ",
            None,
            None,
            no_log
        )
        .unwrap_err(),
        INVALID_REASON_MASQUE_VERSION
    );
    for ok in [
        serde_json::json!({"path": ""}),
        serde_json::json!({"path": "/masque"}),
        serde_json::json!({"version": 0}),
    ] {
        assert!(
            build_masque_endpoint(&with(ok.clone()), "MQ", None, None, no_log).is_ok(),
            "{ok} 内核接受，不该被拒"
        );
    }
}

/// detour / domain_resolver 走 endpoint 具名字段（D2：MASQUE 接前置代理）。
#[test]
fn masque_carries_detour_and_domain_resolver() {
    let dial = crate::builder::helpers::get_node_dial_domain_resolver("dns-bootstrap", false);
    let ep =
        build_masque_endpoint(&masque(None), "MQ", Some(&dial), Some("SOCKS"), no_log).unwrap();
    assert_eq!(ep.detour.as_deref(), Some("SOCKS"));
    assert_eq!(ep.domain_resolver, Some(dial));
}
