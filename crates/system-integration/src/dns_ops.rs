//! 系统 DNS 平台操作抽象 + 接管/还原/重灌编排。
//!
//! 1:1 移植自 上游 `SystemDnsManager.ts` 基类 `SystemDnsBase`：
//! - [`SystemDnsOps`] trait：物理网卡 list/read/apply/读生效解析器（mac 真实实现 + Win/Linux no-op）。
//!   Linux 生产接管不走本抽象，而走 [`crate::linux_resolved`] 的 TUN per-link 生命周期。
//! - [`DnsMarker`]：marker 读写（FS trait 注入，纯逻辑）。
//! - [`SystemDnsController`]：setDns / restoreDns / reconcileDns / restoreDnsSync 编排。
//!
//! 关键不变量（对齐 Polaris）：
//! - setDns **best-effort**：失败仅告警 + 回滚还原，绝不抛（DNS 治理降级不阻断 TUN 启动）。
//! - marker 前置写入（intent）：set 期间崩溃也留 marker，下次启动据此还原。
//! - 防自指：再次接管时若当前已是受控 IP，回退既有 marker 的真实原始。
//! - reconcile：仅 marker 在才动手（接管未激活绝不写系统）；只 apply 未受控服务（幂等）。

#![forbid(unsafe_code)]

use crate::dns::{
    compute_original_to_save, controlled_tun_dns_ip, extract_ipv4s, is_controlled,
    is_controlled_dns_ip_valid, mac_set_dns_args, parse_mac_get_dns_servers,
    parse_scutil_nameservers, parse_win_interfaces, parse_win_show_dns_servers,
    pick_lan_resolver_ip, SystemDnsMarker,
};
use crate::error::SystemIntegrationError;
use crate::exec::{Command, CommandRunner};
use crate::proxy::MarkerFs;
use crate::proxy_ops::{retry_op, RetryConfig, TakeoverOutcome, TakeoverReceipt};
use polaris_helper_proto::Platform;
use std::collections::BTreeMap;
use std::time::Duration;

/// 系统 DNS marker 读写（FS trait 注入）。
/// 上游 `SystemDnsBase` 的 marker IO 部分。
pub struct DnsMarker<Fs: MarkerFs> {
    fs: Fs,
    path: String,
    controlled_ip: String,
}

impl<Fs: MarkerFs> DnsMarker<Fs> {
    pub fn new(fs: Fs, path: impl Into<String>) -> Self {
        Self {
            fs,
            path: path.into(),
            controlled_ip: controlled_tun_dns_ip().to_string(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Durable intent must exist before any OS mutation.
    pub fn write(&self, original: &BTreeMap<String, Vec<String>>) -> bool {
        let marker = SystemDnsMarker {
            controlled_ip: self.controlled_ip.clone(),
            original: original.clone(),
            at: now_ms(),
        };
        let Ok(json) = serde_json::to_string(&marker) else {
            return false;
        };
        self.fs.write_marker(&self.path, &json).is_ok()
    }

    pub fn clear(&self) -> bool {
        self.fs.remove_marker(&self.path).is_ok()
    }

    fn read_checked(&self) -> std::io::Result<Option<SystemDnsMarker>> {
        self.read_raw_checked().map(|(_, marker)| marker)
    }

    fn read_raw_checked(&self) -> std::io::Result<(Option<String>, Option<SystemDnsMarker>)> {
        let Some(raw) = self.fs.read_marker_checked(&self.path)? else {
            return Ok((None, None));
        };
        let marker: SystemDnsMarker = serde_json::from_str(&raw)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if !is_controlled_dns_ip_valid(&marker.controlled_ip) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid DNS marker",
            ));
        }
        Ok((Some(raw), Some(marker)))
    }

    /// 读 marker；不存在 / 损坏 / 结构非法 → None。
    pub fn read(&self) -> Option<SystemDnsMarker> {
        let raw = self.fs.read_marker(&self.path)?;
        let m: SystemDnsMarker = serde_json::from_str(&raw).ok()?;
        if !is_controlled_dns_ip_valid(&m.controlled_ip) {
            return None;
        }
        Some(m)
    }

    pub fn exists(&self) -> bool {
        self.read_checked().map_or(true, |marker| marker.is_some())
    }
}

/// 物理网卡 DNS 操作抽象。mac 真实实现（networksetup），Win/Linux 收敛为 no-op；Linux 的
/// `systemd-resolved` TUN per-link 接管由 [`crate::linux_resolved`] 独立负责。
/// 语义对齐 上游 `SystemDnsBase` 的 abstract 方法。
pub trait SystemDnsOps {
    /// **本平台是否接管系统 DNS（写路径）**。mac=true；win/linux=false。
    ///
    /// ## 为什么是 trait 方法而不是「让 apply_dns 静默 no-op」
    ///
    /// 上游把 no-op 做在**平台子类覆写 `setDns` 整个方法**上（`WindowsSystemDns.setDns` /
    /// `LinuxSystemDns.setDns`），**不是**让 `applyDns` 空转 —— 这个区别是**血证**：
    ///
    /// - **Windows**（上游 2026-06-17 真机实证收口）：① sing-box TUN `strict_route`(WFP) 已在路由层
    ///   把所有 `:53` 强制逼进 TUN → 不需要也不应再改系统 DNS 设置项；② `netsh set dnsservers` **需管理员**，
    ///   GUI 非提权 → 真机**每次 ACCESS DENIED**。而 `set_dns` 是**先写 marker 再 apply** →
    ///   apply 必失败 → **marker 卡死** → 每次启动反复「还原 netsh 失败、保留 marker」刷错误日志。
    /// - **Linux**：发行版差异大（systemd-resolved / resolv.conf / NetworkManager），写入风险高，
    ///   且 TUN 由 sing-box `auto_route` 自身处理 DNS。
    ///
    /// 故若仅让 `apply_dns` 空转、`set_dns` 照常写 marker，**会精确复现上游修掉的 marker 卡死 bug**。
    /// [`SystemDnsController::set_dns`] 据本方法**在写 marker 之前**早退。
    ///
    /// **读路径不受影响**：`list_targets` / `read_dns` / `read_effective_resolvers` 在 win 上仍是真实现
    /// （`netsh show` 非提权可跑），供方案B [`SystemDnsController::get_lan_resolver_for_dns`] 用。
    fn takeover_supported(&self) -> bool;

    /// 列出应接管的网络服务/接口名。
    fn list_targets(&self) -> Result<Vec<String>, crate::error::SystemIntegrationError>;
    /// 读某服务/接口的当前 DNS（`[] = DHCP/自动`）。
    fn read_dns(&self, target: &str) -> Result<Vec<String>, crate::error::SystemIntegrationError>;
    /// 设某服务/接口 DNS（`[] → DHCP/Empty 还原`）。
    fn apply_dns(
        &self,
        target: &str,
        ips: &[String],
    ) -> Result<(), crate::error::SystemIntegrationError>;
    /// 读生效解析器候选 IP（含 DHCP 下发的；方案B 用）。无法读 → `[]`。
    fn read_effective_resolvers(&self)
        -> Result<Vec<String>, crate::error::SystemIntegrationError>;
}

/// 接管请求/结果辅助。
pub struct DnsTakeover {
    pub targets: Vec<String>,
    pub current: BTreeMap<String, Vec<String>>,
}

/// 单条系统 DNS 命令硬超时（上游 `DNS_CMD_TIMEOUT_MS`）。
pub const DNS_CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// [`SystemDnsOps`] 的生产实现（运行时 [`Platform`] 分派 + [`CommandRunner`] 下发；零 cfg）。
///
/// 平台分界（真差异，保留隔离）：
/// - **mac**：真接管。`networksetup -listallnetworkservices` / `-getdnsservers` / `-setdnsservers`；
///   生效解析器读 `scutil --dns`（含 DHCP 下发的，`-getdnsservers` 对 DHCP 返空拿不到）。
/// - **win**：**写路径 no-op**（`takeover_supported=false`，判据见该方法）；读路径真实现
///   （`netsh interface ipv4 show ...`，非提权可跑）供方案B 用。
/// - **linux**：写路径 no-op；生效解析器读 resolvectl / resolv.conf，接管另走 [`crate::linux_resolved`]。
/// - **android**（2026-09-04 K10 具名）：与 linux 同为全 no-op，但**理由不同**，别读成「继承 linux」：
///   linux 是「这层抽象不合适、真接管在别处」，android 是「这件事整个不存在」——系统解析器不在
///   链路上（DNS 由核在 tun fd 内自理），且非 root 应用无权读写别的网卡的 DNS。因此 android 上
///   `takeover_supported()` 恒 false、控制器在写 marker 前就早退，四个方法的返回值只是纵深防御。
pub struct SystemDnsOpsImpl<R: CommandRunner> {
    runner: R,
    platform: Platform,
    /// Windows `netsh.exe` 绝对路径（规避 PATH 缺 System32）。
    netsh_exe: String,
}

impl<R: CommandRunner> SystemDnsOpsImpl<R> {
    /// 生产构造：平台取本机。
    pub fn new(runner: R) -> Self {
        Self::with_platform(runner, Platform::current())
    }

    /// 指定平台构造（测试用：Linux 上断言 mac/win 的 argv 与解析）。
    pub fn with_platform(runner: R, platform: Platform) -> Self {
        Self {
            runner,
            platform,
            netsh_exe: crate::exec::system32_from_env("netsh.exe"),
        }
    }

    fn run(&self, cmd: &Command) -> Result<crate::exec::CommandOutput, SystemIntegrationError> {
        self.runner
            .run(cmd, DNS_CMD_TIMEOUT)
            .map_err(SystemIntegrationError::dns)
    }

    /// Windows：读单接口 DNS 输出（读路径，show 非提权可跑）。
    fn win_show_dnsservers(&self, iface: &str) -> Result<String, SystemIntegrationError> {
        let cmd = Command::new(
            &self.netsh_exe,
            [
                "interface",
                "ipv4",
                "show",
                "dnsservers",
                &format!("name={iface}"),
            ],
        );
        Ok(self.run(&cmd)?.stdout)
    }
}

impl<R: CommandRunner> SystemDnsOps for SystemDnsOpsImpl<R> {
    fn takeover_supported(&self) -> bool {
        // 仅 mac 接管系统 DNS。win/linux 的判据见 trait 方法 doc（上游真机实证收口）。
        matches!(self.platform, Platform::Mac)
    }

    fn list_targets(&self) -> Result<Vec<String>, SystemIntegrationError> {
        match self.platform {
            // 与系统代理共用**同一个**口径函数（此前是共用同一个「按名字过滤」的解析器，
            // 于是两处一起把 DNS/代理写到了别家 VPN 的网络服务上）。判据与回落见该函数 doc。
            Platform::Mac => crate::proxy_ops::mac_list_manageable_services(|c| self.run(c)),
            Platform::Win => {
                let out = self.run(&Command::new(
                    &self.netsh_exe,
                    ["interface", "ipv4", "show", "interfaces"],
                ))?;
                Ok(parse_win_interfaces(&out.stdout))
            }
            // list_targets。iOS → 空集：没有「网络服务」这个可枚举面（`networksetup`
            // 的对应物不存在，NEHotspot/NetworkExtension 也不暴露服务清单给普通应用）。
            // 空集让上层的逐目标循环一次都不进，与 `takeover_supported=false` 同向。
            Platform::Linux | Platform::Other | Platform::Android | Platform::Ios => Ok(vec![]),
        }
    }

    fn read_dns(&self, target: &str) -> Result<Vec<String>, SystemIntegrationError> {
        match self.platform {
            Platform::Mac => {
                let out = self.run(&Command::new("networksetup", ["-getdnsservers", target]))?;
                Ok(parse_mac_get_dns_servers(&out.stdout))
            }
            Platform::Win => Ok(parse_win_show_dns_servers(
                &self.win_show_dnsservers(target)?,
            )),
            // read_dns。iOS → 空集：没有读某个网络接口 DNS 配置的公开 API。空集在这里是
            // 「读不到」而不是「没有配」——但两者在本函数的唯一消费方（接管前的原值快照）
            // 上等价，因为接管本身已被 `takeover_supported=false` 挡在更前面。
            Platform::Linux | Platform::Other | Platform::Android | Platform::Ios => Ok(vec![]),
        }
    }

    fn apply_dns(&self, target: &str, ips: &[String]) -> Result<(), SystemIntegrationError> {
        match self.platform {
            Platform::Mac => {
                // execFile argv：服务名含空格（如 "USB 10/100/1000 LAN"）也安全，无引号歧义。
                self.run(&Command::new("networksetup", mac_set_dns_args(target, ips)))?;
                Ok(())
            }
            // 写路径 no-op —— 但**正常不可达**：`takeover_supported=false` 已让 set_dns/reconcile_dns
            // 在写 marker 前早退。此处是纵深防御（万一有人绕过控制器直调 ops）。
            // 上游 `winSetDnsCommands` 纯函数保留在 `dns::win_set_dns_commands`（移植真值 + 待
            // Windows 接管解禁时复用），故意不接线。
            // apply_dns。iOS → no-op，与 Win/Linux 同臂且同为**正常不可达**：写系统 DNS 在
            // iOS 上没有任何 API 面（NE 侧的 `NEDNSSettings` 只作用于本隧道，属于配置生成的
            // 领域而不是「接管系统解析器」）。
            Platform::Win
            | Platform::Linux
            | Platform::Other
            | Platform::Android
            | Platform::Ios => Ok(()),
        }
    }

    fn read_effective_resolvers(&self) -> Result<Vec<String>, SystemIntegrationError> {
        match self.platform {
            Platform::Mac => {
                // scutil --dns 反映生效解析器（含 DHCP 下发的）。
                let out = self.run(&Command::new("scutil", ["--dns"]))?;
                Ok(parse_scutil_nameservers(&out.stdout))
            }
            Platform::Win => {
                // 逐**已连接**接口读（与 list_targets 同口径，避免 VMware/Hyper-V/VPN 虚拟网卡的
                // 私网 DNS 抢先被选）。单接口 show 同时含 static 与 dhcp 行 → extract_ipv4s 两者都取。
                let mut all: Vec<String> = Vec::new();
                for iface in self.list_targets()? {
                    // 单接口读失败跳过（上游 per-iface try/catch）。
                    let Ok(stdout) = self.win_show_dnsservers(&iface) else {
                        continue;
                    };
                    for ip in extract_ipv4s(&stdout) {
                        if !all.contains(&ip) {
                            all.push(ip);
                        }
                    }
                }
                Ok(all)
            }
            Platform::Linux => {
                // resolved stub (127.0.0.53) 不是上游；先读各链路真实 DNS，失败再读 resolv.conf。
                let resolved = self.run(&Command::new("resolvectl", ["dns"]));
                let mut ips: Vec<String> = resolved
                    .ok()
                    .map(|out| {
                        out.stdout
                            .split_whitespace()
                            .filter(|v| v.parse::<std::net::IpAddr>().is_ok())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                if pick_lan_resolver_ip(&ips, controlled_tun_dns_ip()).is_none() {
                    if let Ok(out) = self.run(&Command::new("cat", ["/etc/resolv.conf"])) {
                        ips.extend(out.stdout.lines().filter_map(|line| {
                            let mut fields = line.split_whitespace();
                            (fields.next() == Some("nameserver"))
                                .then(|| fields.next())
                                .flatten()
                                .filter(|v| v.parse::<std::net::IpAddr>().is_ok())
                                .map(str::to_string)
                        }));
                    }
                }
                Ok(ips)
            }
            // iOS 第三方应用没有公开的系统生效 DNS 读取接口。
            Platform::Other | Platform::Android | Platform::Ios => Ok(vec![]),
        }
    }
}

// ── set_dns 重试配置（1:1 移植 上游 `SystemDnsManager.ts:214-229` 的 `retry(...)` 块）──
//
// 复用 [`crate::proxy_ops`] 的通用重试原语（`RetryConfig`/`retry_op` 已 `pub(crate)`），不另写一套。
// **仅套 `set_dns` 一处**：上游 `restoreDns`/`reconcileDns` 均无 retry 包裹（`reconcileDns` 逐服务
// best-effort `.catch()` 吞错即放弃，`restoreDns` 单次 apply），故 [`SystemDnsController::restore_dns_inner`]
// 保持原样不套 retry —— 对齐上游这一不对称。

/// `set_dns` 的重试配置：`maxRetries:2, delay:500`（指数退避缺省 true → 退避 500/1000ms），
/// `shouldRetry`= 权限拒绝 / 未授权 → 不重试，其余瞬时抖动 → 重试（`SystemDnsManager.ts:221-226`）。
///
/// 仅 mac 会用到——**唯一真接管平台**（见 [`SystemDnsOps::takeover_supported`]）；win/linux 在
/// [`SystemDnsController::set_dns`] 写 marker 前已早退，走不到这条重试。
const DNS_SET_RETRY: RetryConfig = RetryConfig {
    max_retries: 2,
    delay: Duration::from_millis(500),
    exponential_backoff: true,
    should_retry: dns_set_should_retry,
};

/// `set_dns` 的 `shouldRetry`（上游 `SystemDnsManager.ts:223-225`）：权限拒绝 / 未授权 → 不重试，
/// 其余（`networksetup` 瞬时抖动）→ 重试。**配置**（次数/退避）与 `proxy_ops` 的 mac enable 独立
/// ——对齐「三平台参数不同，勿合并成单一配置」；但**判据词表**共用
/// [`crate::proxy_ops::PERMISSION_DENIED_NEEDLES`]（两份手抄的词表必然漂移，且漏词的后果
/// 在这条路径上更贵：本重试是**持 `dns_controller` 锁**跑的，误判瞬时 → 多 2 次必败重试 + 1.5s 退避
/// 全程占锁）。
///
/// 上游只判 `permission` / `not authorized` 两词 —— 那是 Node 侧文案；Rust 侧把子进程 stderr 原文
/// 归入消息串，macOS `networksetup` 的 `requires admin privileges` 形态一个都不含，故必须补表。
fn dns_set_should_retry(e: &SystemIntegrationError) -> bool {
    !crate::proxy_ops::is_permission_denied(&e.to_string().to_lowercase())
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct DnsEffectMarker {
    operation_id: String,
    phase: String,
    before: BTreeMap<String, Vec<String>>,
    target: BTreeMap<String, Vec<String>>,
}

fn dns_effect_marker(
    raw: &str,
    baseline: &SystemDnsMarker,
) -> Result<Option<DnsEffectMarker>, SystemIntegrationError> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| SystemIntegrationError::dns(e.to_string()))?;
    let Some(effect) = value.get("hostEffect") else {
        return Ok(None);
    };
    let effect: DnsEffectMarker = serde_json::from_value(effect.clone())
        .map_err(|e| SystemIntegrationError::dns(e.to_string()))?;
    if effect.operation_id.trim().is_empty()
        || !matches!(effect.phase.as_str(), "applying" | "owned" | "restoring")
        || effect.before.is_empty()
        || !effect.before.keys().eq(effect.target.keys())
        || !effect.before.keys().eq(baseline.original.keys())
        || effect
            .before
            .values()
            .chain(effect.target.values())
            .flatten()
            .any(|ip| ip.parse::<std::net::IpAddr>().is_err())
        || effect.phase == "owned"
            && effect
                .target
                .values()
                .any(|ips| !is_controlled(ips, &baseline.controlled_ip))
    {
        return Err(SystemIntegrationError::dns(
            "invalid live DNS recovery footprint",
        ));
    }
    Ok(Some(effect))
}

/// Full snapshots are a private App recipe, never an IPC response.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PreparedDnsEffect {
    marker_path: String,
    marker_raw: Option<String>,
    before: BTreeMap<String, Vec<String>>,
    target: BTreeMap<String, Vec<String>>,
    original: BTreeMap<String, Vec<String>>,
    controlled_ip: String,
    enabled: bool,
    physical_lan_resolver: Option<String>,
}

impl PreparedDnsEffect {
    /// Bind the existing App pre-TUN resolver, never a fresh in-TUN probe.
    pub fn with_physical_lan_resolver(mut self, resolver: Option<String>) -> Self {
        self.physical_lan_resolver = resolver;
        self
    }
    pub fn prior_marker_operation_id(&self) -> Option<String> {
        let raw = self.marker_raw.as_ref()?;
        let marker: SystemDnsMarker = serde_json::from_str(raw).ok()?;
        dns_effect_marker(raw, &marker)
            .ok()
            .flatten()
            .map(|effect| effect.operation_id)
    }
    pub fn needs_prior_restore(&self) -> bool {
        self.marker_raw.as_ref().is_some_and(|raw| {
            serde_json::from_str::<SystemDnsMarker>(raw)
                .ok()
                .and_then(|marker| dns_effect_marker(raw, &marker).ok().flatten())
                .is_some_and(|effect| effect.phase != "owned")
        })
    }
    pub fn scope(&self) -> &str {
        &self.marker_path
    }
    pub fn before(&self) -> &BTreeMap<String, Vec<String>> {
        &self.before
    }
    pub fn target(&self) -> &BTreeMap<String, Vec<String>> {
        &self.target
    }
}

/// 系统 DNS 控制器：setDns / restoreDns / reconcileDns / sync 编排。
pub struct SystemDnsController<Ops: SystemDnsOps, Fs: MarkerFs> {
    ops: Ops,
    marker: DnsMarker<Fs>,
    /// 内存中的原始 DNS 快照（restoreDns 用；restoreDnsSync 走 marker 跨会话）。
    original: Option<BTreeMap<String, Vec<String>>>,
    /// `set_dns` 重试退避 sleep（注入便于测试：生产 [`std::thread::sleep`]，测试传 no-op 杜绝真睡；
    /// 对齐 `proxy_ops::SystemProxyOpsImpl` 既有的可注入执行缝风格）。
    sleeper: fn(Duration),
    /// 本次接管**挤掉**的非公网解析器（很可能属于另一个 VPN/组网客户端）。
    ///
    /// 纯运行期观测，不落盘：它描述的是"接管发生时系统上有什么"，重启后要重新观测才有意义。
    /// 空 = 没挤掉任何非公网解析器（也包括"这次没接管"）。
    displaced_resolvers: Vec<String>,
    /// 接管前生效的 LAN DNS（包括 DHCP）；不能混入用于恢复静态设置的 marker.original。
    original_lan_resolver: Option<String>,
}

impl<Ops: SystemDnsOps, Fs: MarkerFs> SystemDnsController<Ops, Fs> {
    pub fn new(ops: Ops, marker: DnsMarker<Fs>) -> Self {
        Self {
            ops,
            marker,
            original: None,
            sleeper: std::thread::sleep,
            displaced_resolvers: Vec::new(),
            original_lan_resolver: None,
        }
    }

    /// 测试：换成 no-op sleeper（重试路径不真睡）。
    #[cfg(test)]
    fn with_noop_sleeper(mut self) -> Self {
        self.sleeper = |_| {};
        self
    }

    /// 受控 DNS IP。
    pub fn controlled_ip(&self) -> &str {
        &self.marker.controlled_ip
    }

    /// 挑接管前的非公网 LAN 解析器，排除受控 IP。
    /// 先读本次接管的生效快照，再读恢复 marker，最后读当前生效解析器。
    pub fn get_lan_resolver_for_dns(&self) -> Option<String> {
        if let Some(ip) = &self.original_lan_resolver {
            return Some(ip.clone());
        }
        let marker = self.marker.read();
        let original: Vec<String> = marker
            .as_ref()
            .map(|m| m.original.values().flatten().cloned().collect())
            .unwrap_or_default();
        pick_lan_resolver_ip(&original, &self.marker.controlled_ip).or_else(|| {
            pick_lan_resolver_ip(
                &self.ops.read_effective_resolvers().unwrap_or_default(),
                &self.marker.controlled_ip,
            )
        })
    }

    /// 本次接管挤掉的非公网解析器（见 [`Self::displaced_resolvers`] 字段文档）。
    ///
    /// 消费面是**告警**：macOS 的接管会把所有网络服务的 DNS 改成受控 IP，于是另一个 VPN
    /// （Tailscale 的 `100.100.100.100`、公司 VPN 的内网解析器…）装的解析器被整个挤掉，
    /// 表现为"IP 通、域名不通"，而此前全程无任何提示。
    #[must_use]
    pub fn displaced_resolvers(&self) -> &[String] {
        &self.displaced_resolvers
    }

    /// DHCP is an observed empty list; read failure cannot authorize takeover.
    fn snapshot_current(
        &self,
        targets: &[String],
    ) -> Result<BTreeMap<String, Vec<String>>, SystemIntegrationError> {
        let mut current = BTreeMap::new();
        for t in targets {
            let ips = self.ops.read_dns(t)?;
            current.insert(t.clone(), ips);
        }
        Ok(current)
    }

    /// Read-only exact recipe. DHCP is an empty static list, never a failed read.
    /// Effective physical resolvers are supplied by the retained pre-TUN deps, not
    /// freshly probed through an active tunnel.
    pub fn prepare_effect(
        &self,
        enabled: bool,
    ) -> Result<PreparedDnsEffect, SystemIntegrationError> {
        if !self.ops.takeover_supported() || !is_controlled_dns_ip_valid(&self.marker.controlled_ip)
        {
            return Err(SystemIntegrationError::UnsupportedPlatform(
                "live system DNS takeover".into(),
            ));
        }
        let (marker_raw, marker) = self
            .marker
            .read_raw_checked()
            .map_err(|e| SystemIntegrationError::dns(e.to_string()))?;
        let mut targets = self.ops.list_targets()?;
        targets.sort();
        if targets.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(SystemIntegrationError::dns(
                "duplicate DNS service identity",
            ));
        }
        if enabled && targets.is_empty() {
            return Err(SystemIntegrationError::dns(
                "no DNS service available for takeover",
            ));
        }
        if let Some(marker) = &marker {
            targets.extend(marker.original.keys().cloned());
            targets.sort();
            targets.dedup();
        }
        let before = self.snapshot_current(&targets)?;
        if let (Some(raw), Some(marker)) = (&marker_raw, &marker) {
            if let Some(effect) = dns_effect_marker(raw, marker)? {
                for (name, target) in &effect.target {
                    let current = before.get(name).ok_or_else(|| {
                        SystemIntegrationError::dns("DNS original scope is unavailable")
                    })?;
                    let owned = effect.phase == "owned" && current == target;
                    let partial = effect.phase != "owned"
                        && (Some(current) == effect.before.get(name)
                            || current == target
                            || Some(current) == marker.original.get(name));
                    if !owned && !partial {
                        return Err(SystemIntegrationError::dns(
                            "live DNS values are outside the exact restoration footprint",
                        ));
                    }
                }
            } else if marker.original.keys().any(|name| {
                !before
                    .get(name)
                    .is_some_and(|ips| is_controlled(ips, &marker.controlled_ip))
            }) {
                return Err(SystemIntegrationError::dns(
                    "legacy DNS intent has no complete owned read-back",
                ));
            }
        }
        if before.iter().any(|(name, values)| {
            is_controlled(values, &self.marker.controlled_ip)
                && !marker
                    .as_ref()
                    .is_some_and(|marker| marker.original.contains_key(name))
        }) {
            return Err(SystemIntegrationError::dns(
                "controlled DNS service has no original owner baseline",
            ));
        }
        let mut original = marker
            .as_ref()
            .map(|m| m.original.clone())
            .unwrap_or_default();
        for (name, ips) in compute_original_to_save(
            &before,
            &self.marker.controlled_ip,
            marker.as_ref().map(|m| &m.original),
        ) {
            original.entry(name).or_insert(ips);
        }
        let target = if enabled {
            before
                .keys()
                .map(|name| (name.clone(), vec![self.marker.controlled_ip.clone()]))
                .collect()
        } else if marker.is_some() {
            original.clone()
        } else {
            before.clone()
        };
        Ok(PreparedDnsEffect {
            marker_path: self.marker.path.clone(),
            marker_raw,
            before,
            target,
            original,
            controlled_ip: self.marker.controlled_ip.clone(),
            enabled,
            physical_lan_resolver: self.original_lan_resolver.clone(),
        })
    }

    /// Read-only adoption requires a complete typed owned scope and target
    /// readback. A legacy marker/policy bit is not an acceptance receipt.
    pub fn checked_owned_receipt(&self) -> Result<Option<TakeoverReceipt>, SystemIntegrationError> {
        let prepared = self.prepare_effect(true)?;
        let Some(operation) = prepared.prior_marker_operation_id() else {
            return Ok(None);
        };
        if prepared.needs_prior_restore()
            || prepared.before != prepared.target
            || !self.effect_facts_match(&prepared)?
        {
            return Ok(None);
        }
        Ok(Some(TakeoverReceipt::new(
            &operation,
            TakeoverOutcome::Unchanged,
            Some(operation.clone()),
            None,
        )))
    }

    pub fn exact_marker_operation_id(&self) -> Option<String> {
        let (raw, marker) = self.marker.read_raw_checked().ok()?;
        let raw = raw?;
        let marker = marker?;
        dns_effect_marker(&raw, &marker)
            .ok()
            .flatten()
            .map(|effect| effect.operation_id)
    }

    pub fn prepare_after_restore(
        &self,
        prior: &PreparedDnsEffect,
    ) -> Result<PreparedDnsEffect, SystemIntegrationError> {
        let next = self
            .prepare_effect(prior.enabled)?
            .with_physical_lan_resolver(prior.physical_lan_resolver.clone());
        if next.marker_path != prior.marker_path
            || next.marker_raw.is_some()
            || next.before != prior.original
            || next.original != prior.original
            || next.target != prior.target
        {
            return Err(SystemIntegrationError::dns(
                "DNS scope/baseline changed after exact prior-owner restoration",
            ));
        }
        Ok(next)
    }

    pub fn effect_facts_match(
        &self,
        prepared: &PreparedDnsEffect,
    ) -> Result<bool, SystemIntegrationError> {
        let current = self
            .prepare_effect(prepared.enabled)?
            .with_physical_lan_resolver(prepared.physical_lan_resolver.clone());
        Ok(current == *prepared)
    }

    fn effect_intent(
        &self,
        prepared: &PreparedDnsEffect,
        operation_id: &str,
        phase: &str,
    ) -> Result<String, String> {
        let baseline = SystemDnsMarker {
            controlled_ip: self.marker.controlled_ip.clone(),
            original: prepared.original.clone(),
            at: now_ms(),
        };
        let mut value = serde_json::to_value(baseline).map_err(|e| e.to_string())?;
        value["hostEffect"] = serde_json::json!({
            "operationId": operation_id, "phase": phase,
            "before": prepared.before, "target": prepared.target,
        });
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    fn raw_matches(&self, expected: &Option<String>) -> bool {
        self.marker
            .fs
            .read_marker_checked(&self.marker.path)
            .is_ok_and(|raw| &raw == expected)
    }

    /// Same controller + durable marker lock as startup/restore/reconcile. Intent
    /// precedes the first OS write and every completion is read back. A failed live
    /// apply compensates to this recipe's before state, not the user baseline.
    pub fn apply_effect(
        &mut self,
        prepared: &PreparedDnsEffect,
        operation_id: &str,
    ) -> TakeoverReceipt {
        let receipt = |outcome, reason| {
            TakeoverReceipt::new(operation_id, outcome, Some(operation_id.to_owned()), reason)
        };
        if operation_id.trim().is_empty() {
            return receipt(
                TakeoverOutcome::FailedUnchanged,
                Some("empty DNS operation identity".into()),
            );
        }
        let _marker_lock = match self
            .marker
            .fs
            .acquire_marker_mutation_lock(&self.marker.path)
        {
            Ok(lock) => lock,
            Err(error) => {
                return receipt(TakeoverOutcome::FailedUnchanged, Some(error.to_string()))
            }
        };
        match self.effect_facts_match(prepared) {
            Ok(true) => {}
            Ok(false) => {
                return receipt(
                    TakeoverOutcome::Superseded,
                    Some("DNS scope, values or marker changed".into()),
                )
            }
            Err(error) => {
                return receipt(TakeoverOutcome::FailedUnchanged, Some(error.to_string()))
            }
        }
        if prepared.needs_prior_restore() {
            let mut pending = receipt(
                TakeoverOutcome::RecoveryPending,
                Some("DNS prior owner requires confirmed restoration before replacement".into()),
            );
            pending.marker_id = prepared.prior_marker_operation_id();
            return pending;
        }
        let typed_owned = prepared.marker_raw.as_ref().is_some_and(|raw| {
            self.marker
                .read()
                .and_then(|marker| dns_effect_marker(raw, &marker).ok().flatten())
                .is_some_and(|effect| effect.phase == "owned")
        });
        if prepared.before == prepared.target
            && (prepared.enabled && typed_owned
                || !prepared.enabled && prepared.marker_raw.is_none())
        {
            if prepared.enabled && self.original_lan_resolver.is_none() {
                self.original_lan_resolver = prepared.physical_lan_resolver.clone();
            }
            let mut unchanged = receipt(TakeoverOutcome::Unchanged, None);
            unchanged.marker_id = None; // no new marker was created by this operation
            return unchanged;
        }
        if prepared.enabled {
            self.displaced_resolvers = self
                .ops
                .read_effective_resolvers()
                .map(|values| {
                    crate::dns::displaced_private_resolvers(&values, &self.marker.controlled_ip)
                })
                .unwrap_or_default();
        }
        let intent = match self.effect_intent(prepared, operation_id, "applying") {
            Ok(raw) => raw,
            Err(error) => return receipt(TakeoverOutcome::FailedUnchanged, Some(error)),
        };
        if !self.raw_matches(&prepared.marker_raw) {
            return receipt(
                TakeoverOutcome::Superseded,
                Some("DNS marker changed before intent".into()),
            );
        }
        if let Err(error) = self.marker.fs.write_marker(&self.marker.path, &intent) {
            return receipt(TakeoverOutcome::FailedUnchanged, Some(error.to_string()));
        }
        self.original = Some(prepared.original.clone());
        let owned_raw = Some(intent);
        let mut attempted = Vec::new();
        let result: Result<(), String> = (|| {
            for (name, target) in &prepared.target {
                if !self.raw_matches(&owned_raw) {
                    return Err("DNS intent replaced before mutation".into());
                }
                let before = prepared
                    .before
                    .get(name)
                    .ok_or("DNS target outside prepared scope")?;
                let current = self.ops.read_dns(name).map_err(|e| e.to_string())?;
                if &current != before {
                    return Err("DNS service changed before mutation".into());
                }
                if before != target {
                    attempted.push(name.clone());
                    self.ops
                        .apply_dns(name, target)
                        .map_err(|e| e.to_string())?;
                }
                if self.ops.read_dns(name).map_err(|e| e.to_string())? != *target {
                    return Err("DNS target read-back mismatch".into());
                }
            }
            let mut visible = self.ops.list_targets().map_err(|error| error.to_string())?;
            visible.sort();
            if visible != prepared.before.keys().cloned().collect::<Vec<_>>() {
                return Err("DNS service inventory changed during mutation".into());
            }
            if !self.raw_matches(&owned_raw) {
                return Err("DNS intent replaced before settlement".into());
            }
            if prepared.enabled {
                let owned = self.effect_intent(prepared, operation_id, "owned")?;
                self.marker
                    .fs
                    .write_marker(&self.marker.path, &owned)
                    .map_err(|e| e.to_string())?;
                if self.original_lan_resolver.is_none() {
                    self.original_lan_resolver = prepared.physical_lan_resolver.clone();
                }
            } else {
                self.marker
                    .fs
                    .remove_marker(&self.marker.path)
                    .map_err(|e| e.to_string())?;
                self.original = None;
                self.displaced_resolvers.clear();
                self.original_lan_resolver = None;
            }
            Ok(())
        })();
        match result {
            Ok(()) => receipt(TakeoverOutcome::Applied, None),
            Err(error) => {
                // Metadata failure may have installed a different phase. Never use a
                // stale receipt to overwrite it; retained intent owns recovery.
                if !self.raw_matches(&owned_raw) {
                    return receipt(TakeoverOutcome::RecoveryPending, Some(error));
                }
                let mut restored = true;
                let mut compensated = false;
                for name in attempted.iter().rev() {
                    let before = &prepared.before[name];
                    let target = &prepared.target[name];
                    match self.ops.read_dns(name) {
                        Ok(current) if &current == before => {}
                        Ok(current) if &current == target => {
                            compensated = true;
                            if self.ops.apply_dns(name, before).is_err()
                                || self
                                    .ops
                                    .read_dns(name)
                                    .map_or(true, |current| &current != before)
                            {
                                restored = false;
                            }
                        }
                        _ => restored = false, // foreign/unknown values confer no restore authority
                    }
                }
                // Include untouched services: a concurrent outside change is not a
                // "restored" receipt merely because our attempted subset recovered.
                restored &= self
                    .snapshot_current(&prepared.before.keys().cloned().collect::<Vec<_>>())
                    .is_ok_and(|current| current == prepared.before);
                if restored && self.raw_matches(&owned_raw) {
                    let settled = match &prepared.marker_raw {
                        Some(raw) => self.marker.fs.write_marker(&self.marker.path, raw),
                        None => self.marker.fs.remove_marker(&self.marker.path),
                    }
                    .is_ok();
                    if settled {
                        self.original = prepared
                            .marker_raw
                            .as_ref()
                            .map(|_| prepared.original.clone());
                        return receipt(
                            if compensated {
                                TakeoverOutcome::FailedRestored
                            } else {
                                TakeoverOutcome::FailedUnchanged
                            },
                            Some(error),
                        );
                    }
                }
                receipt(TakeoverOutcome::RecoveryPending, Some(error))
            }
        }
    }

    /// TUN 启动接管系统 DNS。**best-effort**：失败仅告警 + 回滚，绝不抛（不阻断 TUN 启动）。
    /// 上游 `setDns`。
    pub fn set_dns(&mut self) {
        let Ok(_marker_lock) = self
            .marker
            .fs
            .acquire_marker_mutation_lock(&self.marker.path)
        else {
            log::warn!("DNS marker lock unavailable; retaining recovery authority");
            return;
        };
        // 守卫：本平台不接管 DNS（win/linux）→ 早退。**必须在写 marker 之前** ——
        // 否则 marker 写下、apply 必失败（win netsh 需管理员）→ marker 卡死 → 每次启动反复空跑还原。
        // 判据见 `SystemDnsOps::takeover_supported`。
        if !self.ops.takeover_supported() {
            return;
        }
        // 守卫：受控 IP 在 bootstrap-direct → fail-closed 不接管。
        if !is_controlled_dns_ip_valid(&self.marker.controlled_ip) {
            return;
        }

        let Ok(targets) = self.ops.list_targets() else {
            return;
        };
        if targets.is_empty() {
            return;
        }

        // 读当前 DNS + 防自指计算原始值。
        let Ok(current) = self.snapshot_current(&targets) else {
            return;
        };
        let Ok((raw, existing)) = self.marker.read_raw_checked() else {
            return;
        };
        if let (Some(raw), Some(marker)) = (&raw, &existing) {
            match dns_effect_marker(raw, marker) {
                Ok(Some(effect)) if effect.phase != "owned" || effect.target != current => return,
                Err(_) => return,
                _ => {}
            }
        }
        let original = compute_original_to_save(
            &current,
            &self.marker.controlled_ip,
            existing.as_ref().map(|m| &m.original),
        );

        // **接管前**观测被挤掉的非公网解析器。必须在 apply 之前、且必须走
        // `read_effective_resolvers`（scutil）而不是上面 `snapshot_current` 的结果：
        // 后者读 `networksetup -getdnsservers`，看不见 NetworkExtension / DHCP 那一层，
        // 而 Tailscale 的 quad100 正是从那一层来的 —— 用 marker 里的原始值判会恒空。
        // 读失败不阻断接管（best-effort，与本函数其余部分同口径）。
        self.displaced_resolvers = self
            .ops
            .read_effective_resolvers()
            .map(|effective| {
                crate::dns::displaced_private_resolvers(&effective, &self.marker.controlled_ip)
            })
            .unwrap_or_default();

        // DHCP 在 networksetup 静态快照里是 []，另存生效地址；再次接管保留上一份。
        if self.original_lan_resolver.is_none() {
            self.original_lan_resolver = self.get_lan_resolver_for_dns();
        }

        // marker 前置写（intent）。
        if !self.raw_matches(&raw) {
            return;
        }
        if !self.marker.write(&original) {
            log::warn!("DNS recovery intent could not be persisted; takeover skipped");
            return;
        }
        self.original = Some(original.clone());

        // apply（整循环重试，非逐 target 单独重试——上游 `setDns` 的 `retry()` 包裹的是整个
        // for-loop：单条 target 抖动失败即整轮重跑，幂等安全，重复 apply 受控 IP 无害）。
        // 任一 attempt 耗尽重试后仍失败 → best-effort 回滚还原（不抛）。
        let controlled_ip = self.marker.controlled_ip.clone();
        let ops = &self.ops;
        let result = retry_op(
            &DNS_SET_RETRY,
            || -> Result<(), SystemIntegrationError> {
                for t in &targets {
                    ops.apply_dns(t, std::slice::from_ref(&controlled_ip))?;
                }
                Ok(())
            },
            self.sleeper,
        );
        if result.is_err() {
            // A failed rollback still owns recovery work; retain both sources.
            self.restore_dns_inner();
        }
    }

    /// 还原系统 DNS。无 marker/原始 → 仅清 marker。上游 `restoreDns`。
    pub fn restore_dns(&mut self) {
        let Ok(_marker_lock) = self
            .marker
            .fs
            .acquire_marker_mutation_lock(&self.marker.path)
        else {
            log::warn!("DNS marker lock unavailable; retaining recovery authority");
            return;
        };
        // 不接管的平台（win/linux）：**仍要清 marker** —— 清掉历史版本（旧版 netsh 失败）残留的
        // stuck marker，否则 has_marker 恒 true 致每个终态点/启动 recovery 反复空跑还原。
        // 对齐上游 `WindowsSystemDns.restoreDns` = `this.clearMarker()`。
        if !self.ops.takeover_supported() {
            self.original = None;
            self.marker.clear();
            return;
        }
        self.restore_dns_inner();
    }

    fn restore_dns_inner(&mut self) -> bool {
        let Ok((mut raw, marker)) = self.marker.read_raw_checked() else {
            return false;
        };
        let effect = match (&raw, &marker) {
            (Some(raw), Some(marker)) => match dns_effect_marker(raw, marker) {
                Ok(effect) => effect,
                Err(_) => return false,
            },
            _ => None,
        };
        let original = self
            .original
            .clone()
            .or_else(|| marker.as_ref().map(|m| m.original.clone()));
        let Some(original) = original else {
            return self.marker.clear();
        };

        // A live receipt retains only its exact before/target footprint. An
        // outside administrator's values do not become restore authority.
        if let Some(effect) = &effect {
            // A visible foreign value blocks all writes. A temporarily missing
            // service keeps debt but does not block restoring the visible subset.
            for (name, ips) in &original {
                if let Ok(current) = self.ops.read_dns(name) {
                    if Some(&current) != effect.before.get(name)
                        && Some(&current) != effect.target.get(name)
                        && &current != ips
                    {
                        return false;
                    }
                }
            }
            let Some(expected) = raw.as_ref() else {
                return false;
            };
            if !self.raw_matches(&raw) {
                return false;
            }
            let Ok(mut value) = serde_json::from_str::<serde_json::Value>(expected) else {
                return false;
            };
            value["hostEffect"]["phase"] = "restoring".into();
            let restoring = value.to_string();
            if self
                .marker
                .fs
                .write_marker(&self.marker.path, &restoring)
                .is_err()
            {
                return false;
            }
            raw = Some(restoring);
        }
        // Recorded custody, not today's enabled-service enumeration, owns restoration.
        // Unavailable services retain their recovery evidence for a later retry.
        let mut all_ok = true;
        for (target, ips) in &original {
            if effect.is_some() && !self.raw_matches(&raw) {
                return false;
            }
            if let Some(effect) = &effect {
                let Ok(current) = self.ops.read_dns(target) else {
                    all_ok = false;
                    continue;
                };
                if Some(&current) != effect.before.get(target)
                    && Some(&current) != effect.target.get(target)
                    && &current != ips
                {
                    all_ok = false;
                    continue;
                }
            }
            if self.ops.apply_dns(target, ips).is_err()
                || effect.is_some()
                    && self
                        .ops
                        .read_dns(target)
                        .map_or(true, |current| &current != ips)
            {
                all_ok = false;
            }
        }
        if all_ok && (effect.is_none() || self.raw_matches(&raw)) && self.marker.clear() {
            self.original = None;
            self.displaced_resolvers.clear();
            self.original_lan_resolver = None;
            true
        } else {
            // 部分失败 → 保留 marker 交下次启动重试。
            false
        }
    }

    /// 热插重灌：接管激活中（marker 在）时，把「新出现 / 仍未受控」的服务也接管为受控 IP。
    /// 不变量：① 仅 marker 在才动手；② 先写 marker 再 apply；③ 只 apply 未受控服务（幂等）；
    /// ④ best-effort 逐服务；⑤ 防自指（mergedOriginal 以既有 marker.original 为底）。
    /// 上游 `reconcileDns`。
    pub fn reconcile_dns(&mut self) {
        let _ = self.reconcile_dns_checked();
    }

    pub fn reconcile_dns_checked(&mut self) -> Option<TakeoverReceipt> {
        let Ok(_marker_lock) = self
            .marker
            .fs
            .acquire_marker_mutation_lock(&self.marker.path)
        else {
            log::warn!("DNS marker lock unavailable; retaining recovery authority");
            return None;
        };
        // 守卫：本平台不接管 DNS → 无物可重灌（marker 也不会有）。
        if !self.ops.takeover_supported() {
            return None;
        }
        // 守卫：受控 IP 在 bootstrap-direct → fail-closed。
        if !is_controlled_dns_ip_valid(&self.marker.controlled_ip) {
            return None;
        }
        let Ok((Some(raw), Some(marker))) = self.marker.read_raw_checked() else {
            // marker 不在 = 接管未激活 → 绝不擅自接管。
            return None;
        };

        let effect = match dns_effect_marker(&raw, &marker) {
            Ok(effect) => effect,
            Err(_) => return None,
        };
        let debt = |reason: &str| {
            effect.as_ref().map(|effect| {
                TakeoverReceipt::new(
                    &effect.operation_id,
                    TakeoverOutcome::RecoveryPending,
                    Some(effect.operation_id.clone()),
                    Some(reason.into()),
                )
            })
        };
        let mut targets = match self.ops.list_targets() {
            Ok(targets) => targets,
            Err(_) => return debt("DNS service enumeration is unavailable"),
        };
        targets.sort();
        if targets.windows(2).any(|pair| pair[0] == pair[1]) {
            return debt("duplicate DNS service identity");
        }
        if targets.is_empty() && effect.is_none() {
            return None;
        }
        let current = match self.snapshot_current(&targets) {
            Ok(current) => current,
            Err(_) => return debt("visible DNS service read-back is unavailable"),
        };
        if self.ops.list_targets().map_or(true, |mut visible| {
            visible.sort();
            visible != targets
        }) {
            return debt("DNS service inventory changed during capture");
        }
        if let Some(effect) = effect {
            return Some(self.reconcile_live_dns(raw, marker, effect, current));
        }

        // 以既有 marker.original 为底，并入当前各服务的应保存原始（防自指）。
        let mut merged = marker.original.clone();
        let computed =
            compute_original_to_save(&current, &self.marker.controlled_ip, Some(&merged));
        for (k, v) in computed {
            merged.insert(k, v);
        }

        // 只 apply 未受控服务。
        let to_apply: Vec<&String> = targets
            .iter()
            .filter(|t| {
                let ips = current.get(*t);
                !matches!(ips, Some(ips) if is_controlled(ips, &self.marker.controlled_ip))
            })
            .collect();
        if to_apply.is_empty() {
            return None; // 全部已受控 → 幂等 no-op。
        }

        // 先写 marker 再 apply（崩溃留 intent）。
        if !self.raw_matches(&Some(raw)) {
            return None;
        }
        if !self.marker.write(&merged) {
            log::warn!("DNS recovery intent update failed; reconcile skipped");
            return None;
        }
        self.original = Some(merged.clone());

        for t in to_apply {
            let _ = self
                .ops
                .apply_dns(t, std::slice::from_ref(&self.marker.controlled_ip));
        }
        None
    }

    /// Extend an accepted exact footprint under its original operation identity.
    /// Missing old services keep custody; they never prevent admission of a new
    /// visible service. Foreign values are not promoted to restore authority.
    fn reconcile_live_dns(
        &mut self,
        raw: String,
        mut marker: SystemDnsMarker,
        mut effect: DnsEffectMarker,
        current: BTreeMap<String, Vec<String>>,
    ) -> TakeoverReceipt {
        let document: serde_json::Value = match serde_json::from_str(&raw) {
            Ok(document) => document,
            Err(error) => {
                return TakeoverReceipt::new(
                    &effect.operation_id,
                    TakeoverOutcome::RecoveryPending,
                    Some(effect.operation_id.clone()),
                    Some(error.to_string()),
                )
            }
        };
        let operation_id = effect.operation_id.clone();
        let receipt = |outcome, reason| {
            TakeoverReceipt::new(&operation_id, outcome, Some(operation_id.clone()), reason)
        };
        if effect.phase == "restoring" {
            return receipt(
                TakeoverOutcome::RecoveryPending,
                Some("DNS restoration cannot be reversed by reconcile".into()),
            );
        }
        let applying = effect.phase == "applying";
        let missing = effect.target.keys().any(|name| !current.contains_key(name));
        if effect.target.iter().any(|(name, target)| {
            current.get(name).is_some_and(|value| {
                value != target && !(applying && Some(value) == effect.before.get(name))
            })
        }) {
            return receipt(
                TakeoverOutcome::RecoveryPending,
                Some("owned DNS service has foreign values".into()),
            );
        }
        let additions: Vec<_> = current
            .keys()
            .filter(|name| !effect.target.contains_key(*name))
            .cloned()
            .collect();
        if additions.is_empty() && !applying {
            return receipt(
                if missing {
                    TakeoverOutcome::RecoveryPending
                } else {
                    TakeoverOutcome::Unchanged
                },
                missing.then(|| "recorded DNS service is not visible".into()),
            );
        }
        for name in &additions {
            let before = &current[name];
            if is_controlled(before, &marker.controlled_ip) {
                return receipt(
                    TakeoverOutcome::RecoveryPending,
                    Some("new DNS service has no restoration baseline".into()),
                );
            }
            marker.original.insert(name.clone(), before.clone());
            effect.before.insert(name.clone(), before.clone());
            effect
                .target
                .insert(name.clone(), vec![marker.controlled_ip.clone()]);
        }
        let pending: Vec<_> = current
            .keys()
            .filter(|name| current.get(*name) != effect.target.get(*name))
            .cloned()
            .collect();
        marker.at = now_ms();
        effect.phase = "applying".into();
        let encode = |effect: &DnsEffectMarker| -> Result<String, String> {
            let mut value = document.clone();
            value["original"] =
                serde_json::to_value(&marker.original).map_err(|error| error.to_string())?;
            value["at"] = serde_json::json!(marker.at);
            value["hostEffect"] =
                serde_json::to_value(effect).map_err(|error| error.to_string())?;
            serde_json::to_string(&value).map_err(|error| error.to_string())
        };
        let intent = match encode(&effect) {
            Ok(intent) => intent,
            Err(error) => return receipt(TakeoverOutcome::FailedUnchanged, Some(error)),
        };
        if !self.raw_matches(&Some(raw)) {
            return receipt(
                TakeoverOutcome::Superseded,
                Some("DNS owner changed before reconcile".into()),
            );
        }
        if let Err(error) = self.marker.fs.write_marker(&self.marker.path, &intent) {
            return receipt(TakeoverOutcome::RecoveryPending, Some(error.to_string()));
        }
        self.original = Some(marker.original.clone());
        let owned = Some(intent);
        for name in &pending {
            if !self.raw_matches(&owned)
                || self
                    .ops
                    .read_dns(name)
                    .map_or(true, |value| value != effect.before[name])
            {
                return receipt(
                    TakeoverOutcome::RecoveryPending,
                    Some("DNS service or intent changed before reconcile write".into()),
                );
            }
            if self.ops.apply_dns(name, &effect.target[name]).is_err()
                || self
                    .ops
                    .read_dns(name)
                    .map_or(true, |value| value != effect.target[name])
            {
                return receipt(
                    TakeoverOutcome::RecoveryPending,
                    Some("DNS reconcile write/read-back failed".into()),
                );
            }
        }
        for name in current.keys() {
            if self
                .ops
                .read_dns(name)
                .map_or(true, |value| Some(&value) != effect.target.get(name))
            {
                return receipt(
                    TakeoverOutcome::RecoveryPending,
                    Some("DNS scope drifted during reconcile".into()),
                );
            }
        }
        if self.ops.list_targets().map_or(true, |mut visible| {
            visible.sort();
            visible != current.keys().cloned().collect::<Vec<_>>()
        }) {
            return receipt(
                TakeoverOutcome::RecoveryPending,
                Some("DNS service inventory changed during reconcile".into()),
            );
        }
        if !self.raw_matches(&owned) {
            return receipt(
                TakeoverOutcome::RecoveryPending,
                Some("DNS owner changed before reconcile settlement".into()),
            );
        }
        if applying && missing {
            return receipt(
                TakeoverOutcome::RecoveryPending,
                Some("partial DNS scope is still unavailable; applying footprint retained".into()),
            );
        }
        effect.phase = "owned".into();
        let settled = match encode(&effect) {
            Ok(settled) => settled,
            Err(error) => return receipt(TakeoverOutcome::RecoveryPending, Some(error)),
        };
        if self
            .marker
            .fs
            .write_marker(&self.marker.path, &settled)
            .is_err()
            || !self.raw_matches(&Some(settled))
        {
            return receipt(
                TakeoverOutcome::RecoveryPending,
                Some("DNS reconcile settlement is unconfirmed".into()),
            );
        }
        receipt(
            if missing {
                TakeoverOutcome::RecoveryPending
            } else {
                TakeoverOutcome::Applied
            },
            missing
                .then(|| "old DNS service restoration evidence retained while unavailable".into()),
        )
    }

    /// 是否存在接管 marker（终态清理门控用）。
    pub fn has_marker(&self) -> bool {
        self.marker.exists()
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
