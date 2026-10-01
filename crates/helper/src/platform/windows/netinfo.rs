//! 本机网络接口**只读**信息枚举（Windows `GetAdaptersAddresses` 的单播地址 + `OnLinkPrefixLength`）。
//!
//! ## 职责边界：零策略，只做枚举
//!
//! 本模块只回答「这台机器上有哪些单播地址、各自前缀多长、是不是回环」，**不做任何取舍**
//! （不滤回环、不去重、不拼 CIDR 串）—— 那些是 `polaris-config-engine::user_config::own_lan`
//! 的纯逻辑，已有确定性单测。调用方（`runtime/proxy::enumerate_own_lan_cidrs`）拿本模块的
//! 原始三元组喂那套纯逻辑，与 unix 腿（`getifaddrs` → 同一套纯逻辑）结构逐条对称。
//!
//! ## 为什么住在 helper crate 而不是 src-tauri
//!
//! 三条硬约束的交集，只剩这一个位置：
//!
//! 1. `GetAdaptersAddresses` 是 FFI ⇒ 必须 `unsafe`；而 `src-tauri/src/runtime/proxy.rs` 是
//!    `#![forbid(unsafe_code)]`（`forbid` 不可被内层 `allow` 覆盖），unix 腿之所以能写在那里，
//!    是因为 `nix` 提供了 `getifaddrs` 的 safe wrapper —— Windows 侧依赖树里没有等价物。
//! 2. 本 crate 已经有 `windows-sys` 的 `Win32_NetworkManagement_IpHelper`（target-specific 依赖）
//!    **且同一个 `GetAdaptersAddresses` 已在 [`super::wintun`] 里被调用** ⇒ 复用既有能力，
//!    不给 `src-tauri` 加新依赖（简约阶梯：workspace 里已有等价能力就不再引一份）。
//! 3. `src-tauri` 已依赖 `polaris-helper`，`platform::windows` 在 `cfg(any(windows, test))` 下可见 ⇒
//!    跨 crate 调用零新增接线。
//!
//! **免提权**：`GetAdaptersAddresses` 是普通用户 API（与 helper 的 SYSTEM 身份无关），app 进程直调即可，
//! 无需经命名管道走 helper 协议。放在本 crate 是**依赖复用**，不是「这件事需要特权」。
//!
//! ## 不触碰宿主
//!
//! 纯读（枚举），不改任何接口/路由/DNS。纯逻辑部分（八位组→地址串、前缀合法性、缓冲区容量换算、
//! 重试预算判据）无 cfg，Linux 可测；FFI 腿本身（`cfg(windows)`）只能靠交叉编译 + Windows 真机覆盖。

/// 一条本机单播地址（原始枚举结果）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalUnicastAddr {
    /// 地址串（v4 点分 / v6 冒号分，压缩形）。
    pub ip: String,
    /// on-link 前缀长度（v4 ≤32 / v6 ≤128）。
    pub prefix: u8,
    /// 是否回环接口（`IfType == IF_TYPE_SOFTWARE_LOOPBACK`）。
    pub is_loopback: bool,
}

/// 一张 Windows 网络适配器的只读摘要。`name` 是 sing-box `bind_interface` 实际接受的
/// InterfaceAlias（`FriendlyName`）；`display_name` 优先使用适配器描述，仅供 UI 展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkAdapterInfo {
    pub name: String,
    pub display_name: String,
    pub is_up: bool,
    pub is_loopback: bool,
    pub addresses: Vec<String>,
}

/// `IF_TYPE_SOFTWARE_LOOPBACK`（IANA ifType 24）。本地常量而非从 `windows-sys` 取：
/// 该常量在不同 feature 组合下的模块路径会变，而值是 IANA 注册号、永不变。
pub const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

/// v4 八位组 → 点分串（纯逻辑，跨平台可测）。
#[must_use]
pub fn v4_octets_to_string(o: [u8; 4]) -> String {
    std::net::Ipv4Addr::from(o).to_string()
}

/// v6 十六字节 → 压缩冒号串（纯逻辑，跨平台可测）。
#[must_use]
pub fn v6_octets_to_string(o: [u8; 16]) -> String {
    std::net::Ipv6Addr::from(o).to_string()
}

/// 前缀长度合法性（纯逻辑，跨平台可测）。合法域 **v4 `1..=32` / v6 `1..=128`**。
///
/// **必须校验**：`IP_ADAPTER_UNICAST_ADDRESS_LH.OnLinkPrefixLength` 在部分接口（隧道 / 尚未配置完成的
/// 适配器）上会给出 `255` 之类的哨兵值；不校验就会拼出 `192.168.1.5/255` 这种下游 CIDR 解析器直接
/// 拒收（或更糟：解析成别的东西）的串。越界 → 整条丢弃，对齐 unix 腿「掩码非法即跳过」的 best-effort。
///
/// **为什么 0 与 255 同列哨兵**：同一批接口（隧道 / 未配置完成态）也会报 `OnLinkPrefixLength = 0`。
/// 0 不是「本机 LAN 段」的合法描述而是默认路由：这条一旦混进 own_lan，
/// `builder::tun_route_exclude::compute_win_bypass_exclude` 拿 own_lan 当 carve guard 时，`/0` 与**一切**
/// mesh 段相交 ⇒ 全部 mesh 段进 `mesh_skipped_own_lan`、一条都不 carve ⇒ bypassLAN 下组网段整体绕 TUN
/// 静默失效。此处早丢弃是第一道；汇流点 `own_lan_cidr` 还有第二道（unix 腿共用）。
#[must_use]
pub fn prefix_is_valid(prefix: u8, is_v6: bool) -> bool {
    if is_v6 {
        (1..=128).contains(&prefix)
    } else {
        (1..=32).contains(&prefix)
    }
}

/// 两步法（探大小 → 按 size 填充）**共用**的重试上限（接口在两次调用之间增减 → size 变大 → 重来）。
pub const SIZE_PROBE_MAX_RETRIES: u32 = 3;

/// 还能不能再重试一次（纯逻辑，跨平台可测）。`retries` = 已消耗的重试次数。
///
/// **为什么探大小与填充共用一个预算**：填充调用同样会返回 `ERROR_BUFFER_OVERFLOW`（两次调用之间
/// 适配器增多），此时 API 已把新的 size 回写。填充腿若直接放弃，本次起核的 own_lan 整体缺位 ——
/// 而 own_lan 是 Windows bypassLAN carve 的 guard，缺位 = 物理子网保护失效。两条腿各记一套预算则最坏
/// 翻倍系统调用，故共用同一个 `retries` 计数。
///
/// **测试诚实说明**：FFI 腿（`cfg(windows)` + 真 `GetAdaptersAddresses`）本机测不到，本函数抽出来供
/// Linux 直测的是**重试预算判据**（第几次该放弃），不是 FFI 行为本身。
#[must_use]
pub fn should_retry_after_overflow(retries: u32) -> bool {
    retries < SIZE_PROBE_MAX_RETRIES
}

/// 承载 `GetAdaptersAddresses` 输出所需的 `u64` 槽数（纯逻辑，跨平台可测）。
///
/// **为什么用 `Vec<u64>` 而不是 `Vec<u8>`**：填充结果要按 `&IP_ADAPTER_ADDRESSES_LH` 解引用，该结构体
/// 含指针 / u64 ⇒ align = 8，而 `Vec<u8>` 只保证 align 1 —— 从未对齐的地址造引用按语言规则即 UB
/// （实践上 Windows 堆分配恰好 16 字节对齐所以不炸，但 Miri 判红，且这是「靠分配器实现细节」而非靠语言
/// 保证）。`Vec<u64>` 的 align = 8 == 目标 align，由各 `win_impl` 里的 `const _` 断言编译期钉死。
///
/// 向上取整（`size` 不是 8 的倍数时多分配一个槽），保证容量**不缩水**。
#[must_use]
pub fn u64_cells_for(size: u32) -> usize {
    (size as usize).div_ceil(std::mem::size_of::<u64>())
}

// ── 外来隧道冲突探测（D1）用的只读枚举：路由表 / 适配器类型 / RAS 连接 ────────────────────
//
// 这三组只在 **app 进程**里被调（`src-tauri` 注入给 `polaris_system_integration` 的
// `WindowsNetInfoSource`），与 helper 的 SYSTEM 身份无关 —— 三个 API 都是免提权的用户会话作用域
// 查询。放在本 crate 的理由与上面那两个枚举完全相同：FFI 只能住在已有 `windows-sys` 且允许
// item 级 `allow(unsafe_code)` 的地方，而 `system-integration` 是零 windows-sys 的 crate、
// `src-tauri/runtime/proxy.rs` 是 `forbid(unsafe_code)`。
//
// **为什么要有这一组**：它们替换掉的是 6 个串行外部进程（`route.exe print -4/-6` +
// 4 条 `Get-VpnConnection` / `Get-NetAdapter` PowerShell，各 5s 预算）。起核峰值窗口里
// PowerShell 冷启动跑不完 ⇒ 探测超时 ⇒ 冲突提示整条拿不到。

/// 一条路由表条目（`GetIpForwardTable2` 的一行，已解成与文本解析器同形的中间结构）。
///
/// `interface_alias` 由 `ConvertInterfaceLuidToAlias` 解出 —— 与 `Get-NetIPAddress` 的
/// `InterfaceAlias` 是同一个命名空间（NDIS ifAlias），也是 [`AdapterKind::alias`] 与
/// RAS 连接名所在的那个命名空间。三者不同源就join 不上，这是本结构存在的全部理由。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpForwardEntry {
    /// 规范化目的前缀（`0.0.0.0/0` / `fd7a:115c::/48` 形态，**含**默认路由）。
    pub prefix: String,
    /// 出接口别名。
    ///
    /// **不带接口索引**：下游（`route_probe`）整条链都按别名逐字 join，索引没有消费方，
    /// 留一个没人读的字段只会让「这份结构里哪些是判据」变糊。
    pub interface_alias: String,
}

/// 一张适配器的**类型**信息（`GetAdaptersAddresses` 的 `FriendlyName` + `IfType`）。
///
/// 与 [`NetworkAdapterInfo`] 分开而不是加字段：那个类型是 UI 绑定网卡选择器的展示面（排过序、
/// 滤过空名、带地址列表），这个是隧道判据的取材面（要原始顺序、要 ifType、不要地址）。
/// 合成一个就得让判据面跟着展示面的排序/过滤走。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterKind {
    /// `FriendlyName`（= InterfaceAlias），与路由条目的 `interface_alias` 同命名空间。
    pub alias: String,
    /// IANA ifType（`131` = 协议隧道、`53` = 厂商虚拟接口 …）。判据在调用方。
    pub if_type: u32,
}

/// 一条**活动的** RAS / VPN 连接（`RasEnumConnections` 的一行）。
///
/// 与 `Get-VpnConnection` 的关键差异：那条 cmdlet 列的是**配置**（断开的连接照样在表里，
/// 故文本腿必须按 `ConnectionStatus == Connected` 过滤），而 `RasEnumConnections` 枚举的
/// 本来就只有**已建立**的连接 —— 不需要状态列，也就没有「状态列名一漂移就逢配置必报」这条缝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasConnection {
    /// `szEntryName` —— 连接建立后，承载流量那个接口的别名逐字就是它（w207 实测
    /// `PolarisProbeL2TP`，ifIndex 35），故它与路由条目的 `interface_alias` 可直接比。
    pub name: String,
    /// `dwFlags` 含 [`RASCF_ALL_USERS`]：这条是「所有用户」作用域的连接（企业下发的常见形态）。
    pub all_users: bool,
}

/// 一次只读枚举失败：哪个 API、Win32 错误码多少。
///
/// 带上 `api` 而不是只回错误码：三个 API 的失败面完全不同（RAS 那条的失败是 **API 运行期**
/// 失败 —— RasMan 服务被禁用 / 组策略受限时 `RasEnumConnectionsW` 返回非零；路由表那条失败
/// 基本等于系统异常），调用方要按腿分别决定降级还是报错。
///
/// # 🔴 降级腿覆盖的**不是**「没有 rasapi32 的 SKU」
///
/// `RasEnumConnectionsW` 是**导入表符号**（静态链接到 rasapi32.dll），DLL 缺席时失败的是
/// **进程加载**——helper / app 根本起不来，一行 Rust 代码都没执行，更够不到本类型。故本腿的
/// 降级分支唯一覆盖的是「DLL 在、函数调得到、但返回了非零错误码」这一格。把它写成「没有
/// rasapi32 的 SKU 会降级」是给一条永远走不到的路径留了假的安全感。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{api} 失败（Win32 错误码 {code}）")]
pub struct NetInfoError {
    /// 出错的 API 名（逐字，便于对着 MS 文档查错误码）。
    pub api: &'static str,
    /// Win32 错误码。
    pub code: u32,
}

/// 路由前缀长度的合法域：v4 `0..=32` / v6 `0..=128`。
///
/// # 🔴 与 [`prefix_is_valid`] **不同**，`0` 在这里是合法的
///
/// 那个函数判的是「本机 LAN 段」（own_lan），`0` 在那里是哨兵（默认路由不是 LAN 段，混进去会让
/// carve guard 与一切 mesh 段相交、bypassLAN 整体静默失效）。本函数判的是**路由表条目**，
/// 而 `0.0.0.0/0` / `::/0` 恰恰是这条链**最重要**的那一条 —— 一条抢默认路由的全隧道。
/// 复用那个函数会把它整条丢掉，于是「某个 VPN 要走你全部流量」这件事在界面上消失。
#[must_use]
pub fn route_prefix_len_is_valid(len: u8, is_v6: bool) -> bool {
    if is_v6 {
        len <= 128
    } else {
        len <= 32
    }
}

/// v4 目的前缀 → 规范串（`{点分}/{长度}`）。长度越界 → `None`（整条丢弃）。
///
/// 规范形与 `route_probe::parse_route_print_routes` 的 v4 分支**逐字同形**
/// （`format!("{dest}/{len}")`）—— 两条腿产出的 `RouteEntry` 要能直接比，才谈得上夹具对照。
#[must_use]
pub fn v4_route_prefix(octets: [u8; 4], len: u8) -> Option<String> {
    route_prefix_len_is_valid(len, false).then(|| format!("{}/{len}", v4_octets_to_string(octets)))
}

/// v6 目的前缀 → 规范串（压缩地址 + `/长度`）。长度越界 → `None`。
///
/// 与 `route_probe::canonical_ipv6_prefix` 同形：地址走 [`std::net::Ipv6Addr`] 的标准压缩
/// （`route print -6` 打印的也是压缩写法，两边不走同一种规范化就比不上）。
#[must_use]
pub fn v6_route_prefix(octets: [u8; 16], len: u8) -> Option<String> {
    route_prefix_len_is_valid(len, true).then(|| format!("{}/{len}", v6_octets_to_string(octets)))
}

/// `ERROR_NOT_FOUND`（winerror.h `1168`）。本地常量的理由同 [`RASCF_ALL_USERS`]：
/// 判据要在 Linux 上被单测覆盖，值由 `win_impl` 的 `const _` 断言对着 `windows-sys` 钉死。
pub const ERROR_NOT_FOUND: u32 = 1168;

/// `ERROR_INVALID_PARAMETER`（winerror.h `87`）。同上。
pub const ERROR_INVALID_PARAMETER: u32 = 87;

/// `ConvertInterfaceLuidToAlias` 的这个错误码是不是「这个接口已经不在了」（纯逻辑，跨平台可测）。
///
/// # 为什么必须分出这一格
///
/// 路由表是 `GetIpForwardTable2` 一次性取回的**快照**，接口别名却要**逐行**再问一次内核。
/// 两次调用之间接口可以消失 —— 起核峰值正是它最常发生的时刻（TUN 建/拆、RAS 拨号重连、
/// wintun 适配器热插）。把这一格当成「拿不到事实」整表报错，结果是用户在最需要冲突提示的
/// 那一刻收到「探测失败」，而真相只是路由表里有一行属于一个刚拆掉的接口。
///
/// **其余错误码仍必须整次 `Err`**：那些是「读法塌了」（句柄不可用、缓冲不够、权限异常），
/// 少掉的行可能正是唯一那条隧道宣告 —— 与 [`enumerate_ip_forward_entries`] 头注的纪律一致。
///
/// # `ERROR_INVALID_PARAMETER` 为什么也算「接口没了」
///
/// 该码在本 API 上有两类成因：(1) 出参缓冲为空 / 长度不足；(2) LUID 解不到接口。本调用点把
/// (1) **结构性排除**了 —— 缓冲是定长 `[u16; IF_MAX_STRING_SIZE + 1]` 栈数组、长度按 MS 文档的
/// 上界给出、LUID 是调用方栈上的合法引用，三者都不随宿主状态变。故在这一个调用点上，该码
/// 只可能来自 (2)。**推测**（本机判不了，列真机项）：LUID 失效时具体回 1168 还是 87 由内核
/// 版本决定，两个都认才不会漏。
#[must_use]
pub const fn luid_alias_error_means_interface_gone(code: u32) -> bool {
    code == ERROR_NOT_FOUND || code == ERROR_INVALID_PARAMETER
}

/// `RASCONN.dwFlags` 的 `RASCF_AllUsers` 位（`ras.h` `0x00000001`）。
///
/// 本地常量而非从 `windows-sys` 取：这一位要在 Linux 上被纯逻辑单测覆盖，而 `windows-sys`
/// 只在 Windows target 上进依赖图。值由 `win_impl` 里的 `const _` 断言对着
/// `windows_sys::…::Rras::RASCF_AllUsers` 编译期钉死 —— 抄错会在交叉编译时红，不会静默漂。
pub const RASCF_ALL_USERS: u32 = 0x0000_0001;

/// 这条 RAS 连接是不是「所有用户」作用域（纯逻辑，跨平台可测）。
///
/// Q8 拍板的落点：`RasEnumConnections` **一次枚举**就同时覆盖当前用户与 all-user 两个作用域
/// （文本腿要跑 `Get-VpnConnection` 与 `Get-VpnConnection -AllUserConnection` 两条命令），
/// 本位只用来如实标注某条连接属于哪个作用域，**不作过滤判据** —— 两个作用域的连接都要报。
#[must_use]
pub fn ras_flags_are_all_users(flags: u32) -> bool {
    flags & RASCF_ALL_USERS != 0
}

/// `RasEnumConnectionsW` 回写的「需要多少字节」→ 下一次该开几个 `RASCONN`（纯逻辑，跨平台可测）。
///
/// # 为什么要**单调增长**而不是直接信 `cb`
///
/// 两步取缓冲与 `GetAdaptersAddresses` 同形：第二次调用同样会再回 `ERROR_BUFFER_TOO_SMALL`
/// （两次调用之间又多拨上来一条 VPN）。此时若照抄回写的 `cb` 重开，而 `cb` 恰好没变大
/// （API 报的是「当前这一刻需要的字节数」，第三条连接可能在下一瞬才建立），重试就在原地空转、
/// 把预算白白烧完 —— 表现为一次本来能成的枚举被判失败，RAS 那一族隧道整族失联。故取
/// `max(按 cb 算出的条数, 当前条数 + 1)`：每一轮**至少**多一个槽，预算里的每一次都真的在推进。
///
/// 向上取整同 [`u64_cells_for`]（`cb` 不是 `sizeof(RASCONN)` 整数倍时宁多一个槽）；
/// 首帧的 1 条由调用方给定，本函数只管「下一次开多大」。
#[must_use]
pub fn ras_entries_for(wanted_bytes: u32, entry_size: usize, current: usize) -> usize {
    let by_api = (wanted_bytes as usize).div_ceil(entry_size.max(1));
    by_api.max(current.saturating_add(1))
}

/// 定长 `WCHAR` 缓冲（NUL 结尾）→ `String`。无 NUL 时吃满整个缓冲（纯逻辑，跨平台可测）。
///
/// **必须按 NUL 截断**而不是整段解码：`RASCONNW.szEntryName` 是 `[u16; 257]` 的定长数组，
/// 整段解码会把连接名后面那一串 `\0` 一起带进 `String` —— 而下游是拿它与路由表的接口别名
/// **逐字**比的，带尾巴就永远比不上，RAS 那一族隧道于是整族静默失联。
#[must_use]
pub fn wide_nul_terminated_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

#[cfg(windows)]
#[allow(unsafe_code)] // windows-sys FFI（GetAdaptersAddresses + 链表遍历）必须 unsafe；每处附 SAFETY。
mod win_impl {
    use super::{
        luid_alias_error_means_interface_gone, prefix_is_valid, ras_entries_for,
        ras_flags_are_all_users, should_retry_after_overflow, u64_cells_for, v4_octets_to_string,
        v4_route_prefix, v6_octets_to_string, v6_route_prefix, wide_nul_terminated_to_string,
        AdapterKind, IpForwardEntry, LocalUnicastAddr, NetInfoError, NetworkAdapterInfo,
        RasConnection, ERROR_INVALID_PARAMETER, ERROR_NOT_FOUND, IF_TYPE_SOFTWARE_LOOPBACK,
        RASCF_ALL_USERS,
    };
    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ConvertInterfaceLuidToAlias, FreeMibTable, GetAdaptersAddresses, GetIpForwardTable2,
        GAA_FLAG_INCLUDE_ALL_INTERFACES, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER,
        GAA_FLAG_SKIP_MULTICAST, GAA_FLAG_SKIP_UNICAST, IP_ADAPTER_ADDRESSES_LH,
        MIB_IPFORWARD_TABLE2,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::{IF_MAX_STRING_SIZE, NET_LUID_LH};
    use windows_sys::Win32::NetworkManagement::Rras::{
        RASCF_AllUsers, RasEnumConnectionsW, ERROR_BUFFER_TOO_SMALL, RASCONNW,
    };
    use windows_sys::Win32::Networking::WinSock::{
        AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6,
    };

    /// `RASCF_AllUsers` 的本地常量必须与 `windows-sys` 的一致。抄错在交叉编译期就红，
    /// 而不是让「all-user 作用域标注」在真机上默默恒 false。
    const _: () = assert!(
        RASCF_ALL_USERS == RASCF_AllUsers,
        "RASCF_ALL_USERS 与 windows-sys 的 RASCF_AllUsers 不一致"
    );

    /// 「接口已消失」两个错误码的本地镜像同样编译期钉死 —— 抄错会让 A1 的跳过腿要么恒不命中
    /// （回到「一行失败整表丢弃」），要么误把别的失败当成「接口没了」而吞掉真异常。
    const _: () = assert!(
        ERROR_NOT_FOUND == windows_sys::Win32::Foundation::ERROR_NOT_FOUND
            && ERROR_INVALID_PARAMETER == windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER,
        "ERROR_NOT_FOUND / ERROR_INVALID_PARAMETER 与 windows-sys 的取值不一致"
    );

    /// 承载缓冲区用 `Vec<u64>` 的**前提**：目标结构体的 align 不得超过 u64 的 align。
    /// 编译期钉死而非写注释 —— windows-sys 将来改 layout（比如塞进 align(16) 字段）时这里就红，
    /// 而不是留一个「实践上没炸」的未对齐引用（UB）。
    const _: () = assert!(
        std::mem::align_of::<IP_ADAPTER_ADDRESSES_LH>() <= std::mem::align_of::<u64>(),
        "IP_ADAPTER_ADDRESSES_LH 的对齐已超过 u64，Vec<u64> 承载不再安全"
    );

    /// 枚举本机全部单播地址（v4 + v6）。失败 → 空 Vec（best-effort，对齐 unix 腿的 `getifaddrs` 失败腿）。
    pub fn enumerate_local_unicast_addrs() -> Vec<LocalUnicastAddr> {
        // 只跳过用不到的族；**绝不能带 GAA_FLAG_SKIP_UNICAST**（本函数要的正是它）。
        let flags: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 0;
        let mut retries = 0u32;
        // 探大小与填充在**同一个** retries 预算里循环：填充调用也会 overflow（两次调用之间适配器增多），
        // 那时 API 已回写新 size，直接返空等于本次 own_lan 整体缺位（carve guard 失效）。
        let buf: Vec<u64> = loop {
            // SAFETY: 探大小形态（pAdapterAddresses = NULL），API 契约保证此形态只写 size。
            let rc = unsafe {
                GetAdaptersAddresses(0, flags, std::ptr::null(), std::ptr::null_mut(), &mut size)
            };
            if rc == NO_ERROR {
                return Vec::new(); // 无适配器（size=0）或系统直接给全（罕见）→ 无从遍历，诚实返空
            }
            if rc != ERROR_BUFFER_OVERFLOW {
                if !should_retry_after_overflow(retries) {
                    return Vec::new();
                }
                retries += 1;
                continue;
            }
            let mut cells: Vec<u64> = vec![0u64; u64_cells_for(size)];
            // SAFETY: cells 容量 ≥ size 字节（u64_cells_for 向上取整），且 Vec<u64> 的 align 满足
            // IP_ADAPTER_ADDRESSES_LH（上方 const 断言编译期保证）；API 只写 cells 内，不持有它。
            let rc = unsafe {
                GetAdaptersAddresses(
                    0,
                    flags,
                    std::ptr::null(),
                    cells.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            if rc == NO_ERROR {
                break cells;
            }
            // 填充期 overflow：API 已回写更大的 size，共用预算重来；其余错误 → 诚实返空。
            if rc == ERROR_BUFFER_OVERFLOW && should_retry_after_overflow(retries) {
                retries += 1;
                continue;
            }
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut ptr = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: 链表由 GetAdaptersAddresses 填充在 buf 内，Next 为 NULL 终止；仅读不写。
        while !ptr.is_null() {
            let entry: &IP_ADAPTER_ADDRESSES_LH = unsafe { &*ptr };
            let is_loopback = entry.IfType == IF_TYPE_SOFTWARE_LOOPBACK;
            let mut ua = entry.FirstUnicastAddress;
            // SAFETY: 单播地址子链表同样在 buf 内、Next 为 NULL 终止。
            while !ua.is_null() {
                let addr = unsafe { &*ua };
                if let Some(item) = read_unicast(
                    addr.Address.lpSockaddr.cast(),
                    addr.OnLinkPrefixLength,
                    is_loopback,
                ) {
                    out.push(item);
                }
                ua = addr.Next;
            }
            ptr = entry.Next.cast_const();
        }
        out
    }

    /// 枚举可供 sing-box `bind_interface` 使用的适配器名与展示信息。
    pub fn enumerate_network_adapters() -> Vec<NetworkAdapterInfo> {
        let flags: u32 = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size = 0u32;
        let rc = unsafe {
            GetAdaptersAddresses(0, flags, std::ptr::null(), std::ptr::null_mut(), &mut size)
        };
        if rc != ERROR_BUFFER_OVERFLOW || size == 0 {
            return Vec::new();
        }
        let mut cells = vec![0u64; u64_cells_for(size)];
        let rc = unsafe {
            GetAdaptersAddresses(
                0,
                flags,
                std::ptr::null(),
                cells.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                &mut size,
            )
        };
        if rc != NO_ERROR {
            return Vec::new();
        }

        let mut out = Vec::new();
        let mut ptr = cells.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        while !ptr.is_null() {
            let entry = unsafe { &*ptr };
            let adapter_name = if entry.AdapterName.is_null() {
                String::new()
            } else {
                unsafe { std::ffi::CStr::from_ptr(entry.AdapterName.cast()) }
                    .to_string_lossy()
                    .into_owned()
            };
            // Go `net.InterfaceByName`（sing-box `bind_interface` 的 Windows 查找路径）使用
            // InterfaceAlias/FriendlyName，不接受 AdapterName GUID。真机用同一随包核拨号验证：
            // Alias 成功，GUID 报 `no such network interface`。
            let name = wide_string(entry.FriendlyName).unwrap_or_else(|| adapter_name.clone());
            if !name.is_empty() {
                let display_name = wide_string(entry.Description).unwrap_or_else(|| name.clone());
                let mut addresses = Vec::new();
                let mut ua = entry.FirstUnicastAddress;
                while !ua.is_null() {
                    let addr = unsafe { &*ua };
                    if let Some(item) = read_unicast(
                        addr.Address.lpSockaddr.cast(),
                        addr.OnLinkPrefixLength,
                        entry.IfType == IF_TYPE_SOFTWARE_LOOPBACK,
                    ) {
                        addresses.push(item.ip);
                    }
                    ua = addr.Next;
                }
                addresses.sort();
                addresses.dedup();
                out.push(NetworkAdapterInfo {
                    name,
                    display_name,
                    // IF_OPER_STATUS::IfOperStatusUp 的稳定 Win32 数值。
                    is_up: entry.OperStatus == 1,
                    is_loopback: entry.IfType == IF_TYPE_SOFTWARE_LOOPBACK,
                    addresses,
                });
            }
            ptr = entry.Next.cast_const();
        }
        out.sort_by(|a, b| {
            b.is_up
                .cmp(&a.is_up)
                .then_with(|| a.display_name.cmp(&b.display_name))
        });
        out
    }

    fn wide_string(ptr: *const u16) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
        let mut len = 0usize;
        while unsafe { *ptr.add(len) } != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) });
        (!text.trim().is_empty()).then_some(text)
    }

    /// 读一条 `SOCKADDR`（v4/v6）→ [`LocalUnicastAddr`]。非 IP 族 / 前缀越界 / 空指针 → `None`。
    ///
    /// SAFETY 契约：`sa` 要么为 NULL，要么指向 API 填充的、按 `sa_family` 对应大小的合法 sockaddr。
    fn read_unicast(
        sa: *const windows_sys::Win32::Networking::WinSock::SOCKADDR,
        prefix: u8,
        is_loopback: bool,
    ) -> Option<LocalUnicastAddr> {
        if sa.is_null() {
            return None;
        }
        // SAFETY: 非空即指向合法 sockaddr（见上方契约）；只读 sa_family 这一个定长头字段。
        let family = unsafe { (*sa).sa_family };
        if family == AF_INET {
            if !prefix_is_valid(prefix, false) {
                return None;
            }
            // SAFETY: sa_family == AF_INET ⇒ 该缓冲区是 SOCKADDR_IN（Win32 契约）。
            let v4 = unsafe { &*sa.cast::<SOCKADDR_IN>() };
            // SAFETY: S_un 是 in_addr 的 union，S_addr（u32，网络序）与四字节数组同一块存储。
            let octets = unsafe { v4.sin_addr.S_un.S_addr }.to_ne_bytes();
            Some(LocalUnicastAddr {
                ip: v4_octets_to_string(octets),
                prefix,
                is_loopback,
            })
        } else if family == AF_INET6 {
            if !prefix_is_valid(prefix, true) {
                return None;
            }
            // SAFETY: sa_family == AF_INET6 ⇒ 该缓冲区是 SOCKADDR_IN6（Win32 契约）。
            let v6 = unsafe { &*sa.cast::<SOCKADDR_IN6>() };
            // SAFETY: u 是 in6_addr 的 union，Byte 成员即 16 字节网络序地址。
            let octets = unsafe { v6.sin6_addr.u.Byte };
            Some(LocalUnicastAddr {
                ip: v6_octets_to_string(octets),
                prefix,
                is_loopback,
            })
        } else {
            None
        }
    }

    // ── D1：路由表 / 适配器类型 / RAS 连接（外来隧道冲突探测的三条取材腿）────────────────

    /// 两步法取 `GetAdaptersAddresses` 的输出缓冲（探大小 → 按 size 填充，共用重试预算）。
    ///
    /// 与 [`enumerate_local_unicast_addrs`] 里那段同形，但**返回错误码而不是空 Vec**：
    /// 隧道判据那条腿上「一张适配器都没有」会被下游读成「这台机器上没有隧道」——
    /// 与 `route_probe::parse_windows_tunnel_interfaces` 对空名单必须报错是同一条纪律。
    fn adapters_buffer(flags: u32) -> Result<Vec<u64>, NetInfoError> {
        const API: &str = "GetAdaptersAddresses";
        let mut size: u32 = 0;
        let mut retries = 0u32;
        loop {
            // SAFETY: 探大小形态（pAdapterAddresses = NULL），API 契约保证此形态只写 size。
            let rc = unsafe {
                GetAdaptersAddresses(0, flags, std::ptr::null(), std::ptr::null_mut(), &mut size)
            };
            if rc == NO_ERROR {
                // size=0（无适配器）或系统直接给全：都没有可遍历的缓冲，交调用方按空表处理。
                return Ok(Vec::new());
            }
            if rc != ERROR_BUFFER_OVERFLOW {
                return Err(NetInfoError { api: API, code: rc });
            }
            let mut cells: Vec<u64> = vec![0u64; u64_cells_for(size)];
            // SAFETY: cells 容量 ≥ size 字节（u64_cells_for 向上取整），align 由上方 const 断言
            // 保证 ≥ IP_ADAPTER_ADDRESSES_LH 所需；API 只写 cells 内，不持有它。
            let rc = unsafe {
                GetAdaptersAddresses(
                    0,
                    flags,
                    std::ptr::null(),
                    cells.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
                    &mut size,
                )
            };
            if rc == NO_ERROR {
                return Ok(cells);
            }
            // 填充期 overflow：API 已回写更大的 size，共用预算重来。
            if rc == ERROR_BUFFER_OVERFLOW && should_retry_after_overflow(retries) {
                retries += 1;
                continue;
            }
            return Err(NetInfoError { api: API, code: rc });
        }
    }

    /// 枚举全部适配器的**别名 + IANA ifType**（隧道判据的取材面）。
    ///
    /// `GAA_FLAG_INCLUDE_ALL_INTERFACES` 是 `Get-NetAdapter -IncludeHidden` 的对应物：不带它时
    /// 未启用 IP 的 NDIS 接口（Teredo / 6to4 / IP-HTTPS 这类 `Not Present` 的协议隧道）不会返回。
    /// 方向与本条链一致 —— **报多不报少**：多出来的伪接口在路由表上一条都没有，进不了 `foreign`；
    /// 漏掉一张隧道网卡则是一句自信的「无冲突」。
    ///
    /// 地址三族全 skip：本函数只要名字与类型，地址由 [`enumerate_local_unicast_addrs`] 那条腿负责。
    ///
    /// # Errors
    ///
    /// [`NetInfoError`]：`GetAdaptersAddresses` 非 `NO_ERROR`/`ERROR_BUFFER_OVERFLOW`，
    /// 或重试预算用尽。**不折成空表**（空表 = 「没有隧道」）。
    pub fn enumerate_adapter_kinds() -> Result<Vec<AdapterKind>, NetInfoError> {
        let flags: u32 = GAA_FLAG_SKIP_UNICAST
            | GAA_FLAG_SKIP_ANYCAST
            | GAA_FLAG_SKIP_MULTICAST
            | GAA_FLAG_SKIP_DNS_SERVER
            | GAA_FLAG_INCLUDE_ALL_INTERFACES;
        let buf = adapters_buffer(flags)?;
        let mut out = Vec::new();
        if buf.is_empty() {
            return Ok(out);
        }
        let mut ptr = buf.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
        // SAFETY: 链表由 GetAdaptersAddresses 填充在 buf 内，Next 为 NULL 终止；仅读不写。
        while !ptr.is_null() {
            let entry: &IP_ADAPTER_ADDRESSES_LH = unsafe { &*ptr };
            // 别名（FriendlyName）是与路由表 / RAS 连接名同一个命名空间的那一个；`AdapterName`
            // 是 GUID，两者混用会让 join 半数对不上（见 enumerate_network_adapters 的头注）。
            if let Some(alias) = wide_string(entry.FriendlyName) {
                out.push(AdapterKind {
                    alias,
                    if_type: entry.IfType,
                });
            }
            ptr = entry.Next.cast_const();
        }
        Ok(out)
    }

    /// LUID → 接口别名。`Ok(None)` = API 成功但这个接口**真的没有**别名（不是读不到）。
    ///
    /// 两支必须分开：读不到（rc≠0）会让整张路由表失去接口名，是「拿不到事实」，得往上报错；
    /// 真的没别名则是一个事实 —— 那种接口永远匹配不上任何隧道名，跳过它不丢信息。
    fn luid_to_alias(luid: &NET_LUID_LH) -> Result<Option<String>, NetInfoError> {
        // MS 文档给的上界：NDIS_IF_MAX_STRING_SIZE + 1 个 WCHAR（含结尾 NUL）。
        let mut buf = [0u16; IF_MAX_STRING_SIZE as usize + 1];
        // SAFETY: luid 指向调用方栈上的合法 NET_LUID_LH；buf 是定长数组，length 按**字符数**
        // 给出且与其真实容量一致；API 只往 buf 内写。
        let rc = unsafe { ConvertInterfaceLuidToAlias(luid, buf.as_mut_ptr(), buf.len()) };
        if rc != NO_ERROR {
            return Err(NetInfoError {
                api: "ConvertInterfaceLuidToAlias",
                code: rc,
            });
        }
        let alias = wide_nul_terminated_to_string(&buf);
        Ok((!alias.trim().is_empty()).then_some(alias))
    }

    /// 枚举内核路由表（IPv4 + IPv6 一次取全），接口名解成别名。
    ///
    /// `AF_UNSPEC` 一次拿双栈 —— 文本腿要跑 `route print -4` 与 `route print -6` 两个进程。
    ///
    /// 非 IP 族的行、前缀长度越界的行整条跳过（内核不产出这种行；判据在
    /// [`v4_route_prefix`] / [`v6_route_prefix`]，`/0` 是合法值必须留下）。
    ///
    /// # Errors
    ///
    /// [`NetInfoError`]：`GetIpForwardTable2` 失败，或某一行的 LUID 解别名时返回
    /// **除「接口已消失」以外**的错误码（判据 [`luid_alias_error_means_interface_gone`]）。
    /// 那些不折成跳过 —— 与 `route_probe::parse_route_print_routes` 的 `UnresolvedInterface`
    /// 同一条纪律：少掉的那几条可能正是唯一那条隧道宣告。
    ///
    /// 「接口已消失」是**例外且只是这一格**：接口在两次调用之间被拆掉（起核峰值常见）时整表
    /// 丢弃 = 用户在最需要冲突提示的那一刻收到「探测失败」。这类行跳过并汇总成一条 warn。
    pub fn enumerate_ip_forward_entries() -> Result<Vec<IpForwardEntry>, NetInfoError> {
        const API: &str = "GetIpForwardTable2";
        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        // SAFETY: 出参形态；API 成功时分配整张表并回写指针，失败时不写。
        let rc = unsafe { GetIpForwardTable2(AF_UNSPEC, &mut table) };
        if rc != NO_ERROR {
            return Err(NetInfoError { api: API, code: rc });
        }
        if table.is_null() {
            // 契约上不该发生（rc=NO_ERROR 必回写指针）。当成失败报，不当成「零条路由」。
            return Err(NetInfoError { api: API, code: 0 });
        }
        // 从这里往下**每条返回路径都必须先 FreeMibTable**，故结果先收进变量。
        let collected = collect_ip_forward_rows(table);
        // SAFETY: table 由 GetIpForwardTable2 分配，契约要求用 FreeMibTable 释放，且此后不再解引用。
        unsafe { FreeMibTable(table.cast()) };
        collected
    }

    /// [`enumerate_ip_forward_entries`] 的读表主体（拆出来是为了让 `FreeMibTable` 只写一处）。
    fn collect_ip_forward_rows(
        table: *const MIB_IPFORWARD_TABLE2,
    ) -> Result<Vec<IpForwardEntry>, NetInfoError> {
        // SAFETY: 调用方已保证 table 非空且指向 API 分配的合法表；只读。
        let header = unsafe { &*table };
        let count = header.NumEntries as usize;
        // SAFETY: `Table` 是 C 柔性数组（声明成 `[_; 1]`），实际长度由 NumEntries 给出，
        // 整块内存由 API 一次分配；只读不写。
        let rows = unsafe { std::slice::from_raw_parts(header.Table.as_ptr(), count) };
        let mut out = Vec::with_capacity(count);
        // 汇总而不是逐行 warn：接口热插时一次能带走同一接口的十几条路由，逐行出声只会把日志淹掉，
        // 而要判断「这次探测是不是缺了料」，需要的恰恰是**条数**。
        let mut vanished = 0usize;
        for row in rows {
            let len = row.DestinationPrefix.PrefixLength;
            // SAFETY: SOCKADDR_INET 是 union，`si_family` 与两支地址的 `sin*_family` 同偏移，
            // 读它是这个 union 的判别式读法（Win32 契约）。
            let family = unsafe { row.DestinationPrefix.Prefix.si_family };
            let prefix = if family == AF_INET {
                // SAFETY: si_family == AF_INET ⇒ 该 union 存的是 SOCKADDR_IN。
                let v4 = unsafe { row.DestinationPrefix.Prefix.Ipv4 };
                // SAFETY: S_un 是 in_addr 的 union，S_addr（u32，网络序）与四字节数组同一块存储。
                v4_route_prefix(unsafe { v4.sin_addr.S_un.S_addr }.to_ne_bytes(), len)
            } else if family == AF_INET6 {
                // SAFETY: si_family == AF_INET6 ⇒ 该 union 存的是 SOCKADDR_IN6。
                let v6 = unsafe { row.DestinationPrefix.Prefix.Ipv6 };
                // SAFETY: u 是 in6_addr 的 union，Byte 成员即 16 字节网络序地址。
                v6_route_prefix(unsafe { v6.sin6_addr.u.Byte }, len)
            } else {
                None
            };
            let Some(prefix) = prefix else {
                continue;
            };
            // 三分：解得出 ⇒ 收；解得出但为空（接口真的没有别名）⇒ 跳过；解不出 ⇒ 看错误码 ——
            // 「接口已消失」跳过并计数，其余整次枚举失败。
            let interface_alias = match luid_to_alias(&row.InterfaceLuid) {
                Ok(Some(alias)) => alias,
                Ok(None) => continue,
                Err(error) if luid_alias_error_means_interface_gone(error.code) => {
                    vanished += 1;
                    continue;
                }
                Err(error) => return Err(error),
            };
            out.push(IpForwardEntry {
                prefix,
                interface_alias,
            });
        }
        if vanished > 0 {
            log::warn!(
                "路由表有 {vanished} 行的出接口在 GetIpForwardTable2 与 ConvertInterfaceLuidToAlias \
                 之间已消失（接口热插 / 起核峰值常见）→ 已跳过这些行，其余 {} 行照常参与隧道判定",
                out.len()
            );
        }
        Ok(out)
    }

    /// 枚举**活动的** RAS / VPN 连接（两步取缓冲：先拿 `ERROR_BUFFER_TOO_SMALL`）。
    ///
    /// 一次枚举覆盖当前用户与 all-user 两个作用域（Q8）；作用域由 `dwFlags` 的
    /// [`RASCF_ALL_USERS`] 位如实标注，**不作过滤**。
    ///
    /// 首帧的 `dwSize` 必须填结构体大小 —— 那是 RAS API 的版本协商字段，不填直接 `ERROR_INVALID_SIZE`。
    ///
    /// **重试预算与 [`adapters_buffer`] 是同一套**（[`should_retry_after_overflow`]）：两条腿都是
    /// 「探大小 → 按回写的 size 重开」，也都会在第二次调用上再次 `TOO_SMALL`（两次调用之间又拨上来
    /// 一条连接）。此前 RAS 腿只重开一次就放弃，同一个竞态在这条腿上就是一次失败 —— 而失败的
    /// 后果是 RAS 那一族隧道整族看不见。口径统一后两条腿的缓冲竞态行为逐字相同。
    ///
    /// # Errors
    ///
    /// [`NetInfoError`]：`RasEnumConnectionsW` 非零返回，或重试预算用尽仍 `TOO_SMALL`。
    /// 调用方对本腿的失败**降级为空表**（API 运行期失败 —— 如 RasMan 被禁用 —— 的机器不该因此
    /// 整次探测失败），与另外两腿不同。降级覆盖的不是「没装 rasapi32」，理由见 [`NetInfoError`]。
    pub fn enumerate_ras_connections() -> Result<Vec<RasConnection>, NetInfoError> {
        const API: &str = "RasEnumConnectionsW";
        let entry_size = std::mem::size_of::<RASCONNW>();
        let entry_size_u32 = u32::try_from(entry_size).unwrap_or(u32::MAX);
        let mut wanted = 1usize;
        let mut retries = 0u32;
        let (conns, count) = loop {
            let mut conns: Vec<RASCONNW> = vec![RASCONNW::default(); wanted];
            conns[0].dwSize = entry_size_u32;
            let mut cb = u32::try_from(wanted * entry_size).unwrap_or(u32::MAX);
            let mut count: u32 = 0;
            // SAFETY: conns 至少 1 个元素、cb 与其字节容量一致、首帧 dwSize 已填；API 只写 conns 内。
            let rc = unsafe { RasEnumConnectionsW(conns.as_mut_ptr(), &mut cb, &mut count) };
            if rc == NO_ERROR {
                break (conns, count);
            }
            if rc != ERROR_BUFFER_TOO_SMALL || !should_retry_after_overflow(retries) {
                return Err(NetInfoError { api: API, code: rc });
            }
            retries += 1;
            // API 已把需要的字节数回写进 cb；按它重开，但每轮至少多一个槽（见 `ras_entries_for`）。
            wanted = ras_entries_for(cb, entry_size, wanted);
        };
        let count = (count as usize).min(conns.len());
        let mut out = Vec::with_capacity(count);
        for conn in conns.iter().take(count) {
            // RASCONNW 在 64 位上是 `repr(C, packed(4))`：不得对字段取引用，整条 Copy 出来再读。
            let conn = *conn;
            let name_buf: [u16; 257] = conn.szEntryName;
            let flags: u32 = conn.dwFlags;
            let name = wide_nul_terminated_to_string(&name_buf);
            if name.trim().is_empty() {
                continue;
            }
            out.push(RasConnection {
                name,
                all_users: ras_flags_are_all_users(flags),
            });
        }
        Ok(out)
    }
}

#[cfg(windows)]
pub use win_impl::{
    enumerate_adapter_kinds, enumerate_ip_forward_entries, enumerate_local_unicast_addrs,
    enumerate_network_adapters, enumerate_ras_connections,
};

#[cfg(test)]
mod tests;
