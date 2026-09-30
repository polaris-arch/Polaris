//! `Platform` 分派面：新增平台变体 / 新增分派点时不许「默默继承」。
//!
//! # 守的是什么（血证，不是假设）
//!
//! 2026-09-04 K6b 在 Android 上查清一条阻断级缺陷：`Platform::current()` 只认 macos/win/linux ⇒
//! Android 落进 `Platform::Other`，而 `Other` 上两条腿同时缺席 —— `exit_interface_for` 恒
//! `Ok(None)`、`managed_tun_interface_for_network_watcher` 恒 `None`。两条叠加让「起核前后网卡
//! 事实是否变了」这条判据恒真 ⇒ **300 秒内起核 69 次**。
//!
//! 两条腿各自都写着自洽的理由。错的是它们被同一个 `Other` 串在一起，而**没有一处代码为
//! Android 答过题** —— `Other` 的语义是「没有 helper 实现」（helper 谱系轴上的答案），各分派点
//! 却在自己的轴上把它读成了「未知平台，做保守的那件事」，而"保守"逐处不同。
//!
//! K10（2026-09-04）的处置是给 Android 一个**具名变体**：穷举 `match` 于是在编译期强制每一处
//! 作者写下 Android 的答案。本门守的是那份强制力的两个缺口：
//!
//! | 缺口 | 谁能钻过编译器 | 本门哪一支拦 |
//! |---|---|---|
//! | `match … { _ => … }` | 裸通配臂把新变体静默吸收 | **J2** |
//! | `p == Platform::X` / `matches!(p, …)` | 编译器对比较型分派**一句话都不说** | **J3** |
//! | 加了变体但没同步 `Platform::ALL` | 数组不是穷举 match | **J1** |
//! | `platform == "android"` / `match os { … }` | 字符串轴上编译器同样不说话 | **K11** |
//! | `#[cfg(target_os = "android")]` / `cfg(desktop)` | **编译期**分叉：新平台落进另一个平台的那一半 | **K13** |
//! | `matches!(self, Self::Mac \| …)` | `Self::` 写法让扫描器本身瞎掉 | **K13**（[`resolve_self_in_platform_impls`]） |
//! | 扫描器的 `VARIANT_NAMES` 漏了新变体 | 门还在，取材面塌了一半 | **K13**（[`variant_names_cover_every_variant`]） |
//!
//! # 四条轴，一句话各是什么（2026-09-06 起）
//!
//! | 轴 | 判据物 | 编译器管吗 | 本门哪一条 |
//! |---|---|---|---|
//! | 枚举（穷举 `match`） | `Platform` 值 | **管**（E0004） | J2 只补通配臂那个缺口 |
//! | 比较型 | `Platform` 值 | 不管 | [`comparison_style_platform_dispatch_is_registered`] |
//! | 字符串 | `process.platform` 风格串 | 不管 | [`string_platform_dispatch_is_registered`] |
//! | **cfg** | **编译目标**（不是运行期的值） | 不管 | [`cfg_axis_platform_dispatch_is_registered`] |
//!
//! 前三条判的都是**运行期**的平台值，第四条判的是**编译期**把代码切成两份的那条线 ——
//! 它的失效形态因此也不同：不是「新平台落进兜底」，而是「新平台落进**另一个平台的那一半**」。
//!
//! # 为什么门必须存在（编译期不变式不进产物）
//!
//! 穷举 `match` 的强制力只在**编译那一刻**存在，它不进产物、CI 上看不到、也无法回答「今天全仓
//! 有几处分派点、其中几处是编译器管不着的形态」。本门把那份不变式写成一条会红的判据，且它自己
//! 带取材面下限（FLOOR）—— 扫描器坏掉、路径改名、正则失配一律表现为「扫到的比下限少」而变红，
//! 不会静默判绿。
//!
//! # 取材面（先剥后取，见 [`mask`]）
//!
//! `src-tauri/src/**` + `crates/*/src/**` 的**生产**文件（排除 `tests/` 目录与 `tests.rs`：测试里
//! 用通配臂是正当的，它们不是分派点）。判据一律在**剥掉注释与字符串字面量之后**的代码面上取 ——
//! 本文件自己的头注就写满了 `Platform::Other`，不剥就会自己污染自己。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use polaris_helper_proto::Platform;

// ===================== 判据参数 =====================

/// 生产代码里 `Platform` 分派 `match` 块数的**下限**（正面断言）。
///
/// 2026-09-04 K10 实测 38 处。取 30 是给正常增删留余量，但**不给「扫描器坏掉」留余量**：
/// 取材面塌掉（路径写错 / `mask` 把代码也剥了 / 块配平写反）表现为个位数甚至 0，一定低于本值。
/// 门的红绿因此不依赖「恰好没扫到违规」这种无信息量的绿。
const MATCH_BLOCK_FLOOR: usize = 30;

/// 比较型分派点**出现次数**（含重复形态）的下限（正面断言，同上）。
///
/// 2026-09-04 实测 25 次；2026-09-06 [`resolve_self_in_platform_impls`] 把 `impl Platform` 里的
/// `Self::` 也纳入取材面后实测 33 次（多出的 8 处全在 `helper-proto/src/lib.rs`，见登记表末尾）。
const COMPARISON_FLOOR: usize = 20;

/// 允许保留裸通配臂的 `Platform` match：(仓库相对路径, **折叠空白后的块体原文**, 理由)。
///
/// # 为什么第二列是块体原文，而不是只写路径
///
/// 初版按路径放行，那等于给整个文件开一张长期通行证：同文件里此后新加的任何一处裸通配臂
/// 都会跟着被放行，而门不会说一个字 —— 「有理由的洞」变成「没人看着的洞」。
/// 改成按块体原文匹配之后，放行的射程恰好是被审过的那一个 `match`：
///
/// - 同文件**另开**一处裸通配 ⇒ 块体对不上 ⇒ 红（必须单独来这里答题）；
/// - 被放行的这个 `match` **改了臂** ⇒ 登记的块体找不到了 ⇒ 红（判据变了就重新审一次）。
///
/// 块体取的是**掩码后**的代码面（[`mask`]：注释与字符串字面量已抹成空格，`impl Platform` 里的
/// `Self::` 已归一成 `Platform::`），故 `parse` 那条里 `"darwin" | "macos"` 只剩一个裸 `|`，
/// `"linux"` 之类只剩空白 —— 那不是笔误，而是这一面上它本来的样子。
///
/// # 唯一一条放行（2026-09-06）
///
/// K10 把当时仅有的四处裸通配全部展开成了具名臂（`helper-client/src/manager.rs` 的
/// `is_installed` / `install_script_name` / `uninstall_script_name`，与
/// `src-tauri/src/runtime/helper.rs` 的 `protected_core_dir`），表因此一度为空。
/// 本次给扫描器补上 `Self::` 归一化后，`Platform::parse` 进入了取材面 —— 它的 `_ =>` 是
/// **匹配 `&str` 的**通配臂，不是 `Platform` 的：`&str` 上不存在穷举，通配是唯一写法，
/// 而它的兜底值 `Platform::Other` 恰恰就是「本仓没为这个平台答过题」这件事的显式表达。
/// 放行它不放松任何强制力：新增平台**变体**时该红的是别处（穷举 `match` 编译不过、
/// `platform_variants_stay_in_sync_with_all` 与 [`variant_names_cover_every_variant`] 变红）；
/// 新增平台**名字面量**时该红的是 `STRING_DISPATCH_REGISTRY` 里 `parse` 那条（语句原文变了）。
const WILDCARD_ALLOW: &[(&str, &str, &str)] = &[(
    "crates/helper-proto/src/lib.rs",
    "{ | => Platform::Mac, | => Platform::Win, => Platform::Linux, => Platform::Android, \
     => Platform::Ios, _ => Platform::Other, }",
    "`Platform::parse` 的 `match s { … _ => Self::Other }`。被 match 的是 `&str` 而不是 \
     `Platform`：字符串上没有穷举可言，`_` 是唯一写法。它的兜底值 `Platform::Other` 就是\
     「本仓没为这个平台答过题」的显式落点，正是本门头注里 `Other` 该有的语义。\
     2026-09-06 因扫描器补上 `Self::` 归一化而首次进入取材面 —— 判据没变，看得见的东西变多了。",
)];

/// 比较型（非 `match`）`Platform` 分派点登记表。
///
/// 每条 = (仓库相对路径, 折叠空白后的语句原文, 该形态在该文件里的出现次数,
/// 该处对 **Android** 的答案与为什么可以是这个答案)。
///
/// # 这张表的强制力在哪
///
/// - **新增一处比较型分派点** ⇒ 扫到的多于表里的 ⇒ 红。作者被迫在这里写下 Android 的答案。
/// - **改动或删除已登记的一处** ⇒ 表里的找不到了 ⇒ 红（防腐烂）。改了判据就要重新答一次题。
/// - 它**不能**保证第三列写的是真话 —— 那一列是审计结论，force 在「必须写一条」而不在内容。
///   内容的真值由各站点自己的单测与 §可达性证据承担。
///
/// # 2026-09-06：iOS 逐条复核（当时的射程 25 次 / 19 种形态；扩面后见下一节）
///
/// 第三列问的是「Android 在这里得到什么」。加 `Platform::Ios` 时逐条又问了一遍 iOS，结论：
///
/// | 处置 | 次数 | 是哪些 |
/// |---|---|---|
/// | **必须补 iOS，否则是本次改动引入的回归** | 1 | `builder/log.rs` 的 `log.output` 允许清单（漏了 ⇒ 导出诊断里核日志为空；已由全变体单测钉死，实测变异红） |
/// | **iOS 落在安全侧，答案恰好是对的**（显式确认，非白捡） | 23 | 其余全部 |
/// | **不是分派点**（cfg(windows) 内的常量定义） | 1 | `helper/platform/windows/mod.rs` |
///
/// 那 23 次为什么安全：它们分派的都是 mac/win/linux 三家的**平台专属机制**
/// （utun 基线快照、`networksetup`、注册表、gsettings、`resolvectl`、pkexec 127、SCM…），
/// iOS 一律落到「跳过这条腿」那一侧；其中 `startup.rs::should_start_via_helper` 与
/// `dns_ops.rs::takeover_supported` 两处是**允许清单**形态、与 log.rs 同形却答案相反，
/// 已在各自条目的第三列里单独写下，不许互相推广。
///
/// # 2026-09-06 K13：射程从 25 次 / 19 种扩到 **33 次 / 27 种**
///
/// 上一版这里写着一句「⚠️ 本表射程之外的一处同形判据：`Platform::has_token_line` 写作
/// `matches!(self, Self::Mac | Self::Win)`，[`find_variant`] 扫不到它 —— iOS 在那里落 false
/// 是对的，但那是**没有门看着的对**」。那个洞已经堵上：[`resolve_self_in_platform_impls`]
/// 按**所在 `impl` 块的自身类型**把 `impl … Platform` 里的 `Self::` 归一成 `Platform::`
/// （只在那里，故别的枚举的 `Self::Other` 不受影响 —— 由
/// [`self_alias_does_not_leak_into_other_enums`] 拿四处真实源码正反双向钉住）。
///
/// 新进表的八处全在 `helper-proto/src/lib.rs`：`has_token_line` ×1、`Platform::ALL` ×1、
/// `Platform::current()` 的六个分支体。见本表末尾那一段。
///
/// 「不顺手改扫描器」的旧理由（别的枚举也有 `Self::Linux`/`Self::Other`）**复核成立**，
/// 所以收法不是一刀切，而是按 impl 块限定 —— 理由成立不等于洞可以留着。
///
/// # 为什么按「语句原文」而不是行号
///
/// 行号在任何一次插入注释后就全错，那种红是噪音、会训练人去无脑更新表。语句原文折叠空白后对
/// 排版免疫，只在**判据本身**改动时才变 —— 那正是要重新审的时刻。
///
/// # 为什么带出现次数
///
/// 同一形态多出一处也是一个**新的**分派点（它落在别的函数、别的生命周期阶段上），必须被看见
/// 一次。只判「有没有」会让「同文件里又加了一处一模一样的比较」静默通过。
type Comparison = (&'static str, &'static str, usize, &'static str);
const COMPARISON_REGISTRY: &[Comparison] = &[
    (
        "crates/helper/src/platform/windows/mod.rs",
        "pub const PLATFORM: polaris_helper_proto::Platform = polaris_helper_proto::Platform::Win",
        1,
        "不是分派点，是常量定义：本模块整个带 cfg(windows)，Android 上不编译。登记它只因为它\
         在取材面内 —— 让「表里每一条都必须能被找到」这条反腐烂判据不留例外口。",
    ),
    (
        "crates/system-integration/src/route_probe.rs",
        "platform: Platform::Win, command: CMD, missing: , needed: ,",
        2,
        "不是分派点，是**错误载荷的平台标签**：两处都在 `probe_windows` 唯一可达的解析器里，\
         构造 `RouteTableParseError::CaptureIncomplete` 时如实写下这份抓取来自哪个平台。\
         Android 走不到它们 —— `probe_foreign_tunnels` 的穷举 `match` 把 Android/iOS 判成 \
         `Unsupported`，一条 PowerShell 都不会跑。登记它只因为它在取材面内（同本表第一条的理由）：\
         让「表里每一条都必须能被找到」这条反腐烂判据不留例外口。",
    ),
    (
        "crates/mesh/src/exit_route.rs",
        "if self.platform != Platform::Mac",
        1,
        "Android → 走 else（不做 utun 基线快照）。正确：utun 是 macOS 特有的动态接口命名，\
         Android 上没有这个概念。且 mesh System 在 Android 已由 mesh_system_supported_on_platform 禁掉。",
    ),
    (
        "crates/mesh/src/exit_route.rs",
        "if self.platform == Platform::Mac",
        2,
        "两处同形（reassert 的接口消失检查 / clear_inner 的防误删）。Android → 跳过，正确：\
         两处守的都是 macOS utun 随停核消失的时序，Android 上 mesh System 整条不启用。",
    ),
    (
        "crates/config-engine/src/builder/log.rs",
        "let writes_log_to_file = input.proxy_mode_type.effective_on(deps.platform).is_tun() \
         && matches!( deps.platform, Platform::Mac | Platform::Win | Platform::Linux | \
         Platform::Android | Platform::Ios | Platform::Other )",
        1,
        "Android **必须在列**（K10 现补）：核是进程内 libbox，stdout 没有子进程管道可接管，\
         日志落盘是导出诊断拿到核原文的唯一途径。此前靠 parse(\"android\")==Other 顺带盖住，\
         给 Android 具名之后那条腿就断了 —— 不补即为本次改动引入的回归。\n\
         2026-09-05 K12 判据加了半条：接管方式改读 `ProxyModeType::effective_on(deps.platform)` \
         的**本平台生效值**。成因是 `proxy_mode_type` 缺省 `SystemProxy`，而 Android 上那个档位\
         没有承载物 ⇒ 照裸值判会让全新安装/备份恢复的客户端一条核日志都不落盘，正好把上面这条\
         抵消掉。新平台在这一格拿到的仍是「照自己存的档位办事」（`effective_on` 只对 Android \
         分叉），与本表其余条目同口径。\n\
         **2026-09-06：`Platform::Ios` 同样必须在列，本条因此腐烂变红了一次** —— 那是这张表在这一格\
         上唯一的牙。它与上面 Android 那段是**逐字同一条回归**：`matches!` 少一个变体只是求值 false，\
         本批 40 处穷举 match 一处都拦不住它，症状是「导出诊断里核日志是空的」，离成因很远。\
         iOS 的依据比 Android 更强一档：核跑在 NE 扩展进程里，app 进程连它的 stdout 都不在同一个进程\
         树上，落盘（写进 App Group 共享容器）是拿到核原文的唯一途径。\
         行为面由 `builder/log/tests/mod.rs::every_platform_writes_the_log_file_under_tun` 逐变体钉死\
         （删掉这里的 `Platform::Ios` 即红，已实测）。\
         另一处同形态、答案却相反的比较是 `runtime/proxy/startup.rs::should_start_via_helper` 的允许\
         清单：那里 iOS **不在列**才是对的（无 helper）。两格的差别只在「不在允许清单里」是不是想要的\
         结果，故两格都要各自写下答案，不能互相推广。",
    ),
    (
        "crates/system-integration/src/dns_ops.rs",
        "matches!(self.platform, Platform::Mac)",
        1,
        "takeover_supported。Android → false，正确：系统解析器不在链路上（DNS 由核在 tun fd 内\
         自理），且非 root 应用无权写别的网卡的 DNS。false 让控制器在写 marker 前就早退。",
    ),
    (
        "crates/system-integration/src/proxy_ops/ops.rs",
        "ops.native_macos = cfg!(target_os = ) && ops.platform == Platform::Mac",
        1,
        "Android → false。native_macos 是 macOS 原生事务腿的开关，与本平台无关。",
    ),
    (
        "crates/system-integration/src/proxy_ops/ops.rs",
        "self.native_macos && self.platform == Platform::Mac",
        1,
        "uses_native_macos，函数本身带 cfg(target_os = \"macos\")，Android 上不编译。",
    ),
    (
        "crates/system-integration/src/proxy_ops/ops.rs",
        "if self.platform == Platform::Linux",
        1,
        "Android → false，正确：这是 Linux gsettings 腿的分流；Android 没有 gsettings，\
         且整个系统代理面在 Android 上由 Other|Android 臂返 Err(UnsupportedPlatform)。",
    ),
    (
        "crates/system-integration/src/proxy_ops/ops.rs",
        "if self.platform != Platform::Mac",
        1,
        "Android → 走非 mac 腿。同上，本模块在 Android 上一律 Err，此处不可达。",
    ),
    (
        "crates/system-integration/src/proxy_ops/ops.rs",
        "if self.platform == Platform::Mac && !original.mac_services.is_empty()",
        2,
        "两处同形（restore/reconcile 的 mac 服务快照腿）。Android → false，同上不可达。",
    ),
    (
        "crates/system-integration/src/proxy_ops/windows.rs",
        "if Platform::current() != Platform::Win",
        1,
        "Windows QUIC 旧规则清理预热。Android → 直接 return false，正确且必要（否则会去\
         spawn netsh）。",
    ),
    (
        "crates/helper-client/src/manager.rs",
        "if self.platform != Platform::Win",
        1,
        "pipe_self_uninstall（win 零 UAC 自卸载）。Android → false。整个 helper-client 在\
         Android 上不可达（runtime/helper.rs::platform_supported 恒 false）。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if matches!(platform, Platform::Linux) && code == 127",
        1,
        "pkexec 127 → AuthorizationUnavailable 的判据。Android → 走 Failed 分支，不可达\
         （Android 上没有提权脚本可跑）。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if self.platform != Platform::Linux",
        2,
        "两处同形（linux_resolved_takeover / linux_resolved_revert）。Android → 立即返\
         Err(\"unavailable on this platform\")，这是**显式**降级而非静默假成功，正确。",
    ),
    (
        "src-tauri/src/runtime/mesh.rs",
        "if !self.enabled || self.platform != Platform::Mac",
        1,
        "list_utuns。Android → 返空集，正确（无 utun 概念）。",
    ),
    (
        "src-tauri/src/runtime/mesh.rs",
        "if self.platform != Platform::Mac",
        1,
        "find_tailnet_iface。Android → 返逻辑名不轮询。不可达：mesh System 在 Android 已禁。",
    ),
    (
        "src-tauri/src/runtime/proxy/dns_takeover.rs",
        "if Platform::current() == Platform::Linux",
        4,
        "四处同形（set/restore/has_marker/reconcile 的 Linux resolved 分流）。Android → 走\
         通用 dns_controller 腿，而该控制器的 takeover_supported 在 Android 为 false ⇒ 写 marker \
         前早退 ⇒ 诚实 no-op。正确：Android 上没有要接管的系统解析器。",
    ),
    (
        "src-tauri/src/runtime/proxy/dns_takeover.rs",
        "if Platform::current() == Platform::Linux && !flushed",
        1,
        "刷 DNS 缓存失败的告警只给 Linux 报。Android → 不报，且 flush_os_dns_cache 的 Android \
         臂恒 true（本平台没有应用可刷的缓存），二者一致。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "mode.effective_on(platform).is_tun() && matches!(platform, Platform::Mac | Platform::Win \
         | Platform::Linux)",
        1,
        "should_start_via_helper。Android → false，正确且是起核能走对腿的前提：Android 起核\
         走进程内 libbox（android_bridge），不建 helper client。与 helper.rs::platform_supported \
         是同一个平台集合的两份写法，两份都必须把 Android 排除在外。\n\
         2026-09-05 R1 判据加了半条：接管方式改读 `ProxyModeType::effective_on` 的**本平台生效值**\
         （只对 Android 分叉）。对本条**零行为差** —— 合取项是 mac/win/linux 允许清单，Android 永远\
         走不到 —— 接上是为了让 `src-tauri/tests/android_takeover_is_a_platform_fact.rs` 那道\
         「每一处接管方式判据都读生效值」的门没有例外。Android 在这一格拿到的仍是 false。\n\
         **2026-09-06 iOS 的显式确认（本条语句原文未变，故本表不会因它变红 —— 所以更要写下来）**：\
         允许清单是 mac/win/linux，`Platform::Ios` 不在列 ⇒ 求值 false ⇒ 不建 helper client。\
         这个答案**恰好是对的**（`runtime::helper::platform_supported(Ios) == false`，两处是同一个平台\
         集合的两份写法），但它是白捡来的、没有人答过题。同一形态在 `builder/log.rs` 那一格上「不在\
         允许清单里」是**错的**（=导出诊断无核日志），两格必须各自记一笔，不许互相推广。\
         与 `is_tun()` 那半的交互：`effective_on(Ios)` 恒 Tun ⇒ 第一个合取项在 iOS 上恒真，整条判据完\
         全由平台允许清单决定，不存在「靠模式判据兜住」的第二层。",
    ),
    // ═══════════════════════════════════════════════════════════════════════
    // 2026-09-06 新进取材面的八处：`impl Platform` 块里写作 `Self::<Variant>` 的形态。
    //
    // 它们**一直都在生产代码里**，只是扫描器按 `Platform::<Variant>` 取材、看不见
    // （见 [`resolve_self_in_platform_impls`] 的头注：这正是上一批如实登记在函数注释里、
    // 却没有任何门看着的那个洞）。补上归一化之后它们第一次被要求答题。
    // ═══════════════════════════════════════════════════════════════════════
    (
        "crates/helper-proto/src/lib.rs",
        "matches!(self, Platform::Mac | Platform::Win)",
        1,
        "🔵 **本条就是那个洞本身**：`Platform::has_token_line`，源码写作 \
         `matches!(self, Self::Mac | Self::Win)`。它是 wire 头部发不发 token 行的**唯一**判据\
         （`codec::encode_frame` 曾另写一份且已经漂了，K10 已收口到本函数）。\n\
         iOS → false（不发 token 行）。**允许清单形态，答案是对的**：token 行是给 mac/win 那两种\
         helper 传送凭据用的，iOS 上根本没有 helper（`runtime::helper::platform_supported(Ios)` \
         恒 false，本批已为它写下具名答案），这条 wire 路径整个不可达。判错的方向不对称 ——\
         多发是把凭据发给未鉴权对端，少发只是被拒。Android / Other 同答同理。\n\
         与 `builder/log.rs` 那条**同形而异答**的允许清单相比，这一格「不在列」正是想要的结果，\
         那一格「不在列」是回归。两格各自记一笔，不许互相推广。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "pub const ALL: &'static [Self] = &[ Platform::Mac, Platform::Win, Platform::Linux, \
         Platform::Android, Platform::Ios, Platform::Other, ]",
        1,
        "不是分派点，是**全变体清单本体**：`Platform::ALL`。登记它只因为它落在取材面内 ——\
         让「表里每一条都必须能被找到」这条反腐烂判据不留例外口（同 `helper/src/platform/\
         windows/mod.rs` 那条常量登记）。\n\
         它自己的强制力不在本表：`platform_variants_stay_in_sync_with_all` 把它与 `enum Platform` \
         的声明逐名对拍，`variant_names_cover_every_variant` 又把本门扫描器的 `VARIANT_NAMES` \
         钉在它上面。漏一个变体，那三处会同时红。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "Platform::Mac",
        1,
        "`Platform::current()` 的 `cfg!(target_os = \"macos\")` 分支体。**这是整条枚举轴的产地**\
         （本仓唯一一处把编译目标翻译成 `Platform` 的地方），六个分支各是一处独立的构造点，\
         故逐个登记。iOS 走不到本支：`target_os` 对 iOS 是 `\"ios\"`，与 `\"macos\"` 互斥。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "Platform::Win",
        1,
        "`Platform::current()` 的 `cfg!(target_os = \"windows\")` 分支体。iOS 走不到（互斥）。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "Platform::Android",
        1,
        "`Platform::current()` 的 `cfg!(target_os = \"android\")` 分支体。iOS 走不到（互斥）。\
         本支必须排在 `linux` 之前 —— Rust 对 Android 的 `target_os` 是 `\"android\"` 而非 \
         `\"linux\"`，两条本就互斥，顺序在此无语义，但写成独立分支是为了让「Android 与 Linux \
         是两个答案」在产地就成立。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "Platform::Ios",
        1,
        "🔵 `Platform::current()` 的 `cfg!(target_os = \"ios\")` 分支体 —— **iOS 在枚举轴上的唯一产地**。\
         少了它，本批那 40 处穷举 `match` 的 iOS 答案一处都走不到（全落 `Other`），\
         那正是 K6b 的成因形态。\n\
         **本仓今天构不出 iOS 产物 ⇒ 这一支一次都不会被求值**，`cfg!` 是编译期常量折叠，\
         三种宿主构建下恒 false。它今天的价值是「答案已经在那里」，不是「答案在跑」——\
         这一点**没有任何门能证明**（构不出产物就没有可观测的事实），故它写在代码里、\
         也写在这里，而不是被伪装成一条测得出来的断言。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "Platform::Linux",
        1,
        "`Platform::current()` 的 `cfg!(target_os = \"linux\")` 分支体。iOS 走不到（互斥）。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "Platform::Other",
        1,
        "`Platform::current()` 的 `else` 兜底体（freebsd/openbsd/…）。iOS **不再**落在这里\
         —— 那正是本批加 `Ios` 具名变体要改掉的事；`Other` 从此只表示「本仓没为这个平台答过题」。",
    ),
    // ── 2026-09-25 基线合并（origin/main N1–N3 网络场景）带进来的两处比较型分派 ──
    // 同批带进来的第三处 `dhcp_privileged`（`matches!(platform, Win | Mac) || tun`）已改成穷举
    // `match` 交给编译器守，不进本表（Android/iOS 的答案写在那个函数的文档里）。
    (
        "crates/config-engine/src/builder/network_env.rs",
        "else if platform == Platform::Win && cidrs.is_empty()",
        1,
        "`resolve_probe_source` 里「system 探测源在 Windows 上拿不到搜索域」那条 Win 专属限制         （Windows 系统 API 只给 DNS 服务器地址时无法按搜索域判场景）。Android/iOS → false ⇒ 不报         `SystemNoSearchDomain`，落到 `System`。这一格对移动端的答案是「那条限制是 Windows 的，         与本平台无关」，仅此而已；**system 探测源在 Android/iOS 上是否真能拿到当前网络的 DNS 服务器         与搜索域（libbox 平台接口的供给面）未经真机核验** —— 网络场景面板本身尚未接到移动端         （移动端接线债），核验随那一批做。",
    ),
    (
        "src-tauri/src/runtime/proxy/dns_takeover.rs",
        "platform == polaris_helper_proto::Platform::Mac && is_tun && takeover != Some(false)",
        1,
        "`system_dns_takeover_active`（网络场景 auto 探测源用的「macOS + TUN + 接管系统 DNS 生效」事实）。         Android/iOS → false，正确：与 `dns_ops.rs::takeover_supported`（Android → false）同一个事实 ——         移动端没有可接管的系统解析器（DNS 由核在 tun fd 内自理）。调用方传入的 `is_tun` 已取本平台         生效值（`effective_on`，移动端恒 Tun），但平台合取项先把移动端排除，故生效值不改变这里的答案。",
    ),
    (
        "src-tauri/src/commands/misc/backup.rs",
        "if platform == polaris_helper_proto::Platform::Android",
        1,
        "`import_interface_names_from` 在 Android 上返回 None：备份导入时不向系统枚举网卡，\
         因而保留原有接口绑定；其他平台仍尝试枚举。Android 分支是独立平台行为，新增平台\
         不能默默继承枚举一侧，须在这里重新核对。",
    ),
    // 2026-10-01：对齐出生/退出托管与 Android 调试桥的真实新增点。
    (
        "src-tauri/src/runtime/helper.rs",
        "else if matches!(client.platform(), Platform::Mac | Platform::Win)",
        1,
        "出生状态查询的 Mac/Win 分类器只认 NativeBirth；Android/iOS/Other 不进此臂。它们没有桌面 helper，实现不得借 Linux 或 native 凭据兜底。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if !matches!(self.platform, Platform::Mac | Platform::Win)",
        1,
        "native birth 能力准入在非 Mac/Win 直接返回；Android/iOS/Other 的 supported 为 false，主链不会因此获得启动许可，未来新平台须实现自己的能力门。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if client.platform() != Platform::Linux",
        1,
        "Linux birth 能力探测在非 Linux 不发送 Linux 专属请求；Mac/Win 另问 native birth，Android/iOS/Other 无 helper，不能靠此提前返回取得能力。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if client.platform() == Platform::Linux",
        1,
        "托管状态的 Linux 响应须提供 LinuxStartBirth；Android/iOS/Other 不进此臂，末端 Unsupported 不接受遗留 PID。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if matches!(client.platform(), Platform::Mac | Platform::Win)",
        1,
        "native stop 的 Mac/Win 响应只结算原生精确出生凭据；Android/iOS/Other 不进此臂，无此平台的 helper 停核协议。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if matches!(platform, Platform::Linux | Platform::Mac | Platform::Win) && error.code == polaris_helper_proto::ErrorCode::Unknown",
        1,
        "Unknown 只在现有三个桌面 helper 上转成协议升级提示；Android/iOS/Other 仍保留原错误，不能假称安装新 helper 就能支持其运行模型。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if matches!(self.platform, Platform::Mac | Platform::Win)",
        1,
        "native birth 状态请求只由 Mac/Win 发送；Android/iOS/Other 使用不了该请求，必须由其原有平台桥管理。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if self.platform != Platform::Linux",
        1,
        "新增 require_linux_birth_capability 的非 Linux 提前返回只免掉 Linux 专属询问；Android/iOS/Other 不支持 helper，准入仍由 supported 和平台启动分派限制。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if self.platform == Platform::Linux",
        1,
        "Linux 启动响应只进 LinuxBirth 分类器；Mac/Win 使用下一条 NativeBirth 分类，Android/iOS/Other 不能被降级为遗留启动凭据。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "if self.platform == Platform::Linux && matches!(target, HelperStopTarget::Legacy(_))",
        1,
        "Linux 遗留 PID 停核目标被拒绝，强制使用精确出生身份；Android/iOS/Other 不进此拒绝臂，但它们也无 helper 停核实现，不能因此获准停核。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "let response = client .send(&if client.platform() == Platform::Linux",
        1,
        "托管状态请求在 Linux 取 LinuxBirth，Mac/Win 取 NativeBirth，Android/iOS/Other 在发送前明确 Unsupported；新平台不继承桌面请求。",
    ),
    (
        "src-tauri/src/runtime/helper.rs",
        "let response = if matches!(client.platform(), Platform::Mac | Platform::Win)",
        1,
        "Mac/Win 停核用 StopNativeBirth，其余臂显式区分 Linux 并拒绝移动端/Other；不是向新平台发送旧 Stop(PID) 请求。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "if helper.platform() == Platform::Linux",
        1,
        "启动前 Linux helper 自证只询问 LinuxBirth 能力；Mac/Win 询问 NativeBirth。Android/iOS/Other 没有 helper 路径，不能用返回 Ok 代替平台准入。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "if matches!(self.helper.platform(), Platform::Mac | Platform::Win)",
        1,
        "接收 helper 启动凭据前 Mac/Win 必须声明 NativeBirth；Linux 另有 LinuxBirth。Android/iOS/Other 不会进入此桌面 helper 启动路径。",
    ),

];

// ===================== 判据本体 =====================

#[test]
fn platform_variants_stay_in_sync_with_all() {
    let src = std::fs::read_to_string(repo_root().join("crates/helper-proto/src/lib.rs"))
        .expect("读不到 helper-proto/src/lib.rs —— 取材面本身塌了");
    let code = mask(&src);
    let declared = parse_platform_variants(&code);

    // 正面断言：真的抠出了变体，而不是解析失败后拿空集去比空集。
    assert!(
        declared.len() >= 4,
        "从 `enum Platform` 抠出的变体只有 {} 个（{declared:?}）—— 解析口径坏了，\
         而不是枚举真的只剩这么点",
        declared.len()
    );

    let known: Vec<String> = Platform::ALL.iter().map(|p| format!("{p:?}")).collect();
    assert_eq!(
        declared, known,
        "`enum Platform` 的变体与 `Platform::ALL` 不一致。\n\
         新增变体时**两处都要改**：ALL 是本仓所有全变体断言（含本门）的取材面，漏改它\
         等于新变体不受任何全变体判据管辖。\n  声明={declared:?}\n  ALL={known:?}"
    );
}

#[test]
fn platform_matches_have_no_bare_wildcard_arm() {
    let files = production_sources();
    assert!(
        files.len() >= 100,
        "只扫到 {} 个生产源文件 —— 取材面塌了",
        files.len()
    );

    let mut blocks = 0usize;
    let mut violations: Vec<String> = Vec::new();
    let mut allowed_hit: Vec<(&str, &str)> = Vec::new();

    for (rel, path) in &files {
        let src = std::fs::read_to_string(path).expect("读源文件失败");
        let code = mask(&src);
        for (start, _end, body) in platform_match_blocks(&code) {
            blocks += 1;
            let folded = body.split_whitespace().collect::<Vec<_>>().join(" ");
            for offset in top_level_wildcard_arms(body) {
                let line = code[..start + offset].matches('\n').count() + 1;
                // 放行的射程 = (路径, 块体原文) 这一对，不是整个文件。
                match WILDCARD_ALLOW
                    .iter()
                    .find(|(p, snippet, _)| *p == rel && fold(snippet) == folded)
                {
                    Some((p, snippet, _)) => allowed_hit.push((p, snippet)),
                    None => violations.push(format!("{rel}:{line}\n      {folded}")),
                }
            }
        }
    }

    // 切片自检：把取材面的规模打出来。扫到 0 个块也「没有违规」，那种绿没有信息量。
    println!(
        "[platform-dispatch] 生产源文件 {} 个，Platform 分派 match 块 {blocks} 个",
        files.len()
    );
    assert!(
        blocks >= MATCH_BLOCK_FLOOR,
        "只扫到 {blocks} 个 Platform 分派 match 块（下限 {MATCH_BLOCK_FLOOR}）——\
         取材面塌了（路径/掩码/括号配平任一坏掉都长这样），不是全仓真的只剩这么几处"
    );

    assert!(
        violations.is_empty(),
        "以下 `Platform` match 用了**裸通配臂** `_ =>`：\n  {}\n\n\
         裸通配臂让「新增平台变体」在编译期无声通过 —— 那正是 K6b 那次 Android 起停循环的\
         成因形态（Android 静默落进 Other，没有一处代码为它答过题）。\n\
         改法：把 `_` 展开成具名臂（`Platform::A | Platform::B => …`），语义逐值不变，\
         但下一个平台进来时编译器会在这里拦住你。\n\
         确有必要保留通配的，登记进 `WILDCARD_ALLOW` 并写明理由。",
        violations.join("\n  ")
    );

    // 反腐烂：白名单条目必须真的还命中**那一个块**。
    for (path, snippet, why) in WILDCARD_ALLOW {
        assert!(
            allowed_hit.contains(&(path, snippet)),
            "`WILDCARD_ALLOW` 里这条已经找不到对应的裸通配 `match` 了：\n  {path}\n  {snippet}\n\n\
             判据被改动或删除 ⇒ 重新审一次这个 match（改了臂就更新语句原文；真的不再需要通配\
             就删掉本条）。（原理由：{why}）"
        );
    }
}

/// 折叠空白：登记表里的语句原文按可读性换行，比对前一律折成单空格。
fn fold(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn comparison_style_platform_dispatch_is_registered() {
    let registered_total: usize = COMPARISON_REGISTRY.iter().map(|(_, _, n, _)| *n).sum();
    assert!(
        registered_total >= COMPARISON_FLOOR,
        "登记表只覆盖 {registered_total} 次出现（下限 {COMPARISON_FLOOR}）—— \
         有人把表清空了，而不是分派点真的消失了"
    );

    // 实到：(路径, 语句原文) → 出现次数。
    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (rel, path) in production_sources() {
        let src = std::fs::read_to_string(&path).expect("读源文件失败");
        let code = mask(&src);
        for snippet in comparison_snippets(&code) {
            *found.entry((rel.clone(), snippet)).or_default() += 1;
        }
    }

    // 登记：(路径, 语句原文) → 条数（同一条登记覆盖同文件同形态的全部出现）。
    let mut registered: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (path, snippet, count, _why) in COMPARISON_REGISTRY {
        *registered
            .entry(((*path).to_owned(), (*snippet).to_owned()))
            .or_default() += *count;
    }

    let unregistered: Vec<String> = found
        .iter()
        .filter(|(k, n)| registered.get(*k).copied().unwrap_or(0) < **n)
        .map(|((p, s), n)| {
            let r = registered
                .get(&(p.clone(), s.clone()))
                .copied()
                .unwrap_or(0);
            format!("{p}  （实到 {n} 次，登记 {r} 次）\n      {s}")
        })
        .collect();
    let rotten: Vec<String> = registered
        .iter()
        .filter(|(k, n)| found.get(*k).copied().unwrap_or(0) < **n)
        .map(|((p, s), n)| {
            let f = found.get(&(p.clone(), s.clone())).copied().unwrap_or(0);
            format!("{p}  （登记 {n} 次，实到 {f} 次）\n      {s}")
        })
        .collect();

    let found_total: usize = found.values().sum();
    println!(
        "[platform-dispatch] 比较型分派点：实到 {found_total} 次 / {} 种形态；登记 {registered_total} 次 / {} 种形态",
        found.len(),
        registered.len()
    );
    assert!(
        found_total >= COMPARISON_FLOOR,
        "只扫到 {found_total} 次比较型分派（下限 {COMPARISON_FLOOR}）—— 取材面塌了"
    );

    assert!(
        unregistered.is_empty(),
        "以下**比较型** `Platform` 分派点没有登记：\n    {}\n\n\
         编译器对 `== / != / matches!` 形态的分派**一句话都不说** —— 加变体不会让它们变红，\
         新平台会静默走进某一侧。故每一处都必须在 `COMPARISON_REGISTRY` 里写下\
         「新平台（当下是 Android）在这里得到什么答案、为什么那个答案可以」。\n\
         若这处判据能改成穷举 `match`，优先改 —— 那样编译器就替你守了，表里也不必再有它。",
        unregistered.join("\n    ")
    );
    assert!(
        rotten.is_empty(),
        "以下登记条目在源码里已经找不到（判据被改动或删除）：\n    {}\n\n\
         登记表不是垃圾桶：判据变了就要重新答一次「新平台在这里得到什么」，\
         然后把这一条更新成新的语句原文。",
        rotten.join("\n    ")
    );
}

// ===================== 取材面 =====================

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri 必有上级目录")
        .to_path_buf()
}

/// 生产源码面：`src-tauri/src/**` + `crates/*/src/**`，排除测试模块。
///
/// 排除 `tests/` 目录与 `tests.rs`：测试里用通配臂、用比较，都不是分派点（它们**构造**
/// `Platform` 值而不是按它分派）。把它们算进来只会让登记表被测试改动搅动。
fn production_sources() -> Vec<(String, PathBuf)> {
    let root = repo_root();
    let mut out = Vec::new();
    let mut roots = vec![root.join("src-tauri/src")];
    let mut crate_dirs: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("crates/ 必须存在")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    crate_dirs.sort();
    roots.extend(crate_dirs.into_iter().map(|d| d.join("src")));

    for r in roots {
        walk(&r, &mut out);
    }
    out.sort();
    out.into_iter()
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .expect("扫描面必在仓内")
                .to_string_lossy()
                .replace('\\', "/");
            (rel, p)
        })
        .filter(|(rel, _)| !rel.contains("/tests/") && !rel.ends_with("/tests.rs"))
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 把注释（行/块/doc）与字符串、字符字面量抹成空格，保留换行与字节偏移。
///
/// **为什么必须剥**：本仓的注释密度极高，`Platform::Other` 在注释里出现的次数比在代码里还多
/// （本文件自己就是最好的例子）。不剥就会把注释里的举例当成分派点，而这类假阳性会训练人去
/// 改注释迁就门 —— 门就此失去意义。
fn mask(src: &str) -> String {
    resolve_self_in_platform_impls(&mask_code(src, true))
}

/// 在 `impl … Platform` 块**内部**把 `Self::` 归一成 `Platform::`。
///
/// # 这条归一化补的是哪个洞（2026-09-06，实测存在于生产代码里）
///
/// `helper-proto` 的 `Platform::has_token_line` 写作 `matches!(self, Self::Mac | Self::Win)` ——
/// `Self::` 而不是 `Platform::`。[`find_variant`] 按 `Platform::<Variant>` 取材 ⇒ **扫不到它**：
/// 那是一处真实的比较型分派点，加平台变体时既不会编译错（`matches!` 不是穷举 `match`），
/// 也不会有任何门变红。上一批把它如实登记进了函数注释，但注释没有牙。
///
/// # 为什么不是「把 `Self::` 也当成 `Platform::`」那种一刀切
///
/// 那会引入**真实的**假阳性：`crates/updater/src/github.rs` 的 `AssetPlatform` 有
/// `Self::Linux` / `Self::Other`，`crates/helper-proto/src/error.rs`、
/// `crates/system-integration/src/error.rs`、`src-tauri/src/commands/server.rs` 各有别的枚举的
/// `Self::Other`（2026-09-06 逐处复核，上一批给的这条理由**成立**）。
/// 扫描器无法从一个裸 `Self::Other` 文本判断 `Self` 是谁。
///
/// 收法因此是**按所在 `impl` 块的自身类型限定**：只有当块头的 self 类型逐字是 `Platform` 时，
/// 块体里的 `Self::` 才被读成 `Platform::`。上面那四处分别在 `impl AssetPlatform` /
/// `impl ErrorKind` 等块里，一处都不会被改写 —— 这一点由
/// [`self_alias_does_not_leak_into_other_enums`] 用真实源码正反双向钉住。
///
/// # 两个边界（说清楚，免得下次有人以为它比实际更强）
///
/// - 只认块头**最后一个路径段**逐字为 `Platform` 的 impl（`impl Platform`、`impl X for Platform`）。
///   将来若出现 `impl polaris_helper_proto::Platform`，末段仍是 `Platform`，仍认得。
/// - 不做嵌套 impl 的类型作用域分析：`impl Platform` 块里再嵌一个别的类型的 `impl`，
///   其块体也会被改写。今天仓里没有这种形态（`impl Platform` 只有 helper-proto 那一处），
///   真出现时症状是**多扫**（多一条要登记的分派点），不是漏扫 —— 失效方向是安全的那一侧。
///
/// # 偏移与行号
///
/// 改写把 6 字节的 `Self::` 换成 10 字节的 `Platform::`，**字节偏移会变**，故本函数必须在
/// [`mask_code`] 之后、任何取材之前跑一次，此后全流程只看返回的这一份。换行一个不增不减，
/// 因此「第几行」仍与原文件一致（违规消息里的行号照样能用）。
///
/// 登记表里的语句原文因此写的是**归一化后**的形态（`Platform::Mac`），源码里可能是 `Self::Mac`。
/// 反腐烂那一半不受影响：源码一改，归一化结果跟着改。
///
/// # 只在 [`mask`] 上做，不在 [`mask_comments`] 上做
///
/// 字符串轴判的是字符串字面量落没落在分派位，与 `Self::` 这个**类型路径**写法无关；
/// 在那一侧改写只会让 `STRING_DISPATCH_REGISTRY` 里 `Platform::parse` 那条的语句原文平白变形。
fn resolve_self_in_platform_impls(code: &str) -> String {
    let mut out = String::with_capacity(code.len());
    let mut cursor = 0usize;
    for (open, close) in platform_impl_bodies(code) {
        out.push_str(&code[cursor..open]);
        out.push_str(&code[open..=close].replace("Self::", "Platform::"));
        cursor = close + 1;
    }
    out.push_str(&code[cursor..]);
    out
}

/// 已掩码代码面里，所有「self 类型是 `Platform`」的 `impl` 块体：(`{` 下标, `}` 下标)，按序不重叠。
fn platform_impl_bodies(code: &str) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = code.get(i..).and_then(|s| s.find("impl")) {
        let at = i + rel;
        i = at + 4;
        let pre_ok = at == 0 || !code[..at].chars().next_back().is_some_and(is_ident_char);
        let post_ok = !code[at + 4..].chars().next().is_some_and(is_ident_char);
        if !(pre_ok && post_ok) {
            continue;
        }
        let Some(open) = code[at..].find('{').map(|o| at + o) else {
            continue;
        };
        // 块头里出现 `;` 说明中间隔了别的语句，那个 `{` 不属于这个 `impl`。
        if code[at..open].contains(';') {
            continue;
        }
        let Some(close) = balanced_end(code, open) else {
            continue;
        };
        if impl_self_type(&code[at + 4..open]) == "Platform" {
            // 嵌套：外层已收进来的块不再重复收（`replace` 已经覆盖整块）。
            if !out.iter().any(|(a, b)| *a <= at && close <= *b) {
                out.push((open, close));
            }
        }
    }
    out
}

/// `impl` 与块体 `{` 之间那段文本 → self 类型的**末路径段**。
///
/// `impl X for Y` 取 `Y`（`for` 之后），否则取 `impl` 之后的第一段。泛型参数、`where` 子句、
/// 类型实参一律剥掉：判的只是「这个 impl 的 `Self` 是谁」。
fn impl_self_type(header: &str) -> String {
    let ty = match header.rfind(" for ") {
        Some(at) => &header[at + 5..],
        None => header,
    };
    // `where` 子句在类型之后，先切掉。
    let ty = ty.split(" where ").next().unwrap_or(ty);
    // 泛型实参 / 生命周期：取第一个 `<` 之前。
    let ty = ty.split('<').next().unwrap_or(ty).trim();
    ty.rsplit("::").next().unwrap_or(ty).trim().to_owned()
}

/// 归一化**只**发生在 `impl … Platform` 块内 —— 别的枚举的 `Self::Linux` / `Self::Other` 不许被改写。
///
/// 正反两半都用**真实源码**，不用自撰夹具：假阳性那一半的四个样本是本仓今天真有的四处
/// （上一批「不改扫描器」的理由就建在它们身上），自撰夹具证明不了「今天的仓里没被误伤」。
#[test]
fn self_alias_does_not_leak_into_other_enums() {
    // ── 正面：`impl Platform` 里的 `Self::` 真的被改写了 ──
    //
    // 断言的是**性质**（这一面上不再有 `Self::` 形态的 Platform 变体），不是某一句判据的原文：
    // 钉原文等于把 `has_token_line` 的判据在这里再抄一份，判据一改这里就红，而那是登记表的活。
    let proto = std::fs::read_to_string(repo_root().join("crates/helper-proto/src/lib.rs"))
        .expect("读不到 helper-proto/src/lib.rs");
    // 取材面自检：源文件里必须真有 `Self::<Variant>` 这种写法，否则下面那条断言在验一件不存在的事。
    assert!(
        VARIANT_NAMES
            .iter()
            .any(|v| mask_comments(&proto).contains(&format!("Self::{v}"))),
        "helper-proto 里已经没有 `Self::<Variant>` 写法了 —— 本条正面对照失去样本；\
         若真的全改成了 `Platform::`，这条归一化可以删，但要连同本测一起删"
    );
    let code = mask(&proto);
    for v in VARIANT_NAMES {
        assert!(
            !code.contains(&format!("Self::{v}")),
            "`impl Platform` 里的 `Self::{v}` 没有被归一化 —— 那一处比较型分派点仍然对本门隐形\
             （`has_token_line` 正是这条归一化要收的洞）"
        );
    }
    assert!(
        find_variant(&code, "Ios", 0).is_some(),
        "归一化后 helper-proto 里一处 `Platform::Ios` 都找不到 —— 取材面塌了"
    );

    // ── 反面：别的枚举一处都不许被改写（四处真实样本，逐个查）──
    for (rel, needle) in [
        ("crates/updater/src/github.rs", "Some(Self::Linux)"),
        ("crates/helper-proto/src/error.rs", "_ => Self::Other,"),
        ("crates/system-integration/src/error.rs", "Self::Other("),
        ("src-tauri/src/commands/server.rs", "Self::Other(message)"),
    ] {
        let src = std::fs::read_to_string(repo_root().join(rel))
            .unwrap_or_else(|e| panic!("读不到 {rel}：{e}"));
        // 取材面自检：样本必须真的还在原文里，否则下面那条断言是在验一件不存在的事。
        assert!(
            src.contains(needle),
            "{rel} 里已经没有 {needle:?} 了 —— 本条反向对照失去样本，换一个真实样本，\
             不要改成自撰夹具"
        );
        let masked = mask(&src);
        assert!(
            masked.contains(needle),
            "{rel} 的 {needle:?} 被归一化改写了 —— 那是**别的枚举**的 `Self::`，\
             扫描器会把它当成 `Platform` 的分派点（这正是上一批不改扫描器时给的理由）"
        );
    }
}

/// 只剥注释、**保留字符串字面量**。
///
/// 字符串平台面（[`string_platform_dispatch_is_registered`]）判的就是 `platform == "android"`
/// 这种形态，被判据物本身是字符串字面量 —— 用 [`mask`] 会把要找的东西一起抹掉。
///
/// 少剥一层就要多守一层：本函数保留的字符串里可能有日志/错误文案含平台名，故那条判据**不看
/// 字面量出现，只看它是否落在分派位**（`== / != / => / |` 或 `eq_ignore_ascii_case(` 等的紧邻处），
/// 见 [`string_dispatch_snippets`]。注释仍然照剥 —— 本仓真出过「判据被同文件的注释喂饱」。
fn mask_comments(src: &str) -> String {
    mask_code(src, false)
}

/// [`mask`] / [`mask_comments`] 的共同实现。
///
/// `strip_strings=false` 时仍然**完整解析**字符串与字符字面量（只是原样留下），这一步不能省：
/// 一个 `"// 不是注释"` 里的 `//` 若被当成注释起点，后面整行代码都会被抹掉，判据于是漏扫。
fn mask_code(src: &str, strip_strings: bool) -> String {
    let c: Vec<char> = src.chars().collect();
    let n = c.len();
    let mut out: Vec<char> = vec![' '; n];
    let mut i = 0usize;

    let blank = |out: &mut [char], c: &[char], a: usize, b: usize| {
        for (k, slot) in out.iter_mut().enumerate().take(b).skip(a) {
            *slot = if c[k] == '\n' { '\n' } else { ' ' };
        }
    };
    let keep = |out: &mut [char], c: &[char], a: usize, b: usize| {
        for (k, slot) in out.iter_mut().enumerate().take(b).skip(a) {
            *slot = c[k];
        }
    };

    while i < n {
        let ch = c[i];
        // 原始字符串 r"…" / r#"…"# / br##"…"##
        if (ch == 'r' || ch == 'b') && (i == 0 || !is_ident_char(c[i - 1])) {
            let mut j = i;
            if c[j] == 'b' {
                j += 1;
            }
            if j < n && c[j] == 'r' {
                j += 1;
                let hs = j;
                while j < n && c[j] == '#' {
                    j += 1;
                }
                if j < n && c[j] == '"' {
                    let hashes = j - hs;
                    let mut k = j + 1;
                    let end = loop {
                        if k >= n {
                            break n;
                        }
                        if c[k] == '"' {
                            let mut h = 0;
                            while h < hashes && k + 1 + h < n && c[k + 1 + h] == '#' {
                                h += 1;
                            }
                            if h == hashes {
                                break k + 1 + hashes;
                            }
                        }
                        k += 1;
                    };
                    if strip_strings {
                        blank(&mut out, &c, i, end);
                    } else {
                        keep(&mut out, &c, i, end);
                    }
                    i = end;
                    continue;
                }
            }
        }
        if ch == '/' && i + 1 < n && c[i + 1] == '/' {
            let mut j = i;
            while j < n && c[j] != '\n' {
                j += 1;
            }
            blank(&mut out, &c, i, j);
            i = j;
            continue;
        }
        if ch == '/' && i + 1 < n && c[i + 1] == '*' {
            let mut j = i + 2;
            let mut depth = 1usize;
            while j < n && depth > 0 {
                if c[j] == '/' && j + 1 < n && c[j + 1] == '*' {
                    depth += 1;
                    j += 2;
                } else if c[j] == '*' && j + 1 < n && c[j + 1] == '/' {
                    depth -= 1;
                    j += 2;
                } else {
                    j += 1;
                }
            }
            blank(&mut out, &c, i, j);
            i = j;
            continue;
        }
        if ch == '"' {
            let mut j = i + 1;
            while j < n {
                if c[j] == '\\' {
                    j += 2;
                    continue;
                }
                if c[j] == '"' {
                    j += 1;
                    break;
                }
                j += 1;
            }
            if strip_strings {
                blank(&mut out, &c, i, j.min(n));
            } else {
                keep(&mut out, &c, i, j.min(n));
            }
            i = j.min(n);
            continue;
        }
        if ch == '\'' {
            // 字符字面量 vs 生命周期：只有 '\x' 与 'x' 两种形态是字面量。
            if i + 1 < n && c[i + 1] == '\\' {
                let mut k = i + 2;
                while k < n && c[k] != '\'' {
                    k += 1;
                }
                let end = (k + 1).min(n);
                if strip_strings {
                    blank(&mut out, &c, i, end);
                } else {
                    keep(&mut out, &c, i, end);
                }
                i = end;
                continue;
            }
            if i + 2 < n && c[i + 2] == '\'' {
                if strip_strings {
                    blank(&mut out, &c, i, i + 3);
                } else {
                    keep(&mut out, &c, i, i + 3);
                }
                i += 3;
                continue;
            }
        }
        out[i] = ch;
        i += 1;
    }
    out.into_iter().collect()
}

const fn is_ident_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

const fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// 从**已掩码**的 helper-proto 源码里抠 `enum Platform { … }` 的变体名（按声明顺序）。
fn parse_platform_variants(code: &str) -> Vec<String> {
    let Some(at) = code.find("enum Platform") else {
        panic!("找不到 `enum Platform` 声明 —— 取材面塌了");
    };
    let Some(open) = code[at..].find('{').map(|o| at + o) else {
        panic!("`enum Platform` 后没有 `{{`");
    };
    let close = balanced_end(code, open).expect("`enum Platform` 括号不配平");
    code[open + 1..close]
        .split(',')
        .filter_map(|seg| {
            let name: String = seg
                .trim()
                .chars()
                .take_while(|ch| is_ident_char(*ch))
                .collect();
            (!name.is_empty()).then_some(name)
        })
        .collect()
}

/// `{` 起的配平右括号**字节**下标。
///
/// # 为什么全程按字节而不是按 `char`
///
/// 本文件其余取材口（`str::find`、`statement_span`）给出的都是字节偏移。混用字节偏移与
/// `Vec<char>` 下标在纯 ASCII 上**恰好相等**，一旦被扫描的代码面里出现一个多字节字符就整体错位
/// —— 而本仓注释里全是中文。掩码后代码面通常已是纯 ASCII（注释被抹成空格），但那是**巧合**
/// 而不是保证，判据不该建在巧合上。
///
/// 按字节扫 `{`/`}` 是安全的：UTF-8 的多字节序列里不会出现 ASCII 字节，故不会误命中；
/// 返回的下标落在 ASCII 字符上，因此也是合法的切片边界。
fn balanced_end(code: &str, open: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut depth = 0i32;
    for (k, byte) in bytes.iter().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// `Platform::<Variant>` 的词边界查找（前后都不是标识符字符）。
///
/// **词边界不能省**：`InstallPlatform::Macos` 里含子串 `Platform::Mac`，不看边界就会把一个
/// 与 `Platform` 毫无关系的 `match` 当成分派点（实测：`runtime/update_install.rs` 因此被误报）。
fn find_variant(code: &str, variant: &str, from: usize) -> Option<usize> {
    let token = format!("Platform::{variant}");
    let mut i = from;
    while let Some(rel) = code.get(i..).and_then(|s| s.find(&token)) {
        let at = i + rel;
        let end = at + token.len();
        let pre_ok = at == 0 || !code[..at].chars().next_back().is_some_and(is_ident_char);
        let post_ok = !code[end..].chars().next().is_some_and(is_ident_char);
        if pre_ok && post_ok {
            return Some(at);
        }
        i = at + 1;
    }
    None
}

/// 扫描器认得的变体名清单 —— **本门的取材面本身**。
///
/// # 🔴 这份清单必须与 `Platform::ALL` 逐名同步，而这条同步由 [`variant_names_cover_every_variant`] 守
///
/// 2026-09-06 实测的缺口：加 `Platform::Ios` 那一批把 40 处穷举 `match` 全答了，却没有人告诉
/// 本门「多了一个变体名」。后果是**只提 `Platform::Ios` 的分派点对整道登记门完全隐形** ——
/// [`find_variant`] 按 `Platform::<Variant>` 逐名找，名字不在这张表里就一次都不会被找。
/// 那正是本门自己头注里写的那种失效：**加了平台，却没告诉守平台的那道门**。
///
/// 手写这张表是刻意的（`Platform::ALL` 在**被测的那一侧**，直接 `map(|p| format!("{p:?}"))`
/// 就会让门跟着被测物一起漂），代价是它会腐烂 —— 故必须有一条判据把它钉在 `ALL` 上。
const VARIANT_NAMES: &[&str] = &["Mac", "Win", "Linux", "Android", "Ios", "Other"];

/// [`VARIANT_NAMES`] 必须逐名等于 `Platform::ALL` 的 `Debug` 名。
///
/// 没有这一条，上面那张手写表就是又一份「加变体时会漏改的清单」，而它漏改的后果是
/// **本门的三条判据同时静默缩小取材面**（match 块少认、比较型少认、`body_mentions_variant`
/// 少认），却仍然全绿 —— 门还在，牙没了。
#[test]
fn variant_names_cover_every_variant() {
    let all: Vec<String> = Platform::ALL.iter().map(|p| format!("{p:?}")).collect();
    let names: Vec<String> = VARIANT_NAMES.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(
        names, all,
        "扫描器的 `VARIANT_NAMES` 与 `Platform::ALL` 不一致。\n\
         这张表是本门**全部三条判据的取材面**：少一个名字，只提该变体的 `match` 块与比较型\n\
         分派点就对整道门隐形（加了平台却没告诉守平台的那道门 —— 2026-09-06 `Ios` 的实况）。\n\
         顺序也要一致：两边都按声明序，差异一眼可读。\n  VARIANT_NAMES={names:?}\n  ALL={all:?}"
    );
}

fn body_mentions_variant(body: &str) -> bool {
    VARIANT_NAMES
        .iter()
        .any(|v| find_variant(body, v, 0).is_some())
}

/// 所有「arm 里出现 `Platform::<Variant>`」的 `match` 块：(块体起始下标, 结束下标, 块体)。
fn platform_match_blocks(code: &str) -> Vec<(usize, usize, &str)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = code.get(i..).and_then(|s| s.find("match")) {
        let at = i + rel;
        let pre_ok = at == 0 || !code[..at].chars().next_back().is_some_and(is_ident_char);
        let post_ok = !code[at + 5..].chars().next().is_some_and(is_ident_char);
        if !(pre_ok && post_ok) {
            i = at + 5;
            continue;
        }
        let Some(open) = code[at..].find('{').map(|o| at + o) else {
            break;
        };
        let Some(close) = balanced_end(code, open) else {
            break;
        };
        let body = &code[open..=close];
        if body_mentions_variant(body) {
            out.push((open, close + 1, body));
        }
        i = at + 5;
    }
    out
}

/// 块体（以 `{` 开头）里**顶层**的裸通配臂偏移。
///
/// 顶层 = 相对块体的花括号深度 1 且圆/方括号深度 0。这两条一起才能把嵌套 `match`
/// （如 `proxy_ops/ops.rs` 的 `match (from, to, current)`）与元组模式里的 `_` 排除掉 ——
/// 那些 `_` 匹配的不是 `Platform`。
fn top_level_wildcard_arms(body: &str) -> Vec<usize> {
    // 按字节扫，理由同 [`balanced_end`]：与本文件其余取材口的偏移口径统一。
    let bytes = body.as_bytes();
    let mut hits = Vec::new();
    let (mut brace, mut paren) = (0i32, 0i32);
    for (i, byte) in bytes.iter().enumerate() {
        match byte {
            b'{' => brace += 1,
            b'}' => brace -= 1,
            b'(' | b'[' => paren += 1,
            b')' | b']' => paren -= 1,
            b'_' if brace == 1 && paren == 0 => {
                let prev_ok = i == 0 || !is_ident_byte(bytes[i - 1]);
                let next_ok = i + 1 >= bytes.len() || !is_ident_byte(bytes[i + 1]);
                if prev_ok && next_ok {
                    let mut k = i + 1;
                    while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                        k += 1;
                    }
                    let is_arm = bytes.get(k) == Some(&b'|')
                        || (bytes.get(k) == Some(&b'=') && bytes.get(k + 1) == Some(&b'>'));
                    if is_arm {
                        hits.push(i);
                    }
                }
            }
            _ => {}
        }
    }
    hits
}

/// `match` 块之外的 `Platform::<Variant>` 出现，按**所在语句**合并、折叠空白。
///
/// 语句边界取「上一个 `;`/`{`/`}` 到下一个 `;`/`{`/`}`」：够粗但确定，且对排版免疫。
/// 同一条 `matches!(p, A | B | C)` 里的三次出现因此合成一条，而不是三条碎片。
fn comparison_snippets(code: &str) -> Vec<String> {
    let covered: Vec<(usize, usize)> = platform_match_blocks(code)
        .into_iter()
        .map(|(a, b, _)| (a, b))
        .collect();

    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0usize;
    loop {
        let next = VARIANT_NAMES
            .iter()
            .filter_map(|v| find_variant(code, v, i))
            .min();
        let Some(at) = next else { break };
        if !covered.iter().any(|(a, b)| *a <= at && at < *b) {
            let span = statement_span(code, at);
            if !spans.contains(&span) {
                spans.push(span);
            }
        }
        i = at + 1;
    }
    spans
        .into_iter()
        .map(|(a, b)| code[a..b].split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

fn statement_span(code: &str, pos: usize) -> (usize, usize) {
    let bytes = code.as_bytes();
    let mut a = pos;
    while a > 0 && !matches!(bytes[a - 1], b';' | b'{' | b'}') {
        a -= 1;
    }
    let mut b = pos;
    while b < bytes.len() && !matches!(bytes[b], b';' | b'{' | b'}') {
        b += 1;
    }
    (a, b)
}

// ═══════════════════════════════════════════════════════════════════════════
// 字符串平台面（2026-09-05 K11）
//
// # 为什么枚举面守完了还要这一面
//
// K10 那道穷尽性门守的是 **`Platform` 枚举**：新增变体时 `match` 缺臂即编译不过。但仓里还有一批
// 按**字符串**判平台的分派点（`deps.platform == "android"`、`match os { "macos" => …, _ => … }`），
// 它们的兜底臂对编译器完全透明 —— 新平台接进来时，这些点静默走 `_ =>` 或 `else`，编译器一句话
// 都不说，与 K6b 那次「Android 静默落进 `Other`」是同一种失效。
//
// 真缺陷回放（本门的历史样本，2026-09-04 模拟器实证）：`config-engine/builder/inbounds.rs` 的
// `route_exclude_address` 曾是 `if win32 {…} else if linux {…} else { 回环排除 }`。Android 落进
// 最后那个 `else` 拿到 `127.0.0.0/8`，而 `VpnService.Builder.excludeRoute()` **拒收回环前缀**，
// Builder 的语义是任何一条被拒就整个 `establish()` 失败 ⇒ `configure tun interface: Bad address`
// ⇒ **隧道一次都建不起来**。修法是补一条具名的 `else if deps.platform == "android"` 空集臂。
// 本门的反腐烂那一半正是钉住这类臂：把它删掉（回到缺陷态）⇒ 登记条目找不到 ⇒ 红。
//
// # 取材面：意图面之外的那一圈，怎么剥
//
// 本面**只剥注释、保留字符串**（[`mask_comments`]）—— 被判据物本身就是字符串字面量，用剥字符串
// 的 [`mask`] 会把要找的东西一起抹掉。少剥一层就多守一层：判据不看「字面量出现过」，只看它是否
// 落在**分派位**上（紧邻 `== / != / => / |`，或 `eq_ignore_ascii_case(` 这类比较调用的实参位），
// 于是日志文案里的 "android"、错误消息里的 "linux" 一个都不会进来。注释照剥 —— 本仓真出过
// 「判据被同文件的注释喂饱」。
//
// **平台名字面量集合是派生的，不是手写的**：从 `helper-proto` 的 `Platform::parse` 里抠。
// 那个函数是字符串轴与枚举轴唯一的桥；给 `parse` 加一个新平台名，本门的取材面自动跟着变宽，
// 不需要有人记得回来改这里。
//
// # 不在本面之内的（显式声明边界，否则「扫到 0 个也叫全绿」）
//
// - `#[cfg(target_os = "…")]` / `cfg!(target_os = "…")`：那是**编译期**平台轴，由
//   `scripts/gate-rust.sh` 的三目标 cross-clippy 与 `tests/cross_target_coverage.rs` 守。
//   形态上也区分得开：`cfg` 用的是单个 `=`，不落在本门的分派位判据里。
// - 测试代码（`production_sources` 已排除）：测试里按平台串分叉是**构造输入**，不是分派点。
// ═══════════════════════════════════════════════════════════════════════════

/// 从 `Platform::parse` 抠出的平台名字面量条数下限（正面断言）。
///
/// 2026-09-05 实测 6 条（darwin/macos/win32/windows/linux/android）。取 5 是给别名增删留余量，
/// 但不给「抠取口径坏掉」留余量 —— 那种情形是 0 或 1。
const PLATFORM_NAME_FLOOR: usize = 5;

/// 字符串型平台分派点（按语句折叠后）条数下限（正面断言，同 [`MATCH_BLOCK_FLOOR`] 之理）。
/// 2026-09-05 实测 42 条（K12 给 `inbounds.rs` 的 `strict_route` 加了一条具名 Android 臂）。
const STRING_DISPATCH_FLOOR: usize = 30;

/// 字符串型平台分派点登记表。
///
/// 每条 = (仓库相对路径, 折叠空白后的语句原文, 该形态在该文件里的出现次数, **本仓没为它答过题的
/// 那个平台在这里得到什么、以及为什么那个答案可以**)。
///
/// 强制力与 [`COMPARISON_REGISTRY`] 同构（新增即红 / 腐烂即红 / 内容不可自证），此处不复述；
/// 差别只有一条：那张表问「Android 在这里得到什么」，本表问的是**「一个本仓从未构建过的平台
/// 在这里得到什么」** —— 因为字符串轴上没有 `Platform::Other` 那样的具名落点，未知平台一律
/// 落在 `_ =>` / `else` 里，而那正是本门要让人看见一次的东西。
///
/// # 2026-09-06：iOS **就是**第三列问的那个平台，全表逐条复核过一遍
///
/// 加 `Platform::Ios` 变体时，本表的第三列不需要重新推导 —— 它逐字就是 iOS 今天的答案。
/// 复核结论（42 条 / 40 种形态）：
///
/// | 处置 | 条数 | 是哪些 |
/// |---|---|---|
/// | **改了判据**（第三列的理由在 iOS 上不成立） | 4 | `inbounds.rs` 的 mixed 入站、回环排除（新开具名支）、`strict_route`；`tun_stack.rs` 的 `platform_default_stack`（**2026-09-06 补，见下**；2026-09-24 随上游整体移除 TUN stack，`tun_stack.rs` 连同两条登记一并撤下，默认 MTU 改为全平台 `DEFAULT_TUN_MTU`；2026-09-25 起缺席即不下发 `mtu`、交内核取默认，Polaris 不再持有平台 → MTU 表） |
/// | **改了写法、答案不变**（把兜底的 false 变成答过的 false） | 1 | `startup.rs` 的 `core_has_builtin_cronet` |
/// | **判据本体变了**（取材面的产地） | 1 | `helper-proto` 的 `Platform::parse` |
/// | **理由在 iOS 上仍成立，一个字没动** | 36 | 其余全部 |
///
/// 那 36 条为什么可以不动：它们的第三列理由分两类 —— ① 「这条机制是某平台独有的」
/// （Windows 注册表 / WFP 死环 / macOS utun 动态命名 / Linux gsettings…），iOS 与任何未知平台
/// 一样不具备，判 false 即正确；② 「上一层已经把这条腿关掉了」（mesh System 允许清单、
/// `takeover_supported`、`platform_supported`），而那些上层判据本批都为 iOS 显式答过题。
/// 两类都不依赖「未知平台是桌面」这个假设 —— 而那个假设正是被改掉的那几条所依赖的。
///
/// # 🔴 上面这张表**第一版是错的**，这条更正本身值得读（2026-09-06 复核）
///
/// 第一版写的是「改了判据 3 条 / 一个字没动 37 条」。`tun_stack.rs` 的 `platform_default_stack`
/// 被算进了那 37 条 —— 而它恰恰属于第一类：iOS 在那一格静默落 `_ => Gvisor`，**与 Android 反答**，
/// 而 Android 那一格是有具名臂的。「37 条理由仍成立」这句话在那一格上是假的。
///
/// 成因不是疏忽的等价物，是**取材方式**：那一轮是按「第三列写的是未知平台的答案，iOS 就是那个
/// 未知平台」逐条读理由，而 `platform_default_stack` 那条的第三列讲的是**兜底臂的取值依据**
/// （为什么未知平台该拿 gvisor），读起来完全成立 —— 成立的是「未知平台拿 gvisor」，不是
/// 「iOS 拿 gvisor」。两者在 iOS 具名之后就不是一回事了，而第三列不会自己提醒这一点。
///
/// 教训写在这里而不是删掉：凡是**同一张表里已经有别的移动平台具名臂**的那一格，
/// 「未知平台的答案仍成立」推不出「iOS 的答案仍成立」——那一格必须单独问一次
/// 「iOS 与 Android 在这里同答吗？不同答的理由是什么？」。
///
/// **一条点名的待验证项**（答案是安全侧，但依据不完整）：`outbound_helpers.rs` 的
/// `Some("apple") => platform == "darwin"` —— iOS 在字面上就是 Apple，却得到 `false`（不下发
/// utls 的 apple engine）。详见该条第三列。
type StringDispatch = (&'static str, &'static str, usize, &'static str);
const STRING_DISPATCH_REGISTRY: &[StringDispatch] = &[
    (
        "crates/updater/src/github.rs",
        "\"windows\" => Some(Self::Windows), \"macos\" => Some(Self::Macos), \"linux\" => Some(Self::Linux), \"android\" => Some(Self::Android), _ => None,",
        1,
        "未知平台 → `None` ⇒ 这一档**没有安装包资产**。正确，且这是唯一诚实的答案：资产名是\
         发布流水线按平台逐个约定出来的（`ANDROID_APK_SUFFIX` 那一族），本仓从没为这个平台\
         产出过任何包，猜一个名字只会让它去下载一个不存在的 URL。\n\
         `None` 不等于「没有更新」：`app_update.rs` 的 Android 分支在选不到资产时改问\
         `check_app_update_release_only`（只比版本、不选资产），所以未知平台仍答得出「有新版本」，\
         只是不画下载按钮。**这条分流本身就是 2026-09-14 接 Android 那一批补上的** —— 在那之前\
         `from_os` 返 `None` 会让整条检查腿早退成「已是最新」，一句话都不说。\n\
         iOS 尚未接：它进来时这里要么加一支（有 ipa 资产），要么继续落 `None`（走 TestFlight，\
         应用内本来就不该自己装包），两条路都要在这里显式表态，不许靠 `_ =>` 默认。",
    ),
    (
        "crates/config-engine/src/builder/dns.rs",
        "let win_loop_risk = deps.platform == \"win32\" && config .proxy_mode_type \
         .effective_on(Platform::parse(&deps.platform)) .is_tun()",
        1,
        "未知平台 → false（不走 Windows 死环防护）。正确：这条防的是 Win strict_route(WFP) 把所有\
          :53 逼进 TUN 后 type:local 经 svchost 回流的死环，机制是 Windows 独有的；判 false 只让内\
         网/反查解析器用 dns-local 而不是 dns-domestic。\n\
         2026-09-05 K12 判据加了半条：接管方式改读 `ProxyModeType::effective_on` 的**本平台生效值**\
         （只对 Android 分叉）。对本条**零行为差** —— 合取项写死 `win32`，Android 永远走不到 —— \
         接上是为了让「config-engine 里每一处平台相关的 proxy_mode_type 判据都读生效值」没有例外。\
         未知平台在这一格拿到的仍是 false。",
    ),
    (
        "crates/config-engine/src/builder/endpoint_routes.rs",
        "matches!( platform.to_ascii_lowercase().as_str(), \"darwin\" | \"macos\" | \"linux\" )",
        1,
        "未知平台 → false（不支持组网 System 内核接口）。2026-09-05 由禁止清单 `!= \"win32\" && !\
         = \"android\"` 改成允许清单，改的正是这一格：旧写法让任何没被点名的平台默认拿到「让核再建\
         一张内核 TUN」这项特权能力。与枚举版 `Platform::Other => false` 的同答由本文件 `mesh_sys\
         tem_support_agrees_across_enum_and_string_faces` 逐名对拍钉死。",
    ),
    (
        "crates/config-engine/src/builder/endpoints.rs",
        "if uses_system && platform != \"darwin\"",
        1,
        "未知平台 → 走非 darwin 腿，给 WG system endpoint 写死逻辑接口名。只有 macOS 的内核接口名\
         由内核动态分配（utunN）不能预写。未知平台若也动态分配，后果是名字对不上 ⇒ 出口路由装不上\
         （降级），不是起不来；且 System 在未知平台已由 endpoint_routes 那条禁掉 ⇒ 本臂不可达。",
    ),
    (
        "crates/config-engine/src/builder/endpoints.rs",
        "if platform != \"darwin\"",
        1,
        "同上一条，TS endpoint 的 `system_interface_name`。两处是同一条判据的两个下发点。",
    ),
    (
        "crates/config-engine/src/builder/generate.rs",
        "if platform.eq_ignore_ascii_case(\"darwin\")",
        1,
        "未知平台 → 走通用文案。这是 naive 缺 cronet 时的**用户可见原因串**分支，两侧都不改任何行\
         为，只影响提示里写不写「macOS 核未内置 cronet」。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "let fakeip_ranges: Vec<String> = if uses_fake_ip(config) && deps.platform != \"linux\"",
        1,
        "未知平台 → 计算 fakeip 段。它只作为 route_exclude 减法的输入；Linux 例外是因为该平台 rou\
         te_exclude 恒空，算了也白算。多算在未知平台上不产生下发。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "let mut exclude_addr: Vec<String> = if deps.platform == \"win32\" && should_bypass_lan",
        1,
        "未知平台 → false，继续往下判 linux / android / else。Win 这条是 bypassLAN carve 的专属计\
         算（wintun + WFP 语义），别的平台没有对应机制。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "else if deps.platform == \"linux\"",
        1,
        "未知平台 → false，继续往下判 android / else。Linux 恒空是加法态实证（route_exclude 非空即\
         触发策略路由表两族分解 ⇒ 连入与 allowLan 回包全断）。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "else if deps.platform == \"android\"",
        1,
        "🔴 **本门的历史回放样本**。未知平台 → false ⇒ 落到最后的 else 拿回环排除 `127.0.0.0/8` +\
          `::1/128`。这条 Android 空集臂是 2026-09-04 的缺陷修复：`VpnService.Builder.excludeRout\
         e()` 拒收回环前缀，而 Builder 的语义是任何一条被拒就整个 `establish()` 失败 ⇒ 隧道一次都\
         建不起来（模拟器实证 `configure tun interface: Bad address`）。删掉本臂 = 回到缺陷态 ⇒ 本\
         门反腐烂那一半立刻红。新平台在这里拿到的是回环排除，它是不是也拒收回环，必须现场答。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "else if deps.platform == \"ios\"",
        1,
        "🔵 本条是 **2026-09-06 为 iOS 新开的具名分支**，不是既有分派点的改写。此前 iOS 落进最后那个\
         `else`，拿到回环排除 `127.0.0.0/8` + `::1/128` —— 那正是本门历史样本（Android 上 \
         `VpnService.Builder.excludeRoute()` 拒收回环前缀 ⇒ `establish()` 整个失败 ⇒ 隧道一次都建不\
         起来）的**同一个落点**。\n\
         iOS 的答案是**恒空**，但依据与 Android 那条不同，故独立成支而不是并进去：Android 那条有一\
         次实测的拒收；iOS 上 `NEPacketTunnelNetworkSettings` 收不收回环排除**本仓没有验证过**（构\
         不出 iOS 产物）。支撑发空集的是另一半、且那一半不依赖实测：回环流量根本不进隧道路由表\
         （内核在 lo 上直接闭环）⇒ 发这两条前缀买不到任何东西，而代价上限是阻断级。收益零 / 代价上\
         限阻断 ⇒ 不发。\n\
         对**未知平台零影响**：它们仍落最后那个 `else` 拿回环排除，那一支一个字都没动。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "let strict_route = if deps.platform == \"android\" || deps.platform == \"ios\"",
        1,
        "未知平台 → 走 else，照发用户档位的 `strict_route`（缺省 true）。Android 那一支发 `None`\
         （一个键都不发），因为该字段在 libbox + `VpnService` tun fd 这条路上**一行代码都不会跑**\
         ：唯一落点在 `NativeTun::rules()`，只由 `NativeTun::Start()` 调用，而 tun fd 根本不走 \
         `Start()`；模拟器实测起隧道前后 `iptables -S`/`ip6tables -S` 逐字节相同（正对照：同一时\
         刻 `ip rule` +14 条 netd 规则，观测手段是有效的）。照发就是在配置里写一句假话，而 UI 侧\
         已按同一判据撤掉了这个开关。未知平台按桌面口径照发：它若也用 tun fd 而非内核 TUN，这条\
         就是必须现场重答的题 —— 本表就是让它被看见一次的地方。\n\
         2026-09-06：**iOS 一并不发**。承重的依据是仓内可查的那条 —— 移动端 UI 是 Android/iOS 共用\
         的一份（`ui/mobile.html` → `ui/src/mobile/*`，装载面在 `lib.rs` 的 `#[cfg(mobile)]` 那一行，\
         而 `mobile` = ios|android，见 `tauri-build-2.6.3/src/lib.rs:475-477`），而 \
         `ui/src/mobile/settings/TunPage.tsx` 里这个开关整个不显示。iOS 照发 `true` 就成了「UI 上没\
         有这个东西、配置里却写着防绕行已开启」，与 Android 那条要避免的形态逐字相同。\n\
         **不拿 Android 那条源码链当 iOS 的依据**：那条的落点是 `sing-tun/tun_linux.go`，而 Apple 上\
         参与编译的是 `tun_darwin.go` —— 本仓没读过它。未知平台在这一格拿到的仍是「照发」。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if deps.platform == \"win32\"",
        2,
        "两处同形（DNS IP 排除、TUN 接口名）。未知平台 → false：两处都是 Windows 专属（防 DNS 回流\
         死循环的额外排除段、wintun 适配器命名），别的平台既无该机制也无该命名空间。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if deps.platform != \"linux\"",
        1,
        "未知平台 → true，做节点 IP 排除。这是「别把自己的出口流量绕回 TUN」的通用防护，Linux 例外\
         的理由是它的 route_exclude 整块恒空。对未知平台做这层排除是安全侧：多排一条服务端 IP 不会\
         断网。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if !user_inbound_cidrs.is_empty() && deps.platform == \"linux\"",
        1,
        "未知平台 → false ⇒ 走 else 真正计算用户「连入来源排除」。Linux 那条是忽略腿 + 告警（该平\
         台上此项是毒丸）。未知平台按通用腿处理，产出仍受 route_exclude 下发面约束。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if deps.platform == \"darwin\"",
        2,
        "两处同形（TUN inet4 默认地址 /30 vs /16、`platform.http_proxy` 下发）。未知平台 → false：\
         拿 /16 默认地址、不下发 http_proxy 键。前者是 macOS NE 的具体限制，后者在 Android 上会被\
         消费成 `VpnService.Builder.setHttpProxy`（明确不扩），对未知平台不下发是不做假设的那一侧\
         。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if deps.platform == \"linux\"",
        1,
        "未知平台 → false，不写死 TUN 接口名。Linux 写死是因为 systemd-resolved per-link 接管、ap\
         p marker 与 root helper 三方都引用那个稳定名。未知平台没有那三方，不写死 = 交给内核命名，\
         无副作用。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if deps.platform == \"android\"",
        1,
        "未知平台 → false，不下发 `exclude_package`。按应用分流是 Android 独有的 tun 字段（`VpnSe\
         rvice` 语义），别的平台的核不认它。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if platform != \"android\" && platform != \"ios\"",
        1,
        "`drop_mobile_loopback_excludes`（2026-09-06 新增）。未知平台 → true ⇒ **提前返回、一条都\
         不剔**，用户声明的回环排除段原样下发。这是安全侧：剔除是为绕开一条**实测过的系统拒收**\
         （Android `VpnService.Builder.excludeRoute` 对回环前缀抛 `Bad address`，一条被拒整个 `est\
         ablish()` 失败），对一个本仓从未构建过的平台，既不知道它的 tun 接口收不收回环，也不知道\
         它的用户是不是真需要那一段 —— 静默吞掉一条用户显式声明的排除段，比多发一条更难查。\n\
         桌面三端在这一格拿到的也是 true（不剔），且那是**必要**的：win32/darwin 的平台基线本来\
         就含 `127.0.0.0/8` 与 `::1/128`。\n\
         iOS 与 Android 同答，但两半依据强度不同：Android 那半是实测拒收；iOS 那半承重的是「回环\
         流量根本不进隧道路由表 ⇒ 排除它买不到任何东西，而代价上限是隧道一次都建不起来」——\
         与本文件 ios 平台基线那一臂是同一条已采纳的推理，**不是**把 Android 的实测挪用过去。\n\
         若哪天把这条判据改成按 `Platform` 枚举穷举，编译器就替这一格守了。",
    ),
    (
        "crates/config-engine/src/builder/outbound_helpers.rs",
        "Some(\"windows\") => platform == \"win32\", Some(\"apple\") => platform == \"darwin\", _ => false,",
        1,
        "未知平台 → 两条比较都 false ⇒ 不下发 TLS engine。正确且是安全侧：utls 的 windows/apple e\
         ngine 是平台原生 TLS 栈的绑定，未知平台上不存在；不下发 = 用默认 TLS 栈，功能在。\n\
         🔵 **2026-09-06 iOS 点名（本表唯一一条「答案安全但依据不完整」）**：iOS 在字面上就是 \
         Apple，可这里比的是 `\"darwin\"`，故 `platform == \"ios\"` 时 apple engine **不下发**。\
         保留 false 而不是改成 `darwin | ios`：那要求「iOS 上的核里真的编入了 apple utls engine」，\
         而本仓构不出 iOS 产物、无从取证；判错成 true 的后果是给核下发一个它没有的 engine 名，\
         判错成 false 只是退回默认 TLS 栈（功能在、指纹不同）。代价不对称 ⇒ 取 false。\n\
         **未验证，待 iOS 核构建出来后重答**：iOS 版 libbox 是否含 apple utls engine。",
    ),
    (
        "crates/config-engine/src/builder/tun_route_exclude.rs",
        "let (extra, dropped_own_lan_mac) = if input.platform == \"darwin\"",
        1,
        "未知平台 → false ⇒ 不做「物理 LAN 段从用户 exclude 里剔除」那一步。该步是 macOS NE 反向路\
         由丢包的专项规避；未知平台不做的后果是用户声明的段原样保留（尊重用户输入），不是断网。",
    ),
    (
        "crates/config-engine/src/user_config/neighbor.rs",
        "matches!(platform.to_ascii_lowercase().as_str(), \"linux\" | \"darwin\")",
        1,
        "未知平台 → false（不支持 source_mac_address / source_hostname 规则）。**允许清单形态，已\
         经是安全侧**：这是内核能力事实（neighbor_resolver 只有 linux/mac 实现），未知平台判不支持\
          = 该规则不下发，而不是下发一条核不认的字段。",
    ),
    (
        "crates/config-engine/src/user_config/neighbor.rs",
        "platform.eq_ignore_ascii_case(\"linux\")",
        1,
        "未知平台 → false（不支持 TUN include/exclude_mac_address）。同上，允许清单 + 内核硬限界（仅\
          Linux 且需 auto_route+auto_redirect+nftables）。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "\"darwin\" | \"macos\" => Self::Mac, \"win32\" | \"windows\" => Self::Win, \"linux\" => Self::Linux, \"android\" => Self::Android, \"ios\" => Self::Ios, _ => Self::Other,",
        1,
        "🔵 **这是字符串轴与枚举轴唯一的桥，本门的取材面就从它派生**（见 `platform_name_literals`\
         ）。未知平台 → `Platform::Other`，即「本仓没有为这个平台答过题」—— 这正是它该给的答案，`\
         Other` 的全部意义就是把「不知道」显式化。给新平台加别名时改这里，本门的取材面自动跟着变宽\
         。\n\
         2026-09-06 加 `\"ios\" => Self::Ios`：本条**按设计当场腐烂变红**了一次（语句原文变了），\
         那正是这张表逼人重新答一遍「未知平台在各分派点得到什么」的方式。连带效果有两个，都要记住：\
         ① 取材面变宽 —— `\"ios\"` 从此也是分派位字面量，全仓凡是按它分叉的地方都要登记（本批新增了\
         `inbounds.rs` 三条与 `startup.rs` 一条）；② `mesh_system_support_agrees_across_enum_and_str\
         ing_faces` 里那圈「未知平台」用例不能再拿 `\"ios\"` 当样本，已换掉。",
    ),
    (
        "src-tauri/src/commands/misc/logs.rs",
        "\"windows\" => \"win32\", \"macos\" => \"darwin\", other => other,",
        1,
        "未知平台 → 原样透传（`other => other`）。正确：本函数只做 Rust `consts::OS` → Node `proc\
         ess.platform` 的**别名映射**，两套词汇只在 windows/macos 上不同名；透传是「不认识就不改名\
         」，而不是猜一个名字。",
    ),
    (
        "src-tauri/src/commands/misc/support.rs",
        "\"windows\" => \"win32\", \"macos\" => \"darwin\", other => other,",
        1,
        "同 `commands/misc/logs.rs` 那条，逐字同形（诊断包与备份文件各自需要 Node 口径平台串）。未\
         知平台原样透传。",
    ),
    (
        "src-tauri/src/runtime/core_paths.rs",
        "\"windows\" => Some(\"libcronet.dll\"), \"linux\" => Some(\"libcronet.so\"), _ => None,",
        1,
        "未知平台 → `None` = 「本平台没有 cronet 动态 sidecar」。与 macOS 同臂（macOS 静态编入）。\
         诚实：未知平台既没有随包动态库，也不知道它的动态库扩展名，返 None 让上层按「无 sidecar」\
         处理。",
    ),
    (
        "src-tauri/src/runtime/core_paths.rs",
        "if os == \"windows\"",
        1,
        "未知平台 → `sing-box`（无 `.exe` 后缀）。正确：`.exe` 后缀是 Windows 独有的可执行文件约定\
         ，类 unix 与未知平台一律无后缀。",
    ),
    (
        "src-tauri/src/runtime/proxy/platform_contracts.rs",
        "\"macos\" => \"darwin\", \"windows\" => \"win32\", other => other,",
        1,
        "未知平台 → 原样透传。这是 `platform_tag()`，config-engine 全部平台串分支的**唯一产地**：\
         Android 上 `consts::OS` 就是 `android`，透传后 config-engine 侧直接比 `\"android\"`。透传\
         保证「新平台名不经这里被改写成别的平台」。",
    ),
    (
        "src-tauri/src/runtime/proxy/platform_contracts.rs",
        "\"macos\" =>",
        1,
        "`core_platform_dirs` 的 macOS 臂（按运行架构在 mac-arm64 / mac-x64 之间排序）。未知平台落\
         在下一条登记的 `_ =>`。",
    ),
    (
        "src-tauri/src/runtime/proxy/platform_contracts.rs",
        "\"windows\" => vec![\"win\"], _ => vec![\"linux\"],",
        1,
        "未知平台 → `vec![\"linux\"]`，即去 `resources/linux/` 找核。这是**找不到就找不到**的纯路\
         径候选：未知平台上那个目录里不会有能跑的核，结局是 `resolve` 失败并显式报错，不是静默跑错\
         的核。",
    ),
    (
        "src-tauri/src/runtime/proxy/route_replan.rs",
        "mode.effective_on(Platform::parse(platform)).is_tun() && platform == \"win32\"",
        1,
        "未知平台 → false（不探 wintun 适配器）。正确：wintun 是 Windows 专属，`WinAdapterProbe` \
         枚举的也只有 Windows 适配器；在别处探必然恒 `Absent`，那会把一次完全正常的起核判成失败。\n\
         2026-09-05 R1 判据加了半条：接管方式改读 `ProxyModeType::effective_on` 的**本平台生效值**\
         （字符串轴经 `Platform::parse` 这座唯一的桥过去；只对 Android 分叉）。对本条**零行为差** ——\
         合取项写死 `win32`，Android 与未知平台都走不到 —— 接上是为了让 \
         `src-tauri/tests/android_takeover_is_a_platform_fact.rs` 那道门没有例外。\
         未知平台在这一格拿到的仍是 false。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "\"darwin\" | \"android\" => true, \"ios\" => false, _ => false,",
        1,
        "未知平台 → false（判「核里没有静态编入的 cronet」）。允许清单形态，安全侧：判错成 false \
         只是让 naive 节点在该平台被标为不可用（用户看得见原因），判错成 true 会放行一个核里其实没\
         有 cronet 的节点 ⇒ 连不上且无解释。两个 true 的平台各自有二进制取证（见函数文档的取证表）\
         。\n\
         2026-09-06 由 `matches!(platform, \"darwin\" | \"android\")` 改成显式 `match`，只为把 iOS \
         的 false 从「兜底落进去的」变成「答过的」——**求值结果逐平台不变**（本条的未知平台答案也没\
         变）。iOS 答 false 的依据：本仓今天构不出 iOS 产物，「那个核里有没有 cronet」没有事实可查；\
         而将来有了也不白送（iOS 的 cronet 是预编译静态库，链接面要另接十几个 Apple framework，与\
         Android 那份 `build_libbox` 默认 sharedTags 就含 `with_naive_outbound` 完全不同形）。\n\
         ⚠️ 这个诚实的 false 带着一个**已知的坏形态**：`is_node_usable` 会静默丢弃全部 naive/H3 节点，\
         用户看到的是「节点无效」而不是「本构建不含 naive」。iOS 腿真正接上核时必须连同归因提示一起\
         重答；在那之前由 `runtime/proxy/tests/platform_contracts.rs` 的 \
         `cronet_available_across_core_forms` 钉住（翻成 true 即红）。",
    ),
    (
        "src-tauri/src/runtime/uninstall.rs",
        "\"macos\" => mac_app_bundle_from_exe(exe).map_or_else( ||",
        1,
        "`plan_app_removal` 的 macOS 臂。未知平台落在该 `match` 的 `_` 臂（本门只登记出现平台名字\
         面量的臂；`_` 臂的存在由紧邻的三条登记共同界定）。",
    ),
    (
        "src-tauri/src/runtime/uninstall.rs",
        ", AppRemoval::RemoveDir, ), \"linux\" =>",
        1,
        "`plan_app_removal` 的 Linux 臂（AppImage / `/usr` 包管理器 / 其余三分）。未知平台 → `_ =\
         > Unsupported`，即**明确拒绝删任何东西并让用户手动处理**。这正是本函数全篇的纪律：不猜路\
         径。删除是不可逆操作，未知平台上「不做」是唯一正确答案。",
    ),
    (
        "src-tauri/src/runtime/uninstall.rs",
        "\"windows\" =>",
        1,
        "`plan_app_removal` 的 Windows 臂（拉 NSIS uninstall.exe；进程不能删自己）。未知平台同上落\
          `_ => Unsupported`。",
    ),
    (
        "src-tauri/src/runtime/update_install.rs",
        "\"linux\" =>",
        1,
        "`detect_run_form` 的 Linux 臂（AppImage ⇒ Loose，否则 Installed）。未知平台 → `_ => Inst\
         alled`，见下一条。",
    ),
    (
        "src-tauri/src/runtime/update_install.rs",
        "\"windows\" =>",
        1,
        "`detect_run_form` 的 Windows 臂（portable.marker ⇒ Loose，否则 Installed）。未知平台 → `\
         _ => Installed`。",
    ),
    (
        "src-tauri/src/runtime/update_install.rs",
        "\"macos\" => RunForm::Loose, _ => RunForm::Installed,",
        1,
        "未知平台 → `Installed`。**这一格是失败安全的那一侧**（函数文档已登记同一取舍）：判成 Ins\
         talled 会走安装器路径（安装器能装、不会砸掉便携副本），判成 Loose 则会对一个真装过的副本\
         做原地替换。未知平台上取安全侧正确。",
    ),
    // 2026-10-01：对齐出生/退出托管与 Android 调试桥的真实新增点。
    (
        "crates/config-engine/src/builder/generate.rs",
        "if deps.platform == \"android\" && deps.has_management_api",
        1,
        "双模式 Clash 投影仅给已有管理 API 的 Android JNI 宿主；iOS/未知平台不因此取得该投影或管理能力，须先接入自己的管理 API 面。",
    ),
    (
        "crates/config-engine/src/builder/inbounds.rs",
        "if cfg!(debug_assertions) && deps.platform == \"android\" && deps.debug_probe_mixed_udp",
        1,
        "认证回环探针的 UDP mixed 仅限 Debug Android 且 runtime 明确开启；release/iOS/未知平台保持原 HTTP 入站，不凭平台字符串扩大监听能力。",
    ),
    (
        "src-tauri/src/runtime/proxy/mesh_apply/candidate.rs",
        "profile: CandidateProfile::DesktopNonTunDirectVlessV1, target_os: deps.platform.clone(), target_arch: deps.arch.clone(), ports: PortsProvenance::ProbedNumbersNotReserved, interface_binding: SubsetApplicability::NotRequiredByValidatedNonTunSubset, dns_sidecar: SubsetApplicability::NotRequiredByValidatedNonTunSubset, check_support: if deps.platform == \"linux\" && deps.arch == \"x86_64\" && plain_tcp",
        1,
        "严格候选核检查目前仅支持 Linux x86_64 的 plain TCP 子集；其它平台/架构落 UnsupportedProfile。候选生成已按 effective_on 拒绝移动端 Tun，且 managed_launch_unsupported 保持成立，此元数据不能许可出生。",
    ),
    (
        "src-tauri/src/runtime/updater.rs",
        "if allowed_absent .iter() .any(|module| key == \"linux\" || *module != \"github.com/sagernet/nftables\")",
        1,
        "core-manifest 补丁模块分区：Linux 不允许 nftables 缺席，其它已有目标可缺 Linux 专属模块。key 来自闭集清单目标；未知平台不会由该判断生成新核资源或运行能力。",
    ),

];

// ── 判据本体 ──

#[test]
fn platform_name_literals_are_derived_from_parse() {
    let names = platform_name_literals();

    assert!(
        names.len() >= PLATFORM_NAME_FLOOR,
        "只从 `Platform::parse` 抠出 {} 个平台名字面量（{names:?}，下限 {PLATFORM_NAME_FLOOR}）\
         —— 抠取口径坏了，不是别名真的只剩这么点",
        names.len()
    );

    // 正面断言：派生出来的名字**真的能把每个具名变体都命中一次**。
    // 只判条数会被「抠出 6 个但全是别的串」骗过。
    for variant in Platform::ALL.iter().copied() {
        if variant == Platform::Other {
            continue; // Other 按定义没有对应字面量（它是 `_ =>` 的落点）。
        }
        assert!(
            names.iter().any(|n| Platform::parse(n) == variant),
            "{variant:?} 没有任何一个派生字面量映射到它 —— `Platform::parse` 的别名表与本门的\
             取材面已经脱钩，字符串轴上这个平台的分派点将全部扫不到"
        );
    }
    // 反向：派生出来的每一个名字都必须真的被 `parse` 认得（抠到了注释残渣就会在这里红）。
    for name in &names {
        assert_ne!(
            Platform::parse(name),
            Platform::Other,
            "派生出的字面量 {name:?} 并不被 `Platform::parse` 认作任何具名平台 —— 抠取口径把\
             不相干的串也捞进来了"
        );
    }
    println!("[string-dispatch] 派生平台名字面量：{names:?}");
}

#[test]
fn string_platform_dispatch_is_registered() {
    let names = platform_name_literals();
    let registered_total: usize = STRING_DISPATCH_REGISTRY.iter().map(|(_, _, n, _)| *n).sum();
    assert!(
        registered_total >= STRING_DISPATCH_FLOOR,
        "登记表只覆盖 {registered_total} 条（下限 {STRING_DISPATCH_FLOOR}）—— 有人把表清空了，\
         而不是字符串分派点真的消失了"
    );

    let files = production_sources();
    assert!(
        files.len() >= 100,
        "只扫到 {} 个生产源文件 —— 取材面塌了",
        files.len()
    );

    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (rel, path) in &files {
        let src = std::fs::read_to_string(path).expect("读源文件失败");
        let code = mask_comments(&src);
        for snippet in string_dispatch_snippets(&code, &names) {
            *found.entry((rel.clone(), snippet)).or_default() += 1;
        }
    }

    let mut registered: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (path, snippet, count, _why) in STRING_DISPATCH_REGISTRY {
        *registered
            .entry(((*path).to_owned(), (*snippet).to_owned()))
            .or_default() += *count;
    }

    let unregistered: Vec<String> = found
        .iter()
        .filter(|(k, n)| registered.get(*k).copied().unwrap_or(0) < **n)
        .map(|((p, s), n)| {
            let r = registered
                .get(&(p.clone(), s.clone()))
                .copied()
                .unwrap_or(0);
            format!("{p}  （实到 {n} 次，登记 {r} 次）\n      {s}")
        })
        .collect();
    let rotten: Vec<String> = registered
        .iter()
        .filter(|(k, n)| found.get(*k).copied().unwrap_or(0) < **n)
        .map(|((p, s), n)| {
            let f = found.get(&(p.clone(), s.clone())).copied().unwrap_or(0);
            format!("{p}  （登记 {n} 次，实到 {f} 次）\n      {s}")
        })
        .collect();

    // 切片自检：把取到的东西打出来。扫到 0 条也「没有未登记项」，那种绿没有信息量。
    let found_total: usize = found.values().sum();
    println!(
        "[string-dispatch] 生产源文件 {} 个；字符串平台分派点实到 {found_total} 条 / {} 种形态；\
         登记 {registered_total} 条 / {} 种形态",
        files.len(),
        found.len(),
        registered.len()
    );
    for ((p, s), n) in &found {
        println!("  {p} ×{n}\n      {s}");
    }
    assert!(
        found_total >= STRING_DISPATCH_FLOOR,
        "只扫到 {found_total} 条字符串平台分派点（下限 {STRING_DISPATCH_FLOOR}）—— 取材面塌了\
         （路径 / 注释掩码 / 分派位判据任一坏掉都长这样）"
    );

    assert!(
        unregistered.is_empty(),
        "以下**字符串型**平台分派点没有登记：\n    {}\n\n\
         编译器对 `platform == \"android\"` / `match os {{ \"macos\" => … }}` 形态一句话都不说 ——\
         新平台接进来时它们静默走 `_ =>` 或 `else`，而那正是 `inbounds.rs` 那条\
         「Android 拿到回环排除 ⇒ VpnService 拒收 ⇒ 隧道建不起来」的成因形态。\n\
         故每一处都必须在 `STRING_DISPATCH_REGISTRY` 里写下「一个本仓从未构建过的平台在这里\
         得到什么、为什么那个答案可以」。\n\
         若这处判据能改成按 `Platform` 枚举穷举 `match`，优先改 —— 那样编译器就替你守了。",
        unregistered.join("\n    ")
    );
    assert!(
        rotten.is_empty(),
        "以下登记条目在源码里已经找不到（判据被改动或删除）：\n    {}\n\n\
         登记表不是垃圾桶：判据变了就要重新答一次「未知平台在这里得到什么」，然后把这一条\
         更新成新的语句原文。**删掉一条具名平台臂也会在这里红** —— 那正是本门的历史样本\
         （`inbounds.rs` 的 Android 空 `route_exclude_address`）被删除时的形态。",
        rotten.join("\n    ")
    );
}

/// `mesh_system_supported_on_platform` 的**两份实现**（枚举版 / 字符串版）逐名对拍。
///
/// # 守的是什么
///
/// 这条判据同时决定三件事：config 生成侧的 `system_interface_available`（endpoint 降不降级成
/// gVisor）、起核重试预算、以及出口路由状态机三个入口是否早退。而它有两份实现：
/// `polaris_mesh`（收 `Platform` 枚举）与 `polaris_config_engine`（收 `process.platform` 风格串）。
/// 两份不同答就会造出「config 生成认为 System 可用、出口路由状态机认为不可用」的半开状态 ——
/// 此前这条一致性只写在两处函数注释里，**没有任何东西断言过它**。
///
/// 对拍面是派生的：按 `Platform::parse` 的全部字面量逐名比对，故加别名/加平台时自动覆盖。
#[test]
fn mesh_system_support_agrees_across_enum_and_string_faces() {
    use polaris_config_engine::builder::endpoint_routes::mesh_system_supported_on_platform as by_str;
    use polaris_mesh::mesh_system_supported_on_platform as by_enum;

    let names = platform_name_literals();
    assert!(names.len() >= PLATFORM_NAME_FLOOR, "取材面塌了：{names:?}");

    for name in &names {
        let variant = Platform::parse(name);
        assert_eq!(
            by_enum(variant),
            by_str(name),
            "平台名 {name:?}（→ {variant:?}）上两份实现不同答：枚举版 {} / 字符串版 {}。\
             两份必须逐名同答，否则 config 生成侧与出口路由状态机会对同一台机器给出相反的答案。",
            by_enum(variant),
            by_str(name)
        );
        // 大小写不敏感是字符串版的既有契约（`win32` / `WIN32` 同答），一并钉住。
        assert_eq!(
            by_str(name),
            by_str(&name.to_ascii_uppercase()),
            "字符串版对 {name:?} 的大小写敏感了"
        );
    }

    // 正面断言：支持面非空。两份实现同时坏成全 false 时，上面那圈相等断言会平凡成立。
    assert!(by_enum(Platform::Mac) && by_enum(Platform::Linux));
    assert!(by_str("darwin") && by_str("linux"));

    // ── 未知平台这一格（2026-09-05 的决定，本条就是钉住它的判据）──
    //
    // 枚举轴的未知落点是 `Platform::Other`，字符串轴的未知落点是任何不在别名表里的串。
    // 两边都必须答 false：`Other` 的语义是「本仓没有为这个平台答过题」，把它读成「答案是支持」
    // 正是 K6b 那条阻断级缺陷的形状（Android 曾靠这条 true 去申请一张它建不出来的内核 TUN）。
    // 代价不对称：判错成 false 只是退 gVisor（功能在），判错成 true 是起核 FATAL。
    // `"ios"` **不再在这条清单里**（2026-09-06）：它已被 `Platform::parse` 认领成具名变体，
    // 于是上面那圈按 `platform_name_literals()` 派生的逐名对拍**自动覆盖了它** —— 那才是它该被
    // 检查的地方。留在这里只会让下面那条 `parse(unknown) == Other` 的自检当场红（实测红过一次，
    // 报错原文就是「它已被 Platform::parse 认领，换一个」）。
    //
    // 具名之后 iOS 在这两份实现上都答 false，与它在 `Other` 里时**结果相同、依据不同**：
    // 那时是「没人答过题的默认安全侧」，现在是 `crates/mesh/src/exit_route.rs` 的 `Ios` 臂
    // 写下的答案（唯一 tun fd 由 NEPacketTunnelProvider 授予且已被主 TUN 占用 ⇒ 第二张内核接口
    // 无来源）。下面新增的那条断言钉的就是「结果相同」这一半。
    for unknown in ["freebsd", "openbsd", "solaris", ""] {
        assert_eq!(
            Platform::parse(unknown),
            Platform::Other,
            "本用例假定 {unknown:?} 是未知平台；它已被 `Platform::parse` 认领，换一个"
        );
        assert!(
            !by_str(unknown),
            "字符串版对未知平台串 {unknown:?} 答了「支持」—— 禁止清单式写法（`!= \"win32\"`）\
             回来了：任何没被点名的平台都会默认拿到 System 内核接口"
        );
    }
    assert!(
        !by_enum(Platform::Other),
        "枚举版对 `Platform::Other` 答了「支持」—— 未知平台不得默认获得「让核再建一张内核 TUN」\
         这项特权能力，理由见该函数文档的代价不对称表"
    );

    // ── iOS 这一格（2026-09-06 具名变体落地时的决定）──
    //
    // 它已被上面的逐名循环覆盖（`"ios"` 现在是 `platform_name_literals()` 的成员），这里再钉一条
    // **绝对值**而不是相对相等：逐名循环判的是「两份实现同答」，两份同时错成 `true` 时它照样绿。
    // 代价不对称在这一格与 `Other` 完全相同（判错成 false 只退 gVisor，判错成 true 是起核 FATAL），
    // 而 iOS 的物理约束是确定的：唯一的 tun fd 由 `NEPacketTunnelProvider` 在扩展沙箱内授予且已被
    // 主 TUN 占用，应用侧没有创建第二张内核接口的路子。
    assert!(
        !by_enum(Platform::Ios) && !by_str("ios"),
        "iOS 上有一份实现答了「支持组网 System 内核接口」—— 那要求核再建一张它建不出来的内核 TUN，\
         失败形态是起核 FATAL 而不是降级"
    );

    // 跨语言那一份（`ui/src/domain/endpoint-routes.ts`）**不在本门射程内** —— 它由
    // `ui/src/domain/endpoint-routes-mesh-parity.test.ts` 守（2026-09-06 补，此前三份实现里
    // 只有 TS 那份还是禁止清单，在 `ios` 与未经别名映射的 `windows` 上与本文件这两份不同答）。
}

// ── 字符串面的取材实现 ──

/// 从 `helper-proto` 的 `Platform::parse` 里抠出全部平台名字面量（去重、排序）。
///
/// **派生而非手写**：`parse` 是字符串轴与枚举轴唯一的桥，给它加一个新平台名，本门的取材面
/// 自动跟着变宽。手写一份清单会在「加了别名、忘了改门」时静默漏扫 —— 那正是本门要防的形态。
fn platform_name_literals() -> Vec<String> {
    let src = std::fs::read_to_string(repo_root().join("crates/helper-proto/src/lib.rs"))
        .expect("读不到 helper-proto/src/lib.rs —— 取材面本身塌了");
    // 只剥注释：要抠的正是字符串字面量。
    let code = mask_comments(&src);
    assert_eq!(
        code.matches("fn parse").count(),
        1,
        "`helper-proto/src/lib.rs` 里出现了不止一个 `fn parse` —— 本函数的锚点不再唯一，\
         抠到的可能是别的函数体"
    );
    let at = code.find("fn parse").expect("找不到 `Platform::parse`");
    let open = code[at..]
        .find('{')
        .map(|o| at + o)
        .expect("`parse` 后没有 `{`");
    let close = balanced_end(&code, open).expect("`parse` 括号不配平");
    let body = &code[open..=close];

    let mut names: Vec<String> = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                j += 1;
            }
            if j < bytes.len() {
                let lit = &body[start..j];
                if !lit.is_empty()
                    && lit
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                    && !names.iter().any(|n| n == lit)
                {
                    names.push(lit.to_owned());
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    names.sort();
    names
}

/// 比较型调用：实参位上的平台名字面量也算分派（`platform.eq_ignore_ascii_case("win32")`）。
const STRING_DISPATCH_CALLS: &[&str] = &[
    "eq_ignore_ascii_case",
    "eq",
    "starts_with",
    "ends_with",
    "contains",
];

/// 已剥注释的代码面里，所有**落在分派位**的平台名字面量，按所在语句合并、折叠空白。
///
/// 分派位判据（三选一，全部只看字面量的紧邻上下文）：
/// 1. 前面紧挨 `==` / `!=` / 模式的 `|`；
/// 2. 后面紧跟 `=>`（match 臂）或模式的 `|`；
/// 3. 前面紧挨 [`STRING_DISPATCH_CALLS`] 里某个比较方法的左括号。
///
/// **为什么不能只判「字面量出现」**：本面保留了字符串，日志与错误文案里满是平台名
/// （`"Android 上不适用…"`、`"linux 腿…"`）。只判出现会把它们全捞进来，而那类假阳性会训练人去
/// 改文案迁就门。
///
/// `#[cfg(target_os = "linux")]` 天然落在三条之外：它用的是单个 `=`。这是有意的边界，见本节头注。
fn string_dispatch_snippets(code: &str, names: &[String]) -> Vec<String> {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for name in names {
        let token = format!("\"{name}\"");
        let mut i = 0usize;
        while let Some(rel) = code.get(i..).and_then(|s| s.find(&token)) {
            let at = i + rel;
            i = at + 1;
            // 转义引号（`\"linux\"`，出现在嵌套字面量里）不是代码里的字面量。
            if at > 0 && code.as_bytes()[at - 1] == b'\\' {
                continue;
            }
            let end = at + token.len();
            let pre = code[..at].trim_end();
            let post = code[end..].trim_start();
            let hit = pre.ends_with("==")
                || pre.ends_with("!=")
                || (pre.ends_with('|') && !pre.ends_with("||"))
                || post.starts_with("=>")
                || (post.starts_with('|') && !post.starts_with("||"))
                || (pre.ends_with('(') && ends_with_compare_call(pre));
            if !hit {
                continue;
            }
            let span = statement_span(code, at);
            if !spans.contains(&span) {
                spans.push(span);
            }
        }
    }
    spans
        .into_iter()
        .map(|(a, b)| code[a..b].split_whitespace().collect::<Vec<_>>().join(" "))
        .collect()
}

/// `pre` 以 `<比较方法名>(` 结尾？（`pre` 已 `trim_end` 且以 `(` 结尾）
fn ends_with_compare_call(pre: &str) -> bool {
    let head = pre[..pre.len() - 1].trim_end();
    STRING_DISPATCH_CALLS.iter().any(|call| {
        head.ends_with(call) && {
            let cut = head.len() - call.len();
            cut == 0 || !head[..cut].ends_with(is_ident_char)
        }
    })
}

// ═══════════════════════════════════════════════════════════════════════════
// cfg 轴（2026-09-06 K13）
//
// # 前三面守完了，为什么还有第四面
//
// 前三面（穷举 `match` / 比较型 / 字符串型）判的都是**运行期**的平台值。本仓还有第四条平台
// 分派轴，它在**编译期**就把代码分成两份：`#[cfg(target_os = "android")]` /
// `#[cfg(not(target_os = "android"))]` / `#[cfg(desktop)]` / `#[cfg(mobile)]`。
//
// 这一轴此前**零门**。它的失效形态与前三面不同，也更隐蔽：
//
// - 它不是「新平台落进兜底」，而是「新平台落进**另一个平台的那一半**」。
//   `not(target_os = "android")` 在 iOS 上是 **true** ⇒ iOS 拿到的是**桌面那一份代码**，
//   而本批在枚举轴上为 iOS 写下的 40 个答案，绝大多数说的是「iOS 与 Android 同形：
//   进程内核、无 helper、无核子进程」。两条轴**互相矛盾**，而没有任何东西会说一句话。
// - 前三面的登记表都能靠「扫到没登记的就红」发现新分派点，这一面此前连扫描器都没有。
//
// # 本面的判据形态：**机器判边、人写理由**
//
// 与前三张表的关键差别：第四列（iOS 落哪一侧）**不是人写的**，是本门自己把谓词按
// `target_os = "ios", mobile, !desktop` 求值算出来的，再与 Android 环境下的求值对比。
// 人只负责第五列「那一侧对不对、为什么」。于是「登记表里写错了边」这种事会当场红 ——
// 前三张表做不到这一点（那三列的内容不可自证，force 只在「必须写一条」）。
//
// # 取材面（射程自曝）
//
// 只收**提到本轴的**谓词：`target_os = "android"` / `target_os = "ios"` / `desktop` / `mobile`。
// `#[cfg(target_os = "macos")]` 这类**不在**面内 —— iOS 与 Android 在它上面同落「都不是」那一侧，
// 不产生本轴要找的分叉。`unix` / `windows` 同理。这不是漏，是本面判据的定义域。
//
// 词表是**封闭**的：谓词里出现本门不认识的原子（`feature = …` / `target_arch = …` / …）时
// 本门**变红**并要求扩词表，而不是猜一个值。失效方向因此是 fail-closed。
// ═══════════════════════════════════════════════════════════════════════════

/// cfg 轴分派点**出现次数**的下限（正面断言，同 [`MATCH_BLOCK_FLOOR`] 之理）。
/// 2026-09-06 实测 112 处 / 30 种 (文件, 谓词) 对。
const CFG_SITE_FLOOR: usize = 80;

/// 一次 cfg 求值的环境。本门只需要两个：iOS 与 Android。
#[derive(Clone, Copy)]
struct CfgEnv {
    target_os: &'static str,
    /// `debug_assertions` 是构建构型轴，独立于移动平台轴。
    debug_assertions: bool,
    /// Tauri 的 `mobile` cfg（`tauri-build` 按 `target_os == "ios" || target_os == "android"` 发出）。
    mobile: bool,
    /// Tauri 的 `desktop` cfg（`mobile` 的反面）。
    desktop: bool,
}

const IOS_ENV: CfgEnv = CfgEnv {
    target_os: "ios",
    debug_assertions: false,
    mobile: true,
    desktop: false,
};
const ANDROID_ENV: CfgEnv = CfgEnv {
    target_os: "android",
    debug_assertions: false,
    mobile: true,
    desktop: false,
};
const IOS_DEBUG_ENV: CfgEnv = CfgEnv {
    debug_assertions: true,
    ..IOS_ENV
};
const ANDROID_DEBUG_ENV: CfgEnv = CfgEnv {
    debug_assertions: true,
    ..ANDROID_ENV
};

/// iOS 在这一格落哪一侧、那一侧对不对。**前半由本门算，后半由人写。**
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IosSide {
    /// iOS 与 Android 落在**同一侧**。本轴上不产生分叉。
    WithAndroid,
    /// release 同侧；debug 构型里仅 Android 编译此分支。
    DiffersOnlyInDebug,
    /// iOS 与 Android 落在**相反侧**，且相反是**对的**（两个平台在这一格本就该不同答）。
    DiffersRight,
    /// iOS 与 Android 落在**相反侧**，而 iOS 那一侧对它是**错的** —— 与枚举轴上写下的答案矛盾。
    /// 今天没改，第五列必须写清「为什么没改」与「改它要先有什么」。
    DiffersWrongToday,
    /// iOS 与 Android 落在**相反侧**，而**本仓今天判不了哪一侧对**（缺 iOS 侧事实）。
    /// 与 `DiffersWrongToday` 的区别是认识论的：那个是「知道错」，这个是「不知道」。
    DiffersUndecided,
}

/// cfg 轴分派点登记表。
///
/// 每条 = (仓库相对路径, 折叠空白后的 cfg 谓词, 该谓词在该文件里的出现次数, iOS 落哪一侧, 理由)。
///
/// 强制力：新增即红 / 腐烂即红（同前三张表），**外加**第四列被机器复核 —— 写错边当场红。
///
/// # 这一轴今天的总账（2026-09-06 首次盘点）
///
/// | iOS 侧 | 出现次数 | 说的是什么 |
/// |---|---|---|
/// | `WithAndroid` | 41 | 全是 `desktop` / `mobile` 及其合取。`mobile = ios\|android` ⇒ iOS 天然与 Android 同侧 |
/// | `DiffersRight` | 32 | Android 专属实现（JNI 桥、Kotlin 插件、平台证书校验器）与 `Platform::current()` 的产地 |
/// | `DiffersWrongToday` | 38 | **起核 / 停核 / 存活探测 / 统计数据面 / custom 探测（2026-09-25 +1）**：iOS 落在「桌面子进程 + clash API gRPC」那一份代码上 |
/// | `DiffersUndecided` | 2 | 窗口可见性可不可观测 —— iOS 侧没有事实 |
///
/// # 那 38 处为什么本批**不改**（这是本批最大的一条「未做」，写在这里而不是藏在报告里）
///
/// 它们的正确改法是 `any(target_os = "android", target_os = "ios")` 之类，但那一改会让 iOS
/// 编译单元去引用 `android_bridge` —— 一个 **JNI + Tauri Android 插件**的实现，iOS 上不存在
/// 对应物。改完的结果不是「iOS 走对了路」，是「iOS 走上了一条引用不存在符号的路」，
/// 而本仓**构不出 iOS 产物、无法验证改动的编译面**（三种宿主构建下这些分支一行都不编译）。
///
/// 用一次无法验证的批量改写去换一个「看起来一致」的登记表，正是本批要防的那种修法。
/// 故本批的处置是：**把矛盾登记成可见、可计数、会腐烂的事实**，并写下解锁条件 ——
/// 这 38 处要连同「iOS 侧的核桥」（`NEPacketTunnelProvider` + 扩展进程内 libbox 的 Rust 侧入口）
/// 一起改，那是另一批，且必须有 iOS 编译面才能取到收据。
///
/// 在那之前，本表的价值是：谁再往这条轴上加一处 `target_os = "android"` 分叉，
/// 都会被本门要求当场写下「iOS 落哪一侧」——而不是像 2026-09-06 之前那样，
/// 加 40 个 iOS 答案而这一轴一句话都不说。
type CfgSite = (&'static str, &'static str, usize, IosSide, &'static str);
const CFG_REGISTRY: &[CfgSite] = &[
    // ── crates ──
    (
        "crates/helper-proto/src/lib.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersRight,
        "`Platform::current()` 的 Android 分支。iOS 走不到本支是**对的**：它自己有下一条 \
         `target_os = \"ios\"` 分支。这两条正是枚举轴的产地 —— 本轴与枚举轴在这一格不但不矛盾，\
         这一格就是两轴的接缝本身。",
    ),
    (
        "crates/helper-proto/src/lib.rs",
        "target_os = \"ios\"",
        1,
        IosSide::DiffersRight,
        "🔵 **全仓唯一一处为 iOS 而写的 cfg**（本轴今天只有这一处正面提到 iOS）。\
         `Platform::current()` 的 iOS 分支 ⇒ `Platform::Ios`。少了它，本批那 40 个枚举轴答案\
         一处都走不到（全落 `Other`），即 K6b 的成因形态。\
         **本仓构不出 iOS 产物 ⇒ 这一支一次都不会被求值**，没有任何门能证明它跑过。",
    ),
    // ── src-tauri：desktop / mobile 面（iOS 与 Android 同侧）──
    (
        "src-tauri/src/app_tray.rs",
        "all(desktop, not(target_os = \"macos\"))",
        1,
        IosSide::WithAndroid,
        "托盘菜单里的非 macOS 桌面项。`desktop` 在两个移动平台上同为 false ⇒ 同侧，\
         合取里的 `not(macos)` 不改变这一点。",
    ),
    (
        "src-tauri/src/app_tray.rs",
        "desktop",
        9,
        IosSide::WithAndroid,
        "整棵原生托盘（`tauri::menu` / `tauri::tray` 在移动端根本不存在）。iOS 与 Android 同侧。\
         **答案同侧，但既有注释里的理由只写了 Android**（「Android 不可达」）—— iOS 上不可达的\
         理由不同：iOS 连「后台常驻 + 状态栏图标」这个形态都没有，不是「有托盘但用不上」。\
         答案不变，故不改代码；理由记在这里。",
    ),
    (
        "src-tauri/src/commands/misc/autostart.rs",
        "desktop",
        3,
        IosSide::WithAndroid,
        "开机自启插件（`tauri_plugin_autostart`）的两条桌面腿 + 它们要的 `use tauri::Manager`。\
         两个移动平台同侧（不编译）。2026-09-25 由 2 增至 3：`Manager` 的 import 只剩桌面腿在用，\
         挂同一个 cfg 免得 Android 编译单元里出现未用 import。",
    ),
    (
        "src-tauri/src/commands/misc/autostart.rs",
        "target_os = \"android\"",
        2,
        IosSide::DiffersRight,
        "2026-09-25：`autoStart` 的 Android 腿 = 「开机自动连接」，经起停核桥写/读 Kotlin 标记\
         （开机时由 `BootReceiver` 消费）。iOS 不走这一支是**对的**：桥是 Android 插件 API；\
         iOS 的对应物是 `NEOnDemandRule`（Connect On Demand），要的是新分支而不是 `any(android, ios)`。",
    ),
    (
        "src-tauri/src/commands/misc/autostart.rs",
        "all(mobile, not(target_os = \"android\"))",
        2,
        IosSide::DiffersRight,
        "上一条的孪生支（原先的 `mobile` 支收窄而来）：iOS 写侧显式报「本平台不支持」、读侧回 `false`\
         —— 诚实降级。iOS 落这一侧是**对的**：今天它确实没有开机自动连接的执行者。",
    ),
    (
        "src-tauri/src/commands/misc/logs.rs",
        "all(target_os = \"android\", debug_assertions)",
        2,
        IosSide::DiffersOnlyInDebug,
        "Android debug 构建采集并分享 libbox 原生日志；iOS debug 没有 Android 插件桥，\
         不编译这两处。release 构建两平台都不进入此分支。",
    ),
    (
        "src-tauri/src/commands/misc/logs.rs",
        "not(all(target_os = \"android\", debug_assertions))",
        1,
        IosSide::DiffersOnlyInDebug,
        "上一条的互补导出路径：仅 Android debug 使用原生分享；iOS 与全部 release\
         构建走文件导出。debug 时两平台反侧，release 时同侧。",
    ),
    (
        "src-tauri/src/commands/updater/app_update.rs",
        "all(target_os = \"android\", debug_assertions)",
        1,
        IosSide::DiffersOnlyInDebug,
        "debugReportAvailable 只表示 Android debug 的原生报告能力；iOS 没有这条桥，\
         release 两平台都返回 false。",
    ),
    (
        "src-tauri/src/commands/updater/uninstall.rs",
        "desktop",
        2,
        IosSide::WithAndroid,
        "应用自卸载（`plan_app_removal` 那条链）。两个移动平台同侧：应用的安装/卸载由商店与\
         系统持有，应用侧没有可执行的卸载动作。",
    ),
    (
        "src-tauri/src/commands/updater/uninstall.rs",
        "mobile",
        2,
        IosSide::WithAndroid,
        "上一条的孪生支（移动端诚实返不支持）。同侧同答。",
    ),
    (
        "src-tauri/src/commands/window.rs",
        "desktop",
        2,
        IosSide::WithAndroid,
        "窗口几何/装饰命令。移动端没有可被应用摆布的窗口对象，两个平台同侧。",
    ),
    (
        "src-tauri/src/commands/window.rs",
        "mobile",
        2,
        IosSide::WithAndroid,
        "上一条的孪生支。同侧同答。",
    ),
    (
        "src-tauri/src/lib.rs",
        "all(desktop, not(target_os = \"macos\"))",
        2,
        IosSide::WithAndroid,
        "非 macOS 桌面的窗口铬/特效。两个移动平台同侧（`desktop` 为 false）。",
    ),
    (
        "src-tauri/src/lib.rs",
        "all(desktop, not(target_os = \"windows\"), not(target_os = \"macos\"))",
        1,
        IosSide::WithAndroid,
        "Linux 专属桌面腿。两个移动平台同侧。",
    ),
    (
        "src-tauri/src/lib.rs",
        "all(desktop, target_os = \"windows\")",
        1,
        IosSide::WithAndroid,
        "Windows 专属桌面腿。两个移动平台同侧。",
    ),
    (
        "src-tauri/src/lib.rs",
        "desktop",
        8,
        IosSide::WithAndroid,
        "桌面专属插件与窗口装配（single-instance / autostart / 托盘锚点 / 窗口铬…）。\
         两个移动平台同侧。**其中 single-instance 那条的既有理由只写了 Android**\
         （「应用默认单进程 + tun fd 由 VpnService.prepare() 仲裁」）；iOS 的等价物是\
         「系统本就不允许同一 app 起第二个进程实例」+「`NEVPNManager` 侧的隧道单例」，\
         结论同、依据不同。答案不变，故不改代码。",
    ),
    (
        "src-tauri/src/lib.rs",
        "mobile",
        3,
        IosSide::WithAndroid,
        "🔵 **移动端装载面**：`window_config.url = \"mobile.html\"`、`mobile_entry_point`、\
         移动端托盘缺席。这一格是 `builder/inbounds.rs` 里 `strict_route` 那条 iOS 判据的\
         **依据本身** —— 「Android 与 iOS 共用同一份移动端 UI」正是靠这三处成立的\
         （`mobile = ios | android`）。两个平台同侧，且这一次连理由都真的相同。",
    ),
    (
        "src-tauri/src/tray.rs",
        "desktop",
        1,
        IosSide::WithAndroid,
        "`tray_present()` 的桌面实现。两个移动平台同侧。",
    ),
    (
        "src-tauri/src/tray.rs",
        "mobile",
        1,
        IosSide::WithAndroid,
        "`tray_present()` 的移动端孪生体（恒 false）。同侧同答。",
    ),
    (
        "src-tauri/src/tray/window.rs",
        "desktop",
        1,
        IosSide::WithAndroid,
        "托盘悬浮窗的桌面窗口属性。两个移动平台同侧。",
    ),
    (
        "src-tauri/src/runtime/update_popup.rs",
        "desktop",
        1,
        IosSide::WithAndroid,
        "更新弹窗的桌面窗口装饰（`WebviewWindowBuilder` 的桌面专属方法）。两个移动平台同侧。",
    ),
    // ── src-tauri：Android 专属实现面（iOS 反侧，且反侧是对的）──
    (
        "src-tauri/src/lib.rs",
        "target_os = \"android\"",
        3,
        IosSide::DiffersRight,
        "`mod android_tls`、`android_tls::assert_ready_before_any_https()`、\
         `builder.plugin(android_bridge::init())`。三处都是 **Android 专属实现**\
         （JNI 打进来的平台证书校验器、Tauri Android 插件），iOS 上不存在对应物 ⇒ \
         不编译/不注册是对的。\n\
         ⚠️ **但这不等于 iOS 不需要那件事**：平台证书校验器那条，iOS 侧的对应物是\
         `SecTrustEvaluate`/ATS 那条链，届时要写的是**一个新的 `target_os = \"ios\"` 分支**，\
         不是把这三处扩成 `any(android, ios)`。本表在那天会因「新增未登记谓词」而红，那正是想要的。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "target_os = \"android\"",
        31,
        IosSide::DiffersRight,
        "整条 Rust ↔ Kotlin/libbox 桥（`PluginHandle`、JNI 调用、`VpnService` 授权查询）。\
         iOS 不编译它是**对的**：这是 Android 的插件 API，iOS 上一个符号都不存在。\
         iOS 侧将来要的是**另一条桥**（`NEPacketTunnelProvider` + 扩展进程内 libbox），\
         那是一份新文件、新 cfg，不是把本文件扩成 `any(android, ios)`。\n\
         2026-09-06 由 21 处增至 25 处：W-09b（已装应用枚举）与 W-21（交系统安装器）各带来\
         两处（超时常量 + 命令腿）。两条腿的 iOS 侧答案与整条桥一致 —— `PackageManager` / \
         `FileProvider` / `ACTION_VIEW` 是 Android 的 API，iOS 上的对应物是 `LSApplicationWorkspace`\
         （私有）与 App Store 更新，形态完全不同，同样要的是新文件而不是 `any(android, ios)`。\n\
         2026-09-25 由 25 增至 29：系统起核对账（`systemStartStatus`）与开机自动连接开关读/写三条腿\
         + 一档超时常量。iOS 侧答案同整条桥（对应物是 `NEOnDemandRule` / 扩展进程，需要新文件）。\n\
         2026-09-25（系统备份）由 29 增至 31：「系统备份」开关读/写两条腿（Kotlin `PolarisBackupAgent`\
         的运行期闸门）。iOS 不走这一支是**对的**：Android Auto Backup 的对象在 iOS 上是 iCloud 备份的\
         `isExcludedFromBackup` 资源键，形态完全不同，要的是新文件而不是 `any(android, ios)`。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "all(target_os = \"android\", debug_assertions)",
        2,
        IosSide::DiffersOnlyInDebug,
        "collect_debug_diagnostics/share_debug_report 是 Android 插件的调试专用桥；\
         iOS debug 没有这些 JNI 入口，release 两平台都不编译。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "not(target_os = \"android\")",
        13,
        IosSide::DiffersRight,
        "本文件的非 Android 桩：每一处都返 `Err(\"Android 起核桥在本平台不存在\")` 或等价的\
         `Unknown`（含 `VpnAuthState` 那条 `cfg_attr(not(android), allow(dead_code))`）。\
         iOS 落这一侧是**诚实的**：今天它确实没有桥。桩返 Err 而不是假成功，\
         正是本仓「能力缺席就显式说不支持」的口径。\n\
         2026-09-06 由 6 处增至 10 处：W-09b / W-21 各带来一处非 Android 桩 + 一条\
         `cfg_attr(not(android), allow(dead_code))`（`InstalledApp` 与 `ApkHandoff` 的字段在\
         非 Android 上没有读点）。两处新桩同样返 `Err` 而不是「空表 / 假装交出去了」——\
         那两种假成功正是 `runtime/proxy/android_bridge/tests` 里那条变异锁钉住的东西。\n\
         2026-09-25 由 10 增至 11：`system_started_core_running` 的非 Android 桩恒 `false`\
         （本平台没有「系统替应用起的核」可对账；有变异锁 `non_android_never_reports_a_system_started_core`）。\n\
         2026-09-25（系统备份）由 11 增至 13：系统备份开关读/写的非 Android 桩，一律 `Err`、读侧不折成\
         `false`（变异锁 `non_android_system_backup_toggle_fails_honestly`）。",
    ),
    // ── src-tauri：iOS 落在错的一侧（本批不改，解锁条件见表头）──
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "target_os = \"android\"",
        7,
        IosSide::DiffersWrongToday,
        "🔴 起核链：核二进制占位、`spawn` 腿分叉、pid 提交、存活探测、内核闸门的\
         「无二进制也要跑」例外。iOS 落**非 Android 侧** ⇒ 去盘上解析一个 `sing-box` 可执行文件、\
         `spawn` 一个子进程、拿 pid 做存活探测。**iOS 上这三件事一件都不可能**\
         （沙箱不允许 fork/exec 任意可执行文件；核只能是扩展进程内的库）。\n\
         这与本批在枚举轴上写下的答案**直接矛盾**：`runtime::helper::platform_supported(Ios)` \
         为 false、`ProxyModeType::effective_on(Ios)` 恒 Tun、`core_has_builtin_cronet(\"ios\")` \
         的理由写的是「核是扩展进程内的库」。\n\
         **本批不改**：正确写法要引 iOS 侧核桥，而那不存在；改成 `any(android, ios)` 会让 iOS \
         引用 `android_bridge` 的 JNI 符号。解锁条件见本表表头。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "not(target_os = \"android\")",
        2,
        IosSide::DiffersWrongToday,
        "🔴 上一条的孪生侧：`let binary = binary_res?`（解析不到核就是终态 Err）与\
         「跑起来的核二进制对账」。iOS 落这一侧 ⇒ 起核第一步就会因为找不到 `sing-box` 而终态失败，\
         且会去挂一条恒 `Unobservable` 的二进制对账。同上，本批不改。",
    ),
    (
        "src-tauri/src/commands/proxy.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersWrongToday,
        "🔴 `kernel_probe_outbound`（custom 协议兼容性探测）：Android 侧改问 libbox `CheckConfig`\
         （进程内核，同起核闸门那条腿），iOS 落非 Android 侧 ⇒ 去解析一个不存在的核子进程二进制，\
         恒 failOpen 成「无法判定」（不谎报不支持，但探测形同虚设）。与起核那一族同一个解锁条件：\
         要有 iOS 侧核桥与 iOS 编译面，本批不改。",
    ),
    (
        "src-tauri/src/runtime/proxy/process_supervision.rs",
        "target_os = \"android\"",
        2,
        IosSide::DiffersWrongToday,
        "🔴 `kill_core()`：Android 侧「停核 = 请 VpnService 拆隧道」，iOS 落非 Android 侧 ⇒ \
         去 `take()` 一个不存在的 child 句柄、发一个不存在的 pid 的信号。\
         与枚举轴的答案矛盾（iOS 无核子进程）。同上，本批不改。\n\
         2026-09-25 由 1 增至 2：`cleanup_stale_cores` 的 Android 腿（停掉系统拉起、本运行时不认识的核）。\
         iOS 落非 Android 侧 ⇒ 去扫进程表找 `sing-box` 孤儿；而 iOS 的 Connect On Demand 同样会在应用\
         不在时拉起扩展进程里的核 —— 同一族债，解锁条件同表头。",
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "target_os = \"android\"",
        11,
        IosSide::DiffersWrongToday,
        "🔴 统计数据面的供数通道：Android 侧订 libbox 命令通道（进程内），\
         非 Android 侧连 clash API 的 gRPC 管理口。iOS 落**非 Android 侧** ⇒ 去连一个\
         `clash_api_port`，而 iOS 上核在扩展进程内、没有这个端口概念\
         （`stream_ready` / `stream_port` / `stream_still_valid` 三条判据全部读它）。\
         结果是五条 topic 全部无数据源，且**不报错**（流建不起来只表现为屏一直空着）。同上，本批不改。",
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "not(target_os = \"android\")",
        13,
        IosSide::DiffersWrongToday,
        "🔴 上一条的孪生侧，含 `use polaris_singbox_grpc::{…}` 这条**导入**。\
         iOS 落这一侧 ⇒ 编译期就会把整套 gRPC 客户端拖进 iOS 编译单元。同上，本批不改。",
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "any(target_os = \"android\", test)",
        3,
        IosSide::DiffersWrongToday,
        "🔴 `STATS_LIBBOX_COMMAND` / `CONNECTIONS_LIBBOX_COMMAND` / `TOPIC_SOURCE` 三个常量 ——\
         「哪条前端 topic 由哪个 libbox 通道供数」的声明。iOS 落**不在场**那一侧 ⇒ \
         它将来接进程内核时，这张对应表在 iOS 编译单元里根本不存在，而契约门 A6 的双向对拍\
         也就跟着不覆盖 iOS。同上，本批不改；改时这三条要与上面两条一起动。",
    ),
    // ── src-tauri：反侧，且本仓判不了哪一侧对 ──
    (
        "src-tauri/src/runtime/stats/gate.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersUndecided,
        "❓ 窗口可见性降流闸门。Android 侧的理由是「`tao` 的 `is_visible`/`is_minimized` \
         在本平台恒 false ⇒ 不可观测 ⇒ 按『始终有人在看』处理」。\
         **iOS 上那两个方法返什么，本仓没有任何事实**（构不出产物）。\
         不写成 `DiffersWrongToday`：那要求我知道 iOS 那一侧是错的，而我不知道 ——\
         「不知道」与「知道错」在本表里是两个不同的格子，混掉会让下一个人以为这条已经被判过。\
         代价面：判错成「不可观测」上限是多耗电；判错成「可观测但恒 false」是统计屏在前台也不刷新。",
    ),
    (
        "src-tauri/src/runtime/stats/gate.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersUndecided,
        "❓ 上一条的孪生侧（真的去问 `w.is_visible()` / `w.is_minimized()`）。同上，判不了。",
    ),
    // ── 2026-09-29：debug 原子可求值后，旧的提前失败不再遮住这些未登记站点 ──
    (
        "src-tauri/src/commands/misc/backup.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersUndecided,
        "Android 文件选择器不加扩展名过滤，否则 .polaris-backup 会被 MIME 转换隐藏。\
         iOS 选择器如何处理这两档过滤尚无平台事实；保留当前过滤但登记为待验证。",
    ),
    (
        "src-tauri/src/commands/speedtest.rs",
        "not(target_os = \"android\")",
        4,
        IosSide::DiffersWrongToday,
        "四处非 Android 腿使用桌面子进程测速、无认证本地代理与 helper 错误码。\
         iOS 核在扩展进程内，不能直接继承桌面起子进程的实现；需 iOS 原生测速宿主。",
    ),
    (
        "src-tauri/src/commands/speedtest.rs",
        "target_os = \"android\"",
        6,
        IosSide::DiffersWrongToday,
        "六处 Android 腿装配 libbox 测速、内存凭据和系统接口不支持的错误码。\
         iOS 不可使用 Android 插件，但落到桌面测速腿同样不成立；需要自己的原生桥。",
    ),
    (
        "src-tauri/src/commands/subscription.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersUndecided,
        "Android 文件选择器过滤会隐藏合法 .conf；iOS 系统选择器的 MIME/扩展名\
         映射尚未验证，当前沿用桌面过滤，须真机确认。",
    ),
    (
        "src-tauri/src/commands/system.rs",
        "not(target_os = \"android\")",
        2,
        IosSide::DiffersUndecided,
        "iOS 的 getifaddrs 系统调用可用，故共用非 Android 枚举在 API 层有依据；\
         沙箱下实际返回哪些物理网卡未真机确认（源码原注释已标明），故保留待验证。",
    ),
    (
        "src-tauri/src/commands/system.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersRight,
        "Android 从 ConnectivityManager 桥取可绑定接口；iOS 没有该 Android 插件，\
         不编译此腿是对的。iOS 当前改走 getifaddrs，覆盖范围另列待验证。",
    ),
    (
        "src-tauri/src/commands/updater/core_update.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersWrongToday,
        "换核事务拿桌面 legacy lease；Android 无可替换核文件故跳过。iOS 核同样在\
         扩展进程内，没有可由应用替换的独立核二进制，现有非 Android 换核链不适用。",
    ),
    (
        "src-tauri/src/commands/updater/shared.rs",
        "not(target_os = \"android\")",
        2,
        IosSide::DiffersWrongToday,
        "导入 core_paths 并解析可写核目录；iOS 随 app 扩展分发 libbox，\
         同样没有桌面可换的独立核文件，当前落桌面侧不成立。",
    ),
    (
        "src-tauri/src/commands/updater/shared.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersWrongToday,
        "Android 诚实拒绝独立核文件替换；iOS 也应拒绝，但当前落入非 Android\
         的磁盘内核路径，需接 iOS 原生发行模型。",
    ),
    (
        "src-tauri/src/lib.rs",
        "not(target_os = \"android\")",
        2,
        IosSide::DiffersWrongToday,
        "启动时播种/更新桌面核二进制的两处；iOS 核是扩展内 libbox，\
         不能把 app bundle 内的库当可写独立核替换。",
    ),
    (
        "src-tauri/src/runtime/geo_seed.rs",
        "any(target_os = \"android\", test)",
        1,
        IosSide::DiffersRight,
        "Android 预置资源经 APK BundledRules 落到 app data 的 bundled-geo；\
         iOS 不经过 Android Kotlin 资产物化函数，此分支不编译是对的。",
    ),
    (
        "src-tauri/src/runtime/geo_seed.rs",
        "not(target_os = \"android\")",
        2,
        IosSide::DiffersUndecided,
        "非 Android 候选从 exe 目录找随包 resources/data 并在 release 剔除源码仓；\
         iOS app bundle 的真实资源布局尚无构建产物验证，当前沿用此路径待核。",
    ),
    (
        "src-tauri/src/runtime/geo_seed.rs",
        "target_os = \"android\"",
        2,
        IosSide::DiffersRight,
        "Android 专属 APK bundled-geo 候选函数及其别名；iOS 不含 APK/Kotlin\
         BundledRules，必须走另一种随包资源布局。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "any(target_os = \"android\", test)",
        13,
        IosSide::DiffersRight,
        "确切主核回执/归属/legacy fence 的 Android 数据结构和校验器仅供\
         Kotlin 插件桥与跨平台单测；生产 iOS 不调用 Android 插件。iOS 须另建桥与回执。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "not(target_os = \"android\")",
        4,
        IosSide::DiffersRight,
        "确切停核、主核状态、归属、legacy fence 的非 Android 桩明确返 Err；\
         iOS 无 Android Kotlin 状态来源，不能把未知伪装成已关闭。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "target_os = \"android\"",
        17,
        IosSide::DiffersRight,
        "新增的接口枚举、确切主核状态/停核、归属与 legacy fence、瞬态测速/登录\
         都通过 Android Kotlin 插件；iOS 不存在这些 JNI 符号，需独立原生桥。",
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersUndecided,
        "非 Android 生产 lease 阻止旧路径与受管启动重叠；iOS 将来如何接入\
         受管起核尚未确定，当前沿用此准入闸，不能推断其最终归属。",
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersUndecided,
        "Android 此处不持桌面 config lease，归属在原生桥；iOS 扩展进程\
         将来应由谁持 lease 尚无实现，不能照搬 Android 的 Ok(None)。",
    ),
    (
        "src-tauri/src/runtime/proxy/route_replan.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersRight,
        "非 Android 同步网卡判据读取 getifaddrs；iOS 有该系统调用且此处\
         不持 Android JNI 桥。沙箱返回的网卡集合仍由 system.rs 条目待验证。",
    ),
    (
        "src-tauri/src/runtime/proxy/route_replan.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersRight,
        "Android selector 写事务持同步锁，不能 block_on 原生桥，故跳过此处\
         的同步枚举，由运行时异步观测和 socket hook 兜底；iOS 没有该桥。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "not(target_os = \"android\")",
        3,
        IosSide::DiffersWrongToday,
        "新增桌面核二进制解析与就绪后的磁盘文件自证；iOS 核为扩展进程内库，\
         没有这些文件/PID 对账的对象，延续本文件既有 iOS 债。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "target_os = \"android\"",
        4,
        IosSide::DiffersWrongToday,
        "新增 in-process 占位、managed 启动收据与自证旁路；iOS 不能用\
         Android 桥，但落桌面子进程侧也错误，延续既有 iOS 起核债。",
    ),
    (
        "src-tauri/src/runtime/speedtest.rs",
        "any(target_os = \"android\", test)",
        2,
        IosSide::DiffersRight,
        "Android 入站凭据类型和只在内存中注入凭据的 helper；生产 iOS\
         没有 Android transient host，不能直接复用此 Android 凭据协议。",
    ),
    (
        "src-tauri/src/runtime/speedtest.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersRight,
        "Android libbox 测速子模块的声明；iOS 不可编译 Android JNI 宿主，\
         应新增 iOS 平台实现而非共用此模块。",
    ),
    (
        "src-tauri/src/runtime/startup_tasks.rs",
        "not(target_os = \"android\")",
        2,
        IosSide::DiffersUndecided,
        "Android auto-connect 准入改由系统拉起主核/桥持有；iOS Connect On Demand\
         与 app 启动期的 lease 归属尚无实现，保留非 Android 两次检查并列待验证。",
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "any(target_os = \"android\", test)",
        1,
        IosSide::DiffersWrongToday,
        "新增 native stream detached_open 防 future 被丢弃；iOS 核同样不是\
         桌面 gRPC 子进程，现有非 Android 数据面无法提供统计，延续既有债。",
    ),
    (
        "src-tauri/src/runtime/tailscale_login_core.rs",
        "not(target_os = \"android\")",
        2,
        IosSide::DiffersWrongToday,
        "非 Android 解析独立 sing-box 可执行文件并装配 TokioSpawner；\
         iOS 扩展进程内 libbox 无可执行子进程，不能沿用此登录核。",
    ),
    (
        "src-tauri/src/runtime/tailscale_login_core.rs",
        "target_os = \"android\"",
        8,
        IosSide::DiffersWrongToday,
        "Android 瞬态登录用 JNI/libbox instance；iOS 不能编译 Android 桥，\
         但当前又落到桌面可执行文件路径，需独立进程内登录宿主。",
    ),
    // 2026-10-01：对齐出生/退出托管与 Android 调试桥的真实新增点。
    (
        "src-tauri/src/commands/android_batch_qa.rs",
        "all(target_os = \"android\", debug_assertions)",
        1,
        IosSide::DiffersOnlyInDebug,
        "batch QA 命令仅 Debug Android 调 JNI 调试桥；release 两平台都关闭，Debug iOS 没有 Android 桥，保持 disabled。",
    ),
    (
        "src-tauri/src/commands/android_batch_qa.rs",
        "not(all(target_os = \"android\", debug_assertions))",
        1,
        IosSide::DiffersOnlyInDebug,
        "batch QA 非 Debug Android 返回 disabled；release Android/iOS 同侧，Debug iOS 仍禁用而非引用不存在的 JNI。",
    ),
    (
        "src-tauri/src/commands/window.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersWrongToday,
        "restart 的桌面准备/提交在 iOS 也会编译，但 iOS 应由扩展进程管理退出；目前无 iOS exit ports/编译验证，不能将谓词简单扩到 JNI 路径，登记既有债。",
    ),
    (
        "src-tauri/src/commands/window.rs",
        "target_os = \"android\"",
        2,
        IosSide::DiffersWrongToday,
        "Android restart 使用 QuitState 与 Activity 生命周期；iOS 缺扩展退出宿主，当前走桌面退出腿是同一笔债；先有 iOS exit ports 才能改分派。",
    ),
    (
        "src-tauri/src/exit_lifecycle.rs",
        "not(target_os = \"android\")",
        7,
        IosSide::DiffersWrongToday,
        "DesktopExitPorts、prepare/commit、quit/final-exit 七处持桌面 Child/mesh 退出托管；iOS 不能沿用桌面核托管，缺 NE 扩展退出端口，登记债而不扩编 Android JNI。",
    ),
    (
        "src-tauri/src/exit_lifecycle.rs",
        "target_os = \"android\"",
        5,
        IosSide::DiffersWrongToday,
        "五处 Android 退出入口拒绝桌面 prepare/commit 并保留原 mobile cleanup；iOS 当前落桌面侧，需扩展进程 quit/cleanup 协议与编译验证后再实现。",
    ),
    (
        "src-tauri/src/lib.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersWrongToday,
        "新增桌面退出路由在 iOS 仍落桌面 child 托管，延续本文件既有债；须先提供 iOS exit ports，不能换谓词去引用 Android 桥。",
    ),
    (
        "src-tauri/src/lib.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersRight,
        "新增 Android Exit/ExitRequested 路由调用原 Android cleanup，专属 JNI 生命周期不能由 iOS 共用；非 Android 退出托管的 iOS 债由同文件反侧条目保留。",
    ),
    (
        "src-tauri/src/runtime/proxy.rs",
        "any(all(target_os = \"android\", debug_assertions), test)",
        3,
        IosSide::DiffersOnlyInDebug,
        "Android probe snapshot/session 输入及 QA/probe 模块只为调试 Android 桥，test 可编纯状态机；release iOS/Android 都无调试模块，Debug iOS 不引用 JNI。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "all(target_os = \"android\", debug_assertions)",
        4,
        IosSide::DiffersOnlyInDebug,
        "新增四处 JNI debug command/core probe 实现只在 Debug Android 存在；iOS 无该插件，release 两端均无调试入口。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "any(target_os = \"android\", test)",
        5,
        IosSide::DiffersRight,
        "新增五处 Android 主核/瞬态核的容量、出生与 drain 协议类型可在 test 编译纯状态机；iOS 不持 Android instance 身份，需自己的桥类型。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersRight,
        "新增非 Android debug command 桩只返回 unsupported；iOS 同样没有 Android JNI，不能假装可调用该调试命令。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_bridge.rs",
        "target_os = \"android\"",
        3,
        IosSide::DiffersRight,
        "新增三个 Android debug command/probe JNI 调用点仅属于 Android 插件；iOS 不可复用 Java 宿主，保持该支不编译。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_capacity.rs",
        "any(target_os = \"android\", test)",
        1,
        IosSide::DiffersRight,
        "Android 容量错误转 SpawnError 时使用 android-libbox 标识；test 只验错误投影，iOS 没有此容量宿主，须定义自己的错误转换。",
    ),
    (
        "src-tauri/src/runtime/proxy/android_probe_loan.rs",
        "all(target_os = \"android\", debug_assertions)",
        1,
        IosSide::DiffersOnlyInDebug,
        "debug_android_core_probe 消费 Debug Android 的会话/instance 出生凭据；release 两端均无此入口，iOS 不具 Android 实例身份。",
    ),
    (
        "src-tauri/src/runtime/proxy/hot_switch.rs",
        "any(all(target_os = \"android\", debug_assertions), test)",
        2,
        IosSide::DiffersOnlyInDebug,
        "hot switch 的 probe snapshot 输入/提取仅 Debug Android 或纯 test；release 无此调试字段，Debug iOS 没有 Android probe session。",
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "any(target_os = \"android\", test)",
        2,
        IosSide::DiffersUndecided,
        "Android lifecycle 的 legacy admission 与主核观察类型可在 test 验纯协议；iOS 的扩展主核 token/租约关系尚无事实，延续原未裁定债，需 iOS 宿主后判断。",
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "target_os = \"android\"",
        7,
        IosSide::DiffersUndecided,
        "新增七处 Android reconnect、主核 claim/drain/stop 用原 JNI instance；iOS 启停租约与扩展宿主尚无实现，延续本文件未裁定而不将它标为已支持。",
    ),
    (
        "src-tauri/src/runtime/proxy/process_supervision.rs",
        "any(all(target_os = \"android\", debug_assertions), test)",
        2,
        IosSide::DiffersOnlyInDebug,
        "stop observation 的 debug_probe_input 初始与历史 Unknown 构造仅 Debug Android/test；release 两平台均无该字段，Debug iOS 无 Android probe 会话。",
    ),
    (
        "src-tauri/src/runtime/proxy/process_supervision.rs",
        "target_os = \"android\"",
        2,
        IosSide::DiffersWrongToday,
        "新增主核停核 admission/legacy 权限在 Android 走实例凭据；iOS 仍落桌面 Child 观察，缺扩展主核精确出生/停止协议，延续既有债。",
    ),
    (
        "src-tauri/src/runtime/proxy/process_supervision/direct_stop.rs",
        "target_os = \"android\"",
        3,
        IosSide::DiffersWrongToday,
        "三个 direct-stop 入口仅拒绝 Android/已有 helper，iOS 当前仍进入桌面 Child custody；iOS 不提供此 Child，需 NE 扩展 birth/stop adapter 后再收紧平台闭集。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "all(target_os = \"android\", debug_assertions)",
        3,
        IosSide::DiffersOnlyInDebug,
        "启动后 Android probe 开始记录、发布 snapshot、deps UDP flag 仅 Debug Android；release 两平台同侧，Debug iOS 无 Android调试会话。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "not(all(target_os = \"android\", debug_assertions))",
        1,
        IosSide::DiffersOnlyInDebug,
        "release 或非 Android 的常规 switch snapshot 分支不持 Android probe 凭据；Debug iOS 仍走常规分支，尚无 Android 调试能力。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "not(target_os = \"android\")",
        1,
        IosSide::DiffersWrongToday,
        "新增运行中二进制自证仅在非 Android 跑独立可执行文件；iOS 落此臂仍是桌面路径债，须以扩展内 libbox 自证替换，不得只扩大 JNI cfg。",
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "target_os = \"android\"",
        4,
        IosSide::DiffersWrongToday,
        "新增 Android 启动 placeholder、自证免除及 API port ledger 使用进程内主核；iOS 仍落独立二进制/桌面端口逻辑，延续既有 iOS 启动债。",
    ),
    (
        "src-tauri/src/runtime/speedtest.rs",
        "not(target_os = \"android\")",
        21,
        IosSide::DiffersWrongToday,
        "21 处非 Android 测速持桌面 Child、PID kill/reap/config retirement；iOS 扩展不提供这些进程句柄，必须先有 iOS 瞬态实例桥与关闭凭据，登记债而不扩大 Android JNI 分支。",
    ),
    (
        "src-tauri/src/runtime/speedtest.rs",
        "target_os = \"android\"",
        9,
        IosSide::DiffersRight,
        "新增九处 Android 测速走 Android JNI 瞬态实例、drain 与 snapshot；iOS 没有该 Java 宿主，不能编 Android 腿。其错误继承桌面路径的事实在 not(android) 条目单独登记为债。",
    ),
    (
        "src-tauri/src/runtime/updater.rs",
        "target_os = \"android\"",
        1,
        IosSide::DiffersRight,
        "core-manifest 的 Android resolver 标志选择 libbox 资源事实，iOS 不应借 Android 资源；未支持目标不因此获得核资源或更新能力。",
    ),

];

// ── cfg 轴的取材与求值 ──

/// 已剥注释、**保留字符串**的代码面里，所有 `cfg(…)` / `cfg!(…)` / `cfg_attr(…)` 的**谓词**，
/// 折叠空白后返回。`cfg_attr` 只取第一个顶层实参（那才是谓词，后面是要挂的属性）。
fn cfg_predicates(code: &str) -> Vec<String> {
    let bytes = code.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while let Some(rel) = code.get(i..).and_then(|s| s.find("cfg")) {
        let at = i + rel;
        i = at + 3;
        // 词边界：`config` / `my_cfg` 不算。
        if at > 0 && is_ident_byte(bytes[at - 1]) {
            continue;
        }
        let mut j = at + 3;
        let is_attr = code[j..].starts_with("_attr");
        if is_attr {
            j += 5;
        }
        if bytes.get(j) == Some(&b'!') {
            j += 1;
        }
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if bytes.get(j) != Some(&b'(') {
            continue;
        }
        let Some(close) = balanced_paren_end(code, j) else {
            continue;
        };
        let inner = &code[j + 1..close];
        let pred = if is_attr {
            split_top_level_commas(inner)
                .into_iter()
                .next()
                .unwrap_or_default()
        } else {
            inner.to_owned()
        };
        out.push(fold(&pred));
        i = close;
    }
    out
}

/// `(` 起的配平右括号**字节**下标（理由同 [`balanced_end`]）。
fn balanced_paren_end(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (k, byte) in code.as_bytes().iter().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// 按**顶层**逗号切分（括号内的逗号不切）。
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (k, byte) in s.as_bytes().iter().enumerate() {
        match byte {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(s[start..k].to_owned());
                start = k + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].to_owned());
    parts
}

/// 谓词在给定环境下求值。**词表封闭**：不认识的原子返 `None`（调用方据此变红，不猜值）。
///
/// `test` 恒 false：本门判的是**生产构建面**，`cfg(test)` 那一侧是测试构型，不在射程内
/// （与 [`production_sources`] 排除 `tests/` 是同一条纪律）。
fn eval_cfg(pred: &str, env: CfgEnv) -> Option<bool> {
    let p = pred.trim();
    for (head, len) in [("not(", 4usize), ("any(", 4), ("all(", 4)] {
        if let Some(rest) = p.strip_prefix(head) {
            // 必须是整条谓词都被这个括号包住，否则 `any(a,b) == c` 这种形态会被误读。
            if balanced_paren_end(p, len - 1) != Some(p.len() - 1) {
                continue;
            }
            // 只剥**这一个**配平右括号。`trim_end_matches(')')` 会把嵌套谓词自己的右括号也一起
            // 剥掉（`all(desktop, not(target_os = "macos"))` → `not(` 少一个 `)`），
            // 结果是内层求值返 None、整条谓词被判成「词表外」而让门红在一个假原因上。
            let inner = &rest[..rest.len() - 1];
            let args: Vec<Option<bool>> = split_top_level_commas(inner)
                .iter()
                .filter(|a| !a.trim().is_empty())
                .map(|a| eval_cfg(a, env))
                .collect();
            if args.iter().any(Option::is_none) {
                return None;
            }
            let vals: Vec<bool> = args.into_iter().map(Option::unwrap).collect();
            return Some(match head {
                "not(" => !vals.first().copied().unwrap_or(false),
                "any(" => vals.iter().any(|v| *v),
                _ => vals.iter().all(|v| *v),
            });
        }
    }
    match p {
        "test" => Some(false),
        "debug_assertions" => Some(env.debug_assertions),
        "desktop" => Some(env.desktop),
        "mobile" => Some(env.mobile),
        _ => p
            .strip_prefix("target_os")
            .map(str::trim_start)
            .and_then(|r| r.strip_prefix('='))
            .map(str::trim)
            .and_then(|r| r.strip_prefix('"'))
            .and_then(|r| r.strip_suffix('"'))
            .map(|os| os == env.target_os),
    }
}

/// 两种构型各自比较 iOS/Android 是否同侧，避免把 debug 的答案误推广到 release。
fn cfg_side_profile(pred: &str) -> Option<(bool, bool)> {
    Some((
        eval_cfg(pred, IOS_ENV)? == eval_cfg(pred, ANDROID_ENV)?,
        eval_cfg(pred, IOS_DEBUG_ENV)? == eval_cfg(pred, ANDROID_DEBUG_ENV)?,
    ))
}

/// 求值器自身的正反双向自检 —— 它是本门第四列强制力的全部来源，坏掉就等于第四列没人看。
#[test]
fn cfg_evaluator_answers_both_ways() {
    // 正面：两个环境上各答对一次（只查一边会被「恒 true」骗过）。
    assert_eq!(eval_cfg("target_os = \"android\"", IOS_ENV), Some(false));
    assert_eq!(eval_cfg("target_os = \"android\"", ANDROID_ENV), Some(true));
    assert_eq!(eval_cfg("target_os = \"ios\"", IOS_ENV), Some(true));
    assert_eq!(eval_cfg("target_os = \"ios\"", ANDROID_ENV), Some(false));
    assert_eq!(
        eval_cfg("not(target_os = \"android\")", IOS_ENV),
        Some(true)
    );
    assert_eq!(
        eval_cfg("not(target_os = \"android\")", ANDROID_ENV),
        Some(false)
    );
    assert_eq!(
        eval_cfg("any(target_os = \"android\", test)", IOS_ENV),
        Some(false)
    );
    assert_eq!(eval_cfg("mobile", IOS_ENV), Some(true));
    assert_eq!(eval_cfg("desktop", IOS_ENV), Some(false));
    assert_eq!(eval_cfg("debug_assertions", IOS_ENV), Some(false));
    assert_eq!(eval_cfg("debug_assertions", IOS_DEBUG_ENV), Some(true));
    assert_eq!(eval_cfg("debug_assertions", ANDROID_ENV), Some(false));
    assert_eq!(eval_cfg("debug_assertions", ANDROID_DEBUG_ENV), Some(true));
    assert_eq!(
        cfg_side_profile("all(target_os = \"android\", debug_assertions)"),
        Some((true, false))
    );
    assert_eq!(
        eval_cfg("all(desktop, not(target_os = \"macos\"))", IOS_ENV),
        Some(false)
    );
    assert_eq!(
        eval_cfg("all(target_os = \"ios\", not(test))", IOS_ENV),
        Some(true)
    );
    // 反面（fail-closed）：词表外的原子必须返 None，而不是被当成 false 悄悄算出一个答案。
    for unknown in [
        "feature = \"x\"",
        "target_arch = \"arm\"",
        "windows",
        "unix",
        "any(target_os = \"ios\", feature = \"x\")",
        "not(unknown_build_mode)",
    ] {
        assert_eq!(
            eval_cfg(unknown, IOS_ENV),
            None,
            "{unknown:?} 被求值器猜出了一个值 —— 词表必须是封闭的，认不出来就该让门红"
        );
    }
}

/// cfg 轴登记门：新增即红 / 腐烂即红 / **写错边即红**。
#[test]
fn cfg_axis_platform_dispatch_is_registered() {
    let files = production_sources();
    assert!(
        files.len() >= 100,
        "只扫到 {} 个生产源文件 —— 取材面塌了",
        files.len()
    );

    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut unknown_atoms: Vec<String> = Vec::new();
    for (rel, path) in &files {
        let src = std::fs::read_to_string(path).expect("读源文件失败");
        // 剥注释、保留字符串：被判据物 `"android"` / `"ios"` 本身就是字符串字面量。
        let code = mask_comments(&src);
        for pred in cfg_predicates(&code) {
            if !mentions_mobile_axis(&pred) {
                continue;
            }
            // fail-closed：认不出来的原子一律让门红，不猜。
            if cfg_side_profile(&pred).is_none() {
                unknown_atoms.push(format!("{rel}\n      {pred}"));
                continue;
            }
            *found.entry((rel.clone(), pred)).or_default() += 1;
        }
    }

    assert!(
        unknown_atoms.is_empty(),
        "以下 cfg 谓词里出现了本门词表之外的原子：\n    {}\n\n\
         本门**不猜**：`eval_cfg` 的词表是封闭的（`target_os = \"…\"` / `desktop` / `mobile` / \
         `test` / `debug_assertions` / `not` / `any` / `all`）。要么给词表加上这个原子并在 \
         `cfg_evaluator_answers_both_ways` 里正反各钉一条，要么这条谓词根本不该落在移动轴上。",
        unknown_atoms.join("\n    ")
    );

    let mut registered: BTreeMap<(String, String), (usize, IosSide, &str)> = BTreeMap::new();
    for (path, pred, count, side, why) in CFG_REGISTRY {
        let key = ((*path).to_owned(), fold(pred));
        let slot = registered.entry(key).or_insert((0, *side, why));
        slot.0 += *count;
        assert_eq!(
            slot.1, *side,
            "{path} 的同一条谓词被登记成了两种 iOS 侧：{:?} / {side:?}",
            slot.1
        );
    }

    let reg_count = |key: &(String, String)| registered.get(key).map_or(0, |(c, _, _)| *c);
    // 未登记项直接把**算出来的两侧**打进消息里：作者不必自己去推 iOS 落哪边，
    // 少一次「登记时就理解错了」的机会。
    let unregistered: Vec<String> = found
        .iter()
        .filter(|(k, n)| reg_count(k) < **n)
        .map(|((p, s), n)| {
            let r = reg_count(&(p.clone(), s.clone()));
            let (release_same, debug_same) = cfg_side_profile(s).unwrap();
            format!("{p}  （实到 {n} 次，登记 {r} 次；release 同侧={release_same} / debug 同侧={debug_same}）\n      {s}")
        })
        .collect();
    let rotten: Vec<String> = registered
        .iter()
        .filter(|(k, (n, _, _))| found.get(*k).copied().unwrap_or(0) < *n)
        .map(|((p, s), (n, _, _))| {
            let f = found.get(&(p.clone(), s.clone())).copied().unwrap_or(0);
            format!("{p}  （登记 {n} 次，实到 {f} 次）\n      {s}")
        })
        .collect();

    // 第四列的机器复核：登记的「iOS 落哪一侧」必须与求值结果一致。
    let mislabeled: Vec<String> = registered
        .iter()
        .filter(|(k, _)| found.contains_key(*k))
        .filter_map(|((p, s), (_, side, _))| {
            let actual = cfg_side_profile(s).unwrap();
            let claimed = match side {
                IosSide::WithAndroid => (true, true),
                IosSide::DiffersOnlyInDebug => (true, false),
                IosSide::DiffersRight
                | IosSide::DiffersWrongToday
                | IosSide::DiffersUndecided => (false, false),
            };
            (actual != claimed).then(|| {
                format!(
                    "{p}  （登记 {side:?}={claimed:?}，实算 release/debug 同侧={actual:?}）\n      {s}"
                )
            })
        })
        .collect();

    // 切片自检 + 总账：把算出来的分布打出来。扫到 0 处也「没有未登记项」，那种绿没有信息量。
    let found_total: usize = found.values().sum();
    let mut by_side: BTreeMap<String, usize> = BTreeMap::new();
    for ((p, s), n) in &found {
        if let Some((_, side, _)) = registered.get(&(p.clone(), s.clone())) {
            *by_side.entry(format!("{side:?}")).or_default() += *n;
        }
    }
    println!(
        "[cfg-axis] 生产源文件 {} 个；移动轴 cfg 分派点实到 {found_total} 处 / {} 种 (文件,谓词) 对",
        files.len(),
        found.len()
    );
    for (side, n) in &by_side {
        println!("  {side}: {n} 处");
    }

    assert!(
        found_total >= CFG_SITE_FLOOR,
        "只扫到 {found_total} 处移动轴 cfg 分派点（下限 {CFG_SITE_FLOOR}）—— 取材面塌了\
         （路径 / 注释掩码 / `cfg_predicates` 任一坏掉都长这样）"
    );
    // 分类的反向对照：两类都必须真的出现过。全部落进同一类时，上面那条「写错边即红」的
    // 复核是**平凡成立**的 —— 分类器坏成恒真/恒假也照样全绿。
    assert!(
        by_side.get("WithAndroid").copied().unwrap_or(0) > 0,
        "一处「iOS 与 Android 同侧」都没算出来 —— 求值器或分类塌了"
    );
    assert!(
        by_side.values().sum::<usize>() > by_side.get("WithAndroid").copied().unwrap_or(0),
        "全部都算成了「同侧」—— 这一轴的分叉正是本门要找的东西，一处都没有只可能是求值器塌了"
    );

    assert!(
        unregistered.is_empty(),
        "以下 **cfg 轴**平台分派点没有登记：\n    {}\n\n\
         编译期分派不是「新平台落进兜底」，而是「新平台落进**另一个平台的那一半**」——\
         `not(target_os = \"android\")` 在 iOS 上是 true，iOS 于是拿到桌面那一份代码，\
         而枚举轴上写下的 iOS 答案说的是「与 Android 同形」。两条轴互相矛盾，\
         而编译器与前三张登记表**一句话都不说**。\n\
         故每一处都必须在 `CFG_REGISTRY` 里写下「iOS 落哪一侧、那一侧对不对、为什么」。\
         第四列会被本门自己复核，写错边当场红。",
        unregistered.join("\n    ")
    );
    assert!(
        rotten.is_empty(),
        "以下 cfg 登记条目在源码里已经找不到（谓词被改动或删除）：\n    {}\n\n\
         登记表不是垃圾桶：谓词变了就要重新答一次「iOS 落哪一侧」。",
        rotten.join("\n    ")
    );
    assert!(
        mislabeled.is_empty(),
        "以下 cfg 登记条目的**第四列写错了边**：\n    {}\n\n\
         这一列不是审计意见，是可计算的事实：本门按 `target_os=\"ios\", mobile, !desktop` \
         求值，与 Android 环境比对。写错边说明登记时对这条谓词的理解是错的，\
         那么第五列的理由多半也建在同一个误解上 —— 两列一起重看。",
        mislabeled.join("\n    ")
    );
}

// ── 三态判决的防抹除门（2026-09-06）──

/// `IosSide` 四态的**总账**：(判决名, 该判决下 cfg 分派点的出现次数合计)。按判决名升序。
///
/// 🔴 **第一列刻意是字符串字面量，不是 `IosSide::…`**。这是本条判据的机制本身：
/// 一次 `sed 's/IosSide::DiffersWrongToday/IosSide::DiffersRight/'` 会同时改掉登记表里的 38 处，
/// 但改不到这里的 `"DiffersWrongToday"` —— 算出来的总账与本表当场对不上而红。
/// 若把这里也写成 `IosSide::DiffersWrongToday`，判据就和被判对象同源，同一条 sed 会把两侧一起搬走，
/// 门照样全绿 —— 那就是「判据被自己污染」的形态。
/// 2026-09-06（W-09b / W-21）：`DiffersRight` 由 32 增至 40。**增量全部落在
/// `android_bridge.rs` 一个文件里**（`target_os = "android"` 21→25、`not(…)` 6→10），
/// 即两条新的 Android 专属腿（已装应用枚举 / 交系统安装器）及其非 Android 桩。
/// 三条债的格子（`DiffersWrongToday` 38 / `DiffersUndecided` 2 / `WithAndroid` 41）**一个都没动** ——
/// 本批没有把任何一条既有的 iOS 债重判成「对的」，只新增了八处本来就与 Android 专属实现同形的分叉。
/// 2026-09-25（系统起核 / 开机自动连接）：`DiffersRight` 40→49、`DiffersWrongToday` 37→38、
/// `WithAndroid` 41→40。逐格：`android_bridge.rs` 新增 4 处 android + 1 处非 android 桩（+5 Right）；
/// `autostart.rs` 原 `mobile` ×2（WithAndroid）拆成 `target_os = "android"` ×2 与
/// `all(mobile, not(android))` ×2（+4 Right、−2 WithAndroid），`desktop` 2→3（+1 WithAndroid，
/// `use tauri::Manager` 挂上桌面 cfg）；`process_supervision.rs` 孤儿清扫的 Android 腿 +1 **债**
/// （iOS 落进程表扫描那一侧，与同文件既有那一处同族）。**没有**任何既有债被重判成「对的」。
/// 2026-09-25（系统备份开关）：`DiffersRight` 49→53 —— `android_bridge.rs` 新增读/写两条 android 腿 +
/// 两处非 android 桩。三条债的格子一个都没动。
/// 2026-09-29：识别 `debug_assertions` 后，旧的未知原子提前失败不再遮住随后一批 cfg 点。
/// 按源码逐项登记后的总账是 227 处：debug 专属反侧 6、已核对反侧 96、待验证 12、
/// iOS 现状已知不适用 73、同侧 40；下表与具名债清单分别防数量和位置漂移。
/// 2026-10-01：新增 95 处实际分派并逐点登记；所有既有判决/债条目保留。
/// 新增债仍是债；本次未取得 iOS 编译或真机收据，未还清任何 iOS 支持债。
const IOS_SIDE_CENSUS: &[(&str, usize)] = &[
    ("DiffersOnlyInDebug", 24),
    ("DiffersRight", 117),
    ("DiffersUndecided", 21),
    ("DiffersWrongToday", 120),
    ("WithAndroid", 40),
];

/// 「债」的两个格子。同样只写名字，不写 `IosSide::`，理由同 [`IOS_SIDE_CENSUS`]。
///
/// 为什么 `DiffersUndecided` 也算债：它与 `DiffersWrongToday` 的区别是认识论的（不知道 vs 知道错），
/// 但对「将来必须有人回来重判一次」这件事两者同类 —— 一个被改成 `DiffersRight` 就同样是债被抹掉。
const IOS_DEBT_VERDICTS: [&str; 2] = ["DiffersWrongToday", "DiffersUndecided"];

/// 债的**具名清单**：(仓库相对路径, 折叠后的 cfg 谓词, 出现次数)。按元组升序。
///
/// 光有总账不够：把一处 `DiffersWrongToday` 改成 `DiffersRight`、同时把一处 `DiffersRight` 改成
/// `DiffersWrongToday`，两个计数都不变 ⇒ 总账那条判据平凡通过。具名清单堵的正是这个等量对换。
const IOS_DEBT_SITES: &[(&str, &str, usize)] = &[
    (
        "src-tauri/src/commands/misc/backup.rs",
        "not(target_os = \"android\")",
        1,
    ),
    (
        "src-tauri/src/commands/proxy.rs",
        "target_os = \"android\"",
        1,
    ),
    (
        "src-tauri/src/commands/speedtest.rs",
        "not(target_os = \"android\")",
        4,
    ),
    (
        "src-tauri/src/commands/speedtest.rs",
        "target_os = \"android\"",
        6,
    ),
    (
        "src-tauri/src/commands/subscription.rs",
        "not(target_os = \"android\")",
        1,
    ),
    (
        "src-tauri/src/commands/system.rs",
        "not(target_os = \"android\")",
        2,
    ),
    (
        "src-tauri/src/commands/updater/core_update.rs",
        "not(target_os = \"android\")",
        1,
    ),
    (
        "src-tauri/src/commands/updater/shared.rs",
        "not(target_os = \"android\")",
        2,
    ),
    (
        "src-tauri/src/commands/updater/shared.rs",
        "target_os = \"android\"",
        1,
    ),
    (
        "src-tauri/src/commands/window.rs",
        "not(target_os = \"android\")",
        1,
    ),
    (
        "src-tauri/src/commands/window.rs",
        "target_os = \"android\"",
        2,
    ),
    (
        "src-tauri/src/exit_lifecycle.rs",
        "not(target_os = \"android\")",
        7,
    ),
    (
        "src-tauri/src/exit_lifecycle.rs",
        "target_os = \"android\"",
        5,
    ),
    ("src-tauri/src/lib.rs", "not(target_os = \"android\")", 1),
    ("src-tauri/src/lib.rs", "not(target_os = \"android\")", 2),
    (
        "src-tauri/src/runtime/geo_seed.rs",
        "not(target_os = \"android\")",
        2,
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "any(target_os = \"android\", test)",
        2,
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "not(target_os = \"android\")",
        1,
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "target_os = \"android\"",
        1,
    ),
    (
        "src-tauri/src/runtime/proxy/lifecycle.rs",
        "target_os = \"android\"",
        7,
    ),
    (
        "src-tauri/src/runtime/proxy/process_supervision.rs",
        "target_os = \"android\"",
        2,
    ),
    (
        "src-tauri/src/runtime/proxy/process_supervision.rs",
        "target_os = \"android\"",
        2,
    ),
    (
        "src-tauri/src/runtime/proxy/process_supervision/direct_stop.rs",
        "target_os = \"android\"",
        3,
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "not(target_os = \"android\")",
        1,
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "not(target_os = \"android\")",
        2,
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "not(target_os = \"android\")",
        3,
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "target_os = \"android\"",
        4,
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "target_os = \"android\"",
        4,
    ),
    (
        "src-tauri/src/runtime/proxy/startup.rs",
        "target_os = \"android\"",
        7,
    ),
    (
        "src-tauri/src/runtime/speedtest.rs",
        "not(target_os = \"android\")",
        21,
    ),
    (
        "src-tauri/src/runtime/startup_tasks.rs",
        "not(target_os = \"android\")",
        2,
    ),
    (
        "src-tauri/src/runtime/stats/gate.rs",
        "not(target_os = \"android\")",
        1,
    ),
    (
        "src-tauri/src/runtime/stats/gate.rs",
        "target_os = \"android\"",
        1,
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "any(target_os = \"android\", test)",
        1,
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "any(target_os = \"android\", test)",
        3,
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "not(target_os = \"android\")",
        13,
    ),
    (
        "src-tauri/src/runtime/stats/source.rs",
        "target_os = \"android\"",
        11,
    ),
    (
        "src-tauri/src/runtime/tailscale_login_core.rs",
        "not(target_os = \"android\")",
        2,
    ),
    (
        "src-tauri/src/runtime/tailscale_login_core.rs",
        "target_os = \"android\"",
        8,
    ),
];

/// 已承认的债不许被静默抹掉：`IosSide` 判决的**总账 + 具名清单**双向对拍。
///
/// # 这条门守的不是「答案对不对」
///
/// 那 38 处 iOS 落错侧的分派点，本仓今天**判不了**改完对不对（构不出 iOS 产物，见 [`CFG_REGISTRY`]
/// 表头）。所以本门不试图验证第四列的语义 —— 那一列里「反侧」这半已经由
/// [`cfg_axis_platform_dispatch_is_registered`] 的 `mislabeled` 机器复核了。
///
/// 本门守的是另一件事：**这笔债有没有被人悄悄抹掉**。
///
/// 缺口是可计算的：`mislabeled` 只区分「同侧 / 反侧」两档，而 `DiffersRight`、
/// `DiffersWrongToday`、`DiffersUndecided` 三个变体在它眼里**完全等价**（都属于「反侧」）。
/// 于是一次全局替换就能把 38 条「iOS 今天走错了路」改写成 38 条「iOS 走的是对的路」，
/// 而枚举 / 比较 / 字符串 / cfg 四条轴的门**一条都不会红**：
/// 谓词没动、出现次数没动、同侧反侧的分类没动，动的只是那句人写的判决。
/// 那正是这批改动里唯一「以后必须有人回来还」的东西。
///
/// # 判据形态为什么是「计数 + 具名清单」两条
///
/// · 计数：抓整类改写（全局 sed、删条目、改 count）。
/// · 具名清单：抓等量对换（拿一处 `DiffersRight` 换一处 `DiffersWrongToday`，计数不变）。
///
/// 两条的期望值都写在本文件的 `const` 里，且第一列是**字符串**而非 `IosSide::` 路径 ——
/// 判据与被判对象不同源，同一条 sed 搬不动两侧。
///
/// # 这条门红了该怎么办
///
/// 它红 = 有人动了判决。**不要**顺手把上面两个 `const` 改成新值让它变绿；
/// 先回答「这 N 处的 iOS 侧事实是从哪来的」——今天唯一能提供该事实的是 iOS 编译面 + 真机，
/// 两者本仓都没有。真还清了债再来改期望值，改动会在 diff 里逐条可读。
#[test]
fn ios_side_verdicts_cannot_be_silently_rewritten() {
    // 取材面自检：登记表被清空 / 被大幅缩水时，下面两条对拍会「两边都空」而平凡通过。
    assert!(
        CFG_REGISTRY.len() >= 25,
        "CFG_REGISTRY 只剩 {} 条 —— 登记表本身塌了，本门的两条对拍会退化成空对空",
        CFG_REGISTRY.len()
    );

    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    let mut debt: Vec<(String, String, usize)> = Vec::new();
    for (path, pred, count, side, _why) in CFG_REGISTRY {
        // `{side:?}` 把变体名取成**运行期字符串**，与上面两张表的字面量在同一个域里比 ——
        // 这一步就是「判据与被判对象不同源」的落点。
        let label = format!("{side:?}");
        *census.entry(label.clone()).or_default() += *count;
        if IOS_DEBT_VERDICTS.contains(&label.as_str()) {
            debt.push(((*path).to_owned(), fold(pred), *count));
        }
    }
    debt.sort();

    // 正面对照：两个债格子都必须真的出现过。只写「计数要等于 X」会被「什么都没发生」骗过 ——
    // 债被整类删空时，下面的 deepEqual 固然会红，但红在哪一格读不出来，先在这里点名。
    for want in IOS_DEBT_VERDICTS {
        assert!(
            census.contains_key(want),
            "总账里一条 {want} 都没有了 —— 这笔债不会自己消失，只可能是判决被改写或条目被删。\
             真还清了债，改期望值之前先在 CFG_REGISTRY 表头写下 iOS 侧事实的来源"
        );
    }

    let want_census: BTreeMap<String, usize> = IOS_SIDE_CENSUS
        .iter()
        .map(|(k, v)| ((*k).to_owned(), *v))
        .collect();
    assert_eq!(
        want_census.len(),
        IOS_SIDE_CENSUS.len(),
        "IOS_SIDE_CENSUS 里有重名判决 —— 多半是有人把某个变体名整体替换掉了（两条撞成同一个 key）"
    );
    assert_eq!(
        census, want_census,
        "\n`IosSide` 判决总账变了：\n  实算 {census:?}\n  登记 {want_census:?}\n\n\
         四条轴的门都不会为这件事变红（谓词、次数、同侧/反侧分类都没动），本门是唯一一条。\
         改期望值之前先读本测的文档注释。"
    );

    let want_debt: Vec<(String, String, usize)> = IOS_DEBT_SITES
        .iter()
        .map(|(p, s, n)| ((*p).to_owned(), fold(s), *n))
        .collect();
    let mut want_debt_sorted = want_debt.clone();
    want_debt_sorted.sort();
    assert_eq!(
        want_debt, want_debt_sorted,
        "IOS_DEBT_SITES 没有按元组升序写 —— 顺序是本表的可读性约定，比对前两侧都要有序"
    );
    assert_eq!(
        debt, want_debt,
        "\n债的具名清单变了：\n  实算 {debt:#?}\n  登记 {want_debt:#?}\n\n\
         等量对换（一处 DiffersRight ⇄ 一处 DiffersWrongToday）不会动总账，只有本条看得见。"
    );

    println!(
        "[ios-verdicts] 判决总账 {census:?}；债的具名清单 {} 条（合计 {} 处）",
        debt.len(),
        debt.iter().map(|(_, _, n)| *n).sum::<usize>()
    );
}

/// 谓词提到移动轴了吗（`target_os = "android"` / `target_os = "ios"` / `desktop` / `mobile`）。
///
/// 本门的定义域。`target_os = "macos"` 之类**刻意不在内**：iOS 与 Android 在那上面同落
/// 「都不是」一侧，不产生本轴要找的分叉（见本节头注 §取材面）。
fn mentions_mobile_axis(pred: &str) -> bool {
    pred.contains("target_os = \"android\"")
        || pred.contains("target_os = \"ios\"")
        || has_bare_word(pred, "desktop")
        || has_bare_word(pred, "mobile")
}

/// 词边界包含：`mobile` 命中，`mobile_entry_point` 不命中。
fn has_bare_word(hay: &str, word: &str) -> bool {
    let bytes = hay.as_bytes();
    let mut i = 0usize;
    while let Some(rel) = hay.get(i..).and_then(|s| s.find(word)) {
        let at = i + rel;
        let end = at + word.len();
        let pre_ok = at == 0 || !is_ident_byte(bytes[at - 1]);
        let post_ok = end >= bytes.len() || !is_ident_byte(bytes[end]);
        if pre_ok && post_ok {
            return true;
        }
        i = at + 1;
    }
    false
}
