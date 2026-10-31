//! [`HelperManager`] —— helper 安装/卸载/启动/停止生命周期管理。
//!
//! ## 职责（移植自 上游 `HelperManager.ts`）
//!
//! 三平台 helper 的**生命周期决策**：判定是否已装/就绪、装/卸路径、proto 版本探测。
//! 实际系统操作（launchd bootstrap / systemd enable / SCM install）经 [`SysOps`] trait 抽象，
//! 生产注入真 syscall，测试 mock —— 满足「不触碰宿主」纪律。
//!
//! ## 三平台安装路径（移植锚点）
//!
//! - **macOS**：`/Library/PrivilegedHelperTools/com.polaris.helper`（二进制）+
//!   `/Library/LaunchDaemons/com.polaris.helper.plist`（plist）+ launchd bootstrap。
//!   （上游 `HelperManager.ts:30,33-34`）
//! - **Linux**：`/usr/local/lib/polaris/helper`（二进制）+ `polaris-helper.service`（systemd unit）+ systemctl。
//! - **Windows**：`C:\Program Files\Polaris\helper.exe`（二进制）+ SCM 服务注册（`PolarisHelper`）。
//!
//! ## 状态模型（移植自 `HelperStatus`，Polaris shared/types）
//!
//! [`HelperStatus`] 覆盖：installed / ready / version / loaded / needsRepair / upgradeable。
//! ready = installed ∧ token 有效 ∧ ping 回 proto ≥ [`MIN_USABLE_PROTO`]（上游 `HelperManager.ts:183`）。
//!
//! ## 移植纪律
//!
//! 1. 路径常量 + plist/unit 生成是跨平台纯逻辑（可测）。
//! 2. launchd/systemd/SCM 操作 trait 抽象（[`SysOps`]），测试 mock。
//! 3. `forbid(unsafe_code)`。

use crate::client::HelperClient;
#[cfg(test)]
use crate::client::{ClientError, Connector};
use crate::token;
use polaris_helper_proto::response::{Pong, ResponseKind};
use polaris_helper_proto::{Platform, Request, Response};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// helper 服务标签（移植自 上游 `LABEL = 'com.polaris.helper'`，HelperManager.ts:31）。
/// Polaris 改名 com.polaris.helper。
pub const SERVICE_LABEL: &str = "com.polaris.helper";

/// Runtime readiness requires the executor-owned admission semantics. Stop and
/// explicit administrator upgrade/cleanup remain available to older helpers.
pub const MIN_USABLE_PROTO: u32 = polaris_helper_proto::proto_version::MIN_TRUSTED_EXECUTION;

/// 三平台安装路径集合（移植自 Polaris 路径常量）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPaths {
    /// helper 二进制目标路径（root 拥有）。
    pub binary: PathBuf,
    /// 服务描述符**文件**：mac = plist，linux = systemd unit。
    ///
    /// **Windows 恒 `None`** —— 其服务定义活在 SCM 里，磁盘上没有对应文件。早先这里塞了一个
    /// 从不存在的 `helper-service.yml` 占位路径，而 [`HelperManager::is_installed`] 又拿它做
    /// 存在性判定 ⇒ Windows 上恒判「未安装」。「已安装」的第二条证据改由
    /// [`SysOps::service_exists`] 查 SCM 提供。
    pub descriptor: Option<PathBuf>,
    /// socket / pipe 路径（client 连接目标）。
    pub socket: PathBuf,
    /// 受保护核目录（sing-box 锁定路径）。**三平台都有消费者**：mac/linux 烧进 plist/unit 的
    /// `--coredir`，win（P4 起）由 `build_win_install_script`（本 crate 私有）派生服务 ImagePath 的 `--singbox`
    /// 并在此播种受保护核 —— 它是 Windows 受保护核路径在**安装脚本与 app 侧**的唯一来源。
    ///
    /// **helper 侧不读它**：daemon 只拿 `--support`，核目录由它自己的 `derived_core_dir()`
    /// （`<support>\core`）独立派生。两份字面量靠 [`WIN_CORE_DIR_NAME`] 的等值门收敛，
    /// 见 `manager/tests` 的 `win_core_dir_name_agrees_with_the_helper_side_derivation`。
    pub core_dir: PathBuf,
    /// 服务标识：mac = launchd label，linux = systemd unit 名，**win = SCM 服务名**。
    ///
    /// 早先 `is_loaded`/`start`/`stop` 一律硬传 [`SERVICE_LABEL`]（`com.polaris.helper`），
    /// 而 Windows 装出来的服务叫 `PolarisHelper` ⇒ `sc query/start/stop` 全部打空。
    pub service_label: &'static str,
}

/// 三平台安装路径工厂（移植自 Polaris mac 常量，扩展 linux/win）。
impl InstallPaths {
    /// macOS 路径（移植自 `HelperManager.ts:30,33-35`）。
    #[must_use]
    pub fn mac() -> Self {
        Self {
            // ⚠️ **整串 `from` 而不是 `from(dir).join(leaf)`**（2026-08-05，Windows CI 血证）：
            // 这些是要写进 **macOS shell 脚本**的路径字面量，分隔符必须恒为 `/`。而 `join` 用的是
            // **宿主平台**的分隔符 —— 本 crate 三平台都编译，在 Windows 上会产出
            // `/Library/PrivilegedHelperTools\com.polaris.helper` 这种混合形态。
            // 生产只在 mac 跑，所以不是线上缺陷；但它让 Windows 腿的测试恒红，而 Windows 腿恒红
            // ⇒ rust-cache 的 post 步骤（save-if: success()）永不保存 ⇒ 每轮都冷编译 20 分钟。
            binary: PathBuf::from(format!("/Library/PrivilegedHelperTools/{SERVICE_LABEL}")),
            descriptor: Some(PathBuf::from(format!(
                "/Library/LaunchDaemons/{SERVICE_LABEL}.plist"
            ))),
            socket: PathBuf::from("/Library/Application Support/Polaris/helper.sock"),
            core_dir: PathBuf::from("/Library/Application Support/Polaris/core"),
            service_label: SERVICE_LABEL,
        }
    }

    /// Linux 路径（systemd unit + unix socket + SO_PEERCRED）。
    #[must_use]
    pub fn linux() -> Self {
        Self {
            binary: PathBuf::from("/usr/local/lib/polaris/helper"),
            descriptor: Some(PathBuf::from(format!(
                "/etc/systemd/system/{LINUX_SERVICE_NAME}"
            ))),
            socket: PathBuf::from("/run/polaris/helper.sock"),
            core_dir: PathBuf::from("/usr/local/lib/polaris/core"),
            service_label: LINUX_SERVICE_NAME,
        }
    }

    /// Windows 路径（SCM 服务 + 命名管道）。
    #[must_use]
    pub fn win() -> Self {
        Self {
            // 必须与 `build_win_install_script` 的落点逐字一致 —— 二者曾各拼各的（状态探测查
            // `C:\Program Files\Polaris\helper.exe`，脚本装到 `C:\ProgramData\Polaris\...`），
            // 分叉导致 Windows 上 `is_installed` 恒 false。现同源于 WIN_SUPPORT_DIR/WIN_HELPER_EXE，
            // 由 `win_install_script_targets_the_same_paths_status_probes` 钉死。
            // 字面量拼接而非 `join`：`PathBuf::join` 用**宿主**分隔符，在 Linux/mac 上构造
            // Windows 路径会得到 `C:\ProgramData\Polaris/polaris-helper.exe` 这种混合形态，
            // 与脚本里的 `format!(r"{support}\{WIN_HELPER_EXE}")` 对不上。
            binary: PathBuf::from(format!(r"{WIN_SUPPORT_DIR}\{WIN_HELPER_EXE}")),
            // Windows 服务定义在 SCM，磁盘无描述符文件 → is_installed 走 SysOps::service_exists
            // 单证据（W17：support 目录 ACL 锁拒未提权 stat，文件证据不可用；取舍见 is_installed 头注）。
            descriptor: None,
            socket: PathBuf::from(WIN_PIPE_NAME),
            // P4 起有消费者：[`build_win_install_script`] 在此建目录、播种核、锁 ACL，并把
            // `<core_dir>\sing-box.exe` 拼成服务 ImagePath 的 `--singbox`；app 侧
            // `HelperRuntime::protected_core_dir_path` 对账的也是同一个值。同样字面量拼接
            // 而非 `join`（理由见上面 `binary`）。
            core_dir: PathBuf::from(format!(r"{WIN_SUPPORT_DIR}\{WIN_CORE_DIR_NAME}")),
            service_label: WIN_SERVICE_NAME,
        }
    }

    /// 按平台取路径。
    ///
    /// # `Platform::Android` / `Platform::Ios` 与 `Platform::Other` 同臂的理由（2026-09-04 K10、2026-09-06）
    ///
    /// 本 crate 整条链的入口是 `runtime/helper.rs::platform_supported`，它对这三个变体都恒 `false`
    /// ⇒ 生产路径上 `HelperManager` 在这些平台上**不会被构造**，本函数也就不会被调到。臂里给
    /// linux 路径是**类型完整性占位**，不是「Android 用 linux helper」——Android 上根本没有 helper
    /// 二进制、没有 systemd、没有 pkexec。
    ///
    /// **iOS 的理由更硬一档，且与 Android 不是同一句话**（本 crate 全部 7 处 `Ios` 臂共用本段，
    /// 各站点只写该处特有的那半句）：Android 上「没有 helper」是本仓的构建与分发选择——理论上
    /// 一台 root 过的设备上存在别的形态；iOS 上则是**平台不允许存在这样的东西**——沙箱里没有
    /// 常驻 root 守护进程这个概念，没有 launchd/systemd/SCM 等价的用户可注册面，也没有任何
    /// 应用可用的提权通道（iOS 的「提权」对应物是系统按 entitlement 授予 NE 扩展的隧道权限，
    /// 那条链整个不经过本 crate）。⇒ 本臂在 iOS 上不是「暂未实现」，是**结构性不可达**。
    ///
    /// 为什么不 `unreachable!()`：本函数是 `const`-friendly 的纯路径映射，被单测按全变体扫过；
    /// 一个 panic 臂会把「谁不小心在 Android 上构造了 manager」从一条静默的无用路径变成一次崩溃，
    /// 而那条路径的正确处置是在**入口**（`platform_supported`）拦，不是在末端炸。入口那条判据由
    /// `runtime/helper/tests/mod.rs` 的全变体断言钉死。
    #[must_use]
    pub fn for_platform(platform: Platform) -> Self {
        match platform {
            Platform::Mac => Self::mac(),
            // 路径映射：iOS 同 Android/Other 取 linux 占位路径。本处特有的那半句 —— iOS 上
            // `/Library/...`、`/etc/systemd/...` 这类绝对路径在应用沙箱里连命名空间都不存在，
            // 占位值取哪一套都等价，取 linux 只为与既有臂同形。
            Platform::Linux | Platform::Other | Platform::Android | Platform::Ios => Self::linux(),
            Platform::Win => Self::win(),
        }
    }
}

/// helper 状态快照（移植自 上游 `HelperStatus`，shared/types）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelperStatus {
    /// 是否已安装（二进制 + 描述符存在）。
    pub installed: bool,
    /// 是否就绪（installed + token 有效 + ping proto ≥ MIN_USABLE）。
    pub ready: bool,
    /// 报告的 protoVersion（ping 回）。
    pub version: Option<u32>,
    /// helper 自报构建身份；旧 helper 的 pong 不带该字段。
    pub build_id: Option<String>,
    /// 是否可升级（ready 且协议落后）；构建身份不用于覆盖较新 helper。
    pub upgradeable: bool,
    /// 是否需修复（installed 但 !ready，如 token 丢失 / proto 过旧）。
    pub needs_repair: bool,
}

/// helper 生命周期管理器。
///
/// 持有平台 + 安装路径 + connector（ping 探测）+ 系统操作抽象。
/// install/uninstall/start/stop 是**决策 + 委托** —— 真系统操作走 [`SysOps`]。
pub struct HelperManager {
    platform: Platform,
    paths: InstallPaths,
    token_path: PathBuf,
    sysops: Box<dyn SysOps>,
}

/// 系统操作抽象（移植自 Polaris 的 spawn(launchctl) / systemctl / SCM 操作）。
///
/// 把「文件存在检查 / launchd bootstrap / systemctl enable / SCM install / 启停服务」抽成 trait，
/// 让 [`HelperManager`] 的生命周期决策可在 Linux 上测（mock [`SysOps`]）。
pub trait SysOps: Send {
    /// 检查路径是否存在（移植自 上游 `fs.existsSync`，HelperManager.ts:202）。
    fn exists(&self, path: &Path) -> bool;

    /// 启动 helper daemon（mac: launchctl bootstrap；linux: systemctl start；win: sc start）。
    /// 返回成功 / 失败消息。
    fn start_service(&self, label: &str) -> Result<(), String>;

    /// 停止 helper daemon（mac: launchctl bootout；linux: systemctl stop；win: sc stop）。
    fn stop_service(&self, label: &str) -> Result<(), String>;

    /// 是否已加载/**正在运行**（mac: launchctl print；linux: systemctl is-active；win: sc query 且 RUNNING）。
    fn is_loaded(&self, label: &str) -> bool;

    /// 服务是否**已注册**（不要求正在运行）。
    ///
    /// 与 [`SysOps::is_loaded`] 的区别是本方法对「装了但停着」返回 `true` —— 这正是
    /// [`HelperManager::is_installed`] 在 Windows 上需要的语义（SCM 里有服务定义即算装过；
    /// 若沿用 `is_loaded`，一台 helper 停着的机器会被判成从没装过，进而丢掉可修复态）。
    fn service_exists(&self, label: &str) -> bool;
}

impl HelperManager {
    /// 构造管理器。`token_path` = app 侧 token 文件路径（上游 `getUserDataPath()/helper-client.token`）。
    pub fn new(platform: Platform, token_path: PathBuf, sysops: Box<dyn SysOps>) -> Self {
        Self {
            platform,
            paths: InstallPaths::for_platform(platform),
            token_path,
            sysops,
        }
    }

    /// 当前平台。
    #[must_use]
    pub const fn platform(&self) -> Platform {
        self.platform
    }

    /// 安装路径集。
    #[must_use]
    pub fn paths(&self) -> &InstallPaths {
        &self.paths
    }

    /// 当前 app 侧 token（从 token 文件读，缺失返回空）。
    #[must_use]
    pub fn token(&self) -> String {
        token::read_token(&self.token_path)
    }

    /// 探测 helper 是否已安装 = **二进制在位 + 服务已注册**。
    ///
    /// 移植自 上游 `filesPresent()`（HelperManager.ts:202-205）：
    /// ```text
    /// return fs.existsSync(HELPER_DEST) && fs.existsSync(PLIST_PATH);
    /// ```
    ///
    /// 证据构成按平台（W17 后显式分歧）：
    /// - **Windows：SCM 单证据**（[`SysOps::service_exists`]，`sc query` 退出码，不要求
    ///   RUNNING）。不再 stat 二进制——安装脚本把 support 目录锁成 SYSTEM/Administrators-only，
    ///   UAC 过滤令牌的未提权 app 连 stat 都被拒（2026-08-19 真机对照实测），文件证据恒 false。
    ///   取舍：「拷贝成功 + New-Service 失败」的孤儿二进制态判未装（可重装，脚本 Force 覆盖）；
    ///   「服务在 + 二进制被手删」判已装（ping 挂 → needs_repair 可修复，优于误报未装）。
    /// - **mac/linux：binary + 描述符双证据**（plist/unit 可 stat，目录无 ACL 锁）。
    ///
    /// 共同底线：「装了但没跑」必须仍判已安装，否则 `compute_status_with_client` 短路成
    /// 全 false、连管道都不 ping，把可修复态误报成未安装态。
    #[must_use]
    pub fn is_installed(&self) -> bool {
        // 证据集按 **Platform 枚举**分派（2026-08-20 订正：原先按编译目标 cfg 分叉——win 编译
        // 目标上非 Win 平台的管理器也被拽进 SCM 分支，CI win 腿六测全红；而 push 只跑 ubuntu 腿，
        // 全矩阵 dispatch 才暴露）。生产行为不变：真 Windows 上 Platform 恒为 Win。
        match self.platform {
            // Windows（W17，2026-08-19 首次成功安装后暴露）：安装脚本把 support 目录 ACL 锁成
            // SYSTEM/Administrators-only（token/exe 出生即私有，防窃取），而 Administrators 组在
            // **UAC 过滤令牌**里是 deny-only ⇒ 未提权 app 连 stat 二进制都被拒（真机实测
            // Test-Path=False，提权=True）→ 文件证据恒 false → 状态卡「未安装」。故 **SCM 单证据**：
            // 服务由安装脚本与二进制同事务创建（New-Service 在 Copy-Item 之后），SCM 查询未提权
            // 可用（真机实测 sc query exit=0）。取舍：孤儿二进制（拷了没建成服务）判未装可重装；
            // 服务在而二进制被手删判已装（ping 挂 → needs_repair 可修复，优于误报未装）。
            Platform::Win => self.sysops.service_exists(self.paths.service_label),
            // mac/linux：support 目录无此 ACL 锁（plist/unit 可 stat），binary+描述符双证据。
            // Other/Android/Ios 同臂且不可达（`platform_supported` 恒 false，见
            // `InstallPaths::for_platform` 头注）。**具名而非裸 `_`**（2026-09-04 K10）：裸通配臂
            // 会让「新增平台变体」这件事在编译期无声通过，而这里恰好是 Windows 与其余平台
            // 取证策略不同的那一处 —— 新平台该走哪种证据是必须现场答的题。
            //
            // iOS 现场答题（2026-09-06）：走双证据臂而不是 SCM 单证据臂。本处特有的那半句 ——
            // 选 SCM 那一侧的前提是「存在一个服务注册表可查」，iOS 上没有任何这样的面；而双证据
            // 臂退化成两次 `exists()` 假查询，恒 false = 「没装」，正是诚实答案。
            Platform::Mac
            | Platform::Linux
            | Platform::Other
            | Platform::Android
            | Platform::Ios => {
                if !self.sysops.exists(&self.paths.binary) {
                    return false;
                }
                match &self.paths.descriptor {
                    Some(desc) => self.sysops.exists(desc),
                    None => self.sysops.service_exists(self.paths.service_label),
                }
            }
        }
    }

    /// 是否已加载/运行（launchd/systemd/SCM 视角）。
    #[must_use]
    pub fn is_loaded(&self) -> bool {
        self.sysops.is_loaded(self.paths.service_label)
    }

    /// 启动 helper daemon（委托 [`SysOps::start_service`]）。
    pub fn start(&self) -> Result<(), String> {
        self.sysops.start_service(self.paths.service_label)
    }

    /// 停止 helper daemon（委托 [`SysOps::stop_service`]）。
    pub fn stop(&self) -> Result<(), String> {
        self.sysops.stop_service(self.paths.service_label)
    }

    /// 安装前的 token 准备：复用已有 token，否则生成并写盘（移植自 `HelperManager.ts:478-485`）。
    ///
    /// 返回 token（已写盘）。这步是纯文件操作（不需要 root），在 root 安装脚本之前由 app 跑。
    pub fn prepare_token(&self) -> Result<String, std::io::Error> {
        let existing = self.token();
        if !existing.is_empty() {
            // 复用已有 token（Polaris install 复用，避免 root 侧 helper.token 与 client token 失同步）
            return Ok(existing);
        }
        token::write_token(&self.token_path)
    }

    /// 卸载后的 token 清理：删 app 侧 token 文件（移植自 `HelperManager.ts:567-571`）。
    pub fn clear_token(&self) {
        token::remove_token(&self.token_path);
    }
}

/// 本 build 期望的 protoVersion（**与平台无关**）。
///
/// 上游 这里按平台分三支（mac=9 / win=5 / linux=1），因为它的三套 Go helper 各自演进。Polaris 三
/// 平台共用一个 `helper-proto` crate、helper 也是同一份 Rust 代码按 target 编译 ⇒ 期望值只有一个。
/// 保留成函数（而非内联常量）是为了留住 `upgradeable` 的语义：装着的 helper 比本 build 期望的旧 →
/// 提示可升级；`CURRENT` 将来 +1 时这条自动生效。
fn expected_proto() -> u32 {
    polaris_helper_proto::proto_version::CURRENT
}

/// ping helper 取 protoVersion + build identity（移植自 上游 `sendCommand(['ping'], 1500)` + 正则提取）。
///
/// 返回 None 当 ping 失败或响应非 Pong。
fn ping_for_handshake(client: &HelperClient) -> Option<Pong> {
    let resp = client
        .probe_framed_transport(Duration::from_millis(1500))
        .ok()?;
    match resp {
        Response::Ok(ResponseKind::Pong(p)) => Some(p),
        _ => None,
    }
}

impl HelperManager {
    /// 计算完整状态（接收已构造的 HelperClient，避免 Connector 克隆问题）。
    ///
    /// 这是推荐入口 —— 调用方持有 HelperClient，HelperManager 仅做决策。
    pub fn compute_status_with_client(&self, client: &HelperClient) -> HelperStatus {
        let installed = self.is_installed();
        if !installed {
            return HelperStatus::default();
        }
        let tok = self.token();
        if tok.is_empty() {
            return HelperStatus {
                installed: true,
                needs_repair: true,
                ..Default::default()
            };
        }
        let handshake = match ping_for_handshake(client) {
            Some(v) => v,
            None => {
                return HelperStatus {
                    installed: true,
                    needs_repair: true,
                    ..Default::default()
                };
            }
        };
        let proto_version = handshake.proto_version;
        let ready = proto_version >= MIN_USABLE_PROTO;
        let expected_proto = expected_proto();
        // B3: a newer compatible helper serves multiple App/core versions.
        // Build id remains diagnostic; App overlays its own core-status id proof.
        let upgradeable = proto_version < expected_proto;
        HelperStatus {
            installed: true,
            ready,
            version: Some(proto_version),
            build_id: handshake.build_identity,
            upgradeable,
            needs_repair: !ready,
        }
    }
}

// ============================================================================
// 装卸全流程（install / uninstall）—— 移植自 Polaris 三平台 install 编排：
//   mac: HelperManager.ts:buildInstallScript/buildUninstallScript/runRootScript
//   linux: LinuxServiceHelper.ts:buildInstallScript/buildUnit/runPkexecScript
//   win: WindowsServiceHelper.ts:buildInstallScript/buildUninstallScript/runElevatedPowerShell
//
// **单一编排真值源**（编排者拍板）：install 是「生成一个 root 脚本（内含 拷二进制 + 写 root 侧 token +
// 播种核 + 写 plist/unit/SCM 描述 + bootstrap）→ 经提权（osascript/pkexec/UAC）跑一次」。plist/unit/SCM
// 描述**内嵌进脚本**（root 侧写），与 上游 逐字对齐。launchd.rs 降为纯 render（不再自持 bootstrap 编排）。
// ============================================================================

use crate::privilege::{
    osascript_escalation, pkexec_escalation, run_escalation, shell_quote, uac_escalation,
    Escalation, EscalationOutcome, Executor,
};
use std::io;

/// mac `--support` 目录（= daemon `daemon.rs:DEFAULT_SUPPORT_DIR`，helper.token/socket/core 落此）。
const MAC_SUPPORT_DIR: &str = "/Library/Application Support/Polaris";
/// linux systemd 服务名（= `InstallPaths::linux().descriptor` 文件名）。
const LINUX_SERVICE_NAME: &str = "polaris-helper.service";
/// linux 授权 uid 列表（= daemon `--authfile` 默认，`server.rs:DEFAULT_AUTH_FILE`）。
const LINUX_AUTH_FILE: &str = "/var/lib/polaris/authorized-uids";
/// linux 授权文件所在状态目录（`AUTH_FILE` 的父）。
const LINUX_STATE_DIR: &str = "/var/lib/polaris";
/// win SCM 服务名。与 daemon 侧共引 helper-proto 的同一份真值（构造上单源，见该模块文档）。
const WIN_SERVICE_NAME: &str = polaris_helper_proto::windows_helper::SERVICE_NAME;
/// win 命名管道（client 连接目标）。与 daemon 侧 `CreateNamedPipeW` 共引同一份真值。
const WIN_PIPE_NAME: &str = polaris_helper_proto::windows_helper::PIPE_NAME;
/// win support 目录（= daemon `--support` 默认；helper.exe 外置副本 + helper.token 落此）。与 daemon 侧
/// 共引同一份真值。
const WIN_SUPPORT_DIR: &str = polaris_helper_proto::windows_helper::DEFAULT_SUPPORT_DIR;
/// win helper 外置副本文件名。**单一真相源**：[`InstallPaths::win`] 与
/// [`build_win_install_script`] 必须同取此常量。早先脚本用 `win_basename(src_binary)` 现算、
/// 而 `InstallPaths::win()` 另写死一个不同路径，两者分叉即 Windows 恒判「未安装」的成因。
const WIN_HELPER_EXE: &str = "polaris-helper.exe";
/// win 受保护核目录名（`<support>\core`）。
///
/// **跨 crate 第二份**：helper 侧 `platform::windows::helper.rs` 的 `derived_core_dir()` 自己
/// `join("core")`（它只拿到 `--support`，看不见本 crate）。漂移后果**静默**：ImagePath、seed、
/// app 侧 dest 全跟本常量走，helper 的 install-core 却写进另一个目录 ⇒ app 每次起核都判
/// 「受保护核不存在」、每次白推 80MB 且日志恒「已提升」，而 helper exec 的是安装期播下的那份
/// ⇒ 换核永远不生效。两份由 `manager/tests` 的
/// `win_core_dir_name_agrees_with_the_helper_side_derivation` 钉在一起。
pub const WIN_CORE_DIR_NAME: &str = "core";
/// helper token 文件名（`<support>\helper.token` / `$SUPPORT/helper.token`）。
///
/// 与 helper 侧的 `polaris_helper::token::TOKEN_FILENAME` 是两份同值字面量（本 crate 不依赖
/// helper），由 `src-tauri` 的等值门 `helper_token_filename_agrees_across_crates` 钉住：
/// 漂移后安装脚本写一个名字、daemon 读另一个 ⇒ helper 恒回 `ERR auth`，而安装本身「成功」。
pub const HELPER_TOKEN_FILENAME: &str = "helper.token";
/// win 受保护核目录里的核文件名。
///
/// 与 helper 侧的 `polaris_helper_proto::core_payload::WIN_CORE_FILENAME` 和 app 侧的
/// `core_paths::core_filename_for("windows")` 是三份同值字面量 —— 本 crate 既看不见 helper
/// 也看不见 app，故不在 crate 间新开真值源，改由 `src-tauri` 的等值门把三者钉在一起
/// （`runtime/proxy/platform_contracts` 的 `win_core_binary_names_agree_across_crates`）。
pub const WIN_CORE_BIN_NAME: &str = "sing-box.exe";
/// win 受保护核目录里的 NaiveProxy 配套 DLL 名（同上，由等值门钉住）。
pub const WIN_CORE_SIDECAR_NAME: &str = "libcronet.dll";

/// install 就绪轮询：装完等 daemon 起来绑 socket/pipe 的次数。2026-08-19 真机实测从上游的
/// `for i<10` 放宽：脚本侧 sc 删旧等待窗（≤15s）+ New-Service 重试后，服务起管道常超 3s，
/// 旧窗口把响应快照定格在「未就绪」，卡片停在安装前旧态（W10 跟进项，.207 首装实测）。
pub const READY_POLL_ATTEMPTS: u32 = 20;
/// install 就绪轮询间隔（同上从 300ms 放宽：总窗 10s，覆盖 SCM 冷启动 + 管道监听）。
pub const READY_POLL_DELAY: Duration = Duration::from_millis(500);

/// 装卸流程错误（escalation 之前的失败：二进制缺失 / token 写失败 / 脚本落盘失败 / 平台不支持）。
///
/// escalation 本身的三态（成功/取消/失败）由 [`EscalationOutcome`] 表达（非 `Err`）—— 用户取消是正常流程，不是错误。
#[derive(Debug, thiserror::Error)]
pub enum ManagerError {
    #[error("安装身份参数无效: {0}")]
    InvalidInstallation(&'static str),
    /// helper 源二进制缺失（构建未包含），对齐 上游 `!fs.existsSync(srcBinary)`。
    #[error("helper 二进制缺失: {0}")]
    HelperBinaryMissing(PathBuf),
    /// app 侧 token 写盘失败（移植 Polaris install 的 `写入 token 失败`）。
    #[error("写入 token 失败: {0}")]
    TokenWrite(io::Error),
    /// 安装脚本落盘失败（移植 Polaris runRootScript/runPkexecScript 的 catch）。
    #[error("写入安装脚本失败: {0}")]
    ScriptWrite(io::Error),
    /// 提权执行本身失败（spawn 失败等，非「用户取消」——取消是 [`EscalationOutcome::Cancelled`]）。
    #[error("提权执行失败: {0}")]
    Escalation(#[from] crate::client::ClientError),
    /// 本编译目标不支持该平台的装卸（如 Linux 二进制上构造 `Platform::Mac` 的装脚本 —— 运行期不会发生）。
    #[error("本构建目标不支持平台 {0:?} 的装卸")]
    UnsupportedPlatform(Platform),
}

/// Installation intent. Ordinary installation never opts into stopping legacy sessions.
/// StopAndUpgrade is only valid after explicit confirmation of its cross-session impact.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InstallMode {
    #[default]
    Ordinary,
    StopAndUpgrade,
}

impl InstallMode {
    #[must_use]
    pub const fn supported_on(self, platform: Platform) -> bool {
        matches!(self, Self::Ordinary) || matches!(platform, Platform::Mac | Platform::Linux)
    }
}

/// Confirmed maintenance scope; a zero exit on macOS retires, never deletes data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaintenanceOutcome {
    Removed,
    RetiredDataPreserved,
    Cancelled,
    Failed { stderr: String, code: i32 },
}

fn maintenance_outcome(platform: Platform, outcome: EscalationOutcome) -> MaintenanceOutcome {
    match outcome {
        EscalationOutcome::Success if platform == Platform::Mac => {
            MaintenanceOutcome::RetiredDataPreserved
        }
        EscalationOutcome::Success => MaintenanceOutcome::Removed,
        EscalationOutcome::Cancelled => MaintenanceOutcome::Cancelled,
        EscalationOutcome::Failed { stderr, code } => MaintenanceOutcome::Failed { stderr, code },
    }
}

/// Current bundled utility, sealed again before privileged maintenance.
/// Uninstall deliberately does not depend on a selected or installed core.
#[derive(Debug, Clone)]
pub struct MaintenanceUtility {
    pub src_binary: PathBuf,
    pub helper_sha256: String,
}

/// 安装参数（路径由调用方解析 —— HelperManager 不知道 Tauri 资源布局，对齐「决策+委托」）。
///
/// 各平台按需取用，无关字段忽略（移植锚点见各字段）。
#[derive(Debug, Clone)]
pub struct InstallParams {
    /// Nine captured exact gsettings values plus the responsible HOME, never
    /// a claim that restoration succeeded.
    /// The Linux dispatcher reads every fixed key in that user's
    /// actual session again; missing baseline rejects only legacy maintenance.
    pub linux_legacy_original_proxy: Option<[String; 10]>,
    /// Complete private stage, frozen by the App before administrator approval.
    pub manifest_path: PathBuf,
    pub manifest_sha256: String,
    pub core_id: polaris_helper_proto::core_manifest::CoreId,
    pub helper_sha256: String,
    /// Compiled App selection, never a digest supplied by a stage manifest.
    pub cleaner_sha256: Option<String>,
    pub helper_version: String,
    pub user_identity: polaris_helper_proto::core_protocol::CallerIdentity,
    /// Private stage helper. Administrator scripts copy and hash it before execution.
    /// Same-account replacement of the whole App remains outside this byte receipt.
    pub src_binary: PathBuf,
    /// Staged core and sidecar, consumed against a root-sealed manifest via held FDs.
    pub bundled_core: PathBuf,
    /// Per-native-user config registration; never embedded in a machine service command.
    pub conf_dir: PathBuf,
    /// 授权 uid（linux 写 `authorized-uids`；**mac/win 忽略**）。上游(linux): `process.getuid()`。
    pub uid: u32,
    /// 安装脚本临时落盘目录（安全 0700 私有目录；调用方传 `userData/priv` 等）。
    /// 上游: `getUserDataPath()/priv`(mac) / `getUserDataPath()`(linux) / `os.tmpdir()`(win)。
    pub script_dir: PathBuf,
}

impl HelperManager {
    /// 安装/修复 helper（全流程，移植自三平台 `install()`）。
    ///
    /// 步骤：① 校验源二进制存在 → ② 准备 app 侧 token（复用或生成，[`prepare_token`](Self::prepare_token)）
    /// → ③ 生成 root 安装脚本（内嵌 拷二进制 + 写 root token + 播种核 + 写 plist/unit/SCM 描述 + bootstrap）
    /// → ④ 脚本安全落盘（随机名 + 0700 + O_EXCL）经 [`Executor`] 提权跑（osascript/pkexec/UAC）→ ⑤ 清理脚本。
    ///
    /// 返回 [`EscalationOutcome`]（成功/用户取消/脚本失败）。**就绪等待**由调用方在成功后用
    /// [`wait_until_ready`](Self::wait_until_ready) 轮询（需 [`HelperClient`]，与提权解耦、各自可测）。
    pub fn install(
        &self,
        params: &InstallParams,
        executor: &dyn Executor,
    ) -> Result<EscalationOutcome, ManagerError> {
        self.install_with_mode(params, executor, InstallMode::Ordinary)
    }

    /// Explicit macOS migration is never selected as a fallback from an ordinary install.
    /// Reject unsupported modes before inspecting parameters, touching tokens, or escalating.
    pub fn install_with_mode(
        &self,
        params: &InstallParams,
        executor: &dyn Executor,
        mode: InstallMode,
    ) -> Result<EscalationOutcome, ManagerError> {
        if !mode.supported_on(self.platform) {
            return Err(ManagerError::UnsupportedPlatform(self.platform));
        }
        validate_install_params(self.platform, params)?;
        // ① 源二进制存在（Polaris: !fs.existsSync(srcBinary) → 缺失）。
        if !self.sysops.exists(&params.src_binary) {
            return Err(ManagerError::HelperBinaryMissing(params.src_binary.clone()));
        }
        // ② token：复用或生成并写 app 侧（linux 无 token 语义，写了也无害；脚本不会用）。
        let token = self.prepare_token().map_err(ManagerError::TokenWrite)?;
        if self.platform == Platform::Win {
            #[cfg(windows)]
            {
                use polaris_windows_cleaner::{install::Request, InstallOutcome};
                let original_sid = match &params.user_identity {
                    polaris_helper_proto::core_protocol::CallerIdentity::Windows(sid) => {
                        sid.clone()
                    }
                    _ => {
                        return Err(ManagerError::InvalidInstallation(
                            "native Windows caller required",
                        ));
                    }
                };
                let text = |path: &Path| {
                    path.to_str()
                        .map(str::to_owned)
                        .ok_or(ManagerError::InvalidInstallation(
                            "native installation path must be Unicode",
                        ))
                };
                let request = Request {
                    stage: text(
                        params
                            .src_binary
                            .parent()
                            .ok_or(ManagerError::InvalidInstallation("stage missing"))?,
                    )?,
                    original_sid,
                    config: text(&params.conf_dir)?,
                    core_id: params.core_id.as_str().into(),
                    manifest_sha256: params.manifest_sha256.clone(),
                    helper_sha256: params.helper_sha256.clone(),
                    cleaner_sha256: params.cleaner_sha256.clone().ok_or(
                        ManagerError::InvalidInstallation("compiled cleaner pin required"),
                    )?,
                    token,
                    stage_identity: [0; 3],
                    source_identities: vec![],
                };
                let outcome =
                    polaris_windows_cleaner::launch_install(request).map_err(|error| {
                        ManagerError::Escalation(crate::ClientError::Connect(error.to_string()))
                    })?;
                return Ok(match outcome {
                    InstallOutcome::Cancelled => EscalationOutcome::Cancelled,
                    InstallOutcome::Completed(0) => EscalationOutcome::Success,
                    InstallOutcome::Completed(code) => EscalationOutcome::Failed {
                        stderr: "native Windows installation did not complete".into(),
                        code,
                    },
                });
            }
            #[cfg(not(windows))]
            return Err(ManagerError::UnsupportedPlatform(Platform::Win));
        }
        // ③ 生成 root 安装脚本。
        let script = self.build_install_script(params, &token, mode)?;
        // ④⑤ 落盘 + 提权 + 清理。
        self.run_privileged_script(
            &params.script_dir,
            self.install_script_name(),
            &script,
            executor,
        )
    }

    /// 卸载 helper（全流程，移植自三平台 `uninstall()` 的提权路径）。
    ///
    /// mac/Linux 生成固定卸载脚本；Windows 直接请求 bundled native cleaner UAC，
    /// 不依赖已安装 helper、其版本或 app 配置目录脚本。仅完整成功清 app 侧 token。
    /// 返回 [`EscalationOutcome`]。
    ///
    /// 管道自卸 [`pipe_self_uninstall`](Self::pipe_self_uninstall) 仅表示已启动，
    /// 不证明 SCM/目录清理完成。用户主动卸载使用本方法获得最终 native exit outcome。
    pub fn uninstall(
        &self,
        script_dir: &Path,
        executor: &dyn Executor,
    ) -> Result<EscalationOutcome, ManagerError> {
        self.uninstall_with_maintenance(script_dir, None, executor)
    }

    /// Preserve the platform maintenance scope through the App/report boundary.
    pub fn uninstall_with_maintenance_outcome(
        &self,
        script_dir: &Path,
        utility: Option<&MaintenanceUtility>,
        executor: &dyn Executor,
    ) -> Result<MaintenanceOutcome, ManagerError> {
        self.uninstall_with_maintenance(script_dir, utility, executor)
            .map(|outcome| maintenance_outcome(self.platform, outcome))
    }

    /// Unix callers must provide the current bundled utility. Never invoke an
    /// unknown installed legacy helper as a maintenance command interpreter.
    pub fn uninstall_with_maintenance(
        &self,
        script_dir: &Path,
        utility: Option<&MaintenanceUtility>,
        executor: &dyn Executor,
    ) -> Result<EscalationOutcome, ManagerError> {
        self.uninstall_with_linux_original_proxy(script_dir, utility, None, executor)
    }

    pub fn uninstall_with_linux_original_proxy(
        &self,
        script_dir: &Path,
        utility: Option<&MaintenanceUtility>,
        original: Option<&[String; 10]>,
        executor: &dyn Executor,
    ) -> Result<EscalationOutcome, ManagerError> {
        match self.platform {
            Platform::Mac | Platform::Linux | Platform::Win => {}
            Platform::Android | Platform::Ios | Platform::Other => {
                return Err(ManagerError::UnsupportedPlatform(self.platform));
            }
        }
        if self.platform == Platform::Win {
            #[cfg(windows)]
            {
                let outcome = match polaris_windows_cleaner::launch_bundle() {
                    polaris_windows_cleaner::Outcome::Success => EscalationOutcome::Success,
                    polaris_windows_cleaner::Outcome::Cancelled => EscalationOutcome::Cancelled,
                    outcome => EscalationOutcome::Failed {
                        stderr: format!(
                            "native cleaner {:?}; service/tree cleanup is not confirmed complete",
                            outcome
                        ),
                        code: outcome.code(),
                    },
                };
                if outcome == EscalationOutcome::Success {
                    self.clear_token();
                }
                return Ok(outcome);
            }
            #[cfg(not(windows))]
            return Err(ManagerError::UnsupportedPlatform(Platform::Win));
        }
        let utility = utility.ok_or(ManagerError::InvalidInstallation(
            "current bundled maintenance utility required",
        ))?;
        validate_maintenance_utility(utility)?;
        let script = if self.platform == Platform::Linux {
            uninstall_unix::build_linux_uninstall_script_with_original(
                &self.paths,
                utility,
                original,
                script_dir,
            )
        } else {
            self.build_uninstall_script(utility)
        };
        let outcome = self.run_privileged_script(
            script_dir,
            self.uninstall_script_name(),
            &script,
            executor,
        )?;
        // 只有完整成功清 app 侧 token，取消/partial 保留以支持重试。
        if matches!(outcome, EscalationOutcome::Success) {
            self.clear_token();
        }
        Ok(outcome)
    }

    /// Legacy Windows IPC probe. Current helper replies with an explicit
    /// bundled-cleaner/UAC-fallback error and never starts SYSTEM self-removal.
    /// `true` only records an older helper's OK acknowledgment, not completion;
    /// callers need the synchronous [`uninstall`](Self::uninstall) result.
    /// There is no production callsite using this probe as an uninstall receipt.
    #[must_use]
    pub fn pipe_self_uninstall(&self, client: &HelperClient) -> bool {
        if self.platform != Platform::Win {
            return false;
        }
        matches!(client.send(&Request::Uninstall), Ok(Response::Ok(_)))
    }

    /// 装完轮询到**当前随包构建**（移植 Polaris install 尾的就绪轮询，并补 build identity 对账）。
    ///
    /// 每轮 [`compute_status_with_client`](Self::compute_status_with_client)（内含 ping）。`client` 须用
    /// 生产 [`Connector`](crate::client::Connector)（每 ping 重连）。同 proto 的旧 helper 也会 `ready=true`，
    /// 但仍 `upgradeable=true`；兼容协议与 App 内容身份分别由 manager 和 App 验证。
    /// 因此仅 `ready && !upgradeable` 收口；返回最终状态（当前构建或轮询耗尽）。
    #[must_use]
    pub fn wait_until_ready(
        &self,
        client: &HelperClient,
        attempts: u32,
        delay: Duration,
    ) -> HelperStatus {
        let mut status = self.compute_status_with_client(client);
        for _ in 0..attempts {
            if status.ready && !status.upgradeable {
                break;
            }
            std::thread::sleep(delay);
            status = self.compute_status_with_client(client);
        }
        status
    }

    // ── 脚本生成分派 ────────────────────────────────────────────────────────
    /// 生成 root 安装脚本（按平台分派）。
    fn build_install_script(
        &self,
        params: &InstallParams,
        token: &str,
        mode: InstallMode,
    ) -> Result<String, ManagerError> {
        if !mode.supported_on(self.platform) {
            return Err(ManagerError::UnsupportedPlatform(self.platform));
        }
        validate_install_params(self.platform, params)?;
        match self.platform {
            Platform::Mac => Ok(match mode {
                InstallMode::Ordinary => build_mac_install_script(&self.paths, params, token),
                InstallMode::StopAndUpgrade => {
                    build_mac_migration_script(&self.paths, params, token)
                }
            }),
            // Android/iOS 同 Other：无 helper 可装，本臂不可达（理由见 `InstallPaths::for_platform`）。
            // iOS 本处特有的那半句：脚本这个概念本身在 iOS 上没有承载物 —— 应用沙箱里没有可执行
            // 的 shell，写出来的 .sh 没有任何东西能跑它。
            Platform::Linux | Platform::Other | Platform::Android | Platform::Ios => {
                Ok(match mode {
                    InstallMode::Ordinary => build_linux_install_script(&self.paths, params, token),
                    InstallMode::StopAndUpgrade => {
                        install_unix::build_linux_legacy_install_script(&self.paths, params, token)
                    }
                })
            }
            Platform::Win => {
                #[cfg(test)]
                {
                    Ok(build_win_install_script(&self.paths, params, token))
                }
                #[cfg(not(test))]
                {
                    Err(ManagerError::UnsupportedPlatform(Platform::Win))
                }
            }
        }
    }

    /// 生成 root 卸载脚本（按平台分派）。
    fn build_uninstall_script(&self, utility: &MaintenanceUtility) -> String {
        match self.platform {
            Platform::Mac => uninstall_unix::build_mac_uninstall_script(&self.paths, utility),
            // Android/iOS 同 Other：不可达，理由见 `InstallPaths::for_platform`。
            // iOS 本处特有的那半句：同上——没有 shell 可跑，且没有装过的东西可卸。
            Platform::Linux | Platform::Other | Platform::Android | Platform::Ios => {
                uninstall_unix::build_linux_uninstall_script(&self.paths, utility)
            }
            Platform::Win => unreachable!("Windows uninstall is a fixed native operation"),
        }
    }

    /// 安装脚本文件名（含平台正确扩展名：mac/linux=`.sh` bash、win=`.ps1` PowerShell）。
    const fn install_script_name(&self) -> &'static str {
        match self.platform {
            Platform::Win => "polaris-helper-install.ps1",
            // 具名而非裸 `_`（2026-09-04 K10）：扩展名跟的是脚本语言（PowerShell vs bash），
            // 新平台未必是这两种之一。
            // iOS 现场答题（2026-09-06）：既然两种脚本语言在 iOS 上都不存在，这个名字纯粹是
            // 不可达路径上的占位；跟 bash 那一侧只为与 `build_install_script` 的臂同形——两处
            // 若分叉，将来真有人误在 iOS 上走到这条路时会拿到「名字是 .ps1、内容是 bash」。
            Platform::Mac
            | Platform::Linux
            | Platform::Other
            | Platform::Android
            | Platform::Ios => "polaris-helper-install.sh",
        }
    }

    fn uninstall_script_name(&self) -> &'static str {
        match self.platform {
            Platform::Win => unreachable!("Windows uninstall does not write a script"),
            // iOS 同上一函数：不可达占位，与 `build_uninstall_script` 的臂逐值同形。
            Platform::Mac
            | Platform::Linux
            | Platform::Other
            | Platform::Android
            | Platform::Ios => "polaris-helper-uninstall.sh",
        }
    }

    // ── 提权跑脚本（落盘 + escalation + 清理，移植 runRootScript/runPkexecScript/runElevatedPowerShell）──
    /// 脚本安全落盘（随机名 + 0700 + O_EXCL）→ 构造提权 [`Escalation`] → [`run_escalation`] → 清理脚本。
    fn run_privileged_script(
        &self,
        script_dir: &Path,
        name: &str,
        script: &str,
        executor: &dyn Executor,
    ) -> Result<EscalationOutcome, ManagerError> {
        // TOCTOU 加固（移植 HelperManager.ts:872-882）：私有 0700 目录 + 随机名 + O_EXCL 独占创建。
        let script_path =
            write_secure_script(script_dir, name, script).map_err(ManagerError::ScriptWrite)?;
        let script_path_str = script_path.to_string_lossy().into_owned();
        let escalation = self.build_escalation(&script_path_str)?;
        let outcome = run_escalation(&escalation, executor);
        // finally: 清理脚本（对齐 Polaris runRootScript 的 unlink，成功失败都删）。
        let _ = std::fs::remove_file(&script_path);
        Ok(outcome?)
    }

    /// 按平台构造提权决策（复用 [`privilege`](crate::privilege) 的真逻辑 argv 构造）。
    fn build_escalation(&self, script_path: &str) -> Result<Escalation, ManagerError> {
        Ok(match self.platform {
            Platform::Mac => osascript_escalation(script_path),
            // Android 同 Other：不可达。Android 上既没有 pkexec 也没有任何应用可用的提权通道
            // （提权在那个平台的对应物是系统授予的 `VpnService` 权限，不经本 crate）。
            // iOS 本处特有的那半句：同为不可达，但对应物换成「系统按 packet-tunnel-provider
            // entitlement 授予 NE 扩展的隧道权限」——它在**安装期**由描述文件决定，运行期没有
            // 任何「向用户求一次权」的交互面，故 `Escalation` 这个概念在 iOS 上没有对应物。
            Platform::Linux | Platform::Other | Platform::Android | Platform::Ios => {
                pkexec_escalation(script_path)
            }
            Platform::Win => uac_escalation(script_path)?,
        })
    }
}

fn validate_maintenance_utility(utility: &MaintenanceUtility) -> Result<(), ManagerError> {
    if !utility.src_binary.is_absolute()
        || utility
            .src_binary
            .to_string_lossy()
            .contains(['\0', '\n', '\r'])
        || polaris_helper_proto::core_manifest::CoreId::parse(&utility.helper_sha256).is_err()
    {
        return Err(ManagerError::InvalidInstallation(
            "invalid maintenance utility identity",
        ));
    }
    if !utility.src_binary.is_file() {
        return Err(ManagerError::HelperBinaryMissing(
            utility.src_binary.clone(),
        ));
    }
    Ok(())
}

fn validate_install_params(platform: Platform, params: &InstallParams) -> Result<(), ManagerError> {
    use polaris_helper_proto::{core_manifest::CoreId, core_protocol::CallerIdentity};
    if CoreId::parse(&params.helper_sha256).is_err()
        || CoreId::parse(&params.manifest_sha256).is_err()
    {
        return Err(ManagerError::InvalidInstallation(
            "missing actual byte hashes",
        ));
    }
    if platform == Platform::Win
        && params
            .cleaner_sha256
            .as_deref()
            .is_none_or(|hash| CoreId::parse(hash).is_err())
    {
        return Err(ManagerError::InvalidInstallation(
            "compiled cleaner pin required",
        ));
    }
    let absolute = |path: &Path| {
        let raw = path.to_string_lossy();
        !raw.contains(['\0', '\n', '\r'])
            && (path.is_absolute()
                || platform == Platform::Win
                    && raw.as_bytes().get(1) == Some(&b':')
                    && matches!(raw.as_bytes().get(2), Some(b'\\' | b'/')))
    };
    if [
        &params.src_binary,
        &params.bundled_core,
        &params.manifest_path,
        &params.conf_dir,
        &params.script_dir,
    ]
    .into_iter()
    .any(|path| !absolute(path))
    {
        return Err(ManagerError::InvalidInstallation(
            "absolute single-line paths required",
        ));
    }
    let split = |path: &Path| {
        let raw = path.to_string_lossy();
        raw.rsplit_once(['/', '\\'])
            .map(|(parent, name)| (parent.to_owned(), name.to_owned()))
    };
    let manifest = split(&params.manifest_path);
    let helper = split(&params.src_binary);
    let core = split(&params.bundled_core);
    if manifest
        .as_ref()
        .is_none_or(|(_, name)| name != "core-manifest.json")
    {
        return Err(ManagerError::InvalidInstallation(
            "invalid manifest basename",
        ));
    }
    if helper.as_ref().map(|(parent, _)| parent) != manifest.as_ref().map(|(parent, _)| parent)
        || core.as_ref().map(|(parent, _)| parent) != manifest.as_ref().map(|(parent, _)| parent)
    {
        return Err(ManagerError::InvalidInstallation(
            "installation files must share one private stage",
        ));
    }
    let owner_matches = match platform {
        Platform::Linux | Platform::Mac => {
            matches!(&params.user_identity, CallerIdentity::Unix(uid) if *uid == params.uid)
        }
        Platform::Win => {
            matches!(&params.user_identity, CallerIdentity::Windows(sid) if CallerIdentity::windows(sid).is_some())
        }
        Platform::Android | Platform::Ios | Platform::Other => false,
    };
    if !owner_matches {
        return Err(ManagerError::InvalidInstallation(
            "native source owner differs",
        ));
    }
    Ok(())
}

// ===== 脚本安全落盘（TOCTOU 加固）=====

/// 生成随机后缀（不可预测的脚本文件名，防同用户进程抢占落点 —— 对齐 上游 `randomBytes(12)`）。
///
/// 用 OS 熵源（/dev/urandom），失败回退时间+计数（脚本落私有 0700 目录 + O_EXCL 已是主防线，随机名是纵深）。
fn random_suffix() -> String {
    let mut bytes = [0u8; 12];
    #[cfg(unix)]
    {
        use std::io::Read;
        if std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .is_err()
        {
            fill_fallback(&mut bytes);
        }
    }
    #[cfg(not(unix))]
    {
        fill_fallback(&mut bytes);
    }
    let mut s = String::with_capacity(24);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

fn fill_fallback(buf: &mut [u8]) {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let pid = u64::from(std::process::id());
    let cnt = COUNTER.fetch_add(1, Ordering::Relaxed);
    for (i, byte) in buf.iter_mut().enumerate() {
        let mix = now
            .wrapping_add(pid)
            .wrapping_add(cnt)
            .wrapping_add(i as u64);
        *byte = mix.to_le_bytes()[i % 8];
    }
}

/// 脚本落盘到 `dir/<random>-<name>`：私有 0700 目录 + O_EXCL 独占创建 + 0700 文件（unix）。
///
/// 移植 HelperManager.ts:872-882 的 TOCTOU 加固（`mkdirSync(mode:0700)` + `writeFileSync(mode:0700, flag:'wx')`）。
/// 提权脚本的落盘字节：Windows 前置 UTF-8 BOM，unix 原样。
///
/// **参数化而非 `cfg!`**：BOM 这件事的正确性只在 Windows 上有后果，而 Windows 分支在 Linux 上
/// 根本不编译 —— 用 `#[cfg]` 写就等于「本机永远测不到、CI 交叉 check 也只能看编不编得过」。
/// 收成参数后两个分支在 Linux 上都跑得到，判据落在纯逻辑上（同 `system-integration` 那条
/// 「命令构造与输出解析全是纯函数」的纪律）。
fn script_bytes(content: &str, windows: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(content.len() + 3);
    if windows {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    out.extend_from_slice(content.as_bytes());
    out
}

fn write_secure_script(dir: &Path, name: &str, content: &str) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        // 私有目录收紧到 0700（best-effort：已 0700 或无权改则忽略，对齐 Polaris try/catch chmod）。
        let _ = std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    }
    let path = dir.join(format!("{}-{name}", random_suffix()));
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        // O_EXCL（create_new）拒绝已存在文件/符号链接抢占；0700。
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)?;
        // unix 侧**绝不能**加 BOM：`#!/bin/bash` 前面多三个字节，shebang 就不再是首字节。
        f.write_all(&script_bytes(content, false))?;
    }
    #[cfg(not(unix))]
    {
        use std::io::Write;
        // win：os.tmpdir 落 .ps1；create_new 拒绝抢占（win 无 mode）。
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        // 🔴 **必须带 UTF-8 BOM**：`uac_escalation` 用的是系统自带的 `powershell.exe`（= Windows
        // PowerShell 5.1），它对**无 BOM** 的 .ps1 按系统 **ANSI 代码页**解码，不是 UTF-8。
        //
        // 后果不是「注释乱码」这种观感问题：脚本正文里的 `--confdir "<app config dir>"` 含用户 profile
        // 路径，中文账户（`C:\Users\张三\...`）的 UTF-8 字节被按 CP936 解出另一串汉字 ⇒ 服务
        // `BinaryPathName` 指向不存在的目录 ⇒ 之后每次起核都被 helper 的 `cfg_allowed` 判 denied，
        // 而安装本身「成功」。
        //
        // NSIS 安装态下 app 本体也在 `%LOCALAPPDATA%`（当前形态 `installMode: currentUser`）
        // ⇒ `$helperSrc` 同样中招，会更早死在 `Copy-Item`。
        //
        // 🔮 前瞻：若将来改 `perMachine`，app 与随包 helper 会落 `%PROGRAMFILES%\Polaris`、路径里
        // 不再有用户名 ⇒ **`$helperSrc` 这一条前提失效，但本注释的结论不变**：BOM 仍然必须带，因为
        // `--confdir` 指的是 app config 目录（`%APPDATA%\com.polaris.app\…`，永远在用户 profile 下、
        // 永远可能含中文），那条才是主因，与装机形态无关。
        // 先写下来，是因为「前提没了但结论仍成立」最容易在下一次重构时被误读成「这段可以删了」。
        //
        // 同仓姊妹腿早就踩过并修好了，只是没推广到这一条：`src-tauri/src/runtime/update_install.rs`
        // 的 `utf16le_with_bom` —— 那里的文档逐字写着 `wscript.exe` 按系统代码页解释无 BOM 脚本、
        // 中文用户名路径会让 `fso.CopyFile` 找不到文件。同一根因，两条腿只修了一条。
        //
        // 选 UTF-8 BOM 而非 UTF-16LE：内容本来就是 Rust 的 UTF-8 `str`，加三字节前缀即可，
        // 无需转码；PowerShell 5.1 与 7 都认这个 BOM。
        f.write_all(&script_bytes(content, true))?;
    }
    Ok(path)
}

// ===== 转义工具 =====

/// XML 字符转义（plist label/路径含 `<`/`&`/`"` 时防破坏 plist，移植 HelperManager.ts:744-750）。
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// PowerShell 单引号字符串转义（`''` 转义单引号，移植 WindowsServiceHelper.ts:417）。
fn ps_quote(s: &str) -> String {
    s.replace('\'', "''")
}

// `win_basename`（Windows 语义 basename）已随「安装落点单一真相源」收口删除：安装目标文件名
// 不再从 `src_binary` 现算，而是与 `InstallPaths::win()` 同取 `WIN_HELPER_EXE`。

// ===== mac 脚本（移植 HelperManager.ts:772-852）=====

/// mac helper daemon plist 渲染（`Label`/`ProgramArguments`/`KeepAlive`/`RunAtLoad`）。
///
/// **本 crate 侧的 plist 单一 render**（与 daemon 侧 `launchd::render_plist` 语义等价 —— 见报告 DESIGN-REVIEW：
/// launchd.rs 的 render 在 `cfg(any(target_os="macos",test))` 模块内，作为依赖编入时非 mac target 不可见，
/// 故无法跨 crate 复用于 Linux 宿主可测的 install 路径；两处 render 须保持等价直至后续收敛）。
///
/// `program` = HELPER_DEST（daemon 二进制特权路径）；argv = `--singbox <core/sing-box> --confdir <conf>
/// --support <support> --coredir <core>`（对齐 daemon `macos/daemon.rs:parse_args` flag 名）。
fn render_mac_plist(
    program: &str,
    core_dir: &str,
    support_dir: &str,
    upgrade_closed: bool,
) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key><string>{label}</string>\n\
  <key>ProgramArguments</key>\n\
  <array>\n\
    <string>{program}</string>\n\
    <string>--support</string><string>{support}</string>\n\
    <string>--coredir</string><string>{coredir}</string>\n\
{closed_argument}  </array>\n\
  <key>KeepAlive</key><true/>\n\
  <key>RunAtLoad</key><true/>\n\
</dict>\n\
</plist>",
        label = xml_escape(SERVICE_LABEL),
        program = xml_escape(program),
        support = xml_escape(support_dir),
        coredir = xml_escape(core_dir),
        closed_argument = if upgrade_closed {
            "<string>--upgrade-closed</string>\n"
        } else {
            ""
        },
    )
}

mod install_unix;
use install_unix::{
    build_linux_install_script, build_mac_install_script, build_mac_migration_script,
};

// ===== linux 脚本（移植 LinuxServiceHelper.ts:308-370）=====

/// linux systemd unit（移植 LinuxServiceHelper.ts:308-328）。
///
/// helper 以 root 跑（无 `User=`）：需 root 才能 setuid 拉 child + 穿越登录用户 userData 校验/重定向；
/// child 的 CAP_NET_ADMIN 由 helper 代码经 AmbientCaps 赋予（不在 unit 层）。`ExecStart` flag 对齐
/// daemon `linux/daemon.rs:parse_args`（`--socket`/`--authfile`/`--coredir`）。
fn build_linux_unit(paths: &InstallPaths) -> String {
    let helper_dest = paths.binary.to_string_lossy();
    let socket = paths.socket.to_string_lossy();
    let core_dir = paths.core_dir.to_string_lossy();
    // RuntimeDirectory 名 = socket 父目录末段（/run/polaris → polaris）。
    let runtime_name = paths
        .socket
        .parent()
        .and_then(|p| p.file_name())
        .map_or_else(
            || "polaris".to_owned(),
            |n| n.to_string_lossy().into_owned(),
        );
    format!(
        "[Unit]\n\
Description=Polaris privileged network helper\n\
Documentation=https://github.com/polaris-arch/Polaris\n\
After=network.target\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={helper_dest} --socket={socket} --authfile={authfile} --coredir={core_dir}\n\
RuntimeDirectory={runtime_name}\n\
RuntimeDirectoryMode=0755\n\
Restart=on-failure\n\
RestartSec=2\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n",
        helper_dest = helper_dest,
        socket = socket,
        authfile = LINUX_AUTH_FILE,
        core_dir = core_dir,
        runtime_name = runtime_name,
    )
}

mod linux_service_stop;
/// Fixed, shared hierarchy/namespace proof used by the sealed Linux dispatcher.
/// Its consumer validates its journal-bound mask before invoking the common body.
#[doc(hidden)]
pub const LINUX_SERVICE_CLOSURE_FUNCTIONS: &str = linux_service_stop::FUNCTIONS;

/// The same canonical unit bytes as the ordinary producer, without caller paths.
#[doc(hidden)]
pub fn linux_canonical_unit() -> String {
    build_linux_unit(&InstallPaths::linux())
}
mod uninstall_unix;

// ===== win 脚本（移植 WindowsServiceHelper.ts:429-517）=====

/// win 安装脚本（PowerShell，经 UAC 提权跑）。忠实迁自 WindowsServiceHelper.ts:429-506。
///
/// 关键步骤：外置 helper.exe 到 `ProgramData\Polaris`（与 app 生命周期解耦）；锁目录/文件 ACL
/// （SYSTEM/Admin 私有）；升级前快照旧服务并备份 helper/token；幂等停删与重建。任一步失败时恢复
/// 旧文件、旧 binPath/start mode 与原运行态，避免覆盖升级把可用 helper 留成半安装态。
/// `$ErrorActionPreference = Stop` 让失败以非零退出透出（提权 executor 归类 Failed；privilege.rs 的
/// uac_escalation 无 上游的 flag-file 错误回写协议 —— 见报告 DESIGN-REVIEW）。
#[cfg(test)]
mod install_windows;
#[cfg(test)]
use install_windows::build_win_install_script;

// ===== 默认 SysOps（生产实现，跨平台可编译）=====

/// `sc.exe <args>`，**带 `CREATE_NO_WINDOW`**（winbase.h `0x0800_0000`）。
///
/// 本 crate 是库，跑在宿主 GUI 进程里（`src-tauri/src/main.rs` 的 `windows_subsystem = "windows"`
/// ⇒ 进程自身无控制台）。无控制台的父进程起 console 子系统程序时，`CreateProcess` 会**新分配一个
/// 控制台窗口** —— 装/卸/启停/探活 helper 服务每次都在用户桌面上闪一个黑框。
/// std **没有**任何隐含抑制，必须显式给这个标志（同款先例：`helper/src/platform/windows/winproc/win.rs`）。
///
/// 四个 `sc` 调用点全部经此构造：新增调用点若绕开它，`sc_calls_never_pop_a_console_window` 会红。
#[cfg(target_os = "windows")]
fn sc_command(args: [&str; 2]) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new("sc");
    cmd.args(args).creation_flags(0x0800_0000);
    cmd
}

/// 生产 SysOps：用 std::fs + Command 做真系统操作。
///
/// mac/linux/win 的服务启停命令各异，这里按 target 分支。测试用 MockSysOps。
pub struct StdSysOps;

impl SysOps for StdSysOps {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn start_service(&self, label: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let plist = format!("/Library/LaunchDaemons/{label}.plist");
            std::process::Command::new("launchctl")
                .args(["bootstrap", "system", &plist])
                .status()
                .map_err(|e| format!("launchctl bootstrap 失败: {e}"))?
                .success()
                .then_some(())
                .ok_or_else(|| "launchctl bootstrap 非零退出".to_owned())
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("systemctl")
                .args(["start", label])
                .status()
                .map_err(|e| format!("systemctl start 失败: {e}"))?
                .success()
                .then_some(())
                .ok_or_else(|| "systemctl start 非零退出".to_owned())
        }
        #[cfg(target_os = "windows")]
        {
            sc_command(["start", label])
                .status()
                .map_err(|e| format!("sc start 失败: {e}"))?
                .success()
                .then_some(())
                .ok_or_else(|| "sc start 非零退出".to_owned())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = label;
            Err("不支持的平台".to_owned())
        }
    }

    fn stop_service(&self, label: &str) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            let plist = format!("/Library/LaunchDaemons/{label}.plist");
            let _ = std::process::Command::new("launchctl")
                .args(["bootout", "system", &plist])
                .status();
            Ok(()) // bootout 即便未加载也忽略（上游 `2>/dev/null || true`）
        }
        #[cfg(target_os = "linux")]
        {
            let _ = std::process::Command::new("systemctl")
                .args(["stop", label])
                .status();
            Ok(())
        }
        #[cfg(target_os = "windows")]
        {
            let _ = sc_command(["stop", label]).status();
            Ok(())
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = label;
            Err("不支持的平台".to_owned())
        }
    }

    fn is_loaded(&self, label: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            // Polaris: launchctl print system/<LABEL> 退出码 0=已加载（HelperManager.ts:142-148）
            std::process::Command::new("launchctl")
                .args(["print", &format!("system/{label}")])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(target_os = "linux")]
        {
            std::process::Command::new("systemctl")
                .args(["is-active", "--quiet", label])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(target_os = "windows")]
        {
            sc_command(["query", label])
                .output()
                .map(|o| {
                    let out = String::from_utf8_lossy(&o.stdout);
                    out.contains("RUNNING")
                })
                .unwrap_or(false)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = label;
            false
        }
    }

    fn service_exists(&self, label: &str) -> bool {
        #[cfg(target_os = "macos")]
        {
            // launchd：服务定义即 plist 落盘；已 bootout 但 plist 还在也算装过。
            std::path::Path::new(&format!("/Library/LaunchDaemons/{label}.plist")).exists()
        }
        #[cfg(target_os = "linux")]
        {
            // `systemctl cat` 只要 unit 存在就 0（停着也 0），is-active 会漏掉停着的。
            std::process::Command::new("systemctl")
                .args(["cat", label])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(target_os = "windows")]
        {
            // `sc query` 服务不存在时退出码 1060；存在则 0（无论 RUNNING/STOPPED）。
            // 故判退出码而非像 is_loaded 那样扫 stdout 里的 "RUNNING"。
            sc_command(["query", label])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = label;
            false
        }
    }
}

#[cfg(test)]
mod tests;
