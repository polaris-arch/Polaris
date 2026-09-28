//! subscription-update-in 的随包 sing-box 1.14 地面门。
//!
//! `check` 只能证明 JSON 可解码；本门还真起核心、真走 SOCKS inbound，并用受控 DNS 与上游
//! SOCKS 观察器证明 resolve → private reject → route 的运行顺序。危险地址若抵达上游观察器即失败。
//!
//! **本机禁起核**：`POLARIS_NO_KERNEL_RUN=1`（本机 `scripts/gate-rust.sh` 自动设）下跳过，覆盖交给 CI。

#[path = "support/core_locator.rs"]
mod core_locator;
#[path = "support/kernel_run.rs"]
mod kernel_run;

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use core_locator::{command_for_core, core_or_skip};
use kernel_run::{kernel_run_or_skip, with_run};
use polaris_config_engine::builder::subscription_guard::{
    subscription_update_route_rules, SUBSCRIPTION_UPDATE_INBOUND_TAG,
};
use serde_json::{json, Value};
use tempfile::tempdir;

struct Running {
    child: Child,
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct FixtureWorker {
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Drop for FixtureWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn parse_dns_question(packet: &[u8]) -> Option<(String, u16, usize)> {
    if packet.len() < 17 {
        return None;
    }
    let mut cursor = 12;
    let mut labels = Vec::new();
    loop {
        let len = *packet.get(cursor)? as usize;
        cursor += 1;
        if len == 0 {
            break;
        }
        labels.push(std::str::from_utf8(packet.get(cursor..cursor + len)?).ok()?);
        cursor += len;
    }
    let qtype = u16::from_be_bytes([*packet.get(cursor)?, *packet.get(cursor + 1)?]);
    Some((labels.join("."), qtype, cursor + 4))
}

fn dns_answers(name: &str, qtype: u16, rebind_queries: &AtomicUsize) -> Vec<Vec<u8>> {
    match (name, qtype) {
        ("public.test", 1) => vec![Ipv4Addr::new(203, 0, 113, 10).octets().to_vec()],
        ("fakeip.test", 1) => vec![Ipv4Addr::new(198, 18, 1, 2).octets().to_vec()],
        ("loopback.test", 1) => vec![Ipv4Addr::LOCALHOST.octets().to_vec()],
        ("private.test", 1) => vec![Ipv4Addr::new(10, 1, 2, 3).octets().to_vec()],
        ("cgnat.test", 1) => vec![Ipv4Addr::new(100, 64, 1, 2).octets().to_vec()],
        ("linklocal.test", 1) => vec![Ipv4Addr::new(169, 254, 1, 2).octets().to_vec()],
        ("mixed.test", 1) => vec![
            Ipv4Addr::new(203, 0, 113, 20).octets().to_vec(),
            Ipv4Addr::LOCALHOST.octets().to_vec(),
        ],
        ("rebind.test", 1) => {
            if rebind_queries.fetch_add(1, Ordering::SeqCst) == 0 {
                vec![Ipv4Addr::new(203, 0, 113, 30).octets().to_vec()]
            } else {
                vec![Ipv4Addr::LOCALHOST.octets().to_vec()]
            }
        }
        ("ula.test", 28) => vec!["fd00::1".parse::<Ipv6Addr>().unwrap().octets().to_vec()],
        ("mapped.test", 28) => vec!["::ffff:127.0.0.1"
            .parse::<Ipv6Addr>()
            .unwrap()
            .octets()
            .to_vec()],
        _ => Vec::new(),
    }
}

fn spawn_dns() -> (SocketAddr, FixtureWorker, Arc<AtomicUsize>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let addr = socket.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let rebind_queries = Arc::new(AtomicUsize::new(0));
    let stop_thread = Arc::clone(&stop);
    let rebind_thread = Arc::clone(&rebind_queries);
    let worker = thread::spawn(move || {
        let mut packet = [0u8; 2048];
        while !stop_thread.load(Ordering::Acquire) {
            let Ok((len, peer)) = socket.recv_from(&mut packet) else {
                continue;
            };
            let Some((name, qtype, question_end)) = parse_dns_question(&packet[..len]) else {
                continue;
            };
            let answers = dns_answers(&name, qtype, &rebind_thread);
            let mut response = Vec::with_capacity(512);
            response.extend_from_slice(&packet[0..2]);
            response.extend_from_slice(&0x8180u16.to_be_bytes());
            response.extend_from_slice(&1u16.to_be_bytes());
            response.extend_from_slice(&(answers.len() as u16).to_be_bytes());
            response.extend_from_slice(&0u16.to_be_bytes());
            response.extend_from_slice(&0u16.to_be_bytes());
            response.extend_from_slice(&packet[12..question_end]);
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
    (
        addr,
        FixtureWorker {
            stop,
            worker: Some(worker),
        },
        rebind_queries,
    )
}

fn read_before(stream: &mut TcpStream, mut bytes: &mut [u8], deadline: Instant) -> Option<()> {
    while !bytes.is_empty() {
        stream
            .set_read_timeout(Some(deadline.checked_duration_since(Instant::now())?))
            .ok()?;
        let len = stream.read(bytes).ok()?;
        if len == 0 {
            return None;
        }
        bytes = &mut bytes[len..];
    }
    Some(())
}

fn read_socks_target(stream: &mut TcpStream) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut greeting = [0u8; 2];
    read_before(stream, &mut greeting, deadline)?;
    let mut methods = vec![0u8; greeting[1] as usize];
    read_before(stream, &mut methods, deadline)?;
    stream.write_all(&[5, 0]).ok()?;
    let mut request = [0u8; 4];
    read_before(stream, &mut request, deadline)?;
    let target = match request[3] {
        1 => {
            let mut raw = [0u8; 4];
            read_before(stream, &mut raw, deadline)?;
            Ipv4Addr::from(raw).to_string()
        }
        3 => {
            let mut len = [0u8; 1];
            read_before(stream, &mut len, deadline)?;
            let mut raw = vec![0u8; len[0] as usize];
            read_before(stream, &mut raw, deadline)?;
            String::from_utf8(raw).ok()?
        }
        4 => {
            let mut raw = [0u8; 16];
            read_before(stream, &mut raw, deadline)?;
            Ipv6Addr::from(raw).to_string()
        }
        _ => return None,
    };
    let mut port = [0u8; 2];
    read_before(stream, &mut port, deadline)?;
    stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).ok()?;
    Some(target)
}

fn spawn_upstream_socks() -> (SocketAddr, mpsc::Receiver<String>, FixtureWorker) {
    spawn_upstream_socks_with_accept_mode(false)
}

fn spawn_upstream_socks_with_accept_mode(
    force_inherited_nonblocking: bool,
) -> (SocketAddr, mpsc::Receiver<String>, FixtureWorker) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        // Keep a successful upstream connection alive until the fixture stops,
        // so the core's client reply does not depend on when this fixture FINs.
        let mut connections = Vec::new();
        while !stop_thread.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    if force_inherited_nonblocking {
                        stream.set_nonblocking(true).unwrap();
                    }
                    // accept() can inherit nonblocking mode from the listener. This
                    // fixture uses bounded blocking reads for the SOCKS exchange.
                    stream.set_nonblocking(false).unwrap();
                    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
                    if let Some(target) = read_socks_target(&mut stream) {
                        connections.push(stream);
                        let _ = tx.send(target);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    (
        addr,
        rx,
        FixtureWorker {
            stop,
            worker: Some(worker),
        },
    )
}

fn socks_connect_detail(proxy: SocketAddr, host: &str) -> Result<(), String> {
    let mut stream = TcpStream::connect_timeout(&proxy, Duration::from_secs(2))
        .map_err(|error| format!("connect: {:?}", error.kind()))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("read timeout: {:?}", error.kind()))?;
    stream
        .write_all(&[5, 1, 0])
        .map_err(|error| format!("greeting write: {:?}", error.kind()))?;
    let mut method = [0u8; 2];
    stream
        .read_exact(&mut method)
        .map_err(|error| format!("method read: {:?}", error.kind()))?;
    if method != [5, 0] {
        return Err(format!("method reply: {method:?}"));
    }
    let mut request = vec![5, 1, 0, 3, host.len() as u8];
    request.extend_from_slice(host.as_bytes());
    request.extend_from_slice(&80u16.to_be_bytes());
    stream
        .write_all(&request)
        .map_err(|error| format!("connect write: {:?}", error.kind()))?;
    let mut reply = [0u8; 4];
    stream
        .read_exact(&mut reply)
        .map_err(|error| format!("connect reply read: {:?}", error.kind()))?;
    if reply[1] != 0 {
        return Err(format!("connect reply status: {}", reply[1]));
    }
    Ok(())
}

fn socks_connect(proxy: SocketAddr, host: &str) -> bool {
    socks_connect_detail(proxy, host).is_ok()
}

fn core_error_tail(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return "core stderr unavailable".to_owned();
    };
    String::from_utf8_lossy(&bytes[bytes.len().saturating_sub(4096)..]).into_owned()
}

#[test]
fn upstream_socks_fixture_keeps_successful_connection_until_stop() {
    let (addr, observed, worker) = spawn_upstream_socks_with_accept_mode(true);
    let connect = |host: &str| {
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(&[5, 1, 0]).unwrap();
        let mut method = [0u8; 2];
        stream.read_exact(&mut method).unwrap();
        assert_eq!(method, [5, 0]);
        thread::sleep(Duration::from_millis(40));
        let mut request = vec![5, 1, 0, 3, host.len() as u8];
        request.extend_from_slice(host.as_bytes());
        request.extend_from_slice(&80u16.to_be_bytes());
        stream.write_all(&request).unwrap();
        let mut reply = [0u8; 10];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        stream
    };
    let mut first = connect("public.test");
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(1)).unwrap(),
        "public.test"
    );
    let mut second = connect("fakeip.test");
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(1)).unwrap(),
        "fakeip.test"
    );
    let mut byte = [0u8; 1];
    for stream in [&mut first, &mut second] {
        stream
            .set_read_timeout(Some(Duration::from_millis(150)))
            .unwrap();
        let error = stream.read(&mut byte).unwrap_err();
        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "upstream fixture closed before Stop: {error}"
        );
    }
    drop(worker);
    for stream in [&mut first, &mut second] {
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        match stream.read(&mut byte) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::BrokenPipe
                ) => {}
            result => panic!("upstream fixture did not close after Stop: {result:?}"),
        }
    }
}

fn wait_listening(addr: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&addr, Duration::from_millis(50)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("sing-box SOCKS inbound 未在时限内监听 {addr}");
}

fn config(inbound_port: u16, dns_addr: SocketAddr, upstream_addr: SocketAddr) -> Value {
    let guard_rules = subscription_update_route_rules("fixture-exit");
    json!({
        "log": {"level": "error"},
        "dns": {
            "servers": [{
                "type": "udp",
                "tag": "dns-remote",
                "server": dns_addr.ip().to_string(),
                "server_port": dns_addr.port()
            }],
            "final": "dns-remote",
            "strategy": "prefer_ipv4"
        },
        "inbounds": [{
            "type": "socks",
            "tag": SUBSCRIPTION_UPDATE_INBOUND_TAG,
            "listen": "127.0.0.1",
            "listen_port": inbound_port
        }],
        "outbounds": [{
            "type": "socks",
            "tag": "fixture-exit",
            "server": upstream_addr.ip().to_string(),
            "server_port": upstream_addr.port(),
            "version": "5"
        }],
        "route": {
            "rules": guard_rules,
            "final": "fixture-exit"
        }
    })
}

#[test]
fn bundled_core_enforces_subscription_update_guard_at_runtime() {
    if !kernel_run_or_skip("subscription-update-in 真运行安全门") {
        return;
    }
    let Some(core) = core_or_skip("subscription-update-in 真运行安全门") else {
        return;
    };
    let (dns_addr, _dns_worker, rebind_queries) = spawn_dns();
    let (upstream_addr, observed, _upstream_worker) = spawn_upstream_socks();
    let inbound_addr = SocketAddr::from(([127, 0, 0, 1], free_tcp_port()));
    let temp = tempdir().unwrap();
    let config_path = temp.path().join("subscription-update-guard.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec_pretty(&config(inbound_addr.port(), dns_addr, upstream_addr)).unwrap(),
    )
    .unwrap();

    let check = command_for_core(&core)
        .arg("--disable-color")
        .arg("check")
        .arg("-c")
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "sing-box check 失败: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    let formatted = command_for_core(&core)
        .arg("format")
        .arg("-c")
        .arg(&config_path)
        .output()
        .unwrap();
    assert!(formatted.status.success());
    let formatted: Value = serde_json::from_slice(&formatted.stdout).unwrap();
    assert_eq!(formatted["route"]["rules"][0]["action"], "resolve");
    assert_eq!(formatted["route"]["rules"][0]["server"], "dns-remote");
    assert_eq!(formatted["route"]["rules"][1]["action"], "reject");
    assert_eq!(formatted["route"]["rules"][1]["no_drop"], true);
    assert_eq!(formatted["route"]["rules"][2]["outbound"], "fixture-exit");

    let core_stderr_path = temp.path().join("core.stderr");
    let core_stderr = std::fs::File::create(&core_stderr_path).unwrap();
    let _running = Running {
        child: with_run(command_for_core(&core))
            .arg("-c")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::from(core_stderr))
            .spawn()
            .unwrap(),
    };
    wait_listening(inbound_addr);

    for host in ["public.test", "fakeip.test"] {
        if let Err(stage) = socks_connect_detail(inbound_addr, host) {
            let upstream = observed.recv_timeout(Duration::from_millis(250));
            panic!(
                "{host} 应允许抵达既有出口；SOCKS 阶段={stage}；上游观察={upstream:?}；core stderr 尾部={}",
                core_error_tail(&core_stderr_path)
            );
        }
        let target = observed
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_else(|_| panic!("{host} 未抵达上游 SOCKS"));
        assert!(
            target == "203.0.113.10" || target == "198.18.1.2",
            "resolve action 必须把域名钉成已审查 IP，实得 {target}"
        );
    }

    for host in [
        "loopback.test",
        "private.test",
        "cgnat.test",
        "linklocal.test",
        "ula.test",
        "mapped.test",
        "mixed.test",
    ] {
        assert!(!socks_connect(inbound_addr, host), "{host} 必须被 reject");
        assert!(
            observed.recv_timeout(Duration::from_millis(150)).is_err(),
            "{host} 的危险目标抵达了上游出口"
        );
    }

    assert!(socks_connect(inbound_addr, "rebind.test"));
    assert_eq!(
        observed.recv_timeout(Duration::from_secs(2)).unwrap(),
        "203.0.113.30",
        "首次受控解析的公网 IP 必须被 pin 给出口，不得转交域名后二次解析"
    );
    assert_eq!(
        rebind_queries.load(Ordering::SeqCst),
        1,
        "同一连接不得二次 DNS 查询落到 rebind 私网回答"
    );
}
