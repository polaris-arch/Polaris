//! Explicit acceptance-only normal-library carrier. No cfg(test) fixture or authority injection.
use super::auto_switch::*;
use super::hot_switch::*;
use super::*;
use crate::runtime::auto_switch::RuntimeCandidate;
use crate::runtime::stats::relay::{build_aggregate, now_ms, signature_changed};
use futures::{FutureExt, StreamExt};
use polaris_config_engine::builder::mesh_mode::DASHBOARD_SELECTOR;
use polaris_core_supervisor::Signal;
use polaris_singbox_grpc::{Endpoint, SingBoxApiClient};
use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
#[path = "cleanup.rs"]
mod cleanup;
const FIXTURE_MANAGEMENT_SECRET: &str = "fixture-management-secret";
static RUNTIME: OnceLock<Arc<ProxyRuntime>> = OnceLock::new();
static DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
async fn lock_real_core_tests() -> tokio::sync::MutexGuard<'static, ()> {
    LOCK.lock().await
}
async fn wait_pid_change(runtime: &Arc<ProxyRuntime>, old_pid: u32, secs: u64) -> Option<u32> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    while tokio::time::Instant::now() < deadline {
        let status = runtime.status();
        if status.running && status.pid != 0 && status.pid != old_pid {
            return Some(status.pid);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    None
}

fn real_core_runtime() -> (Arc<ProxyRuntime>, PathBuf, PathBuf, ()) {
    let runtime = RUNTIME
        .get()
        .expect("driver owns production runtime")
        .clone();
    let dir = DIRECTORY
        .get()
        .expect("driver owns acceptance data directory")
        .clone();
    let path = if super::startup::should_start_via_helper(
        polaris_config_engine::user_config::ProxyModeType::Manual,
        runtime.helper.platform(),
    ) {
        super::core_binary::resolve_helper_candidate_data().map(|data| data.into_path())
    } else {
        super::core_binary::resolve_core_binary()
    }
    .expect("compiled selected candidate unavailable");
    (runtime, dir, path, ())
}
fn send_signal(pid: u32, _signal: polaris_core_supervisor::Signal) {
    let runtime = RUNTIME.get().expect("owned runtime");
    assert_eq!(
        *runtime.pid.lock().unwrap(),
        Some(pid),
        "failure injection is only for this retained birth"
    );
    let helper = runtime.child.lock().unwrap().helper_stop_target();
    if let Some((_, target)) = helper {
        runtime
            .helper
            .stop_core(target)
            .expect("terminate exact owned Helper birth");
    } else {
        runtime
            .child
            .lock()
            .unwrap()
            .terminate_for_native_verification()
            .expect("terminate original owned Child");
    }
}

fn ss_node(id: &str, name: &str, port: u16) -> Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "protocol": "shadowsocks",
        "address": "127.0.0.1",
        "port": port,
        "shadowsocksSettings": {
            "method": "aes-128-gcm",
            "password": "polaris-test",
        },
    })
}

fn two_node_config_ports(mixed: u16, selected: &str, pa: u16, pb: u16) -> Value {
    let mut cfg = polaris_store::default_config();
    let obj = cfg.as_object_mut().unwrap();
    obj.insert(
        "servers".into(),
        serde_json::json!([
            ss_node("node-a", "Node A", pa),
            ss_node("node-b", "Node B", pb)
        ]),
    );
    obj.insert("selectedServerId".into(), serde_json::json!(selected));
    obj.insert("proxyMode".into(), serde_json::json!("global"));
    // 安全硬约束：绝不可改成 tun/systemProxy（会破坏工作机网络）。
    obj.insert("proxyModeType".into(), serde_json::json!("manual"));
    obj.insert("mixedPort".into(), serde_json::json!(mixed));
    // Every start/switch/restart snapshot uses the same authenticated management API.
    obj.insert(
        "clashApiSecret".into(),
        serde_json::json!(FIXTURE_MANAGEMENT_SECRET),
    );
    cfg
}

fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub(in crate::runtime) fn local_only_config(mixed: u16) -> Value {
    serde_json::json!({
        "servers": [],
        "selectedServerId": "__direct__", // DIRECT_SERVER_ID：全局直连，无真实节点
        "proxyMode": "direct",
        "proxyModeType": "manual",        // 安全：不接管系统代理、不建 TUN
        "mixedPort": mixed,
        "clashApiSecret": FIXTURE_MANAGEMENT_SECRET,
    })
}

async fn counting_listener() -> (u16, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::AtomicUsize;
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let h = hits.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            h.fetch_add(1, Ordering::SeqCst);
            drop(s); // 立刻断开：核会报连接失败，但「它拨了我」已被记下。
        }
    });
    (port, hits)
}

async fn drive_traffic_through_proxy(mixed: u16) {
    use tokio::io::AsyncWriteExt;
    if let Ok(mut c) = tokio::net::TcpStream::connect(("127.0.0.1", mixed)).await {
        let _ = c
            .write_all(b"GET http://192.0.2.1/ HTTP/1.1\r\nHost: 192.0.2.1\r\n\r\n")
            .await;
        let _ = c.flush().await;
    }
    tokio::time::sleep(Duration::from_millis(600)).await;
}

async fn controlled_socks_http_listener(
    body: String,
) -> (
    u16,
    Arc<std::sync::atomic::AtomicUsize>,
    impl std::future::Future<Output = ()> + Send + 'static,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = hits.clone();
    let server = async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let exchange = async {
                let invalid = || std::io::Error::other("controlled SOCKS fixture mismatch");
                let mut greeting = [0; 2];
                socket.read_exact(&mut greeting).await?;
                if greeting[0] != 5 {
                    return Err(invalid());
                }
                let mut methods = vec![0; usize::from(greeting[1])];
                socket.read_exact(&mut methods).await?;
                if !methods.contains(&0) {
                    return Err(invalid());
                }
                socket.write_all(&[5, 0]).await?;
                let mut connect = [0; 10];
                socket.read_exact(&mut connect).await?;
                if connect != [5, 1, 0, 1, 192, 0, 2, 1, 0, 80] {
                    return Err(invalid());
                }
                socket.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    if request.len() >= 8192 {
                        return Err(invalid());
                    }
                    request.push(socket.read_u8().await?);
                }
                observed.fetch_add(1, Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await?;
                socket.flush().await
            };
            let _ = tokio::time::timeout(Duration::from_secs(3), exchange).await;
        }
    };
    (port, hits, server)
}

async fn controlled_http_request(mixed: u16, nonce: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", mixed)).await.unwrap();
        socket.write_all(format!("GET http://192.0.2.1/{nonce} HTTP/1.1\r\nHost: 192.0.2.1\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut response = Vec::new();
        socket.take(16385).read_to_end(&mut response).await.unwrap();
        assert!(response.len() <= 16384);
        let response = String::from_utf8(response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"));
        response
    }).await.expect("one controlled HTTP exchange must complete without replay")
}

// OS ground truth for the controlled real-core tests. Observation failure must
// fail the test, never masquerade as a dead process or an empty orphan census.

#[cfg(not(windows))]
fn process_ids(command: &str, args: &[&str]) -> Vec<u32> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{command} process observation failed: {error}"));
    parse_process_ids(output.status.code(), &output.stdout, &output.stderr)
}

#[cfg(not(windows))]
fn parse_process_ids(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Vec<u32> {
    assert!(
        stderr.is_empty(),
        "process observation diagnostic: {}",
        String::from_utf8_lossy(stderr)
    );
    let stdout = std::str::from_utf8(stdout).expect("process observation must be UTF-8");
    if code == Some(1) {
        assert!(
            stdout.trim().is_empty(),
            "no-match exit must have no process rows"
        );
        return Vec::new();
    }
    assert_eq!(code, Some(0), "process observation command failed");
    let ids: Vec<u32> = stdout
        .lines()
        .map(|line| {
            let pid = line
                .trim()
                .parse::<u32>()
                .expect("process observation row must be a PID");
            assert_ne!(pid, 0, "process observation cannot return PID zero");
            pid
        })
        .collect();
    assert!(
        !ids.is_empty(),
        "successful process observation must return rows"
    );
    ids
}

#[cfg(not(windows))]
pub(super) fn ps_alive(pid: u32) -> bool {
    assert_ne!(pid, 0, "cannot observe PID zero");
    let ids = process_ids("ps", &["-p", &pid.to_string(), "-o", "pid="]);
    assert!(
        ids.iter().all(|observed| *observed == pid),
        "ps returned another PID"
    );
    !ids.is_empty()
}

#[cfg(not(windows))]
pub(super) fn singbox_proc_count() -> usize {
    process_ids("pgrep", &["-x", "sing-box"]).len()
}

// The proxy subtree forbids unsafe code. Keep all native calls behind the
// existing Windows process boundary, exposed only for controlled tests.
#[cfg(windows)]
use crate::runtime::windows_process::native_verification_observation as native;

#[cfg(windows)]
pub(super) fn ps_alive(pid: u32) -> bool {
    native::alive(pid)
}

#[cfg(windows)]
pub(super) fn singbox_proc_count() -> usize {
    native::count()
}

async fn real_core_full_lifecycle() {
    let _real_core_guard = lock_real_core_tests().await;
    use futures::StreamExt;
    use polaris_singbox_grpc::{Endpoint, ReconnectConfig, SingBoxApiClient};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // 前置断言：env 未设即失败（不静默跳过）。resolve_core_binary 读同一个 env，
    // 故此处**不 set/remove**——两个真机测试同进程跑，改动进程级 env 会互相踩（实测：
    // 先跑完的那个 remove 掉，后跑的 require_core 直接 panic）。
    let (rt, dir, _core, _cleanup) = real_core_runtime();
    let mixed = free_port();
    // 装日志 sink：否则 log:: 全是 no-op，核的 stdout/stderr 无处可看（也顺带验证 logging.rs 接线）。
    crate::logging::init(&dir);

    // ── ① spawn + 就绪 ──────────────────────────────────────────────────────
    let st = rt
        .start(local_only_config(mixed))
        .await
        .expect("起核应成功");
    println!(
        "[①] start → running={} pid={} mixedPort={} apiPort={}",
        st.running, st.pid, st.mixed_port, st.clash_api_port
    );
    assert!(st.running, "start 后必须 running");
    assert_ne!(st.pid, 0, "必须拿到真实 pid");
    assert_eq!(
        st.mixed_port, mixed,
        "mixedPort 必须来自 config，不是硬编码 7890"
    );
    assert_ne!(st.clash_api_port, 0, "管理 API 端口必须已解析");
    assert!(
        ps_alive(st.pid),
        "[①] ps 必须能看到 pid={} —— 进程真在跑",
        st.pid
    );
    println!("[①] ps -p {} → 进程存在 ✓", st.pid);

    // ── ② 管理 API 真的通（h2c gRPC unary RPC，非 clash REST）─────────────────
    let client = SingBoxApiClient::connect(
        Endpoint::new("127.0.0.1", st.clash_api_port),
        FIXTURE_MANAGEMENT_SECRET,
    )
    .await
    .expect("[②] 管理 API gRPC 连接应成功");
    client
        .close_all_connections()
        .await
        .expect("[②] CloseAllConnections unary RPC 应成功返回");
    println!("[②] gRPC CloseAllConnections → OK（h2c 管理 API 真的通）✓");

    // ── ③ stats 数据面真的有数据 ────────────────────────────────────────────
    // `ReconnectingStream` 是**首次 poll 才真正连**的懒流：若只是建好流对象就去造流量，
    // 订阅其实尚未建立 → 错过该连接的 NEW 事件，能否看到它就取决于核会不会为一条空闲连接
    // 再补发 UPDATE ⇒ 测试随机红（实测 2 轮 1 红）。故这里**先起后台 drain 把订阅真正拉起**，
    // 再造流量，NEW 事件必到。
    let conn_stream =
        client.subscribe_connections(200_000_000 /* 200ms */, ReconnectConfig::default());
    let collected: Arc<Mutex<Vec<polaris_stats_engine::ConnectionEntry>>> =
        Arc::new(Mutex::new(Vec::new()));
    let sink = collected.clone();
    tokio::spawn(async move {
        let mut s = Box::pin(conn_stream);
        while let Some(ev) = s.next().await {
            for e in ev.events {
                if let Some(conn) = e.connection {
                    sink.lock()
                        .unwrap()
                        .push(polaris_stats_engine::ConnectionEntry {
                            id: conn.id.clone(),
                            chains: conn.chain_list.clone(),
                            rule: conn.rule.clone(),
                            rule_id: None,
                            rule_name: None,
                            metadata: None,
                            upload: Some(conn.uplink_total as u64),
                            download: Some(conn.downlink_total as u64),
                            start: None,
                        });
                }
            }
        }
    });
    let mut status_stream = client.subscribe_status(200_000_000, ReconnectConfig::default());
    // 等订阅真正建立（懒流首帧）后再造流量。
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 本地回显 HTTP 服务器（仅 127.0.0.1；不出网）。
    let srv = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let srv_port = srv.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = srv.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\n\r\nhello-polar")
                    .await;
                // 保持连接开着，让 stats 有活连接可报。
                tokio::time::sleep(Duration::from_secs(6)).await;
            });
        }
    });

    // 经混合入站发一个 HTTP 代理请求（目标是本地回显服务器）。
    let mut c = tokio::net::TcpStream::connect(("127.0.0.1", mixed))
        .await
        .expect("[③] 混合入站应可连（端口来自 config）");
    let req =
        format!("GET http://127.0.0.1:{srv_port}/ HTTP/1.1\r\nHost: 127.0.0.1:{srv_port}\r\n\r\n");
    c.write_all(req.as_bytes()).await.unwrap();
    let mut resp = vec![0u8; 128];
    let n = tokio::time::timeout(Duration::from_secs(5), c.read(&mut resp))
        .await
        .expect("[③] 经代理读响应超时")
        .expect("[③] 经代理读响应失败");
    let body = String::from_utf8_lossy(&resp[..n]);
    assert!(
        body.contains("200 OK"),
        "[③] 经混合入站的请求应拿到 200，实得：{body}"
    );
    println!(
        "[③] 经 mixed:{mixed} 代理请求 → {} ✓",
        body.lines().next().unwrap_or("")
    );

    // 等后台 drain 收到真实连接事件（拓扑 aggregate_connections 的供数源）。
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    while tokio::time::Instant::now() < deadline && collected.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let entries = collected.lock().unwrap().clone();
    assert!(
        !entries.is_empty(),
        "[③] Connections 流必须报出真实连接（拓扑 aggregate_connections 的供数源）"
    );
    println!("[③] Connections 流 → {} 条真实连接：", entries.len());
    for e in entries.iter().take(3) {
        println!(
            "      id={} rule={:?} chains={:?} up={:?} down={:?}",
            e.id, e.rule, e.chains, e.upload, e.download
        );
    }
    let agg = polaris_stats_engine::aggregate_connections(&entries, 0);
    println!("[③] aggregate_connections → {agg:?}");

    // Status 流也应给出真实数字。
    let s0 = tokio::time::timeout(Duration::from_secs(5), status_stream.next())
        .await
        .expect("[③] Status 流应在 5s 内出帧")
        .expect("[③] Status 流不应立即结束");
    println!(
        "[③] Status 流 → memory={} goroutines={} connectionsOut={} upTotal={} downTotal={}",
        s0.memory, s0.goroutines, s0.connections_out, s0.uplink_total, s0.downlink_total
    );
    assert!(
        s0.memory > 0 && s0.goroutines > 0,
        "[③] Status 必须是真实运行数据"
    );

    // ── ⑤ explicitly confirmed restart scheduling, not implicit HOT success ──
    let (ap, _) = rt.apply_confirmed_for_native_verification().await;
    println!("[⑤] explicitly confirmed restart scheduling → {ap}");
    assert_eq!(ap, "applied");

    // ── ④ 停核干净、无孤儿 ──────────────────────────────────────────────────
    let pid = st.pid;
    rt.stop().await.expect("停核应成功");
    assert!(!rt.status().running, "[④] stop 后 running 必须为 false");
    // 给 OS 一点收尾时间后用 ps 实证。
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !ps_alive(pid),
        "[④] stop 后 pid={pid} 必须不存在（无孤儿进程）"
    );
    println!("[④] stop → ps -p {pid} 已消失（无孤儿）✓");
}

async fn real_core_lifecycle_race_start_then_immediate_stop() {
    let _real_core_guard = lock_real_core_tests().await;
    let (rt, _dir, _core, _cleanup) = real_core_runtime();
    // 孤儿基线：快速起停 3 轮后系统内 sing-box 进程数不得增长。
    let baseline = singbox_proc_count();

    for round in 1..=3 {
        let mixed = free_port();
        let rt2 = rt.clone();
        let starter = tokio::spawn(async move { rt2.start(local_only_config(mixed)).await });
        // 不用固定 sleep 猜 spawn 是否发生：必须观测到真实 pid，且起核腿仍在飞，才允许 stop。
        // 否则安全门/前置失败会让本测「从未起核也通过」，正是本次修复要消灭的假阳性。
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let spawned_pid = loop {
            if let Some(pid) = *rt.pid.lock().unwrap() {
                break pid;
            }
            assert!(
                !starter.is_finished(),
                "[⑥] round{round}: 尚未观测到真实 pid，start 任务却已结束"
            );
            assert!(
                tokio::time::Instant::now() < deadline,
                "[⑥] round{round}: 5s 内未观测到真实 spawn"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        };
        assert!(
            rt.start_inflight.load(Ordering::SeqCst) > 0,
            "[⑥] round{round}: stop 前起核腿必须仍在飞"
        );
        assert!(
            ps_alive(spawned_pid),
            "[⑥] round{round}: 观测到的 pid={spawned_pid} 必须真实存活"
        );
        // 起核在飞（就绪门轮询中）时立刻 stop → bump 世代 → 起核腿 Superseded 让位。
        rt.stop().await.expect("stop 应成功");
        let started = starter.await.expect("start task 不应 panic");
        println!(
            "[⑥] round{round}: 已观测真实 pid={spawned_pid}，start 返回 {:?}，stop 已接管",
            started.map(|s| s.running)
        );

        assert!(
            !rt.status().running,
            "[⑥] round{round}: stop 后不得 running"
        );
        assert!(
            rt.child.lock().unwrap().is_empty(),
            "[⑥] round{round}: stop 后不得残留 child 句柄"
        );
        assert!(
            rt.pid.lock().unwrap().is_none(),
            "[⑥] round{round}: stop 后不得残留 pid"
        );
    }
    // 全轮结束后系统里不应有本测试起的 sing-box 残留（起停竞态最易漏杀之处）。
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after = singbox_proc_count();
    assert_eq!(
        after, baseline,
        "[⑥] 3 轮快速起停后 sing-box 进程数应回到基线 {baseline}，实得 {after} —— 起停竞态漏了孤儿"
    );
    println!(
        "[⑥] 3 轮快速起停完成：child 句柄已清 + sing-box 进程数 {baseline}→{after}（无孤儿）✓"
    );
}

async fn real_core_hot_switch_keeps_pid() {
    let _real_core_guard = lock_real_core_tests().await;
    let (rt, dir, _core, _cleanup) = real_core_runtime();
    crate::logging::init(&dir);
    let mixed = free_port();
    // 两个节点各指到我们自己的本地监听器（127.0.0.1，随机空闲口）→ ③b 直接观测核拨了谁。
    let (pa, hits_a) = counting_listener().await;
    let (pb, hits_b) = counting_listener().await;
    println!("[①] 节点监听器：Node A=127.0.0.1:{pa}, Node B=127.0.0.1:{pb}");

    // ── ① 起核（两节点，选中 Node A）────────────────────────────────────────
    // 生产时序：save_full 落盘 → broadcast_config_changed → switch_mode。测试逐条照做，
    // 否则去抖重启回落读磁盘时会拿到默认配置（restart 腿的载荷来自 config.current()）。
    // Keep the production default enabled; only replace unrelated Internet ZIP
    // download with this test's own inert static bundle via the real resolver.
    let dashboard = rt.config.dir().join("singbox-dashboard");
    std::fs::create_dir_all(&dashboard).unwrap();
    std::fs::write(
        dashboard.join("index.html"),
        "<!doctype html><title>controlled fixture</title>",
    )
    .unwrap();
    assert_eq!(
        super::core_binary::resolve_dashboard_serve_dir(rt.config.dir()),
        Some(dashboard.to_string_lossy().into_owned())
    );
    let cfg_a = two_node_config_ports(mixed, "node-a", pa, pb);
    assert_eq!(
        cfg_a["singboxDashboard"], true,
        "production default dashboard must stay enabled"
    );
    rt.config.save_full(&cfg_a).expect("落盘应成功");
    let started = rt.start(cfg_a.clone()).await;
    // These are the actual bytes written by the production generator for this
    // isolated synthetic fixture, not a second serialization/reconstructed graph.
    // Existing controlled-test logs carry the lossless string; the runner stores
    // its original UTF-8 bytes and checks SHA before the normal start assertion.
    if let Ok(raw) = std::fs::read(rt.runtime_config_path()) {
        let record = serde_json::json!({
            "schema":"polaris-controlled-production-graph-v1",
            "processId":std::process::id(), "stage":"initial-start", "startSucceeded":started.is_ok(),
            "source":"actual-runtime-config-path", "dashboardEnabled":true,
            "dashboardPath":dashboard.to_string_lossy(), "bytes":raw.len(),
            "sha256":polaris_updater::sha256_hex(&raw),
            "configJson":String::from_utf8(raw).expect("generator emits UTF-8 JSON"),
            "publicationEligible":false,
        });
        println!("\nPOLARIS_DEFAULT_GRAPH {record}");
    }
    let st = started.expect("起核应成功");
    let pid1 = st.pid;
    println!(
        "[①] start → pid={pid1} mixedPort={} apiPort={}",
        st.mixed_port, st.clash_api_port
    );
    assert!(ps_alive(pid1), "[①] ps 必须能看到 pid={pid1}");
    assert!(
        rt.switch_snapshot.read().unwrap().is_some(),
        "[①] 起核就绪后必须留下热切换基准快照（否则一切变更退化为重启）"
    );
    let snap_tags = rt
        .switch_snapshot
        .read()
        .unwrap()
        .clone()
        .unwrap()
        .id_to_tag;
    println!("[①] 热切换基准 id→tag = {snap_tags:?}");

    // ── ② 切节点 → 热切换，PID 不变（**核心判据**）───────────────────────────
    let cfg_b = two_node_config_ports(mixed, "node-b", pa, pb);
    rt.config.save_full(&cfg_b).expect("落盘应成功");
    let out = rt.switch_mode(cfg_b.clone()).await;
    println!("[②] switch_mode（node-a → node-b）→ {out:?}");
    assert_eq!(
        out,
        SwitchOutcome::HotSwitched,
        "[②] 切节点必须走热切腿（实得 {out:?}）—— 这正是本任务要接的线"
    );
    let pid_after = rt.status().pid;
    assert_eq!(
        pid_after, pid1,
        "[②] 热切换后 PID 必须不变（{pid1} → {pid_after}）—— 变了就说明还是在重启整个核"
    );
    assert!(
        ps_alive(pid1),
        "[②] 原进程 pid={pid1} 必须仍在跑（ps 实证）"
    );
    println!("[②] ps -p {pid1} 仍存活 + PID 未变 → 热切换真的生效 ✓");

    // ── ③ SelectOutbound 真的经 gRPC 下发且被核接受 ──────────────────────────
    // 负向对照：核必须拒绝不存在的成员 tag。它会拒 ⇒ ② 里那次成功的 PUT 确实选中了真实成员，
    // 而不是「核照单全收、根本没校验」。没有这条，「PUT 返回 Ok」只能证明 RPC 通了。
    let client = SingBoxApiClient::connect(
        Endpoint::new("127.0.0.1", st.clash_api_port),
        FIXTURE_MANAGEMENT_SECRET,
    )
    .await
    .expect("[③] 管理 API gRPC 连接应成功");
    let bogus = client
        .select_outbound("proxy-selector", "no-such-member-xyz")
        .await;
    println!("[③] 负向对照 SelectOutbound(proxy-selector, no-such-member-xyz) → {bogus:?}");
    assert!(
        bogus.is_err(),
        "[③] 核必须拒绝不存在的成员 tag —— 它若照单全收，② 的 PUT 成功就不能证明真的切了"
    );
    let good = client.select_outbound("proxy-selector", "Node B").await;
    println!("[③] 正向 SelectOutbound(proxy-selector, Node B) → {good:?}");
    assert!(good.is_ok(), "[③] 真实成员 tag 的 PUT 必须成功");

    // ── ③b 决定性实证：热切换后**真实流量**改走 Node B（不是只让 RPC 返了个 Ok）──────
    // 前面几条只证明「PUT 被核接受」。要证明**路由真的变了**，就得看核实际拨了谁：
    // 两个节点各指向我们自己的本地监听器，谁被连上一目了然。
    // 不读日志——日志经全局 OnceLock logger 中转，多测试同进程时会绑到别人的目录（实测踩到）。
    client
        .select_outbound("proxy-selector", "Node B")
        .await
        .expect("[③b] 复位到 Node B（③ 的负向对照后需还原）");
    hits_a.store(0, Ordering::SeqCst);
    hits_b.store(0, Ordering::SeqCst);
    drive_traffic_through_proxy(mixed).await;
    let (a, b) = (hits_a.load(Ordering::SeqCst), hits_b.load(Ordering::SeqCst));
    println!("[③b] 热切换后打流量：Node A 监听器被连 {a} 次，Node B 监听器被连 {b} 次");
    assert!(
        b > 0,
        "[③b] 热切换后流量必须由 Node B 承载（B 被连 {b} 次）—— \
             PUT 返回了 Ok 但路由没切 = 假阳性，正是「兜底把失败伪装成成功」的形态"
    );
    assert_eq!(
        a, 0,
        "[③b] 切换后绝不该再有新连接落到 Node A（实得 {a} 次）"
    );
    println!("[③b] 核未重启（pid={pid1}）且流量已改走 Node B → 热切换真的改变了实际路由 ✓");

    // 显式节点命令的新路径：D 保存后只投影 selectedServerId 到 R，必须经真实
    // GroupsSnapshot 读回才回 HotSwitched，且新连接确实回到 A，PID 仍不变。
    rt.config.save_full(&cfg_a).expect("显式选 A 落盘");
    let intent = rt.register_selector_intent();
    let explicit = rt
        .switch_selected_server_if_current("node-a", intent)
        .await
        .expect("显式选择执行")
        .expect("本次选择未被接管");
    assert_eq!(
        explicit,
        SwitchOutcome::HotSwitched,
        "真核严格读回必须确认 A"
    );
    assert_eq!(rt.status().pid, pid1, "显式热切不应重启核");
    hits_a.store(0, Ordering::SeqCst);
    hits_b.store(0, Ordering::SeqCst);
    drive_traffic_through_proxy(mixed).await;
    assert!(
        hits_a.load(Ordering::SeqCst) > 0,
        "显式热切后真实流量须回到 A"
    );
    assert_eq!(hits_b.load(Ordering::SeqCst), 0, "旧出口 B 不得承接新连接");

    // Structural edit is deferred first. Only explicit confirmation may change PID.
    let mixed2 = free_port();
    let cfg_port = two_node_config_ports(mixed2, "node-b", pa, pb);
    rt.config.save_full(&cfg_port).expect("落盘应成功");
    let out = rt.switch_mode(cfg_port).await;
    println!("[④] switch_mode（改 mixedPort {mixed} → {mixed2}）→ {out:?}");
    assert_eq!(
        out,
        SwitchOutcome::Deferred,
        "[④] structural edit must wait for explicit restart permission"
    );
    assert_eq!(
        rt.current_config_snapshot().unwrap()["mixedPort"],
        mixed,
        "去抖等待期间运行态 R 仍须指向旧核端口"
    );
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    assert!(!rt.gate.pending().restart_pending);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        rt.status().pid,
        pid1,
        "no implicit restart during the decision window"
    );
    assert!(ps_alive(pid1));
    let (confirmed, id) = rt.apply_confirmed_for_native_verification().await;
    assert_eq!(confirmed, "applied");
    let (queued_id, queued, source) = rt.pending_force_restart.read().unwrap().clone().unwrap();
    assert_eq!(Some(queued_id), id);
    assert_eq!(queued["mixedPort"], mixed2);
    assert!(matches!(source, ForceRestartSource::Confirmed { .. }));
    let pid2 = wait_pid_change(&rt, pid1, 20)
        .await
        .expect("[④] explicitly confirmed restart must produce a new PID within 20s");
    println!("[④] 重启完成：pid {pid1} → {pid2}");
    assert!(ps_alive(pid2), "[④] 新核必须在跑");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!ps_alive(pid1), "[④] 旧核 pid={pid1} 必须已退出（无孤儿）");
    assert_eq!(
        rt.status().mixed_port,
        mixed2,
        "[④] 重启后必须真的起在新端口上（否则重启是空转）"
    );
    assert_eq!(
        rt.current_config_snapshot().unwrap()["mixedPort"],
        mixed2,
        "timer must use the exact confirmed snapshot and commit R only after core readiness"
    );
    println!("[④] 旧核已退、新核监听 {mixed2} → 重启路径完好 ✓");

    rt.stop().await.expect("停核应成功");
}

async fn real_core_auto_failover_attests_without_applying_saved_debt() {
    let _real_core_guard = lock_real_core_tests().await;
    let (rt, dir, _core, _cleanup) = real_core_runtime();
    crate::logging::init(&dir);
    let mixed = free_port();
    let (pa, _hits_a) = counting_listener().await;
    let (pb, hits_b) = counting_listener().await;
    let runtime = two_node_config_ports(mixed, "node-a", pa, pb);
    rt.config.save_full(&runtime).expect("seed runtime config");
    let started = rt.start(runtime.clone()).await.expect("real core start");
    let pid = started.pid;
    // 生产心跳首拍晚于起核后的 selector reassert/attestation；测试直调事务也保持同一可达时序，
    // 避免人为把启动校正与 failover PUT 叠在一起制造生产中不存在的双写竞态。
    tokio::time::sleep(Duration::from_millis(500)).await;

    let mut disk = runtime.clone();
    disk["mixedPort"] = serde_json::json!(free_port());
    rt.config.save_full(&disk).expect("save unapplied debt");
    let targets = rt
        .speed_probe_targets()
        .expect("running core probe targets");
    let tag = targets
        .id_to_tag
        .get("node-b")
        .cloned()
        .expect("node-b loaded in current selector");
    let candidate = RuntimeCandidate {
        id: "node-b".to_string(),
        name: "Node B".to_string(),
        tag,
    };
    let fingerprint = crate::runtime::node_fingerprints::measurement_table_json(&disk)
        .remove("node-b")
        .expect("candidate fingerprint");

    let outcome = rt
        .auto_hot_switch_transaction(
            rt.core_generation(),
            "node-a",
            &candidate,
            &fingerprint,
            None,
        )
        .await;
    assert_eq!(outcome, AutoHotSwitchOutcome::Applied);
    assert_eq!(rt.status().pid, pid, "自动 failover 必须保持真实核 PID");
    assert!(ps_alive(pid), "真实核仍须存活");
    let after_disk = rt.config.current().unwrap();
    let after_runtime = rt.current_config.read().unwrap().clone().unwrap();
    assert_eq!(after_disk["selectedServerId"], "node-b");
    assert_ne!(after_disk["mixedPort"], runtime["mixedPort"]);
    assert_eq!(after_runtime["selectedServerId"], "node-b");
    assert_eq!(
        after_runtime["mixedPort"], runtime["mixedPort"],
        "未 Apply 的 mixedPort 不得被后台切换夹带入核"
    );

    hits_b.store(0, Ordering::SeqCst);
    drive_traffic_through_proxy(mixed).await;
    assert!(
        hits_b.load(Ordering::SeqCst) > 0,
        "回读自证后的新连接必须真实拨向 Node B"
    );

    rt.stop().await.expect("stop real core");
}

async fn real_core_hot_switch_failure_waits_for_explicit_restart_confirmation() {
    let _real_core_guard = lock_real_core_tests().await;
    let (rt, dir, _core, _cleanup) = real_core_runtime();
    crate::logging::init(&dir);
    let mixed = free_port();

    let nonce = uuid::Uuid::new_v4().to_string();
    let (pa, hits_a, task_a) = controlled_socks_http_listener(format!("A:{nonce}")).await;
    let (pb, hits_b, task_b) = controlled_socks_http_listener(format!("B:{nonce}")).await;
    // JoinSet owns both listener futures and aborts them even if an assertion panics.
    let mut servers = tokio::task::JoinSet::new();
    servers.spawn(task_a);
    servers.spawn(task_b);
    let mut cfg_a = two_node_config_ports(mixed, "node-a", pa, pb);
    cfg_a["servers"] = serde_json::json!([
        {"id":"node-a","name":"Node A","protocol":"socks","address":"127.0.0.1","port":pa},
        {"id":"node-b","name":"Node B","protocol":"socks","address":"127.0.0.1","port":pb}
    ]);
    rt.config.save_full(&cfg_a).expect("落盘应成功");
    let st = rt.start(cfg_a.clone()).await.expect("起核应成功");
    let client1 = polaris_singbox_grpc::SingBoxApiClient::connect(
        polaris_singbox_grpc::Endpoint::new("127.0.0.1", st.clash_api_port),
        FIXTURE_MANAGEMENT_SECRET.to_owned(),
    )
    .await
    .expect("initial authenticated API");
    let incarnation1 = client1
        .get_runtime_capabilities()
        .await
        .expect("initial capabilities")
        .active
        .expect("initial active runtime")
        .incarnation;
    assert!(!incarnation1.is_empty());
    assert!(controlled_http_request(mixed, &nonce)
        .await
        .ends_with(&format!("A:{nonce}")));
    assert_eq!(hits_a.load(Ordering::SeqCst), 1);
    assert_eq!(hits_b.load(Ordering::SeqCst), 0);
    let pid1 = st.pid;
    println!("[⑤] 起核 pid={pid1}");

    // 仅隔离竞争的自动恢复 owner；不调用 stop、不动世代或热切基准快照。
    let generation = rt.gate.generation();
    assert_eq!(rt.crash_lock().restart_count(), 0);
    let auto_restart_aborted_before = rt.crash_lock().auto_restart_aborted();
    assert!(
        !auto_restart_aborted_before,
        "[⑤] 隔离前不得有遗留 abort 标记"
    );
    rt.crash_lock().mark_user_aborted();
    // 失败注入：真实终止原核，管理 API 必须确实不可达。
    send_signal(pid1, Signal::Sigkill);
    tokio::time::sleep(Duration::from_millis(500)).await;
    // 判据用「管理 API 已不可连」而**不用 `ps_alive`**：SIGKILL 后 child 句柄仍被 ProxyRuntime
    // 持有、无人 `wait()` 收割 → 进程处于 **zombie** 态，`ps -p` 照样看得见它（`ps_alive` 分辨
    // 不了 zombie 与存活）。而「管理 API 连不上」正是本测试要注入的失败条件本身。
    let api_dead = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", st.clash_api_port)),
    )
    .await
    .expect("[⑤] 管理 API 拒连检查必须在 5s 内完成")
    .is_err();
    assert!(api_dead, "[⑤] 核已被 SIGKILL → 管理 API 端口应不可连");
    assert!(
        rt.status().running,
        "[⑤] 显式重启确认前提：运行快照仍在，但真实管理 API 已不可达"
    );
    assert_eq!(
        rt.gate.generation(),
        generation,
        "[⑤] 自愈不得抢占显式重启世代"
    );
    assert_eq!(
        rt.crash_lock().restart_count(),
        0,
        "[⑤] 自动恢复必须被本测试隔离"
    );
    assert!(rt.crash_lock().auto_restart_aborted());
    println!("[⑤] 已 SIGKILL 核 pid={pid1}：管理 API 不可连，status 仍自称 running");

    // Failed selector HOT must settle without an automatic restart.
    let mut cfg_b = cfg_a;
    cfg_b["selectedServerId"] = serde_json::json!("node-b");
    rt.config.save_full(&cfg_b).expect("落盘应成功");
    let out = tokio::time::timeout(Duration::from_secs(15), rt.switch_mode(cfg_b.clone()))
        .await
        .expect("[⑤] switch_mode 必须在 15s 内返回 —— 卡死即失败");
    println!("[⑤] 核已死时 switch_mode → {out:?}");
    assert_eq!(
        out,
        SwitchOutcome::Deferred,
        "[⑤] failed HOT must stay pending without implicit restart permission"
    );

    assert_eq!(rt.status().pid, pid1);
    assert_eq!(rt.gate.generation(), generation);
    assert_eq!(
        rt.current_config_snapshot().unwrap()["selectedServerId"],
        "node-a"
    );
    assert_eq!(rt.config.current().unwrap()["selectedServerId"], "node-b");
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    assert!(!rt.gate.pending().restart_pending);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        rt.status().pid,
        pid1,
        "failed HOT must not schedule a successor"
    );
    assert_eq!(rt.gate.generation(), generation);
    let choice = rt.restart_choice_for_native_verification();
    assert_eq!(choice.status, "restartRequired");
    let old_token = choice
        .restart_confirmation
        .expect("original generation choice token");
    let confirmed = rt.apply_pending_transaction(Some(old_token.clone())).await;
    assert_eq!(
        confirmed.status, "unconfirmed",
        "dead incarnation cannot authenticate confirmation"
    );
    assert!(confirmed.apply_id.is_none());
    assert_eq!(rt.gate.generation(), generation);
    assert_eq!(rt.status().pid, pid1);
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    assert!(!rt.gate.pending().restart_pending);

    // A user Restart is a separate existing lifecycle action, not a bypass of
    // the failed transaction's authenticated confirmation contract.
    let restarted = tokio::time::timeout(Duration::from_secs(20), rt.restart(cfg_b))
        .await
        .expect("explicit user restart bounded")
        .expect("explicit user restart succeeds");
    let pid2 = restarted.pid;
    assert_ne!(pid2, pid1);
    assert!(ps_alive(pid2));
    assert!(
        !ps_alive(pid1),
        "old owned child must be reaped by explicit restart"
    );
    assert!(rt.gate.generation() > generation);
    let generation2 = rt.gate.generation();
    let client2 = polaris_singbox_grpc::SingBoxApiClient::connect(
        polaris_singbox_grpc::Endpoint::new("127.0.0.1", restarted.clash_api_port),
        FIXTURE_MANAGEMENT_SECRET.to_owned(),
    )
    .await
    .expect("successor authenticated API");
    let incarnation2 = client2
        .get_runtime_capabilities()
        .await
        .expect("successor capabilities")
        .active
        .expect("successor active runtime")
        .incarnation;
    assert!(!incarnation2.is_empty());
    assert_ne!(incarnation2, incarnation1);
    let stale = rt.apply_pending_transaction(Some(old_token)).await;
    assert_eq!(
        stale.status, "conflict",
        "old choice must not operate on a successor"
    );
    assert!(stale.apply_id.is_none());
    assert_eq!(rt.gate.generation(), generation2);
    assert_eq!(rt.status().pid, pid2);
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    assert!(!rt.gate.pending().restart_pending);

    // PID 变化本身不等于 start 已完成；等既有 lifecycle settle 后读真实 selector。
    tokio::time::timeout(Duration::from_secs(20), async {
        while rt.gate.is_busy() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("[⑤] 用户显式 Restart 后的启动必须完成");
    assert!(rt.status().running);
    assert_eq!(rt.status().pid, pid2);
    let groups = rt
        .management_api()
        .await
        .groups_snapshot()
        .await
        .expect("[⑤] 用户显式 Restart 后的新核必须返回真实 selector 快照");
    assert!(
        groups
            .iter()
            .any(|group| group.tag == "proxy-selector" && group.selected == "Node B"),
        "[⑤] 用户显式 Restart 后的新核必须实际选中 Node B，不得吞掉切节点"
    );
    assert_eq!(
        rt.current_config_snapshot().unwrap()["selectedServerId"],
        "node-b"
    );
    let a_before = hits_a.load(Ordering::SeqCst);
    let b_before = hits_b.load(Ordering::SeqCst);
    assert!(controlled_http_request(restarted.mixed_port, &nonce)
        .await
        .ends_with(&format!("B:{nonce}")));
    assert_eq!(
        hits_a.load(Ordering::SeqCst),
        a_before,
        "successor must not send new traffic to A"
    );
    assert_eq!(hits_b.load(Ordering::SeqCst), b_before + 1);
    assert_eq!(
        rt.crash_lock().restart_count(),
        0,
        "explicit user restart, not automatic recovery"
    );
    // restart_inner → start_guarded(..., Some(stop_generation)) 是条件接续，
    // 不走 expected_generation=None 的显式 Start reset。不要把测试注入的
    // abort 状态误当成产品必须清除的状态，更不能为此更改生产生命周期语义。
    {
        let mut crash = rt.crash_lock();
        assert!(
            crash.auto_restart_aborted(),
            "[⑤] 接续的显式重启 必须保留测试的自愈隔离"
        );
        if auto_restart_aborted_before {
            crash.mark_user_aborted();
        } else {
            crash.reset_user_aborted();
        }
        assert_eq!(
            crash.auto_restart_aborted(),
            auto_restart_aborted_before,
            "[⑤] 测试必须恢复自己注入的 abort 标记"
        );
    }

    rt.stop().await.expect("停核应成功");
    servers.shutdown().await;
}

async fn real_core_aggregate_relay_emits_real_frames() {
    let (proxy, dir, _core, _cleanup) = real_core_runtime();
    crate::logging::init(&dir);
    let mixed = free_port();

    // ── BUG-2：真配置起真核（proxy.start(config: Value) → running）──────────────
    let st = proxy
        .start(local_only_config(mixed))
        .await
        .expect("[BUG-2] proxy.start(config) 起核应成功");
    println!(
        "[BUG-2] proxy.start(config) → running={} pid={} mixedPort={} apiPort={}",
        st.running, st.pid, st.mixed_port, st.clash_api_port
    );
    assert!(st.running, "[BUG-2] 起核后必须 running");
    assert_ne!(st.pid, 0, "[BUG-2] 必须拿到真实 pid");
    assert_ne!(st.clash_api_port, 0, "[BUG-2] 管理 API 端口必须已解析");

    // ── 造真实连接：本地服务器（仅 127.0.0.1，不出网），**延迟 10s 才响应** →
    //    请求已发、响应未回，连接在整个窗口内确定活跃（对齐「首页有活连接」的稳态场景）。──
    let srv = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let srv_port = srv.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = srv.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf).await;
                tokio::time::sleep(Duration::from_secs(10)).await; // 延迟响应 → 连接持续活跃
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello")
                    .await;
            });
        }
    });
    let mut holds = Vec::new();
    for _ in 0..3 {
        let mut c = tokio::net::TcpStream::connect(("127.0.0.1", mixed))
            .await
            .expect("混合入站应可连");
        let req = format!(
            "GET http://127.0.0.1:{srv_port}/ HTTP/1.1\r\nHost: 127.0.0.1:{srv_port}\r\n\r\n"
        );
        c.write_all(req.as_bytes()).await.unwrap();
        holds.push(c); // 持有 + 不等响应 → 连接保持活跃
    }
    // 给 sing-box 一点时间把连接登记进管理面。
    tokio::time::sleep(Duration::from_millis(500)).await;

    // ── BUG-1：走 relay 的真实路径（snapshot → build_aggregate → signature_changed）──
    let client = SingBoxApiClient::connect(
        Endpoint::new("127.0.0.1", st.clash_api_port),
        FIXTURE_MANAGEMENT_SECRET,
    )
    .await
    .expect("[BUG-1] 管理 API gRPC 连接应成功");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    let mut agg = None;
    while tokio::time::Instant::now() < deadline {
        let conns = client
            .first_connection_snapshot()
            .await
            .expect("[BUG-1] 连接快照应成功");
        let alive = conns.iter().filter(|c| c.closed_at <= 0).count();
        eprintln!(
            "[poll] first_connection_snapshot → {} conns（活跃 {alive}）",
            conns.len()
        );
        let a = build_aggregate(&conns, now_ms());
        if a.total > 0 {
            agg = Some(a);
            break;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let agg = agg.expect("[BUG-1] relay 必须从真核聚合出 total>0 的真实帧（否则数据面仍无供数）");
    println!(
        "[BUG-1] build_aggregate(真核快照) → {}",
        serde_json::to_string(&agg).unwrap()
    );
    assert!(agg.total > 0, "[BUG-1] 真实连接总数必须 > 0");
    assert!(!agg.hosts.is_empty(), "[BUG-1] 至少一个真实 host 节点");

    // change-driven：首帧必推，同内容去重不推。
    let sig1 = signature_changed(&agg, &None).expect("[BUG-1] 首帧必推（emit）");
    assert!(
        signature_changed(&agg, &Some(sig1)).is_none(),
        "[BUG-1] 同内容必须去重不推（change-driven）"
    );
    println!("[BUG-1] change-driven：首帧 emit + 同内容去重 ✓");

    // ── 停核干净 ──────────────────────────────────────────────────────────
    drop(holds);
    let pid = st.pid;
    proxy.stop().await.expect("停核应成功");
    assert!(!proxy.status().running, "停核后 running 必须为 false");
    println!("[done] proxy.stop() → running=false（pid={pid} 已收割）");
    let _ = std::fs::remove_dir_all(&dir);
}

/// One bounded case per process. No path, policy, issuer, transport or argv inputs.
pub(crate) async fn run_case(case: &str) -> Result<(), String> {
    if !CASE_NAMES.contains(&case) {
        return Err("unknown native acceptance case".into());
    }
    if std::env::var_os("POLARIS_SINGBOX_PATH").is_some() {
        return Err("Core path override is forbidden in native production verification".into());
    }
    let parent = std::env::temp_dir().join("polaris-native-production-verification");
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let dir = parent.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let config = Arc::new(ConfigManager::new(dir.clone()));
    let helper = Arc::new(HelperRuntime::new(dir.clone()));
    let mesh = Arc::new(MeshRuntime::new_with_helper(dir.clone(), helper.clone()));
    let clearer = Box::new(polaris_system_integration::production_proxy_controller(
        dir.join(polaris_system_integration::PROXY_MARKER_FILENAME)
            .to_string_lossy()
            .into_owned(),
    ));
    let runtime = Arc::new(ProxyRuntime::new(config, helper, mesh, clearer));
    DIRECTORY
        .set(dir.clone())
        .map_err(|_| "one case per process")?;
    RUNTIME
        .set(runtime.clone())
        .map_err(|_| "one case per process")?;
    let result=AssertUnwindSafe(async {match case {


"runtime::proxy::tests::lifecycle::real_core_full_lifecycle"=>real_core_full_lifecycle().await,

"runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop"=>real_core_lifecycle_race_start_then_immediate_stop().await,

"runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid"=>real_core_hot_switch_keeps_pid().await,

"runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt"=>real_core_auto_failover_attests_without_applying_saved_debt().await,

"runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation"=>real_core_hot_switch_failure_waits_for_explicit_restart_confirmation().await,

"runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames"=>real_core_aggregate_relay_emits_real_frames().await,

_=>unreachable!(),}}).catch_unwind().await;
    // Always fence admission and drain through the real production owner, even on
    // assertions or a missing selected candidate. Cleanup is never a native PASS.
    let check_fence = polaris_core_supervisor::begin_check_shutdown().map_err(|e| e.to_string());
    let cleanup = cleanup::drain_after_fence(check_fence, runtime.shutdown_for_exit(), async {
        polaris_core_supervisor::shutdown_checks_for_exit()
            .await
            .map_err(|e| e.to_string())
    })
    .await;
    let fenced = runtime.desktop_shutdown.lock().map(|v| *v).unwrap_or(false);
    let empty = runtime
        .child
        .lock()
        .map(|v| v.is_empty() && !v.has_helper_start())
        .unwrap_or(false);
    let no_pid = runtime.pid.lock().map(|v| v.is_none()).unwrap_or(false);
    if cleanup.is_ok() && fenced && empty && no_pid {
        println!(
            "POLARIS_CONTROLLED_CLEANUP {}",
            serde_json::json!({"schema":"polaris-controlled-cleanup-v1","testName":case,"processId":std::process::id(),"generation":runtime.core_generation(),"shutdownFenced":true,"custody":{"state":"empty-after-shutdown-fence","runRef":null,"corePid":null},"publicationEligible":false})
        );
    } else {
        return Err(format!(
            "production shutdown custody unconfirmed: {cleanup:?}; fenced={fenced}, empty={empty}, no_pid={no_pid}"
        ));
    }
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    };
    println!(
        "POLARIS_NATIVE_CASE {}",
        serde_json::json!({"schema":"polaris-native-production-case-v1","case":case,"processId":std::process::id(),"result":if result.is_ok(){"passed"}else{"failed"},"carrier":"normal-production-library","validationCustodyDrained":true,"publicationEligible":false})
    );
    if result.is_err() {
        return Err("native business assertion failed (see panic diagnostic)".into());
    }
    Ok(())
}
const CASE_NAMES:&[&str]=&[


"runtime::proxy::tests::lifecycle::real_core_full_lifecycle",

"runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop",

"runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid",

"runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt",

"runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_waits_for_explicit_restart_confirmation",

"runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames",

];
