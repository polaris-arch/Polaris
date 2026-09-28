//! Fixed-core CONNECT regression for the *emitted* managed route block.
//! Local POLARIS_NO_KERNEL_RUN=1 skips the run; CI executes via the single
//! support/kernel_run.rs entry. Only loopback DNS, inbound and SOCKS observers
//! are used. The Tailscale endpoint substrate is replaced by those observers
//! after pure emission so the test isolates b609 route matching/dial behavior.

#[path = "support/core_locator.rs"]
mod core_locator;
#[path = "support/kernel_run.rs"]
mod kernel_run;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use core_locator::{command_for_core, core_or_skip};
use kernel_run::{kernel_run_or_skip, with_run};
use polaris_config_engine::builder::managed_mesh_emission::emit_managed_mesh_config;
use polaris_config_engine::builder::managed_mesh_plan::{
    compile_managed_mesh_plan, ManagedMeshCandidate, ManagedMeshPlanInput,
};
use polaris_config_engine::singbox::SingBoxConfig;
use polaris_config_engine::user_config::mesh_route_state::{
    MeshOwnerRef, MeshRoutePolicy, MeshRouteState,
};
use serde_json::{json, Value};

fn owner(server_id: &str, identity_epoch: &str) -> MeshOwnerRef {
    MeshOwnerRef {
        server_id: server_id.into(),
        identity_epoch: identity_epoch.into(),
    }
}

fn plan_input() -> ManagedMeshPlanInput {
    let mut wire: Value = serde_json::from_str(include_str!(
        "../../../ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    wire["meshRoutePolicy"]["assignments"] = json!([
        {"cidr":"100.80.0.0/16","target":{"kind":"owner","serverId":"ts-a","identityEpoch":"epoch-a"}},
        {"cidr":"100.81.0.0/16","target":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}},
        {"cidr":"100.82.0.0/16","target":{"kind":"unmanaged"}},
        {"cidr":"fd7a:115c:a1e0:1::/64","target":{"kind":"owner","serverId":"ts-a","identityEpoch":"epoch-a"}}
    ]);
    wire["meshRoutePolicy"]["overrides"] = json!([{
        "ruleId":"atom-override","scopeCidrs":["100.80.0.0/21"],
        "target":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}
    }]);
    wire["meshRouteState"]["identities"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "serverId":"ts-b","identityEpoch":"epoch-b",
            "controlAuthority":"https://other.example.invalid/",
            "bindingState":"bound","evidenceSource":"status"
        }));
    wire["meshRouteState"]["observations"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "ownerRef":{"serverId":"ts-b","identityEpoch":"epoch-b"},
            "rawHosts":[],"advertisedRoutes":["100.81.0.0/16"],
            "source":"status","lastValidEvidence":"status-b"
        }));
    let matcher = serde_json::from_value(json!({
        "type":"logical","mode":"or","rules":[
            {"domain_suffix":["override-domain.test"]},
            {"ip_cidr":["100.80.2.0/24"]},
            {"ip_cidr":["100.80.3.0/24"]}
        ]
    }))
    .unwrap();
    ManagedMeshPlanInput {
        plan_id: "b609-connect".into(),
        config_version: "connect-1".into(),
        policy: serde_json::from_value::<MeshRoutePolicy>(wire["meshRoutePolicy"].clone()).unwrap(),
        state: serde_json::from_value::<MeshRouteState>(wire["meshRouteState"].clone()).unwrap(),
        candidates: vec![
            ManagedMeshCandidate {
                owner_ref: owner("ts-a", "epoch-a"),
                configured_cidrs: vec!["100.80.0.0/16".into(), "fd7a:115c:a1e0:1::/64".into()],
                endpoint_tag: Some("ep-a".into()),
                evidence_complete: true,
            },
            ManagedMeshCandidate {
                owner_ref: owner("ts-b", "epoch-b"),
                configured_cidrs: vec!["100.81.0.0/16".into()],
                endpoint_tag: Some("ep-b".into()),
                evidence_complete: true,
            },
        ],
        scopeable_rule_matchers: BTreeMap::from([("atom-override".into(), matcher)]),
    }
}

fn legacy(dns: SocketAddr, listen_port: u16) -> SingBoxConfig {
    serde_json::from_value(json!({
        "log":{"level":"error","timestamp":false,"disabled":true},
        "dns":{
            "servers":[
                {"tag":"dns-mdns","type":"mdns"},
                {"tag":"dns-lan","type":"local"},
                {"tag":"dns-remote","type":"udp","server":dns.ip().to_string(),"server_port":dns.port()}
            ],
            "rules":[
                {"domain_suffix":["local"],"server":"dns-mdns"},
                {"domain_regex":["^[^.]+\\.?$"],"server":"dns-lan"},
                {"query_type":["A","AAAA"],"server":"dns-remote"}
            ],
            "final":"dns-remote","strategy":"prefer_ipv4"
        },
        "inbounds":[{"type":"socks","tag":"test-in","listen":"127.0.0.1","listen_port":listen_port}],
        "outbounds":[{"type":"direct","tag":"direct"}],
        "endpoints":[
            {"type":"tailscale","tag":"ep-a","auth_key":"tskey-auth-example","state_directory":"/tmp/d1-test-a","control_url":"https://localhost","hostname":"d1-a"},
            {"type":"tailscale","tag":"ep-b","auth_key":"tskey-auth-example","state_directory":"/tmp/d1-test-b","control_url":"https://localhost","hostname":"d1-b"}
        ],
        "route":{
            "rules":[
                {"action":"sniff"},
                {"process_name":["sing-box","sing-box.exe"],"action":"route","outbound":"direct"},
                {"ip_cidr":["223.5.5.5/32"],"port":[53,443],"action":"route","outbound":"direct"},
                {"port":[53],"action":"hijack-dns"}
            ],
            "default_domain_resolver":"dns-remote","final":"direct"
        }
    }))
    .unwrap()
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn dns_answers(name: &str, qtype: u16) -> Vec<Vec<u8>> {
    let v4 = |a, b, c, d| Ipv4Addr::new(a, b, c, d).octets().to_vec();
    match (name, qtype) {
        ("same-owner.test", 1) => vec![v4(100, 80, 4, 1), v4(100, 80, 5, 1)],
        ("cross-owner.test", 1) => vec![v4(100, 80, 4, 1), v4(100, 81, 4, 1)],
        ("cross-reject.test", 1) => vec![v4(100, 80, 4, 1), v4(100, 90, 4, 1)],
        ("cross-public.test", 1) => vec![v4(100, 80, 4, 1), v4(203, 0, 113, 5)],
        ("cross-release.test", 1) => vec![v4(100, 80, 4, 1), v4(100, 82, 4, 1)],
        ("override-atoms.test", 1) => vec![v4(100, 80, 2, 1), v4(100, 80, 3, 1)],
        ("override-domain.test", 1) => vec![v4(100, 80, 2, 1), v4(100, 80, 4, 1)],
        ("override-domain-fail.test", 1) => vec![v4(100, 80, 2, 1), v4(100, 80, 4, 1)],
        ("ipv6-owner.test", 28) => vec!["fd7a:115c:a1e0:1::9"
            .parse::<Ipv6Addr>()
            .unwrap()
            .octets()
            .to_vec()],
        ("ipv6-cross.test", 28) => ["fd7a:115c:a1e0:1::9", "fd7a:115c:a1e0:2::9"]
            .iter()
            .map(|ip| ip.parse::<Ipv6Addr>().unwrap().octets().to_vec())
            .collect(),
        _ => vec![],
    }
}

fn spawn_dns() -> (SocketAddr, Arc<AtomicBool>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let addr = socket.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    thread::spawn(move || {
        let mut packet = [0u8; 2048];
        while !stop_thread.load(Ordering::Acquire) {
            let Ok((len, peer)) = socket.recv_from(&mut packet) else {
                continue;
            };
            let data = &packet[..len];
            let mut cursor = 12;
            let mut labels = Vec::new();
            loop {
                let Some(&size) = data.get(cursor) else { break };
                cursor += 1;
                if size == 0 {
                    break;
                }
                let Some(label) = data.get(cursor..cursor + size as usize) else {
                    break;
                };
                labels.push(String::from_utf8_lossy(label).to_string());
                cursor += size as usize;
            }
            let Some(question_end) = cursor.checked_add(4) else {
                continue;
            };
            if data.len() < question_end {
                continue;
            }
            let qtype = u16::from_be_bytes([data[cursor], data[cursor + 1]]);
            let answers = dns_answers(&labels.join("."), qtype);
            let mut response = Vec::new();
            response.extend_from_slice(&data[0..2]);
            response.extend_from_slice(&0x8180u16.to_be_bytes());
            response.extend_from_slice(&1u16.to_be_bytes());
            response.extend_from_slice(&(answers.len() as u16).to_be_bytes());
            response.extend_from_slice(&[0; 6]);
            response.extend_from_slice(&data[12..question_end]);
            for answer in answers {
                response.extend_from_slice(&[0xc0, 0x0c]);
                response.extend_from_slice(&qtype.to_be_bytes());
                response.extend_from_slice(&1u16.to_be_bytes());
                response.extend_from_slice(&0u32.to_be_bytes());
                response.extend_from_slice(&(answer.len() as u16).to_be_bytes());
                response.extend_from_slice(&answer);
            }
            let _ = socket.send_to(&response, peer);
        }
    });
    (addr, stop)
}

fn spawn_observer() -> (SocketAddr, mpsc::Receiver<String>, Arc<AtomicBool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    thread::spawn(move || {
        while !stop_thread.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                    let mut greeting = [0u8; 2];
                    if stream.read_exact(&mut greeting).is_err() {
                        continue;
                    }
                    let mut methods = vec![0u8; greeting[1] as usize];
                    if stream.read_exact(&mut methods).is_err() {
                        continue;
                    }
                    if stream.write_all(&[5, 0]).is_err() {
                        continue;
                    }
                    let mut header = [0u8; 4];
                    if stream.read_exact(&mut header).is_err() {
                        continue;
                    }
                    let target = match header[3] {
                        1 => {
                            let mut ip = [0u8; 4];
                            if stream.read_exact(&mut ip).is_err() {
                                continue;
                            }
                            Ipv4Addr::from(ip).to_string()
                        }
                        4 => {
                            let mut ip = [0u8; 16];
                            if stream.read_exact(&mut ip).is_err() {
                                continue;
                            }
                            Ipv6Addr::from(ip).to_string()
                        }
                        3 => {
                            let mut size = [0u8; 1];
                            if stream.read_exact(&mut size).is_err() {
                                continue;
                            }
                            let mut name = vec![0u8; size[0] as usize];
                            if stream.read_exact(&mut name).is_err() {
                                continue;
                            }
                            String::from_utf8_lossy(&name).to_string()
                        }
                        _ => continue,
                    };
                    let mut port = [0u8; 2];
                    if stream.read_exact(&mut port).is_err() {
                        continue;
                    }
                    let _ = tx.send(target);
                    let _ = stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
    });
    (addr, rx, stop)
}

fn connect(proxy: SocketAddr, host: &str) -> bool {
    let Ok(mut stream) = TcpStream::connect_timeout(&proxy, Duration::from_secs(2)) else {
        return false;
    };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    if stream.write_all(&[5, 1, 0]).is_err() {
        return false;
    }
    let mut method = [0u8; 2];
    if stream.read_exact(&mut method).is_err() || method != [5, 0] {
        return false;
    }
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&80u16.to_be_bytes());
    if stream.write_all(&request).is_err() {
        return false;
    }
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).is_ok() && reply[1] == 0
}

fn wait_listening(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("fixed core did not listen on {addr}");
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct StopOnDrop(Vec<Arc<AtomicBool>>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        for stop in &self.0 {
            stop.store(true, Ordering::Release);
        }
    }
}

fn connect_fixture(
    dns_addr: SocketAddr,
    inbound: SocketAddr,
    a: SocketAddr,
    b: SocketAddr,
) -> Value {
    let input = plan_input();
    let plan = compile_managed_mesh_plan(input.clone()).unwrap();
    let mut config = emit_managed_mesh_config(&legacy(dns_addr, inbound.port()), &input, &plan)
        .unwrap()
        .config;
    config.endpoints = None;
    let mut wire = serde_json::to_value(config).unwrap();
    wire["outbounds"].as_array_mut().unwrap().extend([
        json!({"type":"socks","tag":"ep-a","server":"127.0.0.1","server_port":a.port(),"version":"5"}),
        json!({"type":"socks","tag":"ep-b","server":"127.0.0.1","server_port":b.port(),"version":"5"}),
    ]);
    wire
}

fn fixed_core_or_skip(what: &str) -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("POLARIS_TEST_CORE") {
        let path = std::path::PathBuf::from(path);
        assert!(
            path.is_file(),
            "POLARIS_TEST_CORE is absent: {}",
            path.display()
        );
        return Some(path);
    }
    core_or_skip(what)
}

#[test]
fn b609_accepts_emitted_connect_fixture_without_starting_core() {
    let Some(core) = fixed_core_or_skip("D1 managed CONNECT config") else {
        return;
    };
    let loopback = SocketAddr::from(([127, 0, 0, 1], 53000));
    let wire = connect_fixture(loopback, loopback, loopback, loopback);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("d1-connect-check.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&wire).unwrap()).unwrap();
    for subcommand in ["check", "format"] {
        let result = command_for_core(&core)
            .arg(subcommand)
            .arg("-c")
            .arg(&path)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "b609 {subcommand}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn release_risk_mandatory_core_job_runs_both_managed_tests() {
    let workflow = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/release-risk.yml"
    ))
    .unwrap();
    let mandatory = workflow
        .split("- name: Run mandatory bundled-core gates")
        .nth(1)
        .unwrap()
        .split("\n  package:")
        .next()
        .unwrap();
    assert!(mandatory.contains("POLARIS_REQUIRE_KERNEL_GATE: '1'"));
    assert!(mandatory.contains(
        "cargo test -p polaris-config-engine --test managed_mesh_emission_runtime -- --nocapture"
    ));
    assert!(!mandatory.contains("POLARIS_NO_KERNEL_RUN"));
    let source = include_str!("managed_mesh_emission_runtime.rs");
    assert!(source.contains("fn b609_accepts_emitted_connect_fixture_without_starting_core"));
    assert!(source.contains("fn b609_connect_observes_managed_multi_answer_guards"));
}

#[test]
fn b609_connect_observes_managed_multi_answer_guards() {
    if !kernel_run_or_skip("D1 managed multi-answer CONNECT") {
        return;
    }
    let Some(core) = fixed_core_or_skip("D1 managed multi-answer CONNECT") else {
        return;
    };
    let (dns_addr, dns_stop) = spawn_dns();
    let (a_addr, a_rx, a_stop) = spawn_observer();
    let (b_addr, b_rx, b_stop) = spawn_observer();
    let _stop = StopOnDrop(vec![dns_stop, a_stop, b_stop]);
    let inbound = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let wire = connect_fixture(dns_addr, inbound, a_addr, b_addr);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("d1-connect.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&wire).unwrap()).unwrap();
    let check = command_for_core(&core)
        .args(["check", "-c"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let _child = KillOnDrop(
        with_run(command_for_core(&core))
            .arg("-c")
            .arg(&path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_listening(inbound);

    assert!(connect(inbound, "same-owner.test"));
    assert!(a_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .starts_with("100.80."));
    for name in [
        "cross-owner.test",
        "cross-reject.test",
        "cross-public.test",
        "cross-release.test",
        "override-domain-fail.test",
        "ipv6-cross.test",
    ] {
        assert!(!connect(inbound, name), "{name} must reject before dialing");
        assert!(
            a_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "{name} reached owner A"
        );
        assert!(
            b_rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "{name} reached owner B"
        );
    }
    for name in ["override-atoms.test", "override-domain.test"] {
        assert!(
            connect(inbound, name),
            "{name} must keep its valid override"
        );
        assert!(b_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .starts_with("100.80."));
    }
    assert!(connect(inbound, "ipv6-owner.test"));
    assert_eq!(
        a_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        "fd7a:115c:a1e0:1::9"
    );
}
