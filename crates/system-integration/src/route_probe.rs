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
//! # macOS：两半都有样本了，mac 腿是真实现
//!
//! 2026-09-08 的现场机留下了一份真实的 `netstat -rn -f inet` / `-f inet6` 抓取；2026-09-12
//! 同一台又补了一份**全套**（含未过滤的 `ifconfig -a`），两份都逐字入库在
//! `route_probe/tests/fixtures/`。[`parse_netstat_routes`] 是照着**它们**写的，不是照着记忆 ——
//! 几种照记忆绝对写不出来的形态：
//!
//!  - **classful 缩写**：`192.168.10` 其实是 `192.168.10.0/24`，`169.254` 是 `/16`，`127` 是 `/8`；
//!    更反直觉的是 `224.0.0/4` —— 前缀长度**明写着**，地址却**仍然是缩写的**，两件事互相独立，
//!    于是「有 `/` 就照抄、没 `/` 才补」这条直觉规则在它身上恰好错；
//!  - **IPv6 目的地带 `%作用域`**：`fe80::%utun0/64`、`ff01::%utun3/32`、`fe80::1%lo0`。作用域
//!    不属于前缀本体，不剥掉就过不了 CIDR 校验；
//!  - **`Netif` 的列号必须从列头行读**：老 macOS 的列头多出 `Refs`/`Use` 两列，写死"第 4 列是接口"
//!    会把引用计数当成接口名 —— 而它长得像个合法接口名（纯数字不像，但错位后的 `Use` 列同样不像
//!    路由表会报错的东西，只会静默产出一批对不上任何隧道的条目）。
//!
//! 隧道枚举那一半由 [`parse_macos_tunnel_interfaces`] 承担，取材面是 `@@@IFCONFIG` 段
//! （`ifconfig -a` **全文**，未过滤）。它同样有两种照记忆写不出来的形态 ——
//! `bridge0` 的成员行 `\tmember: en1 flags=3<LEARNING,DISCOVER>` 与接口头行**逐字同形**、
//! 只有列位置分得开；`stf0: flags=0<>` 的 flags 列表是**空的**。细节见该函数的头注。
//!
//! 两半都在了，[`ForeignTunnelProbeImpl`] 的 mac 腿走 [`TunnelProbeOutcome::Probed`]。
//!
//! # Windows：路由表能解析了，隧道名单还差一列（如实登记，不是忘了）
//!
//! 2026-09-12 的 Windows 真机抓取到位，于是 [`parse_route_print_routes`] +
//! [`parse_get_netipaddress`] 按**它**写出来了。两处照记忆必错的地方：
//!
//!  - **两张表的「接口」列不是同一种东西，而且都不是接口名**：IPv4 表给的是**本地 IP**、
//!    IPv6 表给的是**接口索引**，都要再查一张对照表（[`WindowsInterfaceNames`]）；
//!  - **`route print` 的表头是本地化的**：那台机器 `culture=zh-CN`、`consoleoutputencoding=gb2312`，
//!    表头逐字是「接口列表」「活动路由:」。按英文表头硬匹配的解析器在这台机器上解析出**空表**
//!    —— 而空表长得像「这台机器没有路由」。故本模块的 Windows 解析**一个表头字面量都不认**，
//!    全靠 token 形状定位（见本模块私有的 `classify_route_print_row`）。
//!
//! 隧道名单那一半由 [`parse_windows_tunnel_interfaces`] 承担，判据是 `Get-NetAdapter` 的
//! `InterfaceType` 列 —— **IANA ifType 的两个标准值**，两个值各有真机正样本，不是从某台
//! 机器上看出来的规律：
//!
//!  - `131`（`IF_TYPE_TUNNEL`）：Teredo / IP-HTTPS / 6to4，两份 Windows 抓取里都有；
//!  - `53`（`IF_TYPE_PROP_VIRTUAL`）：**Tailscale 1.102.4 的 wintun**，Windows 11
//!    build 26200 真机实测（`fixtures/windows-w207-wintun-present-2026-09-12.txt`）。
//!
//! **`53` 是 2026-09-12 被真机坐实的一次判据翻案**：在那之前判据只认 `131`，而 wintun 报的
//! 是 `53` —— 旧判据在它唯一要做的那件事上静默失败（装着 Tailscale 的机器会拿到一句自信的
//! 「无冲突」）。这不是理论风险，是实测发生过的形态；`53` 比 `131` 宽这件事、以及为什么仍然
//! 收它（代价不对称），逐条写在 [`IF_TYPE_PROP_VIRTUAL`] 的头注里。
//!
//! 同一份抓取里另外两列（`NdisPhysicalMedium` / `ComponentID`）看着也能分，但一条在真实数据上
//! 根本不分隔（`以太网` 与三个隧道同为 `0`）、另一条已被 wintun 那行**直接证伪**（它的
//! `ComponentID` 是 `Wintun`，不空），故判据只认 ifType 这一列。
//!
//! 三半都在了，Windows 腿走 [`TunnelProbeOutcome::Probed`]。
//!
//! **🔴 仍然缺的三块（如实登记，逐条有绊线）**：
//!
//!  1. ~~wintun 上没有业务网段~~ —— **2026-09-12 22:14 补齐**：同机第三份抓取
//!     （`windows-w207-ts-on-2026-09-12.txt`，wintun **已登录**）上 wintun 有 30 条**业务**路由
//!     （25 条逐 peer `/32` + `100.100.100.100/32` + 3 条 `fd7a:…` v6），一条都没被噪声过滤器
//!     收掉；未登录那份（3 条全是 link-local）留作过滤器的**负样本**。一对正负样本钉在
//!     `tests::windows_leg_on_the_connected_capture_keeps_business_prefixes` 与
//!     `tests::windows_leg_on_the_logged_out_wintun_capture_sees_only_noise`，三份抓取的台阶
//!     差分钉在 `tests::the_three_windows_captures_step_from_no_wintun_to_business_prefixes`。
//!  2. **`53` 的假阳性面没有样本**：`IF_TYPE_PROP_VIRTUAL` 是「厂商自有虚拟接口」这个大桶，
//!     装了 Hyper-V / VMware / Docker 的机器上可能有别的适配器落进来。仓里两份抓取里报 `53`
//!     的只有 wintun 那一行。
//!  3. **另外两族隧道驱动的 ifType 仍无样本**：TAP-Windows / OpenVPN（`tap0901`）是以太网
//!     仿真驱动、Windows 内置 VPN（SSTP / L2TP / IKEv2）走 RAS —— 两者**落在 `{131, 53}` 之外
//!     的可能性很大**（前者疑报 `6`、后者疑报 `23` = PPP；**均未核实，仓里无样本，需真机抓取，
//!     code review 判不准**）。不盲收这两个值：`6` 是全部物理网卡，`23` 与 PPPoE 拨号同型 ⇒
//!     收了就是把一批真业务网卡判成隧道。
//!
//! 第 2、3 条由 `fixture_harness::the_wintun_capture_pins_iftype_53_and_two_driver_families_are_still_unverified`
//! 钉着：哪天有带这些适配器的抓取入库，它会红并要求按实测值重新评估判据面。
//!
//! 采集脚本：`~/docs/polaris/scripts/polaris-collect-routes-{macos.sh,windows.ps1}`。
//! `route_probe/tests/fixture_harness.rs` 每次 `cargo test` 都把「哪些分节有人解析、
//! 哪些缺什么」重算一遍并逐字点名，不是绿过。
//!
//! **编码**：Windows 侧的接口名走 cmdlet（`Get-NetIPAddress` / `Get-NetAdapter`），
//! 实测在 zh-CN 机器上是干净的 UTF-8；`netsh` 那条回退读法不是（`以太网` 打出来是
//! `浠ュお缃?`）。生产命令另外显式设了子进程输出编码，见私有的 `PS_GET_NETIPADDRESS`。
//!
//! **「未实现」在类型上独占一支**（[`TunnelProbeOutcome::Unsupported`]），不折成空列表：
//! 空列表会被下游读成「看过了，没有冲突」，于是用户 —— 正是 2026-09-08 报障那台 ——
//! 会拿到一个自信的「无冲突」，而真相是这台机器上根本没人去看。缺席必须自己喊出来。

#![forbid(unsafe_code)]
#![allow(
    clippy::tabs_in_doc_comments,
    reason = "头注里那条多路径路由样例含**真实抓取的**字面量 `\\` + 制表符；\
              按 lint 建议换成四个空格，就把'照记忆写不出来的那一半'从文档里抹掉了"
)]

use crate::error::SystemIntegrationError;
use crate::exec::{Command, CommandRunner};
use polaris_config_engine::user_config::cidr::cidr_contains;
use polaris_helper_proto::Platform;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use thiserror::Error;

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

// ── macOS / Windows：真机抓取 → 解析器（取材面纪律见模块头注）──────────────────────────

/// 真机抓取输出的解析失败。
///
/// 这一族解析器返回 `Result` 而不是「坏行跳过、剩下的算数」，理由与 [`TunnelProbeOutcome`]
/// 要给「未实现」独占一支是同一条：一份被截断的抓取会解析出一张**看起来正常、实则少一半**的
/// 路由表，而少掉的那一半恰恰可能是唯一那条隧道宣告 —— 下游读到的又是一个自信的「无冲突」。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RouteTableParseError {
    /// 整份输出里找不到列头行（空文件 / 根本不是路由表输出 / 抓取只剩抬头）。
    #[error("`{command}` 的输出里找不到列头行（应以 `Destination` 开头并含 `{column}` 列）")]
    MissingHeader {
        command: &'static str,
        column: &'static str,
    },
    /// 列头行在，但少了取材必需的那一列。
    #[error("`{command}` 的列头里没有 `{column}` 列：{header}")]
    MissingColumn {
        command: &'static str,
        column: &'static str,
        header: String,
    },
    /// 数据行的列数少于列头要求 —— 抓取被截断，或被人手工"规整"过。
    #[error(
        "`{command}` 第 {line_no} 行只有 {got} 列、至少要 {want} 列（抓取被截断或被手工规整过）：{line}"
    )]
    TruncatedRow {
        command: &'static str,
        line_no: usize,
        want: usize,
        got: usize,
        line: String,
    },
    /// 目的地字段不是可识别的网段/主机地址。
    #[error("`{command}` 第 {line_no} 行的目的地 `{dest}` 不是可识别的网段/主机地址")]
    UnparsableDestination {
        command: &'static str,
        line_no: usize,
        dest: String,
    },
    /// 本平台的这条读法**还没有解析实现**，因为仓里没有它的真机抓取。
    ///
    /// 这一支与 [`TunnelProbeOutcome::Unsupported`] 同源：缺席必须自己喊出来，不许折成空结果。
    ///
    /// 2026-09-12 之后**当前没有解析器返回它**（mac / Windows 的抓取都到了）。留着不是仪式：
    /// 它是下一个登记进 harness、却还一份抓取都没有的平台该走的那一支，harness 里对应的
    /// 分支也一直在（见 `fixture_harness::absorb_routes`）。与 [`Self::CaptureIncomplete`]
    /// 的区别是**缺的东西不同**：这支缺整份样本，那支样本在、缺判据必需的那一列。
    #[error("{platform:?} 的 `{command}` 还没有解析实现：仓里没有它的真机抓取（需要：{needed}）")]
    SampleMissing {
        platform: Platform,
        command: &'static str,
        needed: &'static str,
    },
    /// 样本**到了**，但这份抓取里缺了判据必需的那一列 / 那张表。
    ///
    /// 与 [`Self::SampleMissing`] 分开是因为该做的事不同：那边要「去现场机跑采集脚本」，
    /// 这边要「改采集脚本的 `Select-Object`，再跑一次」。混成一支，下一个人会白跑一趟。
    #[error(
        "{platform:?} 的 `{command}` 抓取不全：缺 {missing} —— 没有它判不出结果（重抓方式：{needed}）"
    )]
    CaptureIncomplete {
        platform: Platform,
        command: &'static str,
        missing: &'static str,
        needed: &'static str,
    },
    /// 路由行的「接口」键（Windows v4 的本地 IP / v6 的接口索引）在对照表里查不到。
    ///
    /// **不折成跳过**：跳过等于产出一份少几条的 `Ok`，而少掉的那几条恰恰可能是唯一那条隧道宣告。
    /// `table_entries` 带着表长度，一眼分得开「对照表压根是空的」与「表有内容、真没这一条」。
    #[error(
        "`{command}` 第 {line_no} 行的接口 `{key}` 在对照表里查不到（表里有 {table_entries} 条）"
    )]
    UnresolvedInterface {
        command: &'static str,
        line_no: usize,
        key: String,
        table_entries: usize,
    },
    /// 一份看着正常的抓取里，一条数据行都没解析出来。
    ///
    /// 与 [`Self::MissingHeader`] 的区别：那条是「连表头都没有」，这条是「表头/形状都在，
    /// 但没有任何一行匹配上」—— 抓取被截断、分节给错、或解析器与真实格式脱节了。
    #[error("`{command}` 的 {lines} 行输出里，一条 {what} 都没解析出来（抓取截断 / 分节给错 / 解析器与真实格式脱节）")]
    NoParsableRows {
        command: &'static str,
        what: &'static str,
        lines: usize,
    },
}

/// `netstat -rn` 的一个**目的地字段** → 规范化 CIDR。
///
/// 三条规则全部取自 2026-09-08 的真实抓取，没有一条是推的：
///
/// | 抓到的样子 | 出来的样子 | 为什么 |
/// |---|---|---|
/// | `192.168.10` | `192.168.10.0/24` | classful 缩写：省掉的是**尾部零字节**，留下几段前缀就是 8×几 |
/// | `224.0.0/4` | `224.0.0.0/4` | 长度明写着、地址**仍然**缩写 —— 补零与定长度是两件独立的事 |
/// | `192.168.10.105` | `192.168.10.105/32` | 主机路由不带 `/前缀`（与 Linux 那侧同型） |
/// | `fe80::%utun0/64` | `fe80::/64` | `%作用域`是 BSD 打印 link-local 的附注，不属于前缀本体 |
/// | `default` | `None` | 不是具体网段，冲突判定用不上（与 [`parse_ip_route_line`] 同口径） |
///
/// 返回 `None` 的两类（`default` / 不可识别）**在本函数里不区分**：需要区分的调用方自己先判
/// `default`，[`parse_netstat_routes`] 正是这么做的 —— 不可识别的目的地在那里是错误，不是跳过。
#[must_use]
pub fn expand_netstat_destination(dest: &str) -> Option<String> {
    let (addr, explicit_len) = match dest.split_once('/') {
        Some((a, l)) => (a, Some(l.parse::<u8>().ok()?)),
        None => (dest, None),
    };
    // `%zone` 先剥：`fe80::%utun0` 与 `fe80::1%lo0` 两种位置都覆盖得到。
    let addr = addr.split('%').next().unwrap_or(addr);
    if addr.is_empty() {
        return None;
    }

    if addr.contains(':') {
        let canonical = addr.parse::<Ipv6Addr>().ok()?;
        let len = explicit_len.unwrap_or(128);
        return (len <= 128).then(|| format!("{canonical}/{len}"));
    }

    // v4：先确认只由数字与点组成，`default` / `link#14` 这类在这里出局。
    if !addr.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let octets: Vec<&str> = addr.split('.').collect();
    if octets.len() > 4 {
        return None;
    }
    let mut full = [0u8; 4];
    for (slot, text) in full.iter_mut().zip(octets.iter()) {
        // 长度上限挡住 `0300` 这类前导零写法（stdlib 的 `Ipv4Addr` 同样拒收，口径一致）。
        if text.is_empty() || text.len() > 3 {
            return None;
        }
        *slot = text.parse::<u8>().ok()?;
    }
    // 缩写掉几段，就少几个 8 位；`/N` 明写时以明写的为准（`224.0.0/4` 是 /4 不是 /24）。
    let implied_len = match octets.len() {
        1 => 8,
        2 => 16,
        3 => 24,
        _ => 32,
    };
    let len = explicit_len.unwrap_or(implied_len);
    (len <= 32).then(|| format!("{}.{}.{}.{}/{len}", full[0], full[1], full[2], full[3]))
}

/// 解析整份 `netstat -rn -f inet` / `netstat -rn -f inet6`（macOS/BSD）。
///
/// **列位置从列头行读，不写死**：本机抓到的列头是
/// `Destination Gateway Flags Netif Expire`（`Netif` 在第 4 列），而老 macOS 中间多出
/// `Refs`/`Use` 两列。写死列号的版本在后者上不会报错，只会静默把别的列当接口名。
///
/// # Errors
///
/// 见 [`RouteTableParseError`]：找不到列头、列头缺 `Netif`、数据行被截断、目的地不可识别。
/// 四种都**不**折成"少几条的 `Ok`" —— 那正是这个 `Result` 存在的理由。
pub fn parse_netstat_routes(stdout: &str) -> Result<Vec<RouteEntry>, RouteTableParseError> {
    const CMD: &str = "netstat -rn";
    const COL: &str = "Netif";

    let mut netif_idx: Option<usize> = None;
    let mut out = Vec::new();

    for (i, raw) in stdout.lines().enumerate() {
        let line_no = i + 1;
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        // `netstat -rn` 不带 `-f` 时一份输出里有两张表，各自带一行表标题 + 一行列头。
        if line == "Routing tables" || line == "Internet:" || line == "Internet6:" {
            continue;
        }

        let tokens: Vec<&str> = line.split_whitespace().collect();

        if tokens.first() == Some(&"Destination") {
            let Some(idx) = tokens.iter().position(|t| *t == COL) else {
                return Err(RouteTableParseError::MissingColumn {
                    command: CMD,
                    column: COL,
                    header: line.to_string(),
                });
            };
            netif_idx = Some(idx);
            continue;
        }

        // 列头还没出现 ⇒ 还在抬头里（脚本写的分节标记、`Routing tables` 之类），跳过。
        let Some(idx) = netif_idx else { continue };

        if tokens.len() <= idx {
            return Err(RouteTableParseError::TruncatedRow {
                command: CMD,
                line_no,
                want: idx + 1,
                got: tokens.len(),
                line: line.to_string(),
            });
        }

        let dest = tokens[0];
        if dest == "default" {
            continue; // 与 `parse_ip_route_line` 同口径：默认路由不是具体网段。
        }
        let Some(prefix) = expand_netstat_destination(dest) else {
            return Err(RouteTableParseError::UnparsableDestination {
                command: CMD,
                line_no,
                dest: dest.to_string(),
            });
        };
        out.push(RouteEntry {
            prefix,
            interface: tokens[idx].to_string(),
        });
    }

    if netif_idx.is_none() {
        return Err(RouteTableParseError::MissingHeader {
            command: CMD,
            column: COL,
        });
    }
    Ok(out)
}

// ── macOS：`ifconfig -a` → 接口花名册 → 隧道接口 ──────────────────────────────────────────

/// `ifconfig -a` 里的一个接口条目：接口名 + 尖括号里那串 flags。
///
/// flags **逐字**保留（不规整大小写、不排序、不去重）：判据只是「列表里有没有
/// [`MAC_TUNNEL_FLAG`]」，多规整一道就多一层与真实输出的距离。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacInterfaceFlags {
    /// 接口名（不含尾随冒号）。
    pub name: String,
    /// 尖括号里那串 flags。**可以是空的** —— 见 [`parse_macos_interface_flags`] 的第二条。
    pub flags: Vec<String>,
}

/// macOS 上「这是个隧道接口」的 flag。
///
/// 用 flag 而不是名字前缀清单（`utun` / `ipsec` / `gif` / …）：名字清单在下一种隧道出现的那天
/// 静默漏掉它，而 `POINTOPOINT` 是内核给点对点设备打的标，新设备天然带上。
pub const MAC_TUNNEL_FLAG: &str = "POINTOPOINT";

/// 解析整份 `ifconfig -a`（macOS/BSD）→ 全部接口的 flags 花名册。
///
/// 取材面是 2026-09-12 那台现场机的真实 `ifconfig -a` 全文（**未过滤**），逐字入库在
/// `route_probe/tests/fixtures/macos-p101-routes-ts-off-2026-09-12.txt` 的 `@@@IFCONFIG` 段。
/// 其中两种形态是照记忆绝对写不出来的：
///
///  - **嵌套的「假接口头行」**：`bridge0` 的成员行逐字是
///    `\tmember: en1 flags=3<LEARNING,DISCOVER>` —— 与真正的接口头行
///    `en1: flags=8963<UP,…>` 几乎同形（`<名字>: … flags=N<…>`），区别只在**列位置**
///    （接口头行顶格，成员行以制表符开头）与 `flags=` **在不在冒号正后面**。
///    按「含 `flags=` 就算一个接口」写，会凭空多出三个叫 `member` 的接口；
///    按「`:` 前是名字」写更糟 —— `en1`/`en2`/`en3` 会被重复登记一遍，且带着 bridge 成员的
///    `LEARNING,DISCOVER` 而不是它们自己的 flags。
///
///    本函数用两条判据叠着挡：**顶格**才是接口头行，且冒号之后**紧跟** `flags=`。
///    这份抓取里的成员行是被后一条挡掉的（`en1` 夹在中间）；前一条挡的是那种逐字
///    同形的续行（`\tmember: flags=…`），它不在这份抓取里 —— 相应的断言用的是构造输入，
///    在 `fixture_harness` 里如实登记着。
///  - **空的 flags 列表**：`stf0: flags=0<> mtu 1280` —— 尖括号里什么都没有。按
///    `flags=\d+<([A-Z,]+)>` 这类「至少一个」的形状写，这一行整个匹配不上 ⇒ `stf0` 从花名册里
///    静默消失；而「某个接口不在花名册上」正是本模块要防的那种无人喊的缺席。
///
/// 顶格但不是 `<名字>: flags=…` 的行一律跳过（本抓取里没有这种行；真有也不是接口头）。
/// 一行都认不出来时由调用方 [`parse_macos_tunnel_interfaces`] 报错，本函数只负责认。
#[must_use]
pub fn parse_macos_interface_flags(ifconfig_stdout: &str) -> Vec<MacInterfaceFlags> {
    let mut out = Vec::new();
    for line in ifconfig_stdout.lines() {
        // 续行（制表符 / 空格开头）一律不是接口头行。
        //
        // 这是**真正的规则**（接口头行顶格），不是对某种续行形状的补丁。2026-09-12 那份抓取里
        // `bridge0` 的成员行 `\tmember: en1 flags=3<…>` 其实在下面那条
        // 「`:` 之后紧跟 `flags=`」上就已经出局（`en1` 夹在中间），轮不到这一条；
        // 但只要哪天出现一条**逐字**是接口头行形状的续行（`\tmember: flags=…<…>`），
        // 就只剩这一条分得开。两条各有各的射程，别因为「另一条也挡得住」把这条删了。
        if line.is_empty() || line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        let Some(after) = rest.trim_start().strip_prefix("flags=") else {
            continue;
        };
        let Some(open) = after.find('<') else {
            continue;
        };
        let Some(close) = after[open..].find('>') else {
            continue;
        };
        // `flags=0<>` ⇒ inner 是空串 ⇒ 空 flags 列表（**不是**跳过这个接口）。
        let inner = &after[open + 1..open + close];
        out.push(MacInterfaceFlags {
            name: name.to_string(),
            flags: inner
                .split(',')
                .filter(|f| !f.is_empty())
                .map(str::to_string)
                .collect(),
        });
    }
    out
}

/// macOS `ifconfig -a` → **全部**隧道接口名。
///
/// 判据是 flags 里有 [`MAC_TUNNEL_FLAG`]。本函数不区分「我方 TUN」与「外来隧道」——
/// 那是调用方拿 `own_interfaces` 做的事（见 [`foreign_tunnel_routes`]）。
///
/// **登记一条边界**：2026-09-12 那份抓取里 `stf0`（6to4 伪设备）是 `flags=0<>` ——
/// 一个 flag 都没有，于是它不在返回值里。这不是漏：`flags=0` 的设备没有 UP、没有地址，
/// 路由表上也不会有它的条目，`foreign_tunnel_routes` 与它取交集恒空。等哪天抓到一份
/// `stf0` 起来了的样本，它会自己带上 `POINTOPOINT` 进来。
///
/// # Errors
///
/// [`RouteTableParseError::NoParsableRows`]：整份输出里一个接口头行都认不出来
/// （空抓取 / 分节给错 / `ifconfig` 输出形态与本解析器脱节）。**不**折成空列表 ——
/// 空列表会被下游读成「这台机器上没有隧道」。
pub fn parse_macos_tunnel_interfaces(
    ifconfig_stdout: &str,
) -> Result<Vec<String>, RouteTableParseError> {
    let roster = parse_macos_interface_flags(ifconfig_stdout);
    if roster.is_empty() {
        return Err(RouteTableParseError::NoParsableRows {
            command: "ifconfig -a",
            what: "接口头行（顶格的 `<名字>: flags=N<…>`）",
            lines: ifconfig_stdout.lines().count(),
        });
    }
    Ok(roster
        .into_iter()
        .filter(|i| i.flags.iter().any(|f| f == MAC_TUNNEL_FLAG))
        .map(|i| i.name)
        .collect())
}

// ── Windows：接口名对照表 + `route print` 路由表 ──────────────────────────────────────────

/// Windows 路由表的「接口」列 → 接口名（`InterfaceAlias`）的对照表。
///
/// 这张表存在的理由是 Windows 与 mac/Linux 最大的一处不对称：**两张路由表的「接口」列
/// 根本不是同一种东西，而且都不是接口名**。
///
/// | 表 | 那一列是 | 抓到的样子 |
/// |---|---|---|
/// | `route print -4` | **本地 IP 地址** | `192.168.10.207` |
/// | `route print -6` | **接口索引**（整数） | `8` |
///
/// 两者都要再查一张表才落得到名字上，而那张表只能从 `Get-NetIPAddress` / `Get-NetAdapter`
/// 这类 cmdlet 来。**不**用 `route print` 自带的 Interface List（`8...Red Hat VirtIO Ethernet
/// Adapter`）：那一列是 `InterfaceDescription`，与 `InterfaceAlias`（`以太网`）是两个命名空间
/// —— 混用会让同一块网卡在 v4 结果里叫 `以太网`、v6 结果里叫 `Red Hat VirtIO Ethernet Adapter`，
/// 而 [`foreign_tunnel_routes`] 是按名字逐字比的，一半会对不上。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowsInterfaceNames {
    by_ip: BTreeMap<String, String>,
    by_index: BTreeMap<u32, String>,
}

impl WindowsInterfaceNames {
    /// 表里一共有多少条映射（IP 的 + 索引的）。错误信息里带上它，一眼分得开
    /// 「表是空的」与「表有内容但真没这一条」。
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_ip.len() + self.by_index.len()
    }

    /// 表里一条映射都没有。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 本地 IP → 接口名（`route print -4` 的 `Interface` 列走这条）。
    #[must_use]
    pub fn by_ip(&self, ip: &str) -> Option<&str> {
        self.by_ip.get(ip).map(String::as_str)
    }

    /// 接口索引 → 接口名（`route print -6` 的 `If` 列走这条）。
    #[must_use]
    pub fn by_index(&self, index: u32) -> Option<&str> {
        self.by_index.get(&index).map(String::as_str)
    }
}

/// 一份 `Format-Table` 输出切好的表：列头各格 + 数据行各格。
///
/// 列**位置**从分隔线行（`---- ---- …`）读，列的**语义**从列头行的字面量读 —— 与
/// [`parse_netstat_routes`] 的 `Netif` 同一条纪律。这里敢认字面量是因为它们是 PowerShell 的
/// **属性名**（`InterfaceAlias` / `InterfaceType` / …），由采集脚本的 `Select-Object` 定死，
/// 与机器的 UI 语言无关；`route print` 的表头才是本地化的，那边一个字面量都不许认
/// （见私有的 `classify_route_print_row`）。
struct FormatTable {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl FormatTable {
    /// 按列头字面量找列号。
    fn column(&self, name: &str) -> Option<usize> {
        self.header.iter().position(|c| c == name)
    }

    /// 取某行某列；越界返回 `None`（`Format-Table` 会把右侧空列整段省掉）。
    fn cell(row: &[String], idx: usize) -> Option<&str> {
        row.get(idx).map(String::as_str)
    }
}

/// `Format-Table` 输出 → [`FormatTable`]。`anchors` 里的字面量必须**同时**出现在列头行上，
/// 否则返回 `None`（调用方据此报自己那支错）。
fn parse_format_table(stdout: &str, anchors: &[&str]) -> Option<FormatTable> {
    let lines: Vec<&str> = stdout.lines().collect();
    let header_at = lines
        .iter()
        .position(|l| anchors.iter().all(|a| l.contains(a)))?;
    // 列头行之后的第一条非空行必须是分隔线行（`Format-Table` 的固定形态）。
    let dashes_at = (header_at + 1..lines.len())
        .find(|i| !lines[*i].trim().is_empty())
        .filter(|i| lines[*i].trim_start().starts_with('-'))?;
    let spans = column_spans(lines[dashes_at]);
    Some(FormatTable {
        header: slice_by_columns(lines[header_at], &spans),
        rows: lines[dashes_at + 1..]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| slice_by_columns(l, &spans))
            .collect(),
    })
}

/// 一行按列跨度切出来的各列。
///
/// 切片按**字符**不按字节：这份抓取里有 `以太网`，按字节切会切在 UTF-8 码点中间 —— 那是
/// 一次 panic，不是一个坏结果。
fn slice_by_columns(line: &str, spans: &[(usize, Option<usize>)]) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    spans
        .iter()
        .map(|(start, end)| {
            let end = end.map_or(chars.len(), |e| e.min(chars.len()));
            let start = (*start).min(end);
            chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect()
}

/// 从 `Format-Table` 的分隔线行（`---- -------- …`）算出各列的字符跨度。
/// 最后一列的终点是 `None`（吃到行尾）—— 右对齐的数字列常常比分隔线还长。
fn column_spans(dashes: &str) -> Vec<(usize, Option<usize>)> {
    let mut starts = Vec::new();
    let mut in_run = false;
    for (i, c) in dashes.chars().enumerate() {
        match (c, in_run) {
            ('-', false) => {
                starts.push(i);
                in_run = true;
            }
            ('-', true) => {}
            (_, _) => in_run = false,
        }
    }
    let mut spans: Vec<(usize, Option<usize>)> = Vec::with_capacity(starts.len());
    for (i, start) in starts.iter().enumerate() {
        spans.push((*start, starts.get(i + 1).copied()));
    }
    spans
}

/// 解析 `Get-NetIPAddress | Select-Object …, InterfaceAlias, InterfaceIndex | Format-Table`
/// → [`WindowsInterfaceNames`]。
///
/// 一条记录同时喂两张映射：`IPAddress` → 名字（给 v4 表用）、`InterfaceIndex` → 名字（给 v6 表用）。
/// IPv6 的 `%作用域`（`fe80::3463:e45e:6a9c:9334%8`）在建索引前剥掉，并按 [`std::net::IpAddr`]
/// 规范化 —— `route print` 那侧打印的是同一个地址的规范写法，两边不走同一条规范化就查不上。
///
/// **为什么这张表必须来自 `Get-NetIPAddress` 而不是 `Get-NetAdapter`**：2026-09-12 那台机器的
/// `Get-NetAdapter -IncludeHidden` 输出里**没有** `Loopback Pseudo-Interface 1`（索引 1），
/// 而 `route print -6` 在索引 1 上有两条路由（`::1/128`、`ff00::/8`）。只拿 Get-NetAdapter 建表，
/// 这两条会解析不出接口名 —— 而「隐藏适配器不在 Get-NetAdapter 里」是照记忆写不出来的。
///
/// # Errors
///
/// - [`RouteTableParseError::MissingHeader`]：找不到同时含 `InterfaceAlias` 与 `InterfaceIndex` 的列头行；
/// - [`RouteTableParseError::MissingColumn`]：列头在、分隔线行给出的列数对不上那两列；
/// - [`RouteTableParseError::NoParsableRows`]：列头在、一条记录都没解析出来（抓取被截断）。
pub fn parse_get_netipaddress(stdout: &str) -> Result<WindowsInterfaceNames, RouteTableParseError> {
    const CMD: &str = "Get-NetIPAddress";
    const ALIAS: &str = "InterfaceAlias";
    const INDEX: &str = "InterfaceIndex";

    let table =
        parse_format_table(stdout, &[ALIAS, INDEX]).ok_or(RouteTableParseError::MissingHeader {
            command: CMD,
            column: ALIAS,
        })?;
    let find = |name: &'static str| {
        table
            .column(name)
            .ok_or(RouteTableParseError::MissingColumn {
                command: CMD,
                column: name,
                header: table.header.join(" | "),
            })
    };
    let alias_idx = find(ALIAS)?;
    let index_idx = find(INDEX)?;
    // `IPAddress` 是第一列；不按字面量找它，按「第一列」拿 —— 采集脚本的 `Select-Object`
    // 把它放在首位，而这张表的用途（IP → 名字）本来就只认得首列那个地址。
    let mut names = WindowsInterfaceNames::default();

    for row in &table.rows {
        let (Some(alias), Some(index)) = (
            FormatTable::cell(row, alias_idx),
            FormatTable::cell(row, index_idx),
        ) else {
            continue;
        };
        let Ok(index) = index.parse::<u32>() else {
            continue;
        };
        if alias.is_empty() {
            continue;
        }
        names.by_index.insert(index, alias.to_string());
        if let Some(ip) = row.first().and_then(|c| normalize_ip_key(c)) {
            names.by_ip.insert(ip, alias.to_string());
        }
    }

    if names.is_empty() {
        return Err(RouteTableParseError::NoParsableRows {
            command: CMD,
            what: "IP/索引 → 接口名的记录",
            lines: stdout.lines().count(),
        });
    }
    Ok(names)
}

/// `fe80::…%8` / `192.168.10.207` → 规范化的地址字面量（查表的键）。非地址返回 `None`。
fn normalize_ip_key(raw: &str) -> Option<String> {
    let addr = raw.split('%').next().unwrap_or(raw);
    addr.parse::<IpAddr>().ok().map(|a| a.to_string())
}

/// `route print` 的一行被认成了什么。认不出来的行一律跳过（分隔线、标题、列头、续行、
/// Interface List、永久路由表 —— 都在这条路上出局）。
enum RoutePrintRow<'a> {
    /// `route print -4` 的活动路由行：目的 + 掩码 + **本地 IP** 形式的出接口。
    V4 {
        dest: Ipv4Addr,
        mask: Ipv4Addr,
        interface_ip: Ipv4Addr,
    },
    /// `route print -6` 的活动路由行：**接口索引** + 已带 `/长度` 的目的前缀。
    V6 { index: u32, dest: &'a str },
}

/// 一行 → [`RoutePrintRow`]，**全靠 token 的形状，一个表头字面量都不认**。
///
/// 为什么不能认表头：2026-09-12 那台机器 `culture=zh-CN`、`consoleoutputencoding=gb2312`，
/// `route print` 的表头逐字是「接口列表」「IPv4 路由表」「活动路由:」——
/// 按英文表头硬匹配的解析器在这台机器上**解析出空表**，而空表长得像「这台机器没有路由」。
/// 更毒的是 v6 的列头行：中文下四个列名之间**没有空格**（` 接口跃点数网络目标                网关`），
/// 按空白切只得到两个 token —— 连「列头有几列」都数不对，谈不上按列名定位。
///
/// 于是判据全落在结构上：
///
///  - **v4 活动路由行**：≥5 个 token，首二为 IPv4（目的 + 掩码）、**末位**为整数（跃点数）、
///    **倒数第二位**为 IPv4（`Interface` 列 = 本地 IP）。两端各下一个锚，中间的网关列爱几个
///    token 都行（`在链路上` / `On-link` 是一个，别的语言可能是两个）。
///  - **v6 活动路由行**：≥3 个 token，首二为整数（`If` + 跃点数）、第三位是带 `/长度` 的 IPv6 前缀。
///    **≥3 而不是 =4**：目的地长到一定程度时 `route print -6` 会把网关**折到下一行**去
///    （`…:98b0/128` 之后另起一行、缩进 36 个空格写 `在链路上`）。要求满 4 个 token 的写法会
///    把这份抓取里 **12 条 /128 主机路由**静默丢掉 —— 恰好是最像「某个隧道在宣告地址」的那批。
///    折行本身不吃：网关列这边根本用不上。
///  - **永久路由表**（`route print -4` 末尾那张）只有 4 列、**没有接口列**，被上面的 ≥5 挡掉。
///    它不带信息损失：永久路由只要生效就一定也在活动路由表里。
fn classify_route_print_row<'a>(tokens: &[&'a str]) -> Option<RoutePrintRow<'a>> {
    if let (Some(first), Some(second)) = (tokens.first(), tokens.get(1)) {
        if let (Ok(dest), Ok(mask)) = (first.parse::<Ipv4Addr>(), second.parse::<Ipv4Addr>()) {
            if tokens.len() >= 5 {
                let last = tokens[tokens.len() - 1];
                let second_last = tokens[tokens.len() - 2];
                if last.parse::<u32>().is_ok() {
                    if let Ok(interface_ip) = second_last.parse::<Ipv4Addr>() {
                        return Some(RoutePrintRow::V4 {
                            dest,
                            mask,
                            interface_ip,
                        });
                    }
                }
            }
            return None;
        }
    }
    if tokens.len() >= 3 {
        if let (Ok(index), Ok(_metric)) = (tokens[0].parse::<u32>(), tokens[1].parse::<u32>()) {
            let dest = tokens[2];
            if dest.contains('/') && canonical_ipv6_prefix(dest).is_some() {
                return Some(RoutePrintRow::V6 { index, dest });
            }
        }
    }
    None
}

/// `aded:14a8:b9be:ee8b::/60` → 规范化前缀。`route print -6` 的目的地**恒带** `/长度`，
/// 故这里不做 mac 那侧的主机路由补长度；不带 `/` 的行压根不会走到这儿。
fn canonical_ipv6_prefix(dest: &str) -> Option<String> {
    let (addr, len) = dest.split_once('/')?;
    let len = len.parse::<u8>().ok()?;
    let addr = addr.parse::<Ipv6Addr>().ok()?;
    (len <= 128).then(|| format!("{addr}/{len}"))
}

/// `255.255.255.0` → `24`。非连续掩码（`255.0.255.0`）返回 `None`，不折成一个看着合理的长度。
fn ipv4_mask_to_prefix_len(mask: Ipv4Addr) -> Option<u8> {
    let bits = u32::from(mask);
    if bits.leading_ones() + bits.trailing_zeros() != 32 {
        return None;
    }
    u8::try_from(bits.leading_ones()).ok()
}

/// 解析一份 `route print -4` **或** `route print -6`（Windows）→ 路由条目。
///
/// 两种都认：v4 行与 v6 行的 token 形状天然不交（见私有的 `classify_route_print_row`），
/// 一个函数吃哪一份都行，调用方不必先判断自己拿到的是哪个 `-4`/`-6`。
///
/// `names` 是 [`parse_get_netipaddress`] 建的对照表 —— **没有它就没有接口名**，
/// 这也是本函数比 mac/Linux 那两支多吃一个参数的全部理由。
///
/// # Errors
///
/// - [`RouteTableParseError::UnparsableDestination`]：掩码不是合法的连续掩码；
/// - [`RouteTableParseError::UnresolvedInterface`]：本地 IP / 接口索引在对照表里查不到 ——
///   **这一条绝不折成跳过**：跳过等于少几条路由的 `Ok`，而少掉的可能正是唯一那条隧道宣告；
/// - [`RouteTableParseError::NoParsableRows`]：一条活动路由行都没认出来（抓取截断 / 分节给错）。
pub fn parse_route_print_routes(
    route_print_stdout: &str,
    names: &WindowsInterfaceNames,
) -> Result<Vec<RouteEntry>, RouteTableParseError> {
    const CMD: &str = "route print";

    let mut out = Vec::new();
    let mut rows = 0usize;
    for (i, raw) in route_print_stdout.lines().enumerate() {
        let line_no = i + 1;
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        let Some(row) = classify_route_print_row(&tokens) else {
            continue;
        };
        rows += 1;
        match row {
            RoutePrintRow::V4 {
                dest,
                mask,
                interface_ip,
            } => {
                let Some(len) = ipv4_mask_to_prefix_len(mask) else {
                    return Err(RouteTableParseError::UnparsableDestination {
                        command: CMD,
                        line_no,
                        dest: format!("{dest} 掩码 {mask}"),
                    });
                };
                if len == 0 {
                    continue; // 默认路由，与 `parse_ip_route_line` / `parse_netstat_routes` 同口径。
                }
                let key = interface_ip.to_string();
                let Some(interface) = names.by_ip(&key) else {
                    return Err(RouteTableParseError::UnresolvedInterface {
                        command: CMD,
                        line_no,
                        key,
                        table_entries: names.len(),
                    });
                };
                out.push(RouteEntry {
                    prefix: format!("{dest}/{len}"),
                    interface: interface.to_string(),
                });
            }
            RoutePrintRow::V6 { index, dest } => {
                let Some(prefix) = canonical_ipv6_prefix(dest) else {
                    return Err(RouteTableParseError::UnparsableDestination {
                        command: CMD,
                        line_no,
                        dest: dest.to_string(),
                    });
                };
                if prefix == "::/0" {
                    continue; // 默认路由。
                }
                let Some(interface) = names.by_index(index) else {
                    return Err(RouteTableParseError::UnresolvedInterface {
                        command: CMD,
                        line_no,
                        key: format!("接口索引 {index}"),
                        table_entries: names.len(),
                    });
                };
                out.push(RouteEntry {
                    prefix,
                    interface: interface.to_string(),
                });
            }
        }
    }

    if rows == 0 {
        return Err(RouteTableParseError::NoParsableRows {
            command: CMD,
            what: "活动路由行",
            lines: route_print_stdout.lines().count(),
        });
    }
    Ok(out)
}

/// IANA ifType `131` = `IF_TYPE_TUNNEL` —— Windows 给**协议隧道**类适配器的值。
/// `Get-NetAdapter` 把它原样放在 `InterfaceType` 列里。
///
/// **实测正样本**（两份 Windows 抓取都有）：`Teredo Tunneling Pseudo-Interface` /
/// `Microsoft IP-HTTPS Platform Interface` / `6to4 Adapter`。
///
/// 用 ifType、而不用「描述列为空」「没有 MAC」这类从抓取里看出来的规律：那两条在 2026-09-12
/// 那台机器上恰好也能圈中三个隧道，但都会漏掉真正要防的 Tailscale wintun（它**有**描述、
/// **自带** MAC）。判据取的是标准值，不是某台机器的巧合。
///
/// 它**不**覆盖用户态 VPN 的虚拟网卡 —— 那些走 [`IF_TYPE_PROP_VIRTUAL`]，实测见下。
pub const IF_TYPE_TUNNEL: u32 = 131;

/// IANA ifType `53` = `IF_TYPE_PROP_VIRTUAL`（厂商自有虚拟接口）。
///
/// **实测正样本**：Tailscale 1.102.4 的 wintun 适配器 —— 别名 `Tailscale`、`ComponentID` =
/// `Wintun`、`DriverDescription` = `Wintun Userspace Tunnel`、`Status` = `Up`，
/// Windows 11 build 26200，见 `fixtures/windows-w207-wintun-present-2026-09-12.txt`。
/// 在这份抓取入库之前，判据只认 [`IF_TYPE_TUNNEL`]，也就是**漏掉了它唯一真要防的那一个**。
///
/// # 它比 [`IF_TYPE_TUNNEL`] 宽，这是**知情**取的
///
/// `53` 是「厂商自有虚拟接口」这个大桶，不是「隧道」。装了 Hyper-V / VMware / Docker 的机器上
/// 可能有别的虚拟适配器落在这个桶里（仓里没有这种样本 ⇒ **未核实**）。仍然收它，理由是
/// **代价不对称**：
///
///  - **假阳性便宜**：一个非隧道的虚拟适配器被当成隧道，后果只是它宣告的网段进了**考察面**。
///    冲突判定在 `polaris_config_engine::builder::tunnel_conflict::detect_tunnel_conflicts`
///    里只认 FakeIP / Mesh / TUN 地址三类**网段相交**，不会凭空多出一条冲突；展示面还会先摘掉
///    link-local 与组播（`src-tauri` 的 `runtime::proxy::tunnel_conflict::announced_routes`）。
///    剩下的代价是「设置页多列一条网段」——**吵，但不瞎**。
///  - **假阴性致命**：漏掉 wintun = 用户装着 Tailscale、Polaris 说「无冲突」。这一整条链存在
///    的理由就是不让那句话出现，而旧判据正是这么失败的。
///
/// 方向与本模块其余部分一致：**报多不报少**（同 [`LINK_LOCAL_AND_MULTICAST_BLOCKS`] 那条
/// 「偏宽是吵、偏窄是瞎」）。
///
/// # 为什么不拿 `ComponentID == "Wintun"` 把它收窄
///
/// 那是把一条**类型**判据换成一张**驱动名单**：wintun 之外的每一种隧道驱动（TAP-Windows 的
/// `tap0901`、WireGuard NT、各家企业 VPN 客户端）都不在名单上，于是全漏 —— 把一个已知的漏换成
/// 一族未知的漏。而 `ComponentID` 这条启发式在这份抓取上已经被 wintun 那行直接证伪：
/// 它的 `ComponentID` 不空。
pub const IF_TYPE_PROP_VIRTUAL: u32 = 53;

/// 隧道判据的**全部**取值：`InterfaceType ∈ {131, 53}`，一个字面量都不额外认。
///
/// 写成 slice（而不是两个 `||`）有一条实用理由：判据面**可枚举** —— 测试能拿它做两份抓取的
/// 差分，变异也一行改得完（删掉 `53` 那项 = 回到那个已知会漏 wintun 的旧判据）。
pub const WINDOWS_TUNNEL_IF_TYPES: &[u32] = &[IF_TYPE_TUNNEL, IF_TYPE_PROP_VIRTUAL];

/// Windows `Get-NetAdapter -IncludeHidden` → **全部**隧道接口名（`InterfaceAlias`）。
///
/// # 判据：`InterfaceType ∈ {131, 53}`（[`WINDOWS_TUNNEL_IF_TYPES`]），只认这一列
///
/// 实测取材面 = `fixtures/windows-w207-wintun-present-2026-09-12.txt` 的 `@@@GET_NETADAPTER`
/// （Tailscale 1.102.4 已装、服务已起；同机的另一份 `…-routes-ts-off-…` 是装之前的对照，
/// 除 `Tailscale` 那行外适配器完全相同）。**四正两负**：
///
/// | InterfaceAlias | InterfaceType | NdisPhysicalMedium | ComponentID | DriverDescription | Status | 是隧道 |
/// |---|---|---|---|---|---|---|
/// | `Teredo Tunneling Pseudo-Interface` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
/// | `Microsoft IP-HTTPS Platform Interface` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
/// | `6to4 Adapter` | **131** | 0 | （空） | 同别名 | Not Present | ✅ |
/// | `Tailscale`（wintun） | **53** | 0 | `Wintun` | `Wintun Userspace Tunnel` | **Up** | ✅ |
/// | `以太网` | 6 | 0 | `PCI\VEN_1AF4&…` | `Red Hat VirtIO Ethernet Adapter` | Up | ❌ |
/// | `以太网(内核调试器)` | 6 | 14 | `root\kdnic` | `Microsoft Kernel Debug Network Adapter` | Not Present | ❌ |
///
/// 两个值各是什么、`53` 为什么宽得起 → [`IF_TYPE_TUNNEL`] / [`IF_TYPE_PROP_VIRTUAL`] 的头注。
/// **`53` 这一行是本表的全部意义**：2026-09-12 之前判据只认 `131`，而唯一真要防的那个适配器
/// 报的是 `53`。
///
/// **另外两列抓回来了，但都不作交叉判据**，理由写在这张表里：
///
///  - `NdisPhysicalMedium`：四个隧道全是 `0`，而 `以太网` **也是 `0`** —— 它在这份真实数据上
///    根本不分隔隧道与网卡，叠进来只会让判据看起来更严、实际一点没变；
///  - `ComponentID`：在装 Tailscale **之前**那份抓取上它看着能用（三个隧道全空、两块网卡都不空）
///    —— 正是这种「看着能用」让它危险。它与「描述列为空」是同一族启发式，而 wintun 那行**直接
///    证伪**了它：`ComponentID` = `Wintun`，不空。叠上「且 ComponentID 为空」会把唯一真要防的
///    那个排除掉。**加它不是更保险，是更错** —— 这句话现在有真机反例，不再只是推理。
///
/// # 🔴 取材面缺口（如实登记，逐条有绊线）
///
///  1. ~~wintun 上没有业务网段~~ —— **已补齐**：同机第三份抓取（`…-ts-on-…`，wintun 已登录）上
///     wintun 有 30 条业务路由（25 条逐 peer `/32` + `100.100.100.100/32` + 3 条 `fd7a:…` v6）。
///     未登录那份（`tailscale status` 逐字 `Logged out.`，wintun 上只有 3 条 link-local）留作
///     噪声过滤的负样本。两侧分别钉在 `tests::windows_leg_on_the_connected_capture_keeps_business_prefixes`
///     与 `tests::windows_leg_on_the_logged_out_wintun_capture_sees_only_noise`。
///  2. **`53` 的假阳性面没有样本**：仓里两份抓取里报 `53` 的只有 wintun 一行；Hyper-V / VMware /
///     Docker 机器上有没有别的适配器报它，**未核实**。
///  3. **TAP-Windows / OpenVPN（`tap0901`）与 Windows 内置 VPN（RAS：SSTP / L2TP / IKEv2）
///     的 ifType 无样本**：前者疑报 `6`、后者疑报 `23`（PPP），**均未核实**；若成立，本判据漏它们。
///     不盲收那两个值 —— `6` 是全部物理网卡、`23` 与 PPPoE 拨号同型，收了就是把一批真业务网卡
///     判成隧道（那才是「报多」真正会翻车的方向）。
///
/// 第 2、3 条由 `fixture_harness::the_wintun_capture_pins_iftype_53_and_two_driver_families_are_still_unverified`
/// 钉着：带这些适配器的抓取一入库它就会红，并要求按实测值重新评估判据面。
///
/// # Errors
///
/// - [`RouteTableParseError::CaptureIncomplete`]：输出里没有 `InterfaceType` 列。两种来源都落这一支
///   —— 旧版采集脚本没 `Select` 它；以及 `netsh interface ipv4 show interfaces` 这条回退读法
///   **结构上**就给不出适配器类型（它的表只有 `Idx/Met/MTU/State/Name`，且 `Name` 列在
///   zh-CN 控制台上还是乱码，`以太网` 打出来是 `浠ュお缃?`）。
/// - [`RouteTableParseError::NoParsableRows`]：列头在、一条适配器都没解析出来（抓取被截断）。
///
/// 两条都**不**折成空名单：空名单会被 [`foreign_tunnel_routes`] 读成「这台机器上没有隧道」。
pub fn parse_windows_tunnel_interfaces(
    adapter_stdout: &str,
) -> Result<Vec<String>, RouteTableParseError> {
    const CMD: &str = "Get-NetAdapter -IncludeHidden";
    const ALIAS: &str = "InterfaceAlias";
    const IF_TYPE: &str = "InterfaceType";

    let Some(table) = parse_format_table(adapter_stdout, &[ALIAS, IF_TYPE]) else {
        return Err(RouteTableParseError::CaptureIncomplete {
            platform: Platform::Win,
            command: CMD,
            missing: "`InterfaceType`（IANA ifType）列 —— 隧道判据就是它",
            needed: "隧道名单只能走 `Get-NetAdapter -IncludeHidden | Select-Object InterfaceAlias, InterfaceIndex, InterfaceType, …`（判据三列靠左放：`Format-Table` 挤不下时从右边开始丢列）；`netsh interface … show interfaces` 这条回退读法**结构上**给不出类型列，顶不上来",
        });
    };
    let find = |name: &'static str| {
        table
            .column(name)
            .ok_or(RouteTableParseError::MissingColumn {
                command: CMD,
                column: name,
                header: table.header.join(" | "),
            })
    };
    let alias_idx = find(ALIAS)?;
    let type_idx = find(IF_TYPE)?;

    let mut seen = 0usize;
    let mut tunnels = Vec::new();
    for row in &table.rows {
        let (Some(alias), Some(if_type)) = (
            FormatTable::cell(row, alias_idx),
            FormatTable::cell(row, type_idx),
        ) else {
            continue;
        };
        let Ok(if_type) = if_type.parse::<u32>() else {
            continue;
        };
        seen += 1;
        if WINDOWS_TUNNEL_IF_TYPES.contains(&if_type) && !alias.is_empty() {
            tunnels.push(alias.to_string());
        }
    }

    if seen == 0 {
        return Err(RouteTableParseError::NoParsableRows {
            command: CMD,
            what: "适配器记录",
            lines: adapter_stdout.lines().count(),
        });
    }
    Ok(tunnels)
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

/// 「按定义不可能是隧道宣告的业务网段」的五个块：v4/v6 各一对 link-local 与组播，外加受限广播。
///
/// 取值来源是 RFC，不是从某台机器上看出来的规律：`fe80::/10`（RFC 4291 §2.5.6）、
/// `ff00::/8`（RFC 4291 §2.7）、`169.254.0.0/16`（RFC 3927 §2.1）、`224.0.0.0/4`（RFC 5771 §1）、
/// `255.255.255.255/32`（受限广播，RFC 919 §7 / RFC 922 §7）。
///
/// # 为什么广播是**单个地址**而不是 `240.0.0.0/4`
///
/// 2026-09-12 的 mac 连接态抓取里，`255.255.255.255/32` 挂在 `utun11` 上 —— 组播块
/// `224.0.0.0/4` 只盖 224–239，受限广播落在 `240.0.0.0/4`，于是它漏进展示面、被当成
/// 「隧道宣告的业务网段」。
///
/// 但收的只能是那**一个地址**：`240.0.0.0/4` 是整段保留空间，把它整块列进来等于凭
/// 「反正没人用」放宽判据 —— 判据要能说出每一条为什么按定义不可能，而不是「看着不像业务」。
/// 受限广播能说：它是链路本地的一次性泛洪目标，没有任何控制面会把它作为可达网段宣告出来。
///
/// **一个私网块都不在里面，这是刻意的**：Tailscale 的 tailnet v6 段 `fd7a:115c:a1e0::/48`
/// 住在 `fc00::/7` 里，而它是货真价实的业务网段。把「私网」一起收掉的写法看上去更干净，
/// 代价是把唯一那条真要报的宣告一起误杀 —— 那正是 2026-09-08 现场那台要看见的东西。
pub const LINK_LOCAL_AND_MULTICAST_BLOCKS: [&str; 5] = [
    "fe80::/10",
    "ff00::/8",
    "169.254.0.0/16",
    "224.0.0.0/4",
    "255.255.255.255/32",
];

/// 这条前缀是不是 link-local 或组播。
///
/// # 判据是**包含**，不是相交
///
/// 只有整条前缀都落在某个块**里面**才算。用相交的话，`224.0.0.0/3`（覆盖 224–255，含整个
/// 保留段）、`::/0` 这种比块更宽的前缀会被一并判成噪声 —— 它们覆盖的可路由空间远不止组播，
/// 收掉就是误杀。偏宽的失败方向是多显示一条（吵），偏窄的方向是静默少显示（瞎）。
///
/// # 本模块**一处都不调用它**（刻意，不是漏接）
///
/// 探测层的职责是如实读回路由表：`netstat -rn` 打印的这些条目确实是那个 utun 宣告的路由，
/// 过滤属于解释。判据住在这里只是为了让「谁算噪声」**全仓只有一份**——此前它已经有过两份口径
/// （本模块覆盖测试里的 `starts_with("fe80:")`），而两份口径迟早会漂。
///
/// 唯一的生产消费方是展示面：`src-tauri` 的
/// `runtime::proxy::tunnel_conflict::TunnelConflictSnapshot::to_wire`。判定面
/// （`config-engine` 的 `builder::tunnel_conflict`）**不用它**，那张表只认 FakeIP / Mesh / TUN
/// 三类真冲突，喂给它的永远是未经过滤的全量事实。
#[must_use]
pub fn is_link_local_or_multicast(prefix: &str) -> bool {
    LINK_LOCAL_AND_MULTICAST_BLOCKS
        .iter()
        .any(|block| cidr_contains(block, prefix))
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
    /// 本平台有解析实现且该平台的几条查询都跑通。
    Probed(ForeignTunnelSnapshot),
    /// 本平台**没有**解析实现（未知平台；freebsd/openbsd/…）。见本模块头注「如实登记，不是忘了」。
    Unsupported(Platform),
}

/// 宿主外来隧道探测抽象。Linux / macOS / Windows 真实现；未知平台走 [`TunnelProbeOutcome::Unsupported`]。
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

/// Windows 取接口名对照表的 PowerShell 脚本。
///
/// 与 `~/docs/polaris/scripts/polaris-collect-routes-windows.ps1` 的 `@@@GET_NETIPADDRESS` 段
/// **逐字同形**：解析器照那份抓取写的，命令一漂移解析的就是另一种输出。
///
/// 开头那句设输出编码是**生产独有**的一步，采集脚本不需要：采集脚本把结果写进 UTF-8 文件，
/// 而这里的 stdout 要过控制台代码页（那台机器是 `gb2312`），再被 [`crate::exec`] 的
/// `from_utf8_lossy` 解码 —— 不设的话 `以太网` 会变成一串 U+FFFD，而**多个不同的名字会塌成
/// 同一串替换字符**，于是「某条路由属于哪个适配器」会张冠李戴。
/// **2026-09-12 真机实测确认**（Windows 11 build 26200 · PS 5.1.26100 · culture `zh-CN` ·
/// consoleoutputencoding `gb2312`）：按生产形态跑 `powershell.exe -NoProfile -NonInteractive
/// -Command <本常量>`，stdout 经管道取回 ——
///
/// | | `file(1)` 判定 | `from_utf8_lossy` 之后 |
/// |---|---|---|
/// | 带这一句 | `UTF-8 text` | `以太网` / `以太网(内核调试器)` 完好 |
/// | 去掉（负向对照） | `ISO-8859`，byte 161 起非法续字节 | 两个名字都被替换字符污染 |
///
/// ⇒ 这一句**必要且充分**，不是防御性冗余。删掉它不会编译失败、也不会让任何单测转红
/// （单测喂的是已解码的字符串），只会在真机上把接口名打成替换字符，进而让「某条路由属于
/// 哪个适配器」张冠李戴。故由 `windows_probe_runs_the_four_read_only_queries_in_order`
/// 逐字钉住它必须在。
const PS_GET_NETIPADDRESS: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
     Get-NetIPAddress | Select-Object IPAddress, PrefixLength, AddressFamily, InterfaceAlias, InterfaceIndex | \
     Sort-Object InterfaceIndex, AddressFamily | Format-Table -AutoSize";

/// Windows 取适配器清单的 PowerShell 脚本（隧道判据 `InterfaceType` 就在这份里）。
///
/// `Select-Object` 的列表与采集脚本**逐字一致**（含末尾的 `LinkSpeed`）——
/// 那份抓取的输出里其实**没有** `LinkSpeed` 列：`Format-Table -AutoSize` 挤不下时从**右边**
/// 开始丢列，它就是被丢掉的那个。照抓取写意味着照产生那份抓取的**命令**写，而不是照它
/// 恰好剩下的列写。
///
/// 推论：列的**顺序**是判据的一部分 —— `InterfaceAlias` / `InterfaceIndex` / `InterfaceType`
/// 三列必须靠左，才轮不到被丢。真丢到 `InterfaceType` 时
/// [`parse_windows_tunnel_interfaces`] 报 [`RouteTableParseError::CaptureIncomplete`]，
/// 不会静默给出一份空名单。
const PS_GET_NETADAPTER: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; \
     Get-NetAdapter -IncludeHidden | Select-Object InterfaceAlias, InterfaceIndex, InterfaceType, \
     NdisPhysicalMedium, ComponentID, DriverDescription, Name, InterfaceDescription, Status, MacAddress, \
     LinkSpeed | Format-Table -AutoSize";

/// [`ForeignTunnelProbe`] 的生产实现（运行时 [`Platform`] 分派 + [`CommandRunner`] 下发；零 cfg）。
pub struct ForeignTunnelProbeImpl<R: CommandRunner> {
    runner: R,
    platform: Platform,
    /// Windows `route.exe` 绝对路径（规避 PATH 缺 System32；见 [`crate::exec::system32`]）。
    route_exe: String,
    /// Windows `powershell.exe` 绝对路径（同上；与 `route_ops` 同形）。
    powershell_exe: String,
}

impl<R: CommandRunner> ForeignTunnelProbeImpl<R> {
    /// 生产构造：平台取本机。
    pub fn new(runner: R) -> Self {
        Self::with_platform(runner, Platform::current())
    }

    /// 指定平台构造（测试用：Linux CI 上就能断言 mac/win 腿的 argv 与解析）。
    pub fn with_platform(runner: R, platform: Platform) -> Self {
        Self {
            runner,
            platform,
            route_exe: crate::exec::system32_from_env("route.exe"),
            powershell_exe: crate::exec::system32_from_env(
                "WindowsPowerShell\\v1.0\\powershell.exe",
            ),
        }
    }

    fn run(&self, program: &str, args: &[&str]) -> Result<String, SystemIntegrationError> {
        self.runner
            .run(
                &Command::new(program, args.iter().copied()),
                TUNNEL_PROBE_CMD_TIMEOUT,
            )
            .map(|out| out.stdout)
            .map_err(SystemIntegrationError::route)
    }

    /// Linux 腿：四条只读 `ip` 查询。
    ///
    /// 四条都是**只读**查询（`show`）：读路由表 / 链路表，不发包、不改任何内核状态、非 root。
    /// `ip -o` 一条记录一行（多路径的续行用字面量 `\` + 制表符接在同一行里），正是那些解析器的取材形态。
    /// 实测（2026-09-11 本机 iproute2）：`link show type <未知类型>` 不报错、返回空 + rc=0，
    /// 故没有 wireguard 模块的内核上这条腿是安全的空，不会把整次探测拖成 `Err`。
    fn probe_linux(
        &self,
        own_interfaces: &[String],
    ) -> Result<TunnelProbeOutcome, SystemIntegrationError> {
        let route_v4 = self.run("ip", &["-o", "route", "show"])?;
        let route_v6 = self.run("ip", &["-o", "-6", "route", "show"])?;
        let tun_links = self.run("ip", &["-o", "link", "show", "type", "tun"])?;
        let wireguard_links = self.run("ip", &["-o", "link", "show", "type", "wireguard"])?;
        Ok(TunnelProbeOutcome::Probed(assemble_linux_probe(
            &route_v4,
            &route_v6,
            &tun_links,
            &wireguard_links,
            own_interfaces,
        )))
    }

    /// macOS 腿：三条只读查询。
    ///
    /// `netstat -rn -f inet|inet6` 读的是内核路由表、`ifconfig -a` 读的是接口表，
    /// 三条都不发包、不改状态、不需要 root（`ifconfig` 只有带**配置参数**时才要 root，
    /// 裸 `-a` 是纯查询）。argv 与 `~/docs/polaris/scripts/polaris-collect-routes-macos.sh`
    /// 的 `sec V4/V6/IFCONFIG` **逐字一致** —— 解析器照那份抓取写的，取材命令一旦漂移，
    /// 解析的是另一种输出。
    ///
    /// `ifconfig -a` 不加任何过滤是有意的：隧道判据在 flags 行上，`grep utun` 之类的过滤
    /// 会把 `gif0` 这种不叫 utun 的隧道整批滤掉（2026-09-08 那份抓取正是被 grep 滤过，
    /// 于是隧道那一半根本没进仓）。
    fn probe_macos(
        &self,
        own_interfaces: &[String],
    ) -> Result<TunnelProbeOutcome, SystemIntegrationError> {
        let route_v4 = self.run("netstat", &["-rn", "-f", "inet"])?;
        let route_v6 = self.run("netstat", &["-rn", "-f", "inet6"])?;
        let ifconfig = self.run("ifconfig", &["-a"])?;
        assemble_macos_probe(&route_v4, &route_v6, &ifconfig, own_interfaces)
            .map(TunnelProbeOutcome::Probed)
            .map_err(|e| SystemIntegrationError::route(e.to_string()))
    }

    /// Windows 腿：四条只读查询。
    ///
    /// `route print` 是查询式子命令（`add`/`delete`/`change` 才写内核），两条 cmdlet 是 `Get-`；
    /// 四条都不发包、不改状态、不需要管理员。argv 与
    /// `~/docs/polaris/scripts/polaris-collect-routes-windows.ps1` 的
    /// `@@@ROUTE_PRINT_4/6`、`@@@GET_NETIPADDRESS`、`@@@GET_NETADAPTER` 四段逐字对应。
    ///
    /// **为什么不用一条 `Get-NetRoute` 代掉两条 `route print` + 对照表**（权衡留档）：
    /// `Get-NetRoute` 一列就给出 `InterfaceAlias`，命令能少两条、也不碰本地化表头。
    /// 不选它的理由有二 —— ① 生产要用就得把它的解析器写成产品级（错误分支 / 变异收据 / 交叉验证
    /// 全要重来一遍），而 `route print` 那支已经有了；② portability 上并不划算：
    /// `Get-NetRoute` 与 `Get-NetIPAddress` 同属 NetTCPIP 模块，换过去省不掉对 cmdlet 的依赖
    /// （隧道判据那条 `Get-NetAdapter` 本来也是 cmdlet）。故保留 `route print` 这条更通用的读法，
    /// 把 `Get-NetRoute` 留在夹具里做交叉验证的参照。
    fn probe_windows(
        &self,
        own_interfaces: &[String],
    ) -> Result<TunnelProbeOutcome, SystemIntegrationError> {
        let route_v4 = self.run(&self.route_exe, &["print", "-4"])?;
        let route_v6 = self.run(&self.route_exe, &["print", "-6"])?;
        let addresses = self.run_powershell(PS_GET_NETIPADDRESS)?;
        let adapters = self.run_powershell(PS_GET_NETADAPTER)?;
        assemble_windows_probe(&route_v4, &route_v6, &addresses, &adapters, own_interfaces)
            .map(TunnelProbeOutcome::Probed)
            .map_err(|e| SystemIntegrationError::route(e.to_string()))
    }

    fn run_powershell(&self, script: &str) -> Result<String, SystemIntegrationError> {
        self.run(
            &self.powershell_exe,
            &["-NoProfile", "-NonInteractive", "-Command", script],
        )
    }
}

impl<R: CommandRunner> ForeignTunnelProbe for ForeignTunnelProbeImpl<R> {
    fn probe_foreign_tunnels(
        &self,
        own_interfaces: &[String],
    ) -> Result<TunnelProbeOutcome, SystemIntegrationError> {
        // 穷举 `match`：`Platform` 日后加一支时编译器逼这里表态，而不是让新平台静默落进
        // 某个兜底 —— 「兜底那一支恰好是『看过了没冲突』」正是本模块要防的形状。
        match self.platform {
            Platform::Linux => self.probe_linux(own_interfaces),
            Platform::Mac => self.probe_macos(own_interfaces),
            Platform::Win => self.probe_windows(own_interfaces),
            // 未知平台（freebsd/openbsd/…）：没有任何真机抓取，也就没有解析器。
            // 一条命令都不跑 —— 跑了也只是拿到一份读不懂的输出。
            Platform::Other => Ok(TunnelProbeOutcome::Unsupported(self.platform)),
        }
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

/// 三段 stdout → 探测事实（macOS；纯函数）。
///
/// 与 [`assemble_linux_probe`] 的两处不同，都是真实差异不是风格：
///
///  - **返回 `Result`**：`netstat` / `ifconfig` 的解析器会失败（截断的抓取、认不出的目的地），
///    而 `ip -o` 那侧是「坏行跳过」。一份少一半的 `Ok` 在这里会被读成事实。
///  - **`foreign` 里会有 link-local 与组播**：`netstat -rn` 打印的表比 `ip -o route show` 宽
///    —— 每个 utun 上都有 `fe80::/64`、`ff00::/8`、`ff01::/32`、`ff02::/32`。它们确实是那个
///    隧道宣告的路由（这里只负责把事实取回来），与 FakeIP / Mesh / TUN 三类判据面不相交，
///    故不影响 `builder::tunnel_conflict` 的判定；要不要在**展示**上收掉是渲染层的事，
///    本函数照旧把它们原样返回。
///
///    2026-09-12 真机实测（p101 / macOS 26.6.2 / Tailscale 断开）：本函数在那份抓取上返回
///    **36 条 foreign，全部**是这两族（10 个 utun 各 4 条），业务网段零条 —— 即「任何一台有
///    utun 的 mac 的常态」。谁算这两族由 [`is_link_local_or_multicast`] 单点判定，**应用**它
///    的是 `src-tauri` 的展示面，不是这里。
///
/// # Errors
///
/// 见 [`RouteTableParseError`]：路由表或接口表任一解析失败即整体失败，不产出半份事实。
pub fn assemble_macos_probe(
    netstat_v4_stdout: &str,
    netstat_v6_stdout: &str,
    ifconfig_stdout: &str,
    own_interfaces: &[String],
) -> Result<ForeignTunnelSnapshot, RouteTableParseError> {
    let mut routes = parse_netstat_routes(netstat_v4_stdout)?;
    routes.extend(parse_netstat_routes(netstat_v6_stdout)?);
    let tunnel_interfaces = parse_macos_tunnel_interfaces(ifconfig_stdout)?;
    let foreign = foreign_tunnel_routes(&routes, &tunnel_interfaces, own_interfaces);
    Ok(ForeignTunnelSnapshot {
        tunnel_interfaces,
        foreign,
    })
}

/// 四段 stdout → 探测事实（Windows；纯函数）。
///
/// 与 mac/Linux 那两支的真差异只有一处：路由表**要先有对照表**才落得到接口名上，
/// 故对照表解析失败时整次探测失败，绝不退成「路由有了、名字将就用 IP」——
/// 后者给出的是一份名字对不上任何隧道的路由表，也就是一句自信的「无冲突」。
///
/// # Errors
///
/// 见 [`RouteTableParseError`]：四段任一解析失败即整体失败，不产出半份事实。
pub fn assemble_windows_probe(
    route_print_v4_stdout: &str,
    route_print_v6_stdout: &str,
    get_netipaddress_stdout: &str,
    get_netadapter_stdout: &str,
    own_interfaces: &[String],
) -> Result<ForeignTunnelSnapshot, RouteTableParseError> {
    let names = parse_get_netipaddress(get_netipaddress_stdout)?;
    let mut routes = parse_route_print_routes(route_print_v4_stdout, &names)?;
    routes.extend(parse_route_print_routes(route_print_v6_stdout, &names)?);
    let tunnel_interfaces = parse_windows_tunnel_interfaces(get_netadapter_stdout)?;
    let foreign = foreign_tunnel_routes(&routes, &tunnel_interfaces, own_interfaces);
    Ok(ForeignTunnelSnapshot {
        tunnel_interfaces,
        foreign,
    })
}

#[cfg(test)]
mod tests;
