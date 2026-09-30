use super::*;
use crate::runtime::proxy::NoNetworkDoh;
use crate::test_support::{crate_code, module_code, TestDir};

fn runtime() -> (Arc<DnsRaceRuntime>, Arc<LifecycleGate>) {
    let gate = Arc::new(LifecycleGate::default());
    let runtime = Arc::new(DnsRaceRuntime::new(
        Arc::clone(&gate),
        Arc::new(NoNetworkDoh),
    ));
    (runtime, gate)
}

fn config(dns: serde_json::Value) -> UserConfig {
    serde_json::from_value(serde_json::json!({ "servers": [], "dnsConfig": dns }))
        .expect("最小 UserConfig")
}

fn method_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let signature_start = source.find(signature).expect("方法签名必须存在");
    let body_start = source[signature_start..]
        .find('{')
        .map(|offset| signature_start + offset)
        .expect("方法体必须存在");
    let mut depth = 0usize;
    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[body_start..=body_start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("方法体大括号未闭合")
}

#[test]
fn projection_defaults_sets_and_clears_all_three_axes() {
    let (runtime, _) = runtime();
    assert_eq!(runtime.config_projection(), (0, Vec::new(), Vec::new()));

    runtime.set_projection(
        5353,
        vec!["1.1.1.1".into(), "8.8.8.8".into()],
        vec![443, 8443],
    );
    assert_eq!(
        runtime.config_projection(),
        (
            5353,
            vec!["1.1.1.1".into(), "8.8.8.8".into()],
            vec![443, 8443]
        )
    );

    assert!(runtime.clear_owned(None));
    assert_eq!(runtime.config_projection(), (0, Vec::new(), Vec::new()));
}

#[test]
fn decoy_override_replaces_builtin_and_empty_file_falls_back() {
    let directory = TestDir::new("polaris-dns-race-owner-test-");
    let builtin = DnsRaceRuntime::load_decoy_set(directory.path());
    assert!(builtin.contains(&[31, 13, 95, 169]));

    let resources = rule_resource_dir(directory.path());
    std::fs::create_dir_all(&resources).unwrap();
    let override_path = resources.join(DnsRaceRuntime::DECOY_OVERRIDE_FILE);
    std::fs::write(&override_path, "1.2.0.0/16\n").unwrap();
    let overridden = DnsRaceRuntime::load_decoy_set(directory.path());
    assert!(overridden.contains(&[1, 2, 3, 4]));
    assert!(!overridden.contains(&[31, 13, 95, 169]));

    std::fs::write(override_path, "# 只有注释\n").unwrap();
    assert!(DnsRaceRuntime::load_decoy_set(directory.path()).contains(&[31, 13, 95, 169]));
}

#[tokio::test]
async fn disabled_race_starts_no_sidecar() {
    let directory = TestDir::new("polaris-dns-race-owner-test-");
    let (runtime, gate) = runtime();
    runtime
        .start(
            &config(serde_json::json!({
                "resolveNodeDomainsAhead": false,
                "nodeResolverPool": ["ali", "dnspod", "system"]
            })),
            directory.path(),
            gate.generation(),
            Platform::Linux,
        )
        .await;
    assert_eq!(runtime.port(), 0);
    assert!(runtime.sidecar.lock().unwrap().is_none());
}

#[tokio::test]
async fn start_commits_sidecar_and_real_upstream_projection_then_replaces_it() {
    let directory = TestDir::new("polaris-dns-race-owner-test-");
    let (runtime, gate) = runtime();
    runtime
        .start(
            &config(serde_json::json!({
                "nodeResolverPool": ["ali", "my-doh"],
                "nodeResolverCustom": [{
                    "id": "my-doh",
                    "spec": "https://9.9.9.9:8443/dns-query"
                }]
            })),
            directory.path(),
            gate.generation(),
            Platform::Linux,
        )
        .await;
    let (port, ips, ports) = runtime.config_projection();
    assert!(port > 0);
    assert!(ips.contains(&"9.9.9.9".to_string()));
    assert!(ports.contains(&443));
    assert!(ports.contains(&8443));
    assert!(runtime.sidecar.lock().unwrap().is_some());

    runtime
        .start(
            &config(serde_json::json!({ "resolveNodeDomainsAhead": false })),
            directory.path(),
            gate.generation(),
            Platform::Linux,
        )
        .await;
    assert_eq!(runtime.config_projection(), (0, Vec::new(), Vec::new()));
    assert!(runtime.sidecar.lock().unwrap().is_none());
}

#[tokio::test]
async fn superseded_generation_cannot_clear_start_or_commit_over_takeover() {
    let directory = TestDir::new("polaris-dns-race-owner-test-");
    let (runtime, gate) = runtime();
    let enabled = serde_json::json!({ "nodeResolverPool": ["ali", "dnspod"] });
    let generation_a = gate.generation();
    let generation_b = gate.bump_generation();

    runtime
        .start(
            &config(enabled.clone()),
            directory.path(),
            generation_b,
            Platform::Linux,
        )
        .await;
    let takeover_port = runtime.port();
    assert!(takeover_port > 0);

    runtime
        .start(
            &config(enabled.clone()),
            directory.path(),
            generation_a,
            Platform::Linux,
        )
        .await;
    assert_eq!(runtime.port(), takeover_port);
    assert!(!runtime.clear_owned(Some(generation_a)));
    assert_eq!(runtime.port(), takeover_port);

    let user_config = config(enabled);
    let upstreams = plan_upstreams(user_config.dns_config.as_ref(), user_config.proxy_mode_type)
        .expect("竞速开启时必须有上游计划");
    let query = Arc::new(DefaultUpstreamQuery::new(Arc::clone(&runtime.doh)));
    let stale_server = NodeDnsRaceServer::start(
        upstreams,
        query,
        DEFAULT_RACE_BUDGET,
        None,
        Arc::new(DecoySet::builtin()),
    )
    .await
    .expect("绑定回环端口");
    assert_ne!(stale_server.port(), takeover_port);
    assert_eq!(
        runtime.commit(
            stale_server,
            vec!["1.1.1.1".into()],
            vec![443],
            generation_a
        ),
        0
    );
    assert_eq!(runtime.port(), takeover_port);

    assert!(runtime.clear_owned(Some(generation_b)));
    assert_eq!(runtime.port(), 0);
}

fn registered_dead_callback(runtime: &DnsRaceRuntime) -> Option<OnRaceServerDead> {
    runtime
        .sidecar
        .lock()
        .unwrap()
        .as_ref()
        .and_then(NodeDnsRaceServer::dead_callback)
}

#[tokio::test]
async fn registered_dead_callback_clears_its_generation_and_yields_to_takeover() {
    let directory = TestDir::new("polaris-dns-race-owner-test-");
    let (runtime, gate) = runtime();
    let enabled = serde_json::json!({ "nodeResolverPool": ["ali", "dnspod"] });
    let generation_a = gate.generation();

    runtime
        .start(
            &config(enabled.clone()),
            directory.path(),
            generation_a,
            Platform::Linux,
        )
        .await;
    let port_a = runtime.port();
    let callback_a =
        registered_dead_callback(&runtime).expect("生产 start 必须给 sidecar 注册死亡回调");

    let generation_b = gate.bump_generation();
    runtime
        .start(
            &config(enabled),
            directory.path(),
            generation_b,
            Platform::Linux,
        )
        .await;
    let port_b = runtime.port();
    assert!(port_b > 0);
    assert_ne!(port_a, port_b);
    callback_a(port_a);
    assert_eq!(runtime.port(), port_b, "旧世代回调必须让位");

    let callback_b = registered_dead_callback(&runtime).expect("B 腿回调");
    callback_b(port_b);
    assert_eq!(runtime.config_projection(), (0, Vec::new(), Vec::new()));
}

// B0 换锚：façade 半边（字段句柄 + 「旧类型不得回到门面」的负向断言）钉死**单文件**锚
// `crate_*("runtime/proxy.rs")`（本批取材面已由 `crate_source` 换成剥注释的 `crate_code`，
// 换的是面不是锚），判据本体就是「这两句只准出现在门面」；调用点半边（`generate_deps` 读投影）另立一条，见下方
// `startup_reads_dns_race_projection_via_the_owner`。
#[test]
fn source_guards_pin_proxy_mode_threading_weak_callback_and_log_order() {
    let source = crate_code("runtime/proxy/dns_race.rs");
    let facade = crate_code("runtime/proxy.rs");
    let start = method_body(&source, "pub(super) async fn start(");
    // 接线门：生产 `start` 必须把**本平台生效值**喂给 `plan_upstreams`。
    // 「机制有门、接线没门」在这一处的形态是：`ProxyModeType::effective_on` 自己有整张真值表的
    // 单测（config-engine 侧），而这里若退回读裸值，那张表一格都不会红 —— INV-1 在 Android 上
    // 静默失效，没有任何判据会说话。
    // 折叠空白后再比：判据要对 rustfmt 的换行免疫，只在**判据本身**改动时才红。
    let start_flat = start.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        start_flat.contains(
            "plan_upstreams( user_config.dns_config.as_ref(), \
             user_config.proxy_mode_type.effective_on(platform), )"
        ),
        "`DnsRaceRuntime::start` 必须把 `proxy_mode_type.effective_on(platform)` 喂给 \
         `plan_upstreams`（INV-1 的接线点）。实得方法体：\n{start}"
    );
    assert!(start.contains("Some(on_dead)"));

    let callback = method_body(&source, "fn dead_callback(");
    let upgraded = callback.find("weak.upgrade()").expect("Weak 升级");
    let cleared = callback
        .find("if runtime.clear_owned(Some(my_generation))")
        .expect("锁内世代清理");
    let downgrade = callback.find("log::error!").expect("降级 error");
    assert!(upgraded < downgrade);
    assert!(cleared < downgrade);
    assert!(facade.contains("dns_race: Arc<DnsRaceRuntime>"));
    // 钉的是**类型**、不是迁移前的字段名 `race_sidecar`：`sidecar: Mutex<Option<NodeDnsRaceServer>>`
    // 早已搬进 `dns_race.rs`（本文件 `source` 变量），旧字段名在 `proxy.rs` 里永远找不回来，钉它
    // 恒真、没有牙。守的应是「这个类型不得再回到 façade」——façade 已经只留 `Arc<DnsRaceRuntime>`
    // 这一个句柄，`Mutex<Option<NodeDnsRaceServer>>` 一旦重新出现在 `proxy.rs` 里，说明状态又被
    // 拖回门面，这条才应该转红。这条断言必须专指门面：`dns_race.rs` 自身的 `source` 变量今天就含
    // 这个真实类型定义（`sidecar: Mutex<Option<NodeDnsRaceServer>>`），换成 `module_source` 会让它
    // 立即恒真式转红（对着自己的真实定义否定），故不能随其余 35 条一起换宽锚。
    assert!(!facade.contains("Mutex<Option<NodeDnsRaceServer>>"));
}

/// 调用点半边：`self.dns_race.config_projection()` 今天在门面的 `generate_deps` 里，B1+ 搬进
/// `startup.rs` 后需要跟随 —— 用 `module_source` 使取材面随生产码搬迁自动跟随。
#[test]
fn startup_reads_dns_race_projection_via_the_owner() {
    let module = module_code("runtime/proxy");
    assert!(module.contains("self.dns_race.config_projection()"));
}
