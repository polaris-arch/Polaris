//! 🔴 网络场景规则的**起核门**（spec §5.5 / §8.2，N1）。
//!
//! `check` 拦不住环境项的错误引用：`dns_server_address` / `dns_search_domain` 引用的 transport 在
//! **Start** 阶段才解析（tag 不存在 / 类型不支持 ⇒ FATAL，spec K3）。所以本门用生成器产出的**完整**
//! DNS/route 配置真起核：
//!
//! 1. 用 `generate_sing_box_config_with_report` 生成（system 源 linux 系统代理；dhcp 源 linux TUN 形态）。
//! 2. 剥掉全部入站与 services（**不建 TUN、不监听生成配置里的端口**），只加一个回环 `direct` UDP 入站 +
//!    `hijack-dns`；canary 就是生成器产出的场景 DNS 规则本身（动作 `predefined`，查询不出网）。
//! 3. `run` 后：进程存活 ≥3s、无 FATAL；canary 正向（`0.0.0.0/0,::/0`）命中、反向（`192.0.2.1/32`）不命中。
//! 4. 反向对照：绕过剪枝把环境 tag 改坏 ⇒ `run` 必须 FATAL（证明本门能检出 Start 阶段失败）。
//!
//! **不触网保障（dhcp 用例）**：开跑前断言非 root、CapEff/CapAmb 全零、`ip_unprivileged_port_start > 68`、
//! 核文件无 file capability ⇒ 内核绑 UDP 68 必在发包之前 EACCES（spec K6，`dhcp.go` 先 listen 后发包）。
//! 不满足：`POLARIS_REQUIRE_KERNEL_GATE=1` 下红，否则跳过。**绝不以 root 身份跑本门。**
//!
//! **本机禁起核**：`POLARIS_NO_KERNEL_RUN=1`（本机 `scripts/gate-rust.sh` 自动设）下全部用例跳过，覆盖交给 CI；
//! 判定与 `run` 子命令的拼接只在 `support/kernel_run.rs` 一处。

mod support;

use std::collections::BTreeMap;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use polaris_config_engine::builder::generate_sing_box_config_with_report;
use polaris_config_engine::user_config::app_config::UserConfig;
use serde_json::{json, Value};
use support::core_locator::{command_for_core, kernel_gate_required};
use support::kernel_gate::{check, core_or_skip, full_config_deps, SnapshotCase};
use support::kernel_run::{kernel_run_or_skip, with_run};
use tempfile::TempDir;

const CANARY_SUFFIX: &str = "np-canary.polaris.invalid";
const PROBE_INBOUND: &str = "np-probe-in";

fn canary(name: &str) -> String {
    format!("{name}.{CANARY_SUFFIX}")
}

fn profile(id: &str, cidrs: &[&str], probe: &str) -> Value {
    json!({"id": id, "name": id, "enabled": true,
        "match": {"dnsServerCidrs": cidrs}, "probe": probe})
}

/// 场景 DNS 规则：命中 ⇒ `predefined` A 127.0.0.1（不经 transport，不出网）。
fn canary_rule(name: &str, profile_id: &str) -> Value {
    let domain = canary(name);
    json!({"id": format!("r-{name}"), "type": "domain", "values": [domain],
        "networkProfileId": profile_id, "action": "direct", "enabled": true,
        "effects": {"dns": {"enabled": true, "resolver": "inherit", "answerMode": "real",
            "action": {"type": "predefined", "rcode": "NOERROR",
                "answer": [format!("{domain}. IN A 127.0.0.1")]}}}})
}

/// 兜底：canary 域名未命中场景规则 ⇒ NXDOMAIN（绝不落到上游解析器）。
fn canary_fallback() -> Value {
    json!({"id": "r-fallback", "type": "domainSuffix", "values": [CANARY_SUFFIX],
        "action": "direct", "enabled": true,
        "effects": {"dns": {"enabled": true, "resolver": "inherit", "answerMode": "real",
            "action": {"type": "predefined", "rcode": "NXDOMAIN"}}}})
}

/// 场景流量规则（inline 默认 + logical 两形态）：环境项在 Start 阶段同样要解析，一并起核。
fn traffic_rules(profile_id: &str) -> Value {
    json!([
        {"id": "rt-inline", "type": "domainSuffix", "values": [canary("rt")],
            "networkProfileId": profile_id, "action": "direct", "enabled": true,
            "effects": {"route": {"enabled": true, "action": "direct"}}},
        {"id": "rt-logical", "type": "domainSuffix", "values": [canary("rtl")],
            "conditions": [
                {"type": "domainSuffix", "values": [canary("rtl")]},
                {"type": "port", "values": ["8443"]}],
            "combineMode": "and", "networkProfileId": profile_id,
            "action": "direct", "enabled": true,
            "effects": {"route": {"enabled": true, "action": "direct"}}},
    ])
}

fn user_config(mode_type: &str, profiles: Value, dns_rules: Value, traffic: Value) -> UserConfig {
    serde_json::from_value(json!({
        "configSchemaVersion": 4,
        "servers": [],
        "selectedServerId": "__direct__",
        "proxyMode": "smart",
        "proxyModeType": mode_type,
        "networkProfiles": profiles,
        "dnsRules": dns_rules,
        "trafficRules": traffic,
    }))
    .expect("fixture UserConfig")
}

/// 生成 → 断言无剪枝 → 剥入站/services、加回环 canary 入站与 hijack-dns。
fn runtime_config(
    name: &str,
    platform: &str,
    input: UserConfig,
    temp: &TempDir,
    port: u16,
) -> Value {
    let case = SnapshotCase {
        name: name.into(),
        platform: platform.into(),
        input,
    };
    let deps = full_config_deps(&case, temp);
    let outcome = generate_sing_box_config_with_report(&case.input, &BTreeMap::new(), &deps)
        .unwrap_or_else(|e| panic!("{name} 生成失败: {e}"));
    assert!(
        outcome.pruned_env_rules.is_empty(),
        "{name} 的场景规则被剪了，本门要测的规则不在配置里：{:?}",
        outcome.pruned_env_rules
    );
    let mut value = serde_json::to_value(&outcome.config).expect("序列化");
    let env_rules = value.to_string().matches("\"dns_server_address\"").count();
    assert!(
        env_rules >= 3,
        "{name} 生成结果里环境项太少（{env_rules}），fixture 没有射程"
    );
    value["log"] = json!({"level": "info", "timestamp": false});
    value["inbounds"] = json!([{
        "type": "direct", "tag": PROBE_INBOUND, "network": "udp",
        "listen": "127.0.0.1", "listen_port": port,
    }]);
    value.as_object_mut().unwrap().remove("services");
    value["route"]["rules"]
        .as_array_mut()
        .expect("route.rules")
        .insert(
            0,
            json!({"inbound": [PROBE_INBOUND], "action": "hijack-dns"}),
        );
    value
}

fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn dns_query(name: &str) -> Vec<u8> {
    let mut packet = vec![0x4e, 0x50, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in name.split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 1, 0, 1]);
    packet
}

/// `(rcode, answer 数)`；超时 = `None`。
fn ask(addr: SocketAddr, name: &str) -> Option<(u8, u16)> {
    let socket = UdpSocket::bind("127.0.0.1:0").ok()?;
    socket
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok()?;
    socket.send_to(&dns_query(name), addr).ok()?;
    let mut buf = [0u8; 1500];
    let (len, _) = socket.recv_from(&mut buf).ok()?;
    (len >= 12).then(|| (buf[3] & 0x0f, u16::from_be_bytes([buf[6], buf[7]])))
}

fn hit(addr: SocketAddr, name: &str) -> bool {
    match ask(addr, name) {
        Some((0, answers)) => answers > 0,
        Some((3, 0)) => false,
        other => panic!("{name} 的应答既不是 NOERROR+答案也不是 NXDOMAIN：{other:?}"),
    }
}

fn write_config(temp: &TempDir, file: &str, value: &Value) -> std::path::PathBuf {
    let path = temp.path().join(file);
    std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    path
}

struct Running {
    child: Child,
    log: std::path::PathBuf,
}

impl Running {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn run(core: &Path, config: &Path, temp: &TempDir, tag: &str) -> Running {
    let log = temp.path().join(format!("{tag}.log"));
    let mut cmd = command_for_core(core);
    cmd.arg("--disable-color");
    let child = with_run(cmd)
        .arg("-c")
        .arg(config)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    Running { child, log }
}

/// 等回环入站可答（canary 兜底恒有应答）；进程先退出则带日志报错。
fn wait_answering(running: &mut Running, addr: SocketAddr, probe: &str) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        if let Ok(Some(status)) = running.child.try_wait() {
            panic!("sing-box 起核后退出（{status}）：\n{}", running.log());
        }
        if ask(addr, probe).is_some() {
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("回环 canary 入站未在时限内应答：\n{}", running.log());
}

fn assert_alive_without_fatal(running: &mut Running, since: Instant, what: &str) {
    let min = Duration::from_secs(3);
    if let Some(rest) = min.checked_sub(since.elapsed()) {
        thread::sleep(rest);
    }
    assert!(
        running.child.try_wait().unwrap().is_none(),
        "{what}：起核后 3s 内退出：\n{}",
        running.log()
    );
    let log = running.log();
    assert!(!log.contains("FATAL"), "{what}：日志出现 FATAL：\n{log}");
}

#[test]
fn system_source_env_rules_start_and_evaluate_on_bundled_core() {
    if !kernel_run_or_skip("网络场景起核门（system 源）") {
        return;
    }
    let Some(core) = core_or_skip("网络场景起核门（system 源）") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let port = free_udp_port();
    let input = user_config(
        "systemProxy",
        json!([
            profile("np-any", &["0.0.0.0/0", "::/0"], "system"),
            profile("np-neg", &["192.0.2.1/32"], "system"),
        ]),
        json!([
            canary_rule("any", "np-any"),
            canary_rule("neg", "np-neg"),
            canary_fallback()
        ]),
        traffic_rules("np-any"),
    );
    let value = runtime_config("system 源（linux 系统代理）", "linux", input, &temp, port);
    let path = write_config(&temp, "np-system.json", &value);
    let (ok, diag) = check(&core, &path);
    assert!(ok, "system 源配置被 check 拒绝：{diag}");

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let started = Instant::now();
    let mut running = run(&core, &path, &temp, "np-system");
    wait_answering(&mut running, addr, &canary("ready"));
    assert!(
        hit(addr, &canary("any")),
        "正向 canary（dns-local ∈ 0.0.0.0/0,::/0）必须命中：\n{}",
        running.log()
    );
    assert!(
        !hit(addr, &canary("neg")),
        "反向 canary（192.0.2.1/32）不得命中"
    );
    assert!(!hit(addr, &canary("nothing")), "兜底必须 NXDOMAIN");
    assert_alive_without_fatal(&mut running, started, "system 源");
}

/// **N4 生产 canary 形态**（spec §11 N4 验收 ①②③）：canary 入站、`hijack-dns` 与 canary DNS 规则全部由
/// 生成器产出（`GenerateConfigDeps::network_canary_port`），本门只剥掉**其它**入站与 services（不建 TUN、
/// 不监听别的端口），canary 入站原样保留。隐私模式开着（核日志抬到 ≥warn）：命中态走 DNS 查询，不读日志。
#[test]
fn production_canary_answers_hit_and_miss_on_bundled_core_in_privacy_mode() {
    if !kernel_run_or_skip("网络场景起核门（生产 canary）") {
        return;
    }
    let Some(core) = core_or_skip("网络场景起核门（生产 canary）") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let port = free_udp_port();
    let input = user_config(
        "systemProxy",
        json!([
            profile("np-any", &["0.0.0.0/0", "::/0"], "system"),
            profile("np-neg", &["192.0.2.1/32"], "system"),
        ]),
        json!([canary_rule("any", "np-any")]),
        traffic_rules("np-any"),
    );
    let case = SnapshotCase {
        name: "生产 canary".into(),
        platform: "linux".into(),
        input,
    };
    let mut deps = full_config_deps(&case, &temp);
    deps.network_canary_port = Some(port);
    deps.privacy_mode = true;
    let outcome =
        generate_sing_box_config_with_report(&case.input, &BTreeMap::new(), &deps).expect("生成");
    let plan = outcome
        .network_canary
        .expect("两个可用场景 ⇒ 必须生成 canary");
    assert_eq!(plan.port, port);
    let domain_of = |id: &str| {
        plan.canaries
            .iter()
            .find(|c| c.profile_id == id)
            .unwrap_or_else(|| panic!("{id} 没有 canary：{:?}", plan.canaries))
            .domain
            .clone()
    };
    let mut value = serde_json::to_value(&outcome.config).expect("序列化");
    let level = value["log"]["level"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        matches!(level.as_str(), "warn" | "error" | "fatal" | "panic"),
        "隐私模式下核日志级别应 ≥warn（本门要证明命中态不依赖日志），实际 {level:?}"
    );
    value["inbounds"]
        .as_array_mut()
        .unwrap()
        .retain(|i| i["tag"] == PROBE_INBOUND);
    assert_eq!(
        value["inbounds"].as_array().unwrap().len(),
        1,
        "生产配置里必须有 canary 入站"
    );
    value.as_object_mut().unwrap().remove("services");
    let path = write_config(&temp, "np-canary.json", &value);
    let (ok, diag) = check(&core, &path);
    assert!(ok, "生产 canary 配置被 check 拒绝：{diag}");

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let started = Instant::now();
    let mut running = run(&core, &path, &temp, "np-canary");
    wait_answering(&mut running, addr, &domain_of("np-neg"));
    assert!(
        hit(addr, &domain_of("np-any")),
        "正向 canary（dns-local ∈ 0.0.0.0/0,::/0）必须命中：\n{}",
        running.log()
    );
    assert!(
        !hit(addr, &domain_of("np-neg")),
        "反向 canary（192.0.2.1/32）不得命中"
    );
    // 不可借道（Android 回环全设备共享，`direct` 入站没有 users）：这个口上的**非 canary** 查询由真核在本地
    // 直接 REFUSED，不落进通用规则 / `dns.final`（那些 server 的 detour 可能是代理）。取 `.invalid` 名：
    // 即便兜底失效，也只会是一次对保留域名的查询，不是可解析的真实域名。
    assert_eq!(
        ask(addr, "borrow-check.polaris.invalid"),
        Some((5, 0)),
        "canary 口上的非 canary 查询必须被真核 REFUSED（rcode 5）：\n{}",
        running.log()
    );
    assert_alive_without_fatal(&mut running, started, "生产 canary");
}

/// 反向对照：绕过剪枝让坏引用进配置 ⇒ `check` 仍 rc=0（拦不住），`run` 必须 FATAL。
#[test]
fn bad_env_ref_bypassing_prune_is_fatal_at_start() {
    if !kernel_run_or_skip("网络场景起核门（反向对照）") {
        return;
    }
    let Some(core) = core_or_skip("网络场景起核门（反向对照）") else {
        return;
    };
    let temp = tempfile::tempdir().unwrap();
    let port = free_udp_port();
    let input = user_config(
        "systemProxy",
        json!([profile("np-any", &["0.0.0.0/0"], "system")]),
        json!([canary_rule("any", "np-any"), canary_fallback()]),
        traffic_rules("np-any"),
    );
    let good = runtime_config("反向对照基线", "linux", input, &temp, port);
    let cases = [
        ("dns-np-missing", "DNS server not found"),
        ("dns-remote", "does not support dns_server_address"),
    ];
    for (bad_tag, expect) in cases {
        let mut value = good.clone();
        let rules = value["dns"]["rules"].as_array_mut().unwrap();
        let target = rules
            .iter_mut()
            .find(|r| r.get("dns_server_address").is_some())
            .expect("基线里必须有带环境项的 DNS 规则");
        let original = target["dns_server_address"]["dns-local"].take();
        target["dns_server_address"] = json!({ bad_tag: original });
        let path = write_config(&temp, &format!("np-bad-{bad_tag}.json"), &value);
        let (ok, diag) = check(&core, &path);
        assert!(
            ok,
            "check 本应拦不住环境引用（Start 才解析）；若它现在拦得住，请更新 spec K3：{diag}"
        );
        let mut running = run(&core, &path, &temp, &format!("np-bad-{bad_tag}"));
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            if let Some(status) = running.child.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "坏引用 {bad_tag} 起核 8s 仍未退出 —— 本门对 Start 阶段失败没有牙：\n{}",
                running.log()
            );
            thread::sleep(Duration::from_millis(100));
        };
        let log = running.log();
        assert!(
            !status.success(),
            "坏引用 {bad_tag} 的核以成功状态退出：\n{log}"
        );
        assert!(
            log.contains("FATAL") && log.contains(expect),
            "坏引用 {bad_tag} 的失败原因不对（应含「{expect}」）：\n{log}"
        );
    }
}

/// dhcp 用例的前置：本进程与核都没有绑 <1024 端口的能力 ⇒ 绑 UDP 68 必在发包前失败。
fn dhcp_cannot_emit_packets(core: &Path) -> Result<(), String> {
    let status = std::fs::read_to_string("/proc/self/status").map_err(|e| e.to_string())?;
    let field = |key: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(key))
            .map(str::trim)
            .unwrap_or_default()
            .to_string()
    };
    let euid = field("Uid:")
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    if euid == "0" || euid.is_empty() {
        return Err(format!(
            "euid={euid:?}（root 可绑 68，会真的发 DHCP DISCOVER）"
        ));
    }
    for key in ["CapEff:", "CapAmb:"] {
        let v = field(key);
        if v.trim_start_matches('0').is_empty() {
            continue;
        }
        return Err(format!("{key} {v} 非零"));
    }
    let start: u32 = std::fs::read_to_string("/proc/sys/net/ipv4/ip_unprivileged_port_start")
        .map_err(|e| e.to_string())?
        .trim()
        .parse()
        .map_err(|e| format!("{e}"))?;
    if start <= 68 {
        return Err(format!("ip_unprivileged_port_start={start} ≤ 68"));
    }
    if let Ok(out) = std::process::Command::new("getcap").arg(core).output() {
        let caps = String::from_utf8_lossy(&out.stdout);
        if !caps.trim().is_empty() {
            return Err(format!("核带 file capability：{caps}"));
        }
    }
    Ok(())
}

#[test]
fn dhcp_source_env_rules_start_and_fail_closed_without_privilege() {
    if !kernel_run_or_skip("网络场景起核门（dhcp 源）") {
        return;
    }
    let Some(core) = core_or_skip("网络场景起核门（dhcp 源）") else {
        return;
    };
    if !cfg!(target_os = "linux") {
        eprintln!("⚠ 跳过 dhcp 起核门：不触网前置只在 Linux 上可证");
        return;
    }
    if let Err(why) = dhcp_cannot_emit_packets(&core) {
        assert!(
            !kernel_gate_required(),
            "dhcp 起核门的不触网前置不成立：{why}"
        );
        eprintln!("⚠ 跳过 dhcp 起核门：{why}");
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let port = free_udp_port();
    let input = user_config(
        "tun",
        json!([
            profile("np-dhcp", &["0.0.0.0/0", "::/0"], "dhcp"),
            profile("np-any", &["0.0.0.0/0", "::/0"], "system"),
        ]),
        json!([
            canary_rule("dhcp", "np-dhcp"),
            canary_rule("any", "np-any"),
            canary_fallback()
        ]),
        traffic_rules("np-dhcp"),
    );
    // TUN 形态只生成、不建 TUN：runtime_config 把 tun 入站连同其它入站一起剥掉。
    let value = runtime_config("dhcp 源（linux TUN 形态）", "linux", input, &temp, port);
    let netenv = value["dns"]["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|s| s["tag"] == "dns-netenv" && s["type"] == "dhcp")
        .count();
    assert_eq!(netenv, 1, "dhcp 源必须生成 dns-netenv");
    let path = write_config(&temp, "np-dhcp.json", &value);
    let (ok, diag) = check(&core, &path);
    assert!(ok, "dhcp 源配置被 check 拒绝：{diag}");

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let started = Instant::now();
    let mut running = run(&core, &path, &temp, "np-dhcp");
    wait_answering(&mut running, addr, &canary("ready"));
    // dhcp 首次失败是粘滞的（spec K7），等它记下失败再查，避免把「还在取」误读成「不命中」。
    let deadline = Instant::now() + Duration::from_secs(6);
    while !running.log().contains("dhcp: fetch DNS servers") && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(100));
    }
    let log = running.log();
    let dhcp_lines: Vec<&str> = log.lines().filter(|l| l.contains("dhcp:")).collect();
    eprintln!("dhcp transport 日志：{dhcp_lines:?}");
    assert!(
        log.contains("dhcp: fetch DNS servers"),
        "dhcp transport 没有启动/失败的日志（应含 `dhcp: fetch DNS servers`）：\n{log}"
    );
    assert!(
        !log.contains("dhcp: updated DNS servers"),
        "非特权环境下 dhcp 竟然拿到了应答 —— 不触网前置被击穿：\n{log}"
    );
    assert!(
        !hit(addr, &canary("dhcp")),
        "非特权下 dhcp 源恒不命中（fail-closed）"
    );
    assert!(
        hit(addr, &canary("any")),
        "同配置里的 system 源 canary 必须照常命中（正向对照，证明不是整条 DNS 链挂了）"
    );
    assert_alive_without_fatal(&mut running, started, "dhcp 源");
}

/// LAN DNS 的完整生成链：公网解析器设为有答案的陷阱；LAN 无上游必须 SERVFAIL，
/// 有上游只进入私网 UDP。仅测试时将已验过的私网地址替换为本机 UDP fixture。
#[test]
fn lan_names_never_fall_through_to_public_dns() {
    if !kernel_run_or_skip("lan_names_never_fall_through_to_public_dns") {
        return;
    }
    let Some(core) = core_or_skip("LAN DNS isolation") else {
        return;
    };
    let names = [
        "router.lan",
        "lan",
        "nas.home.arpa",
        "home.arpa",
        "nas.home",
        "db.internal",
        "printer",
        "1.10.168.192.in-addr.arpa",
        "1.0.0.0.ip6.arpa",
    ];
    for candidate in [None, Some("192.168.1.1"), Some("8.8.8.8")] {
        let temp = TempDir::new().unwrap();
        let input = user_config("systemProxy", json!([]), json!([]), json!([]));
        let case = SnapshotCase {
            name: "LAN isolation".into(),
            platform: "win32".into(),
            input,
        };
        let mut deps = full_config_deps(&case, &temp);
        deps.lan_resolver_for_dns = candidate.map(str::to_string);
        let outcome =
            generate_sing_box_config_with_report(&case.input, &BTreeMap::new(), &deps).unwrap();
        let mut cfg = serde_json::to_value(outcome.config).unwrap();
        let private = candidate == Some("192.168.1.1");
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .unwrap();
        let upstream_port = socket.local_addr().unwrap().port();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_reader = stop.clone();
        let upstream = thread::spawn(move || {
            let mut queries = std::collections::BTreeSet::new();
            let mut packet = [0u8; 4096];
            while !stop_reader.load(std::sync::atomic::Ordering::SeqCst) {
                let Ok((size, peer)) = socket.recv_from(&mut packet) else {
                    continue;
                };
                let mut response = packet[..size].to_vec();
                response[2..4].copy_from_slice(&[0x81, 0x80]);
                response[6..8].copy_from_slice(&[0, 1]);
                response
                    .extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, 10, 66, 66, 1]);
                socket.send_to(&response, peer).unwrap();
                let mut offset = 12;
                let mut labels = Vec::new();
                while packet[offset] != 0 {
                    let len = usize::from(packet[offset]);
                    offset += 1;
                    labels
                        .push(String::from_utf8_lossy(&packet[offset..offset + len]).into_owned());
                    offset += len;
                }
                queries.insert(labels.join("."));
            }
            queries
        });
        for server in cfg["dns"]["servers"].as_array_mut().unwrap() {
            let tag = server["tag"].as_str().unwrap().to_string();
            if tag == "dns-lan" {
                assert!(private);
                assert_eq!(server["type"], "udp");
                assert_eq!(server["server"], "192.168.1.1");
                server["server"] = json!("127.0.0.1");
                server["server_port"] = json!(upstream_port);
            } else if tag == "dns-mdns" {
                assert_eq!(server["type"], "mdns");
            } else {
                // 落入任何原公网/系统/FakeIP 解析器都会得到 NOERROR，不能伪装成失败关闭。
                let predefined: serde_json::Map<String, Value> = names
                    .iter()
                    .map(|name| (name.to_string(), json!(["203.0.113.99"])))
                    .collect();
                *server = json!({"tag": tag, "type": "hosts", "predefined": predefined});
            }
        }
        let port = free_udp_port();
        cfg["log"] = json!({"level": "debug", "timestamp": false});
        cfg["inbounds"] = json!([{"type":"direct", "tag":"lan-test", "network":"udp",
            "listen":"127.0.0.1", "listen_port":port}]);
        cfg.as_object_mut().unwrap().remove("services");
        cfg["route"]["rules"] = json!([{"inbound":["lan-test"], "action":"hijack-dns"}]);
        let path = write_config(&temp, "lan.json", &cfg);
        let (ok, output) = check(&core, &path);
        assert!(ok, "LAN DNS check: {output}");
        let mut running = run(&core, &path, &temp, "lan");
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        wait_answering(&mut running, addr, names[0]);
        for name in names {
            assert_eq!(
                ask(addr, name),
                Some(if private { (0, 1) } else { (2, 0) }),
                "{candidate:?} {name}: {}",
                running.log()
            );
        }
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let queries = upstream.join().unwrap();
        if private {
            assert_eq!(queries, names.into_iter().map(str::to_string).collect());
        } else {
            assert!(queries.is_empty());
        }
    }
}
