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
    expand_netstat_destination, parse_get_netipaddress, parse_macos_interface_flags,
    parse_macos_tunnel_interfaces, parse_netstat_routes, parse_route_print_routes,
    parse_windows_tunnel_interfaces, RouteEntry, RouteTableParseError, WindowsInterfaceNames,
    MAC_TUNNEL_FLAG,
};
use polaris_helper_proto::codec::is_valid_cidr;
use polaris_helper_proto::Platform;
use std::net::IpAddr;
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
];

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
        ],
    )
}

fn macos_fixture_names() -> Vec<String> {
    fixture_names_starting_with(
        "macos-",
        &[MACOS_FIXTURE, MACOS_FIXTURE_FULL, MACOS_FIXTURE_TS_ON],
    )
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
    split_sections(raw)
        .into_iter()
        .find(|(sid, _)| sid == id)
        .unwrap_or_else(|| panic!("样本里没有 @@@{id} 分节"))
        .1
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
fn classify(section_id: &str) -> Option<SectionKind> {
    match section_id {
        "V4" | "V6" => Some(SectionKind::MacNetstatRoutes),
        "IFCONFIG" => Some(SectionKind::MacTunnelInterfaces),
        "ROUTE_PRINT_4" | "ROUTE_PRINT_6" => Some(SectionKind::WinRoutePrint),
        "GET_NETIPADDRESS" => Some(SectionKind::WinInterfaceNames),
        "GET_NETADAPTER" | "NETSH_INTERFACE" => Some(SectionKind::WinTunnelInterfaces),
        "META" | "ENV" | "NETSTAT_I" | "NETWORKSETUP_ORDER" | "TAILSCALE" | "IPCONFIG"
        | "GET_NETROUTE_4" | "GET_NETROUTE_6" | "NETSH_INTERFACE_6" => {
            Some(SectionKind::ReferenceOnly)
        }
        _ => None,
    }
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
                    parse_netstat_routes(body),
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

    // 逐份点名：两份 Windows 抓取各一条，一条不多一条不少（注记形态是 `<文件> 的 @@@<分节>：…`，
    // 文件名不含空格）。
    let mut gap_files: Vec<&str> = gaps
        .iter()
        .map(|g| g.split(' ').next().expect("注记以文件名开头"))
        .collect();
    gap_files.sort();
    let mut want: Vec<&str> = vec![
        WINDOWS_FIXTURE,
        WINDOWS_FIXTURE_WINTUN,
        WINDOWS_FIXTURE_TS_ON,
    ];
    want.sort();
    assert_eq!(
        gap_files,
        want,
        "缺口不是「每份 Windows 抓取的 netsh 各一条」。现在的缺口：{gaps:#?}\n完整报告：\n{}",
        scan.notes.join("\n")
    );

    // 正向对照：四个判据分节在**每一份** Windows 抓取里都真的解析出了东西 ——
    // 否则「只剩 netsh 一条缺口」可能只是因为某份抓取整条腿都没接上。
    for file in [
        WINDOWS_FIXTURE,
        WINDOWS_FIXTURE_WINTUN,
        WINDOWS_FIXTURE_TS_ON,
    ] {
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

/// 🔴 **wintun 的 `InterfaceType` 实测是 `53`；仍未验证的是另外两族驱动 + `53` 的假阳性面**。
///
/// # 这条绊线换过向
///
/// 上一版钉的是「仓里**没有**任何 VPN 虚拟网卡的抓取 ⇒ wintun 报什么 ifType 无证据」。
/// 2026-09-12 那份抓取把它验掉了：wintun 报 **`53`**（`IF_TYPE_PROP_VIRTUAL`），**不是 `131`**
/// —— 旧判据（只认 `131`）在它唯一要做的那件事上静默失败。于是这条绊线改成钉住**新的边界**：
///
///  1. **`53` 那个值本身**：逐字钉在抓取的那一行上（`ComponentID` = `Wintun`、`Status` = `Up`），
///     并与生产常量对齐 —— 判据表与实测值改一个忘一个时这里先红；
///  2. **`53` 的假阳性面**：`IF_TYPE_PROP_VIRTUAL` 是「厂商自有虚拟接口」这个大桶。凡有抓取里
///     出现**不是已登记正样本**的 `53` 行（Hyper-V / VMware / Docker 的虚拟适配器等），本条红 ——
///     那时要么把它登记成新的隧道驱动正样本，要么重新评估 `{131, 53}` 这条放宽；
///  3. **另外两族隧道驱动仍无样本**：TAP-Windows / OpenVPN（`tap0901`，以太网仿真，疑报 `6`）与
///     WireGuard NT / 各家企业 VPN 客户端。它们的 ifType **仓里没有任何证据**；若真报 `6`，
///     判据漏它们 —— 而 `6` 是全部物理网卡，不能盲收。哪天有这种抓取入库，本条红并要求读出实测值。
///
/// 三条的取材面都是**全部** Windows 抓取（[`windows_fixture_names`]），不是写死某一份：
/// 上一版写死单文件名，第二份抓取带着 wintun 进来那天它压根没看。
#[test]
fn the_wintun_capture_pins_iftype_53_and_two_driver_families_are_still_unverified() {
    // ── ① `53` 逐字钉在那一行上 ──
    let raw = read_fixture(WINDOWS_FIXTURE_WINTUN);
    let table = cut_adapter_table(&section(&raw, "GET_NETADAPTER"));
    let wintun: Vec<&Vec<String>> = table
        .rows
        .iter()
        .filter(|r| table.cell(r, "ComponentID") == "Wintun")
        .collect();
    assert_eq!(
        wintun.len(),
        1,
        "这份抓取里 `ComponentID == Wintun` 的行不是恰好一条：{:?}",
        table.rows
    );
    assert_eq!(
        table.cell(wintun[0], "InterfaceType"),
        "53",
        "wintun 报的 ifType 变了 —— 判据表（route_probe 的 IF_TYPE_PROP_VIRTUAL 头注）该跟着改"
    );
    assert_eq!(table.cell(wintun[0], "InterfaceAlias"), "Tailscale");
    assert_eq!(
        table.cell(wintun[0], "Status"),
        "Up",
        "那张 wintun 适配器不是 Up —— 「已装且驱动真的起来了」这条前提没了，这行的说服力打折"
    );
    // 判据常量与实测值是同一个；改一个忘一个时这里红。
    assert_eq!(crate::route_probe::IF_TYPE_TUNNEL, 131);
    assert_eq!(crate::route_probe::IF_TYPE_PROP_VIRTUAL, 53);
    assert_eq!(crate::route_probe::WINDOWS_TUNNEL_IF_TYPES, &[131_u32, 53]);

    // ── ② `53` 的假阳性面：报 53 的行必须是已登记的隧道驱动 ──
    /// `53`（`IF_TYPE_PROP_VIRTUAL`）这个桶里**已登记**的隧道驱动痕迹（`ComponentID` /
    /// `DriverDescription` / `InterfaceAlias` 任一列里留下的名字）。
    ///
    /// 这是白名单，但它**不是生产判据**（生产只认 ifType 那一列）—— 它是「这个宽桶里出现了
    /// 我们没见过的东西」的报警器。
    const REGISTERED_IF_TYPE_53_DRIVERS: &[&str] = &["Wintun", "Tailscale"];
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
                 ① 若它是新的隧道驱动，把它登记进 route_probe 的 IF_TYPE_PROP_VIRTUAL 正样本清单；\
                 ② 若它是虚拟交换机之类的非隧道适配器，按 IF_TYPE_PROP_VIRTUAL 头注里的代价对称性\
                 重新评估 `{{131, 53}}` 这条放宽（别忘了假阳性的代价只是展示面多一条网段）。\
                 整行：{text}",
                table.cell(row, "InterfaceAlias")
            );
        }
    }
    // 正向对照：确实有报 `53` 的行，否则上面那个循环恒真（一条都没跑）。
    // 两份带 wintun 的抓取（未登录 / 已登录）各一行 —— 判据不随连接态漂。
    assert_eq!(
        fifty_three_rows, 2,
        "报 `53` 的适配器行数不是 2 —— 取材面变了，`53` 的依据与假阳性面都要重核"
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

    // ── ③ 另外两族驱动仍无样本 ──
    /// ifType **未验证**的隧道驱动族在抓取里会留下的名字。
    ///
    /// 命中 ⇒ 仓里第一次有了这族的真机抓取，该读出它实际报的 `InterfaceType` 并据此决定判据面。
    const UNVERIFIED_DRIVER_FAMILIES: &[&str] = &[
        "tap0901",
        "TAP-Windows",
        "OpenVPN",
        "WireGuard",
        "wireguard",
        "Zscaler",
        "GlobalProtect",
    ];
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
             （注意 `6` = 全部物理网卡、`23` 与 PPPoE 拨号同型，这两个值收了会把真业务网卡判成隧道）；\
             ③ 把结论写进 parse_windows_tunnel_interfaces 的判据表，并在这里把它从未验证清单里划掉。"
        );
    }
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
    let routes = parse_netstat_routes(&section(&raw, "V4")).expect("真机 v4 抓取应解析得动");

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
    // `default` 不产出条目（与 `parse_ip_route_line` 同口径）。
    assert!(
        !routes.iter().any(|r| r.prefix.starts_with("0.0.0.0")),
        "default 行不该产出条目：{routes:?}"
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
    let routes = parse_netstat_routes(&section(&raw, "V6")).expect("真机 v6 抓取应解析得动");

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
#[test]
fn macos_tailnet_prefixes_appear_only_in_the_connected_capture() {
    let mut connected_seen = false;
    for file in macos_fixture_names() {
        let raw = read_fixture(&file);
        let mut routes = parse_netstat_routes(&section(&raw, "V4")).expect("v4");
        routes.extend(parse_netstat_routes(&section(&raw, "V6")).expect("v6"));

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

        if file != MACOS_FIXTURE_TS_ON {
            assert!(
                tailnet.is_empty(),
                "{file} 里出现了 tailnet 网段 {tailnet:?} —— 这份登记的是**断开态**。\
                 若它真换成了连接态抓取，请把它移到连接态那一侧并同步 fixtures/README.md 的覆盖表"
            );
            continue;
        }

        connected_seen = true;
        // ── 连接态那份：逐条点名 ──
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
        // 条数钉死（换样本时会红，提醒重核上面的点名）。
        assert_eq!(
            tailnet.len(),
            29,
            "连接态抓取里的 tailnet 业务网段条数变了：{tailnet:?}"
        );

        // 🔴 **本机自己那条 v6 `/128` 挂在 `lo0` 上，不在 utun 上** —— 照记忆写不出来：
        // BSD 把本地地址的主机路由装到环回上（`fd7a:115c:a1e0::d3` 是这台 mac 自己的 tailnet
        // 地址，`ifconfig utun11` 里逐字有它）。**后果是它进不了 `foreign`**：
        // `foreign_tunnel_routes` 按接口名过滤，`lo0` 不在隧道名单里。也就是说 mac 侧看得见
        // 的是「**别人**的网段」，看不见自己那条 —— 对本模块（找**外来**隧道）恰好是对的方向，
        // 但它是真机形态，不是设计意图，写在这里免得下一个人以为漏了一条。
        let (on_tunnel, elsewhere): (Vec<&RouteEntry>, Vec<&RouteEntry>) =
            tailnet.iter().partition(|r| r.interface == "utun11");
        assert_eq!(
            elsewhere
                .iter()
                .map(|r| (r.prefix.as_str(), r.interface.as_str()))
                .collect::<Vec<_>>(),
            [("fd7a:115c:a1e0::d3/128", "lo0")],
            "挂在 utun11 之外的 tailnet 网段不是「本机自己那条 /128 在 lo0 上」这一条：{elsewhere:?}"
        );
        assert_eq!(
            on_tunnel.len(),
            28,
            "挂在 utun11 上的条数变了：{on_tunnel:?}"
        );
    }
    // 缺了这条，上面那个 `if` 整支可能一次都没执行（连接态夹具被改名 / 删掉）。
    assert!(connected_seen, "连接态那份抓取没进循环 —— 正样本那一半没跑");
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
    let routes = parse_netstat_routes(older_macos).expect("多两列的列头也要解析得动");
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
    let good = parse_netstat_routes(&good).expect("好样本必须 Ok");
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
        let got = parse_netstat_routes(&raw);
        assert!(
            got.is_err(),
            "{name}：畸形样本被静默解析成 {got:?} —— 短的/空的 `Ok` 会被下游当成事实"
        );
    }

    // 两种畸形各自落在**不同**的错误支上：空文件 = 连列头都没有；截断 = 列头在、数据行短了。
    // 只断 `is_err()` 的话，一个「凡事都报 MissingHeader」的实现也能全绿。
    assert!(
        matches!(
            parse_netstat_routes(&read_fixture("malformed/macos-empty.txt")),
            Err(RouteTableParseError::MissingHeader { .. })
        ),
        "空文件应报「找不到列头」"
    );
    let truncated = read_fixture("malformed/macos-truncated-row.txt");
    assert!(
        matches!(
            parse_netstat_routes(&truncated),
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

    // 条目数逐字钉死：默认路由（`0.0.0.0/0` 与 `::/0`）不产出条目，永久路由表不产出条目。
    assert_eq!(v4.len(), 10, "v4 条目数与真机抓取对不上：{v4:?}");
    assert_eq!(v6.len(), 30, "v6 条目数与真机抓取对不上：{v6:?}");
    assert!(
        !v4.iter().any(|r| r.prefix.starts_with("0.0.0.0/")),
        "v4 的默认路由产出了条目：{v4:?}"
    );
    assert!(
        !v6.iter().any(|r| r.prefix == "::/0"),
        "v6 的默认路由产出了条目：{v6:?}"
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
/// 就算被误当成活动路由也会在「默认路由不产出条目」那一步被吞掉 —— 于是真样本证不了这件事。
/// 构造一条**非默认**的永久路由来补这个缺口，并在此如实登记它是构造的。
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

    // 🔴 跑遍**三份** Windows 抓取，不是只对着最早那份。取材面写死一份时，后来入库的
    // 那两份（含唯一带 wintun 业务路由的那份）压根没人拿它们对差 —— 而「静默丢行」正是
    // 路由数量多起来之后才容易发生的事（折行、30 条隧道路由）。
    for (file, want_rows) in [
        (WINDOWS_FIXTURE, 40),
        (WINDOWS_FIXTURE_WINTUN, 43),
        (WINDOWS_FIXTURE_TS_ON, 73),
    ] {
        let raw = read_fixture(file);

        // 参照侧：Get-NetRoute 的 (前缀, 接口别名)，跳掉默认路由（与生产口径一致）。
        let mut reference: Vec<(String, String)> = ["GET_NETROUTE_4", "GET_NETROUTE_6"]
            .iter()
            .flat_map(|id| {
                reference_rows(&section(&raw, id), &["DestinationPrefix", "InterfaceAlias"])
            })
            .filter(|row| row[0] != "0.0.0.0/0" && row[0] != "::/0")
            .map(|row| (row[0].clone(), row[1].clone()))
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
