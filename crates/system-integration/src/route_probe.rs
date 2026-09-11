//! 本机路由表探测 —— 找出**其它**隧道接口在宣告哪些网段。
//!
//! 判定（哪些算真冲突）不在这里，在 `config-engine` 的 `builder::tunnel_conflict`：
//! 那边是纯函数、本机跑得完单测；这边是平台相关的宿主读取，只负责把事实取回来。
//!
//! # 取材面纪律
//!
//! 本模块的解析器全部按**真实抓取的输出**写，不按记忆里的格式写。Linux 侧的样本取自
//! 2026-09-08 本机 `ip -o route show` / `ip -o -6 route show` 的实际输出，其中两种形态
//! 是照着记忆绝对写不出来的：
//!
//!  - **主机路由没有 `/前缀`**：`2408:…::be1 dev eno1 proto kernel …`；
//!  - **多路径（ECMP）路由顶层没有 `dev`**：`…/64 proto ra metric 105 pref medium\\	nexthop via … dev eno1 weight 1 \\	nexthop …`
//!    —— 一行里有字面量 `\` + 制表符做分隔，`dev` 只出现在各 nexthop 段里。
//!
//! 按"第一段是网段、`dev` 紧随其后"的直觉写，第二种会把 `nexthop` 的 dev 张冠李戴到主路由上。
//!
//! # macOS / Windows 未实现（如实登记，不是忘了）
//!
//! 这两个平台要解析 `netstat -rn` / `route print`，而**仓里没有任何真机抓取**，本机也没有
//! 那两个系统。照着猜写解析器就是拿想象中的格式当判据 —— 那正是本模块开头这段纪律要防的事。
//! 取样命令已经放进给现场设备的取证脚本（`netstat -rn -f inet` / `-f inet6` 两段）。
//!
//! **「未实现」在类型上独占一支**（[`TunnelProbeOutcome::Unsupported`]），不折成空列表：
//! 空列表会被下游读成「看过了，没有冲突」，于是 macOS 用户 —— 正是 2026-09-08 报障那台 ——
//! 会拿到一个自信的「无冲突」，而真相是这台机器上根本没人去看。缺席必须自己喊出来。

#![forbid(unsafe_code)]
#![allow(
    clippy::tabs_in_doc_comments,
    reason = "头注里那条多路径路由样例含**真实抓取的**字面量 `\\` + 制表符；\
              按 lint 建议换成四个空格，就把'照记忆写不出来的那一半'从文档里抹掉了"
)]

use crate::error::SystemIntegrationError;
use crate::exec::{Command, CommandRunner};
use polaris_helper_proto::Platform;
use std::time::Duration;

/// 一条路由：目的前缀 + 出接口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteEntry {
    /// 规范化后的目的前缀（主机路由补 `/32` / `/128`）。`default` 路由不产出条目。
    pub prefix: String,
    /// 出接口名。
    pub interface: String,
}

/// 解析 `ip -o route show` / `ip -o -6 route show` 的一行。
///
/// 返回 `None` 的三种情况，都是**有意跳过**而非失败：
///  - `default …`（不是具体网段，冲突判定用不上）；
///  - 顶层没有 `dev`（多路径路由，出接口在各 nexthop 段里 —— 它天然不是单一隧道的宣告）；
///  - 首段不像地址。
#[must_use]
pub fn parse_ip_route_line(line: &str) -> Option<RouteEntry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    // 多路径行含字面量 `\` + 制表符分隔的 nexthop 段。只取第一段（主路由那半），
    // 否则会把某个 nexthop 的 `dev` 当成主路由的出接口。
    let head = line.split('\\').next().unwrap_or(line);
    let mut tokens = head.split_whitespace();
    let dest = tokens.next()?;
    if dest == "default" || dest == "multicast" || dest == "broadcast" || dest == "local" {
        return None;
    }
    if !dest.contains('.') && !dest.contains(':') {
        return None;
    }
    // 只在**本段**里找 dev；找不到说明这是多路径主路由行，跳过。
    let mut interface = None;
    let mut rest = head.split_whitespace().peekable();
    while let Some(tok) = rest.next() {
        if tok == "dev" {
            interface = rest.peek().map(|s| (*s).to_string());
            break;
        }
    }
    let interface = interface?;
    let prefix = if dest.contains('/') {
        dest.to_string()
    } else if dest.contains(':') {
        format!("{dest}/128")
    } else {
        format!("{dest}/32")
    };
    Some(RouteEntry { prefix, interface })
}

/// 解析整份 `ip -o route show` 输出。
#[must_use]
pub fn parse_ip_routes(stdout: &str) -> Vec<RouteEntry> {
    stdout.lines().filter_map(parse_ip_route_line).collect()
}

/// 解析 `ip -o link show type tun` / `type wireguard` 的接口名。
///
/// 形如 `7: tailscale0: <POINTOPOINT,...> mtu 1280 ...` —— 取第二个冒号分隔字段。
/// 名字可能带 `@父接口`（VLAN/隧道常见），一并剥掉。
#[must_use]
pub fn parse_ip_link_names(stdout: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let mut parts = line.splitn(3, ':');
        let _index = parts.next();
        let Some(name) = parts.next() else { continue };
        let name = name.trim().split('@').next().unwrap_or("").trim();
        if !name.is_empty() && !out.iter().any(|e: &String| e == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// 从路由表里挑出**外来隧道**宣告的网段。
///
/// `tunnel_interfaces` 来自 `ip -o link show type tun|wireguard`；
/// `own_interfaces` 是 Polaris 自己的 TUN 接口名（不排除它，整张表都会被当成"别人的"）。
#[must_use]
pub fn foreign_tunnel_routes(
    routes: &[RouteEntry],
    tunnel_interfaces: &[String],
    own_interfaces: &[String],
) -> Vec<RouteEntry> {
    routes
        .iter()
        .filter(|r| {
            tunnel_interfaces.iter().any(|t| t == &r.interface)
                && !own_interfaces.iter().any(|o| o == &r.interface)
        })
        .cloned()
        .collect()
}

// ── 宿主探测（把上面那些解析器接到真实命令上）────────────────────────────────────────────
//
// 与 `route_ops` 同缝：命令经 [`CommandRunner`] 下发（mock 可单测、绝不在单测里 spawn `ip`），
// 平台用运行时 [`Platform`] 分派而非 `#[cfg(target_os)]` → Linux CI 上也能断言 mac/win 那一支。

/// 单条查询命令的硬超时（对齐 `route_ops::ROUTE_CMD_TIMEOUT`）。
pub const TUNNEL_PROBE_CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// 一次**成功**探测的事实。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForeignTunnelSnapshot {
    /// 本机全部隧道接口名（`type tun` ∪ `type wireguard`）。**含**我方 TUN ——
    /// 这是「本机有哪些隧道」的原始事实，剔除只发生在 [`Self::foreign`] 上。
    pub tunnel_interfaces: Vec<String>,
    /// 剔除 `own_interfaces` 之后，外来隧道宣告的路由。
    ///
    /// 空 `Vec` 在**这一支**里是有意义的结果（看过了，本机没有别的隧道在宣告网段）——
    /// 它与「没看」的区别由 [`TunnelProbeOutcome`] 在类型上承担，不由这个 `Vec` 兼任。
    pub foreign: Vec<RouteEntry>,
}

/// 一次探测的结果。**两支必须在编译期就分得开，这是本类型存在的全部理由。**
///
/// 把「本平台没有探测实现」折成 `Probed(空)` 会让下游把它读成「没有冲突」，于是 macOS 用户
/// —— 正是 2026-09-08 现场那台 —— 会看到一个自信的「无冲突」。故未实现走独立变体，
/// 加上命令失败那条 `Err`，三种「拿不到事实」的情形没有一条会伪装成「事实是空的」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelProbeOutcome {
    /// 本平台有解析实现且四条查询都跑通。
    Probed(ForeignTunnelSnapshot),
    /// 本平台**没有**解析实现（macOS / Windows / 未知平台）。见本模块头注「如实登记，不是忘了」。
    Unsupported(Platform),
}

/// 宿主外来隧道探测抽象。Linux 真实现，mac/win/other 一律 [`TunnelProbeOutcome::Unsupported`]。
pub trait ForeignTunnelProbe {
    /// 探本机**其它**隧道在宣告哪些网段。
    ///
    /// `own_interfaces`：Polaris 自己的 TUN 接口名（传空 = 整张表都会被当成"别人的"，
    /// 与自己的 mesh/FakeIP 段一比就冒出一堆自指告警）。
    ///
    /// - `Ok(Probed(..))`：探到了（`foreign` 可能为空 = 本机真的没有外来隧道）；
    /// - `Ok(Unsupported(p))`：本平台无解析实现 —— **不是**「无冲突」；
    /// - `Err`：命令执行失败（spawn/超时/非零退出）—— 同样**不是**「无冲突」。
    fn probe_foreign_tunnels(
        &self,
        own_interfaces: &[String],
    ) -> Result<TunnelProbeOutcome, SystemIntegrationError>;
}

/// [`ForeignTunnelProbe`] 的生产实现（运行时 [`Platform`] 分派 + [`CommandRunner`] 下发；零 cfg）。
pub struct ForeignTunnelProbeImpl<R: CommandRunner> {
    runner: R,
    platform: Platform,
}

impl<R: CommandRunner> ForeignTunnelProbeImpl<R> {
    /// 生产构造：平台取本机。
    pub fn new(runner: R) -> Self {
        Self::with_platform(runner, Platform::current())
    }

    /// 指定平台构造（测试用：Linux 上断言 mac/win 走未实现那一支）。
    pub fn with_platform(runner: R, platform: Platform) -> Self {
        Self { runner, platform }
    }

    fn run(&self, args: &[&str]) -> Result<String, SystemIntegrationError> {
        self.runner
            .run(
                &Command::new("ip", args.iter().copied()),
                TUNNEL_PROBE_CMD_TIMEOUT,
            )
            .map(|out| out.stdout)
            .map_err(SystemIntegrationError::route)
    }
}

impl<R: CommandRunner> ForeignTunnelProbe for ForeignTunnelProbeImpl<R> {
    fn probe_foreign_tunnels(
        &self,
        own_interfaces: &[String],
    ) -> Result<TunnelProbeOutcome, SystemIntegrationError> {
        if self.platform != Platform::Linux {
            return Ok(TunnelProbeOutcome::Unsupported(self.platform));
        }
        // 四条都是**只读**查询（`show`）：读路由表 / 链路表，不发包、不改任何内核状态、非 root。
        // `ip -o` 一条记录一行（多路径的续行用字面量 `\` + 制表符接在同一行里），正是上面那些解析器的取材形态。
        // 实测（2026-09-11 本机 iproute2）：`link show type <未知类型>` 不报错、返回空 + rc=0，
        // 故没有 wireguard 模块的内核上这条腿是安全的空，不会把整次探测拖成 `Err`。
        let route_v4 = self.run(&["-o", "route", "show"])?;
        let route_v6 = self.run(&["-o", "-6", "route", "show"])?;
        let tun_links = self.run(&["-o", "link", "show", "type", "tun"])?;
        let wireguard_links = self.run(&["-o", "link", "show", "type", "wireguard"])?;
        Ok(TunnelProbeOutcome::Probed(assemble_linux_probe(
            &route_v4,
            &route_v6,
            &tun_links,
            &wireguard_links,
            own_interfaces,
        )))
    }
}

/// 四段 stdout → 探测事实（纯函数）。
///
/// 命令执行与解析拆开，是为了让单测**注入字符串**而不必 spawn `ip`（与本模块既有单测同形态）。
#[must_use]
pub fn assemble_linux_probe(
    route_v4_stdout: &str,
    route_v6_stdout: &str,
    tun_link_stdout: &str,
    wireguard_link_stdout: &str,
    own_interfaces: &[String],
) -> ForeignTunnelSnapshot {
    let mut routes = parse_ip_routes(route_v4_stdout);
    routes.extend(parse_ip_routes(route_v6_stdout));
    let mut tunnel_interfaces = parse_ip_link_names(tun_link_stdout);
    for name in parse_ip_link_names(wireguard_link_stdout) {
        if !tunnel_interfaces.contains(&name) {
            tunnel_interfaces.push(name);
        }
    }
    let foreign = foreign_tunnel_routes(&routes, &tunnel_interfaces, own_interfaces);
    ForeignTunnelSnapshot {
        tunnel_interfaces,
        foreign,
    }
}

#[cfg(test)]
mod tests;
