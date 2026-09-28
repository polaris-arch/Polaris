//! 两个用户态 WG endpoint 的真实入站门。探针与服务只绑定回环，不创建 TUN/System 接口。
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
    fn tcp() -> Self {
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
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
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
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

fn wg_config(own_a: bool, peer_port: u16, policy: Option<Value>) -> UserConfig {
    let (id, name, own_ip, peer_ip, private, peer_public) = if own_a {
        ("wg-a", "WG-A", A_IP, B_IP, A_PRIVATE, B_PUBLIC)
    } else {
        ("wg-b", "WG-B", B_IP, A_IP, B_PRIVATE, A_PUBLIC)
    };
    let mut server = json!({
        "id":id, "name":name, "protocol":"wireguard", "address":"127.0.0.1", "port":peer_port,
        "wireguardSettings":{"privateKey":private,"peerPublicKey":peer_public,
            "localAddress":[format!("{own_ip}/32")], "allowedIPs":[format!("{peer_ip}/32")],
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
    own_port: u16,
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
    let built = generate_sing_box_config(&case.input, &BTreeMap::new(), &deps).unwrap();
    let mut value = serde_json::to_value(built).unwrap();
    // 测试专用回环监听：产品 ServerConfig 不暴露 WG listen_port。
    // 只在已生成 endpoint 上注入，不修改产品 policy/route/DNS 生成路径。
    value["endpoints"][0]["listen_port"] = json!(own_port);
    let endpoint = value["endpoints"][0]["tag"].as_str().unwrap().to_owned();
    if case.input.servers[0].mesh_inbound_policy.is_some() {
        assert_eq!(value["route"]["rules"][0]["inbound"], json!([endpoint]));
        assert_eq!(value["dns"]["rules"][0]["inbound"], json!([endpoint]));
        assert_eq!(value["dns"]["rules"][0]["action"], "reject");
    }
    value["log"] = json!({"level":"warn", "timestamp":false});
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
        let original = value["dns"]["rules"].as_array().unwrap();
        let protected = original[0].clone();
        value["dns"]["rules"] = json!([protected, {
            "domain":[DNS_NAME], "action":"route", "server":"mesh-test-upstream"
        }]);
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
    let a_temp = tempfile::tempdir().unwrap();
    let b_temp = tempfile::tempdir().unwrap();
    let a_wg = free_udp_port();
    let b_wg = free_udp_port();
    let a_tcp = Listener::tcp();
    let a_denied_tcp = Listener::tcp();
    let a_udp = Listener::udp(false);
    let b_tcp = Listener::tcp();
    let b_udp = Listener::udp(false);
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
            target: B_IP,
            port: b_tcp.port,
            udp: false,
        },
        Probe {
            tag: "a-to-b-udp",
            listen: a_out_udp,
            target: B_IP,
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
            target: A_IP,
            port: a_tcp.port,
            udp: false,
        },
        Probe {
            tag: "b-to-a-denied",
            listen: b_in_denied_tcp,
            target: A_IP,
            port: a_denied_tcp.port,
            udp: false,
        },
        Probe {
            tag: "b-to-a-udp",
            listen: b_in_udp,
            target: A_IP,
            port: a_udp.port,
            udp: true,
        },
        Probe {
            tag: "b-to-a-dns",
            listen: b_in_dns,
            target: A_IP,
            port: 53,
            udp: true,
        },
    ];
    let allow = json!({"mode":"allowlist","rules":[{
        "sourceCidrs":["10.77.0.2/32"],"network":"both",
        "ports":[a_tcp.port.to_string(),a_udp.port.to_string()],"target":"local"
    }]});
    for (label, policy, should_allow) in [
        ("allow", allow, true),
        (
            "wrong-source",
            json!({"mode":"allowlist","rules":[{
                "sourceCidrs":["10.77.0.99/32"],"network":"both",
                "ports":[a_tcp.port.to_string(),a_udp.port.to_string()],"target":"local"
            }]}),
            false,
        ),
        (
            "wide-target-excludes-loopback",
            json!({"mode":"allowlist","rules":[{
                "sourceCidrs":["10.77.0.2/32"],"network":"both",
                "ports":[a_tcp.port.to_string(),a_udp.port.to_string()],
                "target":"forward","targetCidrs":["0.0.0.0/0"]
            }]}),
            false,
        ),
        (
            "empty-allowlist",
            json!({"mode":"allowlist","rules":[]}),
            false,
        ),
        ("block", json!({"mode":"block"}), false),
    ] {
        // 每轮两核同建同收，避免 B 的旧 UDP flow/peer 状态跨策略轮次复用。
        let b = runtime_config(wg_config(false, a_wg, None), &b_temp, b_wg, &b_probes, None);
        let b_path = write_config(&b_temp, &b);
        let mut b_core = run(&core, &b_path, &b_temp);
        let a = runtime_config(
            wg_config(true, b_wg, Some(policy)),
            &a_temp,
            a_wg,
            &a_probes,
            Some(upstream.port),
        );
        let a_path = write_config(&a_temp, &a);
        let mut a_core = run(&core, &a_path, &a_temp);
        // 正向 A→B 先成功，证明两核隧道及回包通，且 B 学到了 A 的 peer 地址。
        assert!(
            eventually(|| tcp(a_out_tcp)),
            "{label}: A→B TCP unavailable: {}",
            a_core.log()
        );
        assert!(
            eventually(|| udp(a_out_udp, &[0x48]) == Some(vec![0x48])),
            "{label}: A→B UDP unavailable: {}",
            a_core.log()
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
            // 同一本地允许入口连续查询；第二次仍成功但不再访问上游，证明确实已有热缓存。
            assert!(
                eventually(|| udp(a_warm_dns, &dns_query()).is_some_and(|p| dns_answers(&p) > 0)),
                "local warm DNS query did not reach test upstream: {}",
                a_core.log()
            );
            let upstream_before = upstream.count();
            assert!(
                upstream_before > 0,
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
            assert!(
                protected.as_deref().is_none_or(|p| dns_answers(p) == 0),
                "protected endpoint returned cached DNS answer"
            );
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
        a_core.assert_alive();
        b_core.assert_alive();
        drop(a_core);
        drop(b_core);
    }
}
