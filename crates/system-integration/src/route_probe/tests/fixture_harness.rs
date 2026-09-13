//! 真机抓取样本 → 解析器的验证 harness。
//!
//! # 它守的是什么
//!
//! 本模块的解析器必须照**真实抓取**写（见 `route_probe` 头注）。可「有没有真实抓取」这件事本身
//! 在代码里是看不见的：没有样本时，一个平台的解析腿可以干干净净地不存在，而**没有任何东西会喊**。
//! 这道 harness 就是那个喊的人 —— 它把「哪些平台有样本、哪些没有、有样本却没解析器的是哪些」
//! 变成每次 `cargo test` 都要回答一遍的问题。
//!
//! # 三条死规矩
//!
//! 1. **缺席必须逐字点名**：没有样本的平台在报告里出现「XXX 样本缺席」，不是绿过。
//! 2. **空 harness 不许绿**：一条样本都没解析到时直接红 —— 一个恒绿的空壳比没有 harness 更糟，
//!    它让人以为覆盖了。
//! 3. **畸形样本必须报错**：截断/空的抓取解析出来的"少一半的 Ok"会被下游读成事实，
//!    这与 [`TunnelProbeOutcome::Unsupported`] 要防的是同一个形状。
//!
//! # 夹具形态
//!
//! 见 `fixtures/README.md`。要点：文件名 `<平台>-<机器>-<抓的什么>-<日期>.txt`，
//! 内容按独占一行的 `@@@<分节ID>` 切段，段内**逐字**保留命令输出。

use crate::route_probe::{
    assemble_linux_probe, assemble_macos_probe, expand_netstat_destination, parse_get_netipaddress,
    parse_ip_link_names, parse_ip_routes, parse_macos_interface_flags,
    parse_macos_tunnel_interfaces, parse_netstat_routes, parse_route_print_routes,
    parse_vpn_connection_names, parse_windows_tunnel_interfaces, ForeignTunnelSnapshot, IpFamily,
    RouteEntry, RouteTableParseError, WindowsInterfaceNames, MAC_TUNNEL_FLAG,
};
use polaris_helper_proto::codec::is_valid_cidr;
use polaris_helper_proto::Platform;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

/// 夹具目录相对 crate 根的位置（锚在 crate 根，不锚在本文件 —— 理由见 `polaris-source-probe` 头注）。
const FIXTURES_REL: &str = "src/route_probe/tests/fixtures";

/// 采集脚本在「这条读法在这台机器上不可用」时写进分节体的哨兵行。
///
/// 有它，harness 才分得开「没抓」和「抓了但那台机器上没这条命令」；没它，两者都长成一个空分节。
const UNAVAILABLE: &str = "<<POLARIS-CAPTURE-UNAVAILABLE>>";

/// 2026-09-08 那份 macOS 抓取（只有 `netstat` 那一半）。多处断言直接钉在它身上，故抽成常量。
const MACOS_FIXTURE: &str = "macos-p101-netstat-rn-2026-09-08.txt";

/// 2026-09-12 的 macOS **全套**抓取（含未过滤的 `ifconfig -a`），Tailscale **未连接**。
const MACOS_FIXTURE_FULL: &str = "macos-p101-routes-ts-off-2026-09-12.txt";

/// 2026-09-12 的 macOS **连接态**抓取（`utun11` 连上自建 headscale）。
///
/// 三份 macOS 抓取里**唯一**带 tailnet 业务网段的一份 —— 「隧道宣告的业务网段被摘出来」
/// 那条链路的真样本就在它身上。⚠️ 它的 `@@@TAILSCALE` **仍是「命令不在 PATH 上」哨兵**
/// （Mac App Store 版不装 CLI）：判「这份是不是连接态」只能看路由 / `ifconfig`，不能看那个分节。
const MACOS_FIXTURE_TS_ON: &str = "macos-p101-routes-ts-on-2026-09-12.txt";

/// 2026-09-12 的 Windows 真机抓取（zh-CN / gb2312 控制台），**装 Tailscale 之前**。
const WINDOWS_FIXTURE: &str = "windows-w207-routes-ts-off-2026-09-12.txt";

/// 同一台机器、同一天，**装了 Tailscale for Windows 1.102.4 之后**的抓取
/// （Windows 11 build 26200；服务已起、`tailscale status` 逐字是 `Logged out.`）。
///
/// 它与 [`WINDOWS_FIXTURE`] 构成一对「有 / 无 wintun」的对照，适配器名单只差 `Tailscale`
/// 那一行 —— 隧道判据 `InterfaceType ∈ {131, 53}` 里 `53` 那一半的全部实测依据就在这一行上
/// （`the_three_windows_captures_differ_by_exactly_the_wintun_adapter` 拿它做差分）。
const WINDOWS_FIXTURE_WINTUN: &str = "windows-w207-wintun-present-2026-09-12.txt";

/// 同机同天、**wintun 已登录**（连上自建 headscale）那份抓取。
///
/// 它与 [`WINDOWS_FIXTURE_WINTUN`] 的适配器名单**完全相同**（`Tailscale` 仍是 ifType `53`、
/// `Up`）—— 差的全在路由表上：未登录时 wintun 上只有 3 条 link-local 噪声，登录后是 30 条
/// **业务**路由（25 条逐 peer 的 `/32` 主机路由 + `100.100.100.100/32` + 3 条 `fd7a:…` v6）。
/// 于是这一对是「噪声过滤该收什么、不该收什么」的天然正负样本。
const WINDOWS_FIXTURE_TS_ON: &str = "windows-w207-ts-on-2026-09-12.txt";

/// 2026-09-13 同机、**装了 OpenVPN 的 TAP-Windows6 适配器并已连上**那份抓取。
///
/// 它把「TAP-Windows / OpenVPN 那族驱动报什么 ifType」从**推测**（此前登记的疑值 `6`）
/// 变成**实测**：`OpenVPN TAP-Windows6` 的 `InterfaceType` 逐字是 **`53`**
/// （`ComponentID` = `root\tap0901`、`DriverDescription` = `TAP-Windows Adapter V9`、
/// `Status` = `Up`、`10.8.0.2`）—— 也就是说现有白名单 `{131, 53}` **本来就覆盖它**，
/// 不需要改判据；该改的是那条写错了的登记。
const WINDOWS_FIXTURE_TAP: &str = "windows-w207-ovpn-tun-connected-2026-09-13.txt";

/// 2026-09-13 同机、**装了 Hyper-V 之后**那份抓取 —— `53` 假阳性面的**负向证据**。
///
/// `vSwitch (Default Switch)` 与 `vEthernet (Default Switch)` 的 `InterfaceType` 都是 **`6`**，
/// 不是 `53`。此前「装了 Hyper-V / VMware / Docker 的机器上可能有别的适配器落进 `53` 这个
/// 宽桶」是一条**登记在案的未知风险**；这份抓取把 Hyper-V 那一半证否了。
///
/// 同一份抓取反向印证了另一件事：`6` **绝对不能**收进白名单 —— 它同时是物理网卡
/// （`Red Hat VirtIO Ethernet Adapter`）、Hyper-V 虚拟交换机、以及内核调试适配器。
const WINDOWS_FIXTURE_HYPERV: &str = "windows-w207-hyperv-present-2026-09-13.txt";

/// 2026-09-13 同机、**L2TP 连接已建立**那份抓取 —— Windows RAS 族的第一份真机样本。
///
/// 它坐实的缺陷：承载流量的接口以 VPN 连接名为别名（`PolarisProbeL2TP`，ifIndex 35，
/// 宣告 `0.0.0.0/0` metric 1），而 `Get-NetAdapter -IncludeHidden` 对该 index **返回 0 条** ——
/// ifType 白名单那条腿整个取材面看不到它。判据只能问 `Get-VpnConnection`。
///
/// 同一份里 `WAN Miniport (L2TP/IKEv2/SSTP/PPTP)` 的 ifType 实测 **131**、
/// `WAN Miniport (PPPOE)` 实测 **23**、`WAN Miniport (IP/IPv6/Network Monitor)` 实测 **6** ——
/// 「RAS 疑报 23」那条旧登记的真相：`23` 是 **PPPoE**（接入协议不是隧道），「不收 23」是对的。
const WINDOWS_FIXTURE_RAS: &str = "windows-w207-ras-l2tp-connected-2026-09-13.txt";

/// 虚拟化栈全家福（2026-09-13 w207）：Hyper-V 三型交换机（Default / Internal / Private）、
/// Tailscale(53)、TAP-Windows6(53)、四张 RAS 协议模板(131)、PPPoE(23)、物理网卡(6)
/// **同时在场**。目前取材面最全的一份 —— 「不误报」（虚拟网卡不进名单）与「不漏报」
/// （真隧道仍进名单）两个方向能从同一份输入上取。
///
/// 它证否的是 `53` 的假阳性面：Hyper-V 三型交换机全部报 **6**，一条都不沾 `53`。
/// 反向同时坐实「`6` 不可收」—— 同一台机器上 `6` 既是物理网卡（Red Hat VirtIO）、
/// 又是三个虚拟交换机扩展适配器、两个虚拟以太网、三张 WAN Miniport、内核调试适配器。
const WINDOWS_FIXTURE_VIRT: &str = "windows-w207-virtualization-stack-2026-09-13.txt";

/// 2026-09-13 VM185 上 OpenVPN 2.7 的 **ovpn-dco** 抓取（Linux 侧第一份真机夹具）。
///
/// 它证的是一件照记忆绝对写不出来的事：**设备名叫 `tun0` / `tun1`，link type 却是 `ovpn`**。
/// `ip -o link show type tun` 在这台机器上只回 `tap0`，两条 OpenVPN 隧道一条都查不到。
const LINUX_FIXTURE_OVPN_DCO: &str = "linux-vm185-ovpn-dco-2026-09-13.txt";

/// 2026-09-13 的 macOS **交叉对差**抓取：`netstat -rn` 之外再要一路 `route -n get` 的读数。
///
/// `route -n get` 走 PF_ROUTE 的 `RTM_GET`，与 `netstat -rn` 的路由表 dump 是**不同的内核
/// 接口** —— 这是 macOS 侧第一个能把解析器顶红的独立读数（Windows 侧一直有
/// `route print` 与 `Get-NetRoute` 两路，mac 侧此前只有一路）。
const MACOS_FIXTURE_ROUTE_GET: &str = "macos-p101-route-get-ts-on-2026-09-13.txt";

/// **Parallels Desktop 运行中**的抓取（2026-09-13 p101）。虚拟化是常规场景，
/// 判据要在那种环境下被验证过 —— 这份是 macOS 侧的输入面。
///
/// PD 18+ 建的是 `vmenet0/1/2` + `bridge100/101/102`（不是老版本的 `vnic*`），
/// 六个**全是 `BROADCAST` 型、零 `POINTOPOINT`** ⇒ 判据一个都不收。
/// 这份同时带着 Tailscale 连接态（utun11），所以"不误报"与"不漏报"能从同一份输入上取。
const MACOS_FIXTURE_PARALLELS: &str = "macos-p101-parallels-running-2026-09-13.txt";

/// **连接态**的 macOS 抓取：`(文件名, tailnet 网段总条数, 其中挂在 utun11 上的条数)`。
///
/// 写成一张表而不是一个「是不是连接态」的布尔：两份抓取隔了一天，tailnet 里的 peer 数就不同了
/// （29/28 → 32/31）。拿同一组数字去套两份会红得没有信息量，而把数字整个删掉又会让
/// 「换样本时提醒重核」这件事消失。逐份登记是唯一同时保住这两样的写法。
///
/// 不在这张表里的 macOS 抓取 = 断开态，一条 tailnet 网段都不许有（噪声过滤的负样本）。
const MACOS_CONNECTED_CAPTURES: &[(&str, usize, usize)] = &[
    (MACOS_FIXTURE_TS_ON, 29, 28),
    (MACOS_FIXTURE_ROUTE_GET, 32, 31),
    // PD 运行中那份与前一份同日、同 tailnet ⇒ 同样是 32/31（差的那一条是 `fd7a:…::57/128`
    // 落在 `lo0` 上：本机自己的 tailnet 地址，不属于 utun11）。
    (MACOS_FIXTURE_PARALLELS, 32, 31),
];

/// 判据②专用的**合成**样本：把上面那份的本地化表头换成 en-US，路由数据行一字节不动。
/// 它在 `synthetic/` 子目录里，故不进 [`scan`] 的正常取材面（扫描只读直接子项）。
const WINDOWS_SYNTHETIC_EN: &str = "synthetic/windows-w207-route-print-en-headers.txt";

/// harness 登记在册的平台：(文件名前缀, 报告里的名字, 采集脚本)。
///
/// 加一个平台 = 在这里加一行。漏掉的那个会在覆盖测试里被点名，而不是悄悄不在名单上 ——
/// 「清单式取材在有人加了个东西的那天无声失去覆盖」是本仓吃过的亏。
const PLATFORMS: &[(&str, &str, &str)] = &[
    (
        "macos",
        "macOS",
        "~/docs/polaris/scripts/polaris-collect-routes-macos.sh",
    ),
    (
        "windows",
        "Windows",
        "~/docs/polaris/scripts/polaris-collect-routes-windows.ps1",
    ),
    // ⚠️ Linux 侧**还没有采集脚本**（mac/win 那两条有）。如实写成手抓步骤，而不是指一个
    // 不存在的路径 —— 「缺席被点名」这套报告的价值全在它说的话是真的。
    (
        "linux",
        "Linux",
        "（暂无采集脚本，手抓：`ip -o route show` / `ip -o -6 route show` / \
         `ip -o link show type {tun,wireguard,ovpn}` / `ip -d link show` / `ip -br addr`）",
    ),
];

/// Linux 的三条 `ip -o link show type <T>` 查询各自的分节 ID。
///
/// 顺序与 `probe_linux` 的查询序一致。**单条为空是合法的**（那台机器没装 wireguard / ovpn
/// 模块，`ip link show type <未知>` 返回空 + rc=0）—— 「非空」这条只在三者的**并集**上成立，
/// 故 [`absorb_file`] 按文件断一次，不按分节断。
const LINUX_LINK_SECTIONS: &[&str] = &["LINK_TUN", "LINK_WIREGUARD", "LINK_OVPN"];

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURES_REL)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

pub(super) fn read_fixture(rel: &str) -> String {
    let path = fixtures_dir().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读夹具 {}：{e}", path.display()))
}

/// 夹具目录里**全部** Windows 抓取的文件名（枚举出来的，不是写死某一份）。
///
/// 🔴 上一版的 wintun 绊线把取材面写死成单个文件名，于是第二份 Windows 抓取**带着 wintun**
/// 进来那天它压根没看那份文件、也就没红（真正红的是另一条「缺口条数」断言）。绊线自己也得守
/// 「取材面宽于意图面」：凡是「仓里还没有 X 的样本」这类否定型登记，取材面必须是**全部**样本。
fn fixture_names_starting_with(prefix: &str, must_include: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(fixtures_dir())
        .expect("读夹具目录")
        .map(|e| e.expect("夹具目录项").path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "txt"))
        .map(|p| file_name(&p))
        .filter(|n| n.starts_with(prefix))
        .collect();
    names.sort();
    // 正向对照：点名引用的抓取都确实在里面，否则下面按名字取夹具的断言会在
    // 「文件被改名了」这件事上静默失去输入。
    for known in must_include {
        assert!(
            names.iter().any(|n| n == known),
            "点名引用的 `{known}` 不在夹具目录里（改名了？）：{names:?}"
        );
    }
    names
}

fn windows_fixture_names() -> Vec<String> {
    fixture_names_starting_with(
        "windows-",
        &[
            WINDOWS_FIXTURE,
            WINDOWS_FIXTURE_WINTUN,
            WINDOWS_FIXTURE_TS_ON,
            WINDOWS_FIXTURE_TAP,
            WINDOWS_FIXTURE_HYPERV,
            WINDOWS_FIXTURE_RAS,
            WINDOWS_FIXTURE_VIRT,
        ],
    )
}

fn macos_fixture_names() -> Vec<String> {
    fixture_names_starting_with(
        "macos-",
        &[
            MACOS_FIXTURE,
            MACOS_FIXTURE_FULL,
            MACOS_FIXTURE_TS_ON,
            MACOS_FIXTURE_ROUTE_GET,
            MACOS_FIXTURE_PARALLELS,
        ],
    )
}

fn linux_fixture_names() -> Vec<String> {
    fixture_names_starting_with("linux-", &[LINUX_FIXTURE_OVPN_DCO])
}

/// 这条前缀是不是 tailnet（自建 headscale）装上去的**业务**网段 —— 按抓取里**脱敏后**的形态写。
///
/// 三种形态都在 2026-09-12 的连接态抓取里逐字出现过：
///  - `32.0.0.x/32`：逐 peer 的**主机路由**（脱敏把 `100.64.0.0/10` 映到了这里）。
///    🔴 Tailscale 装的就是一条条 `/32`，**不是**一条 `32.0.0.0/24` 汇总段 ——
///    按「找一条汇总前缀」的直觉写断言会全落空；
///  - `fd7a:115c:a1e0::/48` 与它下面的 `/128`：Tailscale 固定的 ULA 段（未脱敏）。
///    它住在 `fc00::/7` 里，所以「把私网一起当噪声收掉」的写法会把它误杀 ——
///    [`crate::route_probe::LINK_LOCAL_AND_MULTICAST_BLOCKS`] 里一个私网块都没有正是为了它；
///  - `100.100.100.100/32`：MagicDNS 解析器地址，也是 tailnet 装上去的业务路由。
fn is_tailnet_prefix(prefix: &str) -> bool {
    prefix.starts_with("32.0.0.")
        || prefix.starts_with("fd7a:115c:a1e0")
        || prefix.starts_with("100.100.100.100")
}

/// 测试侧**独立切列**的 `Get-NetAdapter` 表：列名 → 列号 + 切好的数据行。
///
/// 切法与生产实现无关（同 `Get-NetRoute` 那条交叉验证的口径）：复用生产的切列函数等于两边
/// 共用同一个错，拿它去核判据的**依据**就没有检出力了。
struct AdapterTable {
    header: Vec<String>,
    rows: Vec<Vec<String>>,
}

impl AdapterTable {
    /// 列号；列不在表头里当场 panic（断言写错列名时说出来，不是静默拿到 0 号列）。
    fn col(&self, name: &str) -> usize {
        self.header
            .iter()
            .position(|c| c == name)
            .unwrap_or_else(|| panic!("`{name}` 列不在表头里：{:?}", self.header))
    }

    fn cell<'a>(&self, row: &'a [String], name: &str) -> &'a str {
        row[self.col(name)].as_str()
    }

    /// `InterfaceType` 恰好是 `want` 的那些行。
    fn rows_with_if_type(&self, want: u32) -> Vec<&Vec<String>> {
        let want = want.to_string();
        self.rows
            .iter()
            .filter(|r| self.cell(r, "InterfaceType") == want)
            .collect()
    }

    /// 某一行的全文（别名 + 驱动名 + ComponentID 都在里面）—— 供「这是不是已知的某族驱动」用。
    fn row_text(&self, row: &[String]) -> String {
        row.join(" ")
    }
}

/// 按**列头行 + 分隔线行**切 `Format-Table` 输出的列。
///
/// 列号必须从分隔线读，不能按第 N 个 token 取：`Teredo Tunneling Pseudo-Interface` 光别名
/// 就占三个 token，按 token 位置取 `InterfaceType` 必错。
fn cut_adapter_table(adapters: &str) -> AdapterTable {
    let lines: Vec<&str> = adapters.lines().collect();
    let head = lines
        .iter()
        .position(|l| l.contains("InterfaceAlias") && l.contains("InterfaceType"))
        .expect("列头行（含 InterfaceAlias 与 InterfaceType）");
    let mut starts = Vec::new();
    let mut prev = false;
    for (i, c) in lines[head + 1].chars().enumerate() {
        if c == '-' && !prev {
            starts.push(i);
        }
        prev = c == '-';
    }
    let cut = |line: &str| -> Vec<String> {
        let cs: Vec<char> = line.chars().collect();
        starts
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let e = starts.get(i + 1).copied().unwrap_or(cs.len()).min(cs.len());
                cs[(*s).min(e)..e]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_string()
            })
            .collect()
    };
    AdapterTable {
        header: cut(lines[head]),
        rows: lines[head + 2..]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| cut(l))
            .collect(),
    }
}

/// 取某个分节的**分节体**；没有该分节直接 panic（断言写错了分节名时当场说出来，不是静默空串）。
pub(super) fn section(raw: &str, id: &str) -> String {
    section_opt(raw, id).unwrap_or_else(|| panic!("样本里没有 @@@{id} 分节"))
}

/// 取某个分节的**分节体**，没有该分节时 `None`。
///
/// 与 [`section`] 的区别只在缺席的处置：这一支给调用方自己决定。**默认仍该用 [`section`]** ——
/// 只有「采集脚本后来才加的分节，老夹具里天然没有」这一种情形才走这里，且调用方必须说清楚
/// 它把缺席当成什么。
pub(super) fn section_opt(raw: &str, id: &str) -> Option<String> {
    split_sections(raw)
        .into_iter()
        .find(|(sid, _)| sid == id)
        .map(|(_, body)| body)
}

/// 把一份抓取切成 `(分节 ID, 分节体)`。
///
/// 分节标记是**独占一行**的 `@@@<ID>`（`A-Z` / `0-9` / `_`）。首个标记之前是给人读的抬头
/// （隐私提示、classful 警告），不属于任何分节，丢弃。
///
/// 分节体**逐字**保留：不 trim、不规整、原样带着行尾空格与原始行终止符。行尾空格是
/// `netstat` 空 Expire 列的样子，CRLF 是 Windows 抓取的样子 —— 解析器在真机上读到的就是它们，
/// harness 在这里把它们规整掉，等于把夹具的全部价值抹掉。
fn split_sections(raw: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;
    for line in raw.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if let Some(id) = trimmed.strip_prefix("@@@") {
            let looks_like_marker = !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
            if looks_like_marker {
                if let Some(done) = current.take() {
                    out.push(done);
                }
                current = Some((id.to_string(), String::new()));
                continue;
            }
        }
        if let Some((_, body)) = current.as_mut() {
            body.push_str(line);
        }
    }
    if let Some(done) = current.take() {
        out.push(done);
    }
    out
}

/// 一份抓取里，harness 认识的分节类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SectionKind {
    /// macOS `netstat -rn`（v4 / v6 共用一个解析器）。
    MacNetstatRoutes,
    /// macOS `ifconfig -a` → 隧道接口。
    MacTunnelInterfaces,
    /// Windows `route print`。**取材面是整份抓取**：还要同一份里的 `@@@GET_NETIPADDRESS`
    /// 才落得到接口名上（Windows 与 mac/Linux 的真差异，见 `WindowsInterfaceNames`）。
    WinRoutePrint,
    /// Windows 的 IP/索引 → 接口名对照表。
    WinInterfaceNames,
    /// Windows 适配器枚举。
    WinTunnelInterfaces,
    /// Linux `ip -o route show` / `ip -o -6 route show`（v4 / v6 共用一个解析器）。
    LinuxIpRoutes,
    /// Linux `ip -o link show type <T>` → 隧道接口名（三条查询各一个分节，见 [`LINUX_LINK_SECTIONS`]）。
    LinuxTunnelLinks,
    /// Windows `Get-VpnConnection`（两个作用域各一个分节）→ 已连接的 VPN 连接名。
    ///
    /// 它是**判据取材面**而不是备查：RAS 族的承载接口根本不在 `Get-NetAdapter` 里。
    WinVpnConnections,
    /// 采集回来**备查**、不是判据取材面的分节。
    ReferenceOnly,
}

/// 分节 ID → 谁来解析它。
///
/// `GET_NETIPADDRESS` 在 2026-09-12 之后**不再是备查**：Windows 路由表的「接口」列是本地 IP /
/// 接口索引，没有这张表就落不到接口名上 —— 它是判据取材面的一部分。
///
/// `GET_NETROUTE_*` 仍列备查：它一列就给出 `InterfaceAlias`，比 `route print` 好解析得多，
/// 但 Windows 的生产读法定在 `route print`（原生 exe，任何 SKU 都有；`Get-NetRoute` 依赖
/// NetTCPIP 模块，Server Core / 精简版可能没有）。留它在，是为了下一份抓取能跟 `route print`
/// 的解析结果对差。
///
/// **同一条命令在两代采集脚本里叫过两个名字**（mac 侧 `V4`/`V6`/`IFCONFIG` →
/// `V4_NETSTAT`/`V6_NETSTAT`/`IFCONFIG_FLAGS`）。两套都认，别名表在 [`MACOS_NETSTAT_V4`] 一族里 ——
/// 只认新名字会让老夹具悄悄失去覆盖，只认老名字会让新夹具躺在目录里没人解析。
fn classify(section_id: &str) -> Option<SectionKind> {
    match section_id {
        "V4" | "V6" | "V4_NETSTAT" | "V6_NETSTAT" => Some(SectionKind::MacNetstatRoutes),
        "IFCONFIG" | "IFCONFIG_FLAGS" => Some(SectionKind::MacTunnelInterfaces),
        "ROUTE_PRINT_4" | "ROUTE_PRINT_6" => Some(SectionKind::WinRoutePrint),
        "GET_NETIPADDRESS" => Some(SectionKind::WinInterfaceNames),
        "GET_NETADAPTER" | "NETSH_INTERFACE" => Some(SectionKind::WinTunnelInterfaces),
        "GET_VPNCONNECTION" | "GET_VPNCONNECTION_ALLUSER" => Some(SectionKind::WinVpnConnections),
        "V4_ROUTE" | "V6_ROUTE" => Some(SectionKind::LinuxIpRoutes),
        id if LINUX_LINK_SECTIONS.contains(&id) => Some(SectionKind::LinuxTunnelLinks),
        // `LINK_DETAIL`（`ip -d link show`）是**证据**不是取材面：ovpn-dco 那条结论
        // （`tun0` 的第三行是 `ovpn addrgenmode …`、`tap0` 的是 `tun type tap …`）就是从它读出来的，
        // 但生产判据只认 `ip -o link show type <T>` 的名单，不解析 detail。
        // `V4_ROUTE_GET` / `V6_ROUTE_GET` 同理：它们是 mac 侧的**第二读数**，由
        // `macos_netstat_reading_agrees_with_the_independent_route_get_reading` 单独消费。
        "META" | "ENV" | "NETSTAT_I" | "NETWORKSETUP_ORDER" | "TAILSCALE" | "IPCONFIG"
        | "GET_NETROUTE_4" | "GET_NETROUTE_6" | "NETSH_INTERFACE_6" | "LINK_DETAIL" | "ADDR"
        | "V4_ROUTE_GET" | "V6_ROUTE_GET" | "GET_NETIPINTERFACE" | "END" => {
            Some(SectionKind::ReferenceOnly)
        }
        _ => None,
    }
}

/// 路由表分节 → 它的地址族。
///
/// 分节名就是**产生这份输出的那条命令**的记号（`@@@V4` = `netstat -rn -f inet`、
/// `@@@V6_ROUTE` = `ip -o -6 route show`），族写在名字里而不在内容里 ——
/// 这正是 [`IpFamily`] 要求调用方传的那个真值，见它的头注。
fn section_family(section_id: &str) -> IpFamily {
    if section_id.contains('6') {
        IpFamily::V6
    } else {
        IpFamily::V4
    }
}

/// macOS `netstat -rn -f inet` 那一段在两代采集脚本里的分节名（新的在前）。
const MACOS_NETSTAT_V4: &[&str] = &["V4_NETSTAT", "V4"];
/// 同上，v6。
const MACOS_NETSTAT_V6: &[&str] = &["V6_NETSTAT", "V6"];
/// macOS `ifconfig -a` 那一段在两代采集脚本里的分节名（新的在前）。
const MACOS_IFCONFIG: &[&str] = &["IFCONFIG_FLAGS", "IFCONFIG"];

/// 取一份抓取里**第一个存在**的候选分节；一个都没有就当场 panic 并把候选名列出来。
///
/// 存在的理由是分节改过名：写死单个名字的调用方会在换代那天 panic 成「样本里没有 @@@V4 分节」，
/// 而真相是它改叫 `@@@V4_NETSTAT` 了 —— 那是个**误导性**的错误消息，会让人去查夹具而不是查别名表。
pub(super) fn section_any(raw: &str, candidates: &[&str]) -> String {
    let sections = split_sections(raw);
    candidates
        .iter()
        .find_map(|id| {
            sections
                .iter()
                .find(|(sid, _)| sid == id)
                .map(|(_, body)| body.clone())
        })
        .unwrap_or_else(|| panic!("样本里 {candidates:?} 这几个分节一个都没有"))
}

/// 一次扫描的结果。
#[derive(Debug, Default)]
struct Scan {
    /// 真的解析出了东西的分节：`(文件名, 分节 ID, 条目数)`。
    parsed: Vec<(String, String, usize)>,
    /// 逐条说明「这里没有覆盖，以及为什么」—— 恒绿空 harness 的解药。
    notes: Vec<String>,
}

impl Scan {
    fn note(&mut self, note: String) {
        self.notes.push(note);
    }
}

fn scan() -> Scan {
    scan_dir(&fixtures_dir())
}

/// 扫某个目录。**参数化不是为了灵活**：`absent_platform_samples_are_named_not_silently_green`
/// 的负向对照要拿一个**空目录**喂进来，证明「缺席被点名」那一支真的会发。两个平台的样本都
/// 到齐之后，那一支在真夹具目录上再也不发了 —— 没有这个参数，它就退化成一句永远轮不到执行的话。
fn scan_dir(dir: &Path) -> Scan {
    // 只取目录的**直接子项**里的 `.txt`：`malformed/`（故意的坏样本）与 `synthetic/`（合成变体）
    // 都不进正常取材面（`read_dir` 不递归 + `is_file()` 过滤，子目录天然出局）。
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("读夹具目录 {}：{e}", dir.display()))
        .map(|e| e.expect("夹具目录项").path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "txt"))
        .collect();
    files.sort();

    let mut scan = Scan::default();

    for (prefix, display, script) in PLATFORMS {
        let tag = format!("{prefix}-");
        let mine: Vec<&PathBuf> = files
            .iter()
            .filter(|p| file_name(p).starts_with(&tag))
            .collect();
        if mine.is_empty() {
            scan.note(format!(
                "{display} 样本缺席：{FIXTURES_REL}/ 下没有 `{prefix}-*.txt`。\
                 到现场机上跑 {script}，把产出的 .txt 原样放进来即可 —— 别手工规整。"
            ));
            continue;
        }
        for path in mine {
            absorb_file(path, &mut scan);
        }
    }

    // 认不出平台前缀的文件同样点名，别静静躺在目录里假装被覆盖了。
    for path in &files {
        let name = file_name(path);
        if !PLATFORMS
            .iter()
            .any(|(p, _, _)| name.starts_with(&format!("{p}-")))
        {
            scan.note(format!(
                "未知平台前缀：{name} —— 命名约定见 {FIXTURES_REL}/README.md"
            ));
        }
    }

    scan
}

/// 同一份抓取里的 `@@@GET_NETIPADDRESS` → 接口名对照表。
///
/// `None` = 这份抓取里没有这个分节（或它被采集脚本标成不可用）。
fn windows_interface_names(
    sections: &[(String, String)],
) -> Option<Result<WindowsInterfaceNames, RouteTableParseError>> {
    sections
        .iter()
        .find(|(id, _)| id == "GET_NETIPADDRESS")
        .filter(|(_, body)| !body.contains(UNAVAILABLE))
        .map(|(_, body)| parse_get_netipaddress(body))
}

fn absorb_file(path: &Path, scan: &mut Scan) {
    let file = file_name(path);
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {file}：{e}"));
    let sections = split_sections(&raw);
    assert!(
        !sections.is_empty(),
        "{file}：一个 `@@@分节` 标记都没有 —— 形态见 {FIXTURES_REL}/README.md"
    );

    // **先**把对照表建起来，再按序解析各分节：Windows 路由表的「接口」列是本地 IP / 接口索引，
    // 对照表在**同一份抓取的另一个分节**里，而且排在 `@@@ROUTE_PRINT_*` **后面**。
    // 一边走一边攒的写法会在解析路由表时手上还没有表 —— 那时唯一能做的就是「先跳过」，
    // 而跳过的结局是一份空的路由表，长得正好像「这台机器没有路由」。
    let names = windows_interface_names(&sections);

    // Linux 的隧道名单散在三条查询里，**任何单条都可以合法为空**（没装 wireguard / ovpn 模块）。
    // 「非空」这条只在并集上成立，故按文件断一次；按分节断会把「这台机器没有这种隧道」
    // 误报成抓漏了，而那正是下一个人会照着去重跑一趟现场的那种假缺口。
    let linux_link_bodies: Vec<&String> = sections
        .iter()
        .filter(|(id, _)| LINUX_LINK_SECTIONS.contains(&id.as_str()))
        .map(|(_, body)| body)
        .collect();
    if !linux_link_bodies.is_empty() {
        let union: Vec<String> = linux_link_bodies
            .iter()
            .flat_map(|b| parse_ip_link_names(b))
            .collect();
        assert!(
            !union.is_empty(),
            "{file}：三条 `ip -o link show type …` 的并集是空的 —— \
             空名单会被 `foreign_tunnel_routes` 读成「这台机器上没有隧道」"
        );
    }

    for (id, body) in &sections {
        if body.contains(UNAVAILABLE) {
            scan.note(format!(
                "{file} 的 @@@{id}：采集时这条读法在那台机器上不可用（脚本已如实登记，不是漏抓）"
            ));
            continue;
        }
        let Some(kind) = classify(id) else {
            scan.note(format!(
                "{file} 的 @@@{id}：harness 不认识这个分节，没有人解析它"
            ));
            continue;
        };
        match kind {
            SectionKind::ReferenceOnly => {
                scan.note(format!("{file} 的 @@@{id}：采集回来备查，不是判据取材面"));
            }
            SectionKind::MacNetstatRoutes => {
                absorb_routes(
                    scan,
                    &file,
                    id,
                    parse_netstat_routes(body, section_family(id)),
                    NameStyle::Unixish,
                );
            }
            SectionKind::WinRoutePrint => match &names {
                Some(Ok(table)) => absorb_routes(
                    scan,
                    &file,
                    id,
                    parse_route_print_routes(body, table),
                    NameStyle::WindowsAlias,
                ),
                Some(Err(e)) => panic!(
                    "{file} 的 @@@GET_NETIPADDRESS 解析失败，@@@{id} 就落不到接口名上：{e}"
                ),
                None => scan.note(format!(
                    "{file} 的 @@@{id}：同一份抓取里没有 @@@GET_NETIPADDRESS —— Windows 路由表的                     「接口」列是本地 IP / 接口索引，没有对照表定不出接口名，故不解析（宁可缺席也不给半对的名字）"
                )),
            },
            SectionKind::WinInterfaceNames => match &names {
                Some(Ok(table)) => {
                    assert!(
                        !table.is_empty(),
                        "{file} 的 @@@{id}：对照表解析出 0 条 —— 路由表会全军覆没在「接口查不到」上"
                    );
                    scan.parsed.push((file.clone(), id.clone(), table.len()));
                }
                Some(Err(e)) => panic!("{file} 的 @@@{id} 解析失败：{e}"),
                None => unreachable!("本分节存在 ⇒ windows_interface_names 必然是 Some"),
            },
            SectionKind::WinTunnelInterfaces => {
                // Windows 的隧道名同样是 `InterfaceAlias`：`Teredo Tunneling Pseudo-Interface`
                // 合法地带空格，与路由那侧同一个命名空间。
                absorb_interfaces(
                    scan,
                    &file,
                    id,
                    parse_windows_tunnel_interfaces(body),
                    NameStyle::WindowsAlias,
                );
            }
            SectionKind::MacTunnelInterfaces => {
                absorb_interfaces(
                    scan,
                    &file,
                    id,
                    parse_macos_tunnel_interfaces(body),
                    NameStyle::Unixish,
                );
            }
            SectionKind::LinuxIpRoutes => {
                // `parse_ip_routes` 是「坏行跳过」而不是 `Result`（与 mac/win 那两支的真差异，
                // 见 `assemble_macos_probe` 的头注）⇒ 这里包成 `Ok` 只是接上同一个形状断言，
                // 不是把错误吞掉：`ip -o` 那侧压根没有「解析失败」这个结局。
                absorb_routes(
                    scan,
                    &file,
                    id,
                    Ok(parse_ip_routes(body, section_family(id))),
                    NameStyle::Unixish,
                );
            }
            SectionKind::WinVpnConnections => {
                let parsed = parse_vpn_connection_names(body)
                    .unwrap_or_else(|e| panic!("{file} 的 @@@{id} 解析失败：{e}"));
                for n in &parsed {
                    assert_interface_name_shape(n, NameStyle::WindowsAlias, &file, id);
                }
                // **空是合法的**（这个作用域里没有已连接的 VPN 连接，夹具的 ALLUSER 段正是空的），
                // 故不走 `absorb_interfaces` —— 它的「解析出 0 个接口」那条断言在这里是错的判据。
                // 空分节与 `<<POLARIS-CAPTURE-UNAVAILABLE>>`（cmdlet 不存在）由**上面**那道
                // 哨兵分支分开，不在这里混。
                scan.parsed.push((file.clone(), id.clone(), parsed.len()));
            }
            SectionKind::LinuxTunnelLinks => {
                let parsed = parse_ip_link_names(body);
                for n in &parsed {
                    assert_interface_name_shape(n, NameStyle::Unixish, &file, id);
                }
                // 空是合法的（非空只在三条的并集上断，见本函数开头），故这里**不**走
                // `absorb_interfaces` —— 它的「解析出 0 个接口」那条断言在这里是错的判据。
                scan.parsed.push((file.clone(), id.clone(), parsed.len()));
            }
        }
    }
}

/// 接口名活在哪个命名空间里 —— 决定「名字里能不能有空格」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameStyle {
    /// BSD / Linux 的设备名：`en0` / `utun3` / `tailscale0` —— 绝不含空白。
    Unixish,
    /// Windows 的 `InterfaceAlias`：`以太网` / `Loopback Pseudo-Interface 1`
    /// —— **合法地含空格**，这是真机事实不是脏数据。
    WindowsAlias,
}

/// 接口名的形状断言。
///
/// 「不含空白」这条在 mac/Linux 上成立，在 Windows 上**必须放宽**：
/// `Loopback Pseudo-Interface 1` 是那台机器上真实的 `InterfaceAlias`，`route print -6` 在
/// 它上面有两条路由。放宽的同时补两条**更贴近真实失效模式**的：
///
///  - 名字不许是一个纯**整数** —— 那是 `route print -6` 的 `If`（接口索引）列漏出来了；
///  - 名字不许是一个 **IP 字面量** —— 那是 `route print -4` 的 `Interface`（本地 IP）列漏出来了。
///
/// 这两条对三个平台都开。也就是说这次改动在「对照表没做 / 做错了」这个维度上**比原来更强**，
/// 不是拿宽松换绿：原来的「不含空白」对 `8` 和 `192.168.10.207` 这两个错答案是全绿的。
fn assert_interface_name_shape(name: &str, style: NameStyle, file: &str, id: &str) {
    assert!(
        !name.is_empty(),
        "{file} 的 @@@{id}：出现了空接口名 —— 列取错了？"
    );
    assert_eq!(
        name.trim(),
        name,
        "{file} 的 @@@{id}：接口名 `{name}` 两端带空白（切列没 trim）"
    );
    assert!(
        !name.contains(['\t', '\n', '\r']),
        "{file} 的 @@@{id}：接口名 `{name}` 里有制表符/换行 —— 跨行拼接了？"
    );
    assert!(
        name.parse::<u32>().is_err(),
        "{file} 的 @@@{id}：接口名 `{name}` 是个纯整数 —— 这是 IPv6 路由表的 `If`（接口索引）列，\
         说明 IP/索引 → 接口名的对照没生效"
    );
    assert!(
        name.parse::<IpAddr>().is_err(),
        "{file} 的 @@@{id}：接口名 `{name}` 是个 IP 字面量 —— 这是 IPv4 路由表的 `Interface`\
         （本地 IP）列，说明 IP/索引 → 接口名的对照没生效"
    );
    if style == NameStyle::Unixish {
        assert!(
            !name.contains(char::is_whitespace),
            "{file} 的 @@@{id}：接口名 `{name}` 含空白 —— BSD/Linux 的设备名不长这样，列号取错了？"
        );
    }
}

/// 四种结局各归各位：解析出东西 → 断形状；样本没到 / 抓取缺列 → 登记待办；真解析失败 → 当场红。
///
/// 中间那两支不能折成「跳过」：样本已经躺在仓里、却没有任何人读它，是比「没有样本」更隐蔽的缺口；
/// 而「样本在、缺的是某一列」与「整份样本都没有」该做的事不同，混成一支下一个人会白跑一趟现场。
fn absorb_routes(
    scan: &mut Scan,
    file: &str,
    id: &str,
    result: Result<Vec<RouteEntry>, RouteTableParseError>,
    style: NameStyle,
) {
    match result {
        Ok(entries) => {
            assert_route_shape(&entries, file, id, style);
            scan.parsed
                .push((file.to_string(), id.to_string(), entries.len()));
        }
        Err(RouteTableParseError::SampleMissing { needed, .. }) => scan.note(format!(
            "{file} 的 @@@{id}：**样本已到、解析器还没写** —— 需要 {needed}"
        )),
        Err(RouteTableParseError::CaptureIncomplete {
            missing, needed, ..
        }) => scan.note(format!(
            "{file} 的 @@@{id}：**抓取不全，缺 {missing}** —— 重抓方式：{needed}"
        )),
        Err(e) => panic!("{file} 的 @@@{id} 解析失败：{e}"),
    }
}

fn absorb_interfaces(
    scan: &mut Scan,
    file: &str,
    id: &str,
    result: Result<Vec<String>, RouteTableParseError>,
    style: NameStyle,
) {
    match result {
        Ok(names) => {
            assert!(
                !names.is_empty(),
                "{file} 的 @@@{id}：解析出 0 个接口 —— 空列表会被下游读成「这台机器上没有隧道」"
            );
            for n in &names {
                assert_interface_name_shape(n, style, file, id);
            }
            scan.parsed
                .push((file.to_string(), id.to_string(), names.len()));
        }
        Err(RouteTableParseError::SampleMissing { needed, .. }) => scan.note(format!(
            "{file} 的 @@@{id}：**样本已到、解析器还没写** —— 需要 {needed}"
        )),
        Err(RouteTableParseError::CaptureIncomplete {
            missing, needed, ..
        }) => scan.note(format!(
            "{file} 的 @@@{id}：**抓取不全，缺 {missing}** —— 重抓方式：{needed}"
        )),
        Err(e) => panic!("{file} 的 @@@{id} 解析失败：{e}"),
    }
}

/// 断言的是**形状**不是内容：换一台机器抓回来的样本仍然要过同一组断言。
///
/// 顺序有讲究：先断具体形态（`%作用域`残留 / classful 没展开），最后才用 `is_valid_cidr` 兜底。
/// 反过来写的话，前两条永远轮不到执行（`is_valid_cidr` 已经把它们否掉了），
/// 就成了两条恒真断言 —— 看着有、其实不做事。
fn assert_route_shape(entries: &[RouteEntry], file: &str, id: &str, style: NameStyle) {
    assert!(
        !entries.is_empty(),
        "{file} 的 @@@{id}：解析出 0 条路由 —— 空结果长得像「看过了，没有冲突」，正是本模块要防的形状"
    );
    for e in entries {
        assert_interface_name_shape(&e.interface, style, file, id);
        let (addr, _len) = e
            .prefix
            .split_once('/')
            .unwrap_or_else(|| panic!("{file} 的 @@@{id}：前缀 `{}` 没有 `/`：{e:?}", e.prefix));
        assert!(
            !addr.contains('%'),
            "{file} 的 @@@{id}：前缀 `{}` 里还留着 `%作用域`：{e:?}",
            e.prefix
        );
        if !addr.contains(':') {
            assert_eq!(
                addr.split('.').count(),
                4,
                "{file} 的 @@@{id}：前缀 `{}` 是 classful 缩写，没展开成四段：{e:?}",
                e.prefix
            );
        }
        assert!(
            is_valid_cidr(&e.prefix),
            "{file} 的 @@@{id}：前缀 `{}` 过不了 CIDR 校验：{e:?}",
            e.prefix
        );
    }
}

// ══════════ 判据 1：缺席必须被点名 ══════════

/// 🔴 **判据 1**：没有真机抓取的平台，harness 逐字说出「XXX 样本缺席」，不是绿过。
///
/// 2026-09-12 两个平台的抓取都到齐了，于是**真夹具目录上一个平台都不该被报缺席** ——
/// 这条原本的写法（断言「Windows 样本缺席」在册）当天就该红，红了之后该做的是照新样本
/// 写解析器、再把断言翻面，而不是把它删掉。现在翻面了：
///
///  - 正面：真目录上，登记在册的每个平台都解析出了东西、都不在缺席名单里；
///  - 反面（**活输入**）：同一套扫描逻辑喂一个**空目录**，每个平台都被逐字点名。
///
/// 反面那半不能省。缺了它，「缺席会被点名」这件事在样本到齐之后就再没有任何输入去证实 ——
/// 哪天有人把那段 `scan.note(...)` 整个删掉，也不会有任何地方红。
#[test]
fn absent_platform_samples_are_named_not_silently_green() {
    let scan = scan();

    // 空壳对照先行：一条样本都没解析到时，下面几句全都恒真 ——
    // 一个恒绿的空壳 harness 比没有 harness 更糟，它让人以为覆盖了。
    assert!(
        !scan.parsed.is_empty(),
        "harness 一个分节都没解析到 —— 这就是那个恒绿的空壳。报告：\n{}",
        scan.notes.join("\n")
    );

    for (prefix, display, _) in PLATFORMS {
        let tag = format!("{prefix}-");
        assert!(
            !scan
                .notes
                .iter()
                .any(|n| n.contains(&format!("{display} 样本缺席"))),
            "{display} 的真机抓取已经在仓里了，却仍被报成缺席：\n{}",
            scan.notes.join("\n")
        );
        assert!(
            scan.parsed.iter().any(|(f, _, _)| f.starts_with(&tag)),
            "{display}（前缀 `{tag}`）有样本却一个分节都没解析出来。已解析：{:?}\n报告：\n{}",
            scan.parsed,
            scan.notes.join("\n")
        );
    }

    // 反向对照（活输入）：空目录 ⇒ 每个平台都被逐字点名、且一条都没解析出来。
    let empty = tempfile::tempdir().expect("建临时目录");
    let absent = scan_dir(empty.path());
    assert!(
        absent.parsed.is_empty(),
        "空目录里居然解析出了东西：{:?}",
        absent.parsed
    );
    for (_, display, _) in PLATFORMS {
        assert!(
            absent
                .notes
                .iter()
                .any(|n| n.contains(&format!("{display} 样本缺席"))),
            "空目录上 {display} 没被点名 —— 「缺席必须逐字点名」这条已经不做事了：\n{}",
            absent.notes.join("\n")
        );
    }
}

/// 🔴 **扫描报告里的 Windows 缺口，逐份逐段点名**。
///
/// # 这条说明的射程（2026-09-12 wintun 夹具入库后重新核过）
///
/// 还缺的**不是** `InterfaceType` 这一列本身：`@@@GET_NETADAPTER` 在**两份** Windows 抓取里
/// 都带着它（新那份还带上了 wintun 那一行，判据 `53` 那一半就是从它读出来的）⇒ 判据取材面
/// 那一段没有缺口。剩下的唯一缺口是 `@@@NETSH_INTERFACE` 这条**回退读法**：它的表只有
/// `Idx/Met/MTU/State/Name`，**结构上**给不出适配器类型。那不是待补的缺口，是这条读法的
/// 天花板，但它必须**出现在报告里** —— 否则下一个人会以为它被覆盖了。
///
/// **射程是「每份抓取各一条」，不是全局一条。** 上一版把条数写死成 `1`，第二份 Windows
/// 抓取入库当天它就红了 —— 红得对（条数确实变了），但那条断言把「一条说明」与「一份抓取
/// 一条说明」混成了同一个数。这里改成按**文件 × 分节**点名：再加一份 Windows 抓取时，
/// 缺的那一条会被逐字点名到它自己的文件上，而不是撞在一个全局计数上。
#[test]
fn windows_scan_gaps_are_exactly_the_netsh_fallback_in_every_capture() {
    let scan = scan();

    let gaps: Vec<&String> = scan
        .notes
        .iter()
        .filter(|n| n.contains("抓取不全") || n.contains("解析器还没写"))
        .collect();

    // 每条缺口都必须是 netsh 那条回退读法，且点名缺的是哪一列。
    for gap in &gaps {
        assert!(
            gap.contains("NETSH_INTERFACE"),
            "出现了 netsh 之外的缺口：{gap}\n完整报告：\n{}",
            scan.notes.join("\n")
        );
        assert!(gap.contains("InterfaceType"), "说明没点名缺的那一列：{gap}");
    }
    // 🔴 判据取材面那一段一条缺口都不许有 —— 它要是也缺列，隧道名单整条就不存在了。
    assert!(
        !gaps.iter().any(|g| g.contains("GET_NETADAPTER")),
        "`@@@GET_NETADAPTER` 报了缺口 —— 隧道判据的取材面塌了：{gaps:#?}"
    );

    // 逐份点名：**每一份** Windows 抓取各一条，一条不多一条不少（注记形态是
    // `<文件> 的 @@@<分节>：…`，文件名不含空格）。
    //
    // 🔴 期望值取自 [`windows_fixture_names`]（枚举目录），不是写死的那三份：写死的话，
    // 第四份抓取入库那天这条会红在「多了一个文件名」上 —— 红得对但说的是错的原因，
    // 真正的判据是「每份都恰好缺 netsh 这一条」，与仓里有几份抓取无关。
    let mut gap_files: Vec<&str> = gaps
        .iter()
        .map(|g| g.split(' ').next().expect("注记以文件名开头"))
        .collect();
    gap_files.sort();
    let want = windows_fixture_names();
    assert_eq!(
        gap_files,
        want.iter().map(String::as_str).collect::<Vec<_>>(),
        "缺口不是「每份 Windows 抓取的 netsh 各一条」。现在的缺口：{gaps:#?}\n完整报告：\n{}",
        scan.notes.join("\n")
    );

    // 正向对照：四个判据分节在**每一份** Windows 抓取里都真的解析出了东西 ——
    // 否则「只剩 netsh 一条缺口」可能只是因为某份抓取整条腿都没接上。
    for file in windows_fixture_names() {
        let file = file.as_str();
        for id in [
            "ROUTE_PRINT_4",
            "ROUTE_PRINT_6",
            "GET_NETIPADDRESS",
            "GET_NETADAPTER",
        ] {
            assert!(
                scan.parsed
                    .iter()
                    .any(|(f, sid, n)| f == file && sid == id && *n > 0),
                "{file} 的 @@@{id} 没解析出任何东西：{:?}",
                scan.parsed
            );
        }
    }
}

/// `53`（`IF_TYPE_PROP_VIRTUAL`）这个宽桶里**已登记**的隧道驱动痕迹
/// （`ComponentID` / `DriverDescription` / `InterfaceAlias` 任一列里留下的名字）。
///
/// 这是白名单，但它**不是生产判据**（生产只认 ifType 那一列）—— 它是「这个宽桶里出现了
/// 我们没见过的东西」的报警器。
///
/// 两族各有真机正样本：
///  - `Wintun` / `Tailscale`：Tailscale 1.102.4，2026-09-12 w207 实测；
///  - `tap0901` / `TAP-Windows` / `OpenVPN`：TAP-Windows Adapter V9（驱动 9.27.0.0），
///    2026-09-13 w207 实测。**此前登记的疑值是 `6`，实测是 `53`** —— 也就是说现有白名单
///    `{131, 53}` 本来就覆盖它，判据不用改，该改的是那条写错了的登记。
const REGISTERED_IF_TYPE_53_DRIVERS: &[&str] =
    &["Wintun", "Tailscale", "tap0901", "TAP-Windows", "OpenVPN"];

/// 🔴 **`53` 的两族正样本（wintun / TAP-Windows6）+ 这个宽桶的假阳性面**。
///
/// # 这条绊线换过两次向
///
/// ① 最早钉的是「仓里**没有**任何 VPN 虚拟网卡的抓取 ⇒ wintun 报什么 ifType 无证据」。
/// 2026-09-12 那份把它验掉了：wintun 报 **`53`**，不是 `131` —— 旧判据（只认 `131`）在它
/// 唯一要做的那件事上静默失败。
///
/// ② 2026-09-13 两份新抓取又把它翻了一次：TAP-Windows / OpenVPN 那族**实测也是 `53`**
/// （此前登记的疑值是 `6`）。于是「另外两族驱动无样本」这条登记只剩一族（WireGuard NT /
/// 各家企业 VPN 客户端），而 TAP 那族从「判据可能漏它」变成「判据本来就覆盖它」。
///
/// 现在钉住的三件事：
///
///  1. **两族正样本的 ifType 值本身**：逐字钉在抓取的那两行上，并与生产常量对齐 ——
///     判据表与实测值改一个忘一个时这里先红；
///  2. **`53` 的假阳性面**：凡有抓取里出现**不是已登记正样本**的 `53` 行，本条红 ——
///     那时要么把它登记成新的隧道驱动正样本，要么重新评估 `{131, 53}` 这条放宽；
///  3. **仍无样本的驱动族**：WireGuard NT / Zscaler / GlobalProtect 这类。它们的 ifType
///     仓里没有任何证据；哪天有这种抓取入库，本条红并要求读出实测值。
///
/// 三条的取材面都是**全部** Windows 抓取（[`windows_fixture_names`]），不是写死某一份：
/// 写死单文件名的那一版，第二份抓取带着 wintun 进来那天它压根没看。
#[test]
fn the_captures_pin_iftype_53_for_wintun_and_tap_windows6() {
    // ── ① 两族正样本逐字钉在那两行上 ──
    /// `(抓取, 认这一行的列, 认这一行的值, 期望的 InterfaceAlias, 期望的 Status)`。
    ///
    /// 认行**不按别名**：别名是用户可改的显示名，`ComponentID` 是驱动自己写的。
    const PINNED_53_ROWS: &[(&str, &str, &str, &str, &str)] = &[
        (
            WINDOWS_FIXTURE_WINTUN,
            "ComponentID",
            "Wintun",
            "Tailscale",
            "Up",
        ),
        (
            WINDOWS_FIXTURE_TAP,
            "ComponentID",
            r"root\tap0901",
            "OpenVPN TAP-Windows6",
            "Up",
        ),
    ];
    for (file, col, value, alias, status) in PINNED_53_ROWS.iter().copied() {
        let raw = read_fixture(file);
        let table = cut_adapter_table(&section(&raw, "GET_NETADAPTER"));
        let hit: Vec<&Vec<String>> = table
            .rows
            .iter()
            .filter(|r| table.cell(r, col) == value)
            .collect();
        assert_eq!(
            hit.len(),
            1,
            "{file} 里 `{col} == {value}` 的行不是恰好一条：{:?}",
            table.rows
        );
        assert_eq!(
            table.cell(hit[0], "InterfaceType"),
            "53",
            "{file}：`{value}` 报的 ifType 变了 —— 判据表（route_probe 的 \
             IF_TYPE_PROP_VIRTUAL 头注）该跟着改"
        );
        assert_eq!(table.cell(hit[0], "InterfaceAlias"), alias, "{file}");
        assert_eq!(
            table.cell(hit[0], "Status"),
            status,
            "{file}：`{value}` 那张适配器不是 {status} —— \
             「已装且驱动真的起来了」这条前提没了，这行的说服力打折"
        );
    }
    // 判据常量与实测值是同一个；改一个忘一个时这里红。
    assert_eq!(crate::route_probe::IF_TYPE_TUNNEL, 131);
    assert_eq!(crate::route_probe::IF_TYPE_PROP_VIRTUAL, 53);
    assert_eq!(crate::route_probe::WINDOWS_TUNNEL_IF_TYPES, &[131_u32, 53]);

    // ── ② `53` 的假阳性面：报 53 的行必须是已登记的隧道驱动 ──
    let mut fifty_three_rows = 0usize;
    for file in windows_fixture_names() {
        let raw = read_fixture(&file);
        let table = cut_adapter_table(&section(&raw, "GET_NETADAPTER"));
        for row in table.rows_with_if_type(53) {
            fifty_three_rows += 1;
            let text = table.row_text(row);
            assert!(
                REGISTERED_IF_TYPE_53_DRIVERS
                    .iter()
                    .any(|d| text.contains(d)),
                "{file}：报 `53` 的适配器 `{}` 不是已登记的隧道驱动 —— \
                 `IF_TYPE_PROP_VIRTUAL` 是个宽桶，这正是它的假阳性面在真机上露头。该做的是：\
                 ① 若它是新的隧道驱动，把它登记进 REGISTERED_IF_TYPE_53_DRIVERS 与 route_probe 的 \
                 IF_TYPE_PROP_VIRTUAL 正样本清单；\
                 ② 若它是虚拟交换机之类的非隧道适配器，按 IF_TYPE_PROP_VIRTUAL 头注里的代价对称性\
                 重新评估 `{{131, 53}}` 这条放宽（别忘了假阳性的代价只是展示面多一条网段）。\
                 整行：{text}",
                table.cell(row, "InterfaceAlias")
            );
        }
    }
    // 正向对照：确实有报 `53` 的行，否则上面那个循环恒真（一条都没跑）。
    // 七份抓取里：`routes-ts-off` 0 行（装 Tailscale 之前）、`wintun-present` 与 `ts-on` 各 1 行、
    // 四份 2026-09-13 的各 2 行（wintun + TAP）。
    //
    // 虚拟化全家福那份**同样只有 2 行** —— 那正是它入库的理由：Hyper-V 的三型交换机
    // （Default / Internal / Private，五张适配器）一张都没报 `53`，全报 `6`。
    // 这个数字变大就说明有新的适配器挤进了 `53` 那个宽桶，该重核而不是改数字。
    assert_eq!(
        fifty_three_rows, 10,
        "报 `53` 的适配器行数不是 10 —— 取材面变了，`53` 的依据与假阳性面都要重核"
    );

    // 🔴 更强的那个正样本：**已登录**那份里 wintun 仍是 `53` 且 `Up`。未登录那份只证得了
    // 「装上了」，这份证的是「它真在转发业务流量时也报 53」—— 判据与连接态无关这件事，
    // 在这里才有输入。
    let on = cut_adapter_table(&section(
        &read_fixture(WINDOWS_FIXTURE_TS_ON),
        "GET_NETADAPTER",
    ));
    let on_wintun = on
        .rows
        .iter()
        .find(|r| on.cell(r, "InterfaceAlias") == "Tailscale")
        .expect("已登录那份里 wintun 那行在名单里");
    assert_eq!(on.cell(on_wintun, "InterfaceType"), "53");
    assert_eq!(on.cell(on_wintun, "Status"), "Up");

    // 🔴 同一条性质在 TAP 那族上的输入：`Up`（已连、`10.8.0.2`）与 `Disconnected` 两份抓取里
    // 它都报 `53`。ifType 是**驱动属性**，不随链路状态漂 —— 只有一份 `Up` 的抓取时这句是推理。
    let down = cut_adapter_table(&section(
        &read_fixture(WINDOWS_FIXTURE_HYPERV),
        "GET_NETADAPTER",
    ));
    let down_tap = down
        .rows
        .iter()
        .find(|r| down.cell(r, "ComponentID") == r"root\tap0901")
        .expect("Hyper-V 那份里 TAP 那行在名单里");
    assert_eq!(down.cell(down_tap, "InterfaceType"), "53");
    assert_eq!(
        down.cell(down_tap, "Status"),
        "Disconnected",
        "前提：这份里的 TAP 确实是断开态，否则「ifType 不随链路状态漂」这条没有对照"
    );

    // ── ③ 仍无样本的驱动族 ──
    /// ifType **未验证**的隧道驱动族在抓取里会留下的名字。
    ///
    /// 命中 ⇒ 仓里第一次有了这族的真机抓取，该读出它实际报的 `InterfaceType` 并据此决定判据面。
    ///
    /// 🔴 **TAP-Windows / OpenVPN（`tap0901`）2026-09-13 已从本清单划掉** —— 实测 `53`，
    /// 见本文件 `PINNED_53_ROWS` 那一行。RAS（SSTP / L2TP / IKEv2）那族没有可靠的字面量痕迹
    /// （适配器名是 `WAN Miniport (IKEv2)` 一类），故它**不在这张表里**：这张表守的是
    /// 「能靠名字认出来的那些」，RAS 的缺口如实登记在 `parse_windows_tunnel_interfaces` 的头注里。
    const UNVERIFIED_DRIVER_FAMILIES: &[&str] =
        &["WireGuard", "wireguard", "Zscaler", "GlobalProtect"];
    for file in windows_fixture_names() {
        let adapters = section(&read_fixture(&file), "GET_NETADAPTER");
        // 前提：这份抓取的适配器段真有内容，否则下面那条否定断言恒真。
        assert!(
            adapters.contains("InterfaceType"),
            "{file} 的 @@@GET_NETADAPTER 里连列头都没有 —— 下一条否定断言没有信息量"
        );
        let hit: Vec<&str> = UNVERIFIED_DRIVER_FAMILIES
            .iter()
            .copied()
            .filter(|m| adapters.contains(m))
            .collect();
        assert!(
            hit.is_empty(),
            "{file} 里出现了 ifType 未验证的隧道驱动族（命中 {hit:?}）—— \
             「这族报什么 InterfaceType 仓里没有证据」这条登记过期了。该做的是：\
             ① 从抓取里按列读出它实际报的 InterfaceType；\
             ② 若落在 `{{131, 53}}` 之外，按 IF_TYPE_PROP_VIRTUAL 头注里的代价对称性决定收不收\
             （注意 `6` = 物理网卡 / Hyper-V 虚拟交换机 / 内核调试适配器**三者同型**，\
             `23` 与 PPPoE 拨号同型，这两个值收了会把真业务网卡判成隧道）；\
             ③ 把结论写进 parse_windows_tunnel_interfaces 的判据表，并在这里把它划掉。"
        );
    }
}

/// 🔴 **`53` 的假阳性面有负向证据了：Hyper-V 的两张虚拟适配器报 `6`，不报 `53`**。
///
/// 一条门里正负都要，这样两个方向的漂移都会红：
///
///  - **负半（白名单被放宽）**：`vEthernet (Default Switch)` / `vSwitch (Default Switch)`
///    **不得**出现在解析出的隧道名单里。它们是 Hyper-V 装出来的虚拟交换机，实测 `InterfaceType`
///    都是 **`6`**。谁哪天把 `6` 收进 [`crate::route_probe::WINDOWS_TUNNEL_IF_TYPES`]，这半红。
///  - **正半（白名单被收窄）**：同一份抓取里的 `Tailscale`（wintun）与 `OpenVPN TAP-Windows6`
///    **必须**在名单里。谁哪天把 `53` 删掉（= 回到 2026-09-12 之前那个判据），这半红。
///
/// # 为什么这份抓取比「多一个正样本」值钱
///
/// 「装了 Hyper-V / VMware / Docker 的机器上可能有别的适配器落进 `53` 这个宽桶」此前是一条
/// **登记在案的未知风险** —— 论证只能靠代价对称性，没有任何输入。这份抓取给了 Hyper-V 那一半
/// 的**负向证据**：装上之后多出来的两张适配器都报 `6`。
///
/// 同一份抓取还反向印证了 `6` **绝对不能**收：它在这一份里同时是物理网卡
/// （`Red Hat VirtIO Ethernet Adapter`）、Hyper-V 虚拟交换机、以及内核调试适配器 ——
/// 三种东西同一个值，收了就是把一批真业务网卡判成隧道。这条由下面的 `SIX_IS_THREE_THINGS` 钉住。
///
/// 断言跑的是**生产解析器**（`parse_windows_tunnel_interfaces`）而不是测试侧的切列表：
/// 要证的是「这两张适配器最终没进隧道名单」，而不是「表里那一格写着 6」。
#[test]
fn iftype_53_admits_the_two_vpn_drivers_but_not_the_hyperv_switches() {
    let raw = read_fixture(WINDOWS_FIXTURE_HYPERV);
    let adapters = section(&raw, "GET_NETADAPTER");
    let tunnels = parse_windows_tunnel_interfaces(&adapters).expect("适配器段解析得动");

    /// Hyper-V 装出来的两张适配器：**不是**隧道，实测 ifType 都是 `6`。
    const HYPERV_ADAPTERS: &[&str] = &["vEthernet (Default Switch)", "vSwitch (Default Switch)"];
    /// 同一份抓取里**必须**被认成隧道的两张 VPN 虚拟网卡（两族 `53` 正样本）。
    const VPN_ADAPTERS: &[&str] = &["Tailscale", "OpenVPN TAP-Windows6"];

    let table = cut_adapter_table(&adapters);
    for name in HYPERV_ADAPTERS.iter().copied() {
        // 正向对照先行：这张适配器**确实在输入里**。缺了它，「它没被判成隧道」可能只是
        // 因为它压根没进来 —— 那是一条恒真断言。
        let row = table
            .rows
            .iter()
            .find(|r| table.cell(r, "InterfaceAlias") == name)
            .unwrap_or_else(|| panic!("`{name}` 不在这份抓取的适配器名单里：{:?}", table.rows));
        assert_eq!(
            table.cell(row, "InterfaceType"),
            "6",
            "`{name}` 的实测 ifType 变了 —— 「Hyper-V 不落进 53 这个桶」这条负向证据的依据没了"
        );
        assert!(
            !tunnels.iter().any(|t| t == name),
            "Hyper-V 的 `{name}` 被判成了隧道 —— 白名单放宽到 `6` 了？\
             `6` 在这一份抓取里同时是物理网卡、虚拟交换机与内核调试适配器。名单：{tunnels:?}"
        );
    }
    for name in VPN_ADAPTERS.iter().copied() {
        assert!(
            tunnels.iter().any(|t| t == name),
            "`{name}` 没进隧道名单 —— 白名单收窄了？（去掉 `53` = 回到 2026-09-12 之前\
             那个会漏 wintun 的旧判据）名单：{tunnels:?}"
        );
    }

    // 🔴 `6` 同时是三种东西 —— 「不能盲收 `6`」这句话的全部依据，钉在真机数据上。
    const SIX_IS_THREE_THINGS: &[&str] = &[
        "以太网",                     // Red Hat VirtIO Ethernet Adapter：真业务网卡
        "vEthernet (Default Switch)", // Hyper-V 虚拟以太网适配器
        "以太网(内核调试器)",         // Microsoft Kernel Debug Network Adapter
    ];
    for name in SIX_IS_THREE_THINGS.iter().copied() {
        let row = table
            .rows
            .iter()
            .find(|r| table.cell(r, "InterfaceAlias") == name)
            .unwrap_or_else(|| panic!("`{name}` 不在这份抓取里：{:?}", table.rows));
        assert_eq!(
            table.cell(row, "InterfaceType"),
            "6",
            "`{name}` 不再报 `6` —— 「`6` 是个三合一的桶」这条论证要重核"
        );
    }
    // 名单整体也断一次：恰好是那两张 VPN 网卡 + 三个 `131` 协议隧道，一个不多一个不少。
    assert_eq!(
        tunnels,
        vec![
            "Teredo Tunneling Pseudo-Interface".to_string(),
            "Tailscale".to_string(),
            "Microsoft IP-HTTPS Platform Interface".to_string(),
            "OpenVPN TAP-Windows6".to_string(),
            "6to4 Adapter".to_string(),
        ],
        "这份抓取的隧道名单变了（顺序按 Get-NetAdapter 的输出序）"
    );
}

/// 每个登记在册的平台，要么真的解析出了东西，要么在报告里有一条署名的说明。
/// 二者皆无 = 悄悄没覆盖，而没有任何地方会红。
#[test]
fn every_registered_platform_is_either_parsed_or_explicitly_accounted_for() {
    let scan = scan();
    for (prefix, display, _) in PLATFORMS {
        let tag = format!("{prefix}-");
        let parsed = scan.parsed.iter().any(|(f, _, _)| f.starts_with(&tag));
        let noted = scan
            .notes
            .iter()
            .any(|n| n.contains(display) || n.contains(&tag));
        assert!(
            parsed || noted,
            "{display}（前缀 `{tag}`）既没解析出任何东西、报告里也没有它的说明。\
             已解析：{:?}\n报告：\n{}",
            scan.parsed,
            scan.notes.join("\n")
        );
    }
}

// ══════════ 判据 2：macOS 那支真的跑起来了 ══════════

/// 🔴 **判据 2**：classful 缩写在**真机抓取**上被展开。
///
/// 断言钉在 2026-09-08 那份抓取的具体行上，不是钉在手写样例上 —— 后者会退化成自证
/// （我照着自己的实现编一行输入，当然过）。
#[test]
fn macos_fixture_expands_classful_abbreviations() {
    let raw = read_fixture(MACOS_FIXTURE);
    let routes =
        parse_netstat_routes(&section(&raw, "V4"), IpFamily::V4).expect("真机 v4 抓取应解析得动");

    let has = |prefix: &str, iface: &str| {
        routes.contains(&RouteEntry {
            prefix: prefix.into(),
            interface: iface.into(),
        })
    };

    // 抓取第 12 行逐字是 `192.168.10         link#14            UCS                   en0      !`
    // —— 没有 `/24`，那是 classful 缩写。
    assert!(
        has("192.168.10.0/24", "en0"),
        "classful `192.168.10` 没展开成 /24：{routes:?}"
    );
    // 另外两种缩写宽度：证明规则不是给 /24 一个值写死的。
    assert!(has("127.0.0.0/8", "lo0"), "`127` 应是 /8：{routes:?}");
    assert!(
        has("169.254.0.0/16", "en0"),
        "`169.254` 应是 /16：{routes:?}"
    );
    // 长度明写、地址**仍然**缩写（`224.0.0/4`）—— 补零与定长度是两件独立的事。
    assert!(
        has("224.0.0.0/4", "en0"),
        "`224.0.0/4` 的地址没补零：{routes:?}"
    );
    // 主机路由（无 `/前缀`）补 /32，与 Linux 那侧同型。
    assert!(
        has("192.168.10.105/32", "en0"),
        "主机路由没补 /32：{routes:?}"
    );
    // `default` 行产出条目，规范成 v4 的默认路由前缀（族由调用方给，见 `IpFamily` 头注）。
    // 这一条是 `netstat -rn` 里唯一**不带** `I`（RTF_IFSCOPE）的默认路由：它在全局转发面上。
    assert!(
        has("0.0.0.0/0", "en0"),
        "default 行没产出条目 —— 抢默认路由的全隧道正是靠它才看得见：{routes:?}"
    );
}

/// classful 展开的两个**似是而非的错答案**各写一条 —— 只断"对的那个"，换成任一错法时
/// 这条测试仍可能因为别的原因过。
#[test]
fn classful_expansion_rejects_the_two_plausible_wrong_answers() {
    // 错法一：「没有 `/` 就是主机路由」⇒ 补完零按 /32 算。
    assert_eq!(
        expand_netstat_destination("192.168.10").as_deref(),
        Some("192.168.10.0/24")
    );
    assert_ne!(
        expand_netstat_destination("192.168.10").as_deref(),
        Some("192.168.10.0/32")
    );
    // 错法二：「有 `/` 就原样照抄」⇒ 地址还是缩写的，过不了 CIDR 校验。
    assert_eq!(
        expand_netstat_destination("224.0.0/4").as_deref(),
        Some("224.0.0.0/4")
    );
    assert_ne!(
        expand_netstat_destination("224.0.0/4").as_deref(),
        Some("224.0.0/4")
    );

    // 非地址 / 越界的输入不许蒙混过关。
    for bad in ["default", "link#14", "192.168.300", "192.168.10/33", ""] {
        assert_eq!(
            expand_netstat_destination(bad),
            None,
            "`{bad}` 不该被展开成前缀"
        );
    }
}

/// IPv6 目的地的 `%作用域` 被剥掉，主机路由补 /128。同样钉在真机抓取的具体行上。
#[test]
fn macos_fixture_strips_ipv6_scope_zones() {
    let raw = read_fixture(MACOS_FIXTURE);
    let routes =
        parse_netstat_routes(&section(&raw, "V6"), IpFamily::V6).expect("真机 v6 抓取应解析得动");

    let has = |prefix: &str, iface: &str| {
        routes.contains(&RouteEntry {
            prefix: prefix.into(),
            interface: iface.into(),
        })
    };

    // `fe80::%utun0/64` —— 作用域在 `/` 之前。
    assert!(has("fe80::/64", "utun0"), "`fe80::%utun0/64` 没剥作用域");
    // `fe80::2fe5:3b81:bad1:be1c%utun0` —— 主机路由 + 作用域，且出接口是 lo0 不是 utun0。
    assert!(
        has("fe80::2fe5:3b81:bad1:be1c/128", "lo0"),
        "v6 主机路由没补 /128 或作用域没剥：{routes:?}"
    );
    // `ff00::/8` 在多个 utun 上各有一条 —— 同前缀不同接口都要留下。
    assert!(has("ff00::/8", "utun3"), "组播路由丢了：{routes:?}");

    assert!(
        routes.iter().all(|r| !r.prefix.contains('%')),
        "有前缀残留 `%作用域`：{:?}",
        routes
            .iter()
            .filter(|r| r.prefix.contains('%'))
            .collect::<Vec<_>>()
    );
}

/// 🔴 **macOS 覆盖边界：tailnet 业务网段只在连接态那一份抓取里出现，且逐条点名**。
///
/// # 这条断言换过向（2026-09-12 22:00）
///
/// 在连接态抓取入库之前，仓里三份 mac 抓取全是断开态，这条钉的是「一条 tailnet 网段都没有
/// ⇒ 证不了业务网段被摘出来」。现在证得了，于是它换成**两侧都断**：
///
///  - 断开态两份：一条 tailnet 网段都没有 —— 噪声过滤的负样本；
///  - 连接态那份：**必须有**，而且逐条点名（逐 peer 的 `/32` + `fd7a:…/48` + MagicDNS 那条）。
///
/// 换向前它还有一个更隐蔽的毛病：取材面写死成 `MACOS_FIXTURE` **一份**，于是连接态抓取入库
/// 那天它压根没看那份文件。一条「仓里还没有 X」的登记，取材面必须是**全部**样本
/// （[`macos_fixture_names`]）。
///
/// **判「这份是不是连接态」不能看 `@@@TAILSCALE`**：连接态那份的该分节仍是「命令不在 PATH 上」
/// 哨兵（Mac App Store 版不装 CLI）。真值在路由表里。
/// 虚拟化跑起来时，macOS 判据既不误报也不漏报（Parallels Desktop 运行中的真机抓取）。
///
/// # 为什么这道门值得单独存在
///
/// macOS 的隧道判据是 [`MAC_TUNNEL_FLAG`]（`POINTOPOINT`）而不是名字前缀清单。
/// 用 flag 的代价是它**认的是"点对点设备"这件事本身**——虚拟化软件建的网卡若恰好是点对点型，
/// 就会被整批收进来。PD 18+ 建六个接口（`vmenet0/1/2` + `bridge100/101/102`），
/// 真机实测**全是 `BROADCAST` 型**，一个 `POINTOPOINT` 都没有。
///
/// 这份抓取同时带着 Tailscale 连接态，所以两个方向能从**同一份输入**上取：
/// 不误报（六个虚拟接口一个都不进名单）与不漏报（12 个 utun 仍全部进名单）。
/// 只验一半的门会被"判据把所有东西都收了"和"判据什么都不收"各骗一次。
///
/// **缺口如实登记**：`vmenet` 是 PD 18+ 的形态，老版本的 `vnic0`/`vnic1` 仓里没有样本；
/// VMware Fusion 的 `vmnet*` 同样没有。判据不按名字走，所以这两族**理论上**同样不收，
/// 但那是推论不是实测。
#[test]
fn parallels_virtual_nics_are_not_mistaken_for_tunnels() {
    let raw = read_fixture(MACOS_FIXTURE_PARALLELS);
    let ifconfig = section_any(&raw, &["IFCONFIG_FLAGS", "IFCONFIG"]);
    assert!(
        !ifconfig.trim().is_empty(),
        "{MACOS_FIXTURE_PARALLELS} 的 ifconfig 分节是空的 —— 下面两半都会空跑"
    );

    // 前提：这六个接口**确实在输入里**。少了这一条，下面的否定断言会被
    //「PD 根本没跑、抓取里压根没有虚拟网卡」骗成绿的。
    const PD_NICS: &[&str] = &[
        "vmenet0",
        "vmenet1",
        "vmenet2",
        "bridge100",
        "bridge101",
        "bridge102",
    ];
    for nic in PD_NICS {
        assert!(
            ifconfig.contains(&format!("\n{nic}: flags=")),
            "{MACOS_FIXTURE_PARALLELS} 里没有 `{nic}` —— 这份抓取不是 PD 运行态，\
             这道门的前提不成立"
        );
    }

    let tunnels = parse_macos_tunnel_interfaces(&ifconfig)
        .expect("PD 运行态那份抓取的 ifconfig 必须解析得动");

    // ① 不误报：PD 的六个虚拟接口一个都不许进隧道名单。
    for nic in PD_NICS {
        assert!(
            !tunnels.iter().any(|t| t == nic),
            "PD 的虚拟网卡 `{nic}` 被判成了隧道 —— 判据取的是 `{}` flag，\
             而这六个接口真机实测全是 BROADCAST 型。名单：{tunnels:?}",
            crate::route_probe::MAC_TUNNEL_FLAG
        );
    }

    // ② 不漏报：同一份输入里的真隧道必须仍然全在名单里。
    //    数字钉死：`gif0` + `utun0..=utun11` = 13 条（与 §21 那批抓取同一台机器）。
    let utun_count = tunnels.iter().filter(|t| t.starts_with("utun")).count();
    assert_eq!(utun_count, 12, "utun 少了 —— 名单：{tunnels:?}");
    assert!(
        tunnels.iter().any(|t| t == "gif0"),
        "系统自带的 `gif0` 也该在名单里（它带 POINTOPOINT，只是零宣告）：{tunnels:?}"
    );
    assert_eq!(
        tunnels.len(),
        13,
        "隧道名单条数变了 —— 多出来的那个正是本门要抓的误报：{tunnels:?}"
    );

    // ③ 正向对照：判据本身是活的。把 PD 的一张网卡的 flags 换成点对点型，它必须被收进来 ——
    //    否则上面那批否定断言可能只是因为解析器压根没在工作。
    let mutated = ifconfig.replace(
        "vmenet0: flags=8963<UP,BROADCAST,SMART,RUNNING,PROMISC,SIMPLEX,MULTICAST>",
        "vmenet0: flags=8963<UP,POINTOPOINT,SMART,RUNNING,PROMISC,SIMPLEX,MULTICAST>",
    );
    assert_ne!(
        mutated, ifconfig,
        "变异没打上 —— `vmenet0` 那行的字面量变了"
    );
    let mutated_tunnels =
        parse_macos_tunnel_interfaces(&mutated).expect("变异后的 ifconfig 仍应解析得动");
    assert!(
        mutated_tunnels.iter().any(|t| t == "vmenet0"),
        "把 `vmenet0` 改成 POINTOPOINT 之后它仍不在名单里 —— \
         说明上面那批「不误报」断言是空跑的：{mutated_tunnels:?}"
    );
}

#[test]
fn macos_tailnet_prefixes_appear_only_in_the_connected_capture() {
    let mut connected_seen = 0usize;
    for file in macos_fixture_names() {
        let raw = read_fixture(&file);
        let mut routes =
            parse_netstat_routes(&section_any(&raw, MACOS_NETSTAT_V4), IpFamily::V4).expect("v4");
        routes.extend(
            parse_netstat_routes(&section_any(&raw, MACOS_NETSTAT_V6), IpFamily::V6).expect("v6"),
        );

        // 正向对照先行：utun **确实**在这份抓取里。否则两侧的结论都可能只是
        // 「utun 压根没抓到」造成的假绿。
        assert!(
            routes.iter().any(|r| r.interface.starts_with("utun")),
            "{file}：这份抓取里一个 utun 都没有 —— 下面的结论没有信息量"
        );

        let tailnet: Vec<&RouteEntry> = routes
            .iter()
            .filter(|r| is_tailnet_prefix(&r.prefix))
            .collect();

        let Some((_, want_tailnet, want_on_tunnel)) = MACOS_CONNECTED_CAPTURES
            .iter()
            .find(|(f, _, _)| *f == file)
            .copied()
        else {
            assert!(
                tailnet.is_empty(),
                "{file} 里出现了 tailnet 网段 {tailnet:?} —— 这份登记的是**断开态**。\
                 若它真换成了连接态抓取，请把它登记进 MACOS_CONNECTED_CAPTURES 并同步 \
                 fixtures/README.md 的覆盖表"
            );
            continue;
        };

        connected_seen += 1;
        // ── 每份连接态抓取都过的那几条（数字逐份取自 MACOS_CONNECTED_CAPTURES）──
        assert_eq!(
            tailnet.len(),
            want_tailnet,
            "{file} 里的 tailnet 业务网段条数变了：{tailnet:?}"
        );
        let (on_utun11, elsewhere): (Vec<&&RouteEntry>, Vec<&&RouteEntry>) =
            tailnet.iter().partition(|r| r.interface == "utun11");
        assert_eq!(
            on_utun11.len(),
            want_on_tunnel,
            "{file} 里挂在 utun11 上的条数变了：{on_utun11:?}"
        );
        // 🔴 **本机自己那条 tailnet v6 `/128` 挂在 `lo0` 上，不在 utun 上**（BSD 把本地地址的
        // 主机路由装到环回）。两份连接态抓取各有一条、地址不同 ⇒ 断的是**形状**不是字面量；
        // 后果是它进不了 `foreign`（`foreign_tunnel_routes` 按接口名过滤，`lo0` 不在隧道名单里），
        // 也就是 mac 侧看得见「别人的网段」、看不见自己那条。对本模块（找**外来**隧道）方向恰好
        // 是对的，但它是真机形态不是设计意图，写在这里免得下一个人以为漏了一条。
        let elsewhere_shape: Vec<(&str, &str)> = elsewhere
            .iter()
            .map(|r| (r.prefix.as_str(), r.interface.as_str()))
            .collect();
        assert_eq!(elsewhere_shape.len(), 1, "{file}：{elsewhere_shape:?}");
        assert!(
            elsewhere_shape[0].1 == "lo0"
                && elsewhere_shape[0].0.starts_with("fd7a:115c:a1e0::")
                && elsewhere_shape[0].0.ends_with("/128"),
            "{file}：挂在 utun11 之外的 tailnet 网段不是「本机自己那条 v6 /128 在 lo0 上」\
             这一形状：{elsewhere_shape:?}"
        );

        if file != MACOS_FIXTURE_TS_ON {
            continue;
        }
        // ── 只在 09-12 那份上做的逐条点名（它是 mac 侧业务网段的首份正样本）──
        let has = |prefix: &str| {
            routes
                .iter()
                .any(|r| r.prefix == prefix && r.interface == "utun11")
        };
        // 逐 peer 的主机路由（v4 第一条与最后一条各点一个，证明不是只摘到某一条）。
        assert!(
            has("32.0.0.1/32"),
            "逐 peer 主机路由 32.0.0.1/32 没摘到：{tailnet:?}"
        );
        assert!(
            has("32.0.0.29/32"),
            "逐 peer 主机路由 32.0.0.29/32 没摘到：{tailnet:?}"
        );
        // 🔴 本机那条在 `netstat` 里是 `32.0.0.30  32.0.0.30  UH`（**没有 `/32`**），
        // 靠主机路由补前缀这条规则才落得到 /32 上。
        assert!(
            has("32.0.0.30/32"),
            "无 `/前缀` 的 UH 主机路由没补成 /32：{tailnet:?}"
        );
        assert!(
            has("100.100.100.100/32"),
            "MagicDNS 那条没摘到：{tailnet:?}"
        );
        // v6：ULA 段本体 + 它下面的一条 /128。
        assert!(
            has("fd7a:115c:a1e0::/48"),
            "tailnet v6 段没摘到：{tailnet:?}"
        );
        assert!(
            has("fd7a:115c:a1e0::16/128"),
            "tailnet v6 主机路由没摘到：{tailnet:?}"
        );
        // 🔴 形态：装的是逐 peer 主机路由，**没有**汇总段。正向对照是上面那几条 /32。
        assert!(
            !routes.iter().any(|r| r.prefix == "32.0.0.0/24"),
            "出现了 `32.0.0.0/24` 汇总段 —— 「Tailscale 装逐 peer /32」这条形态登记过期了：{tailnet:?}"
        );
        // 🔴 这份的那条 lo0 网段逐字是 `fd7a:115c:a1e0::d3/128`（`ifconfig utun11` 里逐字有它，
        // 是这台 mac 自己的 tailnet 地址）。上面那条形状断言对两份抓取都开，这条是它的字面量版本 ——
        // 只有形状断言的话，「本机那条」被换成任意一条 `fd7a:…/128` 也不会有人发现。
        assert_eq!(
            elsewhere_shape,
            [("fd7a:115c:a1e0::d3/128", "lo0")],
            "{file}：{elsewhere_shape:?}"
        );
    }
    // 缺了这条，上面那支可能一次都没执行（连接态夹具被改名 / 删掉）。
    assert_eq!(
        connected_seen,
        MACOS_CONNECTED_CAPTURES.len(),
        "登记在册的连接态抓取没全部进循环 —— 正样本那一半少跑了"
    );
}

/// `Netif` 的列号**从列头行读**，不写死。
///
/// ⚠️ 本条用的是**构造**输入，不是真机抓取：仓里没有老 macOS 的样本。它证的只是
/// 「列号取自列头」这条实现性质，**不**证老 macOS 的真实列头长什么样 —— 如实登记在此。
#[test]
fn netif_column_is_read_from_the_header_not_hardcoded() {
    let older_macos = "Routing tables\n\
                       \n\
                       Internet:\n\
                       Destination        Gateway            Flags   Refs      Use   Netif Expire\n\
                       192.168.10         link#4             UCS        1        0     en1\n";
    let routes = parse_netstat_routes(older_macos, IpFamily::V4).expect("多两列的列头也要解析得动");
    assert_eq!(
        routes,
        vec![RouteEntry {
            prefix: "192.168.10.0/24".into(),
            interface: "en1".into()
        }],
        "列头多出 Refs/Use 两列时接口名取错了"
    );
    // 反向对照：写死"第 4 列是接口"的版本会取到 `1`（Refs）。
    assert_ne!(routes[0].interface, "1", "取到的是 Refs 列，不是 Netif 列");
}

// ══════════ 判据 4：畸形样本必须报错 ══════════

/// 🔴 **判据 4**：截断 / 空的抓取必须 `Err`，不许静默产出空（或少一半的）结果。
#[test]
fn malformed_captures_error_instead_of_yielding_a_quiet_empty_list() {
    // 正向对照先行：同一入口对**好样本**是 `Ok` 且非空。没有这条，下面全 `Err` 也可能
    // 只是说明「这个解析器什么都解析不了」。
    let good = section(&read_fixture(MACOS_FIXTURE), "V4");
    let good = parse_netstat_routes(&good, IpFamily::V4).expect("好样本必须 Ok");
    assert!(!good.is_empty(), "好样本解析出 0 条 —— 正向对照本身塌了");

    let dir = fixtures_dir().join("malformed");
    let mut bad_files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("读 {}：{e}", dir.display()))
        .map(|e| e.expect("目录项").path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "txt"))
        .collect();
    bad_files.sort();
    assert!(
        !bad_files.is_empty(),
        "malformed/ 是空的 —— 这条反向对照没有任何输入，恒绿"
    );

    for path in &bad_files {
        let name = file_name(path);
        let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("读 {name}：{e}"));
        let got = parse_netstat_routes(&raw, IpFamily::V4);
        assert!(
            got.is_err(),
            "{name}：畸形样本被静默解析成 {got:?} —— 短的/空的 `Ok` 会被下游当成事实"
        );
    }

    // 两种畸形各自落在**不同**的错误支上：空文件 = 连列头都没有；截断 = 列头在、数据行短了。
    // 只断 `is_err()` 的话，一个「凡事都报 MissingHeader」的实现也能全绿。
    assert!(
        matches!(
            parse_netstat_routes(&read_fixture("malformed/macos-empty.txt"), IpFamily::V4),
            Err(RouteTableParseError::MissingHeader { .. })
        ),
        "空文件应报「找不到列头」"
    );
    let truncated = read_fixture("malformed/macos-truncated-row.txt");
    assert!(
        matches!(
            parse_netstat_routes(&truncated, IpFamily::V4),
            Err(RouteTableParseError::TruncatedRow { .. })
        ),
        "截断行应报「列数不够」，而不是被当成列头缺失或被跳过"
    );
}

/// 空输入不许变成一份空的 `Ok`：每支解析器都得说出「为什么没有结果」。
///
/// 三支的"没有结果"各是**不同的一种**，测试逐一钉住 —— 只断 `is_err()` 的话，
/// 一个「凡事都报同一个错」的实现也能全绿。
#[test]
fn empty_input_yields_a_named_reason_not_an_empty_result() {
    // ① macOS 隧道枚举：解析器写好了，输入里一个接口头行都没有 ⇒ 「一条都没解析出来」。
    let mac = parse_macos_tunnel_interfaces("").unwrap_err();
    assert!(
        matches!(
            mac,
            RouteTableParseError::NoParsableRows {
                command: "ifconfig -a",
                ..
            }
        ),
        "macOS 隧道枚举对空输入应报「一条都没解析出来」，而不是 Ok(空)：{mac}"
    );

    // ② Windows 路由表：解析器写好了，同样是「一条活动路由行都没有」。
    let win_routes = parse_route_print_routes("", &WindowsInterfaceNames::default()).unwrap_err();
    assert!(
        matches!(
            win_routes,
            RouteTableParseError::NoParsableRows {
                command: "route print",
                ..
            }
        ),
        "Windows 路由表对空输入应报「一条都没解析出来」：{win_routes}"
    );

    // ③ Windows 隧道名单：空输入里连列头都没有 ⇒ 「抓取不全，缺 InterfaceType 列」。
    //    这支必须与上面两支在类型上分得开：缺的是**一列**，不是「一行都没有」。
    let win_tunnels = parse_windows_tunnel_interfaces("").unwrap_err();
    assert!(
        matches!(
            win_tunnels,
            RouteTableParseError::CaptureIncomplete {
                platform: Platform::Win,
                ..
            }
        ),
        "Windows 隧道名单应报「抓取不全」：{win_tunnels}"
    );
    let text = win_tunnels.to_string();
    assert!(
        text.contains("InterfaceType"),
        "错误信息要点名缺的是**哪一列**：{text}"
    );
    assert!(
        text.contains("Select-Object"),
        "错误信息要给出重抓方式，否则下一个人只知道「不行」不知道「怎么行」：{text}"
    );

    // 同一支的**真实**触发面：`netsh interface ipv4 show interfaces` 这条回退读法
    // 结构上就给不出适配器类型（它的表只有 Idx/Met/MTU/State/Name）。喂真样本进去仍报缺列，
    // 证明它报的是「这条读法给不出那一列」，不是「输入是空的」。
    let raw = read_fixture(WINDOWS_FIXTURE);
    let netsh = section(&raw, "NETSH_INTERFACE");
    assert!(
        netsh.contains("Idx") && !netsh.contains("InterfaceType"),
        "前提：netsh 这份真有内容、且确实没有 InterfaceType 列：{netsh}"
    );
    assert!(
        matches!(
            parse_windows_tunnel_interfaces(&netsh),
            Err(RouteTableParseError::CaptureIncomplete { .. })
        ),
        "netsh 回退读法应报缺列，而不是解析出一份空名单"
    );

    // 正向对照：**带 `InterfaceType` 的那份**必须解析得动且非空 —— 否则上面几条红的
    // 可能只是「这支根本什么都解析不了」。
    let adapters = parse_windows_tunnel_interfaces(&section(&raw, "GET_NETADAPTER"))
        .expect("带 InterfaceType 的真样本应解析得动");
    assert!(
        !adapters.is_empty(),
        "真样本应解析出隧道适配器，否则正向对照塌了"
    );
}

// ══════════ 夹具切分本身的纪律 ══════════

/// 分节切分**逐字**：行尾空格与原始行终止符都不许被 harness 自己规整掉 ——
/// 夹具的全部价值就在那些「照记忆写不出来」的字节上。
#[test]
fn sections_are_split_verbatim() {
    let raw = read_fixture(MACOS_FIXTURE);
    let v4 = section(&raw, "V4");

    assert!(
        v4.contains("192.168.10         link#14            UCS                   en0      !\n"),
        "真机那一行的列间距被动过了"
    );
    assert!(
        v4.lines().any(|l| l.ends_with(' ')),
        "行尾空格被吃掉了 —— `netstat` 的 Expire 空列就长这样"
    );

    // 抬头（首个 `@@@` 之前的内容）不属于任何分节；非法标记不当分节看。
    assert_eq!(
        split_sections("给人读的抬头\n@@@A\nx \n@@@not-a-marker\n@@@B\ny\n"),
        vec![
            ("A".to_string(), "x \n@@@not-a-marker\n".to_string()),
            ("B".to_string(), "y\n".to_string()),
        ]
    );
    // 一个标记都没有 ⇒ 空 —— `absorb_file` 据此当场红，不会把整份文件当成一个匿名分节。
    assert!(split_sections("什么标记都没有\n").is_empty());
}

/// 采集脚本登记的「这条读法不可用」哨兵，不会被当成可解析的输出。
///
/// 没有这条，`@@@GET_NETADAPTER` 里的一行错误说明会被送进解析器，结局要么是一条假错、
/// 要么（更糟）是一份空的 `Ok`。
#[test]
fn unavailable_sentinel_is_registered_not_parsed() {
    // 哨兵字符串与采集脚本里写的必须是同一个 —— 对不上时 harness 会把说明文本当输出解析。
    assert_eq!(UNAVAILABLE, "<<POLARIS-CAPTURE-UNAVAILABLE>>");
    let body = format!("{UNAVAILABLE} Get-NetAdapter: 命令不存在\n");
    assert!(
        body.contains(UNAVAILABLE),
        "哨兵判据是「分节体里含这串」，与行首缩进无关"
    );
}

/// 扫描报告在测试输出里可见 —— 缺口要看得见，不是埋在断言里。
///
/// `cargo test -- --nocapture` 时打印；平时靠断言。
#[test]
fn scan_report_is_printable() {
    let scan = scan();
    println!("── route_probe 夹具扫描报告 ──");
    for (file, id, n) in &scan.parsed {
        println!("  ✔ {file} @@@{id}：{n} 条");
    }
    for note in &scan.notes {
        println!("  · {note}");
    }
    assert!(
        !scan.parsed.is_empty() || !scan.notes.is_empty(),
        "报告完全是空的 —— 连夹具目录都没读到？"
    );
}

// ══════════ macOS：`ifconfig -a` → 隧道接口（2026-09-12 全套抓取）══════════

/// 2026-09-12 那份抓取的 `@@@IFCONFIG` 段（`ifconfig -a` 全文，未过滤）。
fn macos_ifconfig() -> String {
    section(&read_fixture(MACOS_FIXTURE_FULL), "IFCONFIG")
}

/// 🔴 **判据 4**：`utun*` 被认成隧道，`en0` / `lo0` 不被认。钉在**真机抓取**上。
#[test]
fn macos_fixture_recognizes_utun_as_tunnel_but_not_en0_or_lo0() {
    let tunnels =
        parse_macos_tunnel_interfaces(&macos_ifconfig()).expect("真机 ifconfig 应解析得动");

    // 逐条列全，不只断包含关系：多出来一个（把 `en0` 当隧道）与少掉一个（漏掉 `gif0`）
    // 是两种相反的错，只写 `contains` 的话前者不会红。
    assert_eq!(
        tunnels,
        vec![
            "gif0", "utun0", "utun1", "utun2", "utun3", "utun4", "utun5", "utun6", "utun7",
            "utun8",
        ],
        "隧道名单与真机抓取对不上"
    );
    for not_a_tunnel in ["en0", "lo0", "bridge0", "awdl0", "llw0", "ap1", "anpi0"] {
        assert!(
            !tunnels.iter().any(|t| t == not_a_tunnel),
            "`{not_a_tunnel}` 被当成隧道了：{tunnels:?}"
        );
    }
    // 正向对照：这些非隧道接口**确实在**这份抓取里（否则上面那串否定断言没有信息量）。
    let roster = parse_macos_interface_flags(&macos_ifconfig());
    for name in ["en0", "lo0", "bridge0"] {
        assert!(
            roster.iter().any(|i| i.name == name),
            "`{name}` 压根不在花名册里 —— 上面那条否定断言是假绿：{roster:?}"
        );
    }
}

/// 🔴 **照记忆写不出来的形态①：嵌套的「假接口头行」。**
///
/// `bridge0` 的成员行逐字是 `\tmember: en1 flags=3<LEARNING,DISCOVER>` ——
/// 与真正的接口头行 `en1: flags=8963<UP,…>` 几乎同形，区别只在列位置与 `flags=` 的位置。
///
/// 两种照直觉写法各有一种错法，这条把两种都钉住：
///  - 「含 `flags=` 就算一个接口」⇒ 凭空多出三个叫 `member` 的接口；
///  - 「`:` 前是名字」⇒ `en1`/`en2`/`en3` 各被登记两次，第二次带的是 bridge 成员的
///    `LEARNING,DISCOVER`，不是它们自己的 flags。
///
/// **射程如实登记**：这份抓取里的成员行是被「冒号后紧跟 `flags=`」那条判据挡掉的
/// （`en1` 夹在中间），**不是**被「顶格才算接口头行」挡掉的 —— 变异实测确认过：单独去掉
/// 列位置判据，本测试仍然全绿。列位置那条的射程由下一条测试单独钉，用的是构造输入。
#[test]
fn bridge_member_lines_are_not_mistaken_for_interfaces() {
    let raw = macos_ifconfig();
    // 前提：这份抓取里**真有**那种嵌套行。没有它，下面全是恒真断言。
    assert!(
        raw.contains("\tmember: en1 flags=3<LEARNING,DISCOVER>\n"),
        "这份抓取里没有 bridge 成员行 —— 本测试没有输入"
    );

    let roster = parse_macos_interface_flags(&raw);
    assert!(
        !roster.iter().any(|i| i.name == "member"),
        "成员行被当成了一个叫 `member` 的接口：{roster:?}"
    );
    let en1: Vec<&crate::route_probe::MacInterfaceFlags> =
        roster.iter().filter(|i| i.name == "en1").collect();
    assert_eq!(
        en1.len(),
        1,
        "`en1` 被登记了 {} 次 —— 成员行也被当成接口头行了：{en1:?}",
        en1.len()
    );
    assert!(
        en1[0].flags.iter().any(|f| f == "PROMISC"),
        "`en1` 拿到的是 bridge 成员行的 flags，不是它自己的：{:?}",
        en1[0].flags
    );
    assert!(
        !en1[0].flags.iter().any(|f| f == "LEARNING"),
        "`en1` 的 flags 里混进了 bridge 成员行的 `LEARNING`：{:?}",
        en1[0].flags
    );
}

/// 「**顶格才算接口头行**」这条判据的射程，单独钉。
///
/// ⚠️ 本条用的是**构造**输入而不是真机抓取，理由如实登记：2026-09-12 那份抓取里的
/// `bridge0` 成员行是 `\tmember: en1 flags=3<…>`，`en1` 夹在冒号与 `flags=` 之间，
/// 于是它在「冒号后紧跟 `flags=`」那条判据上就出局了，**轮不到**列位置这条。
/// 也就是说真样本证不了这条判据在做事（变异实测：单独去掉它，上一条测试仍全绿）。
///
/// 构造的这行是**逐字**的接口头行形状，只多一个前导制表符 —— 正向对照把制表符去掉，
/// 同一行立刻被认成接口。两条之间唯一的差别就是那个制表符，于是这条测试咬的正是它。
#[test]
fn an_indented_line_in_exact_interface_header_shape_is_still_a_continuation() {
    // 顶格的 `bridge0` 是真接口；缩进那行是构造的「逐字同形的续行」。
    let indented = "bridge0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\
                    \tmember: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST>\n";
    let roster = parse_macos_interface_flags(indented);
    assert_eq!(
        roster.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        vec!["bridge0"],
        "缩进的续行被当成了接口头行（列位置判据没在做事）：{roster:?}"
    );
    assert!(
        !parse_macos_tunnel_interfaces(indented)
            .expect("顶格那行能解析出来")
            .iter()
            .any(|t| t == "member"),
        "续行上的 POINTOPOINT 让 `member` 进了隧道名单"
    );

    // 正向对照：同一行**去掉前导制表符**就该被认成接口（且因带 POINTOPOINT 算隧道）。
    // 没有这条，上面可能只是「这行本来就解析不了」造成的假绿。
    let flush = indented.replace("\n\tmember:", "\nmember:");
    assert_ne!(flush, indented, "前提：确实只去掉了那个制表符");
    let roster = parse_macos_interface_flags(&flush);
    assert_eq!(
        roster.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        vec!["bridge0", "member"],
        "去掉制表符后应被认成接口 —— 否则上面那条否定断言没有信息量：{roster:?}"
    );
    assert!(
        parse_macos_tunnel_interfaces(&flush)
            .expect("解析")
            .iter()
            .any(|t| t == "member"),
        "顶格时它带着 POINTOPOINT，应进隧道名单"
    );
}

/// 🔴 **照记忆写不出来的形态②：空的 flags 列表 `flags=0<>`。**
///
/// `stf0: flags=0<> mtu 1280` —— 尖括号里什么都没有。按 `flags=\d+<([A-Z,]+)>` 这类
/// 「至少一个」的形状写，整行匹配不上 ⇒ `stf0` 从花名册里**静默消失**。
///
/// 顺带把「隧道判据是 flags 不是名字前缀」钉住：`stf0` 是 6to4 伪设备，名字一看就是隧道族，
/// 但这份抓取里它一个 flag 都没有 ⇒ 不在隧道名单里。这条边界写在
/// `parse_macos_tunnel_interfaces` 的头注里，这里是它会红的那一半。
#[test]
fn empty_flag_list_is_parsed_not_silently_dropped() {
    let raw = macos_ifconfig();
    assert!(
        raw.contains("stf0: flags=0<> mtu 1280\n"),
        "这份抓取里没有 `flags=0<>` 那一行 —— 本测试没有输入"
    );

    let roster = parse_macos_interface_flags(&raw);
    let stf0 = roster.iter().find(|i| i.name == "stf0").unwrap_or_else(|| {
        panic!("`stf0` 从花名册里消失了（空 flags 列表被当成不匹配）：{roster:?}")
    });
    assert!(
        stf0.flags.is_empty(),
        "`flags=0<>` 应解析成空列表，实际：{:?}",
        stf0.flags
    );

    let tunnels = parse_macos_tunnel_interfaces(&raw).expect("真机 ifconfig 应解析得动");
    assert!(
        !tunnels.iter().any(|t| t == "stf0"),
        "`stf0` 一个 flag 都没有，不该进隧道名单（判据是 flags 不是名字前缀）：{tunnels:?}"
    );
}

/// `gif0`：**DOWN、一条地址行都没有**，但 flags 里有 `POINTOPOINT` ⇒ 算隧道。
///
/// 这条钉的是「判据只有 flags」：加一句「还得 UP」或「还得有地址」都会让它红。
/// 而那两条加法看起来都很合理 —— 正是照记忆会顺手加上的东西。
#[test]
fn a_down_tunnel_device_without_addresses_still_counts() {
    let roster = parse_macos_interface_flags(&macos_ifconfig());
    let gif0 = roster
        .iter()
        .find(|i| i.name == "gif0")
        .expect("`gif0` 应在花名册里");
    assert_eq!(
        gif0.flags,
        vec!["POINTOPOINT".to_string(), "MULTICAST".to_string()],
        "`gif0` 的 flags 与抓取对不上"
    );
    assert!(
        !gif0.flags.iter().any(|f| f == "UP"),
        "前提：`gif0` 在这份抓取里是 DOWN 的，否则下一句没有信息量"
    );
    assert!(
        parse_macos_tunnel_interfaces(&macos_ifconfig())
            .expect("解析")
            .iter()
            .any(|t| t == "gif0"),
        "DOWN 的隧道设备被漏掉了 —— 判据里混进了「还得 UP」"
    );
    // 判据常量与断言里那个字面量是同一个，改一个忘一个时这里会红。
    assert_eq!(MAC_TUNNEL_FLAG, "POINTOPOINT");
}

// ══════════ Windows：对照表 + `route print`（2026-09-12 抓取，zh-CN / gb2312）══════════

fn windows_names() -> WindowsInterfaceNames {
    windows_names_of(WINDOWS_FIXTURE)
}

fn windows_names_of(file: &str) -> WindowsInterfaceNames {
    parse_get_netipaddress(&section(&read_fixture(file), "GET_NETIPADDRESS"))
        .unwrap_or_else(|e| panic!("{file} 的 Get-NetIPAddress 应解析得动：{e}"))
}

fn windows_routes(section_id: &str) -> Vec<RouteEntry> {
    windows_routes_of(WINDOWS_FIXTURE, section_id)
}

fn windows_routes_of(file: &str, section_id: &str) -> Vec<RouteEntry> {
    parse_route_print_routes(
        &section(&read_fixture(file), section_id),
        &windows_names_of(file),
    )
    .unwrap_or_else(|e| panic!("{file} 的 @@@{section_id} 应解析得动：{e}"))
}

/// 🔴 **判据 3**：路由解析出的接口名是**真名**，不是本地 IP、不是接口索引。
///
/// 这是 Windows 与 mac/Linux 最大的一处不对称：两张路由表的「接口」列分别是本地 IP
/// 与接口索引，**都不是名字**。没有对照表这一步，结果里出现的会是 `192.168.10.207` 和 `8`。
#[test]
fn windows_routes_carry_real_interface_names_not_ips_or_indexes() {
    let v4 = windows_routes("ROUTE_PRINT_4");
    let v6 = windows_routes("ROUTE_PRINT_6");

    let has = |rs: &[RouteEntry], prefix: &str, iface: &str| {
        rs.contains(&RouteEntry {
            prefix: prefix.into(),
            interface: iface.into(),
        })
    };

    // v4：`Interface` 列逐字是 `192.168.10.207` / `127.0.0.1` —— 翻成了别名。
    assert!(
        has(&v4, "192.168.10.0/24", "以太网"),
        "v4 的本地 IP 没被翻成接口别名：{v4:?}"
    );
    assert!(
        has(&v4, "127.0.0.0/8", "Loopback Pseudo-Interface 1"),
        "v4 的环回路由没被翻成接口别名（注意这个名字**带空格**，是真名不是脏数据）：{v4:?}"
    );
    // v6：`If` 列逐字是 `8` / `1` —— 翻成了同一套别名。
    assert!(
        has(&v6, "aded:14a8:b9be:ee8b::/64", "以太网"),
        "v6 的接口索引没被翻成接口别名：{v6:?}"
    );
    assert!(
        has(&v6, "::1/128", "Loopback Pseudo-Interface 1"),
        "v6 的索引 1 没被翻成接口别名：{v6:?}"
    );

    // 否定面：结果里绝不许出现 IP 字面量或纯数字当接口名。
    for r in v4.iter().chain(v6.iter()) {
        assert!(
            r.interface.parse::<std::net::IpAddr>().is_err(),
            "接口名是个 IP —— v4 表的 `Interface` 列直接漏出来了：{r:?}"
        );
        assert!(
            r.interface.parse::<u32>().is_err(),
            "接口名是个整数 —— v6 表的 `If` 列直接漏出来了：{r:?}"
        );
    }

    // **同一块网卡在 v4 与 v6 结果里必须是同一个名字**。用 `route print` 自带的
    // Interface List（`8...Red Hat VirtIO Ethernet Adapter`）去翻 v6 的索引就会破这一条：
    // 那一列是 `InterfaceDescription`，与 `InterfaceAlias` 是两个命名空间，
    // 于是同一块网卡在两半结果里叫两个名字，而 `foreign_tunnel_routes` 是逐字比名字的。
    assert!(
        v4.iter().any(|r| r.interface == "以太网") && v6.iter().any(|r| r.interface == "以太网"),
        "同一块网卡在 v4/v6 结果里不是同一个名字：v4={v4:?}\nv6={v6:?}"
    );
    assert!(
        !v6.iter()
            .any(|r| r.interface.contains("Red Hat VirtIO Ethernet Adapter")),
        "v6 的接口名取自 `route print` 的 Interface List（描述列），与 v4 的别名对不上：{v6:?}"
    );

    // 条目数逐字钉死：默认路由**产出条目**（各一条，都在 `以太网` 上），
    // 永久路由表那张（只有 4 列、没有接口列）仍然不产出条目。
    assert_eq!(v4.len(), 11, "v4 条目数与真机抓取对不上：{v4:?}");
    assert_eq!(v6.len(), 31, "v6 条目数与真机抓取对不上：{v6:?}");
    assert!(
        v4.contains(&RouteEntry {
            prefix: "0.0.0.0/0".into(),
            interface: "以太网".into()
        }),
        "v4 的默认路由没产出条目 —— 抢默认路由的全隧道正是靠它才看得见：{v4:?}"
    );
    assert!(
        v6.contains(&RouteEntry {
            prefix: "::/0".into(),
            interface: "以太网".into()
        }),
        "v6 的默认路由没产出条目：{v6:?}"
    );
}

/// 🔴 **判据 2**：解析**不依赖表头字面量**。
///
/// 两路证据：
///  1. `synthetic/windows-w207-route-print-en-headers.txt` —— 同一份抓取，本地化表头逐条
///     换成 en-US，**路由数据行一个字节没动**。解析结果必须逐条相同。
///  2. 同一份 zh-CN 抓取，把那几行表头就地换成方块字符（既不是中文也不是英文）。
///     还相同 ⇒ 解析器压根没读过表头，而不是「中英文两套都认」。
///
/// 没有这条，「按英文表头硬匹配的解析器在中文机器上解析出空表」这个缺陷会一路滑到真机。
#[test]
fn route_print_parsing_does_not_depend_on_localized_headers() {
    /// 这份抓取里全部**本地化**的表头/标签（与合成样本的派生脚本同一张表）。
    const LOCALIZED: &[&str] = &[
        "接口列表",
        "IPv4 路由表",
        "IPv6 路由表",
        "活动路由:",
        "永久路由:",
        "网络目标        网络掩码          网关       接口   跃点数",
        "  网络地址          网络掩码  网关地址  跃点数",
        " 接口跃点数网络目标                网关",
        "在链路上",
        "  无",
    ];

    let zh_raw = read_fixture(WINDOWS_FIXTURE);
    let en_raw = read_fixture(WINDOWS_SYNTHETIC_EN);
    let names = windows_names();

    for id in ["ROUTE_PRINT_4", "ROUTE_PRINT_6"] {
        let zh = section(&zh_raw, id);
        let en = section(&en_raw, id);

        // 正向对照先行：两份**确实不同**，且差异确实落在表头上。少了这条，
        // 「结果相同」可能只是因为我拿了同一份输入比自己。
        assert_ne!(zh, en, "@@@{id}：合成样本与真样本逐字相同 —— 表头没被换过");
        assert!(
            zh.contains("活动路由:") && en.contains("Active Routes:"),
            "@@@{id}：合成样本不是「中文表头换成英文」那种差异"
        );

        let zh_entries = parse_route_print_routes(&zh, &names).expect("zh-CN 真样本");
        let en_entries = parse_route_print_routes(&en, &names).expect("en 合成样本");
        assert!(
            !zh_entries.is_empty(),
            "@@@{id}：真样本解析出 0 条，对照塌了"
        );
        assert_eq!(
            zh_entries, en_entries,
            "@@@{id}：换了表头语言，解析结果就变了 —— 解析依赖了表头字面量"
        );

        // 第二路：换成方块字符。中英文都不是了，结果还得一样。
        let mut blocked = zh.clone();
        let mut swapped = 0usize;
        for lit in LOCALIZED {
            if blocked.contains(lit) {
                blocked = blocked.replace(lit, &"▓".repeat(lit.chars().count()));
                swapped += 1;
            }
        }
        assert!(
            swapped >= 4,
            "@@@{id}：只换掉了 {swapped} 个表头字面量 —— 这条对照没吃到几行"
        );
        assert_eq!(
            parse_route_print_routes(&blocked, &names).expect("表头变方块后仍应解析得动"),
            zh_entries,
            "@@@{id}：表头换成方块字符后结果变了 —— 解析器在读表头"
        );
    }
}

/// 🔴 **照记忆写不出来的形态：`route print -6` 会把网关折到下一行。**
///
/// 目的地长到一定程度时，这一条路由**占两行**：
/// ```text
///   8    271 aded:14a8:b9be:ee8b:f856:b1c9:93f7:98b0/128
///                                     在链路上
/// ```
/// 要求「一行四个 token」的写法会把这批 /128 主机路由**静默丢掉** ——
/// 而它们恰好是最像「某个隧道在宣告地址」的那批。这份抓取里有 18 行是折行形态。
#[test]
fn route_print_v6_wrapped_rows_are_not_dropped() {
    let body = section(&read_fixture(WINDOWS_FIXTURE), "ROUTE_PRINT_6");

    // 前提：这份抓取里**真有**折行。逐字找一对（长目的地行 + 只有网关的续行）。
    assert!(
        body.contains(
            "  8    271 aded:14a8:b9be:ee8b:f856:b1c9:93f7:98b0/128\n                                    在链路上\n"
        ),
        "这份抓取里没有折行形态 —— 本测试没有输入"
    );
    let wrapped_rows = body
        .lines()
        .filter(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            t.len() == 3
                && t[0].parse::<u32>().is_ok()
                && t[1].parse::<u32>().is_ok()
                && t[2].contains('/')
        })
        .count();
    assert_eq!(wrapped_rows, 18, "折行的行数与抓取对不上（样本换了？）");

    let entries = windows_routes("ROUTE_PRINT_6");
    assert!(
        entries.contains(&RouteEntry {
            prefix: "aded:14a8:b9be:ee8b:f856:b1c9:93f7:98b0/128".into(),
            interface: "以太网".into(),
        }),
        "折行那条路由被丢掉了：{entries:?}"
    );
    // 反向对照：把续行删掉（= 折行变成「只有三个 token 的孤行」），条目数不许变 ——
    // 证明取的是**首行**的目的地，不是靠续行凑数。
    let without_continuations: String = body
        .lines()
        .filter(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            !(t.len() == 1 && l.starts_with("      "))
        })
        .map(|l| format!("{l}\n"))
        .collect();
    assert_ne!(without_continuations, body, "前提：确实删掉了一些续行");
    assert_eq!(
        parse_route_print_routes(&without_continuations, &windows_names()).expect("解析"),
        entries,
        "续行参与了结果 —— 折行那条路由的信息不该来自续行"
    );
}

/// 对照表**必须**来自 `Get-NetIPAddress`：`Get-NetAdapter -IncludeHidden` 里根本没有
/// `Loopback Pseudo-Interface 1`（索引 1），而 `route print -6` 在索引 1 上有两条路由。
///
/// 「隐藏适配器枚举里没有环回伪接口」是照记忆写不出来的 —— 直觉恰恰相反
/// （`-IncludeHidden` 听起来就该「什么都有」）。
#[test]
fn interface_table_covers_the_loopback_index_absent_from_get_netadapter() {
    let raw = read_fixture(WINDOWS_FIXTURE);
    let adapters = section(&raw, "GET_NETADAPTER");

    // 前提：`Get-NetAdapter` 确实抓到了东西，但里面确实没有环回。
    assert!(
        adapters.contains("InterfaceIndex"),
        "@@@GET_NETADAPTER 是空的 —— 下一句没有信息量"
    );
    assert!(
        !adapters.contains("Loopback"),
        "这份 Get-NetAdapter 里有环回了 —— 本测试登记的那条不对称已经过期，请同步更新 README 覆盖表"
    );

    let names = windows_names();
    assert_eq!(
        names.by_index(1),
        Some("Loopback Pseudo-Interface 1"),
        "索引 1 不在对照表里 —— 只拿 Get-NetAdapter 建表就是这个下场"
    );
    assert_eq!(names.by_index(8), Some("以太网"));
    assert_eq!(names.by_ip("192.168.10.207"), Some("以太网"));
    assert_eq!(
        names.by_ip("127.0.0.1"),
        Some("Loopback Pseudo-Interface 1")
    );
    // `%作用域` 在建索引前剥掉、地址按 `IpAddr` 规范化 —— 否则 `route print` 那侧查不上。
    assert_eq!(
        names.by_ip("fe80::3463:e45e:6a9c:9334"),
        Some("以太网"),
        "`fe80::…%8` 的作用域没剥掉，建出来的键查不上"
    );
}

/// 接口查不到时**报错**，不折成「少几条的 Ok」。
///
/// 带正向对照：同一份输入配上真表时是 `Ok` 且非空 —— 否则这条红的可能只是「压根解析不动」。
#[test]
fn an_unresolvable_interface_is_an_error_not_a_quietly_shorter_list() {
    let body = section(&read_fixture(WINDOWS_FIXTURE), "ROUTE_PRINT_4");
    assert!(
        parse_route_print_routes(&body, &windows_names()).is_ok_and(|v| !v.is_empty()),
        "正向对照：配上真表时应解析出非空结果"
    );
    let err = parse_route_print_routes(&body, &WindowsInterfaceNames::default())
        .expect_err("空对照表时必须报错");
    assert!(
        matches!(
            err,
            RouteTableParseError::UnresolvedInterface {
                table_entries: 0,
                ..
            }
        ),
        "应报「接口查不到」且带上表长度 0：{err}"
    );
}

/// `route print -4` 末尾那张**永久路由表**只有 4 列、**没有接口列**，不许被当成活动路由。
///
/// ⚠️ 本条用的是**构造**输入而不是真机抓取：那份抓取里唯一一条永久路由恰好是默认路由，
/// 而活动路由表里本来就有同一条默认路由 —— 误当成活动路由时产出的是一条**重复**条目，
/// 与「少列数那条腿」的失败形态混在一起看不出来。构造一条**非默认**的永久路由来补这个缺口，
/// 并在此如实登记它是构造的。
#[test]
fn the_persistent_routes_table_is_not_mistaken_for_active_routes() {
    // 列头与标签逐字取自真样本（zh-CN），只有那条永久路由是构造的。
    let body = "===========================================================================\n\
                活动路由:\n\
                网络目标        网络掩码          网关       接口   跃点数\n\
                     192.168.10.0    255.255.255.0            在链路上    192.168.10.207    271\n\
                ===========================================================================\n\
                永久路由:\n\
                  网络地址          网络掩码  网关地址  跃点数\n\
                       10.9.0.0    255.255.0.0     192.168.10.1       1\n\
                ===========================================================================\n";
    let entries = parse_route_print_routes(body, &windows_names()).expect("应解析得动");
    assert_eq!(
        entries,
        vec![RouteEntry {
            prefix: "192.168.10.0/24".into(),
            interface: "以太网".into()
        }],
        "永久路由表里那条被当成了活动路由（它只有 4 列、没有接口列，硬解只能张冠李戴）"
    );
    assert!(
        !entries.iter().any(|r| r.prefix.starts_with("10.9.")),
        "永久路由产出了条目：{entries:?}"
    );
}

/// 🔴 **独立交叉验证**：`route print` 的解析结果 vs 同一份抓取里 `Get-NetRoute` 的读数。
///
/// `Get-NetRoute` 一列就给出 `DestinationPrefix` + `InterfaceAlias` —— 它是**另一条读法**，
/// 不经过「本地 IP / 接口索引 → 名字」那一整套对照。两路逐条相同，才说明那套对照
/// 与 Windows 自己的账本对得上；只对着自己写的解析器断言，证的永远只是「我和我一致」。
///
/// 参照解析器**刻意在测试里另写一份**（按列头 + 分隔线切列），不复用生产代码的切列函数：
/// 复用等于两边共用同一个错，对差就没有检出力了。
///
/// 边界：`Get-NetRoute` 依赖 NetTCPIP 模块，不是每个 SKU 都有 —— 故它只做参照，
/// 生产读法仍是 `route print`（原生 exe）。
#[test]
fn route_print_result_matches_the_independent_get_netroute_reading() {
    /// 极简参照实现：`Format-Table` 输出 → `(DestinationPrefix, InterfaceAlias)`。
    /// 按**字符**切（这份抓取里有 `以太网`，按字节切会切在码点中间）。
    fn reference_rows(body: &str, want: &[&str]) -> Vec<Vec<String>> {
        let lines: Vec<&str> = body.lines().collect();
        let head = lines
            .iter()
            .position(|l| want.iter().all(|w| l.contains(w)))
            .expect("参照分节里应有列头行");
        let dashes: Vec<char> = lines[head + 1].chars().collect();
        assert!(
            dashes.contains(&'-'),
            "列头下一行不是分隔线行，参照实现的前提不成立"
        );
        let mut starts = Vec::new();
        let mut prev_dash = false;
        for (i, c) in dashes.iter().enumerate() {
            if *c == '-' && !prev_dash {
                starts.push(i);
            }
            prev_dash = *c == '-';
        }
        let cut = |line: &str| -> Vec<String> {
            let cs: Vec<char> = line.chars().collect();
            starts
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let e = starts.get(i + 1).copied().unwrap_or(cs.len()).min(cs.len());
                    cs[(*s).min(e)..e]
                        .iter()
                        .collect::<String>()
                        .trim()
                        .to_string()
                })
                .collect()
        };
        let header = cut(lines[head]);
        let cols: Vec<usize> = want
            .iter()
            .map(|w| {
                header
                    .iter()
                    .position(|c| c == w)
                    .unwrap_or_else(|| panic!("参照列头里没有 `{w}`：{header:?}"))
            })
            .collect();
        lines[head + 2..]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let cells = cut(l);
                cols.iter().map(|c| cells[*c].clone()).collect()
            })
            .collect()
    }

    // 🔴 跑遍**每一份** Windows 抓取，不是只对着最早那几份。取材面写死名单时，后来入库的
    // 抓取压根没人拿它们对差 —— 而「静默丢行」正是路由数量多起来之后才容易发生的事
    // （折行、30 条隧道路由、RAS 那条抢默认路由的全隧道）。
    //
    // 条数表必须**逐份**覆盖枚举出来的文件：新抓取入库那天这里会红在「表里没有它」上，
    // 逼人真去看一眼那份抓取对不对得上，而不是让它悄悄不进对差面。
    const EXPECTED_ROWS: &[(&str, usize)] = &[
        (WINDOWS_FIXTURE, 42),
        (WINDOWS_FIXTURE_WINTUN, 45),
        (WINDOWS_FIXTURE_TS_ON, 75),
        (WINDOWS_FIXTURE_TAP, 85),
        (WINDOWS_FIXTURE_HYPERV, 44),
        // 比别的抓取多一条：`以太网` 与 `PolarisProbeL2TP` 各宣告一条 `0.0.0.0/0`。
        (WINDOWS_FIXTURE_RAS, 52),
        // 虚拟化全家福：两条默认路由都在 `以太网`（物理网卡）上，三型虚拟交换机一条外来
        // 默认路由都不产出 —— 这正是「虚拟化是常规场景」那条兼容性的输入面。
        (WINDOWS_FIXTURE_VIRT, 52),
    ];
    let enumerated = windows_fixture_names();
    let mut listed: Vec<&str> = EXPECTED_ROWS.iter().map(|(f, _)| *f).collect();
    listed.sort_unstable();
    assert_eq!(
        listed,
        enumerated.iter().map(String::as_str).collect::<Vec<_>>(),
        "条数表与夹具目录对不上 —— 新抓取要在表里登记一行（先跑一次拿到它的条数）"
    );

    for (file, want_rows) in EXPECTED_ROWS.iter().copied() {
        let raw = read_fixture(file);

        // 参照侧：Get-NetRoute 的 (前缀, 接口别名)。**默认路由两侧都收**：它是外来隧道
        // 最重的那一条宣告（RAS 那份里 `0.0.0.0/0 PolarisProbeL2TP` 两张表都有），
        // 从对差面里剔掉就等于让这一类的解析没有任何独立读数可对。
        //
        // 🔴 v6 前缀要**先规范化再比**，不能逐字比字符串：`route print` 那侧的解析走
        // `Ipv6Addr`，`0ec2` 会被规范成 `ec2`；而参照侧是从文本里逐字切出来的。
        // 两种写法在真机上本来同形（Windows 自己不打前导零），是**脱敏**造出的差异 ——
        // 「同长度替换」把一个组换成了带前导零的 `0ec2` / `042f` / `0cf5`。
        // 规范化用 stdlib 的 `Ipv6Addr`，不碰生产的 `canonical_ipv6_prefix`：复用它等于两边
        // 共用同一个错，这条对差就没有检出力了。
        let canonical = |p: &str| -> String {
            let Some((addr, len)) = p.split_once('/') else {
                return p.to_string();
            };
            match addr.parse::<Ipv6Addr>() {
                Ok(a) => format!("{a}/{len}"),
                Err(_) => p.to_string(),
            }
        };
        //
        // 🔴 第二条真差异：**`Get-NetRoute` 列出 `Disconnected` 接口的路由，`route print` 不列**。
        // 同一对 2026-09-13 抓取里正负两例俱全，所以这不是猜的：`OpenVPN TAP-Windows6` 在
        // `…-ovpn-tun-connected-…` 里是 `Up`，两侧都有它那几条；在 `…-hyperv-present-…` 里是
        // `Disconnected`，只有 `Get-NetRoute` 那侧有。故参照侧按同一份抓取的 `Get-NetAdapter`
        // 把**非 Up** 的适配器整条剔掉。
        //
        // 判据写成「在适配器表里**且** Status != Up 才剔」，不是「不在 Up 名单里就剔」——
        // `Loopback Pseudo-Interface 1` 压根不在 `Get-NetAdapter` 里（`-IncludeHidden` 也没有），
        // 按后者写会把它那 5 条一起误杀，而它们在两侧都真实存在。
        let adapters = cut_adapter_table(&section(&raw, "GET_NETADAPTER"));
        let not_up: Vec<&str> = adapters
            .rows
            .iter()
            .filter(|r| adapters.cell(r, "Status") != "Up")
            .map(|r| adapters.cell(r, "InterfaceAlias"))
            .collect();
        let mut reference: Vec<(String, String)> = ["GET_NETROUTE_4", "GET_NETROUTE_6"]
            .iter()
            .flat_map(|id| {
                reference_rows(&section(&raw, id), &["DestinationPrefix", "InterfaceAlias"])
            })
            .filter(|row| !not_up.contains(&row[1].as_str()))
            .map(|row| (canonical(&row[0]), row[1].clone()))
            .collect();

        // 生产侧：`route print` 经对照表解析出来的那份。
        let mut produced: Vec<(String, String)> = ["ROUTE_PRINT_4", "ROUTE_PRINT_6"]
            .iter()
            .flat_map(|id| windows_routes_of(file, id))
            .map(|e| (e.prefix, e.interface))
            .collect();

        // 前提：两边都非空，否则「相等」毫无信息量。
        assert!(!reference.is_empty(), "{file}：参照侧读出 0 条");
        assert!(!produced.is_empty(), "{file}：生产侧解析出 0 条");

        reference.sort();
        produced.sort();
        assert_eq!(
            produced, reference,
            "{file}：`route print` 的解析与 `Get-NetRoute` 的独立读数对不上 —— \
             多出来的是误解析，少掉的是静默丢行，两种都致命"
        );
        assert_eq!(
            produced.len(),
            want_rows,
            "{file}：条目总数与这份抓取对不上（样本换了？）"
        );
    }
}

/// 🔴 **Windows 隧道判据 = IANA ifType `∈ {131, 53}`**，正负样本各自点名。
///
/// 正样本四个：`131` 三个（Teredo / IP-HTTPS / 6to4）+ `53` 一个（Tailscale 的 wintun，
/// 2026-09-12 真机实测）。负样本两个（两块以太网卡是 `6`），负向断言带正向对照：
/// 先证明 `以太网` **确实在名单输入里**，否则「它没被判成隧道」可能只是因为它压根没进来。
///
/// **`53` 那一支是判据的全部新东西**：把 [`crate::route_probe::WINDOWS_TUNNEL_IF_TYPES`]
/// 改回只有 `131`（= 2026-09-12 之前那个判据），本条与
/// `the_three_windows_captures_differ_by_exactly_the_wintun_adapter` 会红。
#[test]
fn windows_tunnel_criterion_is_iana_iftype_131_or_53_and_ethernet_is_neither() {
    // 判据面逐字钉死：两个值各自的实测正样本写在 route_probe 的 IF_TYPE_* 常量头注里，
    // 改一个忘一个时这里先红。
    assert_eq!(
        crate::route_probe::WINDOWS_TUNNEL_IF_TYPES,
        &[131_u32, 53],
        "判据面变了 —— 每个值都必须有真机正样本，依据见 route_probe 的 IF_TYPE_* 头注"
    );

    let adapters = section(&read_fixture(WINDOWS_FIXTURE_WINTUN), "GET_NETADAPTER");
    let tunnels = parse_windows_tunnel_interfaces(&adapters).expect("真机抓取应解析得动");

    // 逐条列全，不只断包含关系：多一个（把网卡当隧道）与少一个（漏掉隧道）是两种相反的错。
    // 顺序 = 抓取里的行序。
    assert_eq!(
        tunnels,
        vec![
            "Teredo Tunneling Pseudo-Interface",     // 131
            "Tailscale",                             // 53 ← wintun，判据 53 那一半的全部依据
            "Microsoft IP-HTTPS Platform Interface", // 131
            "6to4 Adapter",                          // 131
        ],
        "隧道名单与真机 Get-NetAdapter 对不上"
    );

    // 负样本：两块以太网卡都不是隧道。
    for not_a_tunnel in ["以太网", "以太网(内核调试器)"] {
        assert!(
            !tunnels.iter().any(|t| t == not_a_tunnel),
            "`{not_a_tunnel}` 被判成隧道了：{tunnels:?}"
        );
        // 🔴 正向对照：它**确实**在这份输入里，否则上一条否定断言只是「它压根没进来」造成的假绿。
        assert!(
            adapters.contains(not_a_tunnel),
            "`{not_a_tunnel}` 根本不在 @@@GET_NETADAPTER 里 —— 上一条断言没有信息量"
        );
    }

    // 🔴 `131` 那一支不许因为加宽而回归：装 Tailscale **之前**那份同机抓取上，
    // 隧道名单仍然逐条是原来那三个。
    let before =
        parse_windows_tunnel_interfaces(&section(&read_fixture(WINDOWS_FIXTURE), "GET_NETADAPTER"))
            .expect("装 Tailscale 之前那份抓取应解析得动");
    assert_eq!(
        before,
        vec![
            "Teredo Tunneling Pseudo-Interface",
            "Microsoft IP-HTTPS Platform Interface",
            "6to4 Adapter",
        ],
        "`131` 那一支在加宽之后回归了"
    );
}

/// 🔴 **三份同机抓取的差分**（无 wintun → wintun up 未登录 → wintun up 已登录）：
/// 同一判据跑三遍，隧道名单的差集**恰好**是 `Tailscale` 一项，而且**只在第一步出现**。
///
/// 这条比单独断言任何一份都强 —— 它一次否掉四种错：
///  - 判据没收到 wintun（第一步差集为空）；
///  - 判据顺手多收了别的（第一步差集里不止它）；
///  - 加宽反而丢了原来认得的（反向差集非空）；
///  - **判据跟着连接态漂**（第二步差集非空）—— 适配器的 ifType 是驱动属性，不该因为
///    登录 / 未登录而变；这一步的差全在路由表上，不在判据上。
///
/// **前提也断在这里**：三份抓取的适配器**输入**名单，除 `Tailscale` 那一行外完全相同。
/// 缺了这半，「差集只有 Tailscale」可能只是因为三份抓取本来就差着别的东西。
#[test]
fn the_three_windows_captures_differ_by_exactly_the_wintun_adapter() {
    // 列切法独立于生产实现。
    let aliases = |file: &str| -> Vec<String> {
        let raw = read_fixture(file);
        let t = cut_adapter_table(&section(&raw, "GET_NETADAPTER"));
        t.rows
            .iter()
            .map(|r| t.cell(r, "InterfaceAlias").to_string())
            .collect()
    };
    let tunnels = |file: &str| -> Vec<String> {
        parse_windows_tunnel_interfaces(&section(&read_fixture(file), "GET_NETADAPTER"))
            .unwrap_or_else(|e| panic!("{file} 的适配器段应解析得动：{e}"))
    };
    let diff = |from: &[String], to: &[String]| -> Vec<String> {
        to.iter().filter(|x| !from.contains(x)).cloned().collect()
    };

    let (off, present, on) = (
        aliases(WINDOWS_FIXTURE),
        aliases(WINDOWS_FIXTURE_WINTUN),
        aliases(WINDOWS_FIXTURE_TS_ON),
    );

    // ── 前提：输入只差 wintun 那一行，且后两份逐条相同 ──
    assert_eq!(
        diff(&off, &present),
        ["Tailscale"],
        "「装 wintun 之前 → 之后」的适配器输入不是只多了 wintun 那一行 —— \
         下面的差分结论会张冠李戴。前：{off:?}\n后：{present:?}"
    );
    assert!(
        diff(&present, &off).is_empty(),
        "装 wintun 之后少了适配器 —— 这两份不再是一对对照：{:?}",
        diff(&present, &off)
    );
    assert_eq!(
        present, on,
        "未登录 / 已登录两份的适配器名单不一致 —— 「第二步的差全在路由表上」这条前提没了"
    );

    // ── 结论：判据的差集恰好是 {Tailscale}，且只在第一步 ──
    let (t_off, t_present, t_on) = (
        tunnels(WINDOWS_FIXTURE),
        tunnels(WINDOWS_FIXTURE_WINTUN),
        tunnels(WINDOWS_FIXTURE_TS_ON),
    );
    assert_eq!(
        diff(&t_off, &t_present),
        ["Tailscale"],
        "判据在带 wintun 的那份上多认出来的不是 `Tailscale` —— 把判据改回只认 131 就是这条红。\
         前：{t_off:?}\n后：{t_present:?}"
    );
    assert!(
        diff(&t_present, &t_off).is_empty(),
        "加宽判据反而丢了隧道：{:?}",
        diff(&t_present, &t_off)
    );
    assert_eq!(
        t_present, t_on,
        "同一张 wintun 适配器在未登录 / 已登录两份上被判得不一样 —— 判据跟着连接态漂了"
    );
    // 条数逐份钉死：3（只有 131）→ 4（多了 53 那张）→ 4。
    assert_eq!(
        [t_off.len(), t_present.len(), t_on.len()],
        [3, 4, 4],
        "三份抓取的隧道条数与登记对不上：{t_off:?} / {t_present:?} / {t_on:?}"
    );
}

/// **为什么不叠交叉判据**：另外两列在真实数据上要么不分隔，要么已被 wintun 那行直接证伪。
///
/// 这条不是在测生产代码，是把「只认 ifType 一列」这个决定的**依据**钉在真机数据上 ——
/// 依据一旦变了它会红，提醒重新评估，而不是让一句注释里的理由无声过期。
///
/// 两份抓取各钉一半，因为这两条驳回理由的证据在不同的抓取里：
///  - `NdisPhysicalMedium` 不分隔 —— 两份都成立（隧道与 `以太网` 同为 `0`）；
///  - `ComponentID` 这条启发式 —— 在装 Tailscale **之前**那份上「看着能用」（隧道全空、
///    网卡不空），正是这种看着能用让它危险；而**之后**那份直接把它证伪（wintun 的
///    `ComponentID` = `Wintun`，不空，却是货真价实的隧道）。
#[test]
fn the_two_rejected_cross_criteria_are_rejected_for_reasons_visible_in_the_capture() {
    // ── 驳回理由①：`NdisPhysicalMedium` 在两份抓取上都不分隔隧道与网卡 ──
    for file in [WINDOWS_FIXTURE, WINDOWS_FIXTURE_WINTUN] {
        let raw = read_fixture(file);
        let table = cut_adapter_table(&section(&raw, "GET_NETADAPTER"));
        let tunnel_media: Vec<&str> = crate::route_probe::WINDOWS_TUNNEL_IF_TYPES
            .iter()
            .flat_map(|t| table.rows_with_if_type(*t))
            .map(|r| table.cell(r, "NdisPhysicalMedium"))
            .collect();
        assert!(
            !tunnel_media.is_empty(),
            "{file}：一个隧道适配器都没有 —— 下面那条断言没有信息量"
        );
        assert!(
            tunnel_media.iter().all(|m| *m == "0"),
            "{file}：隧道的 NdisPhysicalMedium 不再都是 0：{tunnel_media:?}"
        );
        let ethernet_medium = table
            .rows
            .iter()
            .find(|r| table.cell(r, "InterfaceAlias") == "以太网")
            .map(|r| table.cell(r, "NdisPhysicalMedium"))
            .expect("`以太网` 在名单里");
        assert_eq!(
            ethernet_medium, "0",
            "{file}：`以太网` 的 NdisPhysicalMedium 不再是 0 —— 「这一列不分隔」这条驳回理由\
             已过期，请重新评估要不要把它加进判据"
        );
    }

    // ── 驳回理由②上半：装 Tailscale 之前那份上，`ComponentID` 看着分得开 ──
    let before = cut_adapter_table(&section(&read_fixture(WINDOWS_FIXTURE), "GET_NETADAPTER"));
    assert!(
        before
            .rows_with_if_type(131)
            .iter()
            .all(|r| before.cell(r, "ComponentID").is_empty()),
        "装 Tailscale 之前那份里，三个隧道的 ComponentID 不再都是空 —— 驳回理由②的前提变了"
    );
    assert!(
        before
            .rows_with_if_type(6)
            .iter()
            .all(|r| !before.cell(r, "ComponentID").is_empty()),
        "装 Tailscale 之前那份里，两块网卡的 ComponentID 不再都非空 —— 驳回理由②的前提变了"
    );

    // ── 驳回理由②下半（真机反例）：wintun 是隧道，但 `ComponentID` 不空 ──
    // 这半是 2026-09-12 之后才有的。在它之前，「ifType=131」与「ComponentID 为空」在真样本上
    // 圈中的是同一批三个适配器，两条判据完全混在一起、分不开 —— 只能靠推理驳回 ②。
    // 现在有真机反例了：叠上「且 ComponentID 为空」会把唯一真要防的那个排除掉。
    let after = cut_adapter_table(&section(
        &read_fixture(WINDOWS_FIXTURE_WINTUN),
        "GET_NETADAPTER",
    ));
    let wintun = after
        .rows
        .iter()
        .find(|r| after.cell(r, "InterfaceAlias") == "Tailscale")
        .expect("wintun 那行在名单里");
    assert_eq!(after.cell(wintun, "InterfaceType"), "53");
    assert_eq!(
        after.cell(wintun, "ComponentID"),
        "Wintun",
        "wintun 的 ComponentID 变了 —— 驳回理由②的真机反例就是这一格（注意实测是首字母大写的\
         `Wintun`，不是 `wintun`）"
    );
}

/// 🔴 **判据不叠「且 `ComponentID` 为空」**：`ComponentID` 非空的适配器仍然算隧道。
///
/// ⚠️ 上一版**不得不用构造输入**，如实登记过原因：那时的真抓取里「`InterfaceType == 131`」
/// 与「`ComponentID` 为空」圈中的是同一批三个适配器 —— 真机数据把这两条判据完全混在一起，
/// 分不开。能分开它们的恰恰是当时缺席的那个 wintun。
///
/// 2026-09-12 那份抓取入库之后**不必构造了**：`Tailscale` 的 `ComponentID` 是 `Wintun`
/// （不空），而它仍被判成隧道。这个决定从「构造出来的推理」变成了真机事实，构造那张表随之删掉
/// —— 同一件事有真样本时，构造输入只会让人以为它还没有。
#[test]
fn the_tunnel_criterion_does_not_also_require_an_empty_component_id() {
    let raw = read_fixture(WINDOWS_FIXTURE_WINTUN);
    let body = section(&raw, "GET_NETADAPTER");

    // 前提：那一行的 ComponentID 确实不空（否则本条恒真）。
    let table = cut_adapter_table(&body);
    let wintun = table
        .rows
        .iter()
        .find(|r| table.cell(r, "InterfaceAlias") == "Tailscale")
        .expect("wintun 那行在名单里");
    assert!(
        !table.cell(wintun, "ComponentID").is_empty(),
        "wintun 的 ComponentID 是空的 —— 本条断言就没有信息量了"
    );

    let tunnels = parse_windows_tunnel_interfaces(&body).expect("真机抓取应解析得动");
    assert!(
        tunnels.iter().any(|t| t == "Tailscale"),
        "带 ComponentID 的隧道被排除掉了 —— 判据里混进了「且 ComponentID 为空」，\
         而那正是会漏掉 wintun 的那条土办法：{tunnels:?}"
    );
}

// ══════════ Linux（2026-09-13 VM185：OpenVPN 2.7 的 ovpn-dco）══════════

/// 拿 VM185 那份抓取的五个判据分节，跑一遍**真正的** Linux 腿（纯函数那一支）。
///
/// 走 [`assemble_linux_probe`] 而不是只调 [`parse_ip_link_names`]：要证的不是「解析器认得
/// `@@@LINK_OVPN` 里那两行」，而是「这两个接口最终进了 `tunnel_interfaces`、它们宣告的网段
/// 最终进了 `foreign`」—— 中间还隔着三张链路表的合并去重，那一步漏掉的话名字认对了也没用。
fn linux_leg(file: &str) -> ForeignTunnelSnapshot {
    let raw = read_fixture(file);
    assemble_linux_probe(
        &section(&raw, "V4_ROUTE"),
        &section(&raw, "V6_ROUTE"),
        &section(&raw, "LINK_TUN"),
        &section(&raw, "LINK_WIREGUARD"),
        &section(&raw, "LINK_OVPN"),
        &[],
    )
}

/// 🔴 **Linux 腿在 ovpn-dco 抓取上看得见全部三条隧道**（`tap0` / `tun0` / `tun1`）。
///
/// # 这份抓取证的是什么
///
/// OpenVPN 2.7 在 Linux 上默认走 ovpn-dco 内核模块，设备的 **link type 是 `ovpn`**，
/// 而设备**名字**仍叫 `tunN`。于是同一台机器上：
///
/// | 查询 | 回什么 |
/// |---|---|
/// | `ip -o link show type tun` | 只有 `tap0`（传统 TAP） |
/// | `ip -o link show type ovpn` | `tun0`、`tun1` |
///
/// 补 `ovpn` 这条腿**之前**，`tun1` 上那条 `198.18.42.0/24`（落在 FakeIP 段 `198.18.0.0/15`
/// 里）压根进不了 `tunnel_interfaces`，`detect_tunnel_conflicts` 于是在这台机器上返回
/// **0 条冲突** —— 那句自信的「无冲突」正是整条链路存在的理由要防的。
///
/// # 名字的**来源**也断，不只断结果
///
/// 只断「名单里有 `tun0`」的话，一个从 `@@@LINK_DETAIL`（`ip -d link show`，里面什么都有）
/// 或从路由表 `dev` 列反推名字的实现也能全绿 —— 而那两种都不是生产读法。故先逐段断三张
/// 链路表各自解析出什么，再断合并结果。
#[test]
fn linux_leg_on_the_ovpn_dco_capture_sees_tun0_tun1_and_tap0() {
    let raw = read_fixture(LINUX_FIXTURE_OVPN_DCO);

    // ── ① 名字的来源：三段各自解析出什么 ──
    assert_eq!(
        parse_ip_link_names(&section(&raw, "LINK_TUN")),
        vec!["tap0".to_string()],
        "`type tun` 这张表在这台机器上**只有** tap0 —— 两条 OpenVPN 隧道一条都不在里面。\
         这正是本份抓取要证的那件事；它要是也回了 tunN，说明夹具换了，下面的结论全要重核"
    );
    assert!(
        parse_ip_link_names(&section(&raw, "LINK_WIREGUARD")).is_empty(),
        "这台机器没装 wireguard 模块，那张表该是空的"
    );
    assert_eq!(
        parse_ip_link_names(&section(&raw, "LINK_OVPN")),
        vec!["tun0".to_string(), "tun1".to_string()],
        "`type ovpn` 这张表才是 tun0 / tun1 的唯一来源"
    );

    // ── ② 合并结果 ──
    let snapshot = linux_leg(LINUX_FIXTURE_OVPN_DCO);
    assert_eq!(
        snapshot.tunnel_interfaces,
        vec!["tap0".to_string(), "tun0".to_string(), "tun1".to_string()],
        "三张链路表的并集不对（顺序 = 查询序：tun → wireguard → ovpn）"
    );

    // ── ③ 业务后果：FakeIP 段上那条宣告真的被摘出来了 ──
    //
    // 断到「它落在 FakeIP 块里」为止，而不是只断 `foreign` 非空：这条门要防的缺陷是
    // 「装着 OpenVPN 的机器拿到一句无冲突」，而那句话是在**判定面**上说的。
    const FAKE_IP_BLOCK: &str = "198.18.0.0/15";
    let on_ovpn: Vec<&RouteEntry> = snapshot
        .foreign
        .iter()
        .filter(|r| r.interface == "tun1")
        .collect();
    assert!(
        on_ovpn.iter().any(|r| r.prefix == "198.18.42.0/24"
            && polaris_config_engine::user_config::cidr::cidr_contains(FAKE_IP_BLOCK, &r.prefix)),
        "tun1 上那条落在 FakeIP 段 {FAKE_IP_BLOCK} 里的宣告没被摘出来：{on_ovpn:?}"
    );
    // 另外两条隧道宣告的业务网段也在（tun0 的 `10.8.0.0/24`、tap0 的 `10.9.0.0/24`）——
    // 只断 tun1 的话，「ovpn 那张表只带进来了一个名字」这种半截实现也能过。
    for (iface, prefix) in [("tun0", "10.8.0.0/24"), ("tap0", "10.9.0.0/24")] {
        assert!(
            snapshot
                .foreign
                .iter()
                .any(|r| r.interface == iface && r.prefix == prefix),
            "{iface} 宣告的 {prefix} 没进 foreign：{:?}",
            snapshot.foreign
        );
    }

    // ── ④ 负向对照（**活输入**）：把 `@@@LINK_OVPN` 那一段换成空，两条 OpenVPN 隧道必须消失 ──
    //
    // 这一半不能省。缺了它，本条在「ovpn 那条腿被整个删掉」之外的任何一种回归上都还是绿的 ——
    // 比如有人改成从 `@@@LINK_DETAIL` 里捞名字，结果一样、来源全错。
    let without_ovpn = assemble_linux_probe(
        &section(&raw, "V4_ROUTE"),
        &section(&raw, "V6_ROUTE"),
        &section(&raw, "LINK_TUN"),
        &section(&raw, "LINK_WIREGUARD"),
        "",
        &[],
    );
    assert_eq!(
        without_ovpn.tunnel_interfaces,
        vec!["tap0".to_string()],
        "去掉 ovpn 那一段之后名单里还有别的 —— 说明 tun0/tun1 是从别处捞来的，不是这条腿带进来的"
    );
    assert!(
        !without_ovpn
            .foreign
            .iter()
            .any(|r| r.prefix == "198.18.42.0/24"),
        "去掉 ovpn 那一段之后 FakeIP 那条宣告还在 —— 判据没咬在链路表上：{:?}",
        without_ovpn.foreign
    );
}

/// 🔴 **Linux 的隧道 link type 名单 = 有真机样本的那三种，其余如实登记成缺口**。
///
/// `gre` / `sit` / `ipip` / `vti` / `xfrm` / `ip6tnl` 同样是隧道 link type，判据里**没有**它们：
/// 仓里一份实测样本都没有。不盲收的理由与 Windows 侧 `WINDOWS_TUNNEL_IF_TYPES` 那条同 ——
/// 判据面里混着没人验过的项，下一个人就分不清哪些结论有收据。
///
/// 本条是那条登记的绊线：哪天有带这些类型的 Linux 抓取入库（`@@@LINK_GRE` 一类的分节，
/// 或 `@@@LINK_DETAIL` 里出现这些类型名），它会红并要求按实测重新评估判据面。
///
/// 取材面是**全部** Linux 抓取（[`linux_fixture_names`]），不是写死某一份。
#[test]
fn linux_tunnel_link_types_are_exactly_the_three_with_samples() {
    /// 判据里认的三种 link type，各有真机正样本。
    const SAMPLED_LINK_TYPES: &[&str] = &["tun", "wireguard", "ovpn"];
    /// 仍无样本的隧道 link type：出现即红。
    ///
    /// `ip -d link show` 会在接口详情行的**行首**打出 link type（`ovpn addrgenmode …`、
    /// `tun type tap …`），故按「详情行以某个类型名开头」来找，而不是全文 `contains` ——
    /// 后者会被 `gre` 撞上 `aggregate`、`sit` 撞上 `transit` 这类子串命中骗成假红。
    const UNSAMPLED_LINK_TYPES: &[&str] = &[
        "gre", "gretap", "sit", "ipip", "vti", "vti6", "xfrm", "ip6tnl",
    ];

    for file in linux_fixture_names() {
        let raw = read_fixture(&file);
        let sections = split_sections(&raw);

        // 判据分节名单：三种各一个 `@@@LINK_<T>`，多一个都得有人解释。
        let link_ids: Vec<&str> = sections
            .iter()
            .map(|(id, _)| id.as_str())
            .filter(|id| id.starts_with("LINK_") && *id != "LINK_DETAIL")
            .collect();
        assert_eq!(
            link_ids,
            SAMPLED_LINK_TYPES
                .iter()
                .map(|t| format!("LINK_{}", t.to_uppercase()))
                .collect::<Vec<_>>(),
            "{file} 的链路查询分节不是有样本的那三种 —— 多出来的那个要么登记成新判据、\
             要么说明为什么只抓不认"
        );

        // 前提：详情段真有内容，否则下面那条否定断言恒真。
        let detail = section(&raw, "LINK_DETAIL");
        assert!(
            detail.contains("link/"),
            "{file} 的 @@@LINK_DETAIL 里一行 `link/…` 都没有 —— 下一条否定断言没有信息量"
        );
        // 正向对照：这套「行首类型名」的找法在**已知存在**的类型上确实命中
        // （`ovpn addrgenmode …` 与 `tun type tap …` 都在这份抓取里）。
        let starts_with_type = |ty: &str| {
            detail
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{ty} ")))
        };
        assert!(
            starts_with_type("ovpn") && starts_with_type("tun"),
            "{file}：`ovpn` / `tun` 这两个已知存在的 link type 没被这套找法命中 —— \
             下面那条否定断言的找法是坏的"
        );

        let hit: Vec<&str> = UNSAMPLED_LINK_TYPES
            .iter()
            .copied()
            .filter(|ty| starts_with_type(ty))
            .collect();
        assert!(
            hit.is_empty(),
            "{file} 里出现了仓里没有样本的隧道 link type（命中 {hit:?}）—— \
             「这几种无实测样本」这条登记过期了。该做的是：\
             ① 给 probe_linux 加对应的 `ip -o link show type <T>` 查询并补一段抓取；\
             ② 把它从 UNSAMPLED_LINK_TYPES 划到 SAMPLED_LINK_TYPES；\
             ③ 同步 route_probe 模块头注里那条「只收 ovpn 这一种」的取舍说明。"
        );
    }
}

// ══════════ 行尾：真正该钉住的不变量是「解析器不因行尾而分叉」 ══════════

/// 🔴 **Windows 侧四支解析器对 CRLF 与 LF 产出逐条相同的结果**。
///
/// # 为什么写这条：一道「声称在保护某性质、而那性质早已不在」的配置
///
/// 夹具目录的 `.gitattributes` 写着 `*.txt -text`，注释说这是为了让 Windows 抓取的 CRLF
/// **活着进仓**（仓根规则 `* text=auto eol=lf` 会把它规范成 LF）。可是**三份 2026-09-12 的
/// Windows 夹具在磁盘上一个 `\r` 都没有** —— 上一轮入库时就丢了。也就是说那条配置在它唯一
/// 要做的那件事上已经失效了一整轮，而没有任何地方红过：**门在，但没牙**。
///
/// 把夹具的行尾修回去只是提高保真度，治不了这个形状 —— 下一次谁再用一个规范化行尾的工具
/// 过一遍夹具，同样的事会再发生一次，同样没人喊。真正该钉住的不变量不是「夹具里有 `\r`」，
/// 而是**「解析器不因行尾而分叉」**：那条钉住了，夹具的行尾丢没丢就降级成保真度问题
/// （仍然该修，但不再是正确性问题）。
///
/// # 两个方向都跑
///
/// 仓里两类夹具都有（2026-09-13 那两份带 CRLF，2026-09-12 那三份是纯 LF），故不区分来源：
/// 每一份都在内存里规整出 LF 与 CRLF 两个版本，两份都喂解析器，逐条比。
/// 另带一条**磁盘普查**：至少得有一份夹具真的带着 CRLF，否则「CRLF 是真机形态」这件事
/// 在仓里没有任何实物依据，上面那组比对比的就是两个合成串。
///
/// # 如实登记：它现在是**回归绊线**，不是活的鉴别器
///
/// 变异实测（2026-09-13）：当前实现下这条门**单条**变异打不红 —— `str::lines()` 本身就吃掉
/// CRLF，后面又全走 `split_whitespace()` / `trim()`，任一处单独改都还是不分叉。
/// 要两条**同时**打上才红：
///
///  1. `parse_format_table` 的 `stdout.lines()` 换成按换行符 `split`（回车留在行尾）；
///  2. `slice_by_columns` 的 `.trim()` 换成只 trim 空格（回车留在单元格里）。
///
/// 两条一起打上时本条逐字红成：`windows-…-hyperv-present-… CRLF 版对照表：
/// `Get-NetIPAddress` 的列头里没有 `InterfaceIndex` 列` —— 一个看不见的回车把最后一列的
/// 名字毁掉了，而那张表是另外两支解析器的输入。
///
/// 也就是说：这条不变量目前由 `str::lines()` 这**一个收口**结构性地保住，本门守的是
/// 「别把那个收口换掉」。写下这一条是因为「单条变异打不红」与「这道门没用」长得像，
/// 而两者该做的事完全不同。
#[test]
fn windows_parsers_do_not_fork_on_line_endings() {
    /// 磁盘上**必须**带着 CRLF 的夹具：真机行尾的实物依据。
    ///
    /// 只断「这几份必须有」，**不**断「其余几份必须没有」—— 后者会在有人把 2026-09-12 那三份
    /// 的行尾修回去（一件好事）那天变成假红。
    const MUST_CARRY_CRLF: &[&str] = &[
        WINDOWS_FIXTURE_TAP,
        WINDOWS_FIXTURE_HYPERV,
        WINDOWS_FIXTURE_RAS,
    ];

    for file in MUST_CARRY_CRLF.iter().copied() {
        let raw = read_fixture(file);
        assert!(
            raw.contains("\r\n"),
            "{file} 在磁盘上没有 CRLF —— `.gitattributes` 的 `*.txt -text` 又一次没顶住，\
             或者这份夹具被某个规范化行尾的工具过了一遍"
        );
    }

    for file in windows_fixture_names() {
        let raw = read_fixture(&file);
        // 先统一到 LF，再由它派生 CRLF：直接对原文做 `replace("\n","\r\n")` 会把已有的
        // CRLF 打成 `\r\r\n`（对已规范化的那三份没事，对带 CRLF 的两份是坏输入）。
        let lf = raw.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert!(
            !crlf.contains("\r\r"),
            "{file}：派生出来的 CRLF 版本里出现了 `\\r\\r` —— 规整步骤写错了"
        );

        let sec = |body: &str, id: &str| section(body, id);
        // 四支判据解析器逐一对差。对照表那支先做，另外两支要用它的产出。
        let names_lf = parse_get_netipaddress(&sec(&lf, "GET_NETIPADDRESS"))
            .unwrap_or_else(|e| panic!("{file} LF 版对照表：{e}"));
        let names_crlf = parse_get_netipaddress(&sec(&crlf, "GET_NETIPADDRESS"))
            .unwrap_or_else(|e| panic!("{file} CRLF 版对照表：{e}"));

        for id in ["ROUTE_PRINT_4", "ROUTE_PRINT_6"] {
            let a = parse_route_print_routes(&sec(&lf, id), &names_lf)
                .unwrap_or_else(|e| panic!("{file} 的 @@@{id} LF 版：{e}"));
            let b = parse_route_print_routes(&sec(&crlf, id), &names_crlf)
                .unwrap_or_else(|e| panic!("{file} 的 @@@{id} CRLF 版：{e}"));
            // 正向对照：这一段真的解析出了东西，否则「两版相同」可能只是「两版都空」。
            assert!(
                !a.is_empty(),
                "{file} 的 @@@{id} 解析出 0 条 —— 下面那条相等断言没有信息量"
            );
            assert_eq!(a, b, "{file} 的 @@@{id}：解析结果随行尾分叉了");
        }

        let t_lf = parse_windows_tunnel_interfaces(&sec(&lf, "GET_NETADAPTER"))
            .unwrap_or_else(|e| panic!("{file} 的 @@@GET_NETADAPTER LF 版：{e}"));
        let t_crlf = parse_windows_tunnel_interfaces(&sec(&crlf, "GET_NETADAPTER"))
            .unwrap_or_else(|e| panic!("{file} 的 @@@GET_NETADAPTER CRLF 版：{e}"));
        assert!(
            !t_lf.is_empty(),
            "{file} 的隧道名单是空的 —— 下面那条相等断言没有信息量"
        );
        assert_eq!(t_lf, t_crlf, "{file}：隧道名单随行尾分叉了");

        // 对照表本身也比一次（它是另外两支的输入，先塌的话上面几条会一起塌得看不出原因）。
        assert!(!names_lf.is_empty(), "{file}：对照表解析出 0 条");
        assert_eq!(names_lf, names_crlf, "{file}：对照表随行尾分叉了");
    }
}

// ══════════ macOS 第二读数：`route -n get`（PF_ROUTE / RTM_GET）交叉对差 ══════════

/// `@@@V4_ROUTE_GET` / `@@@V6_ROUTE_GET` 里的一条记录。
///
/// 采集脚本对 `netstat -rn` 的 `Destination` 列逐个跑 `route -n get`，每条前面写一行
/// `### dest=<原样> padded=<补零后>`，后跟命令的完整输出。
#[derive(Debug)]
struct RouteGetRecord {
    /// `### dest=` —— **原样**的 netstat 目的地 token（含 classful 缩写与 `%作用域`）。
    /// 它是与第一读数对齐的天然连接键：两边问的是同一个字符串。
    dest: String,
    /// `### padded=` —— 真正喂给 `route -n get` 的那一份。
    ///
    /// 🔴 **v4 必须先补零到四段**：macOS 的 `route -n get` 对 classful 缩写**误解析** ——
    /// `route -n get 192.168.10` 问到的是 `192.168.0.10`，`169.254` 问到的是 `169.0.0.254`
    /// （它把缩写当成「省掉的是中间几段」，而 `netstat` 打印的语义是「省掉的是尾部零字节」）。
    /// 不补零的话第二读数会静悄悄地问了另一批地址，然后与第一读数对不上 —— 而红出来的会是
    /// 「解析器错了」，方向全反。
    padded: String,
    /// `route -n get` 回显的 `destination:` —— **内核自己**渲染的目的地。
    destination: Option<String>,
    /// `route -n get` 回显的 `mask:` —— **内核自己**算出来的掩码，v4 / v6 两族都逐字入库。
    ///
    /// 主机路由不打印这一行（那是「没有掩码」，不是「掩码为空」），见 [`route_get_prefix`]。
    mask: Option<String>,
    /// `interface:` —— 这条门真正要对差的那一列。
    ///
    /// `None` 有真形态：`route -n get 255.255.255.255/32` 回的是 `route: bad address`，
    /// 一行 `interface:` 都没有。那类跳过，**不是**不一致。
    interface: Option<String>,
}

/// 切一段 `@@@V*_ROUTE_GET`。
fn parse_route_get_section(body: &str) -> Vec<RouteGetRecord> {
    let mut out: Vec<RouteGetRecord> = Vec::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("### dest=") {
            let mut parts = rest.split(" padded=");
            let dest = parts.next().unwrap_or_default().trim().to_string();
            let padded = parts.next().unwrap_or_default().trim().to_string();
            out.push(RouteGetRecord {
                dest,
                padded,
                destination: None,
                mask: None,
                interface: None,
            });
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        let t = line.trim();
        // 逐字段各取一次：`route -n get` 的输出是 `字段: 值` 的定宽块，`destination:` 与
        // `route to:` 长得像但不是一回事（后者是**请求**的地址，前者是内核匹配到的路由）。
        for (key, slot) in [
            ("destination:", &mut cur.destination),
            ("mask:", &mut cur.mask),
            ("interface:", &mut cur.interface),
        ] {
            if let Some(v) = t.strip_prefix(key) {
                *slot = Some(v.trim().to_string());
            }
        }
    }
    out
}

/// 连续掩码 → 前缀长度；掩码不连续时 `None`。
///
/// 判据是**位级**的：`leading_ones + trailing_zeros == 位宽`。不查这条的话，
/// 一个坏掩码（`255.0.255.0`）会被 `leading_ones()` 给出一个看着正常的长度 `8`，
/// 然后悄悄地与另一条前缀对齐上 —— 那比报错危险。
fn contiguous_prefix_len(bits: u128, width: u32) -> Option<u8> {
    let shifted = bits << (128 - width);
    if shifted.leading_ones() + shifted.trailing_zeros() != 128 {
        return None;
    }
    u8::try_from(shifted.leading_ones()).ok()
}

/// 第二读数的目的前缀 —— **不经过生产的 `expand_netstat_destination`**。
///
/// 复用生产的规范化函数等于两边共用同一个错，对差就没有检出力了（同 Windows 侧
/// `Get-NetRoute` 那条交叉验证的口径：参照解析器在测试里另写一份）。
///
/// **v4 / v6 同一套读法**：地址与长度都取自内核回显（`destination:` + `mask:`），
/// 与生产解析器一行代码都不共用。主机路由不打印 `mask:` ⇒ `/32` / `/128`。
///
/// 于是两件事在这条门上被**独立**核了一遍：
///
///  - v4 的 classful 展开（`127` → `127.0.0.0/8`、`224.0.0/4` → `224.0.0.0/4` 这条
///    「长度明写、地址仍缩写」）；
///  - v6 的前缀长度 —— `fd7a:115c:a1e0::/48` 那条 `/48` 现在是从内核回显的
///    `mask: ffff:ffff:ffff::` 数出来的，不是抄 `netstat` 打印的 `/48`。
///
/// > 2026-09-13 早些时候这里曾登记「v6 的 `mask:` 被脱敏改坏了，长度只能抄 netstat」。
/// > 那是脱敏脚本的缺陷（把掩码当成普通十六进制组做了同长度替换），**已修**：
/// > `polaris-redact-fixture.py` 现在按位级判据认出合法 v6 掩码并逐字保留。夹具重生成后
/// > v6 这一半的长度也成了独立读数，故那条登记删掉 —— 它不再成立。
fn route_get_prefix(rec: &RouteGetRecord) -> Option<String> {
    let dest = rec.destination.as_deref()?;
    if dest.contains(':') {
        let addr: Ipv6Addr = dest.split('%').next()?.parse().ok()?;
        let len = match rec.mask.as_deref() {
            None => 128,
            Some(m) => contiguous_prefix_len(u128::from(m.parse::<Ipv6Addr>().ok()?), 128)?,
        };
        return Some(format!("{addr}/{len}"));
    }
    let addr: Ipv4Addr = dest.parse().ok()?;
    let len = match rec.mask.as_deref() {
        None => 32,
        Some(m) => contiguous_prefix_len(u128::from(u32::from(m.parse::<Ipv4Addr>().ok()?)), 32)?,
    };
    Some(format!("{addr}/{len}"))
}

/// 🔴 **macOS 有第二个独立读数了**：`netstat -rn` 的解析结果 vs 同一份抓取里 `route -n get` 的读数。
///
/// # 为什么它是**真的**交叉对差
///
/// `route -n get` 走 PF_ROUTE 套接字的 `RTM_GET`（问内核「发往 X 会走哪条路由」），
/// `netstat -rn` 走的是路由表 **dump**。两条是不同的内核接口，产出也不同形
/// （前者是逐字段的块，后者是列式表）。此前 mac 侧只有一路读数：解析器错了没有任何东西能
/// 把它顶红 —— Windows 侧一直有 `route print` 与 `Get-NetRoute` 两路（见
/// [`route_print_result_matches_the_independent_get_netroute_reading`]），mac 侧这是第一次补上。
///
/// # 判据：**第二读数给出的接口，必须在第一读数为同一前缀给出的接口集合里**
///
/// 是**子集**而不是相等，理由是结构性的：`RTM_GET` 只回**内核会选的那一条**，而 dump 打印
/// **全部**。`ff00::/8` 在这份抓取里挂在 13 个接口上（每个接口一份组播克隆），`route -n get`
/// 只会回其中一个 —— 要求相等就是逼一条结构上不可能的事，红了也只能靠加例外来消，
/// 那样门就退化成一张例外清单。子集这条则两个方向都咬得住：解析器把接口读错（列错位 /
/// 对照表塌了）时，第二读数给的那个名字不在集合里，立刻红。
///
/// 单接口那批（正是业务网段所在的那批）上，子集 + 非空 ⇒ **相等**，一点没放松。
///
/// # 三条容忍规则，每条都有正向对照证明它真的被走到过
///
///  1. **`route -n get` 没有 `interface:` 行**：`255.255.255.255/32` 回的是 `route: bad address`。
///     跳过，但名单钉死 —— 悄悄长出第二条就是解析器开始丢东西了。
///  2. **本机自己的地址回 `lo0`**：`route -n get 192.168.10.142` 回 `lo0`，而同一台机器的
///     netstat 表里 `192.168.10.142/32` 是 `en0`。这**不是**矛盾：BSD 把本机地址的主机路由
///     装在环回上，dump 里那两条（`192.168.10.142` → `lo0` 与 `192.168.10.142/32` → `en0`）
///     **都在**，规范化之后落到同一条前缀上 ⇒ 第一读数的集合是 `{en0, lo0}`，子集规则天然
///     容得下。这里不写特判分支，而是**断言这个形态真的出现过** —— 写成特判的话它会退化成
///     一条没有输入的死代码。
///  3. **`### dest=Destination`**：采集脚本的 `awk` 把 `netstat` 的**列头行**当数据取了一次
///     （v4 那段的 `padded` 甚至是 `Destination.0.0.0`）。这是**采集噪声不是数据**，跳过；
///     同样断言它真的在，免得哪天脚本修好了、这条跳过规则却没人发现已经过期。
///     `dest=default` 同跳：`route -n get default` 回的是网关那一跳、不是一条 `0.0.0.0/0` 宣告，
///     两侧形态对不上。生产腿**照常产出**默认路由条目（走 `ForeignTunnelSnapshot::default_routes`），
///     它的对差由 `default_route_gate` 在 Windows 那份抓取上另做，不靠这条 mac 第二读数。
#[test]
fn macos_netstat_reading_agrees_with_the_independent_route_get_reading() {
    use std::collections::{BTreeMap, BTreeSet};

    let raw = read_fixture(MACOS_FIXTURE_ROUTE_GET);

    // ── 第一读数：生产的 mac 腿 ──
    let v4 = section_any(&raw, MACOS_NETSTAT_V4);
    let v6 = section_any(&raw, MACOS_NETSTAT_V6);
    let ifconfig = section_any(&raw, MACOS_IFCONFIG);
    let snapshot =
        assemble_macos_probe(&v4, &v6, &ifconfig, &[]).expect("第一读数：mac 腿应解析得动");
    let mut first_rows = parse_netstat_routes(&v4, IpFamily::V4).expect("v4");
    first_rows.extend(parse_netstat_routes(&v6, IpFamily::V6).expect("v6"));
    let mut first: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for r in &first_rows {
        first
            .entry(r.prefix.clone())
            .or_default()
            .insert(r.interface.clone());
    }

    // ── 第二读数：route -n get ──
    let mut records = parse_route_get_section(&section(&raw, "V4_ROUTE_GET"));
    records.extend(parse_route_get_section(&section(&raw, "V6_ROUTE_GET")));
    assert!(
        records.len() > 100,
        "第二读数只切出 {} 条 —— 分节切分或 `### dest=` 那条前缀写错了",
        records.len()
    );

    /// 采集噪声 / 非具体网段：跳过，理由见本测试头注第 3 条。
    const SKIPPED_DESTS: &[&str] = &["default", "Destination"];

    let mut second: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut no_interface: Vec<&str> = Vec::new();
    let mut skipped_named: BTreeSet<&str> = BTreeSet::new();
    let mut padded_differs: Vec<(&str, &str, String)> = Vec::new();
    // 长度**由内核回显的 `mask:` 数出来**的那批记录：`(dest, mask, 推出的前缀)`。
    //
    // 分 v4 / v6 两张单子，因为这条门的两半此前不是同一强度：v4 一直靠 mask，
    // v6 一度只能抄 `netstat` 打印的 `/N`（脱敏脚本把掩码改坏了）。脚本修好、夹具重生成之后
    // 两半同强度，而「同强度」这件事必须有条数在这里钉着 —— 否则哪天 v6 那半悄悄退回抄 `/N`，
    // 门照样全绿。
    let mut v4_len_from_mask: Vec<(&str, &str, String)> = Vec::new();
    let mut v6_len_from_mask: Vec<(&str, &str, String)> = Vec::new();
    for rec in &records {
        if let Some(name) = SKIPPED_DESTS.iter().find(|d| **d == rec.dest) {
            skipped_named.insert(name);
            continue;
        }
        let Some(iface) = rec.interface.as_deref() else {
            no_interface.push(&rec.dest);
            continue;
        };
        let prefix = route_get_prefix(rec)
            .unwrap_or_else(|| panic!("第二读数认不出这条记录的目的地：{rec:?}"));
        // 🔴 补零那一步真的做对了：补出来的四段地址，必须与内核回显里那个地址逐字一致。
        // 这是 macOS `route -n get` 对 classful 缩写误解析那条真机事实的**收据** ——
        // 没补零的话 `192.168.10` 会被问成 `192.168.0.10`，`padded` 与内核回显对不上。
        if rec.padded != rec.dest {
            let addr = prefix.split('/').next().unwrap_or_default().to_string();
            assert_eq!(
                rec.padded, addr,
                "补零后的目的地与内核回显的 `destination:` 对不上：{rec:?}"
            );
            padded_differs.push((&rec.dest, &rec.padded, prefix.clone()));
        }
        // 记下长度的**来源**：有 `mask:` 行 ⇒ 长度是内核数出来的；没有 ⇒ 走主机路由的兜底
        // （`/32` / `/128`），那条兜底不是独立读数，不该算进下面的正向对照条数里。
        if let Some(mask) = rec.mask.as_deref() {
            let bucket = if rec.destination.as_deref().is_some_and(|d| d.contains(':')) {
                &mut v6_len_from_mask
            } else {
                &mut v4_len_from_mask
            };
            bucket.push((&rec.dest, mask, prefix.clone()));
        }
        second.entry(prefix).or_default().insert(iface.to_string());
    }

    // ── 容忍规则的正向对照：三条都真的被走到过，且名单钉死 ──
    assert_eq!(
        no_interface,
        ["255.255.255.255/32"],
        "「没有 interface: 行」的记录名单变了 —— 多出来的那条要么是新形态、\
         要么是第二读数开始丢东西了"
    );
    assert_eq!(
        skipped_named.into_iter().collect::<Vec<_>>(),
        ["Destination", "default"],
        "跳过名单没被走全 —— `Destination` 那条是采集脚本 awk 把列头当数据取的噪声，\
         它要是不在了，说明脚本修好了而这条跳过规则已经过期"
    );
    // classful 缩写那批确实进了对比（`127` / `169.254` / `192.168.10` 三条各是一种长度）。
    let padded_dests: BTreeSet<&str> = padded_differs.iter().map(|(d, _, _)| *d).collect();
    for want in ["127", "169.254", "192.168.10"] {
        assert!(
            padded_dests.contains(want),
            "classful 缩写 `{want}` 没进第二读数 —— 补零那条收据没有输入：{padded_dests:?}"
        );
    }

    // ── 🔴 正向对照：两个地址族的前缀长度**都**是从内核回显的 `mask:` 数出来的 ──
    //
    // 条数钉死，换样本时红。只断「有」而不断条数的话，v6 那半退化成只剩一两条时也不会有人发现。
    assert_eq!(
        v4_len_from_mask.len(),
        33,
        "v4 由 `mask:` 定出长度的记录条数变了：{v4_len_from_mask:?}"
    );
    assert_eq!(
        v6_len_from_mask.len(),
        42,
        "v6 由 `mask:` 定出长度的记录条数变了 —— 这一半此前不是独立读数（脱敏脚本把 v6 掩码\
         改坏了），条数掉下去就说明它又退回抄 `netstat` 打印的 `/N` 了：{v6_len_from_mask:?}"
    );
    // 逐条点名两条**非平凡**长度：抄 `/N` 与从 mask 数出来在这两条上会给出同一个答案，
    // 所以它们证不了独立性 —— 它们证的是「mask 这条读法在真形态上跑得通」。
    // 真正证独立性的是下面那条变异对照（把 mask 改一位，门必须红）。
    for (dest, mask, prefix) in [
        (
            "fd7a:115c:a1e0::/48",
            "ffff:ffff:ffff::",
            "fd7a:115c:a1e0::/48",
        ),
        ("ff00::/8", "ff00::", "ff00::/8"),
    ] {
        assert!(
            v6_len_from_mask
                .iter()
                .any(|(d, m, p)| *d == dest && *m == mask && p == prefix),
            "`{dest}` 的长度没从 `{mask}` 数出来：{v6_len_from_mask:?}"
        );
    }

    // 🔴 **变异对照（活输入）**：把某条 v6 记录的 `mask:` 改一位，推出来的前缀必须跟着变。
    // 这一条是「v6 长度真的取自 mask」的唯一硬证据 —— 只断条数的话，一个仍在抄 `/N` 的实现
    // 同样能让上面那些全绿。
    let mut mutated = RouteGetRecord {
        dest: "fd7a:115c:a1e0::/48".to_string(),
        padded: "fd7a:115c:a1e0::/48".to_string(),
        destination: Some("fd7a:115c:a1e0::".to_string()),
        mask: Some("ffff:ffff:ffff::".to_string()),
        interface: Some("utun11".to_string()),
    };
    assert_eq!(
        route_get_prefix(&mutated).as_deref(),
        Some("fd7a:115c:a1e0::/48"),
        "前提：未变异时它就是 /48"
    );
    mutated.mask = Some("ffff:ffff::".to_string());
    assert_eq!(
        route_get_prefix(&mutated).as_deref(),
        Some("fd7a:115c:a1e0::/32"),
        "改了 `mask:` 而前缀长度没跟着变 —— v6 那半还在抄 `netstat` 打印的 `/N`"
    );
    // 顺带钉住连续性判据：不连续的掩码必须 `None`，不许被 `leading_ones()` 读成一个正常长度。
    mutated.mask = Some("ffff::ffff".to_string());
    assert_eq!(
        route_get_prefix(&mutated),
        None,
        "不连续的掩码被当成合法长度了"
    );

    // ── 判据主体：子集 ──
    let violations: Vec<(String, Vec<String>, Vec<String>)> = second
        .iter()
        .filter(|(prefix, b)| !b.is_subset(first.get(prefix.as_str()).unwrap_or(&BTreeSet::new())))
        .map(|(p, b)| {
            (
                p.clone(),
                b.iter().cloned().collect(),
                first
                    .get(p)
                    .map(|a| a.iter().cloned().collect())
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert!(
        violations.is_empty(),
        "两路读数对不上（第二读数说的接口不在第一读数为同一前缀给出的集合里）：{violations:#?}"
    );

    // ── 🔴 正向对照①：对上的条数够多，且**业务网段那批逐条相等** ──
    //
    // 一道「什么都没比到也绿」的交叉对差门是假绿。这里钉住：第二读数里独指 `utun11`
    // （= tailnet 那批）的前缀有 30 条，且每一条在第一读数里也**恰好**只有 utun11。
    let utun11_only: BTreeSet<&str> = second
        .iter()
        .filter(|(_, b)| b.len() == 1 && b.contains("utun11"))
        .map(|(p, _)| p.as_str())
        .collect();
    assert_eq!(
        utun11_only.len(),
        30,
        "第二读数独指 utun11 的前缀条数变了：{utun11_only:?}"
    );
    for prefix in &utun11_only {
        assert_eq!(
            first.get(*prefix).map(|a| a.iter().cloned().collect()),
            Some(vec!["utun11".to_string()]),
            "第一读数对 {prefix} 的接口不是「只有 utun11」"
        );
    }
    // 逐条点名那批照记忆写不出来的形态（Tailscale 装的是**逐 peer 的 `/32`**，不是汇总段）。
    assert_eq!(
        utun11_only
            .iter()
            .filter(|p| p.starts_with("32.0.0.") && p.ends_with("/32"))
            .count(),
        27,
        "逐 peer `/32` 的条数变了：{utun11_only:?}"
    );
    for want in [
        "100.100.100.100/32",
        "fd7a:115c:a1e0::/48",
        "fd7a:115c:a1e0::e9/128",
    ] {
        assert!(
            utun11_only.contains(want),
            "{want} 没进交叉对差：{utun11_only:?}"
        );
    }
    // 生产腿的 `foreign` 与之对得上：上面比的是「解析器 vs 内核」，这条比的是
    // 「解析器 vs 它自己的下游」—— 名单对了但 foreign 没落上去的话这里红。
    for prefix in &utun11_only {
        assert!(
            snapshot
                .foreign
                .iter()
                .any(|r| r.prefix == *prefix && r.interface == "utun11"),
            "{prefix} 在两路读数上都属于 utun11，却没进生产腿的 foreign"
        );
    }

    // ── 🔴 正向对照②：三条容忍规则里的 lo0 那条，形态真的出现了 ──
    assert_eq!(
        first
            .get("192.168.10.142/32")
            .map(|a| a.iter().cloned().collect::<Vec<_>>()),
        Some(vec!["en0".to_string(), "lo0".to_string()]),
        "「本机地址的主机路由同时出现在 lo0 与物理口上」这个形态不在了 —— \
         容忍规则第 2 条失去输入，说明里那段话要重核"
    );
    assert_eq!(
        second
            .get("192.168.10.142/32")
            .map(|a| a.iter().cloned().collect::<Vec<_>>()),
        Some(vec!["en0".to_string(), "lo0".to_string()]),
        "第二读数对本机地址的回答变了"
    );

    // ── 🔴 正向对照③：「子集而非相等」那条真的被用到了，且只被这两条用到 ──
    let strict_subset: Vec<&str> = second
        .iter()
        .filter(|(p, b)| first.get(p.as_str()) != Some(*b))
        .map(|(p, _)| p.as_str())
        .collect();
    assert_eq!(
        strict_subset,
        ["224.0.0.0/4", "ff00::/8"],
        "「第二读数只回一条、dump 打印多条」的前缀名单变了 —— 子集这条放宽的射程要重核"
    );

    // ── 🔴 负向对照（**活输入**）：把第二读数里的 utun11 改成 en0，本门必须红 ──
    //
    // 缺了这半，上面那一大片相等只证得了「两边现在一样」，证不了「不一样时会被抓到」。
    let mutated_body = section(&raw, "V4_ROUTE_GET").replace("interface: utun11", "interface: en0");
    let applied = section(&raw, "V4_ROUTE_GET")
        .matches("interface: utun11")
        .count();
    assert!(
        applied >= 20,
        "变异没打上（只替换了 {applied} 处）—— 一次没生效的变异红绿都不算数"
    );
    let mutated: Vec<RouteGetRecord> = parse_route_get_section(&mutated_body);
    let caught = mutated.iter().any(|rec| {
        rec.interface.as_deref() == Some("en0")
            && route_get_prefix(rec).is_some_and(|p| {
                first
                    .get(&p)
                    .is_some_and(|a| !a.contains("en0") && a.contains("utun11"))
            })
    });
    assert!(
        caught,
        "把第二读数的 utun11 改成 en0 之后子集判据仍然没红 —— 这道门咬不住接口那一列"
    );
}
