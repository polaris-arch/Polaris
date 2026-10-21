//! Explicit GitHub-only, source-admitted noTUN consumer tests. There is no RPC mock.
//! The no-Naive profile proves this graph only, never paired/release acceptance.
use super::*;
use polaris_singbox_grpc::{daemon, ClientError, Endpoint, RuntimeConfigError, SingBoxApiClient};
use serde_json::json;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const DOMAIN: &str = "runtime-loopback.test";
const IO_BOUND: Duration = Duration::from_secs(3);
const METHODS: [&str; 6] = [
    "GetRuntimeCapabilities",
    "PlanRuntimeConfig",
    "ApplyRuntimeConfig",
    "GetRuntimeConfigStatus",
    "RetryRuntimePersistence",
    "AcknowledgeRuntimePersistence",
];

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn regular(path: &Path, limit: u64) -> Vec<u8> {
    assert!(path.is_absolute(), "absolute admitted path required");
    let metadata = std::fs::symlink_metadata(path).expect("admitted file metadata");
    assert!(metadata.file_type().is_file(), "regular file required");
    assert!(metadata.len() > 0 && metadata.len() <= limit);
    assert_eq!(path.canonicalize().unwrap(), path, "canonical path required");
    let bytes = std::fs::read(path).expect("read admitted file");
    assert_eq!(bytes.len() as u64, metadata.len());
    bytes
}

struct Admission {
    receipt: Value,
    receipt_path: PathBuf,
    binary: PathBuf,
}

impl Admission {
    fn read() -> Self {
        for (key, value) in [
            ("GITHUB_ACTIONS", "true"),
            ("GITHUB_REPOSITORY", "polaris-arch/Polaris"),
            ("GITHUB_REPOSITORY_ID", "1347028220"),
            ("GITHUB_EVENT_NAME", "workflow_dispatch"),
            ("GITHUB_JOB", "runtime_config_loopback"),
            ("GITHUB_REF", "refs/heads/hot/app-runtime-consumer-20261020"),
            ("GITHUB_WORKFLOW_REF", "polaris-arch/Polaris/.github/workflows/ci.yml@refs/heads/hot/app-runtime-consumer-20261020"),
            ("RUNNER_OS", "Linux"),
            ("RUNNER_ARCH", "X64"),
        ] {
            assert_eq!(std::env::var(key).as_deref(), Ok(value), "CI context: {key}");
        }
        assert_eq!(std::env::consts::OS, "linux");
        assert_eq!(std::env::consts::ARCH, "x86_64");
        assert_ne!(std::fs::metadata("/proc/self").unwrap().uid(), 0, "unprivileged runner required");
        let process_status = std::fs::read_to_string("/proc/self/status").unwrap();
        let capabilities = process_status.lines().find_map(|line| line.strip_prefix("CapEff:")).unwrap().trim();
        assert_eq!(u64::from_str_radix(capabilities, 16).unwrap(), 0, "no effective Linux capabilities");
        let receipt_path = PathBuf::from(
            std::env::var_os("POLARIS_RUNTIME_LOOPBACK_RECEIPT")
                .expect("exact runner receipt required; no local path fallback"),
        );
        let receipt: Value = serde_json::from_slice(&regular(&receipt_path, 1024 * 1024))
            .expect("runner receipt JSON");
        assert_eq!(receipt["schema"], "polaris-runtime-loopback-candidate-v1");
        assert_eq!(receipt["profile"], "linux-amd64-purego-no-naive");
        assert_eq!(receipt["sourceVerified"], true);
        assert_eq!(receipt["naiveIncluded"], false);
        assert_eq!(receipt["publicationEligible"], false);
        for field in ["appCommit", "kernelCommit", "kernelTree"] {
            assert!(hex(receipt[field].as_str().unwrap(), 40), "exact {field}");
        }
        let app = receipt["appCommit"].as_str().unwrap();
        for key in ["GITHUB_SHA", "GITHUB_WORKFLOW_SHA"] {
            assert_eq!(std::env::var(key).as_deref(), Ok(app), "exact {key}");
        }
        assert_eq!(
            receipt["protoSha256"],
            polaris_updater::sha256_hex(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../crates/singbox-grpc/proto/runtime_config.proto"
            )))
        );
        if !receipt["implementationManifestDigest"].is_null() {
            assert!(hex(receipt["implementationManifestDigest"].as_str().unwrap(), 64));
        }
        let binary = PathBuf::from(receipt["binary"]["path"].as_str().unwrap());
        assert_eq!(
            std::env::var_os("POLARIS_SINGBOX_PATH").map(PathBuf::from),
            Some(binary.clone()),
            "POLARIS_SINGBOX_PATH only attests this receipt; it is not a production override"
        );
        let result = Self { receipt, receipt_path, binary };
        result.verify_binary();
        result
    }

    fn verify_binary(&self) {
        let bytes = regular(&self.binary, 256 * 1024 * 1024);
        assert_eq!(self.receipt["binary"]["bytes"].as_u64(), Some(bytes.len() as u64));
        assert_eq!(self.receipt["binary"]["sha256"], polaris_updater::sha256_hex(&bytes));
    }

    fn capabilities(&self, value: &daemon::RuntimeCapabilities) {
        super::super::runtime_config::observe_runtime_pairing(value)
            .expect("real protocol/instance pairing");
        assert!(value.connected_hot_enabled, "this exact generated graph must actually be supported");
        let protocol = value.fields.iter().find(|f| f.effective_path == "runtime.protocol").unwrap();
        assert_eq!(protocol.conditions, METHODS.map(str::to_owned));
        if let Some(expected) = self.receipt["implementationManifestDigest"].as_str() {
            assert_eq!(value.implementation_manifest_digest, expected);
        }
    }
}

struct Evidence {
    path: PathBuf,
    value: Value,
}

impl Evidence {
    fn new(admission: &Admission, name: &str) -> Self {
        let path = admission.receipt_path.parent().unwrap().join(format!("{name}.json"));
        assert!(!path.exists(), "refuse stale per-test evidence");
        let value = json!({
            "schema":"polaris-runtime-loopback-test-v1", "test":name,
            "passed":false, "profile":"linux-amd64-purego-no-naive",
            "publicationEligible":false, "appCommit":admission.receipt["appCommit"],
            "kernelCommit":admission.receipt["kernelCommit"],
            "kernelTree":admission.receipt["kernelTree"],
            "coreSha256":admission.receipt["binary"]["sha256"], "events":[]
        });
        let result = Self { path, value };
        result.flush();
        result
    }

    fn record(&mut self, event: Value) {
        self.value["events"].as_array_mut().unwrap().push(event);
        self.flush();
    }

    fn flush(&self) {
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.value).unwrap())
            .expect("write per-test actual evidence");
    }

    fn passed(&mut self) {
        self.value["passed"] = true.into();
        self.flush();
    }
}

fn identity(value: Option<&daemon::RuntimeIdentity>) -> Value {
    value.map_or(Value::Null, |i| json!({
        "incarnation":i.incarnation, "revision":i.applied_revision,
        "effectiveDigest":i.effective_digest
    }))
}

fn capabilities(value: &daemon::RuntimeCapabilities) -> Value {
    json!({
        "contractVersion":value.contract_version, "enabled":value.connected_hot_enabled,
        "implementationManifestDigest":value.implementation_manifest_digest,
        "active":identity(value.active.as_ref()), "controlRevision":value.control_revision,
        "fullEffectiveSchemaSupported":value.full_effective_schema_supported,
        "receiptRetentionSeconds":value.receipt_retention_seconds,"maxReceipts":value.max_receipts,
        "fields":value.fields.iter().map(|f| json!({
            "path":f.effective_path,"variant":f.variant,"platform":f.platform_build,
            "class":f.update_class,"adapter":f.adapter_version,"conditions":f.conditions
        })).collect::<Vec<_>>()
    })
}

fn operation(value: &daemon::RuntimeConfigOperation) -> Value {
    json!({
        "transaction":value.transaction_id,"state":value.state,"durable":value.durable,
        "before":identity(value.before.as_ref()),"active":identity(value.active.as_ref()),
        "candidateDigest":value.candidate_digest,"impactDigest":value.impact_digest,
        "controlRevision":value.control_revision,"persistenceMode":value.persistence_mode,
        "ownerReloadAcknowledged":value.owner_reload_acknowledged,"retirementState":value.retirement_state,
        "desiredStateDigest":value.desired_state_digest,"desiredStateRevision":value.desired_state_revision,
        "errors":value.errors.iter().map(|e| json!({"code":e.code,"path":e.effective_path})).collect::<Vec<_>>()
    })
}

// Owned threads only. Reads time out, and Drop joins all accepted sockets before
// releasing listeners. No backend resolves or dials the destination requested.
struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !std::thread::panicking() {
                assert!(result.is_ok(), "owned fixture thread failed");
            }
        }
    }
}

#[derive(Clone)]
struct Target {
    connection: usize,
    host: String,
    port: u16,
}

struct Backend {
    address: SocketAddr,
    targets: Arc<Mutex<Vec<Target>>>,
    _worker: Worker,
}

fn socks_target(stream: &mut TcpStream) -> std::io::Result<(String, u16)> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(IO_BOUND))?;
    stream.set_write_timeout(Some(IO_BOUND))?;
    let mut greeting = [0; 2];
    stream.read_exact(&mut greeting)?;
    if greeting[0] != 5 || greeting[1] == 0 {
        return Err(std::io::Error::other("invalid SOCKS greeting"));
    }
    let mut methods = vec![0; usize::from(greeting[1])];
    stream.read_exact(&mut methods)?;
    if !methods.contains(&0) {
        return Err(std::io::Error::other("fixture only accepts no-auth SOCKS"));
    }
    stream.write_all(&[5, 0])?;
    let mut request = [0; 4];
    stream.read_exact(&mut request)?;
    if request[..3] != [5, 1, 0] {
        return Err(std::io::Error::other("fixture only accepts CONNECT"));
    }
    let host = match request[3] {
        1 => {
            let mut ip = [0; 4];
            stream.read_exact(&mut ip)?;
            Ipv4Addr::from(ip).to_string()
        }
        3 => {
            let mut length = [0];
            stream.read_exact(&mut length)?;
            let mut name = vec![0; usize::from(length[0])];
            stream.read_exact(&mut name)?;
            String::from_utf8(name).map_err(std::io::Error::other)?
        }
        4 => {
            let mut ip = [0; 16];
            stream.read_exact(&mut ip)?;
            std::net::Ipv6Addr::from(ip).to_string()
        }
        _ => return Err(std::io::Error::other("invalid SOCKS address")),
    };
    let mut port = [0; 2];
    stream.read_exact(&mut port)?;
    Ok((host, u16::from_be_bytes(port)))
}

impl Backend {
    fn new(label: u8) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let targets = Arc::new(Mutex::new(Vec::new()));
        let observations = Arc::clone(&targets);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut sockets = Vec::new();
            while !stopping.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, peer)) => {
                        assert!(peer.ip().is_loopback());
                        assert!(sockets.len() < 16, "bounded owned connection count");
                        let stop = Arc::clone(&stopping);
                        let targets = Arc::clone(&observations);
                        sockets.push(std::thread::spawn(move || {
                            let (host, port) = socks_target(&mut stream).expect("owned SOCKS handshake");
                            {
                                let mut targets = targets.lock().unwrap();
                                let connection = targets.len() + 1;
                                targets.push(Target { connection, host, port });
                            }
                            stream.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0]).unwrap();
                            stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
                            let mut input = [0; 256];
                            while !stop.load(Ordering::Acquire) {
                                match stream.read(&mut input) {
                                    Ok(0) => break,
                                    Ok(count) => {
                                        let reply: Vec<_> = input[..count].iter().flat_map(|b| [label, *b]).collect();
                                        if stream.write_all(&reply).is_err() { break; }
                                    }
                                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                                    Err(_) => break,
                                }
                            }
                        }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(10)),
                    Err(e) => panic!("owned listener: {e}"),
                }
            }
            for socket in sockets { socket.join().unwrap(); }
        });
        Self { address, targets, _worker: Worker { stop, thread: Some(thread) } }
    }

    fn observations(&self) -> Value {
        json!(self.targets.lock().unwrap().iter().map(|t| json!({
            "connection":t.connection,"host":t.host,"port":t.port
        })).collect::<Vec<_>>())
    }
}

struct Resolver {
    address: SocketAddr,
    queries: Arc<Mutex<Vec<(String, u16)>>>,
    _worker: Worker,
}

fn question(packet: &[u8]) -> Option<(String, u16, usize)> {
    if packet.len() < 12 || packet[4..6] != [0, 1] { return None; }
    let mut offset = 12;
    let mut labels = Vec::new();
    loop {
        let length = usize::from(*packet.get(offset)?);
        offset += 1;
        if length == 0 { break; }
        if length > 63 { return None; }
        labels.push(std::str::from_utf8(packet.get(offset..offset + length)?).ok()?);
        offset += length;
    }
    let tail = packet.get(offset..offset + 4)?;
    if tail[2..] != [0, 1] { return None; }
    Some((labels.join("."), u16::from_be_bytes([tail[0], tail[1]]), offset + 4))
}

impl Resolver {
    fn new(answer: Ipv4Addr) -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        let address = socket.local_addr().unwrap();
        let queries = Arc::new(Mutex::new(Vec::new()));
        let observations = Arc::clone(&queries);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut packet = [0; 2048];
            while !stopping.load(Ordering::Acquire) {
                let (length, peer) = match socket.recv_from(&mut packet) {
                    Ok(value) => value,
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                    Err(e) => panic!("owned DNS fixture: {e}"),
                };
                assert!(peer.ip().is_loopback());
                let Some((name, kind, end)) = question(&packet[..length]) else { continue; };
                observations.lock().unwrap().push((name.clone(), kind));
                let answers = u16::from(name == DOMAIN && kind == 1);
                let mut response = Vec::new();
                response.extend_from_slice(&packet[..2]);
                response.extend_from_slice(&0x8180u16.to_be_bytes());
                response.extend_from_slice(&1u16.to_be_bytes());
                response.extend_from_slice(&answers.to_be_bytes());
                response.extend_from_slice(&[0; 4]);
                response.extend_from_slice(&packet[12..end]);
                if answers == 1 {
                    response.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1]);
                    response.extend_from_slice(&0u32.to_be_bytes()); // TTL 0: no cache evidence.
                    response.extend_from_slice(&4u16.to_be_bytes());
                    response.extend_from_slice(&answer.octets());
                }
                socket.send_to(&response, peer).unwrap();
            }
        });
        Self { address, queries, _worker: Worker { stop, thread: Some(thread) } }
    }

    fn count(&self) -> usize {
        self.queries.lock().unwrap().iter().filter(|(name, kind)| name == DOMAIN && *kind == 1).count()
    }
}

struct Network {
    a: Backend,
    b: Backend,
    dns_a: Resolver,
    dns_b: Resolver,
}

impl Network {
    fn new() -> Self {
        Self {
            a: Backend::new(b'A'), b: Backend::new(b'B'),
            dns_a: Resolver::new(Ipv4Addr::new(127, 0, 0, 2)),
            dns_b: Resolver::new(Ipv4Addr::new(127, 0, 0, 3)),
        }
    }

    fn assert_only_fixture_queries(&self) {
        for resolver in [&self.dns_a, &self.dns_b] {
            assert!(resolver.queries.lock().unwrap().iter().all(|(name, kind)|
                name == DOMAIN && matches!(*kind, 1 | 28)), "unexpected owned-resolver query");
        }
    }

    fn config(&self) -> Value {
        let mut config = polaris_store::default_config();
        config["proxyModeType"] = "manual".into();
        config["proxyMode"] = "smart".into();
        config["mixedPort"] = free_port().into();
        config["controlPort"] = free_port().into();
        config["clashApiSecret"] = FIXTURE_MANAGEMENT_SECRET.into();
        config["selectedServerId"] = "node-a".into();
        for key in ["singboxDashboard", "allowLan", "bypassLAN", "appRoutingEnabled",
            "autoSelectEnabled", "autoSwitchNode", "autoCheckUpdate", "autoUpdateSubscriptionOnStart",
            "ruleResourceAutoUpdate", "interruptConnectionsOnSwitch", "enableIPv6", "blockQuic"] {
            config[key] = false.into();
        }
        for key in ["customRules", "policyRules", "ruleResources", "customRuleSets", "appRules", "networkProfiles", "subscriptions", "autoSelectPins", "dnsServerGroups"] {
            config[key] = json!([]);
        }
        config["servers"] = json!([
            {"id":"node-a","name":"Loop A","protocol":"socks","address":"127.0.0.1","port":self.a.address.port()},
            {"id":"node-b","name":"Loop B","protocol":"socks","address":"127.0.0.1","port":self.b.address.port()}
        ]);
        config["trafficRules"] = json!([{
            "id":"loop-route","type":"domain","values":[DOMAIN],"action":"proxy",
            "enabled":true,"targetServerId":"node-a",
            "effects":{"route":{"enabled":true,"action":"proxy","targetServerId":"node-a"}}
        }]);
        config["dnsRules"] = json!([{
            "id":"loop-dns","type":"domain","values":[DOMAIN],"action":"proxy","enabled":true,
            "effects":{"dns":{"enabled":true,"action":{"type":"server","serverId":"loop-a"}}}
        }]);
        config["routeRuleOrder"] = json!(["loop-route"]);
        config["dnsRuleOrder"] = json!(["loop-dns"]);
        config["dnsServers"] = json!([
            dns_resource("builtin-domestic", self.dns_a.address.port()),
            dns_resource("builtin-remote", self.dns_a.address.port()),
            dns_resource("builtin-bootstrap", self.dns_a.address.port()),
            dns_resource("loop-a", self.dns_a.address.port()),
            dns_resource("loop-b", self.dns_b.address.port())
        ]);
        config["dnsDefaults"] = json!({
            "directServerId":"builtin-domestic", "proxyServerId":"builtin-remote",
            "unmatchedAction":{"type":"reject","method":"drop"},
            "realQueryFallbackAction":{"type":"reject","method":"drop"},
            "connectionResolution":"dnsRules"
        });
        config["dnsConfig"] = json!({
            "domesticDns":format!("udp://{}",self.dns_a.address),
            "foreignDns":format!("udp://{}",self.dns_a.address),
            "enableFakeIp":false,"optimisticCache":false,"takeoverSystemDns":false,
            "resolveNodeDomainsAhead":false,"nodeResolverSingle":"loop-node",
            "nodeResolverPool":["loop-node"],
            "nodeResolverCustom":[{"id":"loop-node","spec":format!("udp://{}",self.dns_a.address)}]
        });
        config
    }
}

fn dns_resource(id: &str, port: u16) -> Value {
    json!({"id":id,"name":id,"enabled":true,"type":"udp",
        "endpoint":{"host":"127.0.0.1","port":port},"outbound":{"type":"direct"}})
}

fn changed_graph(mut config: Value) -> Value {
    config["trafficRules"][0]["targetServerId"] = "node-b".into();
    config["trafficRules"][0]["effects"]["route"]["targetServerId"] = "node-b".into();
    config["dnsRules"][0]["effects"]["dns"]["action"]["serverId"] = "loop-b".into();
    config
}

// This admission is deliberately specific to this two-node fixture. It is not a
// general network sandbox: unused production DNS definitions remain unchanged.
fn canonical_fixture(config: &Value, requested: &Value) {
    for key in ["proxyModeType", "proxyMode", "servers", "selectedServerId", "dnsServers",
        "dnsConfig", "dnsDefaults", "trafficRules", "dnsRules", "routeRuleOrder", "dnsRuleOrder"] {
        assert_eq!(config[key], requested[key], "canonical fixture changed {key}");
    }
    assert_eq!(config["proxyModeType"], "manual");
    assert_eq!(config["proxyMode"], "smart");
    for key in ["singboxDashboard", "allowLan", "bypassLAN", "appRoutingEnabled",
        "autoSelectEnabled", "autoSwitchNode", "autoCheckUpdate", "autoUpdateSubscriptionOnStart",
        "ruleResourceAutoUpdate", "interruptConnectionsOnSwitch", "enableIPv6", "blockQuic"] {
        assert_eq!(config[key], false, "unsafe fixture switch {key}");
    }
    for key in ["subscriptions", "autoSelectPins", "customRules", "policyRules", "ruleResources",
        "customRuleSets", "appRules", "networkProfiles", "dnsServerGroups"] {
        assert_eq!(config[key], json!([]), "unexpected fixture dependency {key}");
    }
    let servers = config["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    for node in servers {
        assert_eq!(node.as_object().unwrap().len(), 5, "no hidden node dial/TLS/mesh options");
        assert_eq!(node["protocol"], "socks");
        assert_eq!(node["address"], "127.0.0.1", "node endpoints need no DNS");
        assert!(node["port"].as_u64().is_some_and(|p| p > 1024 && p <= 65535));
    }
    assert_eq!(config["dnsConfig"]["enableFakeIp"], false);
    assert_eq!(config["dnsConfig"]["optimisticCache"], false);
    assert_eq!(config["dnsConfig"]["takeoverSystemDns"], false);
    assert_eq!(config["dnsConfig"]["resolveNodeDomainsAhead"], false);
    assert_eq!(config["dnsDefaults"]["connectionResolution"], "dnsRules");
    for key in ["trafficRules", "dnsRules"] {
        let rules = config[key].as_array().unwrap();
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["type"], "domain");
        assert_eq!(rules[0]["values"], json!([DOMAIN]));
    }
}

fn strings(value: &Value) -> Vec<&str> {
    if value.is_null() { return Vec::new(); }
    if let Some(value) = value.as_str() { return vec![value]; }
    value.as_array().unwrap().iter().map(|value| value.as_str().unwrap()).collect()
}

// A conservative exclusion proof for this one DOMAIN from mixed-in. An unknown
// matcher never proves exclusion. In particular an unknown rule_set/logical
// branch cannot hide an external DNS fallback behind a nonmatching domain list.
fn excludes_fixture(rule: &Value) -> bool {
    if rule["invert"] == true { return false; }
    if !rule["inbound"].is_null() && !strings(&rule["inbound"]).contains(&"mixed-in") {
        return true;
    }
    if rule["resolver_policy"] == "node-bootstrap" { return true; }
    if !rule["rule_set"].is_null() || !rule["rules"].is_null() { return false; }
    let mut has_domain = false;
    let mut matches = false;
    for key in ["domain", "domain_suffix", "domain_keyword", "domain_regex"] {
        for value in strings(&rule[key]) {
            has_domain = true;
            matches |= match key {
                "domain" => DOMAIN == value,
                "domain_suffix" => {
                    let suffix = value.trim_start_matches('.');
                    DOMAIN == suffix || DOMAIN.ends_with(&format!(".{suffix}"))
                }
                "domain_keyword" => DOMAIN.contains(value),
                "domain_regex" => regex::Regex::new(value).unwrap().is_match(DOMAIN),
                _ => unreachable!(),
            };
        }
    }
    has_domain && !matches
}

fn inspect_generated(rt: &ProxyRuntime, config: &Value, generated: &Value) -> Value {
    let mut listeners = Vec::new();
    for inbound in generated["inbounds"].as_array().unwrap() {
        assert!(matches!(inbound["type"].as_str(), Some("mixed" | "http" | "socks")));
        let tag = inbound["tag"].as_str().unwrap();
        assert!(matches!(tag, "mixed-in" | "update-in" | "subscription-update-in" | "probe-proxy-in")
            || tag.strip_prefix("probe-in-").is_some_and(|n| n.parse::<usize>().is_ok()));
        assert_eq!(inbound["listen"], "127.0.0.1", "no TUN or public listener");
        listeners.push(json!({"type":inbound["type"],"tag":inbound["tag"],"listen":inbound["listen"]}));
    }
    assert!(listeners.iter().any(|i| i["tag"] == "mixed-in"));
    for service in generated["services"].as_array().into_iter().flatten() {
        assert_eq!(service["type"], "api");
        assert_eq!(service["listen"], "127.0.0.1");
        assert!(service["dashboard"].is_null(), "no dashboard download");
    }
    assert!(generated["endpoints"].as_array().is_none_or(Vec::is_empty), "no mesh endpoints");
    let outbounds = generated["outbounds"].as_array().unwrap();
    let tags: std::collections::BTreeSet<_> = outbounds.iter().map(|o| o["tag"].as_str().unwrap()).collect();
    let owned_ports: Vec<_> = config["servers"].as_array().unwrap().iter().map(|s| s["port"].clone()).collect();
    let mut physical = Vec::new();
    for outbound in outbounds {
        match outbound["type"].as_str().unwrap() {
            "socks" => {
                assert_eq!(outbound["server"], "127.0.0.1");
                assert!(owned_ports.contains(&outbound["server_port"]));
                assert!(outbound["detour"].is_null());
                physical.push(json!({"tag":outbound["tag"],"server":outbound["server"],"port":outbound["server_port"]}));
            }
            "selector" => {
                for tag in strings(&outbound["outbounds"]) { assert!(tags.contains(tag)); }
                assert_ne!(outbound["interrupt_exist_connections"], true);
            }
            "direct" | "block" => {}
            other => panic!("unexpected outbound or autonomous probe: {other}"),
        }
    }
    assert_eq!(physical.len(), 2);
    let mut route = generated["route"].clone();
    for set in route["rule_set"].as_array_mut().into_iter().flatten() {
        let rules = match set["type"].as_str().unwrap() {
            "local" => {
                assert_eq!(set["format"], "source");
                let path = PathBuf::from(set["path"].as_str().unwrap());
                assert_eq!(path.parent(), Some(rt.custom_rules_dir().as_path()));
                let content: Value = serde_json::from_slice(&regular(&path, 65536)).unwrap();
                content["rules"].clone()
            }
            "inline" => set["rules"].clone(),
            _ => panic!("no remote or unreviewed rule source"),
        };
        assert_eq!(rules, json!([{"domain":[DOMAIN]}]), "only owned domain rule files");
        // HOT's production snapshot embeds these exact owned source rules.
        *set = json!({"tag":set["tag"],"type":"inline","rules":rules});
    }
    let dns = &generated["dns"];
    let owned_dns_ports: Vec<_> = config["dnsServers"].as_array().unwrap().iter().map(|s| s["endpoint"]["port"].clone()).collect();
    let mut owned_dns = std::collections::BTreeSet::new();
    let mut unused_dns = Vec::new();
    for server in dns["servers"].as_array().unwrap() {
        let tag = server["tag"].as_str().unwrap();
        if server["type"] == "udp" && server["server"] == "127.0.0.1" {
            assert!(owned_dns_ports.contains(&server["server_port"]));
            assert!(server["domain_resolver"].is_null());
            assert!(server["detour"].is_null() || server["detour"] == "direct");
            owned_dns.insert(tag);
        } else {
            assert!(matches!(tag, "dns-node" | "dns-local" | "dns-mdns" | "dns-lan") || tag.starts_with("dns-probe-exit-"), "unexpected non-owned DNS definition: {tag}");
            unused_dns.push(tag);
        }
    }
    assert!(owned_dns.contains(dns["final"].as_str().unwrap()), "no external DNS final");
    assert!(owned_dns.contains(generated["route"]["default_domain_resolver"].as_str().unwrap()));
    let expected_dns = polaris_config_engine::user_config::dns_policy::dns_server_tag(
        config["dnsRules"][0]["effects"]["dns"]["action"]["serverId"].as_str().unwrap(),
    );
    let mut explicit_domain_routes = Vec::new();
    for rule in dns["rules"].as_array().unwrap() {
        assert!(rule["rules"].is_null(), "fixture DNS must not grow unreviewed logical branches");
        if let Some(server) = rule["server"].as_str() {
            if !owned_dns.contains(server) {
                assert!(excludes_fixture(rule), "reachable external DNS rule: {server}");
            }
            if rule["domain"] == json!([DOMAIN]) && !excludes_fixture(rule) {
                explicit_domain_routes.push(server);
            }
        }
    }
    assert_eq!(explicit_domain_routes, vec![expected_dns.as_str()], "unique owned fixture DNS rule");
    assert_ne!(dns["optimistic"], true);
    assert!(dns["fakeip"].is_null());
    json!({"listeners":listeners,"physicalOutbounds":physical,"dns":dns,"route":route,
        "unusedDnsDefinitions":unused_dns,"domain":DOMAIN,"onlyTrafficInbound":"mixed-in"})
}

fn preflight(rt: &ProxyRuntime, config: &Value) -> Value {
    let typed: UserConfig = serde_json::from_value(config.clone()).unwrap();
    // No generated JSON is rewritten and no metadata/kernel check is bypassed.
    let deps = rt.generate_candidate_deps(&typed, config).unwrap();
    let outcome = polaris_config_engine::builder::generate_sing_box_config_with_report(
        &typed, &std::collections::BTreeMap::new(), &deps,
    ).expect("production builder safety preflight before core execution");
    assert!(outcome.invalid_nodes.is_empty());
    assert!(outcome.mesh_route_candidates.is_empty());
    assert!(outcome.network_canary.is_none());
    inspect_generated(rt, config, &serde_json::to_value(outcome.config).unwrap())
}

struct Running {
    // First field cleans the owned Child before its directory is dropped on unwind.
    _cleanup: RealCoreCleanupGuard,
    _directory: TestDir,
    rt: Arc<ProxyRuntime>,
    client: SingBoxApiClient,
    initial: daemon::RuntimeCapabilities,
    pid: u32,
    mixed: u16,
}

impl Running {
    async fn start(admission: &Admission, config: Value, evidence: &mut Evidence) -> Self {
        // Avoid the legacy test factory's age-based sweep of other tests' dirs.
        let directory = TestDir::new("polaris-runtime-loopback-");
        let rt = test_runtime_in(directory.clone());
        let cleanup = RealCoreCleanupGuard::new(&rt);
        rt.inject_real_core_for_test(admission.binary.clone());
        rt.config.save_full(&config).expect("save real canonical App configuration");
        let requested = config;
        let config = rt.config.current().unwrap();
        canonical_fixture(&config, &requested);
        assert!(rt.management_api_stub.lock().unwrap().is_none());
        assert!(rt.error_emitter.get().is_none(), "no attached automatic probe emitter");
        // Ordinary externalization only, inside the owned temporary directory.
        rt.write_custom_rule_files(&serde_json::from_value(config.clone()).unwrap()).await;
        let checked = preflight(&rt, &config);
        evidence.record(json!({"phase":"prestart-safety","projection":checked,
            "claim":"fixture-reachable closure only; unused production DNS definitions are statically excluded",
            "autoProbeEmitterAttached":false,"canonicalNoTun":true}));
        let status = tokio::time::timeout(Duration::from_secs(45), rt.start(config.clone()))
            .await.expect("bounded real start").expect("real App start");
        assert!(status.running && status.pid != 0 && ps_alive(status.pid));
        let executable = std::fs::read_link(format!("/proc/{}/exe", status.pid)).unwrap();
        assert_eq!(executable, admission.binary, "actual child executable identity");
        admission.verify_binary();
        let client = SingBoxApiClient::connect(
            Endpoint::new("127.0.0.1", status.clash_api_port), FIXTURE_MANAGEMENT_SECRET,
        ).await.expect("real authenticated management client");
        let initial = client.get_runtime_capabilities().await.unwrap();
        evidence.record(json!({"phase":"started","pid":status.pid,"capabilities":capabilities(&initial)}));
        admission.capabilities(&initial);
        let path = rt.runtime_config_path();
        let bytes = regular(&path, 4 * 1024 * 1024);
        let generated: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(inspect_generated(&rt, &config, &generated), checked,
            "actual generated safety projection must match production preflight");
        let argv = std::fs::read(format!("/proc/{}/cmdline", status.pid)).unwrap();
        let args: Vec<_> = argv.split(|b| *b == 0).filter(|b| !b.is_empty())
            .map(|b| String::from_utf8(b.to_vec()).unwrap()).collect();
        let config_paths: Vec<_> = args.windows(2).filter(|a| a[0] == "-c").map(|a| a[1].as_str()).collect();
        assert_eq!(config_paths, vec![path.to_str().unwrap()], "the persisted file must be actual CLI restart source");
        assert!(args.iter().any(|a| a == "run"));
        evidence.record(json!({"phase":"source","cliConfigPath":path,
            "initialBytesSha256":polaris_updater::sha256_hex(&bytes),"argv":args,
            "actualExecutable":executable,"actualExecutableSha256":admission.receipt["binary"]["sha256"]}));
        Self { _cleanup: cleanup, _directory: directory, rt, client, initial,
            pid: status.pid, mixed: status.mixed_port }
    }

    async fn status(&self, transaction: &str) -> daemon::RuntimeConfigStatus {
        self.client.get_runtime_config_status(daemon::RuntimeConfigStatusRequest {
            contract_version:1, transaction_id:transaction.into(),
            runtime_incarnation:self.initial.active.as_ref().unwrap().incarnation.clone(),
        }).await.unwrap()
    }

    async fn unchanged_process(&self) -> daemon::RuntimeCapabilities {
        assert_eq!(self.rt.status().pid, self.pid);
        assert!(ps_alive(self.pid));
        let cap = self.client.get_runtime_capabilities().await.unwrap();
        assert_eq!(cap.active.as_ref().unwrap().incarnation, self.initial.active.as_ref().unwrap().incarnation);
        assert_eq!(cap.implementation_manifest_digest, self.initial.implementation_manifest_digest);
        cap
    }

    async fn finish(&self, admission: &Admission, evidence: &mut Evidence) {
        admission.verify_binary();
        tokio::time::timeout(Duration::from_secs(15), self.rt.stop()).await.unwrap().unwrap();
        assert!(!self.rt.status().running);
        assert!(!ps_alive(self.pid), "owned core reaped");
        evidence.record(json!({"phase":"stopped","pid":self.pid,"ownedChildReaped":true}));
    }
}

async fn connect(mixed: u16) -> tokio::net::TcpStream {
    tokio::time::timeout(IO_BOUND, async {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", mixed)).await.unwrap();
        socket.write_all(&[5, 1, 0]).await.unwrap();
        let mut greeting = [0; 2];
        socket.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [5, 0]);
        let mut request = vec![5, 1, 0, 3, u8::try_from(DOMAIN.len()).unwrap()];
        request.extend_from_slice(DOMAIN.as_bytes());
        request.extend_from_slice(&80u16.to_be_bytes());
        socket.write_all(&request).await.unwrap();
        let mut reply = [0; 4];
        socket.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply[..3], [5, 0, 0]);
        let len = match reply[3] { 1 => 4, 4 => 16, 3 => usize::from(socket.read_u8().await.unwrap()), _ => panic!("SOCKS reply address") };
        let mut bound = vec![0; len + 2];
        socket.read_exact(&mut bound).await.unwrap();
        socket
    }).await.expect("bounded real SOCKS connect")
}

async fn exchange(socket: &mut tokio::net::TcpStream, label: u8, nonce: &[u8]) {
    tokio::time::timeout(IO_BOUND, async {
        socket.write_all(nonce).await.unwrap();
        let mut actual = vec![0; nonce.len() * 2];
        socket.read_exact(&mut actual).await.unwrap();
        let expected: Vec<_> = nonce.iter().flat_map(|b| [label, *b]).collect();
        assert_eq!(actual, expected, "actual retained TCP byte stream");
    }).await.expect("bounded retained TCP exchange");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_dispatcher_apply_and_status() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(&admission, "real_hot_dispatcher_apply_and_status");
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    let mut old = connect(running.mixed).await;
    let nonce = format!("{}-old-before", running.initial.active.as_ref().unwrap().incarnation);
    exchange(&mut old, b'A', nonce.as_bytes()).await;
    assert_eq!(network.a.targets.lock().unwrap()[0].host, "127.0.0.2");
    assert_eq!(network.a.targets.lock().unwrap()[0].port, 80);
    assert_eq!(network.a.targets.lock().unwrap().len(), 1);
    assert!(network.b.targets.lock().unwrap().is_empty());
    assert!(network.dns_a.count() > 0);
    assert_eq!(network.dns_b.count(), 0);
    let a_queries = network.dns_a.count();
    let requested = changed_graph(running.rt.config.current().unwrap());
    running.rt.config.save_full(&requested).unwrap();
    let desired = running.rt.config.current().unwrap();
    canonical_fixture(&desired, &requested);
    let desired_projection = preflight(&running.rt, &desired);
    evidence.record(json!({"phase":"preapply-safety","projection":desired_projection}));
    let result = running.rt.apply_pending_transaction(None).await;
    evidence.record(json!({"phase":"apply","appResult":result}));
    assert_eq!(result.status, "hotApplied", "real graph Apply must be durable; no restart/selector substitute");
    assert!(result.restart_confirmation.is_none() && result.apply_id.is_none());
    let transaction = result.transaction_id.as_deref().expect("real transaction");
    let status = running.status(transaction).await;
    assert_eq!(status.lookup_state, daemon::RuntimeLookupState::Found as i32);
    let op = status.operation.as_ref().unwrap();
    assert!(op.durable);
    assert_eq!(op.persistence_mode, "exact_configuration_file");
    assert_eq!(running.rt.current_config_snapshot().as_ref(), Some(&desired));
    assert_eq!(status.durable_applied_digest, op.candidate_digest);
    let accepted: Value = serde_json::from_slice(&std::fs::read(running.rt.runtime_config_path()).unwrap()).unwrap();
    assert_eq!(inspect_generated(&running.rt, &desired, &accepted), desired_projection);
    assert_eq!(op.active.as_ref().unwrap().applied_revision, running.initial.active.as_ref().unwrap().applied_revision + 1);
    let after_nonce = format!("{}-old-after", transaction);
    exchange(&mut old, b'A', after_nonce.as_bytes()).await;
    let mut fresh = connect(running.mixed).await;
    let new_nonce = format!("{}-new-after", transaction);
    exchange(&mut fresh, b'B', new_nonce.as_bytes()).await;
    assert_eq!(network.a.targets.lock().unwrap().len(), 1, "no new admission to A");
    assert_eq!(network.b.targets.lock().unwrap().len(), 1);
    assert_eq!(network.b.targets.lock().unwrap()[0].host, "127.0.0.3");
    assert_eq!(network.b.targets.lock().unwrap()[0].port, 80);
    assert_eq!(network.dns_a.count(), a_queries);
    assert!(network.dns_b.count() > 0, "fresh TTL0 upstream query, not cached evidence");
    let cap = running.unchanged_process().await;
    assert_eq!(cap.active, op.active);
    evidence.record(json!({"phase":"data-plane","operation":operation(op),"capabilities":capabilities(&cap),
        "appRuntimeSnapshotMatchesDesired":true,"acceptedRestartSourceMatchesDesiredProjection":true,
        "oldSocketRetained":true,"oldBeforeNonce":nonce,"oldAfterNonce":after_nonce,"newNonce":new_nonce,
        "aTargets":network.a.observations(),"bTargets":network.b.observations(),
        "dnsAQueries":network.dns_a.count(),"dnsBQueries":network.dns_b.count(),"answerTtl":0,
        "runtimeFileSha256":polaris_updater::sha256_hex(&std::fs::read(running.rt.runtime_config_path()).unwrap())}));
    drop(old);
    drop(fresh);
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_dispatcher_persistence_retry() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(&admission, "real_hot_dispatcher_persistence_retry");
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    let path = running.rt.runtime_config_path();
    let original = std::fs::read(&path).unwrap();
    let mut disturbed = original.clone();
    disturbed.push(b'\n'); // valid JSON, exact CLI owner bytes deliberately differ.
    std::fs::write(&path, &disturbed).unwrap();
    let mut desired = running.rt.config.current().unwrap();
    desired["logLevel"] = "warn".into();
    running.rt.config.save_full(&desired).unwrap();
    let desired = running.rt.config.current().unwrap();
    let desired_projection = preflight(&running.rt, &desired);
    let result = running.rt.apply_pending_transaction(None).await;
    evidence.record(json!({"phase":"pending","appResult":result}));
    assert_eq!(result.status, "hotDurabilityPending");
    let transaction = result.transaction_id.clone().unwrap();
    let pending = running.status(&transaction).await;
    let op = pending.operation.as_ref().unwrap();
    assert_eq!(op.state, daemon::RuntimeOperationState::CommittedDurabilityPending as i32);
    assert!(!op.durable);
    assert_eq!(op.persistence_mode, "exact_configuration_file");
    let published = op.active.clone();
    assert_eq!(published.as_ref().unwrap().applied_revision, running.initial.active.as_ref().unwrap().applied_revision + 1);
    assert_eq!(std::fs::read(&path).unwrap(), disturbed);
    // R describes the committed live graph even while CLI persistence is pending.
    assert_eq!(running.rt.current_config_snapshot().as_ref(), Some(&desired));
    for _ in 0..3 {
        let polled = running.rt.runtime_apply_status(transaction.clone()).await;
        assert_eq!(polled.status, "hotDurabilityPending");
        assert_eq!(std::fs::read(&path).unwrap(), disturbed, "status must not retry persistence");
        assert_eq!(running.status(&transaction).await.active, published);
        assert_eq!(running.rt.current_config_snapshot().as_ref(), Some(&desired));
    }
    assert!(running.rt.pending_force_restart.read().unwrap().is_none());
    evidence.record(json!({"phase":"poll-read-only","operation":operation(op),
        "originalSha256":polaris_updater::sha256_hex(&original),
        "disturbedSha256":polaris_updater::sha256_hex(&disturbed),"polls":3,
        "appRuntimeSnapshotAlreadyMatchesCommittedGraph":true,"cliSourceDurabilityPending":true}));
    std::fs::write(&path, &original).unwrap();
    let recovered = running.rt.retry_runtime_persistence(transaction.clone()).await;
    assert_eq!(recovered.status, "hotApplied");
    let terminal = running.status(&transaction).await;
    let durable = terminal.operation.as_ref().unwrap();
    assert!(durable.durable);
    assert_eq!(durable.active, published, "retry must not republish graph");
    assert_eq!(terminal.durable_applied_digest, durable.candidate_digest);
    let accepted = std::fs::read(&path).unwrap();
    assert_ne!(accepted, original);
    let content: Value = serde_json::from_slice(&accepted).unwrap();
    assert_eq!(content["log"]["level"], "warn");
    assert_eq!(inspect_generated(&running.rt, &desired, &content), desired_projection);
    assert_eq!(running.rt.current_config_snapshot().as_ref(), Some(&desired));
    let repeated = running.rt.retry_runtime_persistence(transaction.clone()).await;
    assert_eq!(repeated.status, "hotApplied");
    assert_eq!(std::fs::read(&path).unwrap(), accepted);
    assert_eq!(running.unchanged_process().await.active, published);
    evidence.record(json!({"phase":"retry-durable","operation":operation(durable),
        "appResult":recovered,"repeatedAppResult":repeated,
        "acceptedRestartSource":path,"acceptedBytesSha256":polaris_updater::sha256_hex(&accepted),
        "singleGraphPublication":true,"statusDidNotWrite":true}));
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}

fn unauthenticated<T>(result: Result<T, RuntimeConfigError>) {
    // Frozen gRPC status code 16; the App need not add a direct tonic dependency.
    assert!(matches!(result, Err(RuntimeConfigError::Call(ClientError::Status(ref status)))
        if status.code() as i32 == 16), "all six public methods authenticate before dispatch");
}

fn failed_precondition<T>(result: Result<T, RuntimeConfigError>) {
    // Transport/deadline failures cannot stand in for a kernel rejection.
    assert!(matches!(result, Err(RuntimeConfigError::Call(ClientError::Status(ref status)))
        if status.code() as i32 == 9), "expected actual FailedPrecondition RPC rejection");
}

fn apply_request(plan: &daemon::RuntimeConfigPlan) -> daemon::ApplyRuntimeConfigRequest {
    daemon::ApplyRuntimeConfigRequest {
        contract_version:1, transaction_id:plan.transaction_id.clone(), plan_id:plan.plan_id.clone(),
        expected:plan.expected.clone(), candidate_digest:plan.candidate_digest.clone(),
        accepted_impact_digest:plan.impact_digest.clone(), expected_control_revision:plan.expected_control_revision,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_public_rpc_contract() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(&admission, "real_hot_public_rpc_contract");
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    for secret in ["", "incorrect-loopback-credential"] {
        let client = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", running.rt.status().clash_api_port), secret).await.unwrap();
        unauthenticated(client.get_runtime_capabilities().await);
        unauthenticated(client.plan_runtime_config(Default::default()).await);
        unauthenticated(client.apply_runtime_config(Default::default()).await);
        unauthenticated(client.get_runtime_config_status(Default::default()).await);
        unauthenticated(client.retry_runtime_persistence(Default::default()).await);
        unauthenticated(client.acknowledge_runtime_persistence(Default::default()).await);
    }
    let original: Value = serde_json::from_slice(&std::fs::read(running.rt.runtime_config_path()).unwrap()).unwrap();
    let request = daemon::PlanRuntimeConfigRequest {
        contract_version:1, expected:running.initial.active.clone(),
        config_content:serde_json::to_string(&original).unwrap(), overrides:None,
        expected_control_revision:running.initial.control_revision, selector_intents:vec![],
    };
    let no_op = running.client.plan_runtime_config(request.clone()).await.unwrap();
    assert!(no_op.applicable && no_op.no_op && no_op.errors.is_empty());
    let no_op_result = running.client.apply_runtime_config(apply_request(&no_op)).await.unwrap();
    assert_eq!(no_op_result.active, running.initial.active);
    assert_eq!(no_op_result.before, no_op_result.active);
    assert_eq!(no_op_result.control_revision, running.initial.control_revision);
    let mut candidate = original;
    candidate["log"]["level"] = "debug".into();
    let plan = running.client.plan_runtime_config(daemon::PlanRuntimeConfigRequest {
        config_content:serde_json::to_string(&candidate).unwrap(), ..request.clone()
    }).await.unwrap();
    assert!(plan.applicable && !plan.no_op && plan.errors.is_empty());
    let apply = apply_request(&plan);
    let op = running.client.apply_runtime_config(apply.clone()).await.unwrap();
    assert!(op.durable);
    assert_eq!(op.persistence_mode, "exact_configuration_file");
    let replay = running.client.apply_runtime_config(apply.clone()).await.unwrap();
    assert_eq!(replay.active, op.active);
    assert_eq!(replay.transaction_id, op.transaction_id);
    let status = running.status(&op.transaction_id).await;
    assert_eq!(status.lookup_state, daemon::RuntimeLookupState::Found as i32);
    assert_eq!(status.operation.as_ref().unwrap().active, op.active);
    let retry = running.client.retry_runtime_persistence(daemon::RuntimePersistenceRetryRequest {
        contract_version:1, transaction_id:op.transaction_id.clone(), active:op.active.clone(),
        control_revision:op.control_revision, persistence_mode:op.persistence_mode.clone(),
    }).await.unwrap();
    assert!(retry.durable);
    assert_eq!(retry.active, op.active);
    let ack = running.client.acknowledge_runtime_persistence(daemon::RuntimePersistenceAcknowledgementRequest {
        contract_version:1, transaction_id:op.transaction_id.clone(), active:op.active.clone(),
        control_revision:op.control_revision, persistence_mode:"app_authoritative_regeneration".into(),
        desired_state_digest:"0".repeat(64), desired_state_revision:"fixture-not-an-owner-receipt".into(),
    }).await;
    failed_precondition(ack); // Exact-file CLI cannot manufacture regeneration Ack.
    let mut wrong = apply;
    wrong.candidate_digest = "0".repeat(64);
    failed_precondition(running.client.apply_runtime_config(wrong).await);
    let wrong_status = running.client.get_runtime_config_status(daemon::RuntimeConfigStatusRequest {
        contract_version:1, transaction_id:op.transaction_id.clone(),runtime_incarnation:"different-instance".into(),
    }).await.unwrap();
    assert_eq!(wrong_status.lookup_state, daemon::RuntimeLookupState::WrongIncarnation as i32);
    failed_precondition(running.client.plan_runtime_config(request).await);
    assert_eq!(running.unchanged_process().await.active, op.active);
    evidence.record(json!({"phase":"six-public-rpc","methods":METHODS,"unauthenticatedPerMethod":2,
        "noOpOperation":operation(&no_op_result),"operation":operation(&op),"sameTransactionReplay":true,
        "terminalRetry":operation(&retry),"wrongIncarnationLookup":wrong_status.lookup_state,
        "cliRegenerationAckRejected":true,"positiveRegenerationAck":"requires real libbox handler companion evidence"}));
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "GitHub exact-source real core only; POLARIS_SINGBOX_PATH alone is insufficient"]
async fn real_hot_dispatcher_rejections_preserve_runtime() {
    let _serial = lock_real_core_tests().await;
    let admission = Admission::read();
    let mut evidence = Evidence::new(&admission, "real_hot_dispatcher_rejections_preserve_runtime");
    let network = Network::new();
    let running = Running::start(&admission, network.config(), &mut evidence).await;
    let snapshot = running.rt.current_config_snapshot().unwrap();
    let source = std::fs::read(running.rt.runtime_config_path()).unwrap();
    let mut desired = running.rt.config.current().unwrap();
    let port = loop { let p = free_port(); if p != running.mixed { break p; } };
    desired["mixedPort"] = port.into();
    running.rt.config.save_full(&desired).unwrap();
    let result = running.rt.apply_pending_transaction(None).await;
    assert_eq!(result.status, "restartRequired");
    assert!(result.restart_required && result.restart_confirmation.is_some());
    assert!(result.apply_id.is_none());
    assert!(running.rt.pending_force_restart.read().unwrap().is_none(), "no implicit restart source");
    assert_eq!(running.rt.current_config_snapshot().unwrap(), snapshot);
    assert_eq!(running.rt.config.current().unwrap()["mixedPort"], port);
    assert_eq!(std::fs::read(running.rt.runtime_config_path()).unwrap(), source);
    assert_eq!(running.unchanged_process().await.active, running.initial.active);
    // The real store is allowed to reject an invalid reference before dispatcher.
    let mut invalid = snapshot.clone();
    invalid["trafficRules"][0]["effects"]["route"]["targetServerId"] = "nonexistent-node".into();
    invalid["trafficRules"][0]["targetServerId"] = "nonexistent-node".into();
    let invalid_saved = running.rt.config.save_full(&invalid);
    let invalid_stage = if invalid_saved.is_err() {
        "store"
    } else {
        let rejected = running.rt.apply_pending_transaction(None).await;
        assert!(matches!(rejected.status, "rejected" | "restartRequired"));
        assert!(rejected.apply_id.is_none());
        "dispatcher"
    };
    assert!(running.rt.pending_force_restart.read().unwrap().is_none());
    assert_eq!(running.unchanged_process().await.active, running.initial.active);
    assert_eq!(running.rt.current_config_snapshot().unwrap(), snapshot);
    assert_eq!(std::fs::read(running.rt.runtime_config_path()).unwrap(), source);
    evidence.record(json!({"phase":"rejected","structuralResult":result,
        "invalidReferenceRejectedAt":invalid_stage,"savedStructuralIntentPreservedBeforeInvalidCase":true,
        "runtimeSourceUnchanged":true,"restartConfirmed":false}));
    running.finish(&admission, &mut evidence).await;
    network.assert_only_fixture_queries();
    drop(network);
    evidence.passed();
}
