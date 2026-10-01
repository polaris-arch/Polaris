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
use polaris_config_engine::user_config::cidr::cidrs_overlap;
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
    assert!(!cidrs_overlap("100.80.8.0/24", "100.80.9.0/24"));
    let mut wire: Value = serde_json::from_str(&polaris_source_probe::repo_file!(
        "ui/src/contracts/mesh-route-state.fixture.json"
    ))
    .unwrap();
    wire["meshRoutePolicy"]["assignments"] = json!([
        {"cidr":"100.80.0.0/16","target":{"kind":"owner","serverId":"ts-a","identityEpoch":"epoch-a"}},
        {"cidr":"100.81.0.0/16","target":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}},
        {"cidr":"100.82.0.0/16","target":{"kind":"unmanaged"}},
        {"cidr":"fd7a:115c:a1e0:1::/64","target":{"kind":"owner","serverId":"ts-a","identityEpoch":"epoch-a"}}
    ]);
    wire["meshRoutePolicy"]["overrides"] = json!([
        {
            "ruleId":"atom-override","scopeCidrs":["100.80.0.0/21"],
            "target":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}
        },
        {
            "ruleId":"and-atom-override","scopeCidrs":["100.80.8.0/23"],
            "target":{"kind":"owner","serverId":"ts-b","identityEpoch":"epoch-b"}
        }
    ]);
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
    let and_matcher = serde_json::from_value(json!({
        "type":"logical","mode":"and","rules":[
            {"ip_cidr":["100.80.8.0/24"]},
            {"ip_cidr":["100.80.9.0/24"]}
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
        scopeable_rule_matchers: BTreeMap::from([
            ("atom-override".into(), matcher),
            ("and-atom-override".into(), and_matcher),
        ]),
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
        ("and-atom-split.test", 1) => vec![v4(100, 80, 8, 1), v4(100, 80, 9, 1)],
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

fn spawn_dns() -> (SocketAddr, mpsc::Receiver<String>, Arc<AtomicBool>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let addr = socket.local_addr().unwrap();
    let (query_tx, query_rx) = mpsc::channel();
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
            while let Some(&size) = data.get(cursor) {
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
            let name = labels.join(".");
            let answers = dns_answers(&name, qtype);
            let _ = query_tx.send(format!("{name} type={qtype} answers={}", answers.len()));
            let mut response = Vec::new();
            response.extend_from_slice(&data[0..2]);
            response.extend_from_slice(&0x8180u16.to_be_bytes());
            response.extend_from_slice(&1u16.to_be_bytes());
            response.extend_from_slice(&(answers.len() as u16).to_be_bytes());
            // DNS header has only NSCOUNT and ARCOUNT after ANCOUNT.
            response.extend_from_slice(&[0; 4]);
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
    (addr, query_rx, stop)
}

#[test]
fn dns_stub_emits_parseable_multi_answer_a_and_empty_aaaa() {
    let (server, queries, stop) = spawn_dns();
    let _stop = StopOnDrop(vec![stop]);
    let client = UdpSocket::bind("127.0.0.1:0").unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    for (qtype, expected) in [
        (1u16, vec![[100, 80, 4, 1], [100, 80, 5, 1]]),
        (28u16, vec![]),
    ] {
        let mut query = vec![0x12, 0x34, 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0];
        for label in ["same-owner", "test"] {
            query.push(label.len() as u8);
            query.extend_from_slice(label.as_bytes());
        }
        query.push(0);
        query.extend_from_slice(&qtype.to_be_bytes());
        query.extend_from_slice(&1u16.to_be_bytes());
        client.send_to(&query, server).unwrap();

        let mut packet = [0u8; 512];
        let (len, source) = client.recv_from(&mut packet).unwrap();
        assert_eq!(source, server);
        let response = &packet[..len];
        let mut header = [0x12, 0x34, 0x81, 0x80, 0, 1, 0, 0, 0, 0, 0, 0];
        header[7] = expected.len() as u8;
        assert_eq!(response.get(..12), Some(header.as_slice()));
        assert_eq!(
            response.get(12..query.len()),
            Some(&query[12..]),
            "DNS question must begin immediately after the 12-byte header"
        );
        let mut offset = query.len();
        for address in expected {
            let end = offset + 16;
            assert_eq!(
                response.get(offset..end),
                Some(
                    [
                        0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 0, 0, 4, address[0], address[1],
                        address[2], address[3],
                    ]
                    .as_slice()
                ),
                "DNS A record must have a valid owner pointer, type, class and RDATA"
            );
            offset = end;
        }
        assert_eq!(offset, response.len(), "unexpected bytes after DNS answers");
        assert_eq!(
            queries.recv_timeout(Duration::from_secs(2)).unwrap(),
            format!("same-owner.test type={qtype} answers={}", header[7])
        );
    }
}

fn spawn_observer() -> (
    SocketAddr,
    mpsc::Receiver<String>,
    mpsc::Receiver<&'static str>,
    Arc<AtomicBool>,
) {
    spawn_observer_with_accept_mode(false)
}

fn spawn_observer_with_accept_mode(
    force_inherited_nonblocking: bool,
) -> (
    SocketAddr,
    mpsc::Receiver<String>,
    mpsc::Receiver<&'static str>,
    Arc<AtomicBool>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let (event_tx, event_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    thread::spawn(move || {
        while !stop_thread.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = event_tx.send("accepted");
                    if force_inherited_nonblocking {
                        stream.set_nonblocking(true).unwrap();
                    }
                    // accept() may inherit the listener's nonblocking mode.
                    // read_exact must wait for fragmented SOCKS messages within
                    // the existing timeout instead of dropping on WouldBlock.
                    stream.set_nonblocking(false).unwrap();
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
                    let _ = event_tx.send("greeted");
                    let mut header = [0u8; 4];
                    if stream.read_exact(&mut header).is_err() {
                        continue;
                    }
                    let _ = event_tx.send("request header received");
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
                    let _ = event_tx.send("target received");
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
    (addr, rx, event_rx, stop)
}

struct SocksFlow {
    status: u8,
    stream: TcpStream,
}

fn open_socks_flow(proxy: SocketAddr, host: &str) -> Result<SocksFlow, String> {
    let mut stream = TcpStream::connect_timeout(&proxy, Duration::from_secs(2))
        .map_err(|error| format!("connect: {error}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream
        .write_all(&[5, 1, 0])
        .map_err(|error| format!("greeting write: {error}"))?;
    let mut method = [0u8; 2];
    stream
        .read_exact(&mut method)
        .map_err(|error| format!("greeting read: {error}"))?;
    if method != [5, 0] {
        return Err(format!("unexpected SOCKS method reply: {method:?}"));
    }
    assert!(host.len() <= u8::MAX as usize);
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&80u16.to_be_bytes());
    stream
        .write_all(&request)
        .map_err(|error| format!("request write: {error}"))?;
    let mut reply = [0u8; 4];
    stream
        .read_exact(&mut reply)
        .map_err(|error| format!("reply header read: {error}"))?;
    if reply[0] != 5 || reply[2] != 0 {
        return Err(format!("invalid SOCKS reply header: {reply:?}"));
    }
    let address_len = match reply[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut len = [0u8; 1];
            stream
                .read_exact(&mut len)
                .map_err(|error| format!("reply domain length read: {error}"))?;
            len[0] as usize
        }
        atyp => return Err(format!("invalid SOCKS reply address type: {atyp}")),
    };
    let mut bound = vec![0u8; address_len + 2];
    stream
        .read_exact(&mut bound)
        .map_err(|error| format!("reply bound address read: {error}"))?;
    Ok(SocksFlow {
        status: reply[1],
        stream,
    })
}

fn drive_sniff(flow: &mut SocksFlow, host: &str) -> Result<(), String> {
    if flow.status != 0 {
        return Err(format!("SOCKS reply rejected with status {}", flow.status));
    }
    let payload = format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n");
    flow.stream
        .write_all(payload.as_bytes())
        .map_err(|error| format!("application payload write: {error}"))
}

fn terminal_rejected(flow: &mut SocksFlow) -> Result<(), String> {
    // b609's sniff read can send SOCKS success before route matching. For an
    // accepted handshake, only a closed application stream proves rejection.
    if flow.status != 0 {
        return Ok(());
    }
    let mut payload = [0u8; 1];
    match flow.stream.read(&mut payload) {
        Ok(0) => Ok(()),
        Ok(_) => Err("application stream received data after expected reject".into()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!("application stream did not reject: {error}")),
    }
}

fn observer_steps(events: &mpsc::Receiver<&'static str>) -> Vec<&'static str> {
    events.try_iter().collect()
}

fn observer_channel_quiet<T: std::fmt::Debug>(
    rx: &mpsc::Receiver<T>,
    label: &str,
) -> Result<(), String> {
    match rx.try_recv() {
        Ok(value) => Err(format!("{label} unexpectedly observed {value:?}")),
        Err(mpsc::TryRecvError::Empty) => Ok(()),
        Err(mpsc::TryRecvError::Disconnected) => Err(format!("{label} observer stopped")),
    }
}

fn observers_quiet(
    a_rx: &mpsc::Receiver<String>,
    b_rx: &mpsc::Receiver<String>,
    a_events: &mpsc::Receiver<&'static str>,
    b_events: &mpsc::Receiver<&'static str>,
    duration: Duration,
) -> Result<(), String> {
    let deadline = Instant::now() + duration;
    loop {
        observer_channel_quiet(a_rx, "owner A target")?;
        observer_channel_quiet(b_rx, "owner B target")?;
        observer_channel_quiet(a_events, "owner A connection")?;
        observer_channel_quiet(b_events, "owner B connection")?;
        if Instant::now() >= deadline {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn expect_dns_answer(queries: &mpsc::Receiver<String>, host: &str, qtype: u16, answers: usize) {
    let wanted = format!("{host} type={qtype} answers={answers}");
    let deadline = Instant::now() + Duration::from_secs(2);
    while let Ok(query) = queries.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        if query == wanted {
            return;
        }
    }
    panic!("controlled DNS answer was not requested: {wanted}");
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

fn route_summary(wire: &Value) -> String {
    let rules = wire["route"]["rules"].as_array().unwrap();
    let interesting: Vec<_> = rules
        .iter()
        .enumerate()
        .filter(|(_, rule)| {
            rule["action"] == "resolve" || rule["outbound"] == "ep-a" || rule["outbound"] == "ep-b"
        })
        .map(|(index, rule)| {
            format!(
                "{index}:{}:{}:ip_cidr={}",
                rule["action"].as_str().unwrap_or("match"),
                rule["outbound"].as_str().unwrap_or("-"),
                rule["ip_cidr"].as_array().map_or(0, Vec::len)
            )
        })
        .collect();
    format!("{} rules; {}", rules.len(), interesting.join(", "))
}

fn core_route_logs(path: &std::path::Path) -> Vec<String> {
    let selected: Vec<String> = std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| {
            [
                "same-owner.test",
                "match[",
                "resolved [",
                "connection closed:",
                "outbound/",
                "dns/",
                "route/",
                "inbound/socks",
            ]
            .iter()
            .any(|needle| line.contains(needle))
        })
        .map(|line| line.chars().take(240).collect())
        .collect();
    selected.into_iter().rev().take(32).rev().collect()
}

#[test]
fn connect_fixture_keeps_dns_stub_and_resolve_before_owner_routes() {
    let dns = SocketAddr::from(([127, 0, 0, 1], 53001));
    let peer = SocketAddr::from(([127, 0, 0, 1], 53002));
    let wire = connect_fixture(dns, peer, peer, peer);
    let servers = wire["dns"]["servers"].as_array().unwrap();
    assert!(servers.iter().any(|server| {
        server["tag"] == "dns-remote"
            && server["type"] == "udp"
            && server["server"] == "127.0.0.1"
            && server["server_port"] == dns.port()
    }));
    let rules = wire["route"]["rules"].as_array().unwrap();
    let resolve = rules
        .iter()
        .position(|rule| rule["action"] == "resolve")
        .unwrap();
    let owner_a = rules
        .iter()
        .position(|rule| rule["outbound"] == "ep-a" && rule["ip_cidr"].is_array())
        .unwrap();
    let owner_b = rules
        .iter()
        .position(|rule| rule["outbound"] == "ep-b" && rule["ip_cidr"].is_array())
        .unwrap();
    assert!(
        resolve < owner_a && resolve < owner_b,
        "{}",
        route_summary(&wire)
    );
    assert!(rules[owner_a]["ip_cidr"]
        .as_array()
        .unwrap()
        .iter()
        .any(|cidr| cidr == "100.80.0.0/16"));
}

#[test]
fn socks_connect_requires_complete_reply() {
    for (reply, expected) in [
        (vec![5, 0, 0, 1], false),
        (vec![5, 0, 0, 1, 127, 0, 0, 1, 0, 80], true),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut greeting = [0u8; 3];
            stream.read_exact(&mut greeting).unwrap();
            assert_eq!(greeting, [5, 1, 0]);
            stream.write_all(&[5, 0]).unwrap();
            let mut request = [0u8; 5];
            stream.read_exact(&mut request).unwrap();
            assert_eq!(&request[..4], &[5, 1, 0, 3]);
            let mut target = vec![0u8; request[4] as usize + 2];
            stream.read_exact(&mut target).unwrap();
            stream.write_all(&reply).unwrap();
        });
        assert_eq!(
            open_socks_flow(addr, "example.test").map(|flow| flow.status) == Ok(0),
            expected
        );
        server.join().unwrap();
    }
}

fn assert_observer_reply_pending(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .unwrap();
    let result = stream.read(&mut [0u8; 1]);
    assert!(
        matches!(&result, Err(error) if matches!(
            error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )),
        "incomplete SOCKS message must stay open without a reply: {result:?}"
    );
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
}

#[test]
fn observer_reads_fragmented_connect_with_inherited_nonblocking() {
    for force_inherited_nonblocking in [false, true] {
        let (addr, targets, events, stop) =
            spawn_observer_with_accept_mode(force_inherited_nonblocking);
        let _stop = StopOnDrop(vec![stop]);
        for (atyp, address, expected) in [
            (1, vec![100, 80, 4, 1], "100.80.4.1"),
            (3, b"\x0cexample.test".to_vec(), "example.test"),
            (
                4,
                "fd7a:115c:a1e0:1::9"
                    .parse::<Ipv6Addr>()
                    .unwrap()
                    .octets()
                    .to_vec(),
                "fd7a:115c:a1e0:1::9",
            ),
        ] {
            let mut stream = TcpStream::connect(addr).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            assert_eq!(
                events.recv_timeout(Duration::from_secs(2)).unwrap(),
                "accepted"
            );
            // The accepted stream has no complete greeting yet. A nonblocking
            // read_exact would drop it rather than wait for the next fragment.
            stream.write_all(&[5]).unwrap();
            assert_observer_reply_pending(&mut stream);
            stream.write_all(&[1, 0]).unwrap();
            let mut method = [0u8; 2];
            stream.read_exact(&mut method).unwrap();
            assert_eq!(method, [5, 0]);
            assert_eq!(
                events.recv_timeout(Duration::from_secs(2)).unwrap(),
                "greeted"
            );

            stream.write_all(&[5, 1]).unwrap();
            assert_observer_reply_pending(&mut stream);
            stream.write_all(&[0, atyp]).unwrap();
            assert_eq!(
                events.recv_timeout(Duration::from_secs(2)).unwrap(),
                "request header received"
            );
            let last = address.len() - 1;
            stream.write_all(&address[..last]).unwrap();
            assert_observer_reply_pending(&mut stream);
            stream.write_all(&[address[last], 0]).unwrap();
            assert_observer_reply_pending(&mut stream);
            assert_eq!(targets.try_recv(), Err(mpsc::TryRecvError::Empty));
            assert_eq!(events.try_recv(), Err(mpsc::TryRecvError::Empty));

            stream.write_all(&[80]).unwrap();
            let mut reply = [0u8; 10];
            stream.read_exact(&mut reply).unwrap();
            assert_eq!(reply, [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
            assert_eq!(
                targets.recv_timeout(Duration::from_secs(2)).unwrap(),
                expected
            );
            assert_eq!(
                events.recv_timeout(Duration::from_secs(2)).unwrap(),
                "target received"
            );
            assert_eq!(targets.try_recv(), Err(mpsc::TryRecvError::Empty));
            assert_eq!(events.try_recv(), Err(mpsc::TryRecvError::Empty));
        }
    }
}

#[test]
fn observer_does_not_publish_truncated_connect() {
    let (addr, targets, events, stop) = spawn_observer_with_accept_mode(true);
    let _stop = StopOnDrop(vec![stop]);
    for (request, complete_header) in [
        (vec![5, 1], false),
        (vec![5, 1, 0, 1, 100, 80], true),
        (vec![5, 1, 0, 3, 5, b'a', b'b'], true),
        (vec![5, 1, 0, 4, 0xfd, 0x7a], true),
        (vec![5, 1, 0, 1, 100, 80, 4, 1, 0], true),
    ] {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(&[5, 1, 0]).unwrap();
        let mut method = [0u8; 2];
        stream.read_exact(&mut method).unwrap();
        assert_eq!(method, [5, 0]);
        stream.write_all(&request).unwrap();
        stream.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(stream.read(&mut [0u8; 1]).unwrap(), 0);
        assert_eq!(targets.try_recv(), Err(mpsc::TryRecvError::Empty));
        let mut expected_events = vec!["accepted", "greeted"];
        if complete_header {
            expected_events.push("request header received");
        }
        assert_eq!(observer_steps(&events), expected_events);
    }
}

#[test]
fn lazy_socks_success_needs_payload_and_terminal_reject() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (payload_tx, payload_rx) = mpsc::channel();
    let (close_tx, close_rx) = mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut greeting = [0u8; 3];
        stream.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [5, 1, 0]);
        stream.write_all(&[5, 0]).unwrap();
        let mut request = [0u8; 5];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(&request[..4], &[5, 1, 0, 3]);
        let mut destination = vec![0u8; request[4] as usize + 2];
        stream.read_exact(&mut destination).unwrap();
        assert_eq!(&destination, b"example.test\0P");
        // Model b609's lazy SOCKS success before sniff, DNS or routing.
        stream
            .write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 80])
            .unwrap();
        let expected = b"GET / HTTP/1.1\r\nHost: example.test\r\n\r\n";
        let mut payload = vec![0u8; expected.len()];
        stream.read_exact(&mut payload).unwrap();
        assert_eq!(&payload, expected);
        payload_tx.send(()).unwrap();
        close_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    });

    let mut flow = open_socks_flow(addr, "example.test").unwrap();
    assert_eq!(flow.status, 0);
    drive_sniff(&mut flow, "example.test").unwrap();
    payload_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    flow.stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    assert!(
        terminal_rejected(&mut flow).is_err(),
        "lazy SOCKS success and an open application stream are not a reject"
    );
    close_tx.send(()).unwrap();
    server.join().unwrap();
    assert!(terminal_rejected(&mut flow).is_ok());
}

#[test]
fn negative_observer_gate_catches_accept_before_target() {
    let (_a_tx, a_rx) = mpsc::channel::<String>();
    let (_b_tx, b_rx) = mpsc::channel::<String>();
    let (a_event_tx, a_events) = mpsc::channel::<&'static str>();
    let (_b_event_tx, b_events) = mpsc::channel::<&'static str>();
    assert!(observers_quiet(&a_rx, &b_rx, &a_events, &b_events, Duration::ZERO).is_ok());
    a_event_tx.send("accepted").unwrap();
    assert!(
        observers_quiet(&a_rx, &b_rx, &a_events, &b_events, Duration::ZERO)
            .unwrap_err()
            .contains("owner A connection")
    );
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
    for logging in [false, true] {
        let mut candidate = wire.clone();
        if logging {
            candidate["log"]["disabled"] = json!(false);
            candidate["log"]["level"] = json!("debug");
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&candidate).unwrap()).unwrap();
        for subcommand in ["check", "format"] {
            let result = command_for_core(&core)
                .arg(subcommand)
                .arg("-c")
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "b609 {subcommand} (debug logging={}): {}",
                logging,
                String::from_utf8_lossy(&result.stderr)
            );
        }
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
    let source = polaris_source_probe::crate_file!("tests/managed_mesh_emission_runtime.rs");
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
    let (dns_addr, dns_rx, dns_stop) = spawn_dns();
    let (a_addr, a_rx, a_events, a_stop) = spawn_observer();
    let (b_addr, b_rx, b_events, b_stop) = spawn_observer();
    let _stop = StopOnDrop(vec![dns_stop, a_stop, b_stop]);
    let inbound = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let mut wire = connect_fixture(dns_addr, inbound, a_addr, b_addr);
    let routes = route_summary(&wire);
    // Only this loopback fixture enables core logging. Failure output below
    // selects route/DNS lines and never dumps the config or endpoint material.
    wire["log"]["disabled"] = json!(false);
    wire["log"]["level"] = json!("debug");
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("d1-connect.json");
    let log_path = temp.path().join("d1-core.log");
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
    let log = std::fs::File::create(&log_path).unwrap();
    let _child = KillOnDrop(
        with_run(command_for_core(&core))
            .arg("-c")
            .arg(&path)
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .unwrap(),
    );
    wait_listening(inbound);

    observers_quiet(&a_rx, &b_rx, &a_events, &b_events, Duration::ZERO).unwrap();
    let mut same_owner = open_socks_flow(inbound, "same-owner.test").unwrap();
    let reply = same_owner.status;
    let probe = drive_sniff(&mut same_owner, "same-owner.test");
    let observed_a = a_rx.recv_timeout(Duration::from_secs(2));
    if reply != 0
        || probe.is_err()
        || !matches!(&observed_a, Ok(target) if ["100.80.4.1", "100.80.5.1"].contains(&target.as_str()))
    {
        panic!(
            "same-owner CONNECT reply={reply:?}, probe={probe:?}, observer-a={observed_a:?}, \
             observer-b={:?}, dns-queries={:?}, a-events={:?}, b-events={:?}, \
             wire-route={routes}, core-route-logs={:?}",
            b_rx.try_iter().collect::<Vec<_>>(),
            dns_rx.try_iter().collect::<Vec<_>>(),
            a_events.try_iter().collect::<Vec<_>>(),
            b_events.try_iter().collect::<Vec<_>>(),
            core_route_logs(&log_path),
        );
    }
    assert_eq!(
        observer_steps(&a_events),
        [
            "accepted",
            "greeted",
            "request header received",
            "target received"
        ]
    );
    observers_quiet(
        &a_rx,
        &b_rx,
        &a_events,
        &b_events,
        Duration::from_millis(100),
    )
    .unwrap();
    expect_dns_answer(&dns_rx, "same-owner.test", 1, 2);
    drop(same_owner);
    for name in [
        "cross-owner.test",
        "cross-reject.test",
        "cross-public.test",
        "cross-release.test",
        "and-atom-split.test",
        "override-domain-fail.test",
        "ipv6-cross.test",
    ] {
        observers_quiet(&a_rx, &b_rx, &a_events, &b_events, Duration::ZERO).unwrap();
        let mut flow = open_socks_flow(inbound, name).unwrap();
        if flow.status == 0 {
            drive_sniff(&mut flow, name).unwrap();
        }
        let terminal = terminal_rejected(&mut flow);
        assert!(
            terminal.is_ok(),
            "{name} did not reject the application stream after SOCKS status {}: \
             {terminal:?}; dns-queries={:?}; core-route-logs={:?}",
            flow.status,
            dns_rx.try_iter().collect::<Vec<_>>(),
            core_route_logs(&log_path),
        );
        observers_quiet(
            &a_rx,
            &b_rx,
            &a_events,
            &b_events,
            Duration::from_millis(100),
        )
        .unwrap_or_else(|error| {
            panic!(
                "{name} reached an owner despite reject: {error}; core-route-logs={:?}",
                core_route_logs(&log_path)
            )
        });
    }
    for name in ["override-atoms.test", "override-domain.test"] {
        observers_quiet(&a_rx, &b_rx, &a_events, &b_events, Duration::ZERO).unwrap();
        let mut flow = open_socks_flow(inbound, name).unwrap();
        assert_eq!(flow.status, 0, "{name} must keep its valid override");
        drive_sniff(&mut flow, name).unwrap();
        let target = b_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|error| {
                panic!(
                    "{name} did not reach owner B: {error}; dns-queries={:?}; \
                 a-events={:?}; b-events={:?}; core-route-logs={:?}",
                    dns_rx.try_iter().collect::<Vec<_>>(),
                    observer_steps(&a_events),
                    observer_steps(&b_events),
                    core_route_logs(&log_path)
                )
            });
        let allowed = if name == "override-atoms.test" {
            ["100.80.2.1", "100.80.3.1"]
        } else {
            ["100.80.2.1", "100.80.4.1"]
        };
        assert!(allowed.contains(&target.as_str()), "{name} dialed {target}");
        assert_eq!(
            observer_steps(&b_events),
            [
                "accepted",
                "greeted",
                "request header received",
                "target received"
            ]
        );
        observers_quiet(
            &a_rx,
            &b_rx,
            &a_events,
            &b_events,
            Duration::from_millis(100),
        )
        .unwrap();
        expect_dns_answer(&dns_rx, name, 1, 2);
        drop(flow);
    }
    observers_quiet(&a_rx, &b_rx, &a_events, &b_events, Duration::ZERO).unwrap();
    let mut ipv6_owner = open_socks_flow(inbound, "ipv6-owner.test").unwrap();
    assert_eq!(ipv6_owner.status, 0);
    drive_sniff(&mut ipv6_owner, "ipv6-owner.test").unwrap();
    let ipv6_target = a_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap_or_else(|error| {
            panic!(
                "ipv6-owner.test did not reach owner A: {error}; dns-queries={:?}; \
             a-events={:?}; b-events={:?}; core-route-logs={:?}",
                dns_rx.try_iter().collect::<Vec<_>>(),
                observer_steps(&a_events),
                observer_steps(&b_events),
                core_route_logs(&log_path)
            )
        });
    assert_eq!(ipv6_target, "fd7a:115c:a1e0:1::9");
    assert_eq!(
        observer_steps(&a_events),
        [
            "accepted",
            "greeted",
            "request header received",
            "target received"
        ]
    );
    observers_quiet(
        &a_rx,
        &b_rx,
        &a_events,
        &b_events,
        Duration::from_millis(100),
    )
    .unwrap();
    expect_dns_answer(&dns_rx, "ipv6-owner.test", 28, 1);
}
