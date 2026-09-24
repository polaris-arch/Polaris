//! 🔴 **随包核真的收得下完整生成配置吗**。
//!
//! 这是出站面门的补集：不剥 `inbounds` / `route` / `dns.rules` / `experimental`，用仓内真实
//! `resources/data/*.srs` 逐份喂给 `sing-box check`。它不替代运行时的
//! `core-supervisor::config_gate`：后者面向用户真实配置、起核时可 fail-open/剥节点；本文件是
//! 固定 37 份生成 fixture 的 fail-closed 回归门。
//!
//! `check` 只 decode + initialize，绝不 Start：不绑端口、不启动 TUN、不联网。因此 selector 的
//! Start-only missing tag 仍是已知边界，继续由运行时配置闸门与引用修剪链负责。

mod support;

use std::collections::BTreeMap;
use std::path::Path;

use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::user_config::proxy_mode::ProxyModeType;
use serde_json::{json, Value};
use support::core_locator::{
    bundled_core_candidates_for, command_for_core_target, kernel_gate_target, target_needs_rosetta,
};
use support::kernel_gate::{
    check, core_or_skip, full_config_deps, load_cases, FixtureRatchet, SnapshotCase,
};
use tempfile::tempdir;

const OUTBOUNDS_GATE: &str = "cargo test -p polaris-config-engine --test kernel_accepts_outbounds";
const FULL_CONFIG_GATE: &str =
    "cargo test -p polaris-config-engine --test kernel_accepts_full_config";

fn workflow(name: &str) -> String {
    let path = support::kernel_gate::repo_root().join(format!(".github/workflows/{name}.yml"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {}: {e}", path.display()))
}

fn assert_hardened_full_gate_after_outbounds(raw: &str, workflow_name: &str) {
    let outbounds = raw
        .find(OUTBOUNDS_GATE)
        .unwrap_or_else(|| panic!("{workflow_name} 缺出站真核门"));
    let full = raw
        .find(FULL_CONFIG_GATE)
        .unwrap_or_else(|| panic!("{workflow_name} 缺完整配置真核门"));
    assert!(
        outbounds < full,
        "{workflow_name} 的完整配置门必须排在出站真核门之后"
    );
    let step_start = raw[..full]
        .rfind("\n      - name:")
        .unwrap_or_else(|| panic!("{workflow_name} 完整配置门前缺 workflow step 边界"));
    let after_step = &raw[step_start..];
    let step_end = after_step[1..]
        .find("\n      - name:")
        .map_or(after_step.len(), |offset| offset + 1);
    let step = &after_step[..step_end];
    assert!(
        step.contains("POLARIS_REQUIRE_KERNEL_GATE: '1'"),
        "{workflow_name} 的完整配置门必须硬化：缺随包核不得静默跳过"
    );
}

/// Production management API requires exactly one API service in every full configuration.
fn assert_api_service_shape(case: &SnapshotCase, value: &Value) -> (bool, bool) {
    let services = value["services"]
        .as_array()
        .unwrap_or_else(|| panic!("{} 未生成 services；完整门被静默削面", case.name));
    assert_eq!(
        services.len(),
        1,
        "{} 必须恰有一个 service，实际 services：{services:?}",
        case.name
    );
    let api: Vec<&Value> = services
        .iter()
        .filter(|service| service["type"].as_str() == Some("api"))
        .collect();
    assert_eq!(
        api.len(),
        1,
        "{} 必须恰有一个 api service，实际 services：{services:?}",
        case.name
    );
    assert_eq!(
        api[0]["listen_port"].as_u64(),
        Some(19090),
        "{} 的 api service 必须下发固定测试端口 19090",
        case.name
    );
    let expected_dashboard = case.input.singbox_dashboard == Some(true);
    let actual_dashboard = api[0].get("dashboard").is_some_and(Value::is_object);
    assert_eq!(
        actual_dashboard, expected_dashboard,
        "{} 的 dashboard presence 未跟随 singboxDashboard 真值",
        case.name
    );
    (expected_dashboard, actual_dashboard)
}

/// CI 每次只在当前宿主执行一支，故用纯函数锁全部打包目标、宿主 fallback 与 x64 Rosetta wrapper。
#[test]
fn bundled_core_target_resolution_contract() {
    assert_eq!(
        bundled_core_candidates_for("linux"),
        &["resources/linux/sing-box"]
    );
    assert_eq!(
        bundled_core_candidates_for("windows"),
        &["resources/win/sing-box.exe"]
    );
    assert_eq!(
        bundled_core_candidates_for("macos-arm64"),
        &["resources/mac-arm64/sing-box"]
    );
    assert_eq!(
        bundled_core_candidates_for("macos-x64"),
        &["resources/mac-x64/sing-box"]
    );
    assert!(matches!(
        kernel_gate_target().as_str(),
        "linux" | "windows" | "macos-arm64" | "macos-x64"
    ));
    assert!(!target_needs_rosetta("linux"));
    assert!(!target_needs_rosetta("windows"));
    assert!(!target_needs_rosetta("macos-arm64"));
    assert!(target_needs_rosetta("macos-x64"));
    let command = command_for_core_target("macos-x64", Path::new("resources/mac-x64/sing-box"));
    assert_eq!(command.get_program(), "/usr/bin/arch");
    assert_eq!(
        command.get_args().collect::<Vec<_>>(),
        ["-x86_64", "resources/mac-x64/sing-box"]
    );
}

/// 打包腿才有刚下载的核与 cronet；CI 常规腿不拉核，在那里挂 REQUIRE=1 是永久假红。
#[test]
fn package_workflow_runs_full_gate_after_cronet_and_outbounds_gate() {
    let raw = workflow("package");
    assert!(
        raw.contains("POLARIS_KERNEL_GATE_TARGET: ${{ matrix.label }}"),
        "package 的真核门必须接收 matrix.label；macos-x64 不能按 arm64 宿主误选核"
    );
    let cronet_linux = raw
        .find("run: node scripts/fetch-cronet.mjs --platform=linux")
        .expect("package.yml 缺 Linux Cronet SO 拉取；Linux 真核门不能初始化 naive");
    let cronet_windows = raw
        .find("run: node scripts/fetch-cronet.mjs --platform=win")
        .expect("package.yml 缺 Windows Cronet DLL 拉取；Windows 真核门不能初始化 naive");
    let outbounds = raw.find(OUTBOUNDS_GATE).expect("package.yml 缺出站真核门");
    let full = raw
        .find(FULL_CONFIG_GATE)
        .expect("package.yml 缺完整配置真核门");
    assert!(
        cronet_linux < outbounds && cronet_windows < outbounds,
        "Linux SO 与 Windows DLL 的拉取都必须早于第一道出站真核门"
    );
    assert!(outbounds < full, "完整配置门必须位于出站真核门之后");
    assert!(
        raw.contains(
            "- name: Fetch cronet library (Linux, SHA256-pinned)\n        if: runner.os == 'Linux'\n        run: node scripts/fetch-cronet.mjs --platform=linux"
        ),
        "package.yml 的 Linux 腿必须只拉 libcronet.so"
    );
    assert!(
        raw.contains(
            "- name: Fetch cronet library (Windows, SHA256-pinned)\n        if: runner.os == 'Windows'\n        run: node scripts/fetch-cronet.mjs --platform=win"
        ),
        "package.yml 的 Windows 腿必须只拉 libcronet.dll"
    );
    assert_eq!(
        raw.matches("run: node scripts/fetch-cronet.mjs").count(),
        2,
        "package.yml 只能有 Linux/Windows 两个动态 Cronet 拉取步骤；macOS 的 cronet 静态编入核心"
    );
    assert_hardened_full_gate_after_outbounds(&raw, "package.yml");
}

/// release-risk 禁用 package.yml 的 `POLARIS_RUN_KERNEL_GATES`，故自身的 mandatory 步必须显式执行完整门。
#[test]
fn release_risk_runs_hardened_full_gate_after_cronet_and_outbounds_gate() {
    let raw = workflow("release-risk");
    let cronet = raw
        .find("run: node scripts/fetch-cronet.mjs --platform=linux")
        .expect("release-risk.yml 缺 Linux Cronet SO 拉取；Ubuntu 真核门不能初始化 naive");
    let full = raw
        .find(FULL_CONFIG_GATE)
        .expect("release-risk.yml 缺完整配置真核门");
    assert!(
        cronet < full,
        "release-risk 的完整配置门必须位于 Linux Fetch cronet 之后"
    );
    assert_eq!(
        raw.matches("run: node scripts/fetch-cronet.mjs").count(),
        1,
        "release-risk 的 Ubuntu 预检只应拉 Linux Cronet SO"
    );
    assert!(
        raw.contains(
            "- name: Fetch cronet library (Linux)\n        if: needs.classify.outputs.kernel == 'true'\n        run: node scripts/fetch-cronet.mjs --platform=linux"
        ),
        "release-risk 的 Ubuntu 预检必须显式只拉 Linux Cronet SO"
    );
    assert!(
        raw.contains("Run mandatory bundled-core gates")
            && raw.contains("if: needs.classify.outputs.kernel == 'true'"),
        "release-risk 的完整配置门必须留在 kernel 影响触发的 mandatory job"
    );
    assert_hardened_full_gate_after_outbounds(&raw, "release-risk.yml");
}

/// 全量 37 个 fixture 必须逐一生成、使用真实 `.srs` 资源并由同一凭据棘轮判定。
#[test]
fn bundled_core_accepts_every_full_generated_config() {
    let Some(core) = core_or_skip("完整生成配置加载门") else {
        return;
    };
    let temp = tempdir().expect("建 TempDir");
    let mut ratchet = FixtureRatchet::default();
    let mut tun_cases = 0usize;
    let mut expected_dashboards = 0usize;
    let mut actual_dashboards = 0usize;

    for case in load_cases() {
        let deps = full_config_deps(&case, &temp);
        let cfg = generate_sing_box_config(&case.input, &BTreeMap::new(), &deps)
            .unwrap_or_else(|e| panic!("{} 生成完整配置失败: {e}", case.name));
        let value = serde_json::to_value(&cfg).expect("完整配置序列化");
        let (expected_dashboard, actual_dashboard) = assert_api_service_shape(&case, &value);
        expected_dashboards += usize::from(expected_dashboard);
        actual_dashboards += usize::from(actual_dashboard);
        let path = temp.path().join(format!("full-{}.json", ratchet.checked));
        std::fs::write(&path, serde_json::to_vec_pretty(&value).expect("JSON 编码"))
            .expect("写完整配置到 TempDir");
        let (ok, diag) = check(&core, &path);
        if case.input.proxy_mode_type == ProxyModeType::Tun {
            tun_cases += 1;
            assert!(
                ok,
                "TUN case 不得豁免或落入凭据 artifact：{} → {diag}",
                case.name
            );
        }
        ratchet.record(&case.name, ok, &diag);
    }

    assert_eq!(
        tun_cases, 8,
        "fixture 的 TUN 覆盖缩水，完整配置门失去权限无关的 TUN 初始化面"
    );
    assert_eq!(
        actual_dashboards, expected_dashboards,
        "37 个 fixture 的 dashboard 形状累计失配"
    );
    ratchet.assert_exact("完整生成配置加载门");
}

/// fixture 未必开启 dashboard；两条显式真核样例锁住 services 的 dashboard None/local-path 分支。
#[test]
fn bundled_core_accepts_dashboard_services_with_none_and_local_serve_dir() {
    let Some(core) = core_or_skip("dashboard services 真核门") else {
        return;
    };
    let temp = tempdir().expect("建 TempDir");
    let base = load_cases()
        .into_iter()
        .find(|case| case.name == "systemProxy+smart+vless（基线）")
        .expect("基线 fixture 不得丢失");
    let local_dir = temp.path().join("dashboard-static");
    std::fs::create_dir_all(&local_dir).expect("建 dashboard 临时目录");
    let variants = [
        ("none", None),
        ("local", Some(local_dir.display().to_string())),
    ];
    let mut expected_dashboards = 0usize;
    let mut actual_dashboards = 0usize;

    for (name, serve_dir) in variants {
        let mut case = SnapshotCase {
            name: format!("dashboard services ({name})"),
            platform: base.platform.clone(),
            input: base.input.clone(),
        };
        case.input.singbox_dashboard = Some(true);
        let mut deps = full_config_deps(&case, &temp);
        deps.dashboard_serve_dir = serve_dir.clone();
        let cfg = generate_sing_box_config(&case.input, &BTreeMap::new(), &deps)
            .unwrap_or_else(|error| panic!("{name} dashboard 配置生成失败: {error}"));
        let value = serde_json::to_value(&cfg).expect("dashboard 配置序列化");
        let (expected_dashboard, actual_dashboard) = assert_api_service_shape(&case, &value);
        expected_dashboards += usize::from(expected_dashboard);
        actual_dashboards += usize::from(actual_dashboard);

        let api = value["services"]
            .as_array()
            .expect("services 已由形状断言验证")[0]
            .clone();
        let dashboard = &api["dashboard"];
        assert_eq!(dashboard["enabled"].as_bool(), Some(true));
        assert_eq!(
            dashboard.get("path").and_then(Value::as_str),
            serve_dir.as_deref(),
            "{name} dashboard path 未按 deps 下发"
        );
        let path = temp.path().join(format!("dashboard-{name}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(&value).expect("JSON 编码"))
            .expect("写 dashboard 配置");
        let (ok, diag) = check(&core, &path);
        assert!(ok, "{name} dashboard 配置被真核拒绝：{diag}");
    }

    assert_eq!(
        expected_dashboards, 2,
        "两条 dashboard 真核样例都必须请求 dashboard"
    );
    assert_eq!(
        actual_dashboards, expected_dashboards,
        "dashboard shape 累计失配"
    );
}

/// 正向变异：未知 inbound 键必须被完整配置门打红。
///
/// 用内存 `Value` 做 mutation，最后逐值恢复，绝不改 fixture；所有落盘仍在 `TempDir`。
#[test]
fn full_config_gate_rejects_unknown_inbound_field_mutation() {
    let Some(core) = core_or_skip("完整配置未知 inbound 字段变异") else {
        return;
    };
    let temp = tempdir().expect("建 TempDir");
    let case = load_cases()
        .into_iter()
        .next()
        .expect("37 个 fixture 不得为空");
    let deps = full_config_deps(&case, &temp);
    let cfg = generate_sing_box_config(&case.input, &BTreeMap::new(), &deps).expect("生成配置");
    let mut value = serde_json::to_value(&cfg).expect("序列化");
    let original = value.clone();
    value["inbounds"][0]["kernel_gate_unknown_inbound_field"] = json!(true);
    let path = temp.path().join("unknown-inbound.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&value).expect("JSON 编码"))
        .expect("写变异配置");
    let (ok, diag) = check(&core, &path);
    assert!(!ok, "未知 inbound 字段被核收下，本完整配置门没有牙");
    assert!(
        diag.contains("kernel_gate_unknown_inbound_field"),
        "变异没有命中目标字段，拒绝原因不可信：{diag}"
    );
    value = original;
    assert_eq!(
        value,
        serde_json::to_value(&cfg).expect("重序列化"),
        "变异未精确还原"
    );
}

/// 正向变异：真实已引用 `.srs` 路径失踪时，完整配置门必须转红。
#[test]
fn full_config_gate_rejects_missing_srs_path_mutation() {
    let Some(core) = core_or_skip("完整配置缺失 .srs 路径变异") else {
        return;
    };
    let temp = tempdir().expect("建 TempDir");
    let case = load_cases()
        .into_iter()
        .find(|case| case.name == "systemProxy+smart+vless（基线）")
        .expect("基线 fixture 不得丢失");
    let deps = full_config_deps(&case, &temp);
    let cfg = generate_sing_box_config(&case.input, &BTreeMap::new(), &deps).expect("生成配置");
    let mut value = serde_json::to_value(&cfg).expect("序列化");
    let original = value.clone();
    let rules = value["route"]["rule_set"]
        .as_array_mut()
        .expect("完整基线必须带真实 route.rule_set");
    let target = rules
        .iter_mut()
        .find(|rule| {
            rule["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(".srs"))
        })
        .expect("完整基线必须引用真实 .srs，缺此项则 mutation 没有射程");
    target["path"] = Value::String(temp.path().join("missing.srs").display().to_string());
    let path = temp.path().join("missing-srs.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&value).expect("JSON 编码"))
        .expect("写变异配置");
    let (ok, diag) = check(&core, &path);
    assert!(!ok, "缺失的 .srs 路径被核收下，本完整配置门没有牙");
    assert!(
        diag.contains("missing.srs"),
        "变异没有因缺失 .srs 转红，拒绝原因不可信：{diag}"
    );
    value = original;
    assert_eq!(
        value,
        serde_json::to_value(&cfg).expect("重序列化"),
        "变异未精确还原"
    );
}

/// Tailcat servers 模式的 DERP 字面 IP 进 TUN `route_exclude_address`（非 Linux）后，完整配置仍被真核接受。
///
/// 出站面门（`kernel_accepts_outbounds::bundled_core_accepts_tailcat_outbound`）剥掉了 inbounds，
/// 看不到 TUN 排除表；这里在 darwin / win32 TUN 基线上换成 Tailcat 出口，整份配置喂 check。
#[test]
fn bundled_core_accepts_tailcat_derp_ips_in_tun_route_exclude() {
    let temp = tempdir().expect("建 TempDir");
    let tailcat: polaris_config_engine::user_config::server_config::ServerConfig =
        serde_json::from_value(json!({
            "id": "tc", "name": "TC-SERVERS", "protocol": "tailcat",
            "tailcatSettings": {
                "serverPublicKey": "lPLDHP0YorENQouqgSUx1GHu+3OcDc/F71Z3roMTSy4=",
                "serverDiscoKey": "qQ+kiWwZ8BrTYDZpj+6bnx2JxWxx0SAh1krqPGndCmQ=",
                "derpServers": ["derp1.example",
                    { "host": "derp2.example", "ipv4": "192.0.2.10", "ipv6": "2001:db8::10" },
                    { "host": "derp3.example", "ipv4": "none" }, { "host": "192.0.2.20" }]
            }
        }))
        .expect("Tailcat 夹具无效");
    let mut values = Vec::new();
    for name in ["TUN+trojan-darwin", "TUN+trojan-win32"] {
        let mut case = load_cases()
            .into_iter()
            .find(|case| case.name == name)
            .unwrap_or_else(|| panic!("基线 fixture {name} 不得丢失"));
        case.input.servers.push(tailcat.clone());
        case.input.selected_server_id = Some("tc".into());
        let deps = full_config_deps(&case, &temp);
        let cfg = generate_sing_box_config(&case.input, &BTreeMap::new(), &deps)
            .unwrap_or_else(|e| panic!("{name} 生成失败: {e}"));
        let value = serde_json::to_value(&cfg).expect("序列化");
        let tun = value["inbounds"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["type"] == "tun")
            .unwrap_or_else(|| panic!("{name} 缺 tun inbound"));
        let exclude = tun["route_exclude_address"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for cidr in ["192.0.2.10/32", "2001:db8::10/128", "192.0.2.20/32"] {
            assert!(
                exclude.contains(&json!(cidr)),
                "{name} 排除表缺 {cidr}：{exclude:?}"
            );
        }
        values.push((name, value));
    }

    let Some(core) = core_or_skip("Tailcat DERP 排除完整配置门") else {
        return;
    };
    for (i, (name, value)) in values.iter().enumerate() {
        let path = temp.path().join(format!("tailcat-tun-{i}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(value).expect("JSON 编码")).expect("写盘");
        let (ok, diag) = check(&core, &path);
        assert!(ok, "{name} 含 DERP 排除的完整配置被真核拒绝：{diag}");
    }
}

/// 网络场景规则全形态（system/dhcp × DNS/流量 × inline/logical/外化）逐平台生成，完整配置喂 `check`。
///
/// `check` 只 decode + initialize，**拦不住环境项的坏引用**（Start 才解析，spec K3）——那一格归
/// `network_profile_runtime` 起核门。本门锁的是另一件事：环境项的键名/值形态、外化外层规则、
/// logical 包裹、`dns-netenv`(dhcp) transport 在随包核上都能 decode。打包腿按目标各跑一次
/// （`POLARIS_KERNEL_GATE_TARGET`），故四平台随包核各自过一遍这批产物。
#[test]
fn bundled_core_accepts_network_profile_rules_in_every_shape() {
    use polaris_config_engine::builder::custom_rule_files::build_custom_rule_files;
    use polaris_config_engine::builder::generate_sing_box_config_with_report;
    use polaris_config_engine::user_config::app_config::UserConfig;

    let temp = tempdir().expect("建 TempDir");
    let custom_dir = temp.path().join("custom-rules");
    std::fs::create_dir_all(&custom_dir).expect("建 custom-rules");
    let traffic = |id: &str, profile: &str, conditions: Value, mode: &str| {
        json!({"id": id, "type": conditions[0]["type"], "values": conditions[0]["values"],
            "conditions": conditions, "combineMode": mode, "networkProfileId": profile,
            "action": "direct", "enabled": true,
            "effects": {"route": {"enabled": true, "action": "direct"}}})
    };
    let dns = |id: &str, suffix: &str, profile: Option<&str>, action: Value| {
        let mut rule = json!({"id": id, "type": "domainSuffix", "values": [suffix],
            "action": "direct", "enabled": true,
            "effects": {"dns": {"enabled": true, "action": action,
                "resolver": "inherit", "answerMode": "real"}}});
        if let Some(p) = profile {
            rule["networkProfileId"] = json!(p);
        }
        rule
    };
    // (平台, 模式, 接管, 场景 A 的 probe, 场景 B 的 probe, 期望生成 dns-netenv)
    let shapes = [
        ("linux", "tun", false, "system", "dhcp", true),
        ("win32", "systemProxy", false, "auto", "system", true),
        ("darwin", "tun", true, "auto", "system", true),
        ("darwin", "systemProxy", false, "system", "auto", false),
    ];
    let Some(core) = core_or_skip("网络场景规则完整配置门") else {
        return;
    };
    for (platform, mode, takeover, probe_a, probe_b, want_netenv) in shapes {
        let name = format!("网络场景全形态 {platform}/{mode}");
        let mut rules = Vec::new();
        for (profile, tag) in [("np-a", "a"), ("np-b", "b")] {
            let ext = json!([{"type": "domainSuffix", "values": [format!("{tag}.corp.example")]}]);
            let inline = json!([{"type": "geosite", "values": ["cn"]}]);
            let logical = json!([
                {"type": "geosite", "values": ["cn"]}, {"type": "port", "values": ["8443"]}]);
            rules.push(traffic(&format!("{tag}-ext"), profile, ext, "or"));
            rules.push(traffic(&format!("{tag}-inline"), profile, inline, "or"));
            rules.push(traffic(&format!("{tag}-logical"), profile, logical, "and"));
        }
        let mut dns_rules = vec![
            dns(
                "d-a",
                "corp.example",
                Some("np-a"),
                json!({"type": "server", "serverId": "builtin-domestic"}),
            ),
            dns(
                "d-b",
                "b.corp.example",
                Some("np-b"),
                json!({"type": "fakeIp"}),
            ),
        ];
        if platform == "linux" {
            // D5：DNS 动作引用内置「当前网络 DHCP 下发的 DNS」。
            dns_rules.push(dns(
                "d-netenv",
                "lan.corp.example",
                None,
                json!({"type": "server", "serverId": "builtin-netenv-dhcp"}),
            ));
        }
        let input: UserConfig = serde_json::from_value(json!({
            "configSchemaVersion": 4, "servers": [], "selectedServerId": "__direct__",
            "proxyMode": "smart", "proxyModeType": mode,
            "networkProfiles": [
                {"id": "np-a", "name": "A", "match": {"dnsServerCidrs": ["10.20.0.0/16"],
                    "searchDomains": ["corp.example"]}, "probe": probe_a},
                {"id": "np-b", "name": "B", "match": {"dnsServerCidrs": ["10.30.0.0/16", "fd00::/8"],
                    "searchDomains": ["b.corp.example"]}, "probe": probe_b},
            ],
            "customRules": rules,
            "dnsRules": dns_rules,
        }))
        .expect("fixture UserConfig");
        for (file, content) in build_custom_rule_files(&input) {
            std::fs::write(custom_dir.join(file), content).expect("写外化文件");
        }
        let case = SnapshotCase {
            name: name.clone(),
            platform: platform.into(),
            input,
        };
        let mut deps = full_config_deps(&case, &temp);
        deps.system_dns_takeover_active = takeover;
        let outcome = generate_sing_box_config_with_report(&case.input, &BTreeMap::new(), &deps)
            .unwrap_or_else(|e| panic!("{name} 生成失败: {e}"));
        assert!(
            outcome.pruned_env_rules.is_empty(),
            "{name}：夹具里不该有被剪的场景规则：{:?}",
            outcome.pruned_env_rules
        );
        let value = serde_json::to_value(&outcome.config).expect("序列化");
        let text = value.to_string();
        let netenv = value["dns"]["servers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["tag"] == "dns-netenv" && s["type"] == "dhcp");
        assert_eq!(
            netenv, want_netenv,
            "{name}：dns-netenv 只在有规则需要时生成"
        );
        let route_env = value["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r.to_string().contains("\"dns_se"))
            .count();
        // 6 条流量规则 × 2 判据 = 12 条（外化/inline/logical 各 4）。
        assert_eq!(route_env, 12, "{name}：流量环境规则条数不对");
        let ext_outer = value["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| {
                r["rule_set"].to_string().contains("custom-rule-")
                    && r.to_string().contains("\"dns_se")
            })
            .count();
        assert_eq!(
            ext_outer, 4,
            "{name}：外化规则的环境项必须在外层 rule_set 规则上"
        );
        assert!(
            text.contains("\"type\":\"logical\""),
            "{name}：缺 logical 形态"
        );
        let path = temp.path().join(format!("np-{platform}-{mode}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(&value).expect("JSON 编码")).expect("写盘");
        let (ok, diag) = check(&core, &path);
        assert!(ok, "{name} 被随包核拒绝：{diag}");
    }
}
