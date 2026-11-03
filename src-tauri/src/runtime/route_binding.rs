//! TUN 节点物理拨号的逐目的网卡规划。
//!
//! sing-box 的 `route.auto_detect_interface` 能原生跟随默认接口，但无法表达「节点 A 经 Wi-Fi、
//! 节点 B 经企业 VPN」这类偏离默认出口的逐目的路由。这里在 TUN 接管路由之前读取系统路由表：
//! 与默认出口一致的根保留原生自动探测，只有不同的根才以会话级 `bind_interface` 注入真正的物理
//! 拨号根；不写回用户配置，显式节点/订阅/全局策略仍优先。

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

// `RoutePrefix` / `RuntimeBindingPlan` 已下沉到 `polaris-platform-events`（E2②）。
//
// 这里**不再导出**：方案原本想用 `pub use` 让 `crate::runtime::route_binding::{…}` 这条路径上的
// 消费点一字不改，但 clippy 当场指出 `pub use … RoutePrefix` 在普通构建里是 unused ——
// 本模块自己已经不用它，而 Linux 上唯一的消费点是 `proxy.rs` 那条 `#[cfg(test)]` import。
// 也就是说那条再导出没有非测试消费者：留着就是拿一个 `#[allow]` 去盖真信号。
// 消费点改成直接 import 新 crate，依赖关系因此是诚实的。
use polaris_platform_events::RuntimeBindingPlan;

use polaris_config_engine::builder::endpoint_routes::{
    active_physical_root_ids, hot_switch_physical_root_ids,
};
use polaris_config_engine::builder::outbounds::effective_proxy_bind_interface;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::{Protocol, ServerConfig};
use polaris_helper_proto::Platform;
#[cfg(not(windows))]
use polaris_system_integration::route_ops::SystemRouteOps;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

const LOOKUP_TIMEOUT: Duration = Duration::from_millis(1_500);
const PLAN_BUDGET: Duration = Duration::from_secs(3);
const MAX_CONCURRENT_TARGETS: usize = 8;
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    server_id: String,
    host: String,
}

fn group_candidates_by_host(candidates: Vec<Candidate>) -> BTreeMap<String, Vec<String>> {
    let mut grouped = BTreeMap::<String, Vec<String>>::new();
    for candidate in candidates {
        grouped
            .entry(candidate.host)
            .or_default()
            .push(candidate.server_id);
    }
    grouped
}

/// 本平台**有没有**逐目的绑定规划所需的那一条事实腿。
///
/// # 判据（不是保守取舍，是能力事实）
///
/// 本模块只有一个事实来源：`query_route_interface` → `SystemRouteOps::exit_interface_for`
/// （「到这个目的会走哪张网卡」）。它按 [`Platform`] 分派，而 `Platform::Other` 那条腿的实现就是
/// `Ok(None)`（`crates/system-integration/src/route_ops.rs`，原文「未知平台：无对应路由工具 →
/// 查不到」）。**Android 落在 `Other`**：`Platform::current()` 只认 `macos`/`windows`/`linux`，
/// 而 `target_os = "android"` 三个都不是。
///
/// 于是在 Android 上这份规划的结局是**恒定**的：每个候选都拿不到接口名 ⇒
/// `bindings` 恒空、`native_roots` 恒空、全部候选留在 `unresolved_roots`。规划一次不产出任何
/// `bind_interface`，重新规划一次也不会。
///
/// # 为什么这不是「少跑一段没用的探测」，而是一条起停循环的根因
///
/// `unresolved_roots` 非空正是 `proxy::route_replan::inferred_binding_replan_needed` 里**最激进**的
/// 那条腿：它会退回「整份网卡指纹逐项对比」。而起核就绪后跑的那次对比，前后两份指纹之间必然多出**我们自己刚建出来
/// 的 TUN**（Android 由 `VpnService.establish()` 建 `tun0`，连 link-local 每次都换）。desktop 靠
/// `managed_tun_interface_for_network_watcher` 把自家 TUN 从对比里剔掉，而那个函数在 `Other` 上
/// 同样返回 `None` ⇒ 剔不掉 ⇒ 判据恒真 ⇒ 每次起核收尾都排一次去抖重启 ⇒ 起停循环
///（真机实测 300s / 69 次 spawn，见
/// `~/docs/polaris/design/polaris-android-core-respawn-2026-09-04.md`）。
///
/// 所以这里关掉的不是「一段耗时」，而是「一条在本平台上永远给不出答案、却被下游当成『情况有变』
/// 来消费的判据输入」。Android 上跟随默认网卡的机制另有其人且已接线：libbox
/// `PlatformInterface.autoDetectInterfaceControl` + `DefaultNetworkMonitor.updateDefaultInterface`
///（K2 落地），它不需要、也不应该用整核重启来表达网络变化。
///
/// **穷举 match 而非 `!matches!(Other)`**：将来加平台变体时编译器强制在这里做一次决定，
/// 而不是默默继承「支持」。
#[must_use]
pub fn runtime_binding_planning_supported(platform: Platform) -> bool {
    match platform {
        Platform::Mac | Platform::Win | Platform::Linux => true,
        // Android 与 Other 同答 false，但**理由是具体的、不是兜底**：本平台没有可用的逐目的
        // 路由查询腿（`SystemRouteOps::exit_interface_for` 的 Android 臂恒 `Ok(None)`，见该处
        // 血证注释），规划再跑一次也只会得到同一份「全部未决」。K6b 的修复就锚在这一行。
        Platform::Android => false,
        // iOS = false，理由具体：`SystemRouteOps::exit_interface_for` 的 `Ios` 臂恒 `Ok(None)`
        // （沙箱内没有逐目的路由查询面），规划再跑一次也只会得到同一份「全部未决」。
        // 这一行同时是 K6b 那条链在 iOS 上的**预先切断**：`managed_tun_interface_for_network_watcher`
        // 在 iOS 上同样返 None（拿不到自家 tun 名字），若这里判 true，「整份网卡指纹逐项对比」
        // 就会重新变成恒真判据 —— 与 Android 上那 300s/69 次是同一条链。
        Platform::Ios => false,
        Platform::Other => false,
    }
}

/// 当前配置是否含至少一个需要在起核前判定“原生默认出口 / 特殊逐目的绑定”的物理拨号根。
/// 网络变化 watcher 用同一候选口径；是否真要重启再由本次 plan 的特殊绑定/未解析状态决定。
#[must_use]
pub fn needs_runtime_binding_plan(config: &UserConfig) -> bool {
    !hot_switch_runtime_binding_candidates(Platform::current(), config).is_empty()
}

/// 当前配置里**正在承流**且需要自动逐目的规划的物理根。
///
/// 配置变更/热切保护门只消费本集合，不能改成 hot-switch 全集：订阅新增一个尚未选中的节点不应让
/// 当前内核重启。全集只由 [`plan_runtime_bindings`] 在起核前消费并记入 `covered_roots`。
/// 显式策略根不进入本集合；非 TUN 模式由 OS 原生路由接管。
#[must_use]
pub fn automatic_runtime_binding_root_ids(config: &UserConfig) -> BTreeSet<String> {
    runtime_binding_candidates_for_roots(
        Platform::current(),
        config,
        active_physical_root_ids(config),
    )
    .into_iter()
    .map(|candidate| candidate.server_id)
    .collect()
}

/// Pure reuse guard for daily graph edits while TUN owns system routes. Protocol
/// credentials/transport edits retain the route fact; a changed automatic target
/// host or uncovered physical root cannot reuse an old destination decision.
pub(crate) fn runtime_binding_targets_reusable(
    old: &UserConfig,
    new: &UserConfig,
    plan: &RuntimeBindingPlan,
) -> bool {
    let previous: BTreeMap<_, _> = hot_switch_runtime_binding_candidates(Platform::current(), old)
        .into_iter()
        .map(|candidate| (candidate.server_id, candidate.host))
        .collect();
    runtime_binding_candidates_for_roots(Platform::current(), new, active_physical_root_ids(new))
        .into_iter()
        .all(|candidate| {
            plan.covered_roots.contains(&candidate.server_id)
                && previous.get(&candidate.server_id) == Some(&candidate.host)
        })
}

/// 在 TUN 接管系统路由之前生成会话级绑定。任何单目标失败都 fail-open：该节点继续使用 sing-box
/// `auto_detect_interface`；其余成功节点不受影响。整轮有硬预算，不能把坏 DNS/路由工具拖进冷启动关键路径。
pub async fn plan_runtime_bindings(config: &UserConfig) -> RuntimeBindingPlan {
    let candidates = hot_switch_runtime_binding_candidates(Platform::current(), config);
    let covered_roots: BTreeSet<String> = candidates
        .iter()
        .map(|candidate| candidate.server_id.clone())
        .collect();
    let candidate_count = covered_roots.len();
    if candidate_count == 0 {
        return RuntimeBindingPlan::default();
    }
    let grouped = group_candidates_by_host(candidates);
    // 先把「每个根的探测目标」落成具名表，再在得出决策时逐个划掉。剩下的就是「不知道的是谁」，
    // 无论它是 DNS 失败、路由查询无果，还是整轮预算超时被 abort、根本没回过包。
    let mut unresolved_roots: BTreeMap<String, String> = grouped
        .iter()
        .flat_map(|(host, server_ids)| server_ids.iter().map(|id| (id.clone(), host.clone())))
        .collect();

    let allow_ipv6 = config.enable_ipv6 == Some(true);
    let limiter = Arc::new(Semaphore::new(MAX_CONCURRENT_TARGETS));
    let default_v4 = Arc::new(tokio::sync::OnceCell::<Option<String>>::new());
    let default_v6 = Arc::new(tokio::sync::OnceCell::<Option<String>>::new());
    let mut tasks = JoinSet::new();
    for (host, server_ids) in grouped {
        let limiter = Arc::clone(&limiter);
        let default_v4 = Arc::clone(&default_v4);
        let default_v6 = Arc::clone(&default_v6);
        tasks.spawn(async move {
            // 失败腿一律带着 server_ids 回来：用 `?` 早退等于把这些根的身份整个丢掉，只在
            // 计数里留下一个数字，下游再也无法判断某条路由前缀与它们是否相关。
            let Ok(_permit) = limiter.acquire_owned().await else {
                return RouteProbeOutcome::Unresolved { server_ids, host };
            };
            let Some(ip) = resolve_route_probe_ip(&host, allow_ipv6).await else {
                return RouteProbeOutcome::Unresolved { server_ids, host };
            };
            let decision = match query_route_interface(ip).await {
                Some(interface) if !interface.trim().is_empty() => {
                    let default_interface = match ip {
                        IpAddr::V4(_) => {
                            default_v4
                                .get_or_init(|| query_route_interface(DEFAULT_ROUTE_PROBE_V4))
                                .await
                        }
                        IpAddr::V6(_) => {
                            default_v6
                                .get_or_init(|| query_route_interface(DEFAULT_ROUTE_PROBE_V6))
                                .await
                        }
                    };
                    Some(classify_runtime_binding(
                        interface.trim(),
                        default_interface.as_deref(),
                    ))
                }
                _ => None,
            };
            RouteProbeOutcome::Resolved {
                server_ids,
                ip,
                decision,
            }
        });
    }

    let mut bindings = BTreeMap::new();
    let mut native_roots = BTreeSet::new();
    let mut probe_ips = BTreeMap::new();
    let collect = async {
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(RouteProbeOutcome::Resolved {
                    server_ids,
                    ip,
                    decision,
                }) => {
                    for server_id in server_ids {
                        probe_ips.insert(server_id.clone(), ip);
                        match decision.as_ref() {
                            Some(RuntimeBindingDecision::Bind(interface)) => {
                                bindings.insert(server_id.clone(), interface.clone());
                                unresolved_roots.remove(&server_id);
                            }
                            Some(RuntimeBindingDecision::Native) => {
                                unresolved_roots.remove(&server_id);
                                native_roots.insert(server_id);
                            }
                            // 目标 IP 有了但路由查询没结果：该根仍然无决策，留在未决集合里。
                            None => {}
                        }
                    }
                }
                Ok(RouteProbeOutcome::Unresolved { server_ids, host }) => {
                    log::warn!(
                        "TUN 逐目的网卡规划：`{host}` 未解析出探针 IP，{} 个根按未决处理（{}）",
                        server_ids.len(),
                        server_ids.join(",")
                    );
                }
                Err(_) => {}
            }
        }
    };
    if tokio::time::timeout(PLAN_BUDGET, collect).await.is_err() {
        // Drop/abort 等待中的异步任务；已进入 spawn_blocking 的系统查询自行在命令硬超时内收口。
        tasks.abort_all();
        log::warn!(
            "TUN 逐目的网卡规划超过 {}ms，已按已完成结果降级",
            PLAN_BUDGET.as_millis()
        );
    }
    log::info!(
        "TUN 逐目的网卡规划：候选 {candidate_count}，特殊路由绑定 {}，原生自动探测 {}，降级 {}",
        bindings.len(),
        native_roots.len(),
        unresolved_roots.len()
    );
    RuntimeBindingPlan {
        bindings,
        native_roots,
        covered_roots,
        probe_ips,
        unresolved_roots,
        candidate_count,
    }
}

/// 单个探测目标的规划结局。**没有「什么都不回」的腿**：任何失败都必须把 `server_ids` 带回来。
enum RouteProbeOutcome {
    Resolved {
        server_ids: Vec<String>,
        ip: IpAddr,
        decision: Option<RuntimeBindingDecision>,
    },
    Unresolved {
        server_ids: Vec<String>,
        host: String,
    },
}

const DEFAULT_ROUTE_PROBE_V4: IpAddr = polaris_system_integration::route_ops::PROBE_IP;
const DEFAULT_ROUTE_PROBE_V6: IpAddr = IpAddr::V6(std::net::Ipv6Addr::new(
    0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111,
));

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeBindingDecision {
    Bind(String),
    Native,
}

fn classify_runtime_binding(
    destination_interface: &str,
    default_interface: Option<&str>,
) -> RuntimeBindingDecision {
    let destination_interface = destination_interface.trim();
    match default_interface.map(str::trim) {
        Some(default_interface) if destination_interface == default_interface => {
            RuntimeBindingDecision::Native
        }
        // 默认出口不可读但节点逐目的路由可读时，保留已知的安全路径；把它误判成 native 可能让
        // sing-box 回退到另一张默认网卡。后续物理路由事件仍会触发一次重规划。
        _ => RuntimeBindingDecision::Bind(destination_interface.to_owned()),
    }
}

fn hot_switch_runtime_binding_candidates(
    platform: Platform,
    config: &UserConfig,
) -> Vec<Candidate> {
    runtime_binding_candidates_for_roots(platform, config, hot_switch_physical_root_ids(config))
}

/// **`platform` 是入参而不是 `cfg!`**：判定要在任何 host 上都跑得到（同
/// `should_probe_wintun_adapter` 的手法）。写成 `cfg!` 就只能在一台永远不编 Android 的机器上
/// 装装样子 —— 而本条判据的全部意义正是「Android 上它必须是空的」。
fn runtime_binding_candidates_for_roots(
    platform: Platform,
    config: &UserConfig,
    roots: BTreeSet<String>,
) -> Vec<Candidate> {
    if !runtime_binding_planning_supported(platform) {
        return Vec::new();
    }
    // 接管方式取**本平台生效值**（[`ProxyModeType::effective_on`]）。Android 上零行为差 ——
    // 上面那道平台闸已经先返空集 —— 接上是为了不留需要人工复核的例外：两道闸的存续期不同，
    // 上面那条将来若因别的理由放开，这条不能跟着一起失守。
    if !config.proxy_mode_type.effective_on(platform).is_tun() {
        return Vec::new();
    }
    let by_id: BTreeMap<&str, &ServerConfig> = config
        .servers
        .iter()
        .map(|server| (server.id.as_str(), server))
        .collect();
    roots
        .into_iter()
        .filter_map(|id| {
            let server = by_id.get(id.as_str()).copied()?;
            if effective_proxy_bind_interface(server, config).is_some() {
                return None;
            }
            Some(Candidate {
                server_id: id,
                host: server_route_host(server)?,
            })
        })
        .collect()
}

fn server_route_host(server: &ServerConfig) -> Option<String> {
    let raw = match server.protocol {
        // 无服务器主机可绑：Tailcat 同 Tor 没有 server（DERP 主机由内核按地图/servers 自行拨）。
        Protocol::Tailscale | Protocol::Tor | Protocol::Tailcat => return None,
        Protocol::Openconnect => server.openconnect_settings.as_ref()?.server.as_deref()?,
        Protocol::OpenvpnClient => server.openvpn_client_settings.as_ref()?.server.as_deref()?,
        Protocol::Custom => server
            .custom_settings
            .as_ref()
            .and_then(|settings| settings.outbound.get("server"))
            .and_then(serde_json::Value::as_str)
            .filter(|server| !server.trim().is_empty())
            .or_else(|| (!server.address.trim().is_empty()).then_some(server.address.as_str()))?,
        _ => server.address.as_str(),
    };
    normalize_host(raw)
}

fn normalize_host(raw: &str) -> Option<String> {
    let mut value = raw.trim();
    if let Some((_, rest)) = value.split_once("://") {
        value = rest;
    }
    value = value.split('/').next().unwrap_or(value);
    value = value.rsplit('@').next().unwrap_or(value);
    if let Some(rest) = value.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        return (!host.is_empty()).then(|| host.to_owned());
    }
    if value.parse::<IpAddr>().is_ok() {
        return Some(value.to_owned());
    }
    if value.matches(':').count() == 1 {
        if let Some((host, port)) = value.rsplit_once(':') {
            if !host.is_empty() && port.parse::<u16>().is_ok_and(|port| port > 0) {
                value = host;
            }
        }
    }
    (!value.is_empty()).then(|| value.to_owned())
}

async fn resolve_route_probe_ip(host: &str, allow_ipv6: bool) -> Option<IpAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        // `enableIPv6` 约束域名的 AAAA 选择，不得把用户明确填写的 IPv6 节点地址当成“不可解析”。
        // 字面量无需 DNS，且三平台路由 API 都支持按 v6 目的查询。
        return Some(ip);
    }
    let resolved = tokio::time::timeout(LOOKUP_TIMEOUT, tokio::net::lookup_host((host, 0)))
        .await
        .ok()?
        .ok()?;
    let mut first_v6 = None;
    for address in resolved {
        match address.ip() {
            ip @ IpAddr::V4(_) => return Some(ip),
            ip @ IpAddr::V6(_) if first_v6.is_none() => first_v6 = Some(ip),
            _ => {}
        }
    }
    allow_ipv6.then_some(first_v6).flatten()
}

async fn query_route_interface(ip: IpAddr) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        #[cfg(windows)]
        {
            polaris_helper::platform::windows::wintun::best_route_interface_alias(ip).ok()
        }
        #[cfg(not(windows))]
        {
            polaris_system_integration::production_route_ops()
                .exit_interface_for(ip)
                .ok()
                .flatten()
        }
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests;

/// Retained pre-TUN physical facts. No mutable config file, current system DNS
/// lookup, hard-coded resolver or newly selected interface can create this owner.
#[derive(Debug)]
pub(crate) struct ProtectedPhysicalRoot {
    captured_generation: u64,
    captured_network_epoch: u64,
    revoked: std::sync::atomic::AtomicBool,
    #[cfg(target_os = "linux")]
    facts: polaris_system_integration::route_ops::LinuxPhysicalRouteFacts,
    #[cfg(target_os = "linux")]
    physical: PhysicalLinkPin,
    #[cfg(target_os = "linux")]
    namespace: std::fs::File,
    #[cfg(target_os = "linux")]
    pre_links: BTreeSet<String>,
    #[cfg(target_os = "linux")]
    seal: std::sync::Mutex<Option<Arc<PhysicalRootSeal>>>,
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct PhysicalLinkPin {
    directory: std::fs::File,
    name: String,
    index: u32,
    address: String,
}

#[cfg(target_os = "linux")]
impl PhysicalLinkPin {
    fn open(name: &str, physical: bool) -> Result<Self, String> {
        if name.is_empty()
            || name.len() >= 16
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
        {
            return Err("invalid physical link name".into());
        }
        let path = std::path::Path::new("/sys/class/net").join(name);
        if physical && !path.join("device").exists() {
            return Err("default route is not a physical device".into());
        }
        let directory = std::fs::File::open(&path).map_err(|_| "physical link cannot be pinned")?;
        let index = std::fs::read_to_string(path.join("ifindex"))
            .map_err(|_| "physical ifindex unavailable")?
            .trim()
            .parse()
            .map_err(|_| "invalid physical ifindex")?;
        let address = std::fs::read_to_string(path.join("address"))
            .map_err(|_| "physical link address unavailable")?;
        let pin = Self {
            directory,
            name: name.to_owned(),
            index,
            address,
        };
        pin.verify()?;
        Ok(pin)
    }
    fn verify(&self) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        let held = self
            .directory
            .metadata()
            .map_err(|_| "held physical link disappeared")?;
        let path = std::path::Path::new("/sys/class/net").join(&self.name);
        let current = std::fs::metadata(&path).map_err(|_| "physical link disappeared")?;
        let index: u32 = std::fs::read_to_string(path.join("ifindex"))
            .map_err(|_| "physical ifindex disappeared")?
            .trim()
            .parse()
            .map_err(|_| "invalid current ifindex")?;
        let address = std::fs::read_to_string(path.join("address"))
            .map_err(|_| "physical link address disappeared")?;
        if held.dev() != current.dev()
            || held.ino() != current.ino()
            || index != self.index
            || address != self.address
            || nix::net::if_::if_nametoindex(self.name.as_str())
                .map_err(|_| "physical link name disappeared")?
                != self.index
        {
            return Err("physical link birth/index/address changed".into());
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
struct PhysicalRootSeal {
    generation: u64,
    network_epoch: u64,
    tun: PhysicalLinkPin,
    view: polaris_system_integration::route_ops::LinuxProtectedRouteView,
}

/// A candidate owns this proof independently of the currently admitted graph.
/// Dropping it never updates accepted route facts. Blocking IO retains its Arc
/// until real return; cancel/timeout is never reported as an IO join.
#[derive(Debug, Clone)]
pub(crate) struct ProtectedBindingPlan {
    plan: RuntimeBindingPlan,
    #[cfg(target_os = "linux")]
    owner: Arc<ProtectedPhysicalRoot>,
    generation: u64,
    network_epoch: u64,
    expires: std::time::Instant,
    #[cfg(target_os = "linux")]
    targets: Vec<(std::net::Ipv4Addr, u16)>,
}

pub(crate) async fn capture_protected_physical_root(
    generation: u64,
    network_epoch: u64,
) -> Result<Arc<ProtectedPhysicalRoot>, String> {
    if generation == 0 {
        return Err("physical source has no original startup generation".into());
    }
    #[cfg(target_os = "linux")]
    return tokio::task::spawn_blocking(move || {
        let namespace = std::fs::File::open("/proc/self/ns/net")
            .map_err(|_| "network namespace cannot be retained")?;
        let ops = polaris_system_integration::production_route_ops();
        let facts = ops
            .capture_linux_physical_route()
            .map_err(|e| e.to_string())?;
        let physical = PhysicalLinkPin::open(&facts.interface, true)?;
        if physical.index != facts.ifindex {
            return Err("physical link changed during capture".into());
        }
        let pre_links = std::fs::read_dir("/sys/class/net")
            .map_err(|_| "pre-TUN links unavailable")?
            .map(|entry| {
                entry
                    .map_err(|_| "pre-TUN link unavailable")
                    .and_then(|entry| {
                        entry
                            .file_name()
                            .into_string()
                            .map_err(|_| "non-UTF8 link name")
                    })
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        physical.verify()?;
        if ops
            .capture_linux_physical_route()
            .map_err(|e| e.to_string())?
            != facts
        {
            return Err("physical route facts changed during pre-TUN capture".into());
        }
        let owner = Arc::new(ProtectedPhysicalRoot {
            captured_generation: generation,
            captured_network_epoch: network_epoch,
            revoked: std::sync::atomic::AtomicBool::new(false),
            facts,
            physical,
            namespace,
            pre_links,
            seal: std::sync::Mutex::new(None),
        });
        owner.verify_namespace()?;
        owner.physical.verify()?;
        Ok(owner)
    })
    .await
    .map_err(|_| "physical capture task did not return".to_owned())?;
    #[cfg(not(target_os = "linux"))]
    {
        let _ = network_epoch;
        Err("protected physical root source unavailable on this platform".into())
    }
}

impl ProtectedPhysicalRoot {
    pub(crate) fn revoke(&self) {
        self.revoked
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    #[cfg(target_os = "linux")]
    fn verify_namespace(&self) -> Result<(), String> {
        use std::os::unix::fs::MetadataExt;
        let held = self
            .namespace
            .metadata()
            .map_err(|_| "held network namespace unavailable")?;
        let current =
            std::fs::metadata("/proc/self/ns/net").map_err(|_| "network namespace unavailable")?;
        if held.dev() != current.dev() || held.ino() != current.ino() {
            return Err("network namespace changed".into());
        }
        Ok(())
    }
    /// The caller supplies the actual ready generation and watcher epoch, not
    /// an expected table number. The unique new table is read back from kernel.
    /// Root must first prove generated mark=0 and auto_redirect=false for all
    /// relevant dialers; absent/unknown native controls are not permission flags.
    pub(crate) async fn seal(
        self: &Arc<Self>,
        managed_tun: &str,
        generation: u64,
        network_epoch: u64,
    ) -> Result<(), String> {
        if generation != self.captured_generation || network_epoch != self.captured_network_epoch {
            return Err(
                "pre-TUN physical source belongs to another startup or network epoch".into(),
            );
        }
        #[cfg(target_os = "linux")]
        {
            let owner = Arc::clone(self);
            let tun = managed_tun.to_owned();
            return tokio::task::spawn_blocking(move || {
                if generation == 0
                    || owner.revoked.load(std::sync::atomic::Ordering::SeqCst)
                    || owner.pre_links.contains(&tun)
                {
                    return Err("TUN is not a new link owned by this startup".into());
                }
                owner.verify_namespace()?;
                owner.physical.verify()?;
                let pin = PhysicalLinkPin::open(&tun, false)?;
                if pin.index == owner.physical.index {
                    return Err("physical root is the managed TUN".into());
                }
                let view = polaris_system_integration::production_route_ops()
                    .observe_linux_protected_route(&owner.facts, &tun)
                    .map_err(|e| e.to_string())?;
                pin.verify()?;
                owner.physical.verify()?;
                let mut slot = owner
                    .seal
                    .lock()
                    .map_err(|_| "physical source owner unavailable")?;
                if slot.is_some() || owner.revoked.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err("physical source was already sealed or revoked".into());
                }
                *slot = Some(Arc::new(PhysicalRootSeal {
                    generation,
                    network_epoch,
                    tun: pin,
                    view,
                }));
                Ok(())
            })
            .await
            .map_err(|_| "physical seal task did not return".to_owned())?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (managed_tun, generation, network_epoch);
            Err("protected physical source unavailable on this platform".into())
        }
    }
    #[cfg(target_os = "linux")]
    fn verify_now(&self, generation: u64, network_epoch: u64) -> Result<(), String> {
        if self.revoked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("original physical source is revoked".into());
        }
        let seal = self
            .seal
            .lock()
            .map_err(|_| "physical source lock unavailable")?
            .clone()
            .ok_or("physical source is not sealed")?;
        if seal.generation != generation || seal.network_epoch != network_epoch {
            return Err("physical source generation/network epoch changed".into());
        }
        self.verify_namespace()?;
        self.physical.verify()?;
        seal.tun.verify()?;
        let view = polaris_system_integration::production_route_ops()
            .observe_linux_protected_route(&self.facts, &seal.tun.name)
            .map_err(|e| e.to_string())?;
        if view != seal.view {
            return Err("physical route policy changed after ready".into());
        }
        self.physical.verify()?;
        seal.tun.verify()?;
        if self.revoked.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("physical source was revoked during observation".into());
        }
        Ok(())
    }
}

impl ProtectedBindingPlan {
    pub(crate) fn binding_plan(&self) -> &RuntimeBindingPlan {
        &self.plan
    }
    /// Memory-only final publication fence. The caller must pass its current
    /// watcher epoch while holding its existing generation/intent gate.
    pub(crate) fn publication_current(&self, generation: u64, network_epoch: u64) -> bool {
        generation == self.generation
            && network_epoch == self.network_epoch
            && std::time::Instant::now() < self.expires
            && !self.owner_revoked()
    }
    fn owner_revoked(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.owner.revoked.load(std::sync::atomic::Ordering::SeqCst)
        }
        #[cfg(not(target_os = "linux"))]
        {
            true
        }
    }
    pub(crate) async fn verify(&self, generation: u64, network_epoch: u64) -> Result<(), String> {
        if !self.publication_current(generation, network_epoch) {
            return Err("protected route proof expired, revoked or changed owner".into());
        }
        #[cfg(target_os = "linux")]
        {
            let owner = Arc::clone(&self.owner);
            let targets = self.targets.clone();
            let expires = self.expires;
            return tokio::task::spawn_blocking(move || {
                owner.verify_now(generation, network_epoch)?;
                let ops = polaris_system_integration::production_route_ops();
                for (ip, port) in targets {
                    for transport in ["tcp", "udp"] {
                        ops.verify_linux_bound_route(
                            &owner.facts.interface,
                            owner.facts.source,
                            ip,
                            port,
                            transport,
                        )
                        .map_err(|e| e.to_string())?;
                    }
                }
                owner.verify_now(generation, network_epoch)?;
                if std::time::Instant::now() >= expires {
                    return Err("protected DNS proof expired during verification".into());
                }
                Ok(())
            })
            .await
            .map_err(|_| "physical verification task did not return".to_owned())?;
        }
        #[cfg(not(target_os = "linux"))]
        Err("protected physical proof unavailable on this platform".into())
    }
}

pub(crate) async fn plan_protected_runtime_bindings(
    owner: Arc<ProtectedPhysicalRoot>,
    trusted: &UserConfig,
    candidate: &UserConfig,
    accepted: &RuntimeBindingPlan,
    generation: u64,
    network_epoch: u64,
) -> Result<ProtectedBindingPlan, String> {
    #[cfg(target_os = "linux")]
    {
        let trusted = trusted.clone();
        let candidate = candidate.clone();
        let accepted = accepted.clone();
        return tokio::task::spawn_blocking(move || {
            owner.verify_now(generation, network_epoch)?;
            let ops = polaris_system_integration::production_route_ops();
            let started = std::time::Instant::now();
            let (plan, ttl, targets) = extend_protected_binding_plan(
                &trusted,
                &candidate,
                &accepted,
                &owner.facts.interface,
                |host| protected_resolve_v4(&owner, host),
                |ip, port| {
                    for transport in ["tcp", "udp"] {
                        ops.verify_linux_bound_route(
                            &owner.facts.interface,
                            owner.facts.source,
                            ip,
                            port,
                            transport,
                        )
                        .map_err(|e| e.to_string())?;
                    }
                    Ok(())
                },
            )?;
            owner.verify_now(generation, network_epoch)?;
            let expires = started + ttl;
            if std::time::Instant::now() >= expires {
                return Err("protected DNS proof expired while planning".into());
            }
            Ok(ProtectedBindingPlan {
                plan,
                owner,
                generation,
                network_epoch,
                expires,
                targets,
            })
        })
        .await
        .map_err(|_| "protected root planning task did not return".to_owned())?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (
            owner,
            trusted,
            candidate,
            accepted,
            generation,
            network_epoch,
        );
        Err("protected root planner unavailable on this platform".into())
    }
}

#[cfg(any(target_os = "linux", test))]
fn extend_protected_binding_plan(
    trusted: &UserConfig,
    candidate: &UserConfig,
    accepted: &RuntimeBindingPlan,
    interface: &str,
    mut resolve: impl FnMut(&str) -> Result<(std::net::Ipv4Addr, Duration), String>,
    mut verify: impl FnMut(std::net::Ipv4Addr, u16) -> Result<(), String>,
) -> Result<(RuntimeBindingPlan, Duration, Vec<(std::net::Ipv4Addr, u16)>), String> {
    let old: BTreeMap<_, _> = hot_switch_runtime_binding_candidates(Platform::Linux, trusted)
        .into_iter()
        .map(|c| (c.server_id, c.host))
        .collect();
    let candidates = runtime_binding_candidates_for_roots(
        Platform::Linux,
        candidate,
        active_physical_root_ids(candidate),
    );
    if candidates.len() > MAX_CONCURRENT_TARGETS {
        return Err("protected physical root inventory exceeds budget".into());
    }
    // Advancing the trusted config after this candidate must not attest an
    // inactive edited host with its old covered fact. Remove only those stale
    // facts from the candidate clone; active roots below earn new facts.
    let new_hosts: BTreeMap<_, _> =
        hot_switch_runtime_binding_candidates(Platform::Linux, candidate)
            .into_iter()
            .map(|root| (root.server_id, root.host))
            .collect();
    let stale: Vec<_> = accepted
        .covered_roots
        .iter()
        .filter(|id| old.get(*id) != new_hosts.get(*id))
        .cloned()
        .collect();
    let mut next = accepted.clone();
    for id in stale {
        next.bindings.remove(&id);
        next.native_roots.remove(&id);
        next.covered_roots.remove(&id);
        next.probe_ips.remove(&id);
        next.unresolved_roots.remove(&id);
    }
    let mut ttl = Duration::from_secs(60);
    let mut targets = Vec::new();
    for root in candidates {
        if accepted.covered_roots.contains(&root.server_id)
            && !accepted.unresolved_roots.contains_key(&root.server_id)
            && old.get(&root.server_id) == Some(&root.host)
        {
            continue;
        }
        let server = candidate
            .servers
            .iter()
            .find(|server| server.id == root.server_id)
            .ok_or("protected root disappeared")?;
        if server.port == 0
            || matches!(
                server.protocol,
                Protocol::Custom | Protocol::Openconnect | Protocol::OpenvpnClient
            )
        {
            return Err(
                "protected root dialer requires an explicit native transport adapter".into(),
            );
        }
        let (ip, lifetime) = if let Ok(ip) = root.host.parse::<std::net::Ipv4Addr>() {
            (ip, ttl)
        } else {
            if root.host.parse::<std::net::IpAddr>().is_ok() {
                return Err("protected physical source has no IPv6 proof".into());
            }
            resolve(&root.host)?
        };
        if lifetime.is_zero() {
            return Err("protected DNS answer already expired".into());
        }
        verify(ip, server.port)?;
        ttl = ttl.min(lifetime);
        next.bindings
            .insert(root.server_id.clone(), interface.to_owned());
        next.native_roots.remove(&root.server_id);
        next.unresolved_roots.remove(&root.server_id);
        next.covered_roots.insert(root.server_id.clone());
        next.probe_ips.insert(root.server_id, IpAddr::V4(ip));
        targets.push((ip, server.port));
    }
    next.candidate_count = next.covered_roots.len();
    Ok((next, ttl, targets))
}

#[cfg(target_os = "linux")]
fn protected_resolve_v4(
    owner: &ProtectedPhysicalRoot,
    host: &str,
) -> Result<(std::net::Ipv4Addr, Duration), String> {
    let (id, query) = physical_dns_query(host)?;
    owner.verify_namespace()?;
    owner.physical.verify()?;
    let socket = std::net::UdpSocket::bind((owner.facts.source, 0))
        .map_err(|e| format!("physical DNS source bind failed: {e}"))?;
    let device = std::ffi::OsString::from(&owner.facts.interface);
    nix::sys::socket::setsockopt(&socket, nix::sys::socket::sockopt::BindToDevice, &device)
        .map_err(|e| format!("physical DNS SO_BINDTODEVICE failed: {e}"))?;
    let observed = nix::sys::socket::getsockopt(&socket, nix::sys::socket::sockopt::BindToDevice)
        .map_err(|e| format!("physical DNS device readback failed: {e}"))?;
    if observed != device {
        return Err("physical DNS device readback mismatch".into());
    }
    socket
        .set_read_timeout(Some(LOOKUP_TIMEOUT))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(LOOKUP_TIMEOUT))
        .map_err(|e| e.to_string())?;
    polaris_system_integration::production_route_ops()
        .verify_linux_bound_route(
            &owner.facts.interface,
            owner.facts.source,
            owner.facts.resolver,
            53,
            "udp",
        )
        .map_err(|e| e.to_string())?;
    if owner.revoked.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("physical DNS owner revoked before send".into());
    }
    socket
        .connect((owner.facts.resolver, 53))
        .map_err(|e| format!("physical DNS connect failed: {e}"))?;
    if socket
        .send(&query)
        .map_err(|e| format!("physical DNS send failed: {e}"))?
        != query.len()
    {
        return Err("physical DNS query was truncated".into());
    }
    let mut bytes = [0u8; 513];
    let count = socket
        .recv(&mut bytes)
        .map_err(|e| format!("physical DNS receive failed: {e}"))?;
    if count > 512 {
        return Err("physical DNS response exceeds UDP budget".into());
    }
    owner.physical.verify()?;
    if owner.revoked.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("physical DNS owner revoked after receive".into());
    }
    physical_dns_answer(&bytes[..count], id, host)
}

#[cfg(any(target_os = "linux", test))]
fn physical_dns_query(host: &str) -> Result<(u16, Vec<u8>), String> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.len() > 253 || !host.contains('.') {
        return Err("protected DNS requires an absolute ASCII hostname".into());
    }
    let nonce = uuid::Uuid::new_v4();
    let id = u16::from_be_bytes([nonce.as_bytes()[0], nonce.as_bytes()[1]]);
    let mut query = vec![(id >> 8) as u8, id as u8, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0];
    for label in host.split('.') {
        if label.is_empty()
            || label.len() > 63
            || !label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err("invalid protected DNS hostname".into());
        }
        query.push(label.len() as u8);
        query.extend_from_slice(label.as_bytes());
    }
    query.extend_from_slice(&[0, 0, 1, 0, 1]);
    Ok((id, query))
}

#[cfg(any(target_os = "linux", test))]
fn physical_dns_name(packet: &[u8], cursor: &mut usize) -> Result<String, String> {
    let mut offset = *cursor;
    let mut end = None;
    let mut labels = Vec::new();
    let mut visited = BTreeSet::new();
    for _ in 0..128 {
        if !visited.insert(offset) {
            return Err("DNS compression loop".into());
        }
        let length = *packet.get(offset).ok_or("truncated DNS name")? as usize;
        if length & 0xc0 == 0xc0 {
            let next = *packet.get(offset + 1).ok_or("truncated DNS pointer")? as usize;
            let target = ((length & 0x3f) << 8) | next;
            if target >= offset {
                return Err("DNS compression is not a backward reference".into());
            }
            end.get_or_insert(offset + 2);
            offset = target;
            continue;
        }
        if length & 0xc0 != 0 || length > 63 {
            return Err("invalid DNS label".into());
        }
        offset += 1;
        if length == 0 {
            *cursor = end.unwrap_or(offset);
            return Ok(labels.join("."));
        }
        let label = packet
            .get(offset..offset + length)
            .ok_or("truncated DNS label")?;
        if !label
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        {
            return Err("invalid DNS answer label".into());
        }
        labels.push(
            std::str::from_utf8(label)
                .map_err(|_| "non-ASCII DNS label")?
                .to_ascii_lowercase(),
        );
        offset += length;
        if labels.iter().map(String::len).sum::<usize>() + labels.len() > 254 {
            return Err("DNS name exceeds budget".into());
        }
    }
    Err("DNS compression exceeds budget".into())
}

#[cfg(any(target_os = "linux", test))]
fn physical_dns_answer(
    packet: &[u8],
    id: u16,
    host: &str,
) -> Result<(std::net::Ipv4Addr, Duration), String> {
    if packet.len() < 12
        || packet.len() > 512
        || u16::from_be_bytes([packet[0], packet[1]]) != id
        || packet[2] & 0xfa != 0x80
        || packet[3] & 0x0f != 0
        || packet[4..6] != [0, 1]
    {
        return Err("unmatched, truncated or failed physical DNS response".into());
    }
    let count = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    if count == 0 || count > 64 {
        return Err("physical DNS answer inventory invalid".into());
    }
    let expected = host.trim_end_matches('.').to_ascii_lowercase();
    let mut cursor = 12;
    if physical_dns_name(packet, &mut cursor)? != expected
        || packet.get(cursor..cursor + 4) != Some([0u8, 1, 0, 1].as_slice())
    {
        return Err("physical DNS question mismatch".into());
    }
    cursor += 4;
    let mut answers = Vec::new();
    let mut aliases = BTreeMap::new();
    for _ in 0..count {
        let name = physical_dns_name(packet, &mut cursor)?;
        let header = packet
            .get(cursor..cursor + 10)
            .ok_or("truncated DNS record")?;
        cursor += 10;
        let kind = u16::from_be_bytes([header[0], header[1]]);
        let class = u16::from_be_bytes([header[2], header[3]]);
        let ttl = u32::from_be_bytes([header[4], header[5], header[6], header[7]]);
        let length = u16::from_be_bytes([header[8], header[9]]) as usize;
        let data = packet
            .get(cursor..cursor + length)
            .ok_or("truncated DNS data")?;
        if class == 1 && kind == 1 && length == 4 {
            answers.push((
                name,
                std::net::Ipv4Addr::new(data[0], data[1], data[2], data[3]),
                ttl,
            ));
        } else if class == 1 && kind == 5 {
            let mut alias_cursor = cursor;
            let target = physical_dns_name(packet, &mut alias_cursor)?;
            if alias_cursor != cursor + length || aliases.insert(name, (target, ttl)).is_some() {
                return Err("invalid or ambiguous DNS alias".into());
            }
        }
        cursor += length;
    }
    let mut name = expected;
    let mut ttl = 60u32;
    let mut visited = BTreeSet::new();
    for _ in 0..16 {
        if !visited.insert(name.clone()) {
            return Err("DNS alias loop".into());
        }
        if let Some((_, ip, lifetime)) = answers.iter().find(|(owner, _, _)| owner == &name) {
            let octets = ip.octets();
            if ip.is_unspecified()
                || ip.is_loopback()
                || ip.is_multicast()
                || ip.is_broadcast()
                || (octets[0] == 198 && matches!(octets[1], 18 | 19))
                || *lifetime == 0
            {
                return Err("physical DNS answer is not a usable physical address".into());
            }
            return Ok((*ip, Duration::from_secs(u64::from(ttl.min(*lifetime)))));
        }
        let (target, lifetime) = aliases
            .get(&name)
            .ok_or("physical DNS has no matching IPv4 answer")?;
        ttl = ttl.min(*lifetime);
        if ttl == 0 {
            return Err("physical DNS alias expired".into());
        }
        name = target.clone();
    }
    Err("DNS alias depth exceeds budget".into())
}

/// Compare actual generated kernel policy, never desired config. Logical route
/// rules/selector final and ordinary node payloads may change. New bind_interface
/// values are supplied by the separately owned ProtectedBindingPlan; this
/// predicate does not manufacture those per-target facts.
pub(crate) fn protected_generated_policy_compatible(
    before: &polaris_config_engine::singbox::SingBoxConfig,
    after: &polaris_config_engine::singbox::SingBoxConfig,
) -> bool {
    fn controls_closed(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Object(object) => object.iter().all(|(key, value)| {
                if matches!(
                    key.as_str(),
                    "routing_mark"
                        | "default_mark"
                        | "auto_redirect_input_mark"
                        | "auto_redirect_output_mark"
                        | "auto_redirect_reset_mark"
                        | "auto_redirect_tproxy_mark"
                ) {
                    return value.as_u64() == Some(0);
                }
                if matches!(
                    key.as_str(),
                    "network_namespace"
                        | "netns"
                        | "inet4_bind_address"
                        | "inet6_bind_address"
                        | "network_strategy"
                        | "network_type"
                        | "fallback_network_type"
                        | "fallback_delay"
                        | "bind_network"
                        | "protect_path"
                        | "control_fd"
                        | "socket_path"
                        | "unix_path"
                        | "bind_address_no_port"
                        | "tcp_fast_open"
                        | "tcp_multi_path"
                        | "reuse_addr"
                        | "udp_fragment"
                        | "auto_redirect_nfqueue"
                ) {
                    return value.is_null() || value.as_bool() == Some(false);
                }
                controls_closed(value)
            }),
            serde_json::Value::Array(values) => values.iter().all(controls_closed),
            _ => true,
        }
    }
    fn projection(
        config: &polaris_config_engine::singbox::SingBoxConfig,
    ) -> Option<(serde_json::Value, serde_json::Value)> {
        let value = serde_json::to_value(config).ok()?;
        if !controls_closed(&value) {
            return None;
        }
        let tunnels: Vec<_> = value
            .get("inbounds")?
            .as_array()?
            .iter()
            .filter(|inbound| inbound["type"].as_str() == Some("tun"))
            .collect();
        if tunnels.len() != 1
            || tunnels[0]
                .get("auto_redirect")
                .is_some_and(|flag| flag.as_bool() != Some(false))
        {
            return None;
        }
        let outbounds = value.get("outbounds")?.as_array()?;
        if outbounds.iter().any(|outbound| {
            !matches!(
                outbound["type"].as_str(),
                Some(
                    "direct"
                        | "block"
                        | "selector"
                        | "urltest"
                        | "shadowsocks"
                        | "socks"
                        | "http"
                        | "vless"
                        | "vmess"
                        | "trojan"
                        | "hysteria"
                        | "hysteria2"
                        | "tuic"
                        | "naive"
                        | "snell"
                        | "anytls"
                        | "ssh"
                        | "shadowtls"
                        | "wireguard"
                )
            )
        }) {
            return None;
        }
        if outbounds.iter().any(|outbound| {
            outbound
                .get("server")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|server| server.contains('/') || server.starts_with("unix:"))
        }) {
            return None;
        }
        let mut route = value.get("route")?.as_object()?.clone();
        for logical in ["rules", "rule_set", "final"] {
            route.remove(logical);
        }
        // The remaining generated route fields include automatic/default
        // interface, resolver and future physical defaults. Unknown changes
        // cannot inherit old kernel policy by omission.
        Some(((*tunnels[0]).clone(), serde_json::Value::Object(route)))
    }
    match (projection(before), projection(after)) {
        (Some(before), Some(after)) => before == after,
        _ => false,
    }
}

/// Negative-only memory fixture for the real settlement/publication tests.
/// `expired=true` is already expired; `expired=false` is owner-revoked. Neither
/// branch can be admitted. The ordinary source-file handles below are explicit
/// placeholders, not sysfs/netns/Native custody, and seal is always None.
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn blocked_physical_binding_proof_for_test(
    generation: u64,
    network_epoch: u64,
    expired: bool,
) -> (Arc<ProtectedPhysicalRoot>, ProtectedBindingPlan) {
    struct MemoryReadback;
    impl polaris_system_integration::exec::CommandRunner for MemoryReadback {
        fn run(
            &self,
            command: &polaris_system_integration::exec::Command,
            _: Duration,
        ) -> Result<polaris_system_integration::exec::CommandOutput, String> {
            let output = match command.program.as_str() {
                "ip" if command.args.iter().any(|arg| arg == "rule") => {
                    r#"[{"priority":0,"src":"all","table":"local"},{"priority":32766,"src":"all","table":"main"},{"priority":32767,"src":"all","table":"default"}]"#
                }
                "ip" if command.args.iter().any(|arg| arg == "address") => {
                    r#"[{"ifname":"memory0","ifindex":2,"flags":["UP"],"operstate":"UP","addr_info":[{"family":"inet","scope":"global","local":"192.0.2.4"}]}]"#
                }
                "ip" if command.args.iter().any(|arg| arg == "get") => {
                    r#"[{"dev":"memory0","prefsrc":"192.0.2.4","table":"main"}]"#
                }
                "ip" => {
                    r#"[{"dst":"default","gateway":"192.0.2.1","dev":"memory0","table":"main"}]"#
                }
                "resolvectl" => match command.args.first().map(String::as_str) {
                    Some("dns") => "Global:\nLink 2 (memory0): 192.0.2.53\n",
                    Some("domain") => "Global:\nLink 2 (memory0):\n",
                    Some("default-route") => "Global:\nLink 2 (memory0): yes\n",
                    _ => return Err("unexpected negative-fixture resolver read".into()),
                },
                _ => return Err("unexpected negative-fixture read".into()),
            };
            Ok(polaris_system_integration::exec::CommandOutput {
                stdout: output.to_owned(),
                stderr: String::new(),
            })
        }
    }
    // Reuse the existing pure CommandRunner parser to obtain the otherwise
    // opaque cross-crate facts type; this runner cannot execute OS commands.
    let facts = polaris_system_integration::route_ops::SystemRouteOpsImpl::with_platform(
        MemoryReadback,
        Platform::Linux,
    )
    .capture_linux_physical_route()
    .expect("negative-only memory readback");
    let placeholder = std::fs::File::open(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("ordinary readonly fixture handle");
    let owner = Arc::new(ProtectedPhysicalRoot {
        captured_generation: generation,
        captured_network_epoch: network_epoch,
        revoked: std::sync::atomic::AtomicBool::new(!expired),
        facts,
        physical: PhysicalLinkPin {
            directory: placeholder
                .try_clone()
                .expect("readonly fixture handle clone"),
            name: "not-a-native-proof".into(),
            index: 0,
            address: String::new(),
        },
        namespace: placeholder,
        pre_links: BTreeSet::new(),
        seal: std::sync::Mutex::new(None),
    });
    let proof = ProtectedBindingPlan {
        plan: RuntimeBindingPlan::default(),
        owner: Arc::clone(&owner),
        generation,
        network_epoch,
        expires: if expired {
            std::time::Instant::now()
        } else {
            std::time::Instant::now() + Duration::from_secs(60)
        },
        targets: Vec::new(),
    };
    assert!(!proof.publication_current(generation, network_epoch));
    (owner, proof)
}
