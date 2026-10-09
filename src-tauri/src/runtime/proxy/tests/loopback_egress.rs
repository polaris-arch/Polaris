//! **α 批：进程内经本机回环入站出网的两道门**（2026-09-25）。
//!
//! # 门一：端口对拍（「status 暴露给消费方的口，在该平台生成的配置里确有监听」）
//!
//! 缺陷形态（A2）：Android 不发射 `mixed-in`，但 `ProxyStatus.mixed_port` 无条件等于配置的端口，
//! 解锁检测 / 出口 IP / 测速回退 / warm RTT 四条腿连向一个没人监听的口 —— 全超时、全程无报错。
//! 本门逐平台（轴从 `Platform::ALL` 派生）走**生产同一组函数**：生成侧 `build_inbounds`、
//! 状态侧 [`exposed_mixed_port`]、凭据侧 [`loopback_auth_for`]、消费侧 [`select_local_http_proxy`]，
//! 断言消费方拿到的每一个口都在生成出来的入站里真有监听，且凭据与该入站要求的**逐字相等**。
//!
//! # 门二：消费方全覆盖（源码级）
//!
//! 「任何经本机回环入站发请求的 Rust 调用点都经过带凭据的构造路径」。取材面 = `src-tauri/src` 与
//! 全部 `crates/*/src` 的**生产**文件，先剥注释与字符串（[`polaris_source_probe::mask_comments_and_strings`]）
//! 再取。两半：
//!
//! 1. **原语登记**：出站代理/回环连接的原语（`Proxy::all|http|https|custom`、`TcpStream::connect*`）
//!    每一处都必须落在登记表里的某个函数内，且登记为「带凭据」的函数体里必须真有挂凭据的那一步。
//!    新增一处原语 ⇒ 未登记 ⇒ 红（逼作者回答「这条腿打不打本机入站、带不带凭据」）。
//! 2. **调用点**：带凭据构造器的每一个生产调用点，凭据实参都不许是字面 `None`（唯一例外登记在
//!    [`NONE_AUTH_ALLOWED`]，附理由）。

use std::path::{Path, PathBuf};

use polaris_config_engine::builder::inbounds::{build_inbounds, InboundsDeps};
use polaris_config_engine::user_config::proxy_ports::local_proxy_port;
use polaris_helper_proto::Platform;

use super::super::startup::{exposed_mixed_port, loopback_auth_for};
use super::*;

// ══════════════════════════════ 门一：端口对拍 ══════════════════════════════

/// 平台枚举 → 生成侧吃的平台串。穷举 `match`、无通配：加变体编译不过，平台轴因此从单一枚举派生。
fn node_tag(platform: Platform) -> &'static str {
    match platform {
        Platform::Mac => "darwin",
        Platform::Win => "win32",
        Platform::Linux => "linux",
        Platform::Android => "android",
        Platform::Ios => "ios",
        Platform::Other => "freebsd",
    }
}

/// 一个入站在生成产物里的「监听事实」：类型 + 端口 + 它要求的凭据。
fn listener_on(
    inbounds: &[serde_json::Value],
    port: u16,
) -> Option<(String, Option<serde_json::Value>)> {
    inbounds.iter().find_map(|ib| {
        (ib.get("listen_port").and_then(serde_json::Value::as_u64) == Some(u64::from(port))).then(
            || {
                (
                    ib.get("type")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    ib.get("users").cloned(),
                )
            },
        )
    })
}

fn users_json(
    auth: Option<&polaris_config_engine::singbox::InboundUser>,
) -> Option<serde_json::Value> {
    auth.map(|u| serde_json::json!([{ "username": u.username, "password": u.password }]))
}

#[test]
fn every_platform_exposes_only_ports_that_its_generated_inbounds_listen_on() {
    let config: UserConfig = serde_json::from_value(serde_json::json!({
        "proxyMode": "smart", "proxyModeType": "tun", "mixedPort": 17890,
        "selectedServerId": null, "servers": []
    }))
    .expect("最小配置可解析");
    let configured_mixed = local_proxy_port(&config);
    assert_eq!(configured_mixed, 17890, "夹具前提：配置的 mixed 端口");

    let (probe_proxy, update_in, subscription_in) = (31002u16, 31003u16, 31004u16);
    let mut android_like = 0usize;
    let mut desktop_like = 0usize;
    for platform in Platform::ALL.iter().copied() {
        let tag = node_tag(platform);
        assert_eq!(Platform::parse(tag), platform, "平台轴投影写歪了：{tag}");
        // 生产同一个函数产凭据（Android 上是真随机值；桌面 None）。
        let loopback_auth = loopback_auth_for(platform);
        let deps = InboundsDeps {
            probe_direct_port: None,
            probe_proxy_port: Some(probe_proxy),
            debug_probe_mixed_udp: false,
            update_in_port: Some(update_in),
            subscription_update_in_port: Some(subscription_in),
            loopback_auth: loopback_auth.clone(),
            probe_pool_ports: vec![31005],
            platform: tag.to_owned(),
            own_lan_cidrs: vec![],
            observed_tailnet_addresses: Default::default(),
            log: |_, _| {},
        };
        let inbounds: Vec<serde_json::Value> = build_inbounds(&config, None, &deps)
            .iter()
            .map(|ib| serde_json::to_value(ib).unwrap())
            .collect();

        // 状态侧：与起核处同一个函数。
        let status_mixed = exposed_mixed_port(platform, configured_mixed);
        if status_mixed != 0 {
            let (kind, users) = listener_on(&inbounds, status_mixed).unwrap_or_else(|| {
                panic!("[{tag}] status.mixed_port={status_mixed} 没有任何入站监听")
            });
            assert_eq!(
                kind, "mixed",
                "[{tag}] mixed_port 上监听的必须是 mixed 入站"
            );
            assert!(users.is_none(), "[{tag}] mixed 入站不带凭据");
        }

        // 消费侧：与 `ProxyRuntime::local_http_proxy` 同一个纯函数。
        let local =
            select_local_http_proxy(status_mixed, deps.probe_proxy_port, loopback_auth.clone())
                .unwrap_or_else(|| {
                    panic!("[{tag}] 有 probe-proxy-in 时消费方必须拿得到一个本机 http 代理口")
                });
        let (kind, users) = listener_on(&inbounds, local.port)
            .unwrap_or_else(|| panic!("[{tag}] 消费方拿到的口 {} 没有任何入站监听", local.port));
        assert!(
            kind == "http" || kind == "mixed",
            "[{tag}] 消费方按 http 代理用它，入站类型却是 {kind}"
        );
        assert_eq!(
            users,
            users_json(local.auth.as_ref()),
            "[{tag}] 消费方带的凭据与入站要求的不一致（多带/少带都是断链）"
        );

        // socks 两口：消费方用 `ProxyRuntime::loopback_auth`（= 同一份 `loopback_auth`）。
        for port in [update_in, subscription_in] {
            let (kind, users) = listener_on(&inbounds, port)
                .unwrap_or_else(|| panic!("[{tag}] status 暴露的 socks 口 {port} 没有入站监听"));
            assert_eq!(kind, "socks", "[{tag}] {port} 应为 socks 入站");
            assert_eq!(
                users,
                users_json(loopback_auth.as_ref()),
                "[{tag}] {port} 凭据不一致"
            );
        }

        if status_mixed == 0 {
            android_like += 1;
        } else {
            desktop_like += 1;
        }
    }
    // 取材面自检：两种形态都真的跑过（缺一条腿，上面对应分支的断言就一次都没执行）。
    assert!(
        android_like >= 2 && desktop_like >= 3,
        "平台轴覆盖不足：无 mixed {android_like} / 有 mixed {desktop_like}"
    );
}

/// Android 凭据：每次调用都是新值、长度 ≥ 128 bit，且 `Debug` 不泄口令。
#[test]
fn android_loopback_credential_is_fresh_strong_and_redacted() {
    let a = loopback_auth_for(Platform::Android).expect("Android 必须产出凭据");
    let b = loopback_auth_for(Platform::Android).expect("Android 必须产出凭据");
    assert_ne!(a.password, b.password, "每次起核必须重新生成");
    assert!(
        a.password.len() >= 32,
        "口令至少 128 bit（32 位 hex），实得 {} 字符",
        a.password.len()
    );
    assert!(
        a.password.chars().all(|c| c.is_ascii_hexdigit()),
        "口令应为 hex"
    );
    let dbg = format!("{a:?}");
    assert!(!dbg.contains(&a.password), "Debug 泄露口令：{dbg}");
    for desktop in [Platform::Mac, Platform::Win, Platform::Linux] {
        assert!(
            loopback_auth_for(desktop).is_none(),
            "{desktop:?} 不该生成凭据（桌面零认证不变）"
        );
    }
}

/// `ProxyRuntime::local_http_proxy` 经快照取址：Android 形态（mixed=0）走 probe-proxy-in 并带凭据；
/// 桌面形态走 mixed、不带凭据；核未运行 → None。
#[test]
fn runtime_local_http_proxy_follows_the_running_snapshot() {
    let (rt, _dir) = test_runtime();
    let auth = polaris_config_engine::singbox::InboundUser {
        username: "polaris".into(),
        password: "00112233445566778899aabbccddeeff".into(),
    };
    *rt.switch_snapshot.write().unwrap() = Some(super::super::hot_switch::SwitchSnapshot {
        mesh_mode_ready: false,
        dashboard_mode_selector: false,
        probe_proxy_port: Some(31002),
        loopback_auth: Some(auth.clone()),
        ..Default::default()
    });
    assert_eq!(
        rt.local_http_proxy(),
        None,
        "核未运行 → 没有可用的本机代理口"
    );

    // Android 形态：status.mixed_port = 0。
    mark_running(&rt);
    rt.status.write().unwrap().mixed_port = 0;
    assert_eq!(
        rt.local_http_proxy(),
        Some(LocalHttpProxy {
            port: 31002,
            auth: Some(auth.clone()),
            inbound: LocalInbound::ProbeProxy,
        })
    );
    assert_eq!(rt.loopback_auth(), Some(auth));

    // 桌面形态：mixed 在 → 用 mixed、不带凭据（mixed 入站零认证）。
    rt.status.write().unwrap().mixed_port = 7890;
    assert_eq!(
        rt.local_http_proxy(),
        Some(LocalHttpProxy {
            port: 7890,
            auth: None,
            inbound: LocalInbound::Mixed,
        })
    );
}

// ══════════════════════════════ 门二：消费方全覆盖 ══════════════════════════════

/// 出站代理 / 回环连接原语（剥注释与字符串之后的代码面上取）。
const PRIMITIVES: &[&str] = &[
    "Proxy::all(",
    "Proxy::http(",
    "Proxy::https(",
    "Proxy::custom(",
    "TcpStream::connect(",
    "TcpStream::connect_timeout(",
];

/// 原语登记表：(仓库相对路径, 所在函数名, 该函数若是「带凭据」构造路径则给出函数体里必须出现的
/// 挂凭据标记；`None` = 不打本机代理入站，理由写第四列)。
const PRIMITIVE_REGISTRY: &[(&str, &str, Option<&str>, &str)] = &[
    (
        "src-tauri/src/runtime/http.rs",
        "with_local_proxy_url",
        Some("basic_auth("),
        "HttpRuntime 经本机 http/socks 入站的唯一构造路径",
    ),
    (
        "crates/unlock-transport/src/lib.rs",
        "via_local_proxy",
        Some("basic_auth("),
        "解锁检测 wreq client 经本机 http 入站",
    ),
    (
        "src-tauri/src/runtime/speedtest_tunnel.rs",
        "open_tunnel",
        Some("connect_request(auth)"),
        "测速 CONNECT 隧道（probe-in-k / probe-proxy-in / mixed / 临时核入站）",
    ),
    (
        "src-tauri/src/runtime/proxy/auto_switch.rs",
        "probe_through_proxy",
        Some("connectivity_probe_request(target_url, auth)"),
        "自动换节点连通性探针（probe-proxy-in）",
    ),
    (
        "src-tauri/src/runtime/speedtest.rs",
        "production",
        None,
        "临时核就绪探测：只 TCP 建连不发任何请求，不经代理出网",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "wait_ready",
        None,
        "管理 API 端口就绪探测（gRPC 管理口，不是代理入站，自带 secret）",
    ),
    (
        "crates/singbox-grpc/src/h2c.rs",
        "connect_h2c",
        None,
        "管理 API gRPC 传输的生产拨号器（带 secret 的管理口，不是代理入站）",
    ),
];

/// 带凭据构造器（名字, 凭据实参下标）。
const CREDENTIALED_CALLEES: &[(&str, usize)] = &[
    ("via_local_proxy", 1),
    ("via_local_socks_proxy", 1),
    ("measure_via_local_proxy", 1),
    ("measure_inbound", 3),
    ("measure_slot", 3),
    ("open_tunnel", 1),
    ("probe_through_proxy", 1),
    ("probe_proxy_connectivity", 1),
];

/// 允许凭据实参为字面 `None` 的生产调用点：(路径, 折叠空白后的调用原文, 理由)。
const NONE_AUTH_ALLOWED: &[(&str, &str, &str)] = &[(
    "src-tauri/src/commands/speedtest.rs",
    "measure_via_local_proxy(port, None, &url, &|_| {})",
    "临时核腿：临时核自建的 `in-<tag>` 入站零认证（`build_temp_core_config`），且临时核只在桌面跑\
     （Android 核在进程内，没有可 spawn 的临时核二进制）。",
)];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 在仓库根下")
        .to_path_buf()
}

/// 取材面：`src-tauri/src` + 每个 `crates/*/src` 的生产文件，(仓库相对路径, 净化后全文)。
fn production_surface() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (rel, src) in crate::test_support::module_files("") {
        out.push((
            format!("src-tauri/src/{rel}"),
            polaris_source_probe::mask_comments_and_strings(&src),
        ));
    }
    let crates = repo_root().join("crates");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&crates)
        .expect("读 crates/")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("src").is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        for (rel, src) in polaris_source_probe::module_files_in(&dir, "") {
            out.push((
                format!("crates/{name}/src/{rel}"),
                polaris_source_probe::mask_comments_and_strings(&src),
            ));
        }
    }
    out
}

/// `pos` 之前最近的 `fn <name>`（按词边界）；找不到返回空串。
fn enclosing_fn(code: &str, pos: usize) -> String {
    let head = &code[..pos];
    let mut search = head.len();
    while let Some(i) = head[..search].rfind("fn ") {
        let boundary = i == 0
            || !head.as_bytes()[i - 1].is_ascii_alphanumeric() && head.as_bytes()[i - 1] != b'_';
        if boundary {
            let name: String = head[i + 3..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return name;
            }
        }
        search = i;
    }
    String::new()
}

/// `code[start..]` 从 `fn name` 起的函数体（到配平的 `}`）。
fn fn_body<'a>(code: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!("fn {name}");
    let mut from = 0;
    while let Some(off) = code[from..].find(&needle) {
        let at = from + off;
        let after = code[at + needle.len()..].chars().next();
        if after.is_some_and(|c| c == '(' || c == '<') {
            let open = at + code[at..].find('{')?;
            let mut depth = 0i32;
            for (i, ch) in code[open..].char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(&code[at..=open + i]);
                        }
                    }
                    _ => {}
                }
            }
            return None;
        }
        from = at + needle.len();
    }
    None
}

/// 从 `(` 起取配平的实参表，按顶层逗号切开。
fn call_args(code: &str, open_paren: usize) -> Option<(Vec<String>, usize)> {
    let mut depth = 0i32;
    let mut args = Vec::new();
    let mut cur = String::new();
    for (i, ch) in code[open_paren..].char_indices() {
        match ch {
            '(' | '[' | '{' => {
                depth += 1;
                if depth > 1 {
                    cur.push(ch);
                }
            }
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    if !cur.trim().is_empty() {
                        args.push(cur.trim().to_string());
                    }
                    return Some((args, open_paren + i));
                }
                cur.push(ch);
            }
            ',' if depth == 1 => {
                args.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    None
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn every_loopback_egress_primitive_is_registered_and_credentialed() {
    let surface = production_surface();
    // 切点自检 ①：取材面不是空的，且真含那两个已知文件。
    assert!(
        surface.len() > 100,
        "取材面塌了：只有 {} 个文件",
        surface.len()
    );
    for must in [
        "src-tauri/src/runtime/http.rs",
        "crates/unlock-transport/src/lib.rs",
    ] {
        assert!(surface.iter().any(|(p, _)| p == must), "取材面缺 {must}");
    }

    let mut found: Vec<(String, String)> = Vec::new();
    let mut unregistered = Vec::new();
    for (path, code) in &surface {
        for prim in PRIMITIVES {
            let mut from = 0;
            while let Some(off) = code[from..].find(prim) {
                let at = from + off;
                from = at + prim.len();
                // `TcpStream::connect(` 是 `TcpStream::connect_timeout(` 的前缀吗？不是（括号在前）。
                let func = enclosing_fn(code, at);
                if PRIMITIVE_REGISTRY
                    .iter()
                    .any(|(p, f, _, _)| p == path && *f == func)
                {
                    found.push((path.clone(), func));
                } else {
                    let line = code[..at].lines().count();
                    unregistered.push(format!("{path}:{line} 在 fn {func} 内用了 `{prim}`"));
                }
            }
        }
    }
    assert!(
        unregistered.is_empty(),
        "发现未登记的出站代理/回环连接原语 —— 回答它打不打本机代理入站、带不带凭据后登记进 \
         PRIMITIVE_REGISTRY：\n{}",
        unregistered.join("\n")
    );
    // 防腐烂：每条登记都真的还在（改名/删除后登记必须跟着改）。
    for (path, func, marker, why) in PRIMITIVE_REGISTRY {
        assert!(
            found.iter().any(|(p, f)| p == path && f == func),
            "登记条目已腐烂：{path} fn {func}（{why}）里找不到任何原语"
        );
        if let Some(marker) = marker {
            let code = &surface.iter().find(|(p, _)| p == path).unwrap().1;
            let body =
                fn_body(code, func).unwrap_or_else(|| panic!("{path} 里找不到 fn {func} 的函数体"));
            assert!(
                body.contains(marker),
                "{path} fn {func} 是带凭据构造路径，但函数体里没有挂凭据的 `{marker}`（{why}）"
            );
        }
    }
    // 两个「间接」凭据步骤本身也要在：CONNECT 报文与共用的头生成器。
    let tunnel = &surface
        .iter()
        .find(|(p, _)| p == "src-tauri/src/runtime/speedtest_tunnel.rs")
        .unwrap()
        .1;
    assert!(
        fn_body(tunnel, "connect_request")
            .is_some_and(|b| b.contains("proxy_authorization_line(auth)")),
        "connect_request 必须把凭据写进 CONNECT"
    );
}

#[test]
fn no_production_call_site_passes_a_literal_none_credential() {
    let surface = production_surface();
    let mut calls = 0usize;
    let mut allowed_hits = vec![0usize; NONE_AUTH_ALLOWED.len()];
    let mut violations = Vec::new();
    for (path, code) in &surface {
        for (callee, auth_idx) in CREDENTIALED_CALLEES {
            let needle = format!("{callee}(");
            let mut from = 0;
            while let Some(off) = code[from..].find(&needle) {
                let at = from + off;
                from = at + needle.len();
                let prev = code[..at].chars().next_back();
                if prev.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
                    continue; // 词边界：`measure_via_local_proxy(` 不算 `via_local_proxy(`
                }
                if code[..at].trim_end().ends_with("fn") {
                    continue; // 定义处
                }
                let Some((args, close)) = call_args(code, at + callee.len()) else {
                    continue;
                };
                calls += 1;
                let auth = args.get(*auth_idx).map(|a| squash(a)).unwrap_or_default();
                assert!(
                    !auth.is_empty(),
                    "{path}: `{callee}` 调用缺凭据实参（实参表 {args:?}）—— 切点写歪了"
                );
                if auth == "None" {
                    let text = squash(&code[at..=close]);
                    match NONE_AUTH_ALLOWED
                        .iter()
                        .position(|(p, t, _)| p == path && *t == text)
                    {
                        Some(i) => allowed_hits[i] += 1,
                        None => violations.push(format!("{path}: {text}")),
                    }
                }
            }
        }
    }
    // 切点自检：生产调用点至少这么多（ipinfo / unlock×2 / subscription / icon / 测速 ×5 / 自动换节点 ×2 …）。
    assert!(
        calls >= 12,
        "只扫到 {calls} 个带凭据构造器调用点 —— 取材面或切点塌了"
    );
    assert!(
        violations.is_empty(),
        "以下生产调用点把凭据写死成 `None`（Android 上必然 407 / socks 认证失败）：\n{}",
        violations.join("\n")
    );
    for (i, (path, text, why)) in NONE_AUTH_ALLOWED.iter().enumerate() {
        assert_eq!(
            allowed_hits[i], 1,
            "豁免条目已腐烂或重复：{path} `{text}`（{why}）"
        );
    }
}

/// 自动换节点探针报文：带凭据 ⇒ 有 `Proxy-Authorization`（Android 上漏发 ⇒ 407 ⇒ 每拍「不通」⇒
/// 自动换走一个好节点）；不带 ⇒ 与改动前逐字节相同。牙：删掉报文里的 `{proxy_auth}` → 正面断言转红。
#[test]
fn connectivity_probe_request_presents_the_loopback_credential() {
    let user = polaris_config_engine::singbox::InboundUser {
        username: "polaris".into(),
        password: "00112233445566778899aabbccddeeff".into(),
    };
    let url = "http://cp.cloudflare.com/generate_204";
    let with = super::super::auto_switch::connectivity_probe_request(url, Some(&user))
        .expect("http 目标可构造");
    let expected_auth = format!(
        "Proxy-Authorization: Basic {}\r\n",
        crate::runtime::mesh::base64_encode(b"polaris:00112233445566778899aabbccddeeff")
    );
    assert!(
        with.contains(&expected_auth),
        "带凭据报文缺 Proxy-Authorization：{with:?}"
    );
    let without = super::super::auto_switch::connectivity_probe_request(url, None).unwrap();
    assert_eq!(
        without,
        "GET http://cp.cloudflare.com/generate_204 HTTP/1.1\r\nHost: cp.cloudflare.com\r\nProxy-Connection: close\r\nConnection: close\r\n\r\n"
    );
    assert_eq!(
        super::super::auto_switch::connectivity_probe_request("https://x/y", None),
        None,
        "非 http:// 目标取不到 Host → None"
    );
}
