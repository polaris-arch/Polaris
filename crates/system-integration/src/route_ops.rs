//! 系统路由出口接口探测 + TUN 出口夺取（post-flight）判定。
//!
//! 解决的问题（真机复现 `~/docs/polaris/design/polaris-tun-conflict-detect-design-2026-07-22.md`）：
//! 其他 VPN（如另一个 TUN 客户端）已占用系统默认路由时，Polaris 起核后 sing-box 的 utun 抢不到流量，
//! 但既有就绪门（进程活 + 管理 API 环回口可连）**照样全真** → 假报「已连接」、连接数恒 0、↓0↑0。
//!
//! ## 关键不变式（设计 §4.1 / §4.5）
//!
//! **成功接管 ⟺ 对「本应走代理的公网目的」的出口接口从 baseline 切走。** 直接观测这条比数流量更可靠：
//! 路由表在 utun 起来时即被写（与有无流量无关），故不受「用户恰好空闲、连接数 0」的假阳性影响。
//! macOS 内核给 tun 分配 `utunN` 名字、进程**无法预知自己的 utun 叫什么**（config-engine 不设
//! `interface_name`），故不靠名字识别「我方 utun」，而靠 **baseline 差分**：起核前记出口，起核 +grace
//! 后仍等于 baseline 那个「起核前就存在的接口」→ 判未夺到路由。
//!
//! ## 手段（设计 §4.5 / §4.6，D4/D5）
//!
//! probe 目的固定为公网 IP `1.1.1.1`（[`PROBE_IP`]）——`route get` / `ip route get` 是**路由表读**，
//! **不发包**、非 root、非破坏。命令经 [`CommandRunner`] 下发（mock 可单测），与 `dns_ops` 同缝。
//!
//! - **macOS**：`route -n get 1.1.1.1` → 解析 `interface: utunN`。**绝不用 `route -n get default`**：
//!   sing-box `auto_route` 装的是 `0.0.0.0/1`+`128.0.0.0/1` 两条半程路由、**不动** `0.0.0.0/0`，查 default
//!   会返物理网卡（假阴性）。查具体公网 IP 让 /1 半程按最长前缀命中（设计 §4.5 陷阱行）。
//! - **Linux**：`ip route get 1.1.1.1` → 解析 `dev tunX`。
//! - **Windows**：`Find-NetRoute -RemoteIPAddress 1.1.1.1` → 解析 `InterfaceAlias`（内核代算最优路由，
//!   等价 `route get`；设计 §4.6 「route print 最低 metric」的可解析替身）。首版命令式，PF_ROUTE 编程读延后。
//!
//! ## 零 cfg（与 `dns_ops` / `proxy_ops` 同纪律）
//!
//! 平台差异只是「跑什么命令 / 怎么解析输出」的纯函数，用运行时 [`Platform`] 分派而非 `#[cfg(target_os)]`
//! → Linux CI 100% 编译 + 跑测三平台逻辑。命令构造与输出解析全是纯函数、mock 一注入即可断言。

#![forbid(unsafe_code)]

use crate::error::SystemIntegrationError;
use crate::exec::{Command, CommandRunner};
use polaris_helper_proto::Platform;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// post-flight 探测的固定公网目的 IP（设计 D4）。route 查询是路由表读、不发包 → 选公网 IP 让
/// sing-box auto_route 的 /1 半程路由按最长前缀命中，避开 `default`(0/0) 的物理网卡假阴性。
pub const PROBE_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));

/// 单条路由查询命令硬超时（对齐 `dns_ops::DNS_CMD_TIMEOUT`）。
pub const ROUTE_CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// 平台路由出口探测抽象。mac/linux/win 真实现（命令 + 解析），Other 收敛为「查不到」（`Ok(None)`）。
pub trait SystemRouteOps {
    /// 查目的 IP 的出口接口名。
    ///
    /// - `Ok(Some(iface))`：路由表命中，出口接口 = `iface`（macOS `utunN` / linux `tunX`/`eth0` /
    ///   win `InterfaceAlias`）。
    /// - `Ok(None)`：命令成功但输出里没有可识别的接口（无匹配路由 / 解析不出）——**非错误**，判定层按
    ///   「不可断言」处理，绝不据此硬闸（避免 §4.7 假阳性）。
    /// - `Err`：命令执行失败（spawn/超时/非零退出）。
    fn exit_interface_for(&self, ip: IpAddr) -> Result<Option<String>, SystemIntegrationError>;
}

/// [`SystemRouteOps`] 的生产实现（运行时 [`Platform`] 分派 + [`CommandRunner`] 下发；零 cfg）。
pub struct SystemRouteOpsImpl<R: CommandRunner> {
    runner: R,
    platform: Platform,
    /// Windows `powershell.exe` 绝对路径（规避 PATH 缺 System32；见 `exec::system32`）。
    powershell_exe: String,
}

impl<R: CommandRunner> SystemRouteOpsImpl<R> {
    /// 生产构造：平台取本机。
    pub fn new(runner: R) -> Self {
        Self::with_platform(runner, Platform::current())
    }

    /// 指定平台构造（测试用：Linux 上断言 mac/win 的 argv 与解析）。
    pub fn with_platform(runner: R, platform: Platform) -> Self {
        Self {
            runner,
            platform,
            powershell_exe: crate::exec::system32_from_env(
                "WindowsPowerShell\\v1.0\\powershell.exe",
            ),
        }
    }

    fn run(&self, cmd: &Command) -> Result<crate::exec::CommandOutput, SystemIntegrationError> {
        self.runner
            .run(cmd, ROUTE_CMD_TIMEOUT)
            .map_err(SystemIntegrationError::route)
    }
}

impl<R: CommandRunner> SystemRouteOps for SystemRouteOpsImpl<R> {
    fn exit_interface_for(&self, ip: IpAddr) -> Result<Option<String>, SystemIntegrationError> {
        let ip = ip.to_string();
        match self.platform {
            Platform::Mac => {
                // `-n` 抑制反向 DNS（不发包、更快）；查具体公网 IP 而非 default（§4.5 半程路由陷阱）。
                let args = if ip.contains(':') {
                    vec!["-n".to_owned(), "get".to_owned(), "-inet6".to_owned(), ip]
                } else {
                    vec!["-n".to_owned(), "get".to_owned(), ip]
                };
                let out = self.run(&Command::new("route", args))?;
                Ok(parse_mac_route_get_interface(&out.stdout))
            }
            Platform::Linux => {
                let args = if ip.contains(':') {
                    vec!["-6".to_owned(), "route".to_owned(), "get".to_owned(), ip]
                } else {
                    vec!["route".to_owned(), "get".to_owned(), ip]
                };
                let out = self.run(&Command::new("ip", args))?;
                Ok(parse_linux_ip_route_dev(&out.stdout))
            }
            Platform::Win => {
                // Find-NetRoute 让内核代算「到该目的会用哪个接口」（等价 route get），Format-List 收窄输出。
                let cmd = Command::new(
                    &self.powershell_exe,
                    [
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        &format!(
                            "Find-NetRoute -RemoteIPAddress {ip} | Format-List InterfaceAlias"
                        ),
                    ],
                );
                Ok(parse_win_find_netroute_alias(&self.run(&cmd)?.stdout))
            }
            // Android：**没有可用的逐目的路由查询**，且这条腿曾经是一次阻断级缺陷的一半。
            //
            // 事实面：非 root 应用读不到内核路由表的有效答案（`ip route get` 在应用沙箱里拿不到
            // 跨网络的真实出口），而「到这个目的会走哪张网卡」在 Android 上本就由系统持有 ——
            // 对应物是 libbox `PlatformInterface.autoDetectInterfaceControl` +
            // `DefaultNetworkMonitor.updateDefaultInterface`（K2 已接线），不经本 trait。
            //
            // 血证（K6b，2026-09-04 模拟器实测）：本臂恒 `Ok(None)` ⇒ 逐目的绑定规划恒产出
            // 「0 绑定 / 全部候选未决」⇒ 重规划判据退回整份网卡指纹比对 ⇒ 比到起核自己建出来的
            // tun0（link-local 每代都换）⇒ 判据恒真 ⇒ **300 秒内起核 69 次**。修在取材口
            // （`runtime/route_binding.rs::runtime_binding_planning_supported`），本臂保持诚实的
            // 「查不到」不变 —— 它没有说谎，说谎的是把「查不到」读成「事实变了」的那条判据。
            Platform::Android => Ok(None),
            // iOS：同答 `Ok(None)`，**独立成臂**——与 Android 同答不同因，且 K6b 那条血证在 iOS 上
            // 的传导路径不同，值得单独写一次。
            //
            // 事实面：iOS 应用与 NE 扩展都没有逐目的路由查询面（无 `route`/`ip` 可执行、
            // `sysctl` 路由表读取对第三方应用不开放）。对应物同样由系统持有：NE 侧是
            // `NWPathMonitor` / `NEPacketTunnelProvider` 的 `reasserting` 与系统自动把扩展自身
            // 出站排除出隧道 —— 后者意味着 iOS 上连「核出站要不要 protect」这个问题都不存在。
            //
            // K6b 传导路径的差别：那次事故的下半程是「网卡指纹比对比到核自建的 tun0」。iOS 上核
            // 在 NE 扩展进程内，`list_network_interfaces` 这条腿本身在该平台的形态尚未落地，
            // 故本臂今天连**不可达**都算不上——它是「等着被接线时答案已经在这里」。
            //
            // **未验证**：上述为平台 API 面推论，本仓构不出 iOS 产物，未经真机取证。
            Platform::Ios => Ok(None),
            // 未知平台：无对应路由工具 → 查不到（判定层按「不可断言」不闸）。
            Platform::Other => Ok(None),
        }
    }
}

// ── 纯解析函数（跨平台可单测；Linux CI 上断言三平台输出解析）──

/// 解析 macOS `route -n get <ip>` 的 `interface: utunN` 行。无该行 → None。
pub fn parse_mac_route_get_interface(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("interface:")?;
        let iface = rest.trim();
        (!iface.is_empty()).then(|| iface.to_string())
    })
}

/// 解析 Linux `ip route get <ip>` 的 `dev <iface>` token。无该 token → None。
///
/// 兼容两形态：`1.1.1.1 via <gw> dev eth0 src ...` 与 `1.1.1.1 dev tun0 src ...`。
pub fn parse_linux_ip_route_dev(stdout: &str) -> Option<String> {
    let mut tokens = stdout.split_whitespace();
    while let Some(tok) = tokens.next() {
        if tok == "dev" {
            if let Some(dev) = tokens.next() {
                if !dev.is_empty() {
                    return Some(dev.to_string());
                }
            }
        }
    }
    None
}

/// 解析 Windows `Find-NetRoute ... | Format-List InterfaceAlias` 的首个 `InterfaceAlias : <name>` 值。
///
/// Find-NetRoute 返回源地址 + 路由两个对象、二者的 InterfaceAlias 均指向出口接口，取首个即可。
pub fn parse_win_find_netroute_alias(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim() != "InterfaceAlias" {
            return None;
        }
        let alias = value.trim();
        (!alias.is_empty()).then(|| alias.to_string())
    })
}

// ── baseline 差分判定（方向①后验的核心；纯逻辑，与探测手段解耦）──

/// 出口接口是否已从 baseline 切走（成功接管的判据）。
///
/// - `current = Some(c)` 且 `c != baseline` → `true`（含 baseline 不可读的情形：任何可读的新出口都算切走，
///   偏向「不闸」这一安全方向——见 [`ExitCaptureOutcome`]）。
/// - `current = Some(c)` 且 `c == baseline` → `false`（出口未动）。
/// - `current = None`（探测不可读） → `false`（无法断言切走）。
fn exit_changed<T: PartialEq>(baseline: &Option<T>, current: &Option<T>) -> bool {
    match current {
        Some(c) => baseline.as_ref() != Some(c),
        None => false,
    }
}

/// post-flight 出口归属判定结果。
///
/// `T` = 调用方使用的**接口身份类型**。本判定只做「同一探测通道的前后两次观测是否相等」，
/// 不解释身份的内部表示；调用方据此可以传别名字符串，也可以传别名/索引合一的结构化身份，
/// 而不必先把索引编码成字符串再假装它是个名字。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExitCaptureOutcome<T> {
    /// 出口已从 baseline 切走 → 成功接管（macOS 不可命名 → 差分即判据）。
    Captured { interface: Option<T> },
    /// grace 耗尽、出口仍等于（可读的）baseline → **未夺到路由**，判定层据此硬闸。
    NotCaptured { baseline: T, last: T },
    /// baseline 或末次探测不可读 → 无法断言 → **不闸**（caveat，设计 §4.7 避免假阳性）。
    Indeterminate,
}

/// 在 grace 窗口内轮询出口接口，按 baseline 差分判定是否夺到路由（方向①后验）。
///
/// - `baseline`：起核**前**（我方 utun 上线前）的出口接口快照。
/// - `max_polls`：grace 窗口内的探测次数（≥1）。任一次探到出口切走即**立即** `Captured`（早退，接管越快
///   越早点亮）；仅全部轮次都未切走才落终判。
/// - `probe`：读当前出口接口（生产 = [`SystemRouteOps::exit_interface_for`]；测试 = 队列）。探测 `Err`
///   按 `None`（best-effort，绝不据探测失败误判为夺到）。
/// - `sleep_between_polls`：相邻两次探测之间的等待（生产 = `thread::sleep`；测试 = 计数）。**末次探测后
///   不再 sleep**（省一个 grace 间隔的白等）。
///
/// 终判：全程未切走时，若 baseline 与末次探测**都可读且相等** → [`ExitCaptureOutcome::NotCaptured`]；
/// 否则 [`ExitCaptureOutcome::Indeterminate`]（不可读 → 不闸）。
pub fn verify_exit_captured<T, P, S>(
    baseline: Option<T>,
    max_polls: usize,
    mut probe: P,
    mut sleep_between_polls: S,
) -> ExitCaptureOutcome<T>
where
    T: PartialEq,
    P: FnMut() -> Result<Option<T>, SystemIntegrationError>,
    S: FnMut(),
{
    let polls = max_polls.max(1);
    let mut last: Option<T> = None;
    for i in 0..polls {
        let current = probe().unwrap_or(None);
        if exit_changed(&baseline, &current) {
            return ExitCaptureOutcome::Captured { interface: current };
        }
        last = current;
        if i + 1 < polls {
            sleep_between_polls();
        }
    }
    match (baseline, last) {
        // 都可读且相等（出口自始至终没动）→ 未夺到路由，硬闸。
        (Some(b), Some(l)) if b == l => ExitCaptureOutcome::NotCaptured {
            baseline: b,
            last: l,
        },
        // baseline 不可读 / 末次探测不可读 / 二者不等但又没触发早退 → 不可断言，不闸。
        _ => ExitCaptureOutcome::Indeterminate,
    }
}

#[cfg(test)]
mod tests;

/// Immutable facts captured before the App's TUN exists. This is deliberately
/// narrower than the cold planner: IPv4, one physical default, canonical policy,
/// and an actual resolver on that same link. It is not a user-selected bypass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxPhysicalRouteFacts {
    pub interface: String,
    pub ifindex: u32,
    pub source: std::net::Ipv4Addr,
    pub resolver: std::net::Ipv4Addr,
    external_routes: Vec<serde_json::Value>,
    all_routes: Vec<serde_json::Value>,
    dns: PhysicalDnsFacts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PhysicalDnsFacts {
    Resolved(Vec<String>),
    ResolvConf(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxProtectedRouteView {
    managed_table: u32,
    rules: Vec<serde_json::Value>,
    managed_routes: Vec<serde_json::Value>,
}

fn protected_error(message: impl Into<String>) -> SystemIntegrationError {
    SystemIntegrationError::route(message.into())
}

fn json_array(raw: &str) -> Result<Vec<serde_json::Value>, SystemIntegrationError> {
    if raw.len() > 256 * 1024 {
        return Err(protected_error(
            "protected route observation exceeds budget",
        ));
    }
    let value: Vec<serde_json::Value> =
        serde_json::from_str(raw).map_err(|_| protected_error("invalid protected route JSON"))?;
    if value.len() > 1024 || value.iter().any(|v| !v.is_object()) {
        return Err(protected_error("invalid protected route inventory"));
    }
    Ok(value)
}

fn canonical_values(mut values: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    values.sort_by_key(serde_json::Value::to_string);
    values
}

fn table_number(value: Option<&serde_json::Value>) -> Option<u32> {
    match value? {
        serde_json::Value::String(s) => match s.as_str() {
            "local" => Some(255),
            "main" => Some(254),
            "default" => Some(253),
            _ => s.parse().ok(),
        },
        serde_json::Value::Number(n) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
        _ => None,
    }
}

fn canonical_policy(rule: &serde_json::Value) -> bool {
    let Some(object) = rule.as_object() else {
        return false;
    };
    if object
        .keys()
        .any(|key| !["priority", "src", "table", "protocol"].contains(&key.as_str()))
    {
        return false;
    }
    if rule.get("src").and_then(serde_json::Value::as_str) != Some("all") {
        return false;
    }
    matches!(
        (
            rule.get("priority").and_then(serde_json::Value::as_u64),
            table_number(rule.get("table"))
        ),
        (Some(0), Some(255)) | (Some(32766), Some(254)) | (Some(32767), Some(253))
    )
}

fn canonical_policy_inventory(rules: &[serde_json::Value]) -> bool {
    let priorities: std::collections::BTreeSet<_> = rules
        .iter()
        .filter(|r| canonical_policy(r))
        .filter_map(|r| r["priority"].as_u64())
        .collect();
    priorities == std::collections::BTreeSet::from([0, 32766, 32767])
        && rules.iter().filter(|r| canonical_policy(r)).count() == 3
}

fn protected_policy(rule: &serde_json::Value, physical: &str, tun: &str, table: u32) -> bool {
    if canonical_policy(rule) {
        return true;
    }
    let Some(object) = rule.as_object() else {
        return false;
    };
    // UID, source-port and transport selectors would make an App UDP probe an
    // invalid proxy for the Core dialer. Unknown/new kernel selectors fail closed.
    let allowed = [
        "priority",
        "src",
        "dst",
        "table",
        "protocol",
        "iif",
        "oif",
        "fwmark",
        "fwmask",
        "goto",
        "suppress_prefixlength",
        "not",
        "dport",
        "action",
    ];
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return false;
    }
    if rule
        .get("priority")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        return false;
    }
    if let Some(value) = rule.get("table") {
        if ![Some(254), Some(255), Some(253), Some(table)].contains(&table_number(Some(value))) {
            return false;
        }
    } else if rule
        .get("goto")
        .and_then(serde_json::Value::as_u64)
        .is_none()
    {
        return false;
    }
    for key in ["iif", "oif"] {
        if let Some(value) = rule.get(key) {
            if !matches!(value.as_str(), Some(name) if name == physical || name == tun || name == "lo")
            {
                return false;
            }
        }
    }
    if let Some(port) = rule.get("dport") {
        if !(port.as_u64() == Some(53) || matches!(port.as_str(), Some("53" | "53-53"))) {
            return false;
        }
    }
    if let Some(action) = rule.get("action") {
        if !matches!(action.as_str(), Some("to_tbl" | "goto" | "unicast")) {
            return false;
        }
    }
    true
}

fn usable_physical_v4(ip: std::net::Ipv4Addr) -> bool {
    let octets = ip.octets();
    !ip.is_unspecified()
        && !ip.is_loopback()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && !(octets[0] == 169 && octets[1] == 254)
        && !(octets[0] == 198 && matches!(octets[1], 18 | 19))
}

fn dns_link(line: &str) -> Option<(&str, &str)> {
    let start = line.find('(')? + 1;
    let end = line[start..].find(')')? + start;
    let rest = line[end + 1..].trim().strip_prefix(':')?.trim();
    Some((&line[start..end], rest))
}

fn resolved_evidence(
    raw: [&str; 3],
    physical: &str,
    ignore: Option<&str>,
) -> Result<(Vec<String>, std::net::Ipv4Addr), SystemIntegrationError> {
    let mut evidence = Vec::new();
    let mut resolver = None;
    let mut default_seen = false;
    for (kind, text) in raw.into_iter().enumerate() {
        if text.len() > 64 * 1024 {
            return Err(protected_error(
                "physical resolver inventory exceeds budget",
            ));
        }
        for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
            if line.starts_with("Global:") {
                if !line.trim_start_matches("Global:").trim().is_empty()
                    && !(kind == 2 && line.trim_start_matches("Global:").trim() == "no")
                {
                    return Err(protected_error(
                        "global/split resolver policy is not captured",
                    ));
                }
            } else {
                let (link, payload) = dns_link(line)
                    .ok_or_else(|| protected_error("unknown per-link resolver format"))?;
                if Some(link) == ignore {
                    continue;
                }
                if link != physical && !payload.is_empty() && !(kind == 2 && payload == "no") {
                    return Err(protected_error("multiple or split physical resolver links"));
                }
                if link == physical {
                    if kind == 0 {
                        let values: Vec<_> = payload.split_whitespace().collect();
                        if values.len() != 1 {
                            return Err(protected_error("physical resolver is not unique"));
                        }
                        let ip: std::net::Ipv4Addr = values[0]
                            .parse()
                            .map_err(|_| protected_error("physical resolver is not IPv4"))?;
                        if !usable_physical_v4(ip)
                            || values[0] == polaris_helper_proto::linux_dns::CONTROLLED_DNS_IP
                        {
                            return Err(protected_error(
                                "physical resolver is local, FakeIP or controlled TUN DNS",
                            ));
                        }
                        resolver = Some(ip);
                    }
                    if kind == 2 {
                        default_seen = payload == "yes";
                    }
                }
            }
            evidence.push(format!("{kind}:{line}"));
        }
    }
    evidence.sort();
    if !default_seen {
        return Err(protected_error(
            "physical resolver link is not the real default",
        ));
    }
    Ok((
        evidence,
        resolver.ok_or_else(|| protected_error("physical resolver is absent"))?,
    ))
}

impl<R: CommandRunner> SystemRouteOpsImpl<R> {
    fn protected_json(
        &self,
        args: &[&str],
    ) -> Result<Vec<serde_json::Value>, SystemIntegrationError> {
        json_array(&self.run(&Command::new("ip", args.iter().copied()))?.stdout)
    }
    fn physical_dns(
        &self,
        interface: &str,
        ignore: Option<&str>,
        previous: Option<&PhysicalDnsFacts>,
    ) -> Result<(PhysicalDnsFacts, std::net::Ipv4Addr), SystemIntegrationError> {
        if !matches!(previous, Some(PhysicalDnsFacts::ResolvConf(_))) {
            match self.run(&Command::new("resolvectl", ["dns"])) {
                Ok(dns) => {
                    let domains = self.run(&Command::new("resolvectl", ["domain"]))?;
                    let defaults = self.run(&Command::new("resolvectl", ["default-route"]))?;
                    let (evidence, resolver) = resolved_evidence(
                        [&dns.stdout, &domains.stdout, &defaults.stdout],
                        interface,
                        ignore,
                    )?;
                    return Ok((PhysicalDnsFacts::Resolved(evidence), resolver));
                }
                Err(error) if previous.is_some() => return Err(error),
                Err(_) => {}
            }
        }
        let raw = self.run(&Command::new("cat", ["/etc/resolv.conf"]))?.stdout;
        if raw.len() > 64 * 1024 {
            return Err(protected_error("resolv.conf exceeds budget"));
        }
        for line in raw.lines() {
            let mut fields = line.split('#').next().unwrap_or("").split_whitespace();
            match fields.next() {
                None | Some("search" | "domain") => {}
                Some("nameserver") => {
                    if fields.next().is_none() || fields.next().is_some() {
                        return Err(protected_error("ambiguous direct resolver record"));
                    }
                }
                _ => {
                    return Err(protected_error(
                        "direct resolver has unsupported transport/options",
                    ))
                }
            }
        }
        let servers: Vec<_> = raw
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('#').next().unwrap_or("").split_whitespace();
                (fields.next() == Some("nameserver"))
                    .then(|| fields.next().unwrap_or("").to_owned())
            })
            .collect();
        if servers.len() != 1 {
            return Err(protected_error("direct resolver is not unique"));
        }
        let resolver = servers[0]
            .parse()
            .map_err(|_| protected_error("direct resolver is not IPv4"))?;
        if !usable_physical_v4(resolver)
            || servers[0] == polaris_helper_proto::linux_dns::CONTROLLED_DNS_IP
        {
            return Err(protected_error("direct resolver is a stub or TUN resolver"));
        }
        Ok((PhysicalDnsFacts::ResolvConf(raw), resolver))
    }
    fn physical_address(
        &self,
        interface: &str,
    ) -> Result<(u32, std::net::Ipv4Addr), SystemIntegrationError> {
        let links = self.protected_json(&["-j", "-4", "address", "show", "dev", interface])?;
        if links.len() != 1 || links[0]["ifname"].as_str() != Some(interface) {
            return Err(protected_error(
                "physical interface observation is ambiguous",
            ));
        }
        let link = &links[0];
        let flags = link["flags"]
            .as_array()
            .ok_or_else(|| protected_error("physical link flags absent"))?;
        if !flags.iter().any(|v| v.as_str() == Some("UP"))
            || link["operstate"].as_str() != Some("UP")
        {
            return Err(protected_error("physical interface is not up"));
        }
        let index = link["ifindex"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n != 0)
            .ok_or_else(|| protected_error("physical ifindex absent"))?;
        let addresses: Vec<std::net::Ipv4Addr> = link["addr_info"]
            .as_array()
            .ok_or_else(|| protected_error("physical IPv4 addresses absent"))?
            .iter()
            .filter(|v| {
                v["family"].as_str() == Some("inet") && v["scope"].as_str() == Some("global")
            })
            .filter_map(|v| v["local"].as_str().and_then(|s| s.parse().ok()))
            .collect();
        if addresses.len() != 1 || !usable_physical_v4(addresses[0]) {
            return Err(protected_error("physical IPv4 source is ambiguous"));
        }
        Ok((index, addresses[0]))
    }
    /// Must run before our TUN route/DNS takeover, never while TUN is active.
    pub fn capture_linux_physical_route(
        &self,
    ) -> Result<LinuxPhysicalRouteFacts, SystemIntegrationError> {
        if self.platform != Platform::Linux {
            return Err(protected_error(
                "protected physical route source is Linux-only",
            ));
        }
        let rules = self.protected_json(&["-j", "-4", "rule", "show"])?;
        if rules.len() != 3 || !canonical_policy_inventory(&rules) {
            return Err(protected_error("pre-TUN route policy is not canonical"));
        }
        let routes = self.protected_json(&["-j", "-4", "route", "show", "table", "main"])?;
        let defaults: Vec<_> = routes
            .iter()
            .filter(|v| v["dst"].as_str() == Some("default"))
            .collect();
        if defaults.len() != 1 {
            return Err(protected_error("physical default route is not unique"));
        }
        let interface = defaults[0]["dev"]
            .as_str()
            .filter(|name| {
                !name.is_empty()
                    && name.len() < 16
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b))
            })
            .ok_or_else(|| protected_error("physical default interface absent"))?
            .to_owned();
        if routes.iter().any(|v| {
            v["dev"].as_str() != Some(interface.as_str())
                || v.get("multipath").is_some()
                || v.get("nhid").is_some()
                || v.get("expires").is_some()
        }) {
            return Err(protected_error(
                "pre-TUN routes require an unsupported physical policy",
            ));
        }
        let (ifindex, source) = self.physical_address(&interface)?;
        let (dns, resolver) = self.physical_dns(&interface, None, None)?;
        self.verify_linux_bound_route(&interface, source, resolver, 53, "udp")?;
        let all_routes = self.protected_json(&["-j", "-4", "route", "show", "table", "all"])?;
        Ok(LinuxPhysicalRouteFacts {
            interface,
            ifindex,
            source,
            resolver,
            external_routes: canonical_values(routes),
            all_routes: canonical_values(all_routes),
            dns,
        })
    }
    /// Observe the full relevant live policy without accepting our TUN as a
    /// physical exit. Only the actual managed table may be added to canonical
    /// policy. All original non-TUN routes and resolver facts must be unchanged.
    pub fn observe_linux_protected_route(
        &self,
        original: &LinuxPhysicalRouteFacts,
        tun: &str,
    ) -> Result<LinuxProtectedRouteView, SystemIntegrationError> {
        if self.platform != Platform::Linux || tun == original.interface {
            return Err(protected_error("invalid managed TUN route identity"));
        }
        let (index, source) = self.physical_address(&original.interface)?;
        if index != original.ifindex || source != original.source {
            return Err(protected_error("physical interface/source changed"));
        }
        let routes = self.protected_json(&["-j", "-4", "route", "show", "table", "main"])?;
        let external = canonical_values(
            routes
                .into_iter()
                .filter(|v| v["dev"].as_str() != Some(tun))
                .collect(),
        );
        if external != original.external_routes {
            return Err(protected_error("external route inventory changed"));
        }
        let all = self.protected_json(&["-j", "-4", "route", "show", "table", "all"])?;
        let external_all = canonical_values(
            all.iter()
                .filter(|v| v["dev"].as_str() != Some(tun))
                .cloned()
                .collect(),
        );
        if external_all != original.all_routes {
            return Err(protected_error("external route tables changed"));
        }
        let managed: Vec<_> = all
            .into_iter()
            .filter(|v| v["dev"].as_str() == Some(tun))
            .collect();
        let tables: std::collections::BTreeSet<_> = managed
            .iter()
            .filter_map(|v| table_number(v.get("table")))
            .filter(|t| *t >= 256)
            .collect();
        if tables.len() != 1 {
            return Err(protected_error("actual managed route table is not unique"));
        }
        let table = *tables
            .first()
            .ok_or_else(|| protected_error("actual managed route table absent"))?;
        if original
            .all_routes
            .iter()
            .any(|v| table_number(v.get("table")) == Some(table))
        {
            return Err(protected_error("managed table existed before TUN"));
        }
        if managed.is_empty()
            || managed
                .iter()
                .any(|v| v.get("multipath").is_some() || v.get("nhid").is_some())
        {
            return Err(protected_error("managed route table has an unknown exit"));
        }
        let rules = self.protected_json(&["-j", "-4", "rule", "show"])?;
        if !canonical_policy_inventory(&rules)
            || !rules
                .iter()
                .all(|v| protected_policy(v, &original.interface, tun, table))
        {
            return Err(protected_error(
                "live route policy contains unsupported selectors or owners",
            ));
        }
        let (dns, resolver) =
            self.physical_dns(&original.interface, Some(tun), Some(&original.dns))?;
        if dns != original.dns || resolver != original.resolver {
            return Err(protected_error("external resolver policy changed"));
        }
        Ok(LinuxProtectedRouteView {
            managed_table: table,
            rules: canonical_values(rules),
            managed_routes: canonical_values(managed),
        })
    }
    /// Match a real mark-zero, interface-bound socket. UID/sport-dependent rules
    /// are rejected above, so this result can also describe the Core process.
    pub fn verify_linux_bound_route(
        &self,
        interface: &str,
        source: std::net::Ipv4Addr,
        target: std::net::Ipv4Addr,
        port: u16,
        transport: &str,
    ) -> Result<(), SystemIntegrationError> {
        if !usable_physical_v4(target) || !matches!(transport, "udp" | "tcp") || port == 0 {
            return Err(protected_error("unsupported protected route target"));
        }
        let values = self.protected_json(&[
            "-j",
            "-4",
            "route",
            "get",
            &target.to_string(),
            "from",
            &source.to_string(),
            "oif",
            interface,
            "mark",
            "0",
            "ipproto",
            transport,
            "dport",
            &port.to_string(),
        ])?;
        if values.len() != 1 {
            return Err(protected_error("bound route result is ambiguous"));
        }
        let route = &values[0];
        let observed_source = route
            .get("prefsrc")
            .or_else(|| route.get("src"))
            .or_else(|| route.get("from"))
            .and_then(serde_json::Value::as_str);
        if route["dev"].as_str() != Some(interface)
            || observed_source != Some(source.to_string().as_str())
            || route
                .get("table")
                .is_some_and(|v| table_number(Some(v)) != Some(254))
            || route.get("multipath").is_some()
            || route.get("nhid").is_some()
        {
            return Err(protected_error(
                "bound socket route is not the retained physical exit",
            ));
        }
        Ok(())
    }
}
