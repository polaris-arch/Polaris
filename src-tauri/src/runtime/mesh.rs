//! mesh 运行时：`polaris-mesh` 装配（WARP / Tailscale / exit-route）。
//!
//! Polaris 锚点：
//! - `main/services/WarpService.ts` → `polaris_mesh::warp_http::WarpService`（匿名设备注册 → WG 草稿）
//! - `main/services/tailscale-state.ts` → `polaris_mesh::tailscale_state`（TS 节点 state 目录管理）
//! - `MeshExitRouteManager` → `polaris_mesh::exit_route`（mesh 出口路由接管 / 让位）
//!
//! 纯逻辑纪律：mesh crate 的 HTTP/FS/keypair 经 trait 抽象（`UnlockHttp` / `TailscaleStateFs` /
//! `ExitRouteOp`），本层注入真实实现。注册 WARP 需真实 HTTP + keypair 生成（Curve25519），
//! 属系统交互批次；本层提供 tailscale state（纯文件操作，注入 [`StdTailscaleFs`]）+ 命令入口。

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use polaris_mesh::tailscale_state::TailscaleStateFs;
#[cfg(not(any(target_os = "ios", windows)))]
use polaris_mesh::tailscale_state::{state_exists, tailscale_state_dir};
use polaris_mesh::warp::{
    enqueue_pending_deregister, plan_deregister_drain, DeregisterResult, DrainAction,
    DrainPlanItem, PendingDeregisterEntry,
};
use polaris_mesh::warp_http::{
    WarpHttp, WarpHttpRequest, WarpHttpResponse, WarpKeypair, WarpLog, WarpService,
};
use polaris_mesh::{ExitRouteCancel, ExitRouteLog, ExitRouteOp, MeshExitRouteManager, Platform};
use tokio::sync::Mutex as AsyncMutex;

use crate::runtime::helper::HelperRuntime;
use crate::runtime::http::HttpRuntime;
use crate::runtime::tailscale_login_core::{
    AppHandleEmitter, LoginCoreRegistry, StartLoginOutcome,
};
use crate::runtime::tailscale_status::{TailscaleStatusEvent, TailscaleStatusSnapshot};
use crate::runtime::vpn_status::{OpenConnectStatusEvent, OpenVpnStatusEvent, VpnStatusSnapshot};
use crate::runtime::x25519;
use polaris_config_engine::user_config::app_config::UserConfig;
use polaris_config_engine::user_config::server_config::ServerConfig;
use tauri::AppHandle;

/// 基于 `std::fs` 的 [`TailscaleStateFs`] 实现（应用层注入）。
/// Polaris 用 `fs.readdirSync(dir)` 直接读盘；失败安全返 None（对齐 Polaris catch → false）。
struct StdTailscaleFs;

fn same_auth_file(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    if !after.is_file()
        || after.file_type().is_symlink()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        before.dev() == after.dev() && before.ino() == after.ino()
    }
    #[cfg(not(unix))]
    {
        before.created().ok() == after.created().ok()
    }
}

impl TailscaleStateFs for StdTailscaleFs {
    fn read_dir_names(&self, dir: &Path) -> Option<Vec<String>> {
        std::fs::read_dir(dir)
            .ok()?
            .map(|res| {
                res.ok()
                    .map(|e| e.file_name().to_string_lossy().into_owned())
            })
            .collect()
    }
}

/// WARP 待注销队列 drain 周期（启动先跑一次，之后按此间隔）。Polaris 无显式常量（按事件 + 定时驱动），
/// 取 1h 折中：孤儿设备清理不必激进（`WARP_DEREGISTER_MAX_AGE_MS`=7 天护栏 + 单次 `MAX_PER_DRAIN`=10 限流
/// 已避免 hammer CF），过密只徒增 CF 1020 风险。真机可再校准。
const WARP_DRAIN_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// 组网登录期出口让位判定输入（上游 `shared/mesh-login-fallback.ts` `MeshLoginFallbackInput` 1:1 镜像）。
///
/// 全部字段与「default=proxy-selector→未连上 TS 出口」这一死锁形态一一对应（见 [`mesh_login_fallback_should_engage`]）。
/// 纯输入、无 I/O：便于单测 + 单一真值防漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshLoginFallbackInput {
    /// 开关：`meshLoginFallbackDirect !== false`（默认开）。
    pub fallback_enabled: bool,
    /// 当前是否 direct 代理模式（default 本就 direct，不适用让位）。
    pub proxy_mode_direct: bool,
    /// 选中出口是否已「回退直连」（`meshSelectedExitFallsBackToDirect`：off-mesh / 仅子网段组网节点）。
    pub selected_exit_falls_back_direct: bool,
    /// 选中出口是否为 Tailscale 协议。
    pub selected_is_tailscale: bool,
    /// 选中 TS 是否配置了 authKey（静态凭据，无交互登录死锁）。
    pub selected_has_auth_key: bool,
    /// 选中 TS 隧道是否已就绪（STATUS backendState=Running）。
    pub selected_tunnel_ready: bool,
}

/// 是否应让默认路由让位直连（引导期）。上游 `meshLoginFallbackShouldEngage`（1:1 移植）。
///
/// 场景（缺陷 1）：选中出口为账号制 Tailscale 且承载全隧道时，proxy-selector.default = 该 TS endpoint。
/// 隧道尚未 Running（未登录/未授权/netmap 未同步）时，浏览器授权页与引导期控制平面流量被导向这个「尚未
/// 连上的出口」→ 授权页打不开 → 授权永不完成 → 引导链死锁。治法：就绪前把默认路由临时热切 direct（零重启），
/// Running 后切回。本谓词判「配置层是否符合让位形态」；就绪与否（tunnel_ready）由 reconcile 按 backendState 决策。
#[must_use]
pub fn mesh_login_fallback_should_engage(i: &MeshLoginFallbackInput) -> bool {
    i.fallback_enabled
        && !i.proxy_mode_direct
        && !i.selected_exit_falls_back_direct
        && i.selected_is_tailscale
        && !i.selected_has_auth_key
        && !i.selected_tunnel_ready
}

/// mesh 运行时（`State`-managed，单实例）。
pub struct MeshRuntime {
    /// 配置根（`<app_config_dir>/polaris/`）。tailscale state 子目录由 crate 自算 `<root>/tailscale/<id>`。
    config_dir: PathBuf,
    /// warp 待注销队列持久化路径。
    warp_queue_path: PathBuf,
    /// warp 队列文件读改写串行化锁（enqueue 同步命令线程 + drain 异步任务共享同一队列文件，
    /// 防交错丢更新）。锁只护「读→改→写」临界段，**绝不跨 await 持有**（drain 的网络调用在锁外）。
    warp_queue_lock: Mutex<()>,
    /// 新注销条目入队时立即唤醒 drain；定时 tick 只作网络失败后的兜底重试。
    warp_queue_changed: tokio::sync::Notify,
    /// Tailscale 瞬态登录核生命周期注册表（与 `ProxyRuntime` 常驻代理核隔离）。
    login_registry: LoginCoreRegistry,
    /// C5 mesh 出口路由托管状态机（`MeshExitRouteManager`，1:1 移植自 上游 `MeshExitRouteManager`）。
    /// async `Mutex`：其 `reconcile`/`clear`/`reassert` 是 `&mut self` async（macOS apply 轮询接口可达
    /// 跨 await），须异步锁串行化独占访问。**OS 路由真操作经 [`HelperExitRouteOp`]**：`MeshRuntime::new`
    /// （测试/未接线默认）注入 `enabled=false` 的诚实 no-op op（`installed` 恒 None，绝不碰宿主网络）；
    /// `MeshRuntime::new_with_helper`（生产 `AppRuntime::new`）注入 `enabled=true` op（真三平台 route 手术，真机门）。
    exit_route: AsyncMutex<MeshExitRouteManager<HelperExitRouteOp, LogExitRouteLog>>,
    /// 出口路由在飞作业的取消令牌（**锁外**句柄，与状态机内那份是同一个 `Arc`）。
    ///
    /// 存在的唯一理由是「取消必须在拿到锁之前就能发出」：macOS 反查轮询持锁最长 18s，若取消信号
    /// 也要先拿锁才发得出去，它就得排在那 18s 后面 = 什么都没解决。见 [`polaris_mesh::ExitRouteCancel`]。
    exit_route_cancel: Arc<ExitRouteCancel>,
    /// 出口路由 op 的调用计数（供接线单测断言「生命周期腿真触达状态机」；生产侧仅原子自增，可忽略）。
    #[cfg(test)]
    exit_route_stats: Arc<ExitRouteOpStats>,
    /// A3：Tailscale STATUS 流末帧缓存（各在册 TS 节点的解码事件）。
    ///
    /// `None` = 尚无帧（核未起 / 起后尚未收到首帧 / 停核已清）。relay（`runtime/proxy::spawn_tailscale_status_relay`）
    /// 每收一帧全量端点快照即整体替换（`update_ts_status`），停核清空（`clear_ts_status`）；
    /// `tailscale_get_status` 命令读它（配合核 running 态给出 `connected`）。
    /// `RwLock`：relay 单写、命令多读，无跨 await 持锁。
    ts_status: RwLock<Option<Vec<TailscaleStatusEvent>>>,
    /// OpenConnect/OpenVPN 原生状态末帧缓存。两条流各自是全量快照，故独立整体替换；停核/崩溃同刻清空。
    openconnect_status: RwLock<Option<Vec<OpenConnectStatusEvent>>>,
    openvpn_status: RwLock<Option<Vec<OpenVpnStatusEvent>>>,
}

impl MeshRuntime {
    pub fn begin_shutdown(&self) {
        self.login_registry.begin_shutdown();
    }

    pub async fn shutdown_for_exit(&self) -> Result<(), String> {
        self.login_registry.shutdown_for_exit().await
    }

    /// 测试/未接线默认构造：出口路由 op **禁用**（`enabled=false`，helper=None）——诚实 no-op，绝不 shell
    /// 任何 `ip`/`route` 命令、绝不碰宿主网络。生产装配走 [`Self::new_with_helper`]（注入 helper + 启用真手术）。
    #[cfg(test)]
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        let stats = Arc::new(ExitRouteOpStats::default());
        let op = HelperExitRouteOp {
            helper: None,
            platform: Platform::current(),
            enabled: false,
            stats: stats.clone(),
        };
        Self::from_parts(config_dir, op, stats)
    }

    /// 生产构造（`AppRuntime::new`）：注入就绪 helper → 出口路由 op **启用**（`enabled=true`）。
    /// 此后 `exit_route_reconcile`/`exit_route_clear` 会真做三平台 route 手术（mac/win 经 helper `route -ifscope`、
    /// Linux app 自身 `ip route` 独立表 7732）——属**真机门**（本机开发/单测路径永不经此构造）。
    #[must_use]
    pub fn new_with_helper(config_dir: PathBuf, helper: Arc<HelperRuntime>) -> Self {
        let stats = Arc::new(ExitRouteOpStats::default());
        let op = HelperExitRouteOp {
            helper: Some(helper),
            platform: Platform::current(),
            enabled: true,
            stats: stats.clone(),
        };
        Self::from_parts(config_dir, op, stats)
    }

    /// 两构造共用装配（仅出口路由 op 不同）。
    fn from_parts(
        config_dir: PathBuf,
        op: HelperExitRouteOp,
        _exit_route_stats: Arc<ExitRouteOpStats>,
    ) -> Self {
        let warp_queue_path = config_dir.join("warp-deregister-queue.json");
        let manager = MeshExitRouteManager::new(op, LogExitRouteLog, Platform::current());
        // 取消令牌由状态机自持，此处取同一个 Arc 的锁外句柄（不是第二份状态）。
        let exit_route_cancel = manager.cancel_handle();
        // A login core left by a killed host process still writes its node's state directory.
        let login_registry = LoginCoreRegistry::production().with_stale_login_sweeper(Arc::new(
            crate::runtime::tailscale_login_core::ProcessStaleLoginSweeper::new(config_dir.clone()),
        ));
        Self {
            config_dir,
            warp_queue_path,
            warp_queue_lock: Mutex::new(()),
            warp_queue_changed: tokio::sync::Notify::new(),
            login_registry,
            exit_route: AsyncMutex::new(manager),
            exit_route_cancel,
            #[cfg(test)]
            exit_route_stats: _exit_route_stats,
            ts_status: RwLock::new(None),
            openconnect_status: RwLock::new(None),
            openvpn_status: RwLock::new(None),
        }
    }

    /// A3：relay 收到一帧全量 TS 端点快照 → 整体替换末帧缓存（非增量：每帧即全量）。
    pub fn update_ts_status(&self, statuses: Vec<TailscaleStatusEvent>) {
        if let Ok(mut g) = self.ts_status.write() {
            *g = Some(statuses);
        }
    }

    /// A3：停核 → 清 TS 状态末帧缓存（陈旧 live 数据不再供 `tailscale_get_status`；核未跑即诚实空）。
    pub fn clear_ts_status(&self) {
        if let Ok(mut g) = self.ts_status.write() {
            *g = None;
        }
    }

    pub fn update_openconnect_status(&self, statuses: Vec<OpenConnectStatusEvent>) {
        if let Ok(mut guard) = self.openconnect_status.write() {
            *guard = Some(statuses);
        }
    }

    pub fn update_openvpn_status(&self, statuses: Vec<OpenVpnStatusEvent>) {
        if let Ok(mut guard) = self.openvpn_status.write() {
            *guard = Some(statuses);
        }
    }

    /// 核会话结束时一次清掉两类 VPN 原生状态，防认证挑战跨会话复用。
    pub fn clear_vpn_status(&self) {
        if let Ok(mut guard) = self.openconnect_status.write() {
            *guard = None;
        }
        if let Ok(mut guard) = self.openvpn_status.write() {
            *guard = None;
        }
    }

    #[must_use]
    pub fn vpn_status_snapshot(&self, connected: bool) -> VpnStatusSnapshot {
        let open_connect = self
            .openconnect_status
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_default();
        let open_vpn = self
            .openvpn_status
            .read()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_default();
        VpnStatusSnapshot {
            connected,
            open_connect,
            open_vpn,
        }
    }

    /// 提交/取消前的挑战新鲜度门：serverId + challengeID 必须仍与末帧一致。
    #[must_use]
    pub fn has_openconnect_challenge(&self, server_id: &str, challenge_id: &str) -> bool {
        self.openconnect_status.read().ok().is_some_and(|guard| {
            guard.as_ref().is_some_and(|statuses| {
                statuses.iter().any(|status| {
                    status.server_id == server_id
                        && status
                            .auth_challenge
                            .as_ref()
                            .is_some_and(|challenge| challenge.id == challenge_id)
                })
            })
        })
    }

    #[must_use]
    pub fn has_openvpn_challenge(&self, server_id: &str, challenge_id: &str) -> bool {
        self.openvpn_status.read().ok().is_some_and(|guard| {
            guard.as_ref().is_some_and(|statuses| {
                statuses.iter().any(|status| {
                    status.server_id == server_id
                        && status
                            .challenge
                            .as_ref()
                            .is_some_and(|challenge| challenge.id == challenge_id)
                })
            })
        })
    }

    /// A3：`TAILSCALE_GET_STATUS` 拉缓存末帧 + 新鲜度。`connected` 由调用方传入（= 主核是否在运行，
    /// 即状态流是否 live）——缓存本身不含 running 态，二者在命令层合成。缓存空（无帧/已清）→ `statuses: []`。
    #[must_use]
    pub fn tailscale_status_snapshot(&self, connected: bool) -> TailscaleStatusSnapshot {
        let statuses = self
            .ts_status
            .read()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or_default();
        TailscaleStatusSnapshot {
            connected,
            statuses,
            main_core_hold: None, // STATUS cache alone cannot attest main-core custody.
        }
    }

    /// A4：选中出口 STATUS 末帧 backendState（读缓存）。上游 `selectedExitBackendState`。
    ///
    /// `expired` → 视作 `"NeedsLogin"`（key 过期即便 backendState 仍 Running 也须重新交互登录，否则过期后
    /// 走死出口黑洞）。无该端点帧（核未起 / 未选中 TS / 首帧未到）→ `None`。登录期出口让位对账据此三态决策。
    #[must_use]
    pub fn selected_exit_backend_state(&self, selected_id: &str) -> Option<String> {
        let guard = self.ts_status.read().ok()?;
        let statuses = guard.as_ref()?;
        let ev = statuses.iter().find(|e| e.server_id == selected_id)?;
        if ev.expired {
            return Some("NeedsLogin".to_string());
        }
        Some(ev.backend_state.clone())
    }

    /// **廉价存在性探问**：末帧缓存里是否有任何在册 TS 端点（`None` / 空 vec → false）。
    ///
    /// 存在的理由是**每帧调用方的开销**：`runtime/proxy::reconcile_ts_exit_block` 由 STATUS relay 每帧
    /// （~1/s）驱动，其判定需要深拷贝整份配置（含 200 节点级 `servers` 数组）+ 反序列化。而无任何 TS
    /// 帧时该判定的结果**恒为「无告警」**（`derive_ts_exit_warning` 在 `logged_in=false` 时提前返回），
    /// 故用这个只读锁 + 一次 `is_empty` 把绝大多数用户（不用 Tailscale）的那份常驻开销整个挡掉。
    /// **只跳过工作、绝不改变结论**（等价性由 `exit_block_is_none_when_status_cache_empty` 钉住）。
    #[must_use]
    pub fn has_ts_status(&self) -> bool {
        self.ts_status
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|v| !v.is_empty()))
            .unwrap_or(false)
    }

    /// item6：某在册 TS 节点的 STATUS 末帧（供选中出口无效直判读 `peers`/`logged_in`）。
    /// 无帧（核未起/未收首帧/已清）/ 未在册 → None。`RwLock` 读，clone 出帧不持锁跨用。
    #[must_use]
    pub fn ts_status_event(&self, server_id: &str) -> Option<TailscaleStatusEvent> {
        let guard = self.ts_status.read().ok()?;
        guard
            .as_ref()?
            .iter()
            .find(|e| e.server_id == server_id)
            .cloned()
    }

    #[cfg(windows)]
    pub(crate) fn directory_in_use(&self, id: &str, main_alive: bool) -> bool {
        self.login_registry.state_in_use(id, main_alive)
    }

    /// 某节点 tailscale state 目录（`<config_dir>/tailscale/<server_id>`）。
    pub fn tailscale_state_dir(&self, server_id: &str) -> std::io::Result<PathBuf> {
        #[cfg(target_os = "ios")]
        {
            crate::runtime::tailscale_directory::DirectoryLease::actual_directory(
                &self.config_dir,
                server_id,
            )
        }
        #[cfg(windows)]
        {
            crate::runtime::windows_directory::actual_directory(&self.config_dir, server_id)
        }
        #[cfg(not(any(target_os = "ios", windows)))]
        tailscale_state_dir(&self.config_dir, server_id).map_err(|error| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, error.to_string())
        })
    }

    /// 批量查 TS 节点 state 目录存在性（上游 `tailscale:stateExists`，纯文件存在性判定）。
    pub fn tailscale_state_exists(
        &self,
        server_ids: &[String],
    ) -> std::collections::HashMap<String, bool> {
        let fs = StdTailscaleFs;
        let mut out = std::collections::HashMap::new();
        for id in server_ids {
            #[cfg(target_os = "ios")]
            let exists = crate::runtime::tailscale_directory::DirectoryLease::acquire(
                &self.config_dir,
                id,
                false,
            )
            .ok()
            .and_then(|lease| fs.read_dir_names(&lease.directory_path()))
            .is_some_and(|names| {
                names
                    .iter()
                    .any(|name| name != ".polaris-directory-incarnation")
            });
            #[cfg(windows)]
            let exists = self
                .tailscale_state_dir(id)
                .ok()
                .and_then(|path| fs.read_dir_names(&path))
                .is_some_and(|names| {
                    names
                        .iter()
                        .any(|name| name != ".polaris-directory-incarnation")
                });
            #[cfg(not(any(target_os = "ios", windows)))]
            let exists = state_exists(&fs, &self.config_dir, id);
            out.insert(id.clone(), exists);
        }
        out
    }

    /// Physical deletion is available only under the shared gate and after state writers exit.
    pub(crate) fn tailscale_logout_under_gate(
        &self,
        server_id: &str,
        _gate: &tokio::sync::MutexGuard<'_, ()>,
        main_alive: bool,
    ) -> std::io::Result<()> {
        if self.login_registry.state_in_use(server_id, main_alive) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Tailscale state is in use",
            ));
        }
        let dir = self.tailscale_state_dir(server_id)?;
        if !dir.exists() {
            return Ok(());
        }
        // 目录从未建出时无物可删；已存在的目录在这两个平台上只能经绑定意图与 custody 删除。
        if cfg!(any(target_os = "ios", windows)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "CleanupUnknown: whole-directory deletion requires bound intent and custody",
            ));
        }

        // 单路径组件堵住直接逃逸；canonical containment 再堵住 `tailscale` 父目录或目标目录被
        // symlink/junction 替换后的间接逃逸。先验检查失败时宁可留下状态，也绝不递归删除边界外目录。
        let config_root = self.config_dir.canonicalize()?;
        let state_root = self.config_dir.join("tailscale").canonicalize()?;
        let target = dir.canonicalize()?;
        if !state_root.starts_with(&config_root)
            || target == state_root
            || !target.starts_with(&state_root)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Tailscale state path escaped its configured root",
            ));
        }
        std::fs::remove_dir_all(&dir)
    }

    /// Authentication retirement keeps Taildrop, machine identity, other
    /// profiles, and unknown FileStore entries. Whole-node deletion above is
    /// reserved for the existing explicit node-deletion journal.
    pub(crate) fn tailscale_retire_auth_under_gate(
        &self,
        server_id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        keep: Option<&Arc<crate::runtime::tailscale_login_core::Attempt>>,
        provenance: polaris_mesh::tailscale_state::AuthProjectionProvenance<'_>,
        sealed_revision: Option<&str>,
    ) -> std::io::Result<()> {
        self.tailscale_retire_auth_at_directory_under_gate(
            server_id,
            gate,
            keep,
            provenance,
            sealed_revision,
            None,
        )
    }

    pub(crate) fn tailscale_retire_auth_at_directory_under_gate(
        &self,
        server_id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        keep: Option<&Arc<crate::runtime::tailscale_login_core::Attempt>>,
        provenance: polaris_mesh::tailscale_state::AuthProjectionProvenance<'_>,
        sealed_revision: Option<&str>,
        sealed_directory: Option<&str>,
    ) -> std::io::Result<()> {
        use polaris_store::fs::{durable_atomic_write, random_tmp_suffix};
        use std::io::Read;
        const MAX_BYTES: u64 = 4 * 1024 * 1024;
        let unknown = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Tailscale authentication state is unknown",
            )
        };
        #[cfg(target_os = "ios")]
        if sealed_directory.is_none() {
            return Err(unknown());
        }
        self.login_registry
            .assert_auth_state_available(server_id, gate, keep)
            .map_err(std::io::Error::other)?;
        #[cfg(unix)]
        let _directory_lease = crate::runtime::tailscale_directory::DirectoryLease::acquire(
            &self.config_dir,
            server_id,
            false,
        )?;
        #[cfg(unix)]
        let directory = _directory_lease.directory_path();
        #[cfg(windows)]
        let _windows_auth_lease =
            crate::runtime::windows_directory::auth_lease(&self.config_dir, server_id)?;
        #[cfg(not(unix))]
        let directory = self.tailscale_state_dir(server_id)?;
        #[cfg(any(target_os = "ios", target_os = "android", test))]
        if let Some(expected) = sealed_directory {
            let actual = crate::runtime::tailscale_login_core::canonical_tailscale_claim_directory(
                &directory,
            )
            .map_err(std::io::Error::other)?;
            if actual != Path::new(expected) {
                return Err(unknown());
            }
        }
        #[cfg(not(any(target_os = "ios", target_os = "android", test)))]
        let _ = sealed_directory;
        let state_root = directory.parent().ok_or_else(unknown)?.to_path_buf();
        for path in [&state_root, &directory] {
            let metadata = std::fs::symlink_metadata(path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Tailscale state path escaped its configured root",
                ));
            }
        }
        let config_root = self.config_dir.canonicalize()?;
        let root = state_root.canonicalize()?;
        let target = directory.canonicalize()?;
        if !root.starts_with(&config_root) || target.parent() != Some(root.as_path()) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "Tailscale state path escaped its configured root",
            ));
        }
        let path = target.join("tailscaled.state");
        let before = std::fs::symlink_metadata(&path)?;
        if !before.is_file() || before.file_type().is_symlink() || before.len() > MAX_BYTES {
            return Err(unknown());
        }
        let file = std::fs::File::open(&path)?;
        let opened = file.metadata()?;
        if !same_auth_file(&before, &opened) {
            return Err(unknown());
        }
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES
            || !same_auth_file(&before, &std::fs::symlink_metadata(&path)?)
        {
            return Err(unknown());
        }
        let revision = polaris_updater::sha256_hex(&bytes);
        if sealed_revision.is_some_and(|expected| expected != revision) {
            return Err(unknown());
        }
        // A sealed revision without SDK Bound is only an already-retired
        // no-write candidate. Local PC retirement has no native sealed revision.
        let require_unchanged = sealed_revision.is_some()
            && matches!(
                &provenance,
                polaris_mesh::tailscale_state::AuthProjectionProvenance::CurrentFileReferences
            );
        let projected =
            polaris_mesh::tailscale_state::project_tailscale_auth_state(&bytes, provenance)
                .map_err(|_| unknown())?;
        if require_unchanged && (projected.changed || projected.bytes != bytes) {
            return Err(unknown());
        }
        // Recheck the actual revision immediately before the existing atomic
        // writer. The caller continuously owns the real TS gate and, on iOS,
        // the captured action-generation/cancel locks across this commit.
        let mut rechecked = Vec::new();
        std::fs::File::open(&path)?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut rechecked)?;
        if !same_auth_file(&before, &std::fs::symlink_metadata(&path)?)
            || rechecked != bytes
            || directory.canonicalize()? != target
        {
            return Err(unknown());
        }
        if projected.changed {
            let text = std::str::from_utf8(&projected.bytes).map_err(|_| unknown())?;
            durable_atomic_write(&path, text, &random_tmp_suffix()).map_err(|_| unknown())?;
        }
        Ok(())
    }

    #[cfg(all(target_os = "ios", unix))]
    pub(crate) fn delete_original_tailscale_directory_under_gate(
        &self,
        intent: &polaris_mesh::tailscale_state::directory_retirement::DirectoryDeletionIntent,
        receipt: Option<&tauri_plugin_polaris_ios::TailscaleDirectoryRetirementReceipt>,
        _gate: &tokio::sync::MutexGuard<'_, ()>,
        main_alive: bool,
        current: &serde_json::Value,
    ) -> std::io::Result<crate::runtime::tailscale_directory::DirectoryLease> {
        if self
            .login_registry
            .state_in_use(&intent.server_id, main_alive)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "Tailscale state is in use",
            ));
        }
        match receipt {
            Some(receipt) => crate::runtime::tailscale_directory::commit_original_directory(
                &self.config_dir,
                intent,
                receipt,
                current,
            ),
            None => crate::runtime::tailscale_directory::resume_committed_directory(
                &self.config_dir,
                intent,
                current,
            ),
        }
    }

    /// PC/Android closure may find no FileStore at all. This is a scoped no-write
    /// observation under the actual gate, never an SDK Bound or global NoOwner receipt.
    pub(crate) fn tailscale_retire_pc_auth_under_gate(
        &self,
        id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        keep: Option<&Arc<crate::runtime::tailscale_login_core::Attempt>>,
    ) -> std::io::Result<()> {
        self.login_registry
            .assert_auth_state_available(id, gate, keep)
            .map_err(std::io::Error::other)?;
        let directory = self.tailscale_state_dir(id)?;
        let config_root = self.config_dir.canonicalize()?;
        for path in [self.config_dir.join("tailscale"), directory.clone()] {
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.is_dir()
                        || metadata.file_type().is_symlink()
                        || !path.canonicalize()?.starts_with(&config_root)
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "Tailscale state path escaped its configured root",
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        match std::fs::symlink_metadata(directory.join("tailscaled.state")) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
            Ok(_) => self.tailscale_retire_auth_under_gate(
                id,
                gate,
                keep,
                polaris_mesh::tailscale_state::AuthProjectionProvenance::CurrentFileReferences,
                None,
            ),
        }
    }

    #[cfg(any(target_os = "android", test))]
    pub(crate) fn tailscale_retire_android_auth_under_gate(
        &self,
        id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        attempt: &Arc<crate::runtime::tailscale_login_core::Attempt>,
        node: &polaris_mesh::tailscale_state::retirement::TailscaleStoreRetirementNode,
    ) -> std::io::Result<()> {
        use polaris_mesh::tailscale_state::{
            retirement::{TailscaleProfileState, TailscaleStateFileState, TailscaleWriterState},
            AuthProjectionProvenance,
        };
        self.login_registry
            .assert_auth_state_available(id, gate, Some(attempt))
            .map_err(std::io::Error::other)?;
        let directory = crate::runtime::tailscale_login_core::canonical_tailscale_claim_directory(
            &self.tailscale_state_dir(id)?,
        )
        .map_err(std::io::Error::other)?;
        if directory.to_str() != Some(node.state_directory.as_str())
            || directory.join("tailscaled.state").to_str() != Some(node.state_file.as_str())
        {
            return Err(std::io::Error::other("profileBindingUnknown"));
        }
        if node.state_file_state == TailscaleStateFileState::Missing {
            // A sealed Missing fact authorizes only no write. A newly present file
            // must not fall through to a cached-profile or unsealed projection.
            return match std::fs::symlink_metadata(&node.state_file) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(std::io::Error::other("stateRevisionChanged")),
            };
        }
        if node.state_file_state != TailscaleStateFileState::Regular {
            return Err(std::io::Error::other("profileBindingUnknown"));
        }
        if node.profile_state == TailscaleProfileState::Bound
            && node.writer_state != TailscaleWriterState::SealedDrained
        {
            return Err(std::io::Error::other("profileBindingUnknown"));
        }
        let provenance = if node.profile_state == TailscaleProfileState::Bound {
            AuthProjectionProvenance::BoundFingerprint(&node.profile_fingerprint)
        } else {
            AuthProjectionProvenance::CurrentFileReferences
        };
        self.tailscale_retire_auth_under_gate(
            id,
            gate,
            Some(attempt),
            provenance,
            Some(&node.state_file_revision),
        )
    }

    #[cfg(target_os = "ios")]
    pub(crate) async fn retire_tailscale_other_attempts_under_gate(
        &self,
        id: &str,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        keep: &Arc<crate::runtime::tailscale_login_core::Attempt>,
    ) -> Result<(), String> {
        self.login_registry
            .retire_other_attempts_under_state_gate(id, gate, keep)
            .await
    }

    /// warp 待注销队列路径（供待注销队列 actor 持久化）。
    #[cfg(test)]
    #[must_use]
    pub fn warp_queue_path(&self) -> &Path {
        &self.warp_queue_path
    }

    /// 读待注销队列（缺失/损坏 → 空，best-effort 不 panic）。**须在持 `warp_queue_lock` 时调**。
    fn load_warp_queue(&self) -> Vec<PendingDeregisterEntry> {
        match std::fs::read_to_string(&self.warp_queue_path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(queue) => queue,
                Err(e) => {
                    // 只记解析位置，不打印队列正文（正文含设备 token）。
                    log::warn!("warp 待注销队列损坏，忽略本轮内容: {e}");
                    Vec::new()
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                log::warn!("warp 待注销队列读取失败: {e}");
                Vec::new()
            }
        }
    }

    /// 原子写待注销队列（临时文件 + rename）。调用方决定失败是 best-effort 还是保留 Apply 意图重试。
    /// **须在持 `warp_queue_lock` 时调**。
    fn save_warp_queue(&self, queue: &[PendingDeregisterEntry]) -> Result<(), String> {
        let text =
            serde_json::to_string(queue).map_err(|e| format!("warp 待注销队列序列化失败: {e}"))?;
        polaris_store::fs::durable_atomic_write(
            &self.warp_queue_path,
            &text,
            &polaris_store::fs::random_tmp_suffix(),
        )
        .map(|_| ())
        .map_err(|error| format!("warp 待注销队列持久写入失败: {error}"))
    }

    /// WARP 节点删除时把远端自删凭据入待注销队列（防孤儿设备计费）。上游 `enqueuePendingDeregister`。
    /// 落盘后由 drain 循环（[`Self::spawn_warp_drain`]）在启动 + 定时 tick 时消费。队列护栏（去最旧超上限）
    /// 与「注销/丢弃/重试」分类判定全在 crate 纯逻辑（`warp.rs`），本层只做锁 + 文件 I/O 装配。
    #[cfg(test)]
    pub fn enqueue_warp_deregister(&self, device_id: &str, token: &str) {
        if let Err(error) = self.try_enqueue_warp_deregister(device_id, token) {
            log::warn!("{error}");
        }
    }

    /// 延迟删除事务使用的可靠入队腿：只有持久队列原子写成功才返回成功；否则上层保留删除意图重试。
    pub(crate) fn try_enqueue_warp_deregister(
        &self,
        device_id: &str,
        token: &str,
    ) -> Result<(), String> {
        if device_id.is_empty() || token.is_empty() {
            return Err("warp 设备注销未入队：缺少设备凭据".to_string());
        }
        let _guard = self
            .warp_queue_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let queue = self.load_warp_queue();
        let entry = PendingDeregisterEntry {
            device_id: device_id.to_string(),
            token: token.to_string(),
            enqueued_at: now_millis(),
        };
        let (next, dropped) = enqueue_pending_deregister(&queue, entry);
        for d in &dropped {
            log::warn!(
                "warp 待注销队列超上限，丢弃最旧 device={}…",
                id_prefix(&d.device_id)
            );
        }
        self.save_warp_queue(&next)?;
        self.warp_queue_changed.notify_one();
        log::debug!(
            "warp 设备注销已入队：device={}…，队列长度={}",
            id_prefix(device_id),
            next.len()
        );
        Ok(())
    }

    /// drain 一遍队列：超龄条目直接出队（不调网络）；在龄条目调 crate `WarpService::unregister`
    /// （真 CF DELETE），按返回 Done/Drop 出队、Retry 留队。**读→改→写**两段各在锁内、网络调用在锁外，
    /// 出队按精确条目匹配（reload 后 retain），故与并发 `enqueue_warp_deregister` 不丢新入队条目。
    pub async fn drain_warp_deregister_once(&self, http: &Arc<HttpRuntime>) {
        let now = now_millis();
        // ① 锁内取快照 + 算计划（纯逻辑，crate）。
        let (plan, deferred) = {
            let _guard = self
                .warp_queue_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let snapshot = self.load_warp_queue();
            if snapshot.is_empty() {
                return;
            }
            plan_deregister_drain(&snapshot, now)
        };
        let eligible = plan
            .iter()
            .filter(|item| item.action == DrainAction::Eligible)
            .count();
        let expired = plan.len().saturating_sub(eligible);
        log::debug!(
            "warp 设备注销队列 drain：eligible={eligible}, expired={expired}, deferred={}",
            deferred.len()
        );
        // ② 锁外跑网络（unregister 是 crate 纯编排 + 真 HTTP；drain 用占位种子——unregister 不碰 keypair）。
        let svc = warp_service(http.clone(), [0u8; 32]);
        let mut eligible_results: Vec<DeregisterResult> = Vec::new();
        for item in &plan {
            if item.action == DrainAction::Eligible {
                eligible_results.push(
                    svc.unregister(&item.entry.device_id, &item.entry.token)
                        .await,
                );
            }
        }
        let to_remove = plan_removals(&plan, &eligible_results);
        if to_remove.is_empty() {
            return;
        }
        // ③ 锁内 reload + 精确出队 + 回写（reload 保住网络期间的并发新入队）。
        let _guard = self
            .warp_queue_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.load_warp_queue();
        let next = retain_unresolved(current, &to_remove);
        if let Err(error) = self.save_warp_queue(&next) {
            log::warn!("{error}");
        }
        log::debug!(
            "warp 设备注销队列 drain 完成：移除={}，剩余={}",
            to_remove.len(),
            next.len()
        );
    }

    /// 启动期 drain 一次（清上次退出遗留）+ 定时 drain。经 `tauri::async_runtime::spawn` 常驻后台任务。
    /// **装配点**：`lib.rs` setup 内 `AppRuntime::new` 之后、`manage` 之前调
    /// `app_runtime.mesh.clone().spawn_warp_drain(app_runtime.http.clone());`（见交接说明）。
    pub fn spawn_warp_drain(self: Arc<Self>, http: Arc<HttpRuntime>) {
        tauri::async_runtime::spawn(async move {
            // 启动即 drain（消化上次会话遗留的孤儿设备）。
            self.drain_warp_deregister_once(&http).await;
            let mut ticker = tokio::time::interval(WARP_DRAIN_INTERVAL);
            ticker.tick().await; // 首 tick 立即返回，跳过（启动已 drain）。
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    _ = self.warp_queue_changed.notified() => {}
                }
                self.drain_warp_deregister_once(&http).await;
            }
        });
    }

    pub async fn prepare_tailscale_login(
        &self,
        server_id: &str,
        attempt_id: &str,
    ) -> Result<(), String> {
        self.login_registry.prepare(server_id, attempt_id).await
    }

    pub fn tailscale_login_progress(
        &self,
        server_id: &str,
        attempt_id: &str,
    ) -> Option<crate::runtime::tailscale_login_core::LoginProgressReceipt> {
        self.login_registry.login_progress(server_id, attempt_id)
    }

    pub async fn start_tailscale_login(
        &self,
        app: AppHandle,
        server: &ServerConfig,
        request: crate::runtime::tailscale_login_core::LoginRequest,
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        main_core: &(dyn Fn() -> crate::runtime::tailscale_login_core::MainLoginSnapshot
              + Send
              + Sync),
    ) -> StartLoginOutcome {
        self.login_registry
            .start_attempt_with_saved(
                server,
                &self.config_dir,
                request,
                saved_server,
                main_core,
                Arc::new(AppHandleEmitter { app }),
            )
            .await
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "normal login retains the frozen saved-config and epoch binding"
    )]
    pub async fn start_tailscale_login_with_normal_main(
        &self,
        app: AppHandle,
        server: &ServerConfig,
        request: crate::runtime::tailscale_login_core::LoginRequest,
        saved_server: &(dyn Fn() -> Result<ServerConfig, String> + Send + Sync),
        main_core: &(dyn Fn() -> crate::runtime::tailscale_login_core::MainLoginSnapshot
              + Send
              + Sync),
        proxy: &Arc<crate::runtime::proxy::ProxyRuntime>,
        saved: &serde_json::Value,
        identity_epoch: Option<String>,
        candidate: &serde_json::Value,
        action_generation: u64,
    ) -> StartLoginOutcome {
        self.login_registry
            .start_attempt_with_normal_main(
                server,
                &self.config_dir,
                request,
                saved_server,
                main_core,
                Arc::new(AppHandleEmitter { app }),
                proxy,
                saved,
                identity_epoch,
                candidate,
                action_generation,
            )
            .await
    }

    pub async fn cancel_tailscale_login_attempt(
        &self,
        server_id: &str,
        attempt_id: &str,
    ) -> Result<(), String> {
        self.login_registry
            .cancel_attempt(server_id, attempt_id)
            .await
    }

    pub async fn tailscale_state_gate(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.login_registry.state_gate().await
    }

    #[cfg(target_os = "android")]
    pub(crate) fn try_tailscale_state_gate(&self) -> Option<tokio::sync::MutexGuard<'_, ()>> {
        self.login_registry.try_state_gate()
    }

    pub(crate) fn mint_tailscale_main_birth(
        &self,
    ) -> crate::runtime::tailscale_login_core::MainBirthToken {
        self.login_registry.mint_main_birth()
    }

    pub(crate) async fn reserve_tailscale_main_states<'a, 'g>(
        &'a self,
        generated: &serde_json::Value,
        gate: &'g tokio::sync::MutexGuard<'a, ()>,
        token: crate::runtime::tailscale_login_core::MainBirthToken,
    ) -> Result<crate::runtime::tailscale_login_core::MainReservation<'a, 'g>, String> {
        self.login_registry
            .reserve_main_states(generated, &self.config_dir, gate, token)
            .await
    }

    pub(crate) fn release_tailscale_main_states_if_token(
        &self,
        token: &crate::runtime::tailscale_login_core::MainBirthToken,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<bool, String> {
        self.login_registry
            .release_main_states_if_token(token, gate)
    }

    #[cfg(target_os = "android")]
    pub(crate) fn bind_android_runtime_main_writer(
        &self,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        token: &crate::runtime::tailscale_login_core::MainBirthToken,
        endpoint: &serde_json::Value,
    ) -> Result<(), String> {
        self.login_registry.bind_android_runtime_main_writer(
            gate,
            token,
            &self.config_dir,
            endpoint,
        )
    }

    pub(crate) fn assert_tailscale_main_claims_drained(
        &self,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), String> {
        self.login_registry.assert_main_claims_drained(gate)
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    pub(crate) fn tailscale_main_scope_if_token(
        &self,
        token: &crate::runtime::tailscale_login_core::MainBirthToken,
        gate: &tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<Vec<(String, String, String)>, String> {
        self.login_registry.main_scope_if_token(token, gate)
    }

    #[cfg(target_os = "ios")]
    pub(crate) fn bind_tailscale_main_actual_scope(
        &self,
        token: &crate::runtime::tailscale_login_core::MainBirthToken,
        gate: &tokio::sync::MutexGuard<'_, ()>,
        digest: &str,
        runs: &[tauri_plugin_polaris_ios::TailscaleStoreRetirementInstance],
    ) -> Result<(), String> {
        self.login_registry
            .bind_main_actual_scope(token, gate, &self.config_dir, digest, runs)
    }

    #[cfg(test)]
    pub(crate) fn poison_tailscale_main_claim_lock_for_test(&self) {
        self.login_registry.poison_main_claim_lock_for_test();
    }

    #[cfg(any(target_os = "android", test))]
    pub fn main_owns_tailscale(&self, id: &str, alive: bool) -> bool {
        self.login_registry.main_owns(id, alive)
    }

    // Compatibility fixture for the original guarded auth-only body. Production
    // now also fences the original saved credential CAS in the entry below.
    #[cfg(test)]
    pub async fn logout_tailscale_safely(
        &self,
        id: &str,
        main_alive: &(dyn Fn() -> bool + Send + Sync),
        keep_attempt: Option<&str>,
    ) -> std::io::Result<bool> {
        self.login_registry
            .logout(id, main_alive, keep_attempt, |gate| {
                self.tailscale_retire_auth_under_gate(
                    id,
                    gate,
                    None,
                    polaris_mesh::tailscale_state::AuthProjectionProvenance::CurrentFileReferences,
                    None,
                )
            })
            .await
    }

    pub(crate) async fn logout_tailscale_with_credentials(
        &self,
        id: &str,
        proxy: &Arc<crate::runtime::proxy::ProxyRuntime>,
        saved: &serde_json::Value,
        generation: u64,
        keep_attempt: Option<&str>,
    ) -> std::io::Result<bool> {
        #[cfg(target_os = "android")]
        {
            if keep_attempt.is_some() {
                return Err(std::io::Error::other("Login request ownership changed"));
            }
            return self
                .login_registry
                .logout_with_android_store(id, proxy, saved, generation)
                .await
                .map(|()| true)
                .map_err(std::io::Error::other);
        }
        #[cfg(not(target_os = "android"))]
        {
            let _lease = proxy
                .tailscale_action_lease()
                .map_err(std::io::Error::other)?;
            self.login_registry
                .logout(
                    id,
                    &|| proxy.tailscale_writer_alive(),
                    keep_attempt,
                    |gate| {
                        proxy
                            .commit_tailscale_credential(saved, id, generation, None, |current| {
                                self.tailscale_retire_pc_auth_under_gate(id, gate, None)
                                    .map_err(|_| "stateRevisionChanged")?;
                                crate::runtime::tailscale_login_core::park_saved_tailscale_key(
                                    current, id,
                                )
                            })
                            .map(|_| ())
                            .map_err(std::io::Error::other)
                    },
                )
                .await
        }
    }

    #[cfg(target_os = "ios")]
    pub(crate) async fn logout_tailscale_with_normal_main(
        &self,
        id: &str,
        proxy: &Arc<crate::runtime::proxy::ProxyRuntime>,
        saved: &serde_json::Value,
        generation: u64,
    ) -> Result<(), (String, String)> {
        self.login_registry
            .logout_with_normal_main(id, proxy, saved, generation)
            .await
    }

    /// PID 保留至进程被收割；主核清扫持有同一 state gate。
    #[must_use]
    pub fn inflight_login_core_pids(&self) -> Vec<u32> {
        self.login_registry.inflight_login_pids()
    }

    /// **测试专用**：暴露登录注册表本体，供跨模块行为门登记/注销假的在飞条目。
    /// 读侧仍走生产的 [`Self::inflight_login_core_pids`]。
    #[cfg(test)]
    pub(crate) fn login_registry_for_test(&self) -> &LoginCoreRegistry {
        &self.login_registry
    }

    #[cfg(test)]
    pub(crate) fn with_login_registry_for_test(
        config_dir: PathBuf,
        registry: LoginCoreRegistry,
    ) -> Self {
        let mut runtime = Self::new(config_dir);
        runtime.login_registry = registry;
        runtime
    }

    // ── C5 mesh 出口路由生命周期腿（ProxyRuntime 核生命周期接线）───────────────────────────────
    //
    // 契约 special #37「绝不抢 sing-box 路由」的让位语义**在 crate 内建**（`plan_mesh_exit_route` 仅当
    // 选中的全局出口 = TS System + 承载全隧道时才装单条 ifscope default，其余一律 None=让位）——本层只做
    // 生命周期接线，不改让位判定。**OS 路由真操作经 [`HelperExitRouteOp`]，已全链接线**：生产构造
    // （[`Self::new_with_helper`]）下 mac/win 经 root/SYSTEM helper `route -ifscope`、Linux 经自身
    // `ip rule/route` 独立表 7732 —— 真手术、真机门；测试构造（[`Self::new`]，`enabled=false`）诚实 no-op。

    /// 起核前快照 utun 基线（macOS：时序 diff 锚点；其它平台 no-op）。ProxyRuntime 在 spawn 核**前**调用
    /// （须早于核创建 TS 内核接口）。
    pub async fn exit_route_snapshot_baseline(&self) {
        // 新一轮起核 = 上一轮的在飞反查（macOS 最长 18s）已彻底作废：先抢占再排队，否则新 start
        // 的整条起核流程要跟着那 18s 一起等（旧腿的世代守卫只挡「再开一轮」，挡不住已在轮询的那轮）。
        self.exit_route_cancel.cancel();
        self.exit_route.lock().await.snapshot_baseline().await;
    }

    /// 对齐出口路由到目标配置（起核就绪 / 切节点 / 切模式后调用，fire-and-forget，绝不抛）。
    /// 生产（`enabled`）下真做 route 手术（真机门）；测试/未接线（`enabled=false`）下诚实 no-op（`installed` 恒 None）。
    ///
    /// **不 cancel、只带凭据**：本腿与在飞那轮属**同一个核会话**，目标接口也是同一张 —— 打断它再从头
    /// 轮询一遍，总时长不变、只多一次 churn。故这里只做「排队期间是否被停核/复位抢占」的判定
    /// （凭据须在**排队之前**快照，见 [`polaris_mesh::ExitRouteCancel::token`]）。
    ///
    /// 同一份 `token` 还要**传进状态机**：拿锁后的这次判定只覆盖「排队期间」，而锁内还有
    /// `clear_inner` 的真实 await —— 那段窗口里发生的 cancel 只有靠这份凭据一路传到 `apply` 才认得出
    /// （状态机内部二次快照 = 把取消吞掉，见 `ExitRouteCancel::token` 文档）。
    pub async fn exit_route_reconcile(&self, config: &UserConfig, enable_ipv6: bool) {
        let token = self.exit_route_cancel.token();
        let mut mgr = self.exit_route.lock().await;
        if self.exit_route_cancel.is_cancelled(token) {
            log::debug!("mesh 出口路由 reconcile：排队期间已被停核/复位抢占 → 放弃本轮");
            return;
        }
        let outcome = mgr.reconcile(config, enable_ipv6, token).await;
        if outcome.changed {
            log::debug!("mesh 出口路由 reconcile：状态机判定有变更");
        }
    }

    /// 停核 / teardown：清理已装出口路由（未装成 / 禁用 op 下 `installed` 恒 None → clear_inner 早退 = 纯 no-op）。
    ///
    /// **先 cancel 再排队**：这正是「点停止最长卡 18s」的修法 —— 取消信号必须走锁外通道发出去，
    /// 在飞的 macOS 反查轮询在一个周期（1.5s）内收手，本方法随即拿到锁。
    pub async fn exit_route_clear(&self) -> bool {
        self.exit_route_cancel.cancel();
        self.exit_route.lock().await.clear().await
    }

    /// TS 出口 re-advertise 恢复腿的重申（上游 `reassert`，R3）。
    ///
    /// **生产调用点**：`runtime/proxy::ts_exit_recover_once`（R2 出口恢复腿，由 STATUS 帧的
    /// blocked→none 翻转对账驱动）。修的是 crate 侧 [`MeshExitRouteManager::reassert`] 文档所述的两个真缺口
    /// （installed 为空 = resolveIface 18s 轮询超时过 / macOS iface 已消失），**不 churn 已存路由**。
    ///
    /// **排队期间的抢占判定**（凭据在拿锁前快照）：调用方 `ts_exit_recover_once` 在调本方法前刚比过
    /// 世代，但那之后还要排 `exit_route` 这把锁 —— 恰在排队期间停核的话，`clear` 先跑完，本腿随后
    /// 醒来会看到 `installed=None` 而去**给一个已停的核重装出口路由**（Linux 下反查直接返逻辑名，
    /// 一装一个准）。世代守卫够不着这个窗口（它在锁外判、锁在它之后拿），故这里再判一次凭据。
    pub async fn exit_route_reassert(&self, config: &UserConfig, enable_ipv6: bool) {
        let token = self.exit_route_cancel.token();
        let mut mgr = self.exit_route.lock().await;
        if self.exit_route_cancel.is_cancelled(token) {
            log::debug!("mesh 出口路由 reassert：排队期间已被停核/复位抢占 → 放弃本轮");
            return;
        }
        mgr.reassert(config, enable_ipv6, token).await;
    }

    /// 崩溃 / 非正常拆除：撤销 installed 断言但保留待核清理身份。
    /// 后续 clear/reconcile 核验路由与策略规则；进程消失不等于清理完成。
    pub async fn exit_route_reset_state(&self) {
        // 崩溃拆除同样是「在飞那轮已作废」：不抢占的话，这条同步复位要排在 18s 轮询后面，
        // 而崩溃恢复腿正等着它把内存态清干净才敢重起核。
        self.exit_route_cancel.cancel();
        self.exit_route.lock().await.reset_state();
    }

    /// 出口路由当前内存态（**仅测试**观测：接线单测断言占位 op 恒不装路由）。
    #[cfg(test)]
    async fn exit_route_installed(&self) -> Option<polaris_mesh::InstalledRoute> {
        self.exit_route.lock().await.installed().cloned()
    }

    /// **仅测试**：占住 `exit_route` 锁直到被通知放手，给 `runtime::proxy::lifecycle` 的 `ProxyRuntime::stop_inner` 换代守卫
    /// 造一个**确定性** await 窗口。
    ///
    /// `stop_inner` 的拆除段里 [`exit_route_clear`](Self::exit_route_clear) 是第一个必然让出执行权的
    /// 点（`lock().await` 拿不到就一定挂起）⇒ 本方法持着锁时停核腿**不可能**越过它，于是测试可以在
    /// 那之后不慌不忙地制造一次换代，再放锁看它是否让位。没有这个窗口就只能靠 sleep 赌时序 ——
    /// 那种测试的绿是没有信息量的。
    ///
    /// 用「占位任务 + 两个 [`Notify`](tokio::sync::Notify)」而不是把 `MutexGuard` 返回给调用方：
    /// 后者要在签名里写出 `MeshExitRouteManager<HelperExitRouteOp, LogExitRouteLog>`，等于为了一条测试
    /// 把两个私有装配类型提成 `pub(crate)`。
    #[cfg(test)]
    pub(crate) async fn occupy_exit_route_lock_for_test(
        &self,
        acquired: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    ) {
        let _guard = self.exit_route.lock().await;
        acquired.notify_one();
        release.notified().await;
    }
}

// ── WARP 服务装配（注入真实 HTTP + keypair + 日志，供 warp_register/warp_apply_license 命令）─────
//
// `polaris_mesh::WarpService<H,K,L>` 是纯逻辑编排（register/applyLicense/unregister），把网络/密钥/日志
// 抽象成 trait。本层注入三个真实实现：
//   H = [`HttpWarpAdapter`]：转发到 [`HttpRuntime`] 的 `WarpHttp` 实现（reqwest+rustls，见 runtime/http.rs）。
//   K = [`SeededWarpKeypair`]：ring CSPRNG 出种子 + RFC 7748 X25519 出公钥（见 runtime/x25519.rs）。
//   L = [`LogWarpLog`]：转发到 `log` crate。

/// 把 `Arc<HttpRuntime>` 适配成 `WarpHttp`（`HttpRuntime` 已实现 `WarpHttp`，此处只做 Arc 转发）。
///
/// 为何要 newtype：`WarpService` 按值持有 `H: WarpHttp`，而命令层握的是 `Arc<HttpRuntime>`；
/// `impl WarpHttp for Arc<HttpRuntime>` 触孤儿规则（trait 与 `Arc` 皆非本 crate）。故 newtype 绕开。
pub struct HttpWarpAdapter(Arc<HttpRuntime>);

#[async_trait]
impl WarpHttp for HttpWarpAdapter {
    async fn json_request(&self, req: &WarpHttpRequest) -> Result<String, String> {
        self.0.json_request(req).await
    }
    async fn status_request(&self, req: &WarpHttpRequest) -> Result<WarpHttpResponse, String> {
        self.0.status_request(req).await
    }
}

/// 由固定 32 字节种子产出 WARP 的 WG keypair（base64 私钥 = 裸种子，公钥 = X25519(种子, 基点)）。
///
/// 种子在命令层用 CSPRNG 预生成（[`generate_warp_seed`]，可失败 → 结构化 error），本类型的
/// `generate_keypair` 遂是**确定性、不可失败**（X25519 标量乘无失败态），满足 `WarpKeypair` 的无错契约。
/// 对齐 上游 `WarpService.generateKeyPair`：存储私钥为**未裁剪**的裸种子（node PKCS8 末 32 字节同款）。
pub struct SeededWarpKeypair {
    seed: [u8; 32],
}

impl WarpKeypair for SeededWarpKeypair {
    fn generate_keypair(&self) -> (String, String) {
        let public = x25519::x25519_base(&self.seed);
        (base64_encode(&self.seed), base64_encode(&public))
    }
}

/// WARP 日志：转发到 `log` crate（对齐 上游 `LogManager` 的 info/warn/error 落盘最小面）。
pub struct LogWarpLog;

impl WarpLog for LogWarpLog {
    fn log(&self, level: &str, message: &str) {
        match level {
            "error" => log::error!("[WarpService] {message}"),
            "warn" => log::warn!("[WarpService] {message}"),
            _ => log::info!("[WarpService] {message}"),
        }
    }
}

/// 用 CSPRNG 生成 32 字节 WARP 私钥种子。
///
/// **不新增依赖**：走 rustls（本仓直接依赖）暴露的 ring `SecureRandom`（`crypto::ring::default_provider`
/// 的 `secure_random` 字段即 ring `SystemRandom`）。失败（OS 熵源不可用）返结构化 Err，命令层转 error code
/// —— 对齐 node `crypto.generateKeyPairSync` 熵源失败即抛（绝不静默返弱/零密钥）。
///
/// # Errors
/// 系统 CSPRNG 不可用（`GetRandomFailed`）。
pub fn generate_warp_seed() -> Result<[u8; 32], String> {
    let mut seed = [0u8; 32];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut seed)
        .map_err(|_| "系统随机源不可用，无法生成 WARP 密钥".to_string())?;
    Ok(seed)
}

/// 装配一个 WARP 服务（注入真实 HTTP + 给定种子的 keypair + 日志）。
///
/// register 路径传 [`generate_warp_seed`] 出的真种子；applyLicense 路径不触碰 keypair（`WarpService::apply_license`
/// 从不调 `generate_keypair`），故可传占位种子 `[0u8; 32]`（永不被用到）。
#[must_use]
pub fn warp_service(
    http: Arc<HttpRuntime>,
    seed: [u8; 32],
) -> WarpService<HttpWarpAdapter, SeededWarpKeypair, LogWarpLog> {
    WarpService::new(
        HttpWarpAdapter(http),
        SeededWarpKeypair { seed },
        LogWarpLog,
    )
}

/// 标准 base64 编码（带 padding）。WARP 的 32 字节私钥/公钥 → 44 字符 base64。
/// 单一用途 32→44，`base64` crate 已在图中但避免升级为直接依赖（禁引新依赖）→ 最小实现。
/// Tailcat 客户端密钥对（`commands::server::tailcat_keypair`）同为 32→44，复用本函数。
pub(crate) fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 0x3f) as usize] as char);
        out.push(T[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 当前 unix 毫秒（对齐 上游 `Date.now()`）。时钟异常 → 0（不 panic）。
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// deviceId 日志前缀（绝不打全 id/token）。
fn id_prefix(device_id: &str) -> String {
    device_id.chars().take(8).collect()
}

/// drain 计划 + 各 Eligible 条目的注销结果 → 应出队条目集（纯逻辑，便于变异测试）。
///
/// - `Expire`：超龄放弃，出队；
/// - `Eligible` 且 `Done`/`Drop`：注销成功或凭据死，出队；
/// - `Eligible` 且 `Retry`：留队（不入移除集）。
///
/// `eligible_results` 与 plan 中 `Eligible` 条目**按序一一对应**（drain 顺序遍历产出）。缺项兜底 `Retry`（留队，
/// 宁可多试一次不误删）。
fn plan_removals(
    plan: &[DrainPlanItem],
    eligible_results: &[DeregisterResult],
) -> Vec<PendingDeregisterEntry> {
    let mut remove = Vec::new();
    let mut ri = 0usize;
    for item in plan {
        match item.action {
            DrainAction::Expire => remove.push(item.entry.clone()),
            DrainAction::Eligible => {
                let result = eligible_results
                    .get(ri)
                    .copied()
                    .unwrap_or(DeregisterResult::Retry);
                ri += 1;
                if matches!(result, DeregisterResult::Done | DeregisterResult::Drop) {
                    remove.push(item.entry.clone());
                }
            }
        }
    }
    remove
}

/// 从当前队列剔除已解决（出队）条目，保留其余（含 Retry + 网络期间的并发新入队）。精确条目匹配
/// （`PendingDeregisterEntry` 全字段 `Eq`，含 `enqueued_at`）→ 不误删同 device 的另一次入队。
fn retain_unresolved(
    current: Vec<PendingDeregisterEntry>,
    removed: &[PendingDeregisterEntry],
) -> Vec<PendingDeregisterEntry> {
    current
        .into_iter()
        .filter(|e| !removed.contains(e))
        .collect()
}

// ── C5 出口路由生产 OS 手术（三平台 route 装/卸 + macOS utun 反查）──────────────────────────────

/// 出口路由 op 的调用计数（接线单测用；生产侧仅原子自增，成本可忽略）。
#[derive(Default)]
struct ExitRouteOpStats {
    /// `run_route`（装/删路由）被调次数。
    route_calls: AtomicU64,
    /// `find_tailnet_iface`（反查内核接口）被调次数。
    iface_lookups: AtomicU64,
}

/// macOS 反查 TS 内核接口的轮询预算（核连上 tailnet 后 utun 才出现，起核后数秒）。上游 `resolveIface`：12×1.5s≈18s。
const MACOS_RESOLVE_ATTEMPTS: u32 = 12;
const MACOS_RESOLVE_DELAY: Duration = Duration::from_millis(1500);
/// Linux 出口路由独立表 + 规则优先级（绝不碰 main 表 → 不抢 sing-box 主 TUN/子网路由）。Polaris runRoute linux：7732。
const LINUX_EXIT_TABLE: &str = "7732";
const LINUX_EXIT_RULE_PRIORITY: &str = "7732";

/// 生产 [`ExitRouteOp`]：mesh 出口路由真 OS 手术（1:1 移植 上游 `MeshExitRouteManager.runRoute` /
/// `listUtuns` / `probeMacosTailnetIface`）。
///
/// 平台分派（**真机门**：真改宿主路由/查接口）：
/// - **macOS**：`ifconfig` 反查 TS utun（起核后新增 utun 时序 diff + tailnet 100.64/10 地址）→ helper(root)
///   `route add/del -ifscope`。utun 名动态，故轮询等待接口出现（[`MACOS_RESOLVE_ATTEMPTS`]）。
/// - **Linux**：app 自身 CAP_NET_ADMIN → 独立表 [`LINUX_EXIT_TABLE`] + `oif` 规则 `ip route/rule`
///   （**绝不碰 main 表**；helper 协议蓄意无 Linux route 命令 → 不经 helper）。sing-box 只装 tailnet/accept
///   子网路由、**不装 exit_node 的 0/0 出口路由**（真机实证）→ 须本 op 补 0/0（否则绑接口 dialer 拨公网 unreachable）。
/// - **Windows**：`MeshExitRouteManager` 入口已 no-op（禁 System），本 op 不到达该分派。
///
/// **`enabled` 闸门**：`false`（`MeshRuntime::new` 测试/未接线默认，`helper=None`）→ 三方法诚实 no-op
/// （`run_route`→false / `find_tailnet_iface`→None / `list_utuns`→None），**绝不 spawn 任何进程**（本机 Linux
/// 开发/单测安全，杜绝改宿主网络）。`true`（`MeshRuntime::new_with_helper` 生产）→ 真 OS 手术。
struct HelperExitRouteOp {
    /// mac/win route 手术经此 helper（root/SYSTEM `route -ifscope`）。Linux 不用（直接 `ip`）；禁用态 = None。
    helper: Option<Arc<HelperRuntime>>,
    platform: Platform,
    /// 生产接线闸门（见类型注释）。
    enabled: bool,
    stats: Arc<ExitRouteOpStats>,
}

#[async_trait]
impl ExitRouteOp for HelperExitRouteOp {
    async fn run_route(&self, op: &str, iface: &str, cidrs: &[String]) -> bool {
        self.stats.route_calls.fetch_add(1, Ordering::SeqCst);
        if !self.enabled {
            log::debug!("出口路由 OS 操作未接线(禁用闸门)：route-{op} iface={iface} → no-op");
            return false; // 诚实：不假装 OS 路由已装（管理器不标 installed）
        }
        match self.platform {
            // 独立表 7732 的策略规则按地址族生效；部分安装也必须保留清理责任。
            Platform::Linux => {
                let commands_ok = run_linux_route_seq(op, iface, cidrs, |argv| async move {
                    run_ip_command(&argv).await
                })
                .await;
                if op == "del" {
                    // Even a vanished interface can leave an oif policy rule. Query
                    // both independently; missing-device stderr is not evidence.
                    verify_linux_exit_route_absent(iface, cidrs).await
                } else {
                    commands_ok
                }
            }
            // `Other`（未知平台）：与 Android 同答 false，**不 spawn**（2026-09-05 从 Linux 臂里摘出）。
            //
            // 原先它并在 Linux 那条上，写作「Linux/其它类 unix」—— 那是一句没有依据的推定：`Other`
            // 的定义是「本仓没有为这个平台答过题」，其中既不含「它是类 unix」，也不含「它有 iproute2」，
            // 更不含「应用在它上面拿得到 `CAP_NET_ADMIN`」。推定错了的代价与 Android 臂那条逐字相同：
            // 一次注定失败的子进程 + 一条把「本平台不支持」写成「路由装失败」的误导性 warn。
            //
            // 上一层已经关掉了这条腿（`mesh_system_supported_on_platform(Other) == false`，2026-09-05
            // 同批），故本臂与 Android 臂一样是**不可达的纵深防御**。仍要写对，理由也同 Android 臂：
            // 「上层关掉了所以下层写什么都行」正是两扇门之间的那道缝。
            Platform::Other => {
                log::debug!(
                    "出口路由 OS 操作在未知平台上不适用（本仓未为该平台验证过路由手术能力）：route-{op} iface={iface} → no-op"
                );
                false
            }
            // iOS：**不 spawn 任何东西**，直接诚实返 false。独立成臂而不是并进 Android 或 Other：
            // 并进 Android 会断言「同一个理由」，而两者的理由不同 —— Android 是 `/system/bin/ip`
            // **存在**但调用会被拒（注定失败的子进程），iOS 是**连子进程这个概念都没有**
            // （应用与扩展沙箱里不能 fork/exec 任意可执行文件）。并进 `Other` 则更糟：`Other`
            // 的语义是「没答过题」，而 iOS 这一格答过了。
            // 上一层同样已关掉（`mesh_system_supported_on_platform(Ios) == false`），本臂是
            // 不可达的纵深防御 —— 「上层关掉了所以下层写什么都行」正是两扇门之间的那道缝。
            Platform::Ios => {
                log::debug!(
                    "出口路由 OS 操作在 iOS 上不适用（沙箱内无法 exec 路由工具，路由由系统持有）：route-{op} iface={iface} → no-op"
                );
                false
            }
            // Android：**不 spawn `ip`**，直接诚实返 false。
            //
            // 上一层已经把这条腿关掉了（`mesh_system_supported_on_platform(Android) == false` ⇒
            // `MeshExitRouteManager` 的 reconcile/reassert/clear 三个入口全早退），本臂是不可达的
            // 纵深防御。写成独立臂而不是并进 Linux 那条，是因为并进去会**真的去 spawn**：
            // `/system/bin/ip` 在 Android 上存在，非 root 应用调 `ip rule add` 得到的是
            // "Operation not permitted"，即一次注定失败的子进程 + 一条误导性的 warn 日志。
            // 「上层关掉了所以下层写什么都行」正是两扇门之间的那道缝。
            Platform::Android => {
                log::debug!(
                    "出口路由 OS 操作在 Android 上不适用（无 CAP_NET_ADMIN，策略路由由系统持有）：route-{op} iface={iface} → no-op"
                );
                false
            }
            // mac/win：经 root/SYSTEM helper（`route -ifscope`）。res.ok 决定是否标 installed（诚实）。
            Platform::Mac | Platform::Win => match &self.helper {
                Some(h) => h.route_op(op, iface, cidrs),
                None => {
                    log::warn!("出口路由:helper 不可用,无法 route-{op}(System+exit 出网将不通)");
                    false
                }
            },
        }
    }

    async fn list_utuns(&self) -> Option<HashSet<String>> {
        if !self.enabled || self.platform != Platform::Mac {
            return None;
        }
        run_command_stdout("ifconfig", &["-l"])
            .await
            .map(|out| parse_utun_list(&out))
    }

    async fn find_tailnet_iface(
        &self,
        logical_name: &str,
        baseline: Option<&HashSet<String>>,
        cancelled: &(dyn Fn() -> bool + Send + Sync),
    ) -> Option<String> {
        self.stats.iface_lookups.fetch_add(1, Ordering::SeqCst);
        if !self.enabled {
            return None; // 禁用闸门 → 管理器 apply 短路，不装路由
        }
        if self.platform != Platform::Mac {
            // Linux/其它：内核接口固定逻辑名（polaris-ts）。不轮询 ⇒ 天然满足取消契约。
            return Some(logical_name.to_string());
        }
        // macOS：核连上 tailnet 后 TS utun 才出现（起核后数秒）→ 轮询等待。
        // 轮询编排（含取消判据）抽成 [`poll_for_tailnet_iface`]：真 `ifconfig` 是真机门（本机绝不
        // spawn），而「取消后几轮内退出」是可测的纯编排 —— 单测注入假 probe 覆盖。
        poll_for_tailnet_iface(
            MACOS_RESOLVE_ATTEMPTS,
            MACOS_RESOLVE_DELAY,
            cancelled,
            || async {
                let out = run_command_stdout("ifconfig", &[]).await?;
                pick_tailnet_iface(&parse_ifconfig_ifaces(&out), baseline)
            },
        )
        .await
    }
}

/// macOS TS 内核接口反查的**轮询编排**（注入式：`probe` 单次探测、`cancelled` 取消判据）。
///
/// 每一轮的顺序是「查取消 → 探测 → sleep」：取消判据放在**探测之前**，故 `cancel()` 之后最多再
/// 睡一个 `delay` 就退出（收手窗口 ≤ 一个周期）。这正是 [`ExitRouteCancel`] 要求实现方守的契约 ——
/// 不守就是「点停止最长卡 `attempts × delay`（生产 12×1.5s≈18s）」。
///
/// 取消时返回 `None` 与「探测不到」同码：调用方（`polaris_mesh` 的 `apply`）自己再查一次凭据来区分
/// 日志措辞，且两条腿都**不装路由** ⇒ `installed` 保持 `None`，状态自洽。
async fn poll_for_tailnet_iface<F, Fut>(
    attempts: u32,
    delay: Duration,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
    mut probe: F,
) -> Option<String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    for _ in 0..attempts {
        if cancelled() {
            log::debug!("出口路由:TS 接口反查被取消(停核/复位/新起核) → 提前退出轮询");
            return None;
        }
        if let Some(found) = probe().await {
            return Some(found);
        }
        tokio::time::sleep(delay).await;
    }
    None
}

/// `ifconfig -l` 输出 → utun 接口名集合（`^utun\d+$`）。上游 `listUtuns`。纯函数。
fn parse_utun_list(stdout: &str) -> HashSet<String> {
    stdout
        .split_whitespace()
        .filter(|n| is_utun_name(n))
        .map(String::from)
        .collect()
}

/// `utun` + 纯数字后缀（`^utun\d+$`）。
fn is_utun_name(n: &str) -> bool {
    n.strip_prefix("utun")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// `ifconfig`（全量）输出 → 每张 utun 接口的 inet(v4) 地址表（保序）。上游 `probeMacosTailnetIface` 解析段。纯函数。
fn parse_ifconfig_ifaces(stdout: &str) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut active = None;
    for line in stdout.lines() {
        if !line.is_empty() && !line.starts_with(char::is_whitespace) {
            active = utun_header_name(line).map(|name| {
                out.push((name, Vec::new()));
                out.len() - 1
            });
        } else if let Some(index) = active {
            if let Some(ip) = inet_v4_addr(line) {
                out[index].1.push(ip);
            }
        }
    }
    out
}

/// 行首（无缩进）`utunN:` → 接口名；否则 None。明细行有缩进 → 不匹配。
fn utun_header_name(line: &str) -> Option<String> {
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let name = line.split(':').next()?;
    is_utun_name(name).then(|| name.to_string())
}

/// 缩进的 `inet <v4> ...` 明细行 → v4 地址（点分四段数字）；否则 None。仅 IPv4（`inet ` 带空格避开 `inet6`）。
fn inet_v4_addr(line: &str) -> Option<String> {
    let addr = line
        .trim_start()
        .strip_prefix("inet ")?
        .split_whitespace()
        .next()?;
    let ok = addr.split('.').count() == 4
        && addr
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    ok.then(|| addr.to_string())
}

/// tailnet 地址判定（100.64.0.0/10 → 100.64.x - 100.127.x）。上游 `isTailnet`。纯函数。
fn is_tailnet_addr(ip: &str) -> bool {
    let mut it = ip.split('.');
    if it.next() != Some("100") {
        return false;
    }
    it.next()
        .and_then(|s| s.parse::<u32>().ok())
        .is_some_and(|n| (64..=127).contains(&n))
}

/// 从 ifconfig 解析结果挑 TS 内核接口：优先「起核后新增（不在 baseline）且带 tailnet 地址」的 utun；
/// 兜底全量 utun 里带 tailnet 地址的（无 baseline / baseline 偏差）。上游 `probeMacosTailnetIface` 决策。纯函数。
fn pick_tailnet_iface(
    ifaces: &[(String, Vec<String>)],
    baseline: Option<&HashSet<String>>,
) -> Option<String> {
    let has_tailnet = |ips: &[String]| ips.iter().any(|ip| is_tailnet_addr(ip));
    // 候选：起核后新增（不在 baseline）；无 baseline → 全部。
    if let Some((name, _)) = ifaces
        .iter()
        .filter(|(n, _)| baseline.is_none_or(|b| !b.contains(n)))
        .find(|(_, ips)| has_tailnet(ips))
    {
        return Some(name.clone());
    }
    // 兜底：全量 utun 里找带 tailnet 地址的。
    ifaces
        .iter()
        .find(|(_, ips)| has_tailnet(ips))
        .map(|(n, _)| n.clone())
}

/// 每个地址族先装策略规则，再装表内路由；删除时先删路由，最后删该族规则。
/// 当后续操作失败时，已安装的规则仍需要 clear。删除后的实际状态由库存查询确认。
async fn run_linux_route_seq<F, Fut>(op: &str, iface: &str, cidrs: &[String], run: F) -> bool
where
    F: Fn(Vec<String>) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let adding = op == "add";
    let mut installed = false;
    let mut all_ok = true;
    for family in ["-4", "-6"] {
        let rule = || {
            [
                family,
                "rule",
                op,
                "oif",
                iface,
                "table",
                LINUX_EXIT_TABLE,
                "priority",
                LINUX_EXIT_RULE_PRIORITY,
            ]
            .map(str::to_owned)
            .into()
        };
        let verb = if adding { "replace" } else { "del" };
        let mut has_routes = false;
        for cidr in cidrs {
            if cidr.contains(':') != (family == "-6") {
                continue;
            }
            if adding && !has_routes {
                if !run(rule()).await {
                    log::warn!("出口路由: Linux ip {family} rule add 失败，跳过该族路由");
                    return installed;
                }
                installed = true;
            }
            has_routes = true;
            let args = [
                family,
                "route",
                verb,
                cidr.as_str(),
                "dev",
                iface,
                "table",
                LINUX_EXIT_TABLE,
            ];
            all_ok &= run(args.map(str::to_owned).into()).await;
        }
        if !adding && has_routes {
            all_ok &= run(rule()).await;
        }
    }
    if adding {
        installed
    } else {
        all_ok
    }
}

// Parse only successful, structured inventories. Unknown shapes remain cleanup debt.
fn linux_exit_route_inventory_absent(
    routes: &serde_json::Value,
    rules: &serde_json::Value,
    iface: &str,
    cidrs: &[String],
) -> bool {
    let (Some(routes), Some(rules)) = (routes.as_array(), rules.as_array()) else {
        return false;
    };
    let numeric_table = |table: &serde_json::Value| {
        table
            .as_u64()
            .or_else(|| {
                table.as_str().and_then(|value| {
                    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                        None
                    } else {
                        value.parse::<u64>().ok()
                    }
                })
            })
            .filter(|table| *table <= u64::from(u32::MAX))
    };
    for route in routes {
        let Some(object) = route.as_object() else {
            return false;
        };
        let destination = object.get("dst").and_then(serde_json::Value::as_str);
        let relevant = object.get("dev").and_then(serde_json::Value::as_str) == Some(iface)
            || destination.is_some_and(|destination| {
                cidrs.iter().any(|cidr| {
                    cidr == destination
                        || (destination == "default" && (cidr == "0.0.0.0/0" || cidr == "::/0"))
                })
            });
        match object.get("table").and_then(&numeric_table) {
            Some(7732) => {
                if destination.is_none() || relevant {
                    return false;
                }
            }
            Some(_) => {} // A known different table cannot contain our route.
            None if relevant => return false,
            None => {} // Unrelated inventory does not establish or disprove our ownership.
        }
    }
    for rule in rules {
        let Some(object) = rule.as_object() else {
            return false;
        };
        if object.get("oif").and_then(serde_json::Value::as_str) != Some(iface) {
            continue;
        }
        match object.get("table").and_then(&numeric_table) {
            Some(7732) | None => return false,
            Some(_) => {} // Only a verified different numeric table may be ignored.
        }
    }
    true
}

// Keep all-table inventory: an absent dedicated table may make a targeted dump
// fail rather than return []. Numeric details distinguish relevant table identities.
fn linux_exit_route_inventory_args(family: &str) -> [Vec<&str>; 2] {
    [
        vec![
            family, "-j", "-N", "-details", "route", "show", "table", "all",
        ],
        vec![family, "-j", "-N", "-details", "rule", "show"],
    ]
}

// A failed, oversized or timed-out query is unknown, never proof of cleanup.
async fn read_linux_exit_route_inventory(
    mut command: tokio::process::Command,
) -> Option<serde_json::Value> {
    use std::process::Stdio;
    use tokio::io::AsyncReadExt;

    const MAX_BYTES: u64 = 4 * 1024 * 1024;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let query = async {
        let mut bytes = Vec::new();
        stdout
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() as u64 > MAX_BYTES || !child.wait().await.ok()?.success() {
            return None;
        }
        serde_json::from_slice(&bytes).ok()
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), query).await;
    // kill_on_drop also covers cancellation by the caller. On normal failure,
    // kill and reap explicitly; do not leave a producer blocked on a full pipe.
    if !matches!(result, Ok(Some(_))) {
        let _ = child.kill().await;
    }
    result.ok().flatten()
}

async fn verify_linux_exit_route_absent(iface: &str, cidrs: &[String]) -> bool {
    for family in ["-4", "-6"] {
        let mut inventories = Vec::new();
        for args in linux_exit_route_inventory_args(family) {
            let mut command = tokio::process::Command::new("ip");
            command.env("LC_ALL", "C").args(args);
            let Some(value) = read_linux_exit_route_inventory(command).await else {
                return false;
            };
            inventories.push(value);
        }
        if !linux_exit_route_inventory_absent(&inventories[0], &inventories[1], iface, cidrs) {
            return false;
        }
    }
    true
}

// Only the exact kernel absence result for a deletion is idempotent success.
// Permission errors, missing executables/devices and unknown failures retain debt.
fn ip_delete_already_absent(argv: &[String], stderr: &[u8]) -> bool {
    argv.iter().any(|arg| arg == "del")
        && std::str::from_utf8(stderr)
            .is_ok_and(|text| text.trim() == "RTNETLINK answers: No such file or directory")
}

/// 运行 `ip <argv>`，**返回是否真的成功**（退出码 0）。Linux 出口路由手术（app CAP_NET_ADMIN）。**真机门**。
///
/// 仍不抛（错误只记日志），但**不再把失败伪装成成功**：调用方据返回值决定是否标 `installed`
/// （见 [`HelperExitRouteOp::run_route`] 的 Linux 分支）。`ip` 不在 PATH（`Err`）与非零退出
/// （无 CAP_NET_ADMIN / 语法错 / 内核不支持）都返 `false`。
async fn run_ip_command(argv: &[String]) -> bool {
    match tokio::process::Command::new("ip")
        .env("LC_ALL", "C")
        .args(argv)
        .output()
        .await
    {
        Ok(o) if o.status.success() => true,
        Ok(o) if ip_delete_already_absent(argv, &o.stderr) => true,
        Ok(o) => {
            log::debug!(
                "出口路由 ip {} → 非零退出({:?})",
                argv.join(" "),
                o.status.code()
            );
            false
        }
        Err(e) => {
            log::debug!("出口路由 ip {} 启动失败: {e}", argv.join(" "));
            false
        }
    }
}

/// 运行命令取 stdout（失败 → None）。macOS `ifconfig` 反查用。**真机门**。
async fn run_command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let out = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 出口路由日志：转发到 `log` crate（对齐 [`LogWarpLog`] 的 info/warn/error 最小面）。
struct LogExitRouteLog;
impl ExitRouteLog for LogExitRouteLog {
    fn log(&self, level: &str, message: &str) {
        match level {
            "error" => log::error!("[MeshExitRoute] {message}"),
            "warn" => log::warn!("[MeshExitRoute] {message}"),
            _ => log::info!("[MeshExitRoute] {message}"),
        }
    }
}

// 本机运行平台 → `Platform`：直接用 [`Platform::current`]，本模块**不再自留一份**。
//
// 原先这里有一个 `current_platform()`，逐字重复了 `Platform::current()` 的三分 `cfg!` 链 + `else
// { Other }`。它是一个**影子产地**：给 `Platform` 加变体时，穷举 `match` 会在全仓报缺臂，而这个
// `else` 分支照样编译通过、把新平台静默吞进 `Other` —— 加变体这件事想守的东西，正好被它绕开。
// 2026-09-04 K10 删除。删掉不改任何行为：两份实现在 mac/win/linux 上逐值相同，且
// `Platform::current()` 现在多认一个 `target_os = "android"`（那正是要修的那条）。

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "mesh/audit_tests.rs"]
mod audit_tests;
