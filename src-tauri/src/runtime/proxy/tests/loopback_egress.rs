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
        "src-tauri/src/native_verification/proxy_carrier.rs",
        "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation",
        None,
        "Exact management API refusal probe: connects only to the retained runtime's API port, sends no proxy request; private purpose is checked separately",
    ),
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
        "open_tunnel_with_tls_config",
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
    (
        "crates/singbox-grpc/src/h2c.rs",
        "connect_captured",
        None,
        "直连已验证 Core 管理口的单 carrier 拨号器；不是代理入站，Bearer 由原管理 client 持有；已捕获后禁止重拨",
    ),
];

// These four sites really DO use the desktop mixed proxy ingress. They are
// not control probes and must never borrow the None/control classification.
const NATIVE_OWNER: &str = "src-tauri/src/native_verification/proxy_carrier.rs";
const PRIVATE_NATIVE_PROXY_FUNCTIONS: &[&str] = &[
    "drive_traffic_through_proxy",
    "controlled_http_request",
    "real_core_full_lifecycle",
    "real_core_aggregate_relay_emits_real_frames",
];

// Exact closed normal-library producer contract, including its actual session owner.
// Read the same seven real producer faces as the configuration guards; no ambient exemption.
fn native_compact(source: &str) -> String {
    polaris_source_probe::mask_comments(source)
        .chars()
        .filter(|c| !rust_whitespace(*c))
        .collect()
}
fn native_code(source: &str) -> String {
    native_compact(&polaris_source_probe::mask_comments_and_strings(source))
}
fn native_carrier_body<'a>(source: &'a str, head: &str) -> Option<&'a str> {
    let bare = polaris_source_probe::mask_comments_and_strings(source);
    if bare.matches(head).count() != 1 {
        return None;
    }
    let start = bare.find(head)?;
    if !native_target_boundary(&bare, start) {
        return None;
    }
    let open = start + bare[start..].find('{')?;
    if let Some(keyword) = head.rfind("fn ") {
        let (_, _, actual_body) = rust_named_function(&bare, start + keyword)?;
        if actual_body.start != open {
            return None;
        }
    }
    let mut depth = 0usize;
    for (offset, byte) in bare[open..].bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&source[open..=open + offset]);
                }
            }
            _ => {}
        }
    }
    None
}
// Match live code, not comments/string text, and require each causal anchor once.
fn native_ordered(source: &str, body: &str, anchors: &[&str]) -> bool {
    let (code, offsets) = native_code_offsets(body);
    let bare = polaris_source_probe::mask_comments_and_strings(body);
    let Some(start) = named_body_offset(source, body) else {
        return false;
    };
    let source_bare = polaris_source_probe::mask_comments_and_strings(source);
    let functions = rust_function_spans(&source_bare);
    let owner = enclosing_fn_with_spans(&source_bare, start + 1, &functions);
    if owner.is_empty() {
        return false;
    }
    let mut previous = None;
    for anchor in anchors {
        if anchor.is_empty() {
            return false;
        }
        let hits: Vec<_> = code
            .match_indices(*anchor)
            .filter(|(at, _)| {
                (!rust_ident_start(anchor.as_bytes()[0])
                    || native_target_boundary(&bare, offsets[*at]))
                    && enclosing_fn_with_spans(&source_bare, start + offsets[*at], &functions)
                        == owner
            })
            .collect();
        if hits.len() != 1 {
            return false;
        }
        let at = hits[0].0;
        if previous.is_some_and(|before| before >= at) {
            return false;
        }
        previous = Some(at);
    }
    true
}
fn closed_native_session_scope(session: &str) -> bool {
    let heads = [
        "fn capability_preflight(",
        "fn caller(",
        "fn root(",
        "fn private_directory(",
        "fn lock_file(",
        "fn selected_owner(",
        "fn matches_selection(",
        "fn hold_session(",
        "fn open(",
        "fn drained(",
    ];
    let bodies: Option<Vec<_>> = heads
        .iter()
        .map(|head| native_carrier_body(session, head))
        .collect();
    let Some(bodies) = bodies else {
        return false;
    };
    let [preflight, caller, root, private, lock, owner, selection, hold, open, drained] =
        bodies.as_slice()
    else {
        return false;
    };
    let s = native_compact(session);
    let root_code = native_code(root);
    let private_code = native_code(private);
    let open_code = native_code(open);
    s.contains("usesuper::CASE_NAMES;")
        && native_code(preflight).starts_with("{super::super::core_binary::dev_core_binary_override()")
        && native_ordered(session, preflight, &[
            "super::super::core_binary::dev_core_binary_override()",
            "ifpolaris_core_supervisor::selected_core::selection_lane()!=polaris_core_supervisor::selected_core::SelectionLane::Formal",
            "forpurposein[ExecPurpose::CheckConfig,ExecPurpose::RunMain]",
            "check_selected_capability(purpose)",
        ])
        && native_compact(preflight).contains("super::super::core_binary::dev_core_binary_override().map(drop).map_err(|e|failure(\"selection\",\"path-override-refused\",e))?;")
        && native_code(preflight).contains("SelectionLane::Formal{returnErr(")
        && native_compact(preflight).contains("check_selected_capability(purpose).map_err(|e|failure(\"capability\",e.kind.status_token(),e))?;")
        && !["std::fs::", "ConfigManager::new(", "HelperRuntime::new(", "ProxyRuntime::new(", "TcpListener::bind("].iter().any(|effect| native_code(preflight).contains(effect))
        && native_code(caller).contains("letuid=nix::unistd::getuid().as_raw();ifuid==0{returnErr(")
        && native_code(caller).contains("Ok(polaris_helper_proto::core_protocol::CallerIdentity::Unix(uid).key())")
        && native_compact(caller).contains("ifvalue==\"sS-1-5-18\"{returnErr(")
        && native_code(caller).contains("polaris_helper::platform::windows::native_caller::current_user()")
        && root_code.contains("letkey=polaris_updater::sha256_hex(caller()?.as_bytes());")
        && native_compact(root).contains("Ok(parent.join(format!(\"polaris-native-production-verification-{key}\")))")
        && root_code.contains("std::env::temp_dir().canonicalize().map_err(|e|e.to_string())?;")
        && root_code.contains("polaris_helper::platform::windows::logic::cfg_allowed(")
        && s.contains("fnroot()->Result<PathBuf,String>{")
        && private_code.contains("std::fs::symlink_metadata(path).map_err(|e|e.to_string())?;")
        && private_code.contains("ifm.uid()!=nix::unistd::getuid().as_raw()||m.mode()&0o077!=0{returnErr(")
        && private_code.contains("PayloadDir::open(path,Some(m.uid()))")
        && private_code.contains(".custom_flags(0x02200000)")
        && private_code.contains("if!m.is_dir()||m.file_attributes()&0x400!=0{returnErr(")
        && private_code.contains("if!polaris_helper::platform::windows::coreacl::is_private_user_config(&sec,&sid[1..]){returnErr(")
        && native_ordered(session, lock, &["letf=matchstd::fs::symlink_metadata(root.join(name))", "f.try_lock()", "Ok(f)"])
        && native_code(lock).contains("f.try_lock().map_err(|e|format!())?;")
        && native_code(lock).contains("Err(e)ife.kind()==std::io::ErrorKind::NotFound=>create_file(&root.join(name))?,Err(e)=>returnErr(e.to_string()),")
        && native_code(owner).contains("crate::runtime::core_identity::selected_manifest(&platform)?;")
        && native_code(owner).contains("caller:caller()?,platform,core_id:id.as_str().into(),provider_fingerprint:m.provider_fingerprint,core_files:m.files,")
        && native_code(owner).contains("helper_sha256:m.helper_sha256,cleaner_sha256:m.cleaner_sha256,required_cases:CASE_NAMES.iter().map(|v|(*v).into()).collect(),")
        && native_code(selection)=="{letmuta=a.clone();a.session_id.clear();a==*b}"
        && s.contains("pub(super)fnhold_session()->Result<(),String>{")
        && native_code(hold).starts_with("{capability_preflight()?;")
        && native_ordered(session, hold, &["capability_preflight()?;", "letmutowner=selected_owner()", "letroot=root()", "letdir=private_directory(&root)", "let_suite=lock_file(&root,&dir,SUITE_LOCK)", "ifstd::fs::symlink_metadata(root.join(OWNER)).is_ok()", "letbefore=retained_size(&root)", "letready=provider_ready(&root);", "ifready.is_ok(){record(", "std::io::stdout().flush()", "letcase_lock=lock_file(&root,&dir,CASE_LOCK);", "letendpoint_idle=provider_idle(&root).ok();"])
        && native_code(hold).contains("ifstd::fs::symlink_metadata(root.join(OWNER)).is_ok(){returnErr(")
        && native_code(hold).contains("letmutdrained=case_lock.is_ok();")
        && native_compact(hold).contains("drained&=matches!((a,b),(Ok(a),Ok(b))ifa==b&&a[\"sessionId\"]==owner.session_id&&a[\"case\"]==CASE_NAMES[i]);")
        && native_compact(hold).contains("foriin0..CASE_NAMES.len(){create_directory(&root.join(format!(\"case-{i}\")))?;}")
        && native_code(hold).contains("record(&root.join(OWNER),&serde_json::to_value(&owner).map_err(|e|e.to_string())?,)?;")
        && native_code(hold).contains("letok=action_ok&&drained&&(started==0||endpoint_idle==Some(true))&&retained.is_ok();")
        && s.contains("pub(super)fnopen(case:&str)->Result<Self,String>{")
        && open_code.starts_with("{capability_preflight()?;")
        && native_ordered(session, open, &["capability_preflight()?;", "leti=CASE_NAMES.iter().position(|v|*v==case)", "letroot=root()?;", "letdirectory=private_directory(&root)?;", "letsuite=directory.open_regular(SUITE_LOCK)", "matchsuite.try_lock()", "letcase_lock=lock_file(&root,&directory,CASE_LOCK)?;", "retained_size(&root)?;", "letowner:Owner=serde_json::from_value(read_record(&directory,OWNER)?)", "ifuuid::Uuid::parse_str(&owner.session_id).is_err()||!matches_selection(&owner,&selected_owner()?)", "provider_ready(&root)", "letcase_dir=root.join(format!(", "private_directory(&case_dir)?;", "record(&root.join(format!(", "Ok(Self{"])
        && open_code.contains("ifuuid::Uuid::parse_str(&owner.session_id).is_err()||!matches_selection(&owner,&selected_owner()?){returnErr(")
        && open_code.contains("Err(std::fs::TryLockError::WouldBlock)=>{}Err(e)=>returnErr(")
        && open_code.contains("Ok(())=>{returnErr(")
        && native_compact(open).contains("provider_ready(&root).map_err(|e|failure(\"provider\",\"provider-unavailable\",e))?;")
        && native_compact(open).contains("letcase_dir=root.join(format!(\"case-{i}\"));")
        && native_compact(open).contains("record(&root.join(format!(\"case-{i}-started.json\")),&receipt)?;")
        && open_code.contains("Ok(Self{config_root:root.clone(),case_dir,root,receipt,_case_lock:case_lock,_directory:directory,})")
        && native_compact(drained).contains("record(&self.root.join(format!(\"case-{i}-drained.json\")),&self.receipt,)")
}
fn closed_native_configuration_scope(
    proxy: &str,
    library: &str,
    carrier: &str,
    session: &str,
    cleanup: &str,
    entrypoint: &str,
    manifest: &str,
) -> bool {
    let c = native_compact(carrier);
    let bare = native_code(carrier);
    let cases = [
        ("runtime::proxy::tests::lifecycle::real_core_full_lifecycle", "lifecycle_case", "real_core_full_lifecycle"),
        ("runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop", "lifecycle_race_case", "real_core_lifecycle_race_start_then_immediate_stop"),
        ("runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid", "hot_switch_case", "real_core_hot_switch_keeps_pid"),
        ("runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt", "failover_case", "real_core_auto_failover_attests_without_applying_saved_debt"),
        ("runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation", "restart_confirmation_case", "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation"),
        ("runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames", "aggregate_relay_case", "real_core_aggregate_relay_emits_real_frames"),
    ];
    let names = format!(
        "constCASE_NAMES:&[&str]=&[{}];",
        cases
            .iter()
            .map(|(name, _, _)| format!("\"{name}\","))
            .collect::<String>()
    );
    let Some(selector) = native_carrier_body(carrier, "fn selected_case_factory(") else {
        return false;
    };
    let expected_selector = format!(
        "{{matchcase{{{}_=>None,}}}}",
        cases
            .iter()
            .map(|(name, factory, _)| format!("\"{name}\"=>{{Some({factory})}}"))
            .collect::<String>()
    );
    if native_compact(selector) != expected_selector
        || !bare.contains("typeNativeCaseFactory=fn()->NativeCaseFuture;")
        || !bare.contains(
            "typeNativeCaseFuture=std::pin::Pin<Box<dynstd::future::Future<Output=()>+'static>>;",
        )
        || !cases.iter().all(|(_, factory, method)| {
            native_carrier_body(carrier, &format!("fn {factory}("))
                .is_some_and(|body| native_compact(body) == format!("{{Box::pin({method}())}}"))
        })
    {
        return false;
    }
    let Some(run_body) = native_carrier_body(carrier, "pub(crate) async fn run_case(") else {
        return false;
    };
    let Some(caught) = native_carrier_body(run_body, "let result = AssertUnwindSafe(async") else {
        return false;
    };
    if !native_ordered(carrier, caught, &["letfuture=factory();", "future.await;"])
        || !native_compact(run_body).contains(&format!(
            "letresult=AssertUnwindSafe(async{}).catch_unwind().await;",
            native_compact(caught)
        ))
    {
        return false;
    }
    let run = native_compact(run_body);
    let run_code = native_code(run_body);
    let policy = "super::core_binary::dev_core_binary_override().map(drop)?;";
    let Some(policy_at) = run_code.find(policy) else {
        return false;
    };
    if !native_ordered(
        carrier,
        run_body,
        &[
            "letfactory=selected_case_factory(case).ok_or(",
            "session::capability_preflight()?;",
            policy,
            "letsession=session::CaseSession::open(case)?;",
            "letdir=session.case_dir.clone();",
            "letconfig=Arc::new(ConfigManager::new(dir.clone()));",
            "lethelper=Arc::new(HelperRuntime::new(session.config_root.clone()));",
            "letmesh=Arc::new(MeshRuntime::new_with_helper(dir.clone(),helper.clone()));",
            "letclearer=Box::new(polaris_system_integration::production_proxy_controller(",
            "letruntime=Arc::new(ProxyRuntime::new(config,helper,mesh,clearer));",
            "DIRECTORY.set(dir.clone()).map_err(",
            "RUNTIME.set(runtime.clone()).map_err(",
            "letresult=AssertUnwindSafe(async{",
            "letfuture=factory();",
            "future.await;",
            ").catch_unwind().await;",
            "letcheck_fence=polaris_core_supervisor::begin_check_shutdown()",
            "letcleanup=cleanup::drain_after_fence(check_fence,runtime.shutdown_for_exit(),async{",
            "polaris_core_supervisor::shutdown_checks_for_exit()",
            "ifcleanup.is_ok()&&fenced&&empty&&no_pid",
            "session.drained(case)?;",
            "ifresult.is_err()",
        ],
    ) || run_code.matches("factory()").count() != 1
        || [
            "std::fs::",
            "eprintln!",
            "ConfigManager::new(",
            "HelperRuntime::new(",
            "ProxyRuntime::new(",
            "CaseSession::open(",
            "factory()",
        ]
        .iter()
        .any(|effect| run_code[..policy_at].contains(effect))
    {
        return false;
    }
    let Some(drain) = native_carrier_body(cleanup, "pub(super) async fn drain_after_fence(") else {
        return false;
    };
    let p = native_compact(proxy);
    let l = native_compact(library);
    let e = native_compact(entrypoint);
    p.contains("#[cfg(desktop)]#[cfg(feature=\"native-production-verification\")]#[cfg(debug_assertions)]#[path=\"../native_verification/proxy_carrier.rs\"]pub(crate)modnative_verification;")
        && l.contains("#[cfg(desktop)]#[cfg(feature=\"native-production-verification\")]#[cfg(debug_assertions)]pubasyncfnrun_native_production_verification(case:&str)->Result<(),String>{runtime::proxy::native_verification::run_case(case).await}")
        && l.contains("#[cfg(desktop)]#[cfg(feature=\"native-production-verification\")]#[cfg(debug_assertions)]pubfnhold_native_production_verification_session()->Result<(),String>{runtime::proxy::native_verification::hold_session()}")
        && e.contains("#[cfg(not(debug_assertions))]compile_error!(\"nativeadmissionverifierisunavailableinreleasebuilds\");")
        && e.contains("letresult=ifargs.len()==1&&args[0]==\"--session\"{polaris_lib::hold_native_production_verification_session()}elseifargs.len()==1{polaris_lib::run_native_production_verification(&args[0]).await}else{")
        && native_compact(manifest).contains("[[bin]]name=\"native-admission-verifier\"path=\"src/bin/native-admission-verifier.rs\"required-features=[\"native-production-verification\"]")
        && c.contains("#[path=\"session.rs\"]modsession;")
        && c.contains("pub(crate)fnhold_session()->Result<(),String>{session::hold_session()}")
        && c.contains("#[cfg(debug_assertions)]pub(crate)asyncfnrun_case(case:&str)->Result<(),String>{if!CASE_NAMES.contains(&case){returnErr(")
        && c.contains(&names)
        && run.contains("DIRECTORY.set(dir.clone()).map_err(|_|\"onecaseperprocess\")?;")
        && run.contains("RUNTIME.set(runtime.clone()).map_err(|_|\"onecaseperprocess\")?;")
        && c.contains("ProxyModeType::Manual.effective_on(runtime.helper.platform())")
        && c.contains("obj.insert(\"proxyModeType\".into(),serde_json::json!(\"manual\"));")
        && c.contains("\"proxyModeType\":\"manual\",")
        && !c.contains("\"allowLan\"") && !c.contains("allow_lan")
        && run_code.contains("letfenced=runtime.desktop_shutdown.lock().map(|v|*v).unwrap_or(false);")
        && run_code.contains(".map(|v|v.is_empty()&&!v.has_helper_start()).unwrap_or(false);")
        && run_code.contains("letno_pid=runtime.pid.lock().map(|v|v.is_none()).unwrap_or(false);")
        && native_code(drain).starts_with("{letmain=main.await;letchecks=checks.await;letmutfailures=Vec::new();")
        && native_ordered(cleanup, drain,&["letmain=main.await;","letchecks=checks.await;","letmutfailures=Vec::new();"])
        && native_compact(drain).contains("[(\"Checkfence\",fence),(\"mainshutdown\",main),(\"Checkdrain\",checks),]")
        && closed_native_session_scope(session)
}
fn native_scope_faces() -> [String; 7] {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    [
        "src-tauri/src/runtime/proxy.rs",
        "src-tauri/src/lib.rs",
        "src-tauri/src/native_verification/proxy_carrier.rs",
        "src-tauri/src/native_verification/session.rs",
        "src-tauri/src/native_verification/cleanup.rs",
        "src-tauri/src/bin/native-admission-verifier.rs",
        "src-tauri/Cargo.toml",
    ]
    .map(|path| std::fs::read_to_string(root.join(path)).expect("actual native producer context"))
}
fn native_scope_is_closed(faces: &[String; 7]) -> bool {
    closed_native_configuration_scope(
        &faces[0], &faces[1], &faces[2], &faces[3], &faces[4], &faces[5], &faces[6],
    )
}
// Mutations below always target the current real producer/owner/caller source.
fn assert_native_scope_controls(faces: &[String; 7]) {
    assert!(native_scope_is_closed(faces));
    // Each conjunct is independently necessary on the actual facade and both exports.
    for (slot, tail) in [
        (0, "#[path = \"../native_verification/proxy_carrier.rs\"]"),
        (1, "pub async fn run_native_production_verification"),
        (1, "pub fn hold_native_production_verification_session"),
    ] {
        let prefix = format!("#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\n{tail}");
        assert_eq!(faces[slot].matches(&prefix).count(), 1);
        for attribute in [
            "#[cfg(desktop)]\n",
            "#[cfg(feature = \"native-production-verification\")]\n",
            "#[cfg(debug_assertions)]\n",
        ] {
            let mut changed = (*faces).clone();
            changed[slot] = faces[slot].replacen(&prefix, &prefix.replacen(attribute, "", 1), 1);
            assert!(
                !native_scope_is_closed(&changed),
                "missing native scope conjunct: {tail}: {attribute}"
            );
        }
    }
    for (slot,old,new) in [
        (0,"#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\n#[path = \"../native_verification/proxy_carrier.rs\"]","#[path = \"../native_verification/proxy_carrier.rs\"]"),
        (1,"#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\npub async fn run_native_production_verification","pub async fn run_native_production_verification"),
        (1,"#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\npub fn hold_native_production_verification_session","pub fn hold_native_production_verification_session"),
        (2,"#[cfg(debug_assertions)]\npub(crate) async fn run_case","pub(crate) async fn run_case"),
        (2,"if !CASE_NAMES.contains(&case)","if false"),
        (2,"ConfigManager::new(dir.clone())","ConfigManager::new(user_directory())"),
        (2,"let dir = session.case_dir.clone();","let dir = user_directory();"),
        (2,"HelperRuntime::new(session.config_root.clone())","HelperRuntime::new(dir.clone())"),
        (2,"let session = session::CaseSession::open(case)?;","let session = unbound_session();"),
        (2,"serde_json::json!(\"manual\")","serde_json::json!(\"tun\")"),
        (2,"\"proxyModeType\": \"manual\"","\"proxyModeType\": \"systemProxy\""),
        (2,".effective_on(runtime.helper.platform())",".without_platform_projection()"),
        (2,"runtime.shutdown_for_exit()","pretend_cleanup()"),
        (2,"polaris_core_supervisor::shutdown_checks_for_exit()","pretend_check_cleanup()"),
        (3,"polaris_updater::sha256_hex(caller()?.as_bytes())","polaris_updater::sha256_hex(b\"another-caller\")"),
        (3,"polaris-native-production-verification-{key}","user-configuration-{key}"),
        (3,"m.uid() != nix::unistd::getuid().as_raw() || m.mode() & 0o077 != 0","false"),
        (3,"!polaris_helper::platform::windows::coreacl::is_private_user_config(&sec, &sid[1..])","false"),
        (3,"config_root: root.clone(),","config_root: user_directory(),"),
        (3,"_case_lock: case_lock,","_case_lock: unrelated_lock(),"),
        (3,"required_cases: CASE_NAMES.iter().map(|v| (*v).into()).collect(),","required_cases: Vec::new(),"),
        (4,"let checks = checks.await;","let checks = Ok(());"),
        (5,"polaris_lib::hold_native_production_verification_session()","ordinary_session()"),
        (6,"required-features = [\"native-production-verification\"]","required-features = []"),
    ] {
        let mut changed=(*faces).clone();
        assert!(changed[slot].contains(old),"negative must mutate actual source: {slot}: {old}");
        changed[slot]=changed[slot].replacen(old,new,1);
        assert!(!native_scope_is_closed(&changed),"closed native scope escaped: {slot}: {old}");
    }
    for (slot, head, old, new) in [
        (
            2,
            "fn selected_case_factory(",
            "Some(lifecycle_case)",
            "Some(hot_switch_case)",
        ),
        (
            2,
            "fn selected_case_factory(",
            "_ => None",
            "_ => Some(lifecycle_case)",
        ),
        (
            2,
            "fn lifecycle_case(",
            "Box::pin(real_core_full_lifecycle())",
            "Box::pin(real_core_hot_switch_keeps_pid())",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "let result = AssertUnwindSafe(async {",
            "let future = factory();\nlet result = AssertUnwindSafe(async {",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "super::core_binary::dev_core_binary_override().map(drop)?;",
            "super::core_binary::dev_core_binary_override().ok();",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "session::capability_preflight()?;",
            "session::capability_preflight().ok();",
        ),
        (
            2,
            "pub(crate) async fn run_case(",
            "if cleanup.is_ok() && fenced && empty && no_pid",
            "if true",
        ),
        (
            3,
            "fn capability_preflight(",
            ".map_err(|e| failure(\"selection\", \"path-override-refused\", e))?;",
            ".ok();",
        ),
        (
            3,
            "fn capability_preflight(",
            "ExecPurpose::CheckConfig, ExecPurpose::RunMain",
            "ExecPurpose::RunMain, ExecPurpose::RunMain",
        ),
        (
            3,
            "fn capability_preflight(",
            "!= polaris_core_supervisor::selected_core::SelectionLane::Formal",
            "== polaris_core_supervisor::selected_core::SelectionLane::Formal",
        ),
        (
            3,
            "fn hold_session(",
            "let mut drained = case_lock.is_ok();",
            "let mut drained = true;",
        ),
        (
            3,
            "fn hold_session(",
            "if std::fs::symlink_metadata(root.join(OWNER)).is_ok()",
            "if false",
        ),
        (
            3,
            "fn hold_session(",
            "capability_preflight()?;",
            "capability_preflight().ok();",
        ),
        (
            3,
            "fn hold_session(",
            "lock_file(&root, &dir, SUITE_LOCK)",
            "lock_file(&root, &dir, CASE_LOCK)",
        ),
        (
            3,
            "fn open(",
            "capability_preflight()?;",
            "capability_preflight().ok();",
        ),
        (
            3,
            "fn open(",
            "Err(std::fs::TryLockError::WouldBlock) => {}",
            "Ok(()) => {}",
        ),
        (
            3,
            "fn open(",
            "!matches_selection(&owner, &selected_owner()?)",
            "false",
        ),
        (
            3,
            "fn open(",
            "private_directory(&case_dir)?;",
            "private_directory(&root)?;",
        ),
        (3, "fn lock_file(", "f.try_lock()", "f.metadata()"),
    ] {
        let body = native_carrier_body(&faces[slot], head).expect("actual native boundary");
        assert_eq!(body.matches(old).count(), 1, "{slot}: {head}: {old}");
        assert_eq!(faces[slot].matches(body).count(), 1);
        let mut changed = (*faces).clone();
        changed[slot] = faces[slot].replacen(body, &body.replacen(old, new, 1), 1);
        assert!(
            !native_scope_is_closed(&changed),
            "native owner boundary escaped: {head}: {old}"
        );
    }
    let drain = native_carrier_body(&faces[4], "pub(super) async fn drain_after_fence(").unwrap();
    assert_eq!(drain.matches("let checks = checks.await;").count(), 1);
    let early_return = drain.replacen(
        "let checks = checks.await;",
        "if main.is_err() { return main; }\nlet checks = checks.await;",
        1,
    );
    let mut changed = (*faces).clone();
    changed[4] = faces[4].replacen(drain, &early_return, 1);
    assert!(
        !native_scope_is_closed(&changed),
        "Main error cannot skip the original Check owner drain"
    );
    let run = native_carrier_body(&faces[2], "pub(crate) async fn run_case(").unwrap();
    for anchor in [
        "let future = factory();",
        "let result = AssertUnwindSafe(async {",
        "session.drained(case)?;",
        "let check_fence =",
    ] {
        assert_eq!(
            run.matches(anchor).count(),
            1,
            "actual movement target: {anchor}"
        );
    }
    let outside = run.replacen("let future = factory();", "", 1).replacen(
        "let result = AssertUnwindSafe(async {",
        "let future = factory();\nlet result = AssertUnwindSafe(async {",
        1,
    );
    let mut changed = (*faces).clone();
    changed[2] = faces[2].replacen(run, &outside, 1);
    assert!(
        !native_scope_is_closed(&changed),
        "the one selected future must be constructed inside catch"
    );
    let moved = run.replacen("session.drained(case)?;", "", 1).replacen(
        "let check_fence =",
        "session.drained(case)?;\nlet check_fence =",
        1,
    );
    let mut changed = (*faces).clone();
    changed[2] = faces[2].replacen(run, &moved, 1);
    assert!(
        !native_scope_is_closed(&changed),
        "checkpoint cannot precede original Main/Check drain"
    );
}

// Rust Reference lex.whitespace: exactly Pattern_White_Space token separators.
fn rust_whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\u{0009}'
            ..='\u{000d}' | ' ' | '\u{0085}' | '\u{200e}' | '\u{200f}' | '\u{2028}' | '\u{2029}'
    )
}
fn rust_ident_character(ch: char) -> bool {
    !rust_whitespace(ch) && (!ch.is_ascii() || rust_ident_continue(ch as u8))
}
// Conservative Rust identifier bytes, including every UTF8 continuation byte.
// This deliberately does not pretend to validate the Unicode XID tables.
fn rust_ident_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic() || byte >= 0x80
}
fn rust_ident_continue(byte: u8) -> bool {
    byte.is_ascii_digit() || rust_ident_start(byte)
}
fn rust_ident_boundary(code: &str, at: usize) -> bool {
    code[..at]
        .chars()
        .next_back()
        .is_none_or(|ch| !rust_ident_character(ch))
}
// Methods/qualified calls are eligible here. A raw identifier denotes the same
// callee, but a Unicode-prefixed different identifier must not donate its suffix.
fn rust_callee_start(code: &str, at: usize) -> Option<usize> {
    if at >= 2 && &code.as_bytes()[at - 2..at] == b"r#" {
        let start = at - 2;
        return rust_ident_boundary(code, start).then_some(start);
    }
    (rust_ident_boundary(code, at) && (at == 0 || code.as_bytes()[at - 1] != b'#')).then_some(at)
}
fn rust_fn_definition_name(code: &str, at: usize) -> bool {
    let before = code[..at].trim_end_matches(rust_whitespace);
    let Some(keyword) = before.len().checked_sub(2) else {
        return false;
    };
    before.ends_with("fn")
        && before.len() < at
        && rust_ident_boundary(code, keyword)
        && (keyword == 0 || code.as_bytes()[keyword - 1] != b'#')
}
fn credentialed_call_start(code: &str, at: usize) -> Option<usize> {
    let start = rust_callee_start(code, at)?;
    (!rust_fn_definition_name(code, start)).then_some(start)
}
// Borrowed slice coordinates are never reconstructed from copied text. Root
// and witness must resolve to the same nonempty actual named owner; unsupported
// or ambiguous ownership cannot stand in for the implicit brace-body root.
fn named_body_offset(source: &str, body: &str) -> Option<usize> {
    let start = (body.as_ptr() as usize).checked_sub(source.as_ptr() as usize)?;
    let end = start.checked_add(body.len())?;
    (end <= source.len() && body.starts_with('{') && body.ends_with('}')).then_some(start)
}
fn named_body_witness(source: &str, body: &str, at: usize) -> bool {
    let Some(start) = named_body_offset(source, body) else {
        return false;
    };
    if at >= body.len() {
        return false;
    }
    let bare = polaris_source_probe::mask_comments_and_strings(source);
    let functions = rust_function_spans(&bare);
    let owner = enclosing_fn_with_spans(&bare, start + 1, &functions);
    !owner.is_empty() && enclosing_fn_with_spans(&bare, start + at, &functions) == owner
}
// Registry credential witnesses are complete callee names in the actual body,
// with method/path separators preserved and inert replay masked out.
fn credential_marker_in_body(source: &str, body: &str, marker: &str) -> bool {
    let code = polaris_source_probe::mask_comments_and_strings(body);
    code.match_indices(marker).any(|(at, _)| {
        credentialed_call_start(&code, at).is_some() && named_body_witness(source, body, at)
    })
}
// Finite source spans only: reuse the established byte-preserving masks and
// call delimiter. Compact-code offsets always map back to the original bytes.
fn native_code_offsets(source: &str) -> (String, Vec<usize>) {
    let bare = polaris_source_probe::mask_comments_and_strings(source);
    let mut code = String::new();
    let mut offsets = Vec::new();
    for (at, ch) in bare.char_indices() {
        if !rust_whitespace(ch) {
            code.push(ch);
            offsets.extend((0..ch.len_utf8()).map(|byte| at + byte));
        }
    }
    (code, offsets)
}
fn native_target_boundary(code: &str, at: usize) -> bool {
    rust_ident_boundary(code, at)
        && code[..at]
            .trim_end_matches(rust_whitespace)
            .chars()
            .next_back()
            .is_none_or(|ch| !matches!(ch, ':' | '.' | '#'))
}
fn native_fragment_span(source: &str, needle: &str) -> Option<std::ops::Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let (code, offsets) = native_code_offsets(source);
    let bare = polaris_source_probe::mask_comments_and_strings(source);
    let hits: Vec<_> = code
        .match_indices(needle)
        .filter(|(at, _)| native_target_boundary(&bare, offsets[*at]))
        .collect();
    if hits.len() != 1 {
        return None;
    }
    let at = hits[0].0;
    Some(offsets[at]..offsets[at + needle.len() - 1] + 1)
}
// Raw unique call location is retained for frozen mutation controls. Positive
// producer evidence additionally goes through native_call_in_owner.
fn native_owned_call<'a>(
    source: &'a str,
    callee: &str,
) -> Option<(std::ops::Range<usize>, &'a str)> {
    if !callee.ends_with('(') {
        return None;
    }
    let (code, offsets) = native_code_offsets(source);
    let bare = polaris_source_probe::mask_comments_and_strings(source);
    let hits: Vec<_> = code
        .match_indices(callee)
        .filter(|(at, _)| native_target_boundary(&bare, offsets[*at]))
        .collect();
    if hits.len() != 1 {
        return None;
    }
    let at = hits[0].0;
    let (_, close) = call_args(&code, at + callee.len() - 1)?;
    let span = offsets[at]..offsets[close] + 1;
    Some((span.clone(), &source[span]))
}
fn native_call_in_owner<'a>(
    source: &str,
    body: &'a str,
    callee: &str,
) -> Option<(std::ops::Range<usize>, &'a str)> {
    let (span, call) = native_owned_call(body, callee)?;
    let bare = polaris_source_probe::mask_comments_and_strings(body);
    if rust_fn_definition_name(&bare, span.start) || !named_body_witness(source, body, span.start) {
        return None;
    }
    Some((span, call))
}
// Preserve literal contents (including whitespace inside them), while ignoring
// only surrounding code whitespace and comments. A literal replay elsewhere
// cannot replace the contents of the actual code-active call span.
fn native_literal_faces(source: &str) -> Vec<String> {
    let visible = polaris_source_probe::mask_comments(source);
    let bare = polaris_source_probe::mask_comments_and_strings(source);
    let mut faces = Vec::new();
    let mut at = 0;
    while let Some(ch) = bare[at..].chars().next() {
        if rust_whitespace(ch) {
            let start = at;
            while let Some(ch) = bare[at..].chars().next() {
                if !rust_whitespace(ch) {
                    break;
                }
                at += ch.len_utf8();
            }
            let face = visible[start..at].trim_matches(rust_whitespace);
            if !face.is_empty() {
                faces.push(face.to_owned());
            }
        } else {
            at += ch.len_utf8();
        }
    }
    faces
}
fn native_same_call(actual: &str, expected: &str) -> bool {
    native_code(actual) == native_code(expected)
        && native_literal_faces(actual) == native_literal_faces(expected)
}
fn native_one_primitive(body: &str) -> bool {
    let code = native_code(body);
    PRIMITIVES
        .iter()
        .map(|primitive| code.matches(*primitive).count())
        .sum::<usize>()
        == 1
}
fn native_proxy_request_scope(source: &str, body: &str, name: &str) -> bool {
    let Some((_, tcp)) = native_call_in_owner(source, body, "tokio::net::TcpStream::connect(")
    else {
        return false;
    };
    if !native_one_primitive(body)
        || !native_same_call(
            tcp,
            r#"tokio::net::TcpStream::connect(("127.0.0.1", mixed))"#,
        )
    {
        return false;
    }
    let (binding, callee, expected) = match name {
        "drive_traffic_through_proxy" => (
            "ifletOk(mutc)=tokio::net::TcpStream::connect(",
            "c.write_all(",
            r#"c.write_all(b"GET http://192.0.2.1/ HTTP/1.1\r\nHost: 192.0.2.1\r\n\r\n")"#,
        ),
        "controlled_http_request" => (
            "letmutsocket=tokio::net::TcpStream::connect(",
            "socket.write_all(",
            r#"socket.write_all(format!("GET http://192.0.2.1/{nonce} HTTP/1.1\r\nHost: 192.0.2.1\r\nConnection: close\r\n\r\n").as_bytes())"#,
        ),
        "real_core_full_lifecycle" | "real_core_aggregate_relay_emits_real_frames" => (
            "letmutc=tokio::net::TcpStream::connect(",
            "c.write_all(",
            "c.write_all(req.as_bytes())",
        ),
        _ => return false,
    };
    let Some((write_span, write)) = native_call_in_owner(source, body, callee) else {
        return false;
    };
    let completion = if name == "drive_traffic_through_proxy" {
        ".await;"
    } else {
        ".await.unwrap();"
    };
    if !native_ordered(source, body, &[binding, callee])
        || !native_same_call(write, expected)
        || !native_code(&body[write_span.end..]).starts_with(completion)
    {
        return false;
    }
    if matches!(
        name,
        "real_core_full_lifecycle" | "real_core_aggregate_relay_emits_real_frames"
    ) {
        let Some((_, request)) = native_call_in_owner(source, body, "format!(") else {
            return false;
        };
        if !native_ordered(source, body, &[binding, "letreq=format!(", callee])
            || !native_same_call(
                request,
                r#"format!("GET http://127.0.0.1:{srv_port}/ HTTP/1.1\r\nHost: 127.0.0.1:{srv_port}\r\n\r\n")"#,
            )
        {
            return false;
        }
    }
    true
}
fn native_management_probe_scope(source: &str, body: &str) -> bool {
    let Some((tcp_span, tcp)) =
        native_call_in_owner(source, body, "tokio::net::TcpStream::connect(")
    else {
        return false;
    };
    let Some((probe_span, probe)) =
        native_call_in_owner(source, body, "letapi_dead=tokio::time::timeout(")
    else {
        return false;
    };
    let (code, offsets) = native_code_offsets(body);
    let bare = polaris_source_probe::mask_comments_and_strings(body);
    let no_write = !code
        .match_indices("write_all(")
        .any(|(at, _)| rust_callee_start(&bare, offsets[at]).is_some());
    native_one_primitive(body)
        && native_same_call(
            tcp,
            r#"tokio::net::TcpStream::connect(("127.0.0.1", st.clash_api_port))"#,
        )
        && tcp_span.start >= probe_span.start
        && tcp_span.end <= probe_span.end
        && native_same_call(
            probe,
            r#"let api_dead = tokio::time::timeout(Duration::from_secs(5), tokio::net::TcpStream::connect(("127.0.0.1", st.clash_api_port)),)"#,
        )
        && native_code(&body[probe_span.end..]).starts_with(".await.expect().is_err();")
        && native_ordered(
            source,
            body,
            &["letapi_dead=tokio::time::timeout(", "assert!(api_dead,"],
        )
        && no_write
}
fn private_native_proxy_scope(faces: &[String; 7]) -> bool {
    if !native_scope_is_closed(faces) {
        return false;
    }
    let carrier = &faces[2];
    let bare = polaris_source_probe::mask_comments_and_strings(carrier);
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let Some(body) = native_carrier_body(carrier, &format!("fn {name}(")) else {
            return false;
        };
        if !native_proxy_request_scope(carrier, body, name) {
            return false;
        }
    }
    let name = "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation";
    let (code, offsets) = native_code_offsets(carrier);
    for primitive in PRIMITIVES {
        for (at, _) in code.match_indices(*primitive) {
            let owner = enclosing_fn(&bare, offsets[at]);
            if !PRIVATE_NATIVE_PROXY_FUNCTIONS.contains(&owner.as_str()) && owner != name {
                return false;
            }
        }
    }
    let Some(body) = native_carrier_body(carrier, &format!("fn {name}(")) else {
        return false;
    };
    native_management_probe_scope(carrier, body)
}

fn native_traffic_faces(faces: &[String; 7], slot: usize, source: String) -> [String; 7] {
    let mut changed = (*faces).clone();
    changed[slot] = source;
    changed
}
fn native_replace_span(body: &str, span: std::ops::Range<usize>, replacement: &str) -> String {
    assert!(!body.is_empty() && span.start < span.end && span.end <= body.len());
    let target = &body[span.clone()];
    assert_ne!(target, replacement, "actual target bytes must change");
    let changed = format!(
        "{}{}{}",
        &body[..span.start],
        replacement,
        &body[span.end..]
    );
    assert!(!changed.trim().is_empty());
    assert_eq!(&changed[..span.start], &body[..span.start]);
    assert_eq!(
        &changed[span.start + replacement.len()..],
        &body[span.end..]
    );
    assert_eq!(
        &changed[span.start..span.start + replacement.len()],
        replacement
    );
    changed
}
fn native_replace_fragment(body: &str, old: &str, new: &str) -> String {
    let span =
        native_fragment_span(body, &native_code(old)).expect("one actual code-active fragment");
    assert!(
        native_same_call(&body[span.clone()], old),
        "exact fragment literals and code"
    );
    native_replace_span(body, span, new)
}
fn native_replace_call(body: &str, callee: &str, replacement: &str) -> String {
    let (span, _) = native_owned_call(body, callee).expect("one actual complete call");
    native_replace_span(body, span, replacement)
}
fn native_redirect_call(body: &str, callee: &str, replacement_callee: &str) -> String {
    let (_, call) = native_owned_call(body, callee).expect("actual write call");
    let (code, offsets) = native_code_offsets(call);
    assert!(code.starts_with(callee));
    let open = offsets[callee.len() - 1];
    assert_eq!(call.as_bytes()[open], b'(');
    let replacement = format!("{replacement_callee}{}", &call[open..]);
    native_replace_call(body, callee, &replacement)
}
fn native_mutate_owner(
    faces: &[String; 7],
    slot: usize,
    head: &str,
    mutation: impl FnOnce(&str) -> String,
) -> [String; 7] {
    assert!(
        private_native_proxy_scope(faces),
        "unmodified positive before each mutation"
    );
    let source = &faces[slot];
    let body = native_carrier_body(source, head).expect("one actual finite mutation owner");
    assert!(!body.trim().is_empty());
    assert_eq!(
        source.matches(body).count(),
        1,
        "unique original body bytes"
    );
    let changed_body = mutation(body);
    assert_ne!(changed_body, body);
    assert!(!changed_body.trim().is_empty());
    let start = body.as_ptr() as usize - source.as_ptr() as usize;
    native_traffic_faces(
        faces,
        slot,
        native_replace_span(source, start..start + body.len(), &changed_body),
    )
}
fn native_replay_in_owner(body: &str, text: &str, kind: usize) -> String {
    assert!(body.starts_with('{') && body.ends_with('}'));
    let replay = match kind {
        0 => format!("\n// {}\n", text.replace('\n', "\n// ")),
        1 => format!("\n/* {text} */\n"),
        2 => format!("\nlet _native_purpose_replay = r####\"{text}\"####;\n"),
        _ => panic!("finite replay kind"),
    };
    format!(
        "{}{}{}",
        &body[..body.len() - 1],
        replay,
        &body[body.len() - 1..]
    )
}

#[test]
fn native_proxy_traffic_and_management_probe_keep_distinct_private_purposes() {
    let faces = native_scope_faces();
    assert_native_scope_controls(&faces);
    assert!(private_native_proxy_scope(&faces));
    let tcp = "tokio::net::TcpStream::connect(";
    // Preserve the legacy purpose negatives, now with explicit current owners.
    for (name, replacement) in [
        (
            "drive_traffic_through_proxy",
            r#"tokio::net::TcpStream::connect(("0.0.0.0", mixed))"#,
        ),
        (
            "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation",
            r#"tokio::net::TcpStream::connect(("127.0.0.1", mixed))"#,
        ),
    ] {
        let changed = native_mutate_owner(&faces, 2, &format!("fn {name}("), |body| {
            native_replace_call(body, tcp, replacement)
        });
        assert!(
            !private_native_proxy_scope(&changed),
            "legacy actual dial purpose: {name}"
        );
    }
    let changed = native_mutate_owner(&faces, 2, "fn drive_traffic_through_proxy(", |body| {
        native_redirect_call(body, "c.write_all(", "c.pretend_write")
    });
    assert!(
        !private_native_proxy_scope(&changed),
        "legacy real GET write must remain connected"
    );
    for (old, new) in [
        (
            "ConfigManager::new(dir.clone())",
            "ConfigManager::new(user_directory())",
        ),
        ("if !CASE_NAMES.contains(&case)", "if false"),
        (
            "super::core_binary::dev_core_binary_override().map(drop)?;",
            "super::core_binary::dev_core_binary_override().ok();",
        ),
        (
            "super::core_binary::dev_core_binary_override().map(drop)?;",
            "",
        ),
    ] {
        let changed = native_mutate_owner(&faces, 2, "pub(crate) async fn run_case(", |body| {
            native_replace_fragment(body, old, new)
        });
        assert!(
            !private_native_proxy_scope(&changed),
            "legacy private scope: {old}"
        );
    }
    let policy = "super::core_binary::dev_core_binary_override().map(drop)?;";
    let constructor = "let config = Arc::new(ConfigManager::new(dir.clone()));";
    let changed = native_mutate_owner(&faces, 2, "pub(crate) async fn run_case(", |body| {
        let without = native_replace_fragment(body, policy, "");
        let span = native_fragment_span(&without, &native_code(constructor)).unwrap();
        assert!(native_same_call(&without[span.clone()], constructor));
        format!("{}\n{policy}{}", &without[..span.end], &without[span.end..])
    });
    assert!(
        !private_native_proxy_scope(&changed),
        "path policy cannot follow config construction"
    );
    let guard = "#[cfg(desktop)]\n#[cfg(feature = \"native-production-verification\")]\n#[cfg(debug_assertions)]\n#[path = \"../native_verification/proxy_carrier.rs\"]\npub(crate) mod native_verification;";
    assert!(private_native_proxy_scope(&faces));
    let facade = native_replace_fragment(
        &faces[0],
        guard,
        &guard.replace("#[cfg(debug_assertions)]\n", ""),
    );
    assert!(!private_native_proxy_scope(&native_traffic_faces(
        &faces, 0, facade
    )));

    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let head = format!("fn {name}(");
        for replacement in [
            r#"tokio::net::TcpStream::connect(("0.0.0.0", mixed))"#,
            r#"tokio::net::TcpStream::connect(("127.0.0.1", st.clash_api_port))"#,
        ] {
            let changed = native_mutate_owner(&faces, 2, &head, |body| {
                native_replace_call(body, tcp, replacement)
            });
            assert!(
                !private_native_proxy_scope(&changed),
                "actual proxy TCP purpose: {name}"
            );
        }
        let callee = if *name == "controlled_http_request" {
            "socket.write_all("
        } else {
            "c.write_all("
        };
        let changed = native_mutate_owner(&faces, 2, &head, |body| {
            native_redirect_call(body, callee, "unrelated_stream.pretend_write")
        });
        assert!(
            !private_native_proxy_scope(&changed),
            "actual proxy request write: {name}"
        );
        let changed = native_mutate_owner(&faces, 2, &head, |body| {
            let (span, _) = native_owned_call(body, callee).unwrap();
            let (code, offsets) = native_code_offsets(&body[span.end..]);
            let completion = if *name == "drive_traffic_through_proxy" {
                ".await;"
            } else {
                ".await.unwrap();"
            };
            assert!(code.starts_with(completion));
            let end = span.end + offsets[completion.len() - 1] + 1;
            native_replace_span(body, span.end..end, ";")
        });
        assert!(
            !private_native_proxy_scope(&changed),
            "actual request write must be awaited: {name}"
        );
        let changed = native_mutate_owner(&faces, 2, &head, |body| {
            assert!(body.starts_with('{'));
            format!("{{ {tcp}(\"127.0.0.1\", mixed)).await;{}", &body[1..])
        });
        assert!(
            !private_native_proxy_scope(&changed),
            "one reviewed primitive per request owner: {name}"
        );
    }
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let head = format!("fn {name}(");
        let original = native_carrier_body(&faces[2], &head).unwrap();
        let write_callee = if *name == "controlled_http_request" {
            "socket.write_all("
        } else {
            "c.write_all("
        };
        let (_, original_write) = native_owned_call(original, write_callee).unwrap();
        let request_callee = if matches!(
            *name,
            "real_core_full_lifecycle" | "real_core_aggregate_relay_emits_real_frames"
        ) {
            "format!("
        } else {
            write_callee
        };
        let (_, original_request) = native_owned_call(original, request_callee).unwrap();
        let (old_url, bad_url) = if request_callee == "format!(" {
            ("GET http://127.0.0.1", "GET http://192.0.2.1")
        } else {
            ("GET http://192.0.2.1", "GET http://198.51.100.1")
        };
        assert_eq!(
            original_request.matches(old_url).count(),
            1,
            "literal in actual request call only"
        );
        let bad_request = original_request.replacen(old_url, bad_url, 1);
        for kind in 0..3 {
            let positive = native_mutate_owner(&faces, 2, &head, |body| {
                native_replay_in_owner(body, original_request, kind)
            });
            assert!(
                private_native_proxy_scope(&positive),
                "inert request replay cannot add code: {name}/{kind}"
            );
            let changed = native_mutate_owner(&faces, 2, &head, |body| {
                native_replay_in_owner(
                    &native_replace_call(body, request_callee, &bad_request),
                    original_request,
                    kind,
                )
            });
            assert!(
                !private_native_proxy_scope(&changed),
                "replay cannot fix real request target: {name}/{kind}"
            );
            let changed = native_mutate_owner(&faces, 2, &head, |body| {
                native_replay_in_owner(
                    &native_redirect_call(body, write_callee, "unrelated_stream.pretend_write"),
                    original_write,
                    kind,
                )
            });
            assert!(
                !private_native_proxy_scope(&changed),
                "replay cannot fix real write receiver: {name}/{kind}"
            );
        }
    }
    let management = "fn real_core_hot_switch_failure_waits_for_explicit_restart_confirmation(";
    let body = native_carrier_body(&faces[2], management).unwrap();
    // The legitimate descriptor and the true TCP probe deliberately share the
    // tuple. The exact code-active TCP target, not that bare tuple, stays unique.
    let (_, client) = native_owned_call(
        body,
        "letclient1=polaris_singbox_grpc::SingBoxApiClient::connect(",
    )
    .unwrap();
    assert!(native_same_call(
        client,
        r#"let client1 = polaris_singbox_grpc::SingBoxApiClient::connect(polaris_singbox_grpc::Endpoint::new("127.0.0.1", st.clash_api_port), FIXTURE_MANAGEMENT_SECRET.to_owned(),)"#
    ));
    assert!(native_owned_call(body, tcp).is_some());
    for replacement in [
        r#"tokio::net::TcpStream::connect(("127.0.0.1", mixed))"#,
        r#"tokio::net::TcpStream::connect(("0.0.0.0", st.clash_api_port))"#,
    ] {
        let changed = native_mutate_owner(&faces, 2, management, |body| {
            native_replace_call(body, tcp, replacement)
        });
        assert!(
            !private_native_proxy_scope(&changed),
            "actual management TCP purpose"
        );
    }
    for write in [
        "unrelated_stream.write_all(b\"GET\").await; let api_dead =",
        "tokio::io::AsyncWriteExt::write_all(&mut unrelated_stream, b\"GET\").await; let api_dead =",
    ] {
        let changed = native_mutate_owner(&faces, 2, management, |body| native_replace_fragment(body, "let api_dead =", write));
        assert!(!private_native_proxy_scope(&changed), "management primitive stays control-only/no-write");
    }
    let changed = native_mutate_owner(&faces, 2, management, |body| {
        native_replace_fragment(
            body,
            "let api_dead =",
            r#"tokio::net::TcpStream::connect(("127.0.0.1", st.clash_api_port)).await; let api_dead ="#,
        )
    });
    assert!(
        !private_native_proxy_scope(&changed),
        "one actual management TCP, despite the legitimate Endpoint descriptor"
    );
    // Both positives and negatives use all five actual current owners. Inert
    // original-looking comments/raw strings neither create a primitive nor fix it.
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS
        .iter()
        .copied()
        .chain(std::iter::once(
            "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation",
        ))
    {
        let head = format!("fn {name}(");
        let body = native_carrier_body(&faces[2], &head).unwrap();
        let (_, original_call) = native_owned_call(body, tcp).unwrap();
        let bad_calls =
            if name == "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation" {
                [
                    r#"tokio::net::TcpStream::connect(("0.0.0.0", st.clash_api_port))"#,
                    r#"tokio::net::TcpStream::connect(("127.0.0.1", mixed))"#,
                ]
            } else {
                [
                    r#"tokio::net::TcpStream::connect(("0.0.0.0", mixed))"#,
                    r#"tokio::net::TcpStream::connect(("127.0.0.1", st.clash_api_port))"#,
                ]
            };
        for kind in 0..3 {
            let positive = native_mutate_owner(&faces, 2, &head, |body| {
                native_replay_in_owner(body, original_call, kind)
            });
            assert!(
                private_native_proxy_scope(&positive),
                "inert replay cannot add a primitive: {name}/{kind}"
            );
            for bad in bad_calls {
                let changed = native_mutate_owner(&faces, 2, &head, |body| {
                    native_replay_in_owner(
                        &native_replace_call(body, tcp, bad),
                        original_call,
                        kind,
                    )
                });
                assert!(
                    !private_native_proxy_scope(&changed),
                    "inert replay cannot fix actual TCP: {name}/{kind}"
                );
            }
        }
    }
    for kind in 0..3 {
        let positive = native_mutate_owner(&faces, 2, management, |body| {
            native_replay_in_owner(body, "unrelated_stream.write_all(b\"GET\").await;", kind)
        });
        assert!(
            private_native_proxy_scope(&positive),
            "NoWrite ignores inert write text: {kind}"
        );
    }
    assert!(private_native_proxy_scope(&faces));
    let added = format!("{}\nasync fn unknown_proxy_caller() {{ tokio::net::TcpStream::connect((\"127.0.0.1\", 1)).await; }}", faces[2]);
    assert_ne!(added, faces[2]);
    assert_eq!(&added[..faces[2].len()], faces[2].as_str());
    assert!(!private_native_proxy_scope(&native_traffic_faces(
        &faces, 2, added
    )));
}

// Exercise the same consumer helpers on finite mutations of the real producer.
// These are source assertions; their authored cardinality is not an execution result.
#[test]
fn native_identifier_and_actual_body_ownership_controls() {
    let faces = native_scope_faces();
    assert!(private_native_proxy_scope(&faces));
    let tcp = r#"tokio::net::TcpStream::connect(("127.0.0.1", 61991))"#;
    let kinds = 0..3;
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let head = format!("fn {name}(");
        let receiver = if *name == "controlled_http_request" {
            "socket"
        } else {
            "c"
        };
        let callee = format!("{receiver}.write_all(");
        let original = native_carrier_body(&faces[2], &head).unwrap();
        let (_, original_write) = native_owned_call(original, &callee).unwrap();
        for prefix in ["é", "e\u{301}", "代理"] {
            let wrong_receiver = format!("{prefix}{receiver}");
            let mutate = |body: &str| {
                let redirected =
                    native_redirect_call(body, &callee, &format!("{wrong_receiver}.write_all"));
                assert!(redirected.starts_with('{'));
                format!(
                    "{{ let mut {wrong_receiver} = tokio::io::sink();{}",
                    &redirected[1..]
                )
            };
            let changed = native_mutate_owner(&faces, 2, &head, mutate);
            let body = native_carrier_body(&changed[2], &head).unwrap();
            assert!(
                native_one_primitive(body),
                "dial cardinality must not feed receiver denial"
            );
            assert!(
                native_owned_call(body, &callee).is_none(),
                "different complete Unicode identifier"
            );
            assert!(!native_proxy_request_scope(&changed[2], body, name));
            assert!(!private_native_proxy_scope(&changed));
            for kind in kinds.clone() {
                let positive = native_mutate_owner(&faces, 2, &head, |body| {
                    native_replay_in_owner(body, original_write, kind)
                });
                assert!(private_native_proxy_scope(&positive));
                let changed = native_mutate_owner(&faces, 2, &head, |body| {
                    native_replay_in_owner(&mutate(body), original_write, kind)
                });
                let body = native_carrier_body(&changed[2], &head).unwrap();
                assert!(native_one_primitive(body));
                assert!(native_owned_call(body, &callee).is_none());
                assert!(!native_proxy_request_scope(&changed[2], body, name));
                assert!(
                    !private_native_proxy_scope(&changed),
                    "inert replay cannot repair {wrong_receiver}/{kind}"
                );
            }
        }
    }

    let head = "fn drive_traffic_through_proxy(";
    // Every unknown item is inserted immediately after the real reviewed body.
    // Original known bodies remain byte-for-byte present. Closure ownership is
    // tested at the actual new primitive offset, independently of whole counts.
    for name in [
        "éunknown_proxy_caller",
        "e\u{301}unknown_proxy_caller",
        "未知出站",
        "drive_traffic_through_proxyé",
        "drive_traffic_through_proxy\u{301}",
    ] {
        for (item, expected) in [
            (format!("async fn {name}() {{ let _ = {tcp}.await; }}"), name.to_owned()),
            (format!("async fn /* masked gap */\n\t{name}<const N: usize>() {{ let _ = {tcp}.await; }}"), name.to_owned()),
            (format!("struct _Owner; impl _Owner {{ async fn {name}() {{ let _ = {tcp}.await; }} }}"), name.to_owned()),
        ] {
            let changed = native_mutate_owner(&faces, 2, head, |body| format!("{body}\n{item}"));
            assert_native_target_owner(&changed[2], tcp, &expected);
            assert!(!private_native_proxy_scope(&changed));
            for kind in 0..3 {
                let positive = native_mutate_owner(&faces, 2, head, |body| native_replay_in_owner(body, &item, kind));
                assert!(private_native_proxy_scope(&positive));
                let changed = native_mutate_owner(&faces, 2, head, |body| format!("{}\n{item}", native_replay_in_owner(body, &item, kind)));
                assert_native_target_owner(&changed[2], tcp, &expected);
                assert!(!private_native_proxy_scope(&changed), "inert sibling replay: {name}/{kind}");
            }
        }
        let changed = native_mutate_owner(&faces, 2, head, |body| {
            format!(
                "{{ async fn {name}() {{ let _ = {tcp}.await; }}{}",
                &body[1..]
            )
        });
        assert_native_target_owner(
            &changed[2],
            tcp,
            &format!("drive_traffic_through_proxy::{name}"),
        );
        assert!(!private_native_proxy_scope(&changed));
    }
    for item in [
        format!("static _OUTSIDE: fn() = || {{ let _ = {tcp}; }};"),
        format!("const _: () = {{ let _ = || {{ let _ = {tcp}; }}; }};"),
    ] {
        let changed = native_mutate_owner(&faces, 2, head, |body| format!("{body}\n{item}"));
        assert_native_target_owner(&changed[2], tcp, "");
        assert!(!private_native_proxy_scope(&changed));
        for kind in 0..3 {
            let positive = native_mutate_owner(&faces, 2, head, |body| {
                native_replay_in_owner(body, &item, kind)
            });
            assert!(private_native_proxy_scope(&positive));
            let changed = native_mutate_owner(&faces, 2, head, |body| {
                format!("{}\n{item}", native_replay_in_owner(body, &item, kind))
            });
            assert_native_target_owner(&changed[2], tcp, "");
            assert!(!private_native_proxy_scope(&changed));
        }
    }
    let alias = format!("struct _Sibling; impl _Sibling {{ async fn drive_traffic_through_proxy() {{ let _ = {tcp}.await; }} }}");
    let changed = native_mutate_owner(&faces, 2, head, |body| format!("{body}\n{alias}"));
    assert_native_target_owner(&changed[2], tcp, "");
    assert!(
        !private_native_proxy_scope(&changed),
        "duplicate named owners cannot share one registration"
    );

    let management = "fn real_core_hot_switch_failure_waits_for_explicit_restart_confirmation(";
    for write in [
        "let mut éstream = tokio::io::sink(); tokio::io::AsyncWriteExt::write_all(&mut éstream, b\"GET\").await;",
        "let mut e\u{301}stream = tokio::io::sink(); e\u{301}stream.write_all(b\"GET\").await;",
        "let mut 流 = tokio::io::sink(); 流.r#write_all(b\"GET\").await;",
        "let mut 流 = tokio::io::sink(); tokio::io::AsyncWriteExt::r#write_all(&mut 流, b\"GET\").await;",
    ] {
        let changed = native_mutate_owner(&faces, 2, management, |body| {
            native_replace_fragment(body, "let api_dead =", &format!("{write} let api_dead ="))
        });
        let body = native_carrier_body(&changed[2], management).unwrap();
        assert!(native_one_primitive(body));
        assert!(native_owned_call(body, "tokio::net::TcpStream::connect(").is_some());
        assert!(!native_management_probe_scope(&changed[2], body));
        assert!(!private_native_proxy_scope(&changed));
        for kind in 0..3 {
            let positive = native_mutate_owner(&faces, 2, management, |body| native_replay_in_owner(body, write, kind));
            assert!(private_native_proxy_scope(&positive), "NoWrite inert Unicode/raw call text");
        }
    }
}
fn assert_native_target_owner(source: &str, tcp: &str, expected: &str) {
    let span = native_fragment_span(source, &native_code(tcp))
        .expect("one actual active added TCP target");
    assert!(native_same_call(&source[span.clone()], tcp));
    let primitive = span.start + source[span].find("TcpStream::connect(").unwrap();
    let bare = polaris_source_probe::mask_comments_and_strings(source);
    assert!(rust_callee_start(&bare, primitive).is_some());
    assert_eq!(
        enclosing_fn(&bare, primitive),
        expected,
        "actual fullscan/native ownership route, not a count-only denial"
    );
}

#[test]
fn identifier_consumers_keep_qualified_calls_and_actual_function_scopes() {
    // Qualified/method calls and raw identifiers are real protected callees.
    for source in [
        "via_local_proxy(port, None)",
        "HttpRuntime::via_local_proxy(port, None)",
        "UnlockClient::via_local_proxy(port, None)",
        "object.via_local_proxy(port, None)",
        "HttpRuntime::r#via_local_proxy(port, None)",
        "object.r#via_local_proxy(port, None)",
    ] {
        let at = source.find("via_local_proxy(").unwrap();
        assert!(credentialed_call_start(source, at).is_some());
        let (args, _) = call_args(source, at + "via_local_proxy".len()).unwrap();
        assert_eq!(
            squash(&args[1]),
            "None",
            "real qualified credential argument stays visible"
        );
    }
    for prefix in ["é", "e\u{301}", "代理", "measure_"] {
        let source = format!("{prefix}via_local_proxy(port, None)");
        let at = source.find("via_local_proxy(").unwrap();
        assert!(credentialed_call_start(&source, at).is_none());
        assert!(!native_target_boundary(&source, at));
    }
    for source in [
        "fn via_local_proxy(port: u16) {}",
        "fn /* gap */\n\tvia_local_proxy(port: u16) {}",
        "fn r#via_local_proxy(port: u16) {}",
    ] {
        let bare = polaris_source_probe::mask_comments_and_strings(source);
        let at = bare.find("via_local_proxy(").unwrap();
        assert!(
            credentialed_call_start(&bare, at).is_none(),
            "actual definition is not a call"
        );
    }
    for name in ["plain", "éowner", "e\u{301}owner", "作用域", "r#raw_owner"] {
        let source = format!("fn earlier() {{}}\nimpl Scope {{ async fn /* gap */\n\t{name}<const N: usize>() {{ let action = || {{ let _ = TcpStream::connect(()); }}; }} }}");
        let bare = polaris_source_probe::mask_comments_and_strings(&source);
        let at = bare.find("TcpStream::connect(").unwrap();
        let expected = name.strip_prefix("r#").unwrap_or(name);
        assert_eq!(
            enclosing_fn(&bare, at),
            expected,
            "real closure inside actual associated fn"
        );
        let body = fn_body(&bare, expected).unwrap();
        assert!(body.contains("TcpStream::connect("));
        assert!(fn_body(&bare, "earlier").unwrap().ends_with("{}"));
        if expected != "plain" {
            assert!(fn_body(&bare, "owner").is_none());
        }
    }
    let source = "fn earlier() {}\ntrait Scope { fn declared(); }\nstatic OUT: fn() = || { let _ = TcpStream::connect(()); };";
    let at = source.find("TcpStream::connect(").unwrap();
    assert_eq!(
        enclosing_fn(source, at),
        "",
        "signature/closed sibling cannot donate a body"
    );
    assert!(fn_body(source, "declared").is_none());
    for kind in 0..3 {
        let original = "{ let _ = TcpStream::connect(()); }";
        let fake = "fn éowner() { let _ = TcpStream::connect(()); }";
        let source = format!(
            "fn actual() {}",
            native_replay_in_owner(original, fake, kind)
        );
        let bare = polaris_source_probe::mask_comments_and_strings(&source);
        let at = bare.find("TcpStream::connect(").unwrap();
        assert_eq!(enclosing_fn(&bare, at), "actual");
        assert!(
            fn_body(&bare, "éowner").is_none(),
            "inert function replay is not a definition"
        );
    }
    for source in [
        "stream.write_all(bytes)",
        "AsyncWriteExt::write_all(stream, bytes)",
        "流.write_all(bytes)",
        "return write_all(bytes)",
        "流.r#write_all(bytes)",
        "AsyncWriteExt::r#write_all(stream, bytes)",
    ] {
        let at = source.find("write_all(").unwrap();
        assert!(
            rust_callee_start(source, at).is_some(),
            "NoWrite keeps method/UFCS/raw calls"
        );
    }
    for source in [
        "stream.éwrite_all(bytes)",
        "stream.e\u{301}write_all(bytes)",
        "stream.写write_all(bytes)",
    ] {
        let at = source.find("write_all(").unwrap();
        assert!(
            rust_callee_start(source, at).is_none(),
            "different complete method name"
        );
    }
    // Bodyless, signature-brace and ambiguous marker cases must not borrow a
    // later/earlier basic_auth marker. Only the genuine parameter/body delimiter
    // is considered; existing impl-trait/generic/where shapes stay represented.
    let source = "trait Scope { fn target(); fn later() { basic_auth(); } }";
    assert!(fn_body(source, "target").is_none());
    let source = "fn target<const N: usize = { 1 }>() where [(); { N }]: Sized { basic_auth(); }";
    let body = fn_body(source, "target").unwrap();
    assert!(body.ends_with("{ basic_auth(); }"));
    let at = source.find("basic_auth(").unwrap();
    assert_eq!(enclosing_fn(source, at), "target");
    let source = "fn target<const N: usize = { basic_auth(); 1 }>() { TcpStream::connect(()); }";
    assert!(
        !fn_body(source, "target").unwrap().contains("basic_auth("),
        "signature expression is not credential evidence in actual body"
    );
    let source = "impl A { fn target() { basic_auth(); } } impl B { fn target() { TcpStream::connect(()); } }";
    assert!(fn_body(source, "target").is_none());
    assert_eq!(
        enclosing_fn(source, source.find("TcpStream::connect(").unwrap()),
        ""
    );
    let source = "fn outer() { fn nested() unsupported ? { let _ = TcpStream::connect(()); } }";
    assert_eq!(
        enclosing_fn(source, source.find("TcpStream::connect(").unwrap()),
        "",
        "unsupported named declaration cannot reopen ancestor"
    );
    let source = "fn unknown() unsupported ? { fn target() { let _ = TcpStream::connect(()); } }";
    assert_eq!(
        enclosing_fn(source, source.find("TcpStream::connect(").unwrap()),
        "",
        "unsupported parent cannot lend a registered inner name"
    );
    let source =
        "fn outer() { trait Scope { fn target<const N: usize = { TcpStream::connect(()) }>(); } }";
    assert_eq!(
        enclosing_fn(source, source.find("TcpStream::connect(").unwrap()),
        "",
        "a signature expression cannot borrow the containing function"
    );
    let source = "fn outer() { macro_rules! inner { () => { fn target() { let _ = TcpStream::connect(()); } } } }";
    assert!(fn_body(source, "target").is_none());
    assert_eq!(
        enclosing_fn(source, source.find("TcpStream::connect(").unwrap()),
        "",
        "macro-shaped fn is unsupported, not outer authority"
    );
    let source = "fn éfn_owner() { let _ = TcpStream::connect(()); }";
    assert_eq!(
        enclosing_fn(source, source.find("TcpStream::connect(").unwrap()),
        "éfn_owner"
    );
    assert!(fn_body(source, "owner").is_none());
    for name in [
        "éconnect_request",
        "connect_requesté",
        "connect_request\u{301}",
    ] {
        let source = format!("fn {name}() {{ basic_auth(); }}");
        assert!(
            fn_body(&source, "connect_request").is_none(),
            "a complete different Unicode name cannot donate credentials"
        );
        assert!(fn_body(&source, name).unwrap().contains("basic_auth("));
    }
    let source = "éfn via_local_proxy(port, None)";
    assert!(
        credentialed_call_start(source, source.find("via_local_proxy(").unwrap()).is_some(),
        "an arbitrary identifier ending in fn is not the keyword"
    );
    for marker in [
        "basic_auth(",
        "connect_request(auth)",
        "connectivity_probe_request(target_url, auth)",
        "proxy_authorization_line(auth)",
    ] {
        let original = format!(
            "{{ object.{marker}{}; }}",
            if marker.ends_with('(') { "auth)" } else { "" }
        );
        let original_source = format!("fn marker_owner() {original}");
        let original_body = fn_body(&original_source, "marker_owner").unwrap();
        assert!(credential_marker_in_body(
            &original_source,
            original_body,
            marker
        ));
        for prefix in ["é", "e\u{301}", "认证"] {
            let different = format!(
                "{{ object.{prefix}{marker}{}; }}",
                if marker.ends_with('(') { "auth)" } else { "" }
            );
            let different_source = format!("fn marker_owner() {different}");
            let different_body = fn_body(&different_source, "marker_owner").unwrap();
            assert!(
                !credential_marker_in_body(&different_source, different_body, marker),
                "different complete credential method"
            );
        }
        for kind in 0..3 {
            let inert = native_replay_in_owner("{ noop(); }", &original, kind);
            let inert_source = format!("fn marker_owner() {inert}");
            let inert_body = fn_body(&inert_source, "marker_owner").unwrap();
            assert!(
                !credential_marker_in_body(&inert_source, inert_body, marker),
                "inert marker cannot lend credentials"
            );
        }
    }
}

// Complete real-producer witnesses: definitions and uncalled nested functions
// are different named owners, even when the actual primitive count stays intact.
#[test]
fn protected_witnesses_stay_in_the_real_reviewed_named_owner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, owner, marker, callee, neutral, nested) in [
        (
            "src-tauri/src/runtime/http.rs",
            "with_local_proxy_url",
            "basic_auth(",
            "proxy.basic_auth(",
            "proxy",
            "fn unused_marker(proxy: reqwest::Proxy, user: &InboundUser) -> reqwest::Proxy",
        ),
        (
            "crates/unlock-transport/src/lib.rs",
            "via_local_proxy",
            "basic_auth(",
            "proxy.basic_auth(",
            "proxy",
            "fn unused_marker(proxy: wreq::Proxy, username: &str, password: &str) -> wreq::Proxy",
        ),
        (
            "src-tauri/src/runtime/speedtest_tunnel.rs",
            "open_tunnel_with_tls_config",
            "connect_request(auth)",
            "target.connect_request(",
            "String::new()",
            "fn unused_marker(target: &SpeedTestTarget, auth: Option<&InboundUser>) -> String",
        ),
        (
            "src-tauri/src/runtime/proxy/auto_switch.rs",
            "probe_through_proxy",
            "connectivity_probe_request(target_url, auth)",
            "connectivity_probe_request(",
            "Some(String::new())",
            "fn unused_marker(target_url: &str, auth: Option<&InboundUser>) -> Option<String>",
        ),
        (
            "src-tauri/src/runtime/speedtest_tunnel.rs",
            "connect_request",
            "proxy_authorization_line(auth)",
            "proxy_authorization_line(",
            "String::new()",
            "fn unused_marker(auth: Option<&InboundUser>) -> String",
        ),
    ] {
        let source = std::fs::read_to_string(root.join(path)).unwrap();
        let head = format!("fn {owner}(");
        let body = native_carrier_body(&source, &head).unwrap();
        assert!(credential_marker_in_body(&source, body, marker));
        let (_, original_call) = native_owned_call(body, callee).unwrap();
        let marker_name = marker.split('(').next().unwrap();
        let nested_item = (format!("{nested} {{ {original_call} }}"), false);
        let witnesses = if marker.ends_with('(') {
            vec![(format!("fn {marker_name}() {{}}"), true), nested_item]
        } else {
            vec![nested_item]
        };
        for (item, definition) in witnesses {
            for replay in 0..4 {
                // The actual unchanged producer positive is established before
                // every distinct mutation, including each inert replay variant.
                assert!(credential_marker_in_body(&source, body, marker));
                let without = native_replace_call(body, callee, neutral);
                let with_nested = format!("{{ {item}{}", &without[1..]);
                let changed_body = if replay == 0 {
                    with_nested
                } else {
                    native_replay_in_owner(&with_nested, original_call, replay - 1)
                };
                let offset = body.as_ptr() as usize - source.as_ptr() as usize;
                let changed =
                    native_replace_span(&source, offset..offset + body.len(), &changed_body);
                let changed_body = native_carrier_body(&changed, &head).unwrap();
                let before_code = polaris_source_probe::mask_comments_and_strings(body);
                let changed_code = polaris_source_probe::mask_comments_and_strings(changed_body);
                for primitive in PRIMITIVES {
                    assert_eq!(
                        before_code.matches(*primitive).count(),
                        changed_code.matches(*primitive).count(),
                        "auth control must preserve actual primitive/count: {path}"
                    );
                    for (at, _) in changed_code.match_indices(*primitive) {
                        let base = changed_body.as_ptr() as usize - changed.as_ptr() as usize;
                        let full = polaris_source_probe::mask_comments_and_strings(&changed);
                        assert_eq!(
                            enclosing_fn(&full, base + at),
                            owner,
                            "actual registered outer primitive retained"
                        );
                    }
                }
                let hits: Vec<_> = changed_code.match_indices(marker).collect();
                assert_eq!(
                    hits.len(),
                    1,
                    "one actual remaining marker candidate, inert replays excluded"
                );
                let at = hits[0].0;
                let base = changed_body.as_ptr() as usize - changed.as_ptr() as usize;
                let full = polaris_source_probe::mask_comments_and_strings(&changed);
                assert_eq!(
                    credentialed_call_start(&changed_code, at).is_none(),
                    definition
                );
                if !definition {
                    assert_eq!(
                        enclosing_fn(&full, base + at),
                        format!("{owner}::unused_marker")
                    );
                }
                assert!(!named_body_witness(&changed, changed_body, at));
                assert!(
                    !credential_marker_in_body(&changed, changed_body, marker),
                    "definition/nested real call cannot lend credentials: {path}/{replay}"
                );
            }
        }
        // Anonymous closure work remains in the same actual named producer.
        let wrapped = native_replace_call(body, callee, &format!("(|| {original_call})()"));
        let offset = body.as_ptr() as usize - source.as_ptr() as usize;
        let positive = native_replace_span(&source, offset..offset + body.len(), &wrapped);
        let positive_body = native_carrier_body(&positive, &head).unwrap();
        assert!(credential_marker_in_body(&positive, positive_body, marker));
        for kind in 0..3 {
            let replay = native_replay_in_owner(body, original_call, kind);
            let positive = native_replace_span(&source, offset..offset + body.len(), &replay);
            let positive_body = native_carrier_body(&positive, &head).unwrap();
            assert!(credential_marker_in_body(&positive, positive_body, marker));
        }
    }

    let faces = native_scope_faces();
    assert!(private_native_proxy_scope(&faces));
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let head = format!("fn {name}(");
        let body = native_carrier_body(&faces[2], &head).unwrap();
        let receiver = if *name == "controlled_http_request" {
            "socket"
        } else {
            "c"
        };
        let callee = format!("{receiver}.write_all(");
        let (write_span, write) = native_owned_call(body, &callee).unwrap();
        let completion = if *name == "drive_traffic_through_proxy" {
            ".await;"
        } else {
            ".await.unwrap();"
        };
        let (tail, offsets) = native_code_offsets(&body[write_span.end..]);
        assert!(tail.starts_with(completion));
        let end = write_span.end + offsets[completion.len() - 1] + 1;
        let start = if *name == "drive_traffic_through_proxy" {
            native_fragment_span(body, "let_=c.write_all(")
                .unwrap()
                .start
        } else {
            write_span.start
        };
        let statement = &body[start..end];
        let parameters = if *name == "controlled_http_request" {
            "mut socket: tokio::net::TcpStream, nonce: &str"
        } else if *name == "drive_traffic_through_proxy" {
            "mut c: tokio::net::TcpStream"
        } else {
            "mut c: tokio::net::TcpStream, req: String"
        };
        let nested = format!("async fn deferred_request({parameters}) {{ {write}{completion} }}");
        for replay in 0..4 {
            let changed = native_mutate_owner(&faces, 2, &head, |body| {
                let changed = native_replace_span(body, start..end, &nested);
                if replay == 0 {
                    changed
                } else {
                    native_replay_in_owner(&changed, statement, replay - 1)
                }
            });
            let body = native_carrier_body(&changed[2], &head).unwrap();
            assert!(
                native_one_primitive(body),
                "one actual outer TCP, no count preemption"
            );
            let (raw, _) = native_owned_call(body, &callee).unwrap();
            let base = body.as_ptr() as usize - changed[2].as_ptr() as usize;
            let full = polaris_source_probe::mask_comments_and_strings(&changed[2]);
            assert_eq!(
                enclosing_fn(&full, base + raw.start),
                format!("{name}::deferred_request")
            );
            assert!(!named_body_witness(&changed[2], body, raw.start));
            assert!(native_call_in_owner(&changed[2], body, &callee).is_none());
            assert!(!native_ordered(
                &changed[2],
                body,
                &["tokio::net::TcpStream::connect(", &callee]
            ));
            assert!(!native_proxy_request_scope(&changed[2], body, name));
            assert!(!private_native_proxy_scope(&changed));
        }
        // The same original write remains legitimate within awaited anonymous
        // async and for blocks, with original literals/completion intact.
        for wrapped in [
            format!("async {{ {write}{completion} }}.await;"),
            format!("for _ in 0..1 {{ {write}{completion} }}"),
        ] {
            let positive = native_mutate_owner(&faces, 2, &head, |body| {
                native_replace_span(body, start..end, &wrapped)
            });
            let body = native_carrier_body(&positive[2], &head).unwrap();
            assert!(native_call_in_owner(&positive[2], body, &callee).is_some());
            assert!(native_proxy_request_scope(&positive[2], body, name));
            assert!(private_native_proxy_scope(&positive));
        }
    }
}

#[test]
fn legal_rust_whitespace_keeps_real_owners_targets_and_none_arguments() {
    let whitespace = [
        '\u{0009}', '\u{000a}', '\u{000b}', '\u{000c}', '\u{000d}', ' ', '\u{0085}', '\u{200e}',
        '\u{200f}', '\u{2028}', '\u{2029}',
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let source = std::fs::read_to_string(root.join("src-tauri/src/runtime/http.rs")).unwrap();
    let head = "fn with_local_proxy_url(";
    let body = native_carrier_body(&source, head).unwrap();
    let marker = "basic_auth(";
    let extra = r#"reqwest::Proxy::all("http://127.0.0.1:61991")"#;
    for ws in whitespace {
        assert!(rust_whitespace(ws));
        for name in [
            "plain_unknown",
            "éunknown",
            "e\u{301}unknown",
            "未知",
            "r#raw_unknown",
        ] {
            assert!(credential_marker_in_body(&source, body, marker));
            let item = format!("{ws}fn{ws}{name}{ws}(){ws}where{ws}():{ws}Sized{ws}{{ let _ = {extra}; }} {name}();");
            let changed_body = format!("{{ {item}{}", &body[1..]);
            let base = body.as_ptr() as usize - source.as_ptr() as usize;
            let changed = native_replace_span(&source, base..base + body.len(), &changed_body);
            let changed_body = native_carrier_body(&changed, head).unwrap();
            assert!(
                credential_marker_in_body(&changed, changed_body, marker),
                "unrelated nested fn must not erase real outer auth"
            );
            let span = native_fragment_span(&changed, &native_code(extra)).unwrap();
            let full = polaris_source_probe::mask_comments_and_strings(&changed);
            let at = span.start + "reqwest::".len();
            assert_eq!(
                enclosing_fn(&full, at),
                format!(
                    "with_local_proxy_url::{}",
                    name.strip_prefix("r#").unwrap_or(name)
                ),
                "true nested primitive cannot borrow registered outer under any legal whitespace"
            );
            assert!(!named_body_witness(&changed, changed_body, at - base));
            if ws == '\u{0085}' {
                for kind in 0..3 {
                    let replay = native_replay_in_owner(body, &item, kind);
                    let positive = native_replace_span(&source, base..base + body.len(), &replay);
                    let positive_body = native_carrier_body(&positive, head).unwrap();
                    assert!(credential_marker_in_body(&positive, positive_body, marker));
                }
            }
        }
        let call = format!("return{ws}c.write_all(bytes)");
        assert!(
            native_owned_call(&call, "c.write_all(").is_some(),
            "do not glue separate return token to receiver"
        );
        for separator in [".", "::"] {
            let call = format!("holder{ws}{separator}{ws}c.write_all(bytes)");
            assert!(
                native_owned_call(&call, "c.write_all(").is_none(),
                "qualified suffix is still qualified across legal whitespace"
            );
        }
        let with_literal = format!("c.write_all(\"a{ws}b\".as_bytes())");
        assert_eq!(
            native_literal_faces(&with_literal),
            vec![format!("\"a{ws}b\"")]
        );
        assert!(
            !native_same_call(&with_literal, "c.write_all(\"ab\".as_bytes())"),
            "literal content must never be stripped by code whitespace handling"
        );
    }

    let faces = native_scope_faces();
    assert!(private_native_proxy_scope(&faces));
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let head = format!("fn {name}(");
        let receiver = if *name == "controlled_http_request" {
            "socket"
        } else {
            "c"
        };
        let callee = format!("{receiver}.write_all(");
        for gap in std::iter::once(String::new()).chain(whitespace.iter().map(ToString::to_string))
        {
            let changed = native_mutate_owner(&faces, 2, &head, |body| {
                let redirected = native_redirect_call(
                    body,
                    &callee,
                    &format!("holder{gap}.{gap}{receiver}.write_all"),
                );
                format!("{{ struct Holder {{ {receiver}: tokio::io::Sink }} let mut holder = Holder {{ {receiver}: tokio::io::sink() }};{}", &redirected[1..])
            });
            let body = native_carrier_body(&changed[2], &head).unwrap();
            assert!(native_one_primitive(body));
            let qualified = format!("holder.{receiver}.write_all(");
            let (actual, _) = native_owned_call(body, &qualified).unwrap();
            assert!(
                named_body_witness(&changed[2], body, actual.start),
                "same named owner does not establish local receiver identity"
            );
            assert!(native_call_in_owner(&changed[2], body, &qualified).is_some());
            assert!(native_call_in_owner(&changed[2], body, &callee).is_none());
            assert!(!native_proxy_request_scope(&changed[2], body, name));
            assert!(!private_native_proxy_scope(&changed));
        }
    }
    // A real existing credentialed second argument: legal LRM/RLM/etc around
    // literal None must take the exact same deny path as ordinary ASCII None.
    let source =
        std::fs::read_to_string(root.join("src-tauri/src/commands/misc/ipinfo.rs")).unwrap();
    let body = native_carrier_body(&source, "fn run_bound_ipinfo").unwrap();
    let (span, call) = native_owned_call(body, "HttpRuntime::via_local_proxy(").unwrap();
    assert!(native_same_call(
        call,
        "HttpRuntime::via_local_proxy(local_proxy.port, local_proxy.auth.as_ref())"
    ));
    for ws in whitespace {
        let replacement = format!("HttpRuntime::via_local_proxy(local_proxy.port, {ws}None{ws})");
        let base = body.as_ptr() as usize - source.as_ptr() as usize;
        let changed =
            native_replace_span(&source, base + span.start..base + span.end, &replacement);
        let code = polaris_source_probe::mask_comments_and_strings(&changed);
        let at = base + span.start + "HttpRuntime::".len();
        let call_start = credentialed_call_start(&code, at).unwrap();
        let (args, close) = call_args(&code, at + "via_local_proxy".len()).unwrap();
        assert_eq!(
            squash(&args[1]),
            "None",
            "all legal lexical separators preserve literal None"
        );
        assert_eq!(enclosing_fn(&code, at), "run_bound_ipinfo");
        let text = squash(&code[call_start..=close]);
        assert!(!NONE_AUTH_ALLOWED.iter().any(|(path, allowed, _)| *path
            == "src-tauri/src/commands/misc/ipinfo.rs"
            && *allowed == text));
    }
    let (allowed_path, allowed_text, _) = NONE_AUTH_ALLOWED[0];
    let source = std::fs::read_to_string(root.join(allowed_path)).unwrap();
    let span = native_fragment_span(&source, &native_code(allowed_text)).unwrap();
    assert_eq!(squash(&source[span.clone()]), allowed_text);
    for ws in whitespace {
        // Replace an existing token separator; do not broaden the exact call's
        // path, argument expressions or exemption table.
        let replacement = allowed_text.replacen("port, None,", &format!("port,{ws}None,"), 1);
        let changed = if &source[span.clone()] == replacement.as_str() {
            // The existing ASCII-space form is already the actual positive.
            source.clone()
        } else {
            native_replace_span(&source, span.clone(), &replacement)
        };
        let code = polaris_source_probe::mask_comments_and_strings(&changed);
        let at = span.start;
        let call_start = credentialed_call_start(&code, at).unwrap();
        let (args, close) = call_args(&code, at + "measure_via_local_proxy".len()).unwrap();
        assert_eq!(squash(&args[1]), "None");
        assert_eq!(squash(&code[call_start..=close]), allowed_text);
    }
    let source =
        std::fs::read_to_string(root.join("src-tauri/src/commands/misc/ipinfo.rs")).unwrap();
    let body = native_carrier_body(&source, "fn run_bound_ipinfo").unwrap();
    let base = body.as_ptr() as usize - source.as_ptr() as usize;
    let extra = "HttpRuntime::via_local_proxy(61992, \u{200e}None\u{200e})";
    let nested = format!(
        "{{ fn forbidden_nested() {{ let _ = {extra}; }}{}",
        &body[1..]
    );
    let changed = native_replace_span(&source, base..base + body.len(), &nested);
    let span = native_fragment_span(&changed, &native_code(extra)).unwrap();
    let code = polaris_source_probe::mask_comments_and_strings(&changed);
    let at = span.start + "HttpRuntime::".len();
    assert_eq!(
        enclosing_fn(&code, at),
        "run_bound_ipinfo::forbidden_nested"
    );
    assert!(
        credentialed_call_start(&code, at).is_some(),
        "global None scanner must retain different nested owner"
    );
    let (args, _) = call_args(&code, at + "via_local_proxy".len()).unwrap();
    assert_eq!(squash(&args[1]), "None");

    let faces = native_scope_faces();
    let management = "fn real_core_hot_switch_failure_waits_for_explicit_restart_confirmation(";
    let changed = native_mutate_owner(&faces, 2, management, |body| {
        format!("{{ async fn deferred_write(mut stream: tokio::net::TcpStream) {{ stream.write_all(b\"GET\").await; }}{}", &body[1..])
    });
    let body = native_carrier_body(&changed[2], management).unwrap();
    assert!(native_one_primitive(body));
    assert!(native_call_in_owner(&changed[2], body, "stream.write_all(").is_none());
    let bare = polaris_source_probe::mask_comments_and_strings(body);
    let at = bare.find("write_all(").unwrap();
    assert!(
        rust_callee_start(&bare, at).is_some(),
        "NoWrite retains actual nested method calls"
    );
    assert!(!native_management_probe_scope(&changed[2], body));
    assert!(!private_native_proxy_scope(&changed));
}

/// 带凭据构造器（名字, 凭据实参下标）。
const CREDENTIALED_CALLEES: &[(&str, usize)] = &[
    ("via_local_proxy", 1),
    ("via_local_socks_proxy", 1),
    ("measure_via_local_proxy", 1),
    ("measure_inbound", 3),
    ("measure_slot", 3),
    ("open_tunnel", 1),
    ("open_tunnel_with_tls_config", 1),
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

// Locate one named function in the existing masked source. Parentheses,
// brackets and const-generic braces are skipped with the existing delimiter;
// an item semicolon or closed containing item cannot donate a later body.
fn rust_named_header(code: &str, at: usize) -> Option<(&str, usize)> {
    let bytes = code.as_bytes();
    if !code[at..].starts_with("fn")
        || !rust_ident_boundary(code, at)
        || (at > 0 && bytes[at - 1] == b'#')
    {
        return None;
    }
    let mut cursor = at + 2;
    let gap = code[cursor..].chars().next()?;
    if !rust_whitespace(gap) {
        return None;
    }
    while code[cursor..].chars().next().is_some_and(rust_whitespace) {
        cursor += code[cursor..].chars().next()?.len_utf8();
    }
    if code[cursor..].starts_with("r#") {
        cursor += 2;
    }
    let name_start = cursor;
    if !rust_ident_start(*bytes.get(cursor)?) || rust_whitespace(code[cursor..].chars().next()?) {
        return None;
    }
    while let Some(ch) = code[cursor..].chars().next() {
        if !rust_ident_character(ch) {
            break;
        }
        cursor += ch.len_utf8();
    }
    Some((&code[name_start..cursor], cursor))
}
// A function-shaped token inside macro arguments/templates is not evidence of
// an actual declaration. Do not attempt expansion or lend its outer authority.
fn rust_in_macro_body(code: &str, at: usize) -> bool {
    let mut delimiters = Vec::new();
    for (offset, byte) in code.as_bytes()[..at].iter().enumerate() {
        match byte {
            b'(' | b'[' | b'{' => delimiters.push(offset),
            b')' | b']' | b'}' => {
                delimiters.pop();
            }
            _ => {}
        }
    }
    delimiters.into_iter().any(|open| {
        let before = code[..open].trim_end_matches(rust_whitespace);
        if before.ends_with('!') {
            return true;
        }
        let mut end = before.len();
        while end > 0 && rust_ident_continue(before.as_bytes()[end - 1]) {
            end -= 1;
        }
        before[..end]
            .trim_end_matches(rust_whitespace)
            .ends_with("macro_rules!")
    })
}
fn rust_named_declaration(
    code: &str,
    at: usize,
) -> Option<(&str, usize, Option<std::ops::Range<usize>>, usize)> {
    let (name, mut cursor) = rust_named_header(code, at)?;
    if rust_in_macro_body(code, at) {
        return None;
    }
    let bytes = code.as_bytes();
    let mut angles = 0usize;
    let parameters;
    loop {
        let ch = code.get(cursor..)?.chars().next()?;
        if rust_whitespace(ch) {
            cursor += ch.len_utf8();
            continue;
        }
        let byte = *bytes.get(cursor)?;
        match byte {
            b'<' => angles += 1,
            b'>' if cursor == 0 || bytes[cursor - 1] != b'-' => {
                angles = angles.checked_sub(1)?;
            }
            b'(' if angles == 0 => {
                parameters = cursor;
                break;
            }
            b'(' | b'[' | b'{' if angles > 0 => {
                cursor = call_args(code, cursor)?.1 + 1;
                continue;
            }
            _ if angles == 0 => return None,
            _ => {}
        }
        cursor += ch.len_utf8();
    }
    cursor = call_args(code, parameters)?.1 + 1;
    let tail = code[cursor..].trim_start_matches(rust_whitespace);
    if !tail.starts_with('{')
        && !tail.starts_with(';')
        && !tail.starts_with("->")
        && !(tail.starts_with("where")
            && tail.as_bytes().get(5).is_none_or(|b| {
                !rust_ident_continue(*b) || tail[5..].chars().next().is_some_and(rust_whitespace)
            }))
    {
        return None;
    }
    angles = 0;
    loop {
        let ch = code.get(cursor..)?.chars().next()?;
        if rust_whitespace(ch) {
            cursor += ch.len_utf8();
            continue;
        }
        match *bytes.get(cursor)? {
            b'(' | b'[' => {
                cursor = call_args(code, cursor)?.1 + 1;
                continue;
            }
            b'<' => angles += 1,
            b'>' if cursor == 0 || bytes[cursor - 1] != b'-' => {
                angles = angles.saturating_sub(1);
            }
            b'{' if angles > 0 => {
                cursor = call_args(code, cursor)?.1 + 1;
                continue;
            }
            b'{' => {
                let end = call_args(code, cursor)?.1 + 1;
                return Some((name, parameters, Some(cursor..end), end));
            }
            b';' if angles == 0 => return Some((name, parameters, None, cursor + 1)),
            b'}' if angles == 0 => return None,
            _ => {}
        }
        cursor += ch.len_utf8();
    }
}
fn rust_named_function(code: &str, at: usize) -> Option<(&str, usize, std::ops::Range<usize>)> {
    let (name, parameters, body, _) = rust_named_declaration(code, at)?;
    Some((name, parameters, body?))
}
fn rust_function_spans(code: &str) -> Vec<(usize, &str, std::ops::Range<usize>)> {
    code.match_indices("fn")
        .filter_map(|(at, _)| {
            let (name, _, body) = rust_named_function(code, at)?;
            Some((at, name, body))
        })
        .collect()
}
/// Actual containment, never the nearest previous sibling. Closures within a
/// real function retain its owner. A nested named fn gets a distinct identity;
/// associated functions retain their own name, not the preceding impl method.
fn enclosing_fn_with_spans(
    code: &str,
    pos: usize,
    functions: &[(usize, &str, std::ops::Range<usize>)],
) -> String {
    let mut owners: Vec<_> = functions
        .iter()
        .filter(|(_, _, body)| body.start < pos && pos < body.end - 1)
        .collect();
    owners.sort_by_key(|(_, _, body)| std::cmp::Reverse(body.end - body.start));
    if owners.is_empty() {
        return String::new();
    }
    // Unsupported named syntax within a containing body must not silently
    // disappear and reopen its known ancestor. A recognized bodyless signature
    // is distinct from an unsupported declaration and never lends a body.
    let innermost = owners.last().unwrap();
    let name = innermost.1;
    let mut same_name = 0usize;
    for (at, _) in code.match_indices("fn") {
        let Some((candidate, _)) = rust_named_header(code, at) else {
            continue;
        };
        same_name += usize::from(candidate == name);
        if at < pos {
            let declaration = functions
                .iter()
                .find(|(start, _, _)| *start == at)
                .map(|(_, name, body)| (*name, 0, Some(body.clone()), body.end))
                .or_else(|| rust_named_declaration(code, at));
            match declaration {
                Some((_, _, Some(body), _)) if pos <= body.start => return String::new(),
                Some((_, _, None, end)) if pos < end => return String::new(),
                None => {
                    let open_ancestor = at < innermost.0
                        && code.as_bytes()[at..innermost.0]
                            .iter()
                            .fold(0isize, |depth, byte| {
                                depth
                                    + match byte {
                                        b'{' => 1,
                                        b'}' => -1,
                                        _ => 0,
                                    }
                            })
                            > 0;
                    if innermost.2.start < at || open_ancestor {
                        return String::new();
                    }
                }
                _ => {}
            }
        }
    }
    if same_name != 1 {
        return String::new();
    }
    owners
        .iter()
        .map(|(_, name, _)| *name)
        .collect::<Vec<_>>()
        .join("::")
}
fn enclosing_fn(code: &str, pos: usize) -> String {
    let functions = rust_function_spans(code);
    enclosing_fn_with_spans(code, pos, &functions)
}
/// Unique actual named body; signature expressions cannot lend credential markers.
fn fn_body<'a>(code: &'a str, name: &str) -> Option<&'a str> {
    let declarations: Vec<_> = code
        .match_indices("fn")
        .filter_map(|(at, _)| {
            let (actual, _) = rust_named_header(code, at)?;
            (actual == name).then_some(at)
        })
        .collect();
    if declarations.len() != 1 {
        return None;
    }
    let at = declarations[0];
    let (_, _, body) = rust_named_function(code, at)?;
    Some(&code[body])
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
    s.split(rust_whitespace)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
                    || (path == NATIVE_OWNER
                        && PRIVATE_NATIVE_PROXY_FUNCTIONS.contains(&func.as_str()))
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
                credential_marker_in_body(code, body, marker),
                "{path} fn {func} 是带凭据构造路径，但函数体里没有挂凭据的 `{marker}`（{why}）"
            );
        }
    }
    for function in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        assert!(
            found
                .iter()
                .any(|(path, name)| path == NATIVE_OWNER && name == function),
            "private native proxy registration must still be present: {function}"
        );
    }
    // 两个「间接」凭据步骤本身也要在：CONNECT 报文与共用的头生成器。
    let tunnel = &surface
        .iter()
        .find(|(p, _)| p == "src-tauri/src/runtime/speedtest_tunnel.rs")
        .unwrap()
        .1;
    assert!(
        fn_body(tunnel, "connect_request").is_some_and(|b| credential_marker_in_body(
            tunnel,
            b,
            "proxy_authorization_line(auth)"
        )),
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
                let Some(call_start) = credentialed_call_start(code, at) else {
                    continue; // Includes Unicode/combining prefixes, not : or . qualifiers.
                };
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
                    let text = squash(&code[call_start..=close]);
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

// Actual native-owner controls for the shared mask's finite raw-prefix class.
// These are source mutations exercised only by the ordinary whole test gate;
// no alternate mask, runtime producer or synthetic source baseline is used.
#[test]
fn pattern11_raw_replays_cannot_supply_real_native_request_witnesses() {
    let whitespace = [
        '\u{0009}', '\u{000a}', '\u{000b}', '\u{000c}', '\u{000d}', ' ', '\u{0085}', '\u{200e}',
        '\u{200f}', '\u{2028}', '\u{2029}',
    ];
    let faces = native_scope_faces();
    for &name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let head = format!("fn {name}(");
        let original = native_carrier_body(&faces[2], &head).unwrap();
        let receiver = if name == "controlled_http_request" {
            "socket"
        } else {
            "c"
        };
        let callee = format!("{receiver}.write_all(");
        let (write_span, write) = native_owned_call(original, &callee).unwrap();
        let completion = if name == "drive_traffic_through_proxy" {
            ".await;"
        } else {
            ".await.unwrap();"
        };
        let (tail, offsets) = native_code_offsets(&original[write_span.end..]);
        assert!(tail.starts_with(completion));
        let start = if name == "drive_traffic_through_proxy" {
            native_fragment_span(original, "let_=c.write_all(")
                .unwrap()
                .start
        } else {
            write_span.start
        };
        let statement_span = start..write_span.end + offsets[completion.len() - 1] + 1;
        let statement = &original[statement_span.clone()];
        let (_, original_tcp) =
            native_owned_call(original, "tokio::net::TcpStream::connect(").unwrap();
        let request = if matches!(
            name,
            "real_core_full_lifecycle" | "real_core_aggregate_relay_emits_real_frames"
        ) {
            Some(native_owned_call(original, "format!(").unwrap().1)
        } else {
            None
        };
        let (binding, following) = match name {
            "drive_traffic_through_proxy" => {
                ("ifletOk(mutc)=tokio::net::TcpStream::connect(", "c.flush(")
            }
            "controlled_http_request" => (
                "letmutsocket=tokio::net::TcpStream::connect(",
                "socket.take(",
            ),
            "real_core_full_lifecycle" => ("letmutc=tokio::net::TcpStream::connect(", "c.read("),
            "real_core_aggregate_relay_emits_real_frames" => {
                ("letmutc=tokio::net::TcpStream::connect(", "holds.push(")
            }
            _ => panic!("finite actual native owner"),
        };
        let (_, original_following) = native_owned_call(original, following).unwrap();
        let mut anchors = vec![binding];
        if request.is_some() {
            anchors.push("letreq=format!(");
        }
        anchors.push(&callee);
        for ws in whitespace {
            for prefix in ["r", "br"] {
                // Positive first for every separator/prefix: preserve the real
                // TCP, actual request form and later flush/read/held stream.
                assert!(private_native_proxy_scope(&faces));
                assert!(native_one_primitive(original));
                assert!(native_call_in_owner(&faces[2], original, &callee).is_some());
                assert!(native_ordered(&faces[2], original, &anchors));
                assert!(native_proxy_request_scope(&faces[2], original, name));
                let literal = format!("{prefix}####\"a\"; {statement} \"b\"####");
                let keep_req = if request.is_some() {
                    " let _ = &req;"
                } else {
                    ""
                };
                let inert = format!("let _native_inert_write = {ws}{literal};{keep_req}");
                // A second, inert copy cannot erase the genuine original call.
                let positive = native_mutate_owner(&faces, 2, &head, |body| {
                    native_replace_span(
                        body,
                        statement_span.clone(),
                        &format!("{inert}{statement}"),
                    )
                });
                let positive_body = native_carrier_body(&positive[2], &head).unwrap();
                assert!(native_one_primitive(positive_body));
                assert!(native_owned_call(positive_body, &callee).is_some());
                assert!(native_call_in_owner(&positive[2], positive_body, &callee).is_some());
                assert!(native_proxy_request_scope(
                    &positive[2],
                    positive_body,
                    name
                ));
                assert!(private_native_proxy_scope(&positive));

                // Only the original completed write moves into the raw data,
                // including drive_traffic's original let _ = statement prefix.
                // Interior quotes distinguish raw from ordinary quote scans.
                let changed = native_mutate_owner(&faces, 2, &head, |body| {
                    native_replace_span(body, statement_span.clone(), &inert)
                });
                let body = native_carrier_body(&changed[2], &head).unwrap();
                assert_eq!(
                    &body[..statement_span.start],
                    &original[..statement_span.start]
                );
                assert_eq!(
                    &body[statement_span.start + inert.len()..],
                    &original[statement_span.end..]
                );
                assert!(
                    native_one_primitive(body),
                    "retain one original actual outer TCP"
                );
                assert_eq!(
                    native_owned_call(body, "tokio::net::TcpStream::connect(")
                        .unwrap()
                        .1,
                    original_tcp
                );
                if let Some(request) = request {
                    assert_eq!(native_owned_call(body, "format!(").unwrap().1, request);
                }
                assert_eq!(
                    native_owned_call(body, following).unwrap().1,
                    original_following
                );
                let bare = polaris_source_probe::mask_comments_and_strings(body);
                let visible = polaris_source_probe::mask_comments(body);
                assert!(visible.contains(&literal));
                assert!(!native_code(body).contains(&callee));
                assert_eq!(bare.len(), body.len());
                assert_eq!(visible.len(), body.len());
                let following_at = body.find(following).unwrap();
                assert_eq!(bare.find(following), Some(following_at));
                assert_eq!(visible.find(following), Some(following_at));
                assert_eq!(
                    bare.match_indices('\n')
                        .map(|(at, _)| at)
                        .collect::<Vec<_>>(),
                    body.match_indices('\n')
                        .map(|(at, _)| at)
                        .collect::<Vec<_>>()
                );
                assert!(native_owned_call(body, &callee).is_none());
                assert!(native_fragment_span(body, &callee).is_none());
                assert!(native_call_in_owner(&changed[2], body, &callee).is_none());
                assert!(!native_ordered(&changed[2], body, &anchors));
                assert!(!native_proxy_request_scope(&changed[2], body, name));
                assert!(
                    !private_native_proxy_scope(&changed),
                    "raw data cannot supply a write: {name}/{ws:?}/{prefix}"
                );
                // Actual ASCII/nonASCII counterparts must have identical
                // code/literal faces and reach the same rejection branches.
                let ascii_inert = format!("let _native_inert_write = {literal};{keep_req}");
                let ascii = native_mutate_owner(&faces, 2, &head, |body| {
                    native_replace_span(body, statement_span.clone(), &ascii_inert)
                });
                let ascii_body = native_carrier_body(&ascii[2], &head).unwrap();
                assert_eq!(native_code(body), native_code(ascii_body));
                assert_eq!(native_literal_faces(body), native_literal_faces(ascii_body));
                assert!(native_one_primitive(ascii_body));
                assert!(native_owned_call(ascii_body, &callee).is_none());
                assert!(native_call_in_owner(&ascii[2], ascii_body, &callee).is_none());
                assert!(!native_ordered(&ascii[2], ascii_body, &anchors));
                assert!(!native_proxy_request_scope(&ascii[2], ascii_body, name));
                assert!(!private_native_proxy_scope(&ascii));
            }
        }
        // Genuine anonymous async/for work retains the completed original call
        // and following real flush/read/hold in every existing native owner.
        for wrapped in [
            format!("async {{ {write}{completion} }}.await;"),
            format!("for _ in 0..1 {{ {write}{completion} }}"),
        ] {
            let positive = native_mutate_owner(&faces, 2, &head, |body| {
                native_replace_span(body, statement_span.clone(), &wrapped)
            });
            let body = native_carrier_body(&positive[2], &head).unwrap();
            assert!(native_one_primitive(body));
            assert!(native_call_in_owner(&positive[2], body, &callee).is_some());
            assert!(native_proxy_request_scope(&positive[2], body, name));
            assert!(private_native_proxy_scope(&positive));
        }
    }
}

#[test]
fn pattern11_raw_replays_keep_marker_nowrite_and_global_none_contracts_distinct() {
    let whitespace = [
        '\u{0009}', '\u{000a}', '\u{000b}', '\u{000c}', '\u{000d}', ' ', '\u{0085}', '\u{200e}',
        '\u{200f}', '\u{2028}', '\u{2029}',
    ];
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    for (path, owner, marker, callee, neutral) in [
        (
            "src-tauri/src/runtime/http.rs",
            "with_local_proxy_url",
            "basic_auth(",
            "proxy.basic_auth(",
            "proxy",
        ),
        (
            "crates/unlock-transport/src/lib.rs",
            "via_local_proxy",
            "basic_auth(",
            "proxy.basic_auth(",
            "proxy",
        ),
        (
            "src-tauri/src/runtime/speedtest_tunnel.rs",
            "open_tunnel_with_tls_config",
            "connect_request(auth)",
            "target.connect_request(",
            "String::new()",
        ),
        (
            "src-tauri/src/runtime/proxy/auto_switch.rs",
            "probe_through_proxy",
            "connectivity_probe_request(target_url, auth)",
            "connectivity_probe_request(",
            "Some(String::new())",
        ),
        (
            "src-tauri/src/runtime/speedtest_tunnel.rs",
            "connect_request",
            "proxy_authorization_line(auth)",
            "proxy_authorization_line(",
            "String::new()",
        ),
    ] {
        let source = std::fs::read_to_string(root.join(path)).unwrap();
        let head = format!("fn {owner}(");
        let body = native_carrier_body(&source, &head).unwrap();
        let (_, original_call) = native_owned_call(body, callee).unwrap();
        let base = body.as_ptr() as usize - source.as_ptr() as usize;
        for ws in whitespace {
            for prefix in ["r", "br"] {
                assert!(credential_marker_in_body(&source, body, marker));
                let literal = format!("{prefix}####\"a\"; {original_call}; \"b\"####");
                let inert = format!("let _native_inert_marker = {ws}{literal};");
                let positive_body = format!("{{ {inert}{}", &body[1..]);
                let positive =
                    native_replace_span(&source, base..base + body.len(), &positive_body);
                let positive_body = native_carrier_body(&positive, &head).unwrap();
                assert!(credential_marker_in_body(&positive, positive_body, marker));
                let without = native_replace_call(body, callee, neutral);
                let changed_body = format!("{{ {inert}{}", &without[1..]);
                let changed = native_replace_span(&source, base..base + body.len(), &changed_body);
                let changed_body = native_carrier_body(&changed, &head).unwrap();
                assert!(polaris_source_probe::mask_comments(changed_body).contains(&literal));
                assert!(
                    !polaris_source_probe::mask_comments_and_strings(changed_body).contains(marker)
                );
                assert!(native_owned_call(changed_body, callee).is_none());
                assert!(!credential_marker_in_body(&changed, changed_body, marker));
                for primitive in PRIMITIVES {
                    assert_eq!(
                        native_code(body).matches(*primitive).count(),
                        native_code(changed_body).matches(*primitive).count()
                    );
                }
            }
        }
    }

    let faces = native_scope_faces();
    let management = "fn real_core_hot_switch_failure_waits_for_explicit_restart_confirmation(";
    for ws in whitespace {
        for prefix in ["r", "br"] {
            assert!(private_native_proxy_scope(&faces));
            let original = native_carrier_body(&faces[2], management).unwrap();
            assert!(native_management_probe_scope(&faces[2], original));
            let fake = "stream.write_all(b\"GET\").await;";
            let literal = format!("{prefix}####\"a\"; {fake} \"b\"####");
            let inert = format!("let _native_inert_nowrite = {ws}{literal};");
            let positive = native_mutate_owner(&faces, 2, management, |body| {
                format!("{{ {inert}{}", &body[1..])
            });
            let body = native_carrier_body(&positive[2], management).unwrap();
            assert!(native_one_primitive(body));
            assert!(!native_code(body).contains("write_all("));
            assert!(polaris_source_probe::mask_comments(body).contains(&literal));
            assert!(native_management_probe_scope(&positive[2], body));
            assert!(private_native_proxy_scope(&positive));
            // A real method write in the same actual owner still violates
            // NoWrite even when an inert copy of it is present beside it.
            let changed = native_mutate_owner(&positive, 2, management, |body| {
                format!(
                    "{{ let mut stream = tokio::io::sink(); {fake}{}",
                    &body[1..]
                )
            });
            let body = native_carrier_body(&changed[2], management).unwrap();
            assert!(native_one_primitive(body));
            assert!(native_owned_call(body, "stream.write_all(").is_some());
            let bare = polaris_source_probe::mask_comments_and_strings(body);
            let at = bare.find("write_all(").unwrap();
            assert!(rust_callee_start(&bare, at).is_some());
            assert!(!native_management_probe_scope(&changed[2], body));
            assert!(!private_native_proxy_scope(&changed));
        }
    }

    let path = "src-tauri/src/commands/misc/ipinfo.rs";
    let source = std::fs::read_to_string(root.join(path)).unwrap();
    let head = "fn run_bound_ipinfo";
    let body = native_carrier_body(&source, head).unwrap();
    let base = body.as_ptr() as usize - source.as_ptr() as usize;
    let (span, actual) = native_owned_call(body, "HttpRuntime::via_local_proxy(").unwrap();
    let expected = "HttpRuntime::via_local_proxy(local_proxy.port, local_proxy.auth.as_ref())";
    for ws in whitespace {
        for prefix in ["r", "br"] {
            assert!(native_same_call(actual, expected));
            let fake = "HttpRuntime::via_local_proxy(61992, None)";
            let literal = format!("{prefix}####\"a\"; {fake}; \"b\"####");
            let inert = format!("let _native_inert_none = {ws}{literal};");
            let positive_body = format!("{{ {inert}{}", &body[1..]);
            let positive = native_replace_span(&source, base..base + body.len(), &positive_body);
            let positive_body = native_carrier_body(&positive, head).unwrap();
            assert!(native_same_call(
                native_owned_call(positive_body, "HttpRuntime::via_local_proxy(")
                    .unwrap()
                    .1,
                expected
            ));
            let code = polaris_source_probe::mask_comments_and_strings(&positive);
            assert!(
                !code.contains(fake),
                "raw None cannot become a global negative call"
            );
            assert!(polaris_source_probe::mask_comments(&positive).contains(&literal));
            // The real call's literal None remains visible to the existing
            // global negative scanner; no positive named-owner restriction is
            // added and the one exact temporary-core exception is unchanged.
            let changed_body = native_replace_span(
                body,
                span.clone(),
                "HttpRuntime::via_local_proxy(local_proxy.port, None)",
            );
            let changed_body = format!("{{ {inert}{}", &changed_body[1..]);
            let changed = native_replace_span(&source, base..base + body.len(), &changed_body);
            let changed_body = native_carrier_body(&changed, head).unwrap();
            let (call_span, _) =
                native_owned_call(changed_body, "HttpRuntime::via_local_proxy(").unwrap();
            let start = named_body_offset(&changed, changed_body).unwrap() + call_span.start;
            let code = polaris_source_probe::mask_comments_and_strings(&changed);
            let at = start + "HttpRuntime::".len();
            let call_start = credentialed_call_start(&code, at).unwrap();
            let (args, close) = call_args(&code, at + "via_local_proxy".len()).unwrap();
            assert_eq!(squash(&args[1]), "None");
            assert_eq!(enclosing_fn(&code, at), "run_bound_ipinfo");
            let text = squash(&code[call_start..=close]);
            assert!(!NONE_AUTH_ALLOWED
                .iter()
                .any(|(allowed_path, allowed, _)| *allowed_path == path && *allowed == text));
        }
    }
}
