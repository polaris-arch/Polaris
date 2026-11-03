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
        .filter(|c| !c.is_whitespace())
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
    let open = start + bare[start..].find('{')?;
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
fn native_ordered(body: &str, anchors: &[&str]) -> bool {
    let code = native_code(body);
    let mut previous = None;
    for anchor in anchors {
        if code.matches(anchor).count() != 1 {
            return false;
        }
        let at = code.find(anchor).unwrap();
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
        && native_ordered(preflight, &[
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
        && native_ordered(lock, &["letf=matchstd::fs::symlink_metadata(root.join(name))", "f.try_lock()", "Ok(f)"])
        && native_code(lock).contains("f.try_lock().map_err(|e|format!())?;")
        && native_code(lock).contains("Err(e)ife.kind()==std::io::ErrorKind::NotFound=>create_file(&root.join(name))?,Err(e)=>returnErr(e.to_string()),")
        && native_code(owner).contains("crate::runtime::core_identity::selected_manifest(&platform)?;")
        && native_code(owner).contains("caller:caller()?,platform,core_id:id.as_str().into(),provider_fingerprint:m.provider_fingerprint,core_files:m.files,")
        && native_code(owner).contains("helper_sha256:m.helper_sha256,cleaner_sha256:m.cleaner_sha256,required_cases:CASE_NAMES.iter().map(|v|(*v).into()).collect(),")
        && native_code(selection)=="{letmuta=a.clone();a.session_id.clear();a==*b}"
        && s.contains("pub(super)fnhold_session()->Result<(),String>{")
        && native_code(hold).starts_with("{capability_preflight()?;")
        && native_ordered(hold, &["capability_preflight()?;", "letmutowner=selected_owner()", "letroot=root()", "letdir=private_directory(&root)", "let_suite=lock_file(&root,&dir,SUITE_LOCK)", "ifstd::fs::symlink_metadata(root.join(OWNER)).is_ok()", "letbefore=retained_size(&root)", "letready=provider_ready(&root);", "ifready.is_ok(){record(", "std::io::stdout().flush()", "letcase_lock=lock_file(&root,&dir,CASE_LOCK);", "letendpoint_idle=provider_idle(&root).ok();"])
        && native_code(hold).contains("ifstd::fs::symlink_metadata(root.join(OWNER)).is_ok(){returnErr(")
        && native_code(hold).contains("letmutdrained=case_lock.is_ok();")
        && native_compact(hold).contains("drained&=matches!((a,b),(Ok(a),Ok(b))ifa==b&&a[\"sessionId\"]==owner.session_id&&a[\"case\"]==CASE_NAMES[i]);")
        && native_compact(hold).contains("foriin0..CASE_NAMES.len(){create_directory(&root.join(format!(\"case-{i}\")))?;}")
        && native_code(hold).contains("record(&root.join(OWNER),&serde_json::to_value(&owner).map_err(|e|e.to_string())?,)?;")
        && native_code(hold).contains("letok=action_ok&&drained&&(started==0||endpoint_idle==Some(true))&&retained.is_ok();")
        && s.contains("pub(super)fnopen(case:&str)->Result<Self,String>{")
        && open_code.starts_with("{capability_preflight()?;")
        && native_ordered(open, &["capability_preflight()?;", "leti=CASE_NAMES.iter().position(|v|*v==case)", "letroot=root()?;", "letdirectory=private_directory(&root)?;", "letsuite=directory.open_regular(SUITE_LOCK)", "matchsuite.try_lock()", "letcase_lock=lock_file(&root,&directory,CASE_LOCK)?;", "retained_size(&root)?;", "letowner:Owner=serde_json::from_value(read_record(&directory,OWNER)?)", "ifuuid::Uuid::parse_str(&owner.session_id).is_err()||!matches_selection(&owner,&selected_owner()?)", "provider_ready(&root)", "letcase_dir=root.join(format!(", "private_directory(&case_dir)?;", "record(&root.join(format!(", "Ok(Self{"])
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
    if !native_ordered(caught, &["letfuture=factory();", "future.await;"])
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
        && native_ordered(drain,&["letmain=main.await;","letchecks=checks.await;","letmutfailures=Vec::new();"])
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

fn private_native_proxy_scope(faces: &[String; 7]) -> bool {
    if !native_scope_is_closed(faces) {
        return false;
    }
    let carrier = &faces[2];
    let compact = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    let bare = polaris_source_probe::mask_comments_and_strings(carrier);
    let text = polaris_source_probe::mask_comments(carrier);
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let Some(body) = fn_body(&bare, name) else {
            return false;
        };
        let start = body.as_ptr() as usize - bare.as_ptr() as usize;
        let source = compact(&text[start..start + body.len()]);
        if PRIMITIVES
            .iter()
            .map(|primitive| body.matches(*primitive).count())
            .sum::<usize>()
            != 1
            || !source.contains("tokio::net::TcpStream::connect((\"127.0.0.1\",mixed))")
            || !match *name {
                "drive_traffic_through_proxy" => {
                    source.contains("ifletOk(mutc)=tokio::net::TcpStream::connect(")
                        && source.contains("c.write_all(b\"GEThttp://192.0.2.1/HTTP/1.1")
                }
                "controlled_http_request" => {
                    source.contains("letmutsocket=tokio::net::TcpStream::connect(")
                        && source.contains(
                            "socket.write_all(format!(\"GEThttp://192.0.2.1/{nonce}HTTP/1.1",
                        )
                }
                "real_core_full_lifecycle" | "real_core_aggregate_relay_emits_real_frames" => {
                    source.contains("letmutc=tokio::net::TcpStream::connect(")
                        && source
                            .contains("letreq=format!(\"GEThttp://127.0.0.1:{srv_port}/HTTP/1.1")
                        && source.contains("c.write_all(req.as_bytes()).await.unwrap();")
                }
                _ => false,
            }
        {
            return false;
        }
    }
    let name = "real_core_hot_switch_failure_waits_for_explicit_restart_confirmation";
    for primitive in PRIMITIVES {
        for (at, _) in bare.match_indices(*primitive) {
            let owner = enclosing_fn(&bare, at);
            if !PRIVATE_NATIVE_PROXY_FUNCTIONS.contains(&owner.as_str()) && owner != name {
                return false;
            }
        }
    }
    let Some(body) = fn_body(&bare, name) else {
        return false;
    };
    let start = body.as_ptr() as usize - bare.as_ptr() as usize;
    let control = compact(&text[start..start + body.len()]);
    control.contains("tokio::net::TcpStream::connect((\"127.0.0.1\",st.clash_api_port))")
        && control.contains("letapi_dead=")
        && control.contains(".is_err();")
        && PRIMITIVES
            .iter()
            .map(|primitive| body.matches(*primitive).count())
            .sum::<usize>()
            == 1
        && !body.contains(".write_all(")
}

fn native_traffic_faces(faces: &[String; 7], slot: usize, source: String) -> [String; 7] {
    let mut changed = (*faces).clone();
    changed[slot] = source;
    changed
}

#[test]
fn native_proxy_traffic_and_management_probe_keep_distinct_private_purposes() {
    let faces = native_scope_faces();
    let proxy = &faces[0];
    let carrier = &faces[2];
    assert_native_scope_controls(&faces);
    assert!(private_native_proxy_scope(&faces));
    for (old, new) in [
        ("\"127.0.0.1\", mixed", "\"0.0.0.0\", mixed"),
        (".write_all(b\"GET", ".pretend_write(b\"GET"),
        (
            "tokio::net::TcpStream::connect((\"127.0.0.1\", st.clash_api_port))",
            "tokio::net::TcpStream::connect((\"127.0.0.1\", mixed))",
        ),
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
        assert!(
            carrier.contains(old),
            "negative must mutate actual native traffic: {old}"
        );
        assert!(
            !private_native_proxy_scope(&native_traffic_faces(
                &faces,
                2,
                carrier.replacen(old, new, 1)
            )),
            "private native purpose regression was not denied: {old}"
        );
    }
    let policy = "super::core_binary::dev_core_binary_override().map(drop)?;";
    let constructor = "let config = Arc::new(ConfigManager::new(dir.clone()));";
    assert_eq!(carrier.matches(policy).count(), 1);
    assert_eq!(carrier.matches(constructor).count(), 1);
    let late_policy = carrier.replacen(policy, "", 1).replacen(
        constructor,
        &format!("{constructor}\n    {policy}"),
        1,
    );
    assert!(!private_native_proxy_scope(&native_traffic_faces(
        &faces,
        2,
        late_policy
    )));
    let guard = "#[cfg(debug_assertions)]\n#[path = \"../native_verification/proxy_carrier.rs\"]";
    assert!(proxy.contains(guard));
    assert!(!private_native_proxy_scope(&native_traffic_faces(
        &faces,
        0,
        proxy.replacen(
            guard,
            "#[path = \"../native_verification/proxy_carrier.rs\"]",
            1
        )
    )));
    for name in PRIVATE_NATIVE_PROXY_FUNCTIONS {
        let head = format!("fn {name}(");
        let body = native_carrier_body(carrier, &head).expect("actual proxy request owner");
        let write = match *name {
            "drive_traffic_through_proxy" => "c\n            .write_all(",
            "controlled_http_request" => "socket.write_all(",
            _ => "c.write_all(req.as_bytes())",
        };
        for (old, new) in [
            ("\"127.0.0.1\", mixed", "\"0.0.0.0\", mixed"),
            ("\"127.0.0.1\", mixed", "\"127.0.0.1\", st.clash_api_port"),
            (write, "unrelated_stream.pretend_write("),
        ] {
            assert!(private_native_proxy_scope(&faces));
            assert_eq!(body.matches(old).count(), 1, "{name}: {old}");
            let changed = carrier.replacen(body, &body.replacen(old, new, 1), 1);
            assert!(
                !private_native_proxy_scope(&native_traffic_faces(&faces, 2, changed)),
                "actual proxy request purpose escaped: {name}: {old}"
            );
        }
        assert!(private_native_proxy_scope(&faces));
        let duplicate = body.replacen(
            '{',
            "{ tokio::net::TcpStream::connect((\"127.0.0.1\", mixed)).await;",
            1,
        );
        let changed = carrier.replacen(body, &duplicate, 1);
        assert!(
            !private_native_proxy_scope(&native_traffic_faces(&faces, 2, changed)),
            "each request owner must keep exactly its one reviewed primitive: {name}"
        );
    }
    let control = native_carrier_body(
        carrier,
        "fn real_core_hot_switch_failure_waits_for_explicit_restart_confirmation(",
    )
    .unwrap();
    for (old, new) in [
        ("\"127.0.0.1\", st.clash_api_port", "\"127.0.0.1\", mixed"),
        (
            "\"127.0.0.1\", st.clash_api_port",
            "\"0.0.0.0\", st.clash_api_port",
        ),
        (
            "let api_dead =",
            "unrelated_stream.write_all(b\"GET\").await; let api_dead =",
        ),
    ] {
        assert!(private_native_proxy_scope(&faces));
        assert_eq!(control.matches(old).count(), 1);
        let changed = carrier.replacen(control, &control.replacen(old, new, 1), 1);
        assert!(
            !private_native_proxy_scope(&native_traffic_faces(&faces, 2, changed)),
            "the API probe must remain loopback/control-only/no-write: {old}"
        );
    }
    let added = format!("{carrier}\nasync fn unknown_proxy_caller() {{ tokio::net::TcpStream::connect((\"127.0.0.1\", 1)).await; }}");
    assert!(!private_native_proxy_scope(&native_traffic_faces(
        &faces, 2, added
    )));
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
                body.contains(marker),
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
