//! Export a real App-generated config for the native core Dial/Listen integration gate.
//! cargo run -p polaris-config-engine --example native-node-dns-fixture -- linux
use polaris_config_engine::builder::{generate_sing_box_config, GenerateConfigDeps};
use polaris_config_engine::user_config::dns_config::CustomDnsUpstream;
use polaris_config_engine::user_config::dns_config::DnsConfig;
use polaris_config_engine::user_config::server_config::{Protocol, ServerConfig};
use polaris_config_engine::user_config::UserConfig;
use std::collections::BTreeMap;

fn deps_default() -> GenerateConfigDeps {
    GenerateConfigDeps {
        platform: "linux".into(),
        arch: "x64".into(),
        node_dns_decoy_override: None,
        probe_direct_port: None,
        probe_proxy_port: None,
        debug_probe_mixed_udp: false,
        update_in_port: None,
        subscription_update_in_port: None,
        loopback_auth: None,
        probe_pool_ports: vec![],
        lan_resolver_for_dns: None,

        has_cronet: true,
        cronet_copy_failed: false,
        has_management_api: false,
        privacy_mode: false,
        log_level: polaris_config_engine::user_config::LogLevel::Info,
        disable_log_file: false,
        dashboard_serve_dir: None,
        tailscale_api_port: 15490,
        cache_path: "/fake/cache.db".into(),
        log_file_path: Some("/fake/singbox.log".into()),
        runtime_rules_dir: "/fake/runtime-rules".into(),
        rule_resources_path: "/fake/rule-resources".into(),
        custom_rules_dir: "/fake/custom-rules".into(),
        tailscale_state_dir_prefix: "/fake/ts".into(),
        // A-0a：tailnet rule-set 目录。夹具里这个目录**不存在** ⇒ 块 0c 的存在性检查
        // 恒假 ⇒ 走 inline 降级腿 ⇒ 产出与本字段出现之前逐字节相同（金样不动）。
        tailnet_rules_dir: "/fake/userData/tailnet-rules".into(),
        // 无运行期观测（本批生产侧同样恒空）。
        observed_tailnet_addresses: Default::default(),
        is_valid_srs_fn: |_| false,
        own_lan_cidrs: vec![],
        log: |_, _| {},
        on_degraded: || {},
        system_dns_takeover_active: false,
        netenv_dhcp_suppressed: false,
        network_canary_port: None,
    }
}

fn main() {
    let mut deps = deps_default();
    deps.platform = std::env::args().nth(1).unwrap_or_else(|| "linux".into());
    deps.disable_log_file = true;
    deps.log_file_path = None;
    let naive = std::env::args().nth(2).as_deref() == Some("naive");
    let config = UserConfig {
        servers: vec![ServerConfig {
            id: "node".into(),
            name: "Native test node".into(),
            protocol: if naive {
                Protocol::Naive
            } else {
                Protocol::Vless
            },
            address: "node.native.test".into(),
            port: 443,
            username: naive.then(|| "fixture-user".into()),
            password: naive.then(|| "fixture-password".into()),
            uuid: Some("00000000-0000-4000-8000-000000000001".into()),
            ..Default::default()
        }],
        selected_server_id: Some("node".into()),
        dns_config: Some(DnsConfig {
            node_resolver_pool: Some(vec![
                "ali".into(),
                "dnspod".into(),
                "udp-fixture".into(),
                "system".into(),
            ]),
            node_resolver_custom: Some(vec![CustomDnsUpstream {
                id: "udp-fixture".into(),
                spec: "udp://9.9.9.9:5353".into(),
            }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let generated = generate_sing_box_config(&config, &BTreeMap::new(), &deps)
        .expect("native fixture generation");
    println!("{}", serde_json::to_string_pretty(&generated).unwrap());
}
