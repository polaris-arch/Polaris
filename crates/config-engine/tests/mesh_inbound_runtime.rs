//! 用户态 WG/MASQUE endpoint 的真实入站门。MASQUE 外层 H3 使用 IPv4 回环，
//! 内层分别验证 IPv4 和 IPv6；探针与服务只绑定回环，不创建 TUN/System 接口。
//! 本机 POLARIS_NO_KERNEL_RUN=1 跳过；打包 CI 用随包核执行。

mod support;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use polaris_config_engine::builder::generate_sing_box_config;
use polaris_config_engine::user_config::app_config::UserConfig;
use serde_json::{json, Value};
use support::core_locator::command_for_core;
use support::kernel_gate::{check, core_or_skip, full_config_deps, SnapshotCase};
use support::kernel_run::{kernel_run_or_skip, with_run};
use tempfile::TempDir;

const A_IP: &str = "10.77.0.1";
const B_IP: &str = "10.77.0.2";
const A_IP6: &str = "fd77::1";
const B_IP6: &str = "fd77::2";
const A_PRIVATE: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=";
const A_PUBLIC: &str = "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9/AsrhtHHw=";
const B_PRIVATE: &str = "ISIjJCUmJygpKissLS4vMDEyMzQ1Njc4OTo7PD0+P0A=";
const B_PUBLIC: &str = "WGmv9FBUlzLLqu1eXfmzCm2jHLDldCutWtShp2jxpns=";
const DNS_NAME: &str = "warm.mesh.polaris.invalid";

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn free_udp_port() -> u16 {
    UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
fn local(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

struct Listener {
    port: u16,
    hits: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Listener {
    fn tcp_on(ipv6: bool) -> Self {
        let socket = TcpListener::bind(if ipv6 { "[::1]:0" } else { "127.0.0.1:0" }).unwrap();
        socket.set_nonblocking(true).unwrap();
        let port = socket.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (hits_clone, stop_clone) = (hits.clone(), stop.clone());
        let worker = thread::spawn(move || {
            while !stop_clone.load(Ordering::Relaxed) {
                match socket.accept() {
                    Ok((mut stream, _)) => {
                        hits_clone.fetch_add(1, Ordering::Relaxed);
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
                        let mut byte = [0u8; 1];
                        if stream.read_exact(&mut byte).is_ok() {
                            let _ = stream.write_all(&byte);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            hits,
            stop,
            worker: Some(worker),
        }
    }
    fn udp(dns: bool) -> Self {
        Self::udp_on(dns, false)
    }
    fn udp_on(dns: bool, ipv6: bool) -> Self {
        let socket = UdpSocket::bind(if ipv6 { "[::1]:0" } else { "127.0.0.1:0" }).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (hits_clone, stop_clone) = (hits.clone(), stop.clone());
        let worker = thread::spawn(move || {
            let mut buf = [0u8; 1500];
            while !stop_clone.load(Ordering::Relaxed) {
                match socket.recv_from(&mut buf) {
                    Ok((n, peer)) => {
                        hits_clone.fetch_add(1, Ordering::Relaxed);
                        let response = if dns {
                            dns_answer(&buf[..n])
                        } else {
                            buf[..n].to_vec()
                        };
                        let _ = socket.send_to(&response, peer);
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) => {}
                    Err(_) => break,
                }
            }
        });
        Self {
            port,
            hits,
            stop,
            worker: Some(worker),
        }
    }
    fn count(&self) -> usize {
        self.hits.load(Ordering::Relaxed)
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn dns_query() -> Vec<u8> {
    let mut packet = vec![0x4d, 0x49, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in DNS_NAME.split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 1, 0, 1]);
    packet
}
fn dns_answer(query: &[u8]) -> Vec<u8> {
    if query.len() < 12 {
        return Vec::new();
    }
    let mut answer = query.to_vec();
    answer[2..4].copy_from_slice(&[0x81, 0x80]);
    answer[6..8].copy_from_slice(&[0, 1]);
    answer.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 7]);
    answer
}
fn dns_answers(packet: &[u8]) -> u16 {
    if packet.len() < 12 {
        0
    } else {
        u16::from_be_bytes([packet[6], packet[7]])
    }
}

fn policy_cases(
    source: &str,
    wrong_source: &str,
    wide_target: &str,
    tcp_port: u16,
    udp_port: u16,
) -> [(&'static str, Option<Value>, bool); 6] {
    let ports = [tcp_port.to_string(), udp_port.to_string()];
    [
        ("legacy", None, true),
        (
            "allow",
            Some(json!({"mode":"allowlist","rules":[{
                "sourceCidrs":[source],"network":"both","ports":ports,"target":"local"
            }]})),
            true,
        ),
        (
            "wrong-source",
            Some(json!({"mode":"allowlist","rules":[{
                "sourceCidrs":[wrong_source],"network":"both","ports":ports,"target":"local"
            }]})),
            false,
        ),
        (
            "wide-target-excludes-loopback",
            Some(json!({"mode":"allowlist","rules":[{
                "sourceCidrs":[source],"network":"both","ports":ports,
                "target":"forward","targetCidrs":[wide_target]
            }]})),
            false,
        ),
        (
            "empty-allowlist",
            Some(json!({"mode":"allowlist","rules":[]})),
            false,
        ),
        ("block", Some(json!({"mode":"block"})), false),
    ]
}

fn wg_config(own_a: bool, ipv6: bool, peer_port: u16, policy: Option<Value>) -> UserConfig {
    let (id, name, own_ip, peer_ip, private, peer_public) = if own_a {
        (
            "wg-a",
            "WG-A",
            if ipv6 { A_IP6 } else { A_IP },
            if ipv6 { B_IP6 } else { B_IP },
            A_PRIVATE,
            B_PUBLIC,
        )
    } else {
        (
            "wg-b",
            "WG-B",
            if ipv6 { B_IP6 } else { B_IP },
            if ipv6 { A_IP6 } else { A_IP },
            B_PRIVATE,
            A_PUBLIC,
        )
    };
    let host_prefix = if ipv6 { 128 } else { 32 };
    let mut server = json!({
        "id":id, "name":name, "protocol":"wireguard", "address":"127.0.0.1", "port":peer_port,
        "wireguardSettings":{"privateKey":private,"peerPublicKey":peer_public,
            "localAddress":[format!("{own_ip}/{host_prefix}")], "allowedIPs":[format!("{peer_ip}/{host_prefix}")],
            "allowInternet":false}
    });
    if let Some(policy) = policy {
        server["meshInboundPolicy"] = policy;
    }
    serde_json::from_value(json!({
        "servers":[server], "selectedServerId":id,
        "proxyMode":"smart", "proxyModeType":"systemProxy"
    }))
    .unwrap()
}

#[derive(Clone, Copy)]
struct Probe {
    tag: &'static str,
    listen: u16,
    target: &'static str,
    port: u16,
    udp: bool,
}
fn runtime_config(
    input: UserConfig,
    temp: &TempDir,
    own_port: Option<u16>,
    probes: &[Probe],
    upstream: Option<u16>,
) -> Value {
    let case = SnapshotCase {
        name: "mesh-inbound-runtime".into(),
        platform: match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            _ => "linux",
        }
        .into(),
        input,
    };
    let deps = full_config_deps(&case, temp);
    let has_policy = case.input.servers[0].mesh_inbound_policy.is_some();
    let built = generate_sing_box_config(&case.input, &BTreeMap::new(), &deps).unwrap();
    let mut value = serde_json::to_value(built).unwrap();
    // 测试专用 WG 监听端口，peer 固定拨回环；产品 ServerConfig 不暴露 WG listen_port。
    // StdNetBind 的传输 socket 不保证只绑定回环，不能把它当作系统接口隔离证明。
    // 只在已生成 endpoint 上注入，不修改产品 policy/route/DNS 生成路径。
    if let Some(port) = own_port {
        value["endpoints"][0]["listen_port"] = json!(port);
    }
    let endpoint = value["endpoints"][0]["tag"].as_str().unwrap().to_owned();
    if has_policy {
        assert_eq!(value["route"]["rules"][0]["inbound"], json!([endpoint]));
        assert_eq!(value["dns"]["rules"][0]["inbound"], json!([endpoint]));
        assert_eq!(value["dns"]["rules"][0]["action"], "reject");
    }
    // 失败时完整打印双核 stderr，保留入站选择与路由路径证据；成功时日志留在临时目录。
    value["log"] = json!({"level":"debug", "timestamp":false});
    value["inbounds"] = Value::Array(
        probes
            .iter()
            .map(|p| {
                json!({
                    "type":"direct", "tag":p.tag, "network":if p.udp {"udp"} else {"tcp"},
                    "listen":"127.0.0.1", "listen_port":p.listen,
                    "override_address":p.target, "override_port":p.port
                })
            })
            .collect(),
    );
    value.as_object_mut().unwrap().remove("services");
    let route = value["route"]["rules"].as_array_mut().unwrap();
    for probe in probes.iter().rev() {
        route.insert(
            0,
            if probe.tag == "warm-dns" {
                json!({"inbound":[probe.tag],"action":"hijack-dns"})
            } else {
                json!({"inbound":[probe.tag],"action":"route","outbound":endpoint})
            },
        );
    }
    if let Some(port) = upstream {
        value["dns"]["servers"].as_array_mut().unwrap().push(json!({
            "tag":"mesh-test-upstream","type":"udp","server":"127.0.0.1","server_port":port,
            "detour":"direct"
        }));
        let mut test_rules = Vec::new();
        if has_policy {
            test_rules.push(value["dns"]["rules"][0].clone());
        }
        test_rules.push(json!({
            "domain":[DNS_NAME], "action":"route", "server":"mesh-test-upstream"
        }));
        value["dns"]["rules"] = Value::Array(test_rules);
    }
    value
}

fn write_config(temp: &TempDir, value: &Value) -> PathBuf {
    let path = temp.path().join("mesh.json");
    std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    path
}
struct Running {
    child: Child,
    log: PathBuf,
}
impl Running {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }
    fn assert_alive(&mut self) {
        assert!(
            self.child.try_wait().unwrap().is_none(),
            "core exited: {}",
            self.log()
        );
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn run(core: &Path, config: &Path, temp: &TempDir) -> Running {
    let (ok, diag) = check(core, config);
    assert!(ok, "mesh config rejected by bundled core: {diag}");
    let log = temp.path().join("core.stderr");
    let mut command = command_for_core(core);
    command.arg("--disable-color");
    let child = with_run(command)
        .arg("-c")
        .arg(config)
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .unwrap();
    Running { child, log }
}
fn tcp(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&local(port), Duration::from_millis(600))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    stream.write_all(&[0x59]).is_ok() && {
        let mut echoed = [0u8; 1];
        stream.read_exact(&mut echoed).is_ok() && echoed == [0x59]
    }
}
fn udp(port: u16, packet: &[u8]) -> Option<Vec<u8>> {
    let socket = UdpSocket::bind("127.0.0.1:0").ok()?;
    socket
        .set_read_timeout(Some(Duration::from_millis(600)))
        .ok()?;
    socket.send_to(packet, local(port)).ok()?;
    let mut buf = [0u8; 1500];
    let (n, _) = socket.recv_from(&mut buf).ok()?;
    Some(buf[..n].to_vec())
}
fn eventually(mut predicate: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if predicate() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    false
}

#[test]
fn userspace_wireguard_ingress_enforces_real_tcp_udp_and_cached_dns() {
    if !kernel_run_or_skip("mesh userspace inbound TCP/UDP/DNS runtime") {
        return;
    }
    let Some(core) = core_or_skip("mesh userspace inbound TCP/UDP/DNS runtime") else {
        return;
    };
    for ipv6 in [false, true] {
        wireguard_family(&core, ipv6);
    }
}

fn wireguard_family(core: &Path, ipv6: bool) {
    let (a_ip, b_ip, peer_cidr, wrong_source, wide_target) = if ipv6 {
        (A_IP6, B_IP6, "fd77::2/128", "fd77::99/128", "::/0")
    } else {
        (A_IP, B_IP, "10.77.0.2/32", "10.77.0.99/32", "0.0.0.0/0")
    };
    let family = if ipv6 { "IPv6" } else { "IPv4" };
    let a_wg = free_udp_port();
    let b_wg = free_udp_port();
    let a_tcp = Listener::tcp_on(ipv6);
    let a_denied_tcp = Listener::tcp_on(ipv6);
    let a_udp = Listener::udp_on(false, ipv6);
    let b_tcp = Listener::tcp_on(ipv6);
    let b_udp = Listener::udp_on(false, ipv6);
    let upstream = Listener::udp(true);
    let a_out_tcp = free_tcp_port();
    let a_out_udp = free_udp_port();
    let a_warm_dns = free_udp_port();
    let b_in_tcp = free_tcp_port();
    let b_in_denied_tcp = free_tcp_port();
    let b_in_udp = free_udp_port();
    let b_in_dns = free_udp_port();
    let a_probes = [
        Probe {
            tag: "a-to-b-tcp",
            listen: a_out_tcp,
            target: b_ip,
            port: b_tcp.port,
            udp: false,
        },
        Probe {
            tag: "a-to-b-udp",
            listen: a_out_udp,
            target: b_ip,
            port: b_udp.port,
            udp: true,
        },
        Probe {
            tag: "warm-dns",
            listen: a_warm_dns,
            target: "127.0.0.1",
            port: a_warm_dns,
            udp: true,
        },
    ];
    let b_probes = [
        Probe {
            tag: "b-to-a-tcp",
            listen: b_in_tcp,
            target: a_ip,
            port: a_tcp.port,
            udp: false,
        },
        Probe {
            tag: "b-to-a-denied",
            listen: b_in_denied_tcp,
            target: a_ip,
            port: a_denied_tcp.port,
            udp: false,
        },
        Probe {
            tag: "b-to-a-udp",
            listen: b_in_udp,
            target: a_ip,
            port: a_udp.port,
            udp: true,
        },
        Probe {
            tag: "b-to-a-dns",
            listen: b_in_dns,
            target: a_ip,
            port: 53,
            udp: true,
        },
    ];
    for (label, policy, should_allow) in
        policy_cases(peer_cidr, wrong_source, wide_target, a_tcp.port, a_udp.port)
    {
        let has_policy = policy.is_some();
        // 每轮独立路径与双核，避免旧 UDP flow、peer 状态和磁盘 DNS cache 跨策略轮次复用。
        let a_temp = tempfile::tempdir().unwrap();
        let b_temp = tempfile::tempdir().unwrap();
        let b = runtime_config(
            wg_config(false, ipv6, a_wg, None),
            &b_temp,
            Some(b_wg),
            &b_probes,
            None,
        );
        let b_path = write_config(&b_temp, &b);
        let mut b_core = run(core, &b_path, &b_temp);
        let a = runtime_config(
            wg_config(true, ipv6, b_wg, policy),
            &a_temp,
            Some(a_wg),
            &a_probes,
            Some(upstream.port),
        );
        let a_path = write_config(&a_temp, &a);
        let mut a_core = run(core, &a_path, &a_temp);
        // 正向 A→B 先成功，证明两核隧道及回包通，且 B 学到了 A 的 peer 地址。
        assert!(
            eventually(|| tcp(a_out_tcp)),
            "{family}/{label}: A→B TCP unavailable: A={} / B={}",
            a_core.log(),
            b_core.log()
        );
        assert!(
            eventually(|| udp(a_out_udp, &[0x48]) == Some(vec![0x48])),
            "{family}/{label}: A→B UDP unavailable: A={} / B={}",
            a_core.log(),
            b_core.log()
        );
        assert!(
            b_tcp.count() > 0 && b_udp.count() > 0,
            "{family}/{label}: forward probes missed live services"
        );
        let before_tcp = a_tcp.count();
        let before_udp = a_udp.count();
        if should_allow {
            assert!(
                eventually(|| tcp(b_in_tcp)),
                "allowed B→A TCP failed: {}",
                a_core.log()
            );
            assert!(
                eventually(|| udp(b_in_udp, &[0x55]) == Some(vec![0x55])),
                "allowed B→A UDP failed: {}",
                a_core.log()
            );
            assert!(a_tcp.count() > before_tcp && a_udp.count() > before_udp);
            if has_policy {
                let denied_before = a_denied_tcp.count();
                assert!(
                    !tcp(b_in_denied_tcp),
                    "wrong port unexpectedly reached live TCP service"
                );
                thread::sleep(Duration::from_millis(150));
                assert_eq!(
                    a_denied_tcp.count(),
                    denied_before,
                    "wrong port listener was contacted"
                );
            }
            // 同一本地允许入口连续查询；第二次仍成功但不再访问上游，证明确实已有热缓存。
            let upstream_before_warm = upstream.count();
            assert!(
                eventually(|| udp(a_warm_dns, &dns_query()).is_some_and(|p| dns_answers(&p) > 0)),
                "local warm DNS query did not reach test upstream: {}",
                a_core.log()
            );
            let upstream_before = upstream.count();
            assert!(
                upstream_before > upstream_before_warm,
                "warm DNS query did not reach live upstream"
            );
            assert!(
                udp(a_warm_dns, &dns_query()).is_some_and(|p| dns_answers(&p) > 0),
                "second local DNS query did not return cached answer"
            );
            thread::sleep(Duration::from_millis(150));
            assert_eq!(
                upstream.count(),
                upstream_before,
                "second local DNS query missed cache"
            );
            let protected = udp(b_in_dns, &dns_query());
            if has_policy {
                assert!(
                    protected.as_deref().is_none_or(|p| dns_answers(p) == 0),
                    "protected endpoint returned cached DNS answer"
                );
            } else {
                assert!(
                    protected.as_deref().is_some_and(|p| dns_answers(p) > 0),
                    "legacy endpoint did not return cached DNS answer: {}",
                    a_core.log()
                );
            }
            thread::sleep(Duration::from_millis(150));
            assert_eq!(
                upstream.count(),
                upstream_before,
                "protected DNS query reached upstream"
            );
        } else {
            assert!(!tcp(b_in_tcp), "{label}: denied TCP returned service echo");
            assert_ne!(
                udp(b_in_udp, &[0x55]),
                Some(vec![0x55]),
                "{label}: denied UDP returned service echo"
            );
            thread::sleep(Duration::from_millis(150));
            assert_eq!(
                a_tcp.count(),
                before_tcp,
                "{label}: TCP listener was contacted"
            );
            assert_eq!(
                a_udp.count(),
                before_udp,
                "{label}: UDP listener was contacted"
            );
        }
        let before_forward_tcp = b_tcp.count();
        let before_forward_udp = b_udp.count();
        assert!(
            eventually(|| tcp(a_out_tcp)),
            "{family}/{label}: A→B TCP died after ingress checks: A={} / B={}",
            a_core.log(),
            b_core.log()
        );
        assert!(
            eventually(|| udp(a_out_udp, &[0x49]) == Some(vec![0x49])),
            "{family}/{label}: A→B UDP died after ingress checks: A={} / B={}",
            a_core.log(),
            b_core.log()
        );
        assert!(
            b_tcp.count() > before_forward_tcp && b_udp.count() > before_forward_udp,
            "{family}/{label}: post-check forward probes missed live services"
        );
        a_core.assert_alive();
        b_core.assert_alive();
        eprintln!("mesh runtime passed: WG {family}/{label}");
        drop(a_core);
        drop(b_core);
    }
}

const MASQUE_SERVER_IP: &str = "10.78.0.1";
const MASQUE_A_IP: &str = "10.78.0.2";
const MASQUE_B_IP: &str = "10.78.0.3";
const MASQUE_SERVER_IP6: &str = "fd78::1";
const MASQUE_A_IP6: &str = "fd78::2";
const MASQUE_B_IP6: &str = "fd78::3";

fn masque_client_config(
    own_a: bool,
    ipv6: bool,
    server_port: u16,
    policy: Option<Value>,
) -> UserConfig {
    let (id, username) = if own_a {
        ("mq-a", "mesh-a")
    } else {
        ("mq-b", "mesh-b")
    };
    let mesh_route = if ipv6 { "fd78::/125" } else { "10.78.0.0/29" };
    let mut server = json!({
        "id":id, "name":id, "protocol":"masque-client", "address":"127.0.0.1",
        "port":server_port, "username":username, "password":"fixture-only-password",
        "meshRoutes":[mesh_route],
        "tlsSettings":{"serverName":"localhost","allowInsecure":true},
        "masqueClientSettings":{"version":3,"disable_version_fallback":true}
    });
    if let Some(policy) = policy {
        server["meshInboundPolicy"] = policy;
    }
    serde_json::from_value(json!({
        "servers":[server], "selectedServerId":id,
        "proxyMode":"smart", "proxyModeType":"systemProxy"
    }))
    .unwrap()
}

fn masque_server_config(server_port: u16, ipv6: bool) -> Value {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mesh-masque");
    let pool = if ipv6 {
        format!("{MASQUE_SERVER_IP6}/125")
    } else {
        format!("{MASQUE_SERVER_IP}/29")
    };
    json!({
        "log":{"level":"info","output":"stderr","timestamp":false},
        "inbounds":[], "outbounds":[{"type":"direct","tag":"direct"}],
        "endpoints":[{
            "type":"masque-server", "tag":"mesh-server", "listen":"127.0.0.1",
            "listen_port":server_port, "address":[pool],
            "version":[3],
            "users":[
                {"username":"mesh-a","password":"fixture-only-password"},
                {"username":"mesh-b","password":"fixture-only-password"}
            ],
            "tls":{"enabled":true,
                "certificate_path":fixture.join("cert.pem.fixture"),
                "key_path":fixture.join("key.pem.fixture")}
        }],
        "route":{"final":"direct"}
    })
}

fn server_assigned(log: &str, user: &str, ip: &str) -> bool {
    log.lines().any(|line| {
        line.contains(&format!("[{user}] inbound tunnel from "))
            && line
                .split_once(" assigned ")
                .is_some_and(|(_, addresses)| addresses.split_whitespace().any(|token| token == ip))
    })
}

#[test]
fn userspace_mesh_ipv6_and_masque_configs_check_with_bundled_core() {
    let dual_stack_log = "[mesh-a] inbound tunnel from 127.0.0.1 assigned 10.78.0.2 fd78::2";
    assert!(server_assigned(dual_stack_log, "mesh-a", "fd78::2"));
    assert!(!server_assigned(dual_stack_log, "mesh-b", "fd78::2"));
    assert!(!server_assigned(dual_stack_log, "mesh-a", "fd78::20"));
    let Some(core) = core_or_skip("mesh IPv6/MASQUE config check") else {
        return;
    };
    for (name, config) in [
        (
            "WG IPv6",
            wg_config(true, true, free_udp_port(), Some(json!({"mode":"block"}))),
        ),
        (
            "MASQUE H3 IPv4",
            masque_client_config(true, false, free_udp_port(), Some(json!({"mode":"block"}))),
        ),
        (
            "MASQUE H3 IPv6",
            masque_client_config(true, true, free_udp_port(), Some(json!({"mode":"block"}))),
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let value = runtime_config(config, &temp, None, &[], None);
        let path = write_config(&temp, &value);
        let (ok, diag) = check(&core, &path);
        assert!(ok, "{name} generated config rejected: {diag}");
    }
    for ipv6 in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = write_config(&temp, &masque_server_config(free_udp_port(), ipv6));
        let (ok, diag) = check(&core, &path);
        assert!(
            ok,
            "MASQUE loopback server IPv6={ipv6} config rejected: {diag}"
        );
    }
}

#[test]
fn userspace_masque_h3_ingress_enforces_real_tcp_udp_and_cached_dns() {
    if !kernel_run_or_skip("mesh MASQUE H3 IPv4/IPv6 inbound TCP/UDP/DNS runtime") {
        return;
    }
    let Some(core) = core_or_skip("mesh MASQUE H3 IPv4/IPv6 inbound TCP/UDP/DNS runtime") else {
        return;
    };
    for ipv6 in [false, true] {
        masque_family(&core, ipv6);
    }
}

fn masque_family(core: &Path, ipv6: bool) {
    let (a_ip, b_ip, source, wrong_source, wide_target) = if ipv6 {
        (
            MASQUE_A_IP6,
            MASQUE_B_IP6,
            "fd78::3/128",
            "fd78::2/128",
            "::/0",
        )
    } else {
        (
            MASQUE_A_IP,
            MASQUE_B_IP,
            "10.78.0.3/32",
            "10.78.0.2/32",
            "0.0.0.0/0",
        )
    };
    let family = if ipv6 { "IPv6" } else { "IPv4" };
    let a_tcp = Listener::tcp_on(ipv6);
    let a_denied_tcp = Listener::tcp_on(ipv6);
    let a_udp = Listener::udp_on(false, ipv6);
    let b_tcp = Listener::tcp_on(ipv6);
    let b_udp = Listener::udp_on(false, ipv6);
    let upstream = Listener::udp(true);
    let a_out_tcp = free_tcp_port();
    let a_out_udp = free_udp_port();
    let a_warm_dns = free_udp_port();
    let b_in_tcp = free_tcp_port();
    let b_in_denied_tcp = free_tcp_port();
    let b_in_udp = free_udp_port();
    let b_in_dns = free_udp_port();
    let a_probes = [
        Probe {
            tag: "a-to-b-tcp",
            listen: a_out_tcp,
            target: b_ip,
            port: b_tcp.port,
            udp: false,
        },
        Probe {
            tag: "a-to-b-udp",
            listen: a_out_udp,
            target: b_ip,
            port: b_udp.port,
            udp: true,
        },
        Probe {
            tag: "warm-dns",
            listen: a_warm_dns,
            target: "127.0.0.1",
            port: a_warm_dns,
            udp: true,
        },
    ];
    let b_probes = [
        Probe {
            tag: "b-to-a-tcp",
            listen: b_in_tcp,
            target: a_ip,
            port: a_tcp.port,
            udp: false,
        },
        Probe {
            tag: "b-to-a-denied",
            listen: b_in_denied_tcp,
            target: a_ip,
            port: a_denied_tcp.port,
            udp: false,
        },
        Probe {
            tag: "b-to-a-udp",
            listen: b_in_udp,
            target: a_ip,
            port: a_udp.port,
            udp: true,
        },
        Probe {
            tag: "b-to-a-dns",
            listen: b_in_dns,
            target: a_ip,
            port: 53,
            udp: true,
        },
    ];
    for (label, policy, should_allow) in
        policy_cases(source, wrong_source, wide_target, a_tcp.port, a_udp.port)
    {
        let has_policy = policy.is_some();
        // 每轮独立 server 地址池。固定顺序仅用于此夹具，必须观测当轮实际分配；
        // MASQUE 没有按设备身份持久租约，不能从这里推导重连地址稳定。
        let server_temp = tempfile::tempdir().unwrap();
        let a_temp = tempfile::tempdir().unwrap();
        let b_temp = tempfile::tempdir().unwrap();
        let server_port = free_udp_port();
        let mut server = masque_server_config(server_port, ipv6);
        if ipv6 {
            server["log"]["level"] = json!("debug");
        }
        let server_path = write_config(&server_temp, &server);
        let mut server_core = run(core, &server_path, &server_temp);
        let mut a = runtime_config(
            masque_client_config(true, ipv6, server_port, policy),
            &a_temp,
            None,
            &a_probes,
            Some(upstream.port),
        );
        if ipv6 {
            a["log"]["level"] = json!("debug");
        }
        assert_eq!(a["endpoints"][0]["version"], 3);
        assert_eq!(a["endpoints"][0]["disable_version_fallback"], true);
        let a_path = write_config(&a_temp, &a);
        let mut a_core = run(core, &a_path, &a_temp);
        assert!(
            eventually(|| server_assigned(&server_core.log(), "mesh-a", a_ip)),
            "{family}/{label}: A was not assigned the expected in-session address: {} / {}",
            server_core.log(),
            a_core.log()
        );
        let mut b = runtime_config(
            masque_client_config(false, ipv6, server_port, None),
            &b_temp,
            None,
            &b_probes,
            None,
        );
        if ipv6 {
            b["log"]["level"] = json!("debug");
        }
        assert_eq!(b["endpoints"][0]["version"], 3);
        assert_eq!(b["endpoints"][0]["disable_version_fallback"], true);
        let b_path = write_config(&b_temp, &b);
        let mut b_core = run(core, &b_path, &b_temp);
        assert!(
            eventually(|| server_assigned(&server_core.log(), "mesh-b", b_ip)),
            "{family}/{label}: B was not assigned the expected in-session address: {} / {}",
            server_core.log(),
            b_core.log()
        );
        // A→B 正向 TCP/UDP 每轮都成功；它们的回包同时证明 A 策略没有切断已建流。
        assert!(
            eventually(|| tcp(a_out_tcp)),
            "{family}/{label}: A→B TCP unavailable: {} / {} / {}",
            server_core.log(),
            a_core.log(),
            b_core.log()
        );
        assert!(
            eventually(|| udp(a_out_udp, &[0x48]) == Some(vec![0x48])),
            "{family}/{label}: A→B UDP unavailable: {} / {} / {}",
            server_core.log(),
            a_core.log(),
            b_core.log()
        );
        assert!(
            b_tcp.count() > 0 && b_udp.count() > 0,
            "{family}/{label}: forward MASQUE probes missed live services"
        );
        let before_tcp = a_tcp.count();
        let before_udp = a_udp.count();
        if should_allow {
            assert!(
                eventually(|| tcp(b_in_tcp)),
                "{family}/{label}: B→A TCP unavailable: {}",
                a_core.log()
            );
            assert!(
                eventually(|| udp(b_in_udp, &[0x55]) == Some(vec![0x55])),
                "{family}/{label}: B→A UDP unavailable: {}",
                a_core.log()
            );
            assert!(a_tcp.count() > before_tcp && a_udp.count() > before_udp);
            if has_policy {
                let denied_before = a_denied_tcp.count();
                assert!(
                    !tcp(b_in_denied_tcp),
                    "{family}/{label}: wrong TCP port returned service echo"
                );
                thread::sleep(Duration::from_millis(150));
                assert_eq!(
                    a_denied_tcp.count(),
                    denied_before,
                    "{family}/{label}: wrong TCP port reached listener"
                );
            }
            let upstream_before_warm = upstream.count();
            assert!(
                eventually(|| udp(a_warm_dns, &dns_query()).is_some_and(|p| dns_answers(&p) > 0)),
                "{family}/{label}: local DNS warmup failed: {}",
                a_core.log()
            );
            let upstream_before = upstream.count();
            assert!(
                upstream_before > upstream_before_warm,
                "{family}/{label}: warm DNS missed real upstream"
            );
            assert!(udp(a_warm_dns, &dns_query()).is_some_and(|p| dns_answers(&p) > 0));
            thread::sleep(Duration::from_millis(150));
            assert_eq!(
                upstream.count(),
                upstream_before,
                "{family}/{label}: second local DNS query missed cache"
            );
            let protected = udp(b_in_dns, &dns_query());
            if has_policy {
                assert!(
                    protected.as_deref().is_none_or(|p| dns_answers(p) == 0),
                    "{family}/{label}: protected endpoint returned cached DNS answer"
                );
            } else {
                assert!(
                    protected.as_deref().is_some_and(|p| dns_answers(p) > 0),
                    "legacy MASQUE endpoint did not return cached DNS answer: {}",
                    a_core.log()
                );
            }
            thread::sleep(Duration::from_millis(150));
            assert_eq!(
                upstream.count(),
                upstream_before,
                "{family}/{label}: protected DNS query reached upstream"
            );
        } else {
            assert!(
                !tcp(b_in_tcp),
                "{family}/{label}: denied TCP returned service echo"
            );
            assert_ne!(
                udp(b_in_udp, &[0x55]),
                Some(vec![0x55]),
                "{family}/{label}: denied UDP returned service echo"
            );
            thread::sleep(Duration::from_millis(150));
            assert_eq!(
                a_tcp.count(),
                before_tcp,
                "{family}/{label}: denied TCP contacted listener"
            );
            assert_eq!(
                a_udp.count(),
                before_udp,
                "{family}/{label}: denied UDP contacted listener"
            );
        }
        let before_forward_tcp = b_tcp.count();
        let before_forward_udp = b_udp.count();
        assert!(
            eventually(|| tcp(a_out_tcp)),
            "{family}/{label}: A→B TCP died after MASQUE ingress checks: {} / {}",
            server_core.log(),
            a_core.log()
        );
        assert!(
            eventually(|| udp(a_out_udp, &[0x49]) == Some(vec![0x49])),
            "{family}/{label}: A→B UDP died after MASQUE ingress checks: {} / {}",
            server_core.log(),
            a_core.log()
        );
        assert!(
            b_tcp.count() > before_forward_tcp && b_udp.count() > before_forward_udp,
            "{family}/{label}: post-check MASQUE probes missed live services"
        );
        a_core.assert_alive();
        b_core.assert_alive();
        server_core.assert_alive();
        eprintln!("mesh runtime passed: MASQUE H3/{family}/{label}, A={a_ip}, B={b_ip}");
    }
}
