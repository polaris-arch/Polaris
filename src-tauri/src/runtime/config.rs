//! 配置运行时：`polaris-store` + `polaris-config-engine` 的运行时装配。
//!
//! Polaris 锚点：`main/services/ConfigManager.ts`。
//! - `loadConfig` → [`ConfigManager::load_full`]（read → sanitize → migrate → validate → 填默认 + currentConfig 缓存）
//! - `saveConfig` → `ConfigManager::save_full`（再跑 sanitize+validate + 原子 tmp→rename 写盘 + 刷缓存）
//! - `get(key)` / `set(key, value)` → currentConfig 投影取值 / 原地改 + 异步落盘
//!
//! 纯逻辑纪律：`store::ConfigStore` / `sanitize` / `validate` / `migrate` 全在 domain crate，
//! 本层仅注入 [`StdFs`]（std::fs，0o600）+ 持有 currentConfig 缓存（Polaris ConfigManager.currentConfig）。

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use polaris_config_engine::user_config::mesh_identity_reconcile::{
    canonical_control_authority, reconcile_controlled_identity, ControlledIdentityChange,
    ReplacementIdentity, RetirementScopeSnapshot,
};
use polaris_config_engine::user_config::mesh_route_state::{
    revise_semantic, MeshOwnerRef, MeshRoutePolicy, MeshRouteState, MeshTransactionPhase,
};
use polaris_core_supervisor::{LifecycleGate, LifecycleKind, LiveClaimGuard};
use polaris_store::fs::{durable_atomic_write, durable_remove, random_tmp_suffix, ConfigFs, StdFs};
use polaris_store::mesh_guard::{self, REQUIRED_MARKER_FILE};
use polaris_store::{ConfigStore, LoadResult, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::commands::config::config_version;
use crate::runtime::proxy::mesh_apply::{self, ApplyClaim, ApplyError, ApplyStep, PhaseEvent};

const DEFERRED_DELETIONS_FILE: &str = "pending-config-deletions.json";
const DEFERRED_DELETIONS_VERSION: u8 = 1;
const STAGED_PENDING_FILE: &str = "staged-config.pending";
const STAGED_PENDING_VERSION: u8 = 1;

/// A legacy operation owns admission until its last stop/start/swap effect has
/// completed. The counter is incremented under ConfigManager's write lock;
/// managed opt-in checks the same counter under that lock before publishing its
/// first marker. This guard owns only an Arc, so it can cross async awaits.
#[must_use]
pub(crate) struct LegacyStartLease {
    active: Arc<AtomicUsize>,
}

impl Drop for LegacyStartLease {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum MeshMarkerPhase {
    Preparing,
    Enabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MeshRequiredMarker {
    phase: MeshMarkerPhase,
    local_id: String,
    legacy_config_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeshPrepareRecovery {
    AwaitingCommit,
    Enabled,
}

/// An owned, strict disk snapshot for managed Apply preflight. The raw source
/// includes credentials, so this type deliberately implements neither Debug
/// nor Serialize. Its private digest binds later admission to the entire raw
/// JSON value read while ConfigManager held its write lock. configVersion is
/// a frontend compatibility token, not a strong content identity.
#[allow(dead_code)] // Production Apply wiring follows this admission slice.
pub(crate) struct ApplyInputSnapshot {
    raw: Value,
    policy: MeshRoutePolicy,
    state: MeshRouteState,
    config_version: String,
    raw_document_sha256: String,
    marker: MeshRequiredMarker,
}

#[allow(dead_code)]
impl ApplyInputSnapshot {
    pub(crate) fn raw(&self) -> &Value {
        &self.raw
    }

    pub(crate) fn policy(&self) -> &MeshRoutePolicy {
        &self.policy
    }

    pub(crate) fn state(&self) -> &MeshRouteState {
        &self.state
    }

    pub(crate) fn config_version(&self) -> &str {
        &self.config_version
    }

    pub(crate) fn raw_document_sha256(&self) -> &str {
        &self.raw_document_sha256
    }
}

/// Content version and ledger revision are checked from the same raw document
/// under ConfigManager's write lock. Prepare additionally requires its opaque
/// `ApplyInputSnapshot` and strong raw-document digest. `StopIntent`
/// deliberately ignores content version so a user Stop can supersede an edit.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ApplyCasExpected<'a> {
    pub config_version: &'a str,
    pub state_revision: &'a str,
}

#[derive(Debug)]
#[allow(dead_code)] // Caller wiring follows in S4; preserve StoreError for commit uncertainty.
pub(crate) enum ApplyPersistError {
    Store(StoreError),
    ConfigChanged,
    Step(ApplyError),
    /// Stop reservation did not return success after a lifecycle claim may
    /// have retired the old monitor. Keep old owner resources and treat their
    /// supervision status as unknown until independently proved, even for an
    /// inner Step(Invalid). The inner cause separately describes disk truth:
    /// a pre-rename Io error did not commit, while CommitUncertain requires a
    /// strict reread before another action.
    StopReservationUncertain(Box<ApplyPersistError>),
}

/// An opaque, non-cloneable reservation receipt. Only this manager's successful
/// reserved Stop CAS can mint one; journal state read back from disk cannot.
/// It binds the issuing manager and the written state's local identity, claim,
/// revision, and exact target. Borrowed checks alone do not make it single-use;
/// the manager consumes it after the final current-reservation callback.
/// A future StopLease constructor must still prove the exact core and TS
/// owner release with a separate, non-recoverable admitted token.
#[derive(Debug)]
pub(crate) struct StopReservationReceipt {
    issuer: Arc<()>,
    domain: Arc<StopRuntimeDomain>,
    local_id: String,
    claim: ApplyClaim,
    state_revision: String,
    stop_target_run_ref: String,
}

impl StopReservationReceipt {
    pub(crate) fn belongs_to(&self, domain: &Arc<StopRuntimeDomain>) -> bool {
        Arc::ptr_eq(&self.domain, domain)
    }

    pub(crate) fn local_id(&self) -> &str {
        &self.local_id
    }

    pub(crate) fn claim(&self) -> &ApplyClaim {
        &self.claim
    }

    pub(crate) fn state_revision(&self) -> &str {
        &self.state_revision
    }

    pub(crate) fn stop_target_run_ref(&self) -> &str {
        &self.stop_target_run_ref
    }

    pub(crate) fn issued_by(&self, manager: &ConfigManager) -> bool {
        Arc::ptr_eq(&self.issuer, &manager.stop_receipt_issuer)
    }
}

/// One process lifetime's stop authority. A fresh boot identity and the
/// actual lifecycle gate are fixed together by ProxyRuntime construction.
/// Neither a disk journal nor a caller-supplied boot string can recreate it.
#[derive(Debug)]
pub(crate) struct StopRuntimeDomain {
    boot_id: String,
    gate: Arc<LifecycleGate>,
}

impl StopRuntimeDomain {
    pub(crate) fn new(gate: Arc<LifecycleGate>) -> Arc<Self> {
        Arc::new(Self {
            boot_id: uuid::Uuid::new_v4().to_string(),
            gate,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_boot_for_test(gate: Arc<LifecycleGate>, boot_id: &str) -> Arc<Self> {
        Arc::new(Self {
            boot_id: boot_id.into(),
            gate,
        })
    }

    pub(crate) fn boot_id(&self) -> &str {
        &self.boot_id
    }

    pub(crate) fn gate(&self) -> &LifecycleGate {
        &self.gate
    }
}

/// A current reservation can be inspected only while its manager still owns
/// the config write lock. Busy returns the same receipt for a later retry;
/// rejection drops it without invoking the custody callback. This checks the
/// bound tuple and localId, not a byte-level raw document digest.
#[allow(dead_code)] // Exact direct Child custody has no production caller yet.
pub(crate) enum StopReservationCheck<T> {
    Busy(Box<StopReservationReceipt>),
    Rejected,
    Current(T),
}

impl From<StoreError> for ApplyPersistError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// S4 must mint this only while it holds lifecycle + TS state gates and has
/// proved that neither the main nor a temporary owner uses this server's state.
/// Those guards must stay held through the config write, with their live
/// generation rechecked immediately before publish; this must never survive
/// an await. S4 must advance the atomic for every owner transition. The
/// recheck is a lock-free load under config write_lock, never an attempt to
/// reacquire lifecycle/state gates (lock order is lifecycle -> state -> config).
/// There is intentionally no production constructor before S4 enforces this.
#[allow(dead_code)]
pub(crate) struct MeshNoOwnerReceipt<'a> {
    owner_ref: MeshOwnerRef,
    local_id: String,
    state_revision: String,
    /// S4 binds this to the still-held lifecycle/state guards' live generation.
    live_generation: &'a AtomicU64,
    expected_generation: u64,
}

#[allow(dead_code)]
pub(crate) enum MeshServerIdentityEdit {
    Replace(Value),
    Delete,
}

fn ts_control_authority(server: &Value) -> Result<String, StoreError> {
    let settings = server.get("tailscaleSettings");
    if settings.is_some_and(|value| !value.is_object()) {
        return Err(StoreError::validation("TS settings are unavailable"));
    }
    let url = match settings.and_then(|value| value.get("controlUrl")) {
        None | Some(Value::Null) => "https://controlplane.tailscale.com",
        Some(Value::String(url)) if url.is_empty() => "https://controlplane.tailscale.com",
        Some(Value::String(url)) => url,
        Some(_) => return Err(StoreError::validation("invalid TS control authority")),
    };
    canonical_control_authority(url).map_err(StoreError::validation)
}

/// 未保存草稿对运行态节点选择的最小投影。正文仍只在渲染端；这里不复制配置，只携带自动故障切换
/// 必须知道的节点 id。`scope_known=false` 只会来自升级前的空 marker：此时调用方必须保守地把整个
/// 节点域视为未知，不能猜一份草稿没有改节点。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StagedNodeMask {
    pub pending: bool,
    pub node_ids: BTreeSet<String>,
    pub scope_known: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StagedPendingMarker {
    version: u8,
    node_ids: BTreeSet<String>,
}

/// [`ConfigManager::with_current`] 闭包内的**重入探针**（debug 构型，release 完全编译掉）。
///
/// # 为什么要有牙，而不是只写一行注释
///
/// `with_current` 的闭包跑在 `cache` 的**读锁**里，闭包内再碰 `ConfigManager` 就是死锁面（细节见该
/// 方法文档）。而这个坑的失效形态**极不友好**：
/// - 取写锁那条（`save_full` / `set_value`）是**必然**自死锁 —— 一写就挂，尚算显形；
/// - 取读锁那条（`current` / 嵌套 `with_current`）平时**看起来是好的** —— std 的 `RwLock` 在无写者
///   排队时递归读通常拿得到，只有「恰好有另一条腿在写配置」的那一瞬才永久阻塞。也就是说它能过
///   全部单测、过 code review、过真机冒烟，然后在用户改配置的那一刻挂死。
///
/// 靠文档防这种坑等于没防。故 debug 构型下用一个 thread-local 深度计数把「在闭包里又回来读/写配置」
/// **就地打成 panic**：坏用法在写出来的当天、在单测里就炸，而不是在生产里挂死。
/// release 构型下 [`ReentrancyProbe`] 是零字段 ZST、`enter`/`Drop` 皆空 —— 无 TLS 访问、无分支。
#[cfg(debug_assertions)]
mod reentrancy {
    use std::cell::Cell;

    thread_local! {
        /// 当前线程正处在几层 `with_current` 闭包里（>0 = 读锁在手，禁止再碰 ConfigManager）。
        static DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    /// 进入闭包时 +1、离开（含 panic 展开）时 -1。用 `Drop` 而非手工配对：闭包 panic 时也必须归零，
    /// 否则一个失败测试会把同线程后续所有配置读全打成 panic，故障从一条变成一片。
    pub(super) struct ReentrancyProbe;

    impl ReentrancyProbe {
        pub(super) fn enter() -> Self {
            DEPTH.with(|d| d.set(d.get() + 1));
            Self
        }
    }

    impl Drop for ReentrancyProbe {
        fn drop(&mut self) {
            DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }

    /// `ConfigManager` 每个入口开头调一次：若正在 `with_current` 闭包里 → 立刻 panic。
    pub(super) fn deny_inside_projection(entry: &str) {
        assert!(
            DEPTH.with(Cell::get) == 0,
            "ConfigManager::{entry} 在 with_current 闭包内被调用 —— 读锁正持在手上，\
             取写锁必然自死锁、递归读会在有写者排队时永久阻塞。闭包内只做纯投影，\
             把第二次配置读平铺到闭包外面。"
        );
    }
}

#[cfg(not(debug_assertions))]
mod reentrancy {
    pub(super) struct ReentrancyProbe;
    impl ReentrancyProbe {
        #[inline(always)]
        pub(super) fn enter() -> Self {
            Self
        }
    }
    #[inline(always)]
    pub(super) fn deny_inside_projection(_entry: &str) {}
}

use reentrancy::{deny_inside_projection, ReentrancyProbe};

/// [`ConfigManager::update`] 闭包的裁决：写不写盘，以及调用方要 return 的那个值。
///
/// 两个变体**都带 `R`**：「不写」在真实站点上通常是**以另一种方式成功了**
/// （净零序、无命中、内容等价），不是失败 —— 失败走 `Err(StoreError)` 或调用方自己塞进 `R`。
#[derive(Debug)]
pub enum Decision<R> {
    /// 落盘，然后把 `R` 与已落盘的配置一起还给调用方。
    Write(R),
    /// **不落盘、不广播**，直接把 `R` 还给调用方（闭包对 cfg 的改动一律丢弃）。
    Skip(R),
}

/// 「配置已保存、运行态尚未 Apply」期间必须保留的不可逆删除意图。
///
/// 这不是第二份配置：条目只携带执行副作用所需的最小身份。执行前仍会对照最新 config 复核；若实体
/// 已被重新加入，则丢弃意图而不删除。WARP token 只落 0o600 的运行时状态文件，不进入前端快照/备份。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum DeferredConfigDeletion {
    RuleResource {
        file_name: String,
    },
    BuiltinRuleResource {
        tag: String,
        file_name: String,
    },
    AppIcon {
        app_id: String,
    },
    TailscaleState {
        server_id: String,
    },
    WarpDevice {
        server_id: String,
        device_id: String,
        token: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeferredDeletionJournal {
    version: u8,
    entries: Vec<DeferredConfigDeletion>,
}

impl Default for DeferredDeletionJournal {
    fn default() -> Self {
        Self {
            version: DEFERRED_DELETIONS_VERSION,
            entries: Vec::new(),
        }
    }
}

/// 延迟删除消费结果。只暴露计数，绝不把可能含 WARP token 的条目带进日志或 IPC。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DeferredDeletionSummary {
    pub applied: usize,
    pub cancelled: usize,
    pub retrying: usize,
}

/// 配置运行时（`State`-managed，单实例）。
///
/// 持有配置目录路径 + currentConfig 缓存（`RwLock<Value>`，读多写少）。
/// FS 经 [`StdFs`]（std::fs 实现，写入 0o600）——纯逻辑 crate 的 trait 注入点。
pub struct ConfigManager {
    /// 配置目录（`<app_config_dir>/polaris/`）。
    dir: PathBuf,
    /// config.json 绝对路径。
    path: PathBuf,
    /// currentConfig 缓存（Polaris ConfigManager.currentConfig；首次 load 填充）。
    cache: RwLock<Option<Value>>,
    /// **配置文件事务锁** —— 读取时可能发生的首装/迁移落盘、普通保存、原子读改写与删除日志消费
    /// 全部按这把锁串行。这样不只避免两个 writer 丢更新，也避免 `load_full` 的迁移落盘从旁路覆盖
    /// 一次已完成的更新。
    ///
    /// **与 [`Self::cache`] 是两把互不相干的锁**，这一点是本设计成立的前提：临界区内会调
    /// `load_full_under_write_lock`（末尾取 `cache` 写锁）与保存腿（先取读锁拿旧 icon id、末尾取写锁刷缓存），
    /// 若本锁与 `cache` 是同一把，那两次调用就是自死锁。
    write_lock: Mutex<()>,
    stop_receipt_issuer: Arc<()>,
    legacy_start_leases: Arc<AtomicUsize>,
    /// 配置保存与延迟删除消费的事务锁。所有 save 都经它串行，关掉「journal 已写、config 未写时被
    /// Apply 提前消费」及「消费复核后实体又被并发加入」两类竞态；与 `write_lock` 分离以免锁层反转。
    deferred_delete_lock: Mutex<()>,
    /// 渲染端是否仍有未落盘草稿。内存位守本进程的托盘/启动入口，旁边的空标记文件守崩溃重启。
    /// 正常退出留下 `clean-exit.marker` 时，新进程会忽略并清掉本标记；app:restart / 崩溃则保留。
    /// 草稿存在位、节点遮罩与范围判据必须是同一份原子快照。拆成多个 atomic/lock 会允许自动
    /// 故障切换读到 `pending=true` 搭配旧遮罩，从而穿透刚建立的未保存节点边界。
    staged_mask: RwLock<StagedNodeMask>,
    /// Test-only post-publication failure: the file is already replaced, but
    /// the caller receives CommitUncertain and must withhold action rights.
    #[cfg(test)]
    test_uncertain_after_mesh_publish_once: AtomicBool,
}

impl ConfigManager {
    #[cfg(test)]
    pub(crate) fn hold_write_lock_for_test(&self) -> std::sync::MutexGuard<'_, ()> {
        self.write_lock.lock().unwrap()
    }

    /// 新建（dir = `<app_config_dir>/polaris/`）。不立即读盘——lazy load（首次命令触发）。
    pub fn new(dir: PathBuf) -> Self {
        let path = dir.join("config.json");
        let staged_pending_path = dir.join(STAGED_PENDING_FILE);
        let clean_exit = StdFs.exists(&dir.join(crate::clean_exit::CLEAN_EXIT_MARKER_FILENAME));
        let staged_pending = !clean_exit && StdFs.exists(&staged_pending_path);
        let staged_marker = staged_pending
            .then(|| StdFs.read_to_string(&staged_pending_path).ok())
            .flatten()
            .and_then(|raw| serde_json::from_str::<StagedPendingMarker>(&raw).ok())
            .filter(|marker| marker.version == STAGED_PENDING_VERSION);
        if clean_exit {
            // 正常退出的渲染端草稿按既有 Q1-b 契约丢弃。这里同步清后端镜像，避免 2s 自动连接
            // 早于主窗 hydrate 时把一份已被判作废的旧草稿误当阻塞条件。
            if let Err(error) = StdFs.remove(&staged_pending_path) {
                log::warn!("清理正常退出遗留的草稿标记失败: {error}");
            }
        }
        Self {
            dir,
            path,
            cache: RwLock::new(None),
            write_lock: Mutex::new(()),
            stop_receipt_issuer: Arc::new(()),
            legacy_start_leases: Arc::new(AtomicUsize::new(0)),
            deferred_delete_lock: Mutex::new(()),
            staged_mask: RwLock::new(StagedNodeMask {
                pending: staged_pending,
                node_ids: staged_marker
                    .as_ref()
                    .map_or_else(BTreeSet::new, |marker| marker.node_ids.clone()),
                scope_known: staged_marker.is_some(),
            }),
            #[cfg(test)]
            test_uncertain_after_mesh_publish_once: AtomicBool::new(false),
        }
    }

    /// 配置目录（供其他运行时复用，如 mesh state / helper token）。
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// config.json 路径。
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Legacy start admission reads the raw disk document under the same lock as
    /// marker publication and config writes. A present marker (including a bad
    /// or preparing marker), managed fields without a marker, or unreadable raw
    /// input cannot be interpreted as permission to use the old start path.
    /// This is only a legacy fence; managed starts require the shared Apply
    /// coordinator's persisted claim and are unsupported here.
    pub(crate) fn admit_legacy_start(&self) -> Result<(), StoreError> {
        deny_inside_projection("admit_legacy_start");
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.admit_legacy_start_under_write_lock()
    }

    pub(crate) fn lease_legacy_start(&self) -> Result<LegacyStartLease, StoreError> {
        deny_inside_projection("lease_legacy_start");
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.admit_legacy_start_under_write_lock()?;
        self.legacy_start_leases.fetch_add(1, Ordering::SeqCst);
        Ok(LegacyStartLease {
            active: Arc::clone(&self.legacy_start_leases),
        })
    }

    /// Keep an already-admitted legacy operation fenced while its blocking
    /// helper IPC runs after the awaiting future is cancelled. A standalone
    /// Stop has no legacy lease, so it remains usable in managed mode.
    pub(crate) fn retain_active_legacy_start_lease(&self) -> Option<LegacyStartLease> {
        deny_inside_projection("retain_active_legacy_start_lease");
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.legacy_start_leases.load(Ordering::SeqCst) == 0 {
            return None;
        }
        self.legacy_start_leases.fetch_add(1, Ordering::SeqCst);
        Some(LegacyStartLease {
            active: Arc::clone(&self.legacy_start_leases),
        })
    }

    fn admit_legacy_start_under_write_lock(&self) -> Result<(), StoreError> {
        match std::fs::symlink_metadata(self.mesh_marker_path()) {
            Ok(_) => {
                return Err(StoreError::validation(
                    "managed mesh route requires a coordinated start (unsupported)",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StoreError::Io(error.to_string())),
        }
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(StoreError::Io(error.to_string())),
        };
        if !metadata.file_type().is_file() {
            return Err(StoreError::validation(
                "legacy start requires a regular config document",
            ));
        }
        let raw = std::fs::read(&self.path).map_err(|error| StoreError::Io(error.to_string()))?;
        let document: Value = serde_json::from_slice(&raw).map_err(StoreError::from_parse)?;
        if !document.is_object() || mesh_guard::has_managed_fields(&document) {
            return Err(StoreError::validation(
                "legacy start requires an intact unmanaged config document",
            ));
        }
        Ok(())
    }

    /// 渲染端未保存草稿的跨入口镜像。只表达“有/无”，草稿正文仍唯一保存在主窗 `localStorage`。
    #[must_use]
    pub fn has_staged_pending(&self) -> bool {
        self.staged_mask
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
    }

    /// 自动故障切换消费的草稿节点遮罩快照。`pending=false` 时其余字段没有语义；升级前空 marker
    /// 会得到 `pending=true, scope_known=false`，调用方据此 fail-closed。
    #[must_use]
    pub fn staged_node_mask(&self) -> StagedNodeMask {
        self.staged_mask
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 更新未保存草稿标记。内存位先更新，确保当前进程里的托盘入口立即看到；持久文件 best-effort，
    /// 失败不会反过来吃掉渲染端草稿，只会让崩溃后的自动连接少一道后端保险（前端恢复后会再次同步）。
    #[cfg(test)]
    pub fn set_staged_pending(&self, pending: bool) {
        self.set_staged_pending_snapshot(pending, None);
    }

    /// 更新草稿 marker 及其节点遮罩。`node_ids=None` 是旧调用方/旧 marker 的未知范围；新渲染端即使
    /// 草稿只改了非节点配置也会显式传 `Some(empty)`，这样故障切换仍可在运行态 clean 节点间工作。
    pub fn set_staged_pending_snapshot(&self, pending: bool, node_ids: Option<Vec<String>>) {
        let scope_known = node_ids.is_some();
        let node_ids: BTreeSet<String> = node_ids.unwrap_or_default().into_iter().collect();
        {
            let mut guard = self
                .staged_mask
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard = StagedNodeMask {
                pending,
                node_ids: node_ids.clone(),
                scope_known,
            };
        }
        let path = self.dir.join(STAGED_PENDING_FILE);
        let result = if pending {
            let marker = StagedPendingMarker {
                version: STAGED_PENDING_VERSION,
                node_ids,
            };
            serde_json::to_string(&marker)
                .map_err(StoreError::from)
                .and_then(|content| {
                    StdFs
                        .create_dir_all(&self.dir)
                        .and_then(|()| StdFs.write(&path, &content))
                })
        } else {
            StdFs.remove(&path)
        };
        if let Err(error) = result {
            log::warn!("同步未保存草稿标记失败（pending={pending}）: {error}");
        }
    }

    /// 加载配置（read → sanitize → migrate → validate + 填默认），刷新 currentConfig 缓存。
    ///
    /// 维度7 #7：坏 JSON/坏字段绝不崩溃，回落默认配置；损坏的磁盘真实文件绝不覆盖。仅新装默认值
    /// 与迁移链已确认的改写会在本层 best-effort 落盘，保证带标记迁移真正一次完成。
    pub fn load_full(&self) -> Result<Value, StoreError> {
        deny_inside_projection("load_full");
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.load_full_under_write_lock()
    }

    /// 调用方已持有 [`Self::write_lock`] 的加载腿。加载本身可能因首装或迁移而写盘，所以不能当成
    /// 普通只读操作从事务锁旁路执行。
    fn load_full_under_write_lock(&self) -> Result<Value, StoreError> {
        let marker = self.mesh_required_marker()?;
        if marker
            .as_ref()
            .is_some_and(|marker| marker.phase == MeshMarkerPhase::Preparing)
        {
            return Err(StoreError::validation(
                "mesh route migration is preparing; explicit recovery is required",
            ));
        }
        let LoadResult {
            mut config,
            loaded_from_disk,
            migration_delta,
            was_missing,
            error,
            protected_error,
        } = ConfigStore::load(&StdFs, &self.path);
        if let Some(error) = protected_error {
            return Err(error);
        }
        match marker.as_ref() {
            Some(marker) => {
                if was_missing || error.is_some() || !mesh_guard::validate_raw(&config)? {
                    return Err(StoreError::validation(
                        "mesh route marker exists but the managed config is unavailable",
                    ));
                }
                if config[mesh_guard::STATE_KEY]["localId"] != marker.local_id {
                    return Err(StoreError::validation(
                        "mesh route marker localId does not match ledger",
                    ));
                }
            }
            None if mesh_guard::has_managed_fields(&config) => {
                return Err(StoreError::validation(
                    "managed mesh route config has no required marker; recovery is required",
                ));
            }
            None => {}
        }
        // 加载或校验失败 → 回落默认（LoadResult 已处理），但记日志保留 error 上下文。
        if let Some(e) = &error {
            log::warn!("config load fallback (loaded_from_disk={loaded_from_disk}): {e}");
        }
        // 新装（文件本不存在）→ 落盘一次默认配置；迁移有改写 → 同步落盘迁移值与幂等标记。
        // 若只把迁移后的 Value 放进 cache、忽略 migration_delta，重启后还会从旧磁盘形态重复迁移；
        // 更糟的是「用户关闭预热」这类一次性默认纠偏无法证明已经完成。损坏配置的 fallback 同时满足
        // was_missing=false + migration_delta.changed=false，仍保持“不覆盖损坏原件”的安全边界。
        //
        // 第 4 参是**原子写的 12hex tmp 后缀**（`randomBytes(6).toString('hex')` 等价），
        // 不是品牌名/应用名。此处曾误传字面量 `"polaris"` → debug 撞 `tmp_path` 的
        // `debug_assert` **首启即崩**（本行正是 P0 的触发点：config.json 不存在才走到）；
        // release 下则静默产出永不被清扫的 `config.json.polaris.tmp`。
        if was_missing || migration_delta.changed {
            let previous = self.raw_disk_for_mesh_under_write_lock()?;
            mesh_guard::reconcile_untrusted(&previous, &mut config)?;
            let persist = self.persist_canonical_under_write_lock(&config);
            if mesh_guard::has_managed_fields(&config) {
                persist?;
            } else if let Err(e) = persist {
                log::warn!(
                    "config load persist failed (was_missing={was_missing}, migrated={}): {e}",
                    migration_delta.changed
                );
            }
        }
        // 刷缓存（持有写锁）。
        if let Ok(mut guard) = self.cache.write() {
            *guard = Some(config.clone());
        }
        Ok(config)
    }

    fn mesh_required_marker(&self) -> Result<Option<MeshRequiredMarker>, StoreError> {
        let path = self.dir.join(REQUIRED_MARKER_FILE);
        if !StdFs.exists(&path) {
            return Ok(None);
        }
        let raw = StdFs.read_to_string(&path)?;
        let marker: MeshRequiredMarker =
            serde_json::from_str(&raw).map_err(StoreError::from_parse)?;
        if marker.local_id.trim().is_empty()
            || marker.legacy_config_digest.len() != 64
            || !marker
                .legacy_config_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(StoreError::validation(
                "invalid mesh-route-state.required marker",
            ));
        }
        Ok(Some(marker))
    }

    fn clear_cached_config(&self) {
        if let Ok(mut guard) = self.cache.write() {
            *guard = None;
        }
    }

    fn mesh_marker_path(&self) -> PathBuf {
        self.dir.join(REQUIRED_MARKER_FILE)
    }

    fn write_mesh_marker(&self, marker: &MeshRequiredMarker) -> Result<(), StoreError> {
        let content = serde_json::to_string(marker).map_err(StoreError::from)?;
        let result = durable_atomic_write(&self.mesh_marker_path(), &content, &random_tmp_suffix());
        if matches!(&result, Err(StoreError::CommitUncertain(_))) {
            self.clear_cached_config();
        }
        result.map(|_| ())
    }

    fn read_mesh_document(&self) -> Result<Value, StoreError> {
        let content = StdFs.read_to_string(&self.path)?;
        let raw: Value = serde_json::from_str(&content).map_err(StoreError::from_parse)?;
        mesh_guard::validate_raw(&raw)?;
        Ok(raw)
    }

    fn raw_mesh_document_digest(raw: &Value) -> Result<String, StoreError> {
        let bytes = serde_json::to_vec(raw).map_err(StoreError::from)?;
        Ok(polaris_updater::sha256_hex(&bytes))
    }

    /// Read one owned Apply input from the raw disk document. This does not
    /// migrate, sanitize, cache, claim a lifecycle generation, or write a
    /// journal. A legacy fallback and a Preparing marker are both forbidden.
    #[allow(dead_code)] // Production Apply preflight follows this slice.
    pub(crate) fn read_mesh_apply_snapshot(&self) -> Result<ApplyInputSnapshot, StoreError> {
        deny_inside_projection("read_mesh_apply_snapshot");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.read_mesh_apply_snapshot_under_write_lock()
    }

    fn read_mesh_apply_snapshot_under_write_lock(&self) -> Result<ApplyInputSnapshot, StoreError> {
        let marker = self
            .mesh_required_marker()?
            .ok_or_else(|| StoreError::validation("managed Apply requires an enabled marker"))?;
        if marker.phase != MeshMarkerPhase::Enabled {
            return Err(StoreError::validation(
                "managed Apply requires an enabled marker",
            ));
        }
        // The raw disk value matters: `current()` could be a stale sanitized
        // cache, and an unknown raw key must invalidate this preflight even if
        // the frontend's projected FNV version is unchanged. JSON whitespace
        // and object-key order are intentionally semantically irrelevant.
        let content = StdFs.read_to_string(&self.path)?;
        let raw: Value = serde_json::from_str(&content).map_err(StoreError::from_parse)?;
        if !mesh_guard::validate_raw(&raw)? {
            return Err(StoreError::validation(
                "managed Apply requires a complete raw document",
            ));
        }
        let policy: MeshRoutePolicy = serde_json::from_value(raw[mesh_guard::POLICY_KEY].clone())
            .map_err(StoreError::from_parse)?;
        let state: MeshRouteState = serde_json::from_value(raw[mesh_guard::STATE_KEY].clone())
            .map_err(StoreError::from_parse)?;
        if state.local_id != marker.local_id {
            return Err(StoreError::validation(
                "managed Apply marker and document do not match",
            ));
        }
        Ok(ApplyInputSnapshot {
            config_version: config_version(&raw),
            raw_document_sha256: Self::raw_mesh_document_digest(&raw)?,
            raw,
            policy,
            state,
            marker,
        })
    }

    /// Read-only admission after any asynchronous preflight. The entire raw
    /// JSON value and enabled marker must still match the original
    /// owned snapshot; a matching configVersion or state revision alone is
    /// insufficient. The future Prepare CAS must repeat this comparison
    /// under its own write lock immediately before durable publication.
    #[allow(dead_code)] // Production Apply preflight follows this slice.
    pub(crate) fn admit_mesh_apply_snapshot(
        &self,
        snapshot: &ApplyInputSnapshot,
    ) -> Result<(), StoreError> {
        deny_inside_projection("admit_mesh_apply_snapshot");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = self.read_mesh_apply_snapshot_under_write_lock()?;
        if current.raw_document_sha256 != snapshot.raw_document_sha256
            || current.marker != snapshot.marker
            || current.config_version != snapshot.config_version
            || current.state.revision != snapshot.state.revision
        {
            return Err(StoreError::validation("managed Apply snapshot changed"));
        }
        Ok(())
    }

    fn legacy_mesh_digest(config: &Value) -> Result<String, StoreError> {
        if mesh_guard::has_managed_fields(config) {
            return Err(StoreError::validation(
                "legacy digest requires legacy config",
            ));
        }
        let canonical = ConfigStore::canonicalize_for_save(config)?;
        let bytes = serde_json::to_vec(&canonical).map_err(StoreError::from)?;
        Ok(polaris_updater::sha256_hex(&bytes))
    }

    /// Step 1 of opt-in. The marker is durable before any managed document can
    /// be published. There is deliberately no UI/product entry point in S3a.
    #[allow(dead_code)]
    pub(crate) fn prepare_mesh_route_enable(&self, local_id: &str) -> Result<(), StoreError> {
        deny_inside_projection("prepare_mesh_route_enable");
        if local_id.trim().is_empty() {
            return Err(StoreError::validation("mesh localId is empty"));
        }
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.legacy_start_leases.load(Ordering::SeqCst) != 0 {
            return Err(StoreError::validation(
                "legacy start operation is active; managed opt-in must retry",
            ));
        }
        if self.mesh_required_marker()?.is_some() {
            return Err(StoreError::validation("mesh route marker already exists"));
        }
        let legacy = self.load_full_under_write_lock()?;
        // Legacy load historically falls back to in-memory defaults for a
        // corrupt on-disk file. Opt-in may not bind a marker to that fallback:
        // the original file must remain readable and semantically identical.
        let disk_legacy = self.read_mesh_document()?;
        let digest = Self::legacy_mesh_digest(&legacy)?;
        if Self::legacy_mesh_digest(&disk_legacy)? != digest {
            return Err(StoreError::validation(
                "legacy config changed during mesh preparation",
            ));
        }
        let marker = MeshRequiredMarker {
            phase: MeshMarkerPhase::Preparing,
            local_id: local_id.into(),
            legacy_config_digest: digest,
        };
        let result = self.write_mesh_marker(&marker);
        self.clear_cached_config();
        result
    }

    /// Step 2: only the exact prepared legacy document can become managed.
    /// A crash after config rename leaves Preparing intact, and recovery can
    /// finish marker promotion without trusting an old UI snapshot.
    #[allow(dead_code)]
    pub(crate) fn commit_prepared_mesh_route(
        &self,
        policy: MeshRoutePolicy,
        state: MeshRouteState,
    ) -> Result<(), StoreError> {
        deny_inside_projection("commit_prepared_mesh_route");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut marker = self
            .mesh_required_marker()?
            .ok_or_else(|| StoreError::validation("mesh route marker is missing"))?;
        if marker.phase != MeshMarkerPhase::Preparing || marker.local_id != state.local_id {
            return Err(StoreError::validation(
                "mesh route preparation identity changed",
            ));
        }
        let mut raw = self.read_mesh_document()?;
        if mesh_guard::has_managed_fields(&raw) {
            return Err(StoreError::validation(
                "managed document already published; recover marker",
            ));
        }
        if Self::legacy_mesh_digest(&raw)? != marker.legacy_config_digest {
            return Err(StoreError::validation(
                "legacy config changed since mesh preparation",
            ));
        }
        raw[mesh_guard::POLICY_KEY] = serde_json::to_value(policy).map_err(StoreError::from)?;
        raw[mesh_guard::STATE_KEY] = serde_json::to_value(state).map_err(StoreError::from)?;
        let canonical = ConfigStore::canonicalize_for_save(&raw)?;
        let content = serde_json::to_string_pretty(&canonical).map_err(StoreError::from)?;
        let publish = durable_atomic_write(&self.path, &content, &random_tmp_suffix());
        self.clear_cached_config();
        publish?;
        marker.phase = MeshMarkerPhase::Enabled;
        self.write_mesh_marker(&marker)
    }

    /// Step 3 after an interrupted opt-in. A managed document with the same
    /// local identity is completed; a still-legacy document stays blocked so
    /// the caller can explicitly retry or prove cancellation safe.
    #[allow(dead_code)]
    pub(crate) fn recover_preparing_mesh_route(&self) -> Result<MeshPrepareRecovery, StoreError> {
        deny_inside_projection("recover_preparing_mesh_route");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut marker = self
            .mesh_required_marker()?
            .ok_or_else(|| StoreError::validation("mesh route marker is missing"))?;
        if marker.phase == MeshMarkerPhase::Enabled {
            self.raw_disk_for_mesh_under_write_lock()?;
            return Ok(MeshPrepareRecovery::Enabled);
        }
        let raw = self.read_mesh_document()?;
        if !mesh_guard::has_managed_fields(&raw) {
            if Self::legacy_mesh_digest(&raw)? != marker.legacy_config_digest {
                return Err(StoreError::validation(
                    "legacy config changed during mesh preparation",
                ));
            }
            return Ok(MeshPrepareRecovery::AwaitingCommit);
        }
        if raw[mesh_guard::STATE_KEY]["localId"] != marker.local_id {
            return Err(StoreError::validation(
                "published mesh localId differs from marker",
            ));
        }
        marker.phase = MeshMarkerPhase::Enabled;
        self.write_mesh_marker(&marker)?;
        self.clear_cached_config();
        Ok(MeshPrepareRecovery::Enabled)
    }

    /// Explicit cancellation is possible only while disk still proves that
    /// no managed document was published and the original legacy semantics
    /// have not changed. This is not a normal route-policy release operation.
    #[allow(dead_code)]
    pub(crate) fn cancel_preparing_mesh_route(&self) -> Result<(), StoreError> {
        deny_inside_projection("cancel_preparing_mesh_route");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let marker = self
            .mesh_required_marker()?
            .ok_or_else(|| StoreError::validation("mesh route marker is missing"))?;
        if marker.phase != MeshMarkerPhase::Preparing {
            return Err(StoreError::validation(
                "enabled mesh route cannot be cancelled",
            ));
        }
        let raw = self.read_mesh_document()?;
        if Self::legacy_mesh_digest(&raw)? != marker.legacy_config_digest {
            return Err(StoreError::validation(
                "mesh route cancellation proof failed",
            ));
        }
        let result = durable_remove(&self.mesh_marker_path());
        self.clear_cached_config();
        result.map(|_| ())
    }

    /// Trusted state-only CAS. Ordinary config writes cannot submit this
    /// ledger, and a no-op observation does not touch disk or bump revision.
    #[cfg(test)]
    pub(crate) fn update_mesh_state_if_revision(
        &self,
        expected_revision: &str,
        mutate: impl FnOnce(&mut MeshRouteState),
    ) -> Result<Option<MeshRouteState>, StoreError> {
        deny_inside_projection("update_mesh_state_if_revision");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut raw = self.raw_disk_for_mesh_under_write_lock()?;
        let previous: MeshRouteState = serde_json::from_value(raw[mesh_guard::STATE_KEY].clone())
            .map_err(StoreError::from_parse)?;
        let mut next = previous.clone();
        mutate(&mut next);
        if next.local_id != previous.local_id {
            return Err(StoreError::validation("mesh localId is immutable"));
        }
        // This generic state CAS has no old-config/active-plan scope proof and
        // no lifecycle gate. Identity retirement and effects require the S3c/S4
        // trusted document transaction; a closure cannot invent that proof.
        if next.identities != previous.identities
            || next.reservations != previous.reservations
            || next.identity_effects != previous.identity_effects
        {
            return Err(StoreError::validation(
                "mesh identity mutation needs a trusted complete-scope transaction",
            ));
        }
        let Some(next) =
            revise_semantic(&previous, expected_revision, next).map_err(StoreError::validation)?
        else {
            return Ok(None);
        };
        raw[mesh_guard::STATE_KEY] = serde_json::to_value(&next).map_err(StoreError::from)?;
        let canonical = ConfigStore::canonicalize_for_save(&raw)?;
        self.persist_canonical_under_write_lock(&canonical)?;
        if let Ok(mut guard) = self.cache.write() {
            *guard = Some(canonical);
        }
        Ok(Some(next))
    }

    /// S3c's narrow trusted identity write. It reads old config and ledger
    /// under one write lock, retains the old configured scope, then publishes
    /// server edit + retired ledger in one durable config.json rename.
    ///
    /// The activePlan currently stores only a digest, not owner slices. Until
    /// S4 supplies a verified old-plan manifest, any activePlan blocks this
    /// transaction. A no-owner receipt must be minted under the future S4
    /// lifecycle/state gate and remain held throughout this synchronous call.
    #[allow(dead_code)]
    pub(crate) fn update_mesh_identity_server_if_revision(
        &self,
        owner: &MeshOwnerRef,
        expected_revision: &str,
        edit: MeshServerIdentityEdit,
        no_owner: Option<&MeshNoOwnerReceipt<'_>>,
    ) -> Result<MeshRouteState, StoreError> {
        deny_inside_projection("update_mesh_identity_server_if_revision");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut raw = self.raw_disk_for_mesh_under_write_lock()?;
        let policy: MeshRoutePolicy = serde_json::from_value(raw[mesh_guard::POLICY_KEY].clone())
            .map_err(StoreError::from_parse)?;
        let previous: MeshRouteState = serde_json::from_value(raw[mesh_guard::STATE_KEY].clone())
            .map_err(StoreError::from_parse)?;
        if previous.revision != expected_revision {
            return Err(StoreError::validation("mesh state revision conflict"));
        }
        if previous.active_plan.is_some() || previous.transaction.is_some() {
            return Err(StoreError::validation(
                "old active mesh owner scope is unavailable without an S4 plan manifest",
            ));
        }
        let receipt = no_owner.ok_or_else(|| {
            StoreError::validation("mesh identity edit requires lifecycle/state no-owner proof")
        })?;
        if receipt.owner_ref != *owner
            || receipt.local_id != previous.local_id
            || receipt.state_revision != previous.revision
        {
            return Err(StoreError::validation(
                "mesh no-owner proof is stale or mismatched",
            ));
        }
        let servers = raw
            .get("servers")
            .and_then(Value::as_array)
            .ok_or_else(|| StoreError::validation("managed servers are unavailable"))?;
        let matching: Vec<usize> = servers
            .iter()
            .enumerate()
            .filter_map(|(index, server)| {
                (server["id"].as_str() == Some(owner.server_id.as_str())).then_some(index)
            })
            .collect();
        let &[old_index] = matching.as_slice() else {
            return Err(StoreError::validation(
                "old managed mesh server is absent or duplicated",
            ));
        };
        let old_server = &servers[old_index];
        if old_server["protocol"] != "tailscale" {
            return Err(StoreError::validation(
                "old managed identity is not a TS server",
            ));
        }
        let old_authority = ts_control_authority(old_server)?;
        let ledger_authority = previous
            .identities
            .iter()
            .find(|identity| {
                identity.server_id == owner.server_id
                    && identity.identity_epoch == owner.identity_epoch
            })
            .map(|identity| canonical_control_authority(&identity.control_authority))
            .transpose()
            .map_err(StoreError::validation)?;
        if ledger_authority.as_deref() != Some(old_authority.as_str()) {
            return Err(StoreError::validation(
                "old TS config and identity authority disagree",
            ));
        }
        let mut configured = Vec::new();
        for source in [
            old_server
                .get("tailscaleSettings")
                .and_then(|settings| settings.get("routes")),
            old_server
                .get("tailscaleSettings")
                .and_then(|settings| settings.get("advertiseRoutes")),
            old_server.get("meshRoutes"),
        ]
        .into_iter()
        .flatten()
        {
            let entries = source.as_array().ok_or_else(|| {
                StoreError::validation("old configured mesh routes are not a complete CIDR array")
            })?;
            for entry in entries {
                configured.push(
                    entry
                        .as_str()
                        .ok_or_else(|| {
                            StoreError::validation("old configured mesh route is not a CIDR string")
                        })?
                        .to_string(),
                );
            }
        }
        let mut replaced_server = false;
        match edit {
            MeshServerIdentityEdit::Replace(mut server) => {
                if server["id"].as_str() != Some(owner.server_id.as_str()) {
                    return Err(StoreError::validation("replacement mesh serverId changed"));
                }
                let new_authority = (server["protocol"] == "tailscale")
                    .then(|| ts_control_authority(&server))
                    .transpose()?;
                // Partial edit/restore payloads must not silently discard the
                // local auth key when only the same authority and state source
                // are edited. Never send an old authority's key to a new one.
                if new_authority.as_deref() == Some(old_authority.as_str())
                    && server["tailscaleSettings"].get("sourceTag")
                        == old_server["tailscaleSettings"].get("sourceTag")
                    && server["tailscaleSettings"].get("authKey").is_none()
                {
                    if let Some(old_key) = old_server["tailscaleSettings"].get("authKey") {
                        server["tailscaleSettings"]["authKey"] = old_key.clone();
                    }
                }
                let identity_changed = new_authority.as_deref() != Some(old_authority.as_str())
                    || ["sourceTag", "authKey", "ephemeral"].iter().any(|key| {
                        old_server["tailscaleSettings"].get(key)
                            != server["tailscaleSettings"].get(key)
                    });
                if !identity_changed {
                    return Err(StoreError::validation(
                        "mesh identity is unchanged; use ordinary config write",
                    ));
                }
                raw["servers"][old_index] = server;
                replaced_server = true;
            }
            MeshServerIdentityEdit::Delete => {
                raw["servers"].as_array_mut().unwrap().remove(old_index);
                if raw["selectedServerId"].as_str() == Some(owner.server_id.as_str()) {
                    raw["selectedServerId"] = Value::Null;
                }
            }
        }
        let mut canonical = ConfigStore::canonicalize_for_save(&raw)?;
        let new_epoch = format!("{}{}", random_tmp_suffix(), random_tmp_suffix());
        let canonical_servers = canonical["servers"]
            .as_array()
            .ok_or_else(|| StoreError::validation("canonical managed servers are unavailable"))?;
        let canonical_matches: Vec<&Value> = canonical_servers
            .iter()
            .filter(|server| server["id"].as_str() == Some(owner.server_id.as_str()))
            .collect();
        let replacement = if replaced_server {
            let &[server] = canonical_matches.as_slice() else {
                return Err(StoreError::validation(
                    "replacement mesh server was not preserved",
                ));
            };
            (server["protocol"] == "tailscale").then_some(server)
        } else {
            None
        };
        let authority = replacement.map(ts_control_authority).transpose()?;
        let new_identity = authority
            .as_deref()
            .map(|control_authority| ReplacementIdentity {
                epoch: &new_epoch,
                control_authority,
                self_stable_id: None,
            });
        let next = reconcile_controlled_identity(
            &previous,
            &policy,
            owner,
            new_identity,
            ControlledIdentityChange::ConfigReplacement,
            None,
            RetirementScopeSnapshot {
                owner_ref: owner.clone(),
                state_revision: &previous.revision,
                active_plan_id: None,
                configured_cidrs: Some(&configured),
                // Only the strict no-activePlan branch above may assert empty.
                // A digest-bearing activePlan cannot prove an empty owner slice.
                active_plan_owner_cidrs: Some(&[]),
            },
        )
        .map_err(StoreError::validation)?;
        canonical[mesh_guard::STATE_KEY] = serde_json::to_value(&next).map_err(StoreError::from)?;
        if receipt.live_generation.load(Ordering::Acquire) != receipt.expected_generation {
            return Err(StoreError::validation(
                "mesh no-owner proof expired before config publish",
            ));
        }
        self.persist_canonical_under_write_lock(&canonical)?;
        if let Ok(mut cache) = self.cache.write() {
            *cache = Some(canonical);
        }
        Ok(next)
    }

    /// Synchronous managed Apply CAS. The caller must first obtain the TS
    /// async gate, then enter `LifecycleGate::with_current_generation`; this
    /// method takes `write_lock` inside that short live-generation guard.
    /// No await, IPC, gate reentry, or external core action may occur here.
    #[allow(dead_code)]
    pub(crate) fn apply_mesh_step_if_current(
        &self,
        expected: ApplyCasExpected<'_>,
        current_boot_id: &str,
        live: &LiveClaimGuard<'_>,
        step: ApplyStep<'_>,
    ) -> Result<MeshRouteState, ApplyPersistError> {
        deny_inside_projection("apply_mesh_step_if_current");
        if matches!(&step, ApplyStep::RequestStopReserved { .. }) {
            return Err(ApplyPersistError::StopReservationUncertain(Box::new(
                ApplyPersistError::Step(ApplyError::Invalid(
                    "reserved Stop requires the typed receipt entry point",
                )),
            )));
        }
        self.apply_mesh_step_if_current_inner(expected, current_boot_id, live, step)
    }

    /// The only typed success receipt for a reserved Stop. A failed or
    /// commit-uncertain CAS yields no receipt, so it cannot be mistaken for
    /// permission to stop the old run. The caller must still check the live
    /// physical run and current request claim before issuing a StopLease.
    #[allow(dead_code)]
    #[allow(
        clippy::too_many_arguments,
        reason = "typed Stop reservation keeps its CAS and run guards explicit"
    )]
    pub(crate) fn reserve_mesh_stop_if_current(
        &self,
        expected: ApplyCasExpected<'_>,
        domain: &Arc<StopRuntimeDomain>,
        live: &LiveClaimGuard<'_>,
        plan: &polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan,
        claim: &ApplyClaim,
        stop_target_run_ref: &str,
        old_generation: u64,
        stop_generation: u64,
    ) -> Result<StopReservationReceipt, ApplyPersistError> {
        deny_inside_projection("reserve_mesh_stop_if_current");
        if !live.belongs_to(domain.gate()) || live.owner() != Some(LifecycleKind::Stop) {
            return Err(ApplyPersistError::StopReservationUncertain(Box::new(
                ApplyPersistError::Step(ApplyError::Superseded),
            )));
        }
        let state = self
            .apply_mesh_step_if_current_inner(
                expected,
                domain.boot_id(),
                live,
                ApplyStep::RequestStopReserved {
                    plan,
                    claim,
                    stop_target_run_ref,
                    old_generation,
                    stop_generation,
                },
            )
            .map_err(|cause| ApplyPersistError::StopReservationUncertain(Box::new(cause)))?;
        let tx = state.transaction.as_ref().ok_or_else(|| {
            ApplyPersistError::StopReservationUncertain(Box::new(ApplyPersistError::Step(
                ApplyError::Superseded,
            )))
        })?;
        if tx.phase != MeshTransactionPhase::StopRequested
            || tx.stop_target_run_ref.as_deref() != Some(stop_target_run_ref)
        {
            return Err(ApplyPersistError::StopReservationUncertain(Box::new(
                ApplyPersistError::Step(ApplyError::Superseded),
            )));
        }
        Ok(StopReservationReceipt {
            issuer: Arc::clone(&self.stop_receipt_issuer),
            domain: Arc::clone(domain),
            local_id: state.local_id.clone(),
            claim: ApplyClaim::from(tx),
            state_revision: state.revision,
            stop_target_run_ref: stop_target_run_ref.into(),
        })
    }

    /// Call only while holding the real TS state gate and an already-locked
    /// direct Child slot, inside `LifecycleGate::with_current_generation`.
    /// `try_lock` avoids the existing config -> selector intent -> lifecycle
    /// edge. The callback must only inspect/take that outer Child guard: no
    /// additional lock, await, IPC, signal, or disk access is allowed in it.
    #[allow(dead_code)] // Managed birth cannot yet supply an exact Child.
    pub(crate) fn try_with_current_stop_reservation<T>(
        &self,
        receipt: StopReservationReceipt,
        live: &LiveClaimGuard<'_>,
        domain: &Arc<StopRuntimeDomain>,
        plan: &polaris_config_engine::builder::managed_mesh_plan::ManagedMeshRoutePlan,
        on_current: impl FnOnce(&MeshRouteState, &StopReservationReceipt) -> T,
    ) -> Result<StopReservationCheck<T>, StoreError> {
        deny_inside_projection("try_with_current_stop_reservation");
        if !receipt.issued_by(self)
            || !receipt.belongs_to(domain)
            || !live.belongs_to(domain.gate())
            || live.owner() != Some(LifecycleKind::Stop)
            || receipt.claim().boot_id != domain.boot_id()
            || receipt.claim().lifecycle_generation != live.generation().to_string()
        {
            return Ok(StopReservationCheck::Rejected);
        }
        let _write_guard = match self.write_lock.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                return Ok(StopReservationCheck::Busy(Box::new(receipt)));
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(StoreError::Io("config write lock poisoned".into()));
            }
        };
        let snapshot = self.read_mesh_apply_snapshot_under_write_lock()?;
        if mesh_apply::check_current_stop_reservation(
            snapshot.state(),
            snapshot.config_version(),
            domain.boot_id(),
            &live.generation().to_string(),
            &receipt,
            plan,
        )
        .is_err()
        {
            return Ok(StopReservationCheck::Rejected);
        }
        let value = on_current(snapshot.state(), &receipt);
        drop(receipt);
        Ok(StopReservationCheck::Current(value))
    }

    fn apply_mesh_step_if_current_inner(
        &self,
        expected: ApplyCasExpected<'_>,
        current_boot_id: &str,
        live: &LiveClaimGuard<'_>,
        step: ApplyStep<'_>,
    ) -> Result<MeshRouteState, ApplyPersistError> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut raw = if let ApplyStep::Prepare { snapshot, .. } = &step {
            let current = self.read_mesh_apply_snapshot_under_write_lock()?;
            if current.raw_document_sha256 != snapshot.raw_document_sha256
                || current.marker != snapshot.marker
                || current.config_version != snapshot.config_version
                || current.state.revision != snapshot.state.revision
                || expected.config_version != snapshot.config_version
                || expected.state_revision != snapshot.state.revision
            {
                return Err(ApplyPersistError::ConfigChanged);
            }
            current.raw
        } else {
            self.raw_disk_for_mesh_under_write_lock()?
        };
        let previous: MeshRouteState = serde_json::from_value(raw[mesh_guard::STATE_KEY].clone())
            .map_err(StoreError::from_parse)?;
        let is_stop = matches!(&step, ApplyStep::StopIntent);
        let actual_config_version = config_version(&raw);
        if !is_stop && actual_config_version != expected.config_version {
            return Err(ApplyPersistError::ConfigChanged);
        }
        let live_generation = live.generation();
        let next = match step {
            ApplyStep::Prepare {
                snapshot: _,
                plan,
                boot_id,
                manifest_ref,
            } => {
                if !matches!(
                    live.owner(),
                    Some(LifecycleKind::Start | LifecycleKind::Restart)
                ) || boot_id != current_boot_id
                {
                    return Err(ApplyPersistError::Step(ApplyError::Superseded));
                }
                mesh_apply::record_prepared(
                    &previous,
                    expected.state_revision,
                    &actual_config_version,
                    plan,
                    boot_id,
                    &live_generation.to_string(),
                    manifest_ref,
                )
            }
            ApplyStep::Advance { plan, claim, event } => {
                if matches!(
                    &event,
                    PhaseEvent::RequestStart { .. } | PhaseEvent::RequestStop { .. }
                ) {
                    return Err(ApplyPersistError::Step(ApplyError::Invalid(
                        "stop/start request requires a reserved generation",
                    )));
                }
                // These value-level transitions deliberately accept caller-supplied
                // facts for state-machine tests. The production CAS cannot turn
                // those booleans (or a missing local Child) into proof of an
                // exact core exit and release of every OS owner. A future
                // coordinator must use a separate typed, platform-verified
                // completion entry point; until then StopRequested is durable
                // custody, never permission to publish OldStopped.
                if matches!(
                    &event,
                    PhaseEvent::NoOldCore | PhaseEvent::OldStopped { .. }
                ) {
                    return Err(ApplyPersistError::Step(ApplyError::Invalid(
                        "stop completion requires exact core and platform receipts",
                    )));
                }
                mesh_apply::advance(
                    &previous,
                    expected.state_revision,
                    &actual_config_version,
                    current_boot_id,
                    &live_generation.to_string(),
                    claim,
                    plan,
                    event,
                )
            }
            ApplyStep::RequestStartReserved {
                plan,
                claim,
                run_id,
                old_generation,
                new_generation,
            } => {
                if live.owner() != Some(LifecycleKind::Start) {
                    return Err(ApplyPersistError::Step(ApplyError::Superseded));
                }
                mesh_apply::request_start_reserved(
                    &previous,
                    expected.state_revision,
                    &actual_config_version,
                    current_boot_id,
                    live_generation,
                    claim,
                    plan,
                    run_id,
                    old_generation,
                    new_generation,
                )
            }
            ApplyStep::RequestStopReserved {
                plan,
                claim,
                stop_target_run_ref,
                old_generation,
                stop_generation,
            } => {
                if live.owner() != Some(LifecycleKind::Stop) {
                    return Err(ApplyPersistError::Step(ApplyError::Superseded));
                }
                mesh_apply::request_stop_reserved(
                    &previous,
                    expected.state_revision,
                    &actual_config_version,
                    current_boot_id,
                    live_generation,
                    claim,
                    plan,
                    stop_target_run_ref,
                    old_generation,
                    stop_generation,
                )
            }
            ApplyStep::StopIntent => {
                if live.owner() != Some(LifecycleKind::Stop) {
                    return Err(ApplyPersistError::Step(ApplyError::Superseded));
                }
                mesh_apply::record_stop_intent(&previous, expected.state_revision)
            }
        }
        .map_err(ApplyPersistError::Step)?;
        if next.local_id != previous.local_id {
            return Err(ApplyPersistError::Step(ApplyError::Invalid(
                "mesh localId is immutable",
            )));
        }
        let next = revise_semantic(&previous, expected.state_revision, next)
            .map_err(|error| ApplyPersistError::Store(StoreError::validation(error)))?
            .ok_or(ApplyPersistError::Step(ApplyError::Invalid(
                "Apply step made no change",
            )))?;
        raw[mesh_guard::STATE_KEY] = serde_json::to_value(&next).map_err(StoreError::from)?;
        let canonical = ConfigStore::canonicalize_for_save(&raw)?;
        self.persist_canonical_under_write_lock(&canonical)?;
        if let Ok(mut guard) = self.cache.write() {
            *guard = Some(canonical);
        }
        Ok(next)
    }

    /// Reads the disk truth while write_lock is held. Legacy malformed files keep
    /// their historical fallback; a marker makes every read/parse error fatal.
    fn raw_disk_for_mesh_under_write_lock(&self) -> Result<Value, StoreError> {
        let marker = self.mesh_required_marker()?;
        if marker
            .as_ref()
            .is_some_and(|marker| marker.phase == MeshMarkerPhase::Preparing)
        {
            return Err(StoreError::validation("mesh route migration is preparing"));
        }
        if !StdFs.exists(&self.path) {
            if marker.is_some() {
                return Err(StoreError::validation("managed config is missing"));
            }
            return Ok(polaris_store::store::default_config());
        }
        let content = StdFs.read_to_string(&self.path)?;
        let raw: Value = match serde_json::from_str(&content) {
            Ok(value) => value,
            Err(error) if marker.is_none() => {
                log::warn!("legacy config parse fallback during save: {error}");
                return Ok(polaris_store::store::default_config());
            }
            Err(error) => return Err(StoreError::from_parse(error)),
        };
        let managed = mesh_guard::validate_raw(&raw)?;
        match marker {
            Some(marker)
                if !managed || raw[mesh_guard::STATE_KEY]["localId"] != marker.local_id =>
            {
                Err(StoreError::validation(
                    "mesh route marker and config do not match",
                ))
            }
            None if managed => Err(StoreError::validation(
                "managed config has no required marker",
            )),
            _ => Ok(raw),
        }
    }

    /// 读 currentConfig 缓存（不触盘）。缓存未暖 → 触发一次 load_full（Polaris getCurrentConfig 懒加载）。
    ///
    /// **恒返回 owned `Value` = 恒一次整份深拷贝**（读锁只护到 clone 为止）。200 节点级配置下这不是
    /// 小数目，故**每帧 / 每 tick 调用的路径一律改用 [`with_current`](Self::with_current) 做投影**；
    /// 本方法留给「确实要整份 owned 配置」的调用点（改完要 `save_full` 的写腿、要跨 `await` 搬运给
    /// 异步任务的腿、要整份 `from_value::<UserConfig>` 的起核腿）—— 那些地方即便换成 `with_current`
    /// 也得在闭包里 clone 出整份，零收益且平白多一条闭包内禁忌。
    pub fn current(&self) -> Result<Value, StoreError> {
        deny_inside_projection("current");
        if let Ok(guard) = self.cache.read() {
            if let Some(c) = guard.as_ref() {
                return Ok(c.clone());
            }
        }
        self.load_full()
    }

    /// currentConfig 缓存的**持锁投影入口**：读锁一直持到 `f` 返回，`f` 只取它真正要的那几个字段。
    ///
    /// 与 [`current`](Self::current) 的唯一差别是**谁付整份深拷贝的账**：`current()` 恒 clone 整份配置
    /// （含 `servers` 数组与全部规则）再把 owned 值交出去；本方法一次都不 clone。缓存未暖 / 锁中毒 →
    /// 与 `current()` 同款回落：先 `load_full()` 读盘，再对结果跑 `f`（此时读锁已释放，不会撞
    /// `load_full` 内部的写锁）。
    ///
    /// # ⚠️ 闭包内禁忌（唯一、但是硬的）
    ///
    /// `f` 执行期间 `self.cache` 的**读锁是持着的**，故闭包内**禁止再调用 `ConfigManager` 的任何方法**：
    ///
    /// - `save_full` / `set_value` / `load_full` 要 `cache.write()` —— 同线程「持读锁再取写锁」是
    ///   **必然自死锁**：`std::sync::RwLock` 既不可重入也不支持读锁升级，那个 `write()` 永远等不到
    ///   自己手里的读锁释放。
    /// - `current` / `get_value` / `with_current` 只要 `cache.read()`，看似无害，**同样禁止**：std 的
    ///   `RwLock::read` 文档明写「本线程已持有该锁时可能 panic」，且在**有写者排队**时，写者优先的实现
    ///   （Linux futex 版即是）会让这次递归读**永久阻塞**。即：平时怎么测都不复现，只在「恰好有另一条
    ///   腿在写配置」的那一瞬变成死锁 —— 最难查的那类。
    ///
    /// 所以 `f` 只该做**纯投影**：从 `&Value` 取字段 → 转成 owned 值返回。不做 I/O、不回调进运行时的
    /// 其它子系统（那些子系统日后完全可能自己去读配置），也无处 `await`（本方法是同步的）。
    /// 需要「投影 + 再读一次配置」的调用点，把两次读**平铺**成先后两句，不要嵌套。
    ///
    /// 这条禁忌**在 debug 构型下是有牙的**：闭包执行期间挂着 [`reentrancy`] 探针，闭包内再调
    /// `ConfigManager` 任一入口会立刻 panic（而不是等某次「恰好有人在写配置」时挂死）。
    pub fn with_current<T>(&self, f: impl FnOnce(&Value) -> T) -> Result<T, StoreError> {
        deny_inside_projection("with_current");
        if let Ok(guard) = self.cache.read() {
            if let Some(c) = guard.as_ref() {
                let _probe = ReentrancyProbe::enter();
                return Ok(f(c));
            }
        }
        // 缓存未暖 / 锁中毒 → 读盘一次。注意读锁已随上面的 `if let` 作用域释放（`load_full` 要写锁）。
        let cfg = self.load_full()?;
        // 回落腿并不持读锁，探针仍照挂：调用方无从得知本次走了哪条腿，禁忌必须两条腿一致，
        // 否则「冷缓存下能跑、暖起来就死」是更坏的形态。
        let _probe = ReentrancyProbe::enter();
        Ok(f(&cfg))
    }

    /// 保存配置（再跑 sanitize+validate + 原子写）+ 刷缓存。上游 `saveConfig`。
    ///
    /// 顺带在此唯一汇流点做**图标缓存驱逐 reconcile**：diff 旧/新 `customAppPresets` 的 id 集，
    /// 删掉已移除自定义应用的 `<userData>/icons/<id>.*` 本地缓存。挂在这里（而非某个屏幕调 evict
    /// 命令）覆盖所有令 app id 消失的写路径（删除 / 备份整类替换 / 工厂重置），避免跨文件缝。
    /// best-effort：unlink 失败仅记日志，绝不影响配置保存本身。
    #[cfg(test)]
    pub fn save_full(&self, config: &Value) -> Result<(), StoreError> {
        deny_inside_projection("save_full");
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.save_full_under_write_lock(config).map(drop)
    }

    /// 调用方已持有 `write_lock` 的保存腿；只供 [`Self::update_with_cleanup`] 复用，避免 Mutex 重入。
    fn save_full_under_write_lock(&self, config: &Value) -> Result<Value, StoreError> {
        let _guard = self
            .deferred_delete_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.save_full_with_icon_reconcile(config, true)
    }

    /// 暂存层「保存」专用：先以旧/新配置生成持久删除意图，再落配置；此刻不执行任何不可逆清理。
    ///
    /// 意图采用 write-ahead 顺序。若进程在写意图后、写 config 前崩溃，消费端会看见实体仍在最新配置中，
    /// 将该条判为 cancelled，故不会误删；反过来若先写 config 再写意图，崩溃会永久丢失清理凭据。
    #[cfg(test)]
    pub fn save_full_deferred_cleanup(
        &self,
        current: &Value,
        config: &Value,
    ) -> Result<(), StoreError> {
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.save_full_deferred_cleanup_under_write_lock(current, config)
            .map(drop)
    }

    /// 调用方已持有 `write_lock` 的延迟清理保存腿；与普通保存共享同一条写队列。
    fn save_full_deferred_cleanup_under_write_lock(
        &self,
        current: &Value,
        config: &Value,
    ) -> Result<Value, StoreError> {
        self.save_full_deferred_cleanup_with_explicit_under_write_lock(current, config, &[])
    }

    fn save_full_deferred_cleanup_with_explicit_under_write_lock(
        &self,
        current: &Value,
        config: &Value,
        explicit: &[DeferredConfigDeletion],
    ) -> Result<Value, StoreError> {
        let _guard = self
            .deferred_delete_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 删除意图必须按**实际会写盘**的规范形求差集。若 sanitize 会剔除一个坏实体，而这里仍拿
        // 清洗前入参求差集，就会出现“磁盘实体没了、删除 journal 却从未记录”的永久资产泄漏。
        let canonical = self.canonicalize_untrusted_under_write_lock(config)?;
        let mut additions = derive_deferred_deletions(current, &canonical);
        additions.extend_from_slice(explicit);
        if mesh_guard::has_managed_fields(&canonical) {
            // The legacy deletion record contains only serverId. A managed
            // identity is (serverId, epoch); deleting by the old journal can
            // erase a newly bound epoch after replacement/recovery. S3c owns
            // exact-epoch effects, so this path must not stage TS state work.
            additions
                .retain(|entry| !matches!(entry, DeferredConfigDeletion::TailscaleState { .. }));
        }
        self.stage_deferred_deletion_entries_locked(additions)?;
        self.save_canonical_with_icon_reconcile(canonical, false)
    }

    fn save_full_with_icon_reconcile(
        &self,
        config: &Value,
        reconcile_icons: bool,
    ) -> Result<Value, StoreError> {
        deny_inside_projection("save_full");
        let canonical = self.canonicalize_untrusted_under_write_lock(config)?;
        self.save_canonical_with_icon_reconcile(canonical, reconcile_icons)
    }

    /// All ordinary config writers, including full snapshots, patch, setValue,
    /// backup merge and load-time migration, must preserve the disk ledger.
    /// In particular, an old policy snapshot cannot omit an already enabled
    /// dnsPolicy and thereby silently return managed DNS to legacy behavior.
    fn canonicalize_untrusted_under_write_lock(&self, config: &Value) -> Result<Value, StoreError> {
        let previous = self.raw_disk_for_mesh_under_write_lock()?;
        let mut incoming = config.clone();
        mesh_guard::reconcile_untrusted(&previous, &mut incoming)?;
        ConfigStore::canonicalize_for_save(&incoming)
    }

    fn save_canonical_with_icon_reconcile(
        &self,
        canonical: Value,
        reconcile_icons: bool,
    ) -> Result<Value, StoreError> {
        // 旧 id 集须在刷缓存前从当前缓存取（此刻仍持旧配置）；缓存未暖则无旧态可 diff（冷启无删除发生）。
        let old_ids = self
            .cache
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(crate::icon_cache::custom_app_ids));
        // 同上：第 4 参是随机 12hex tmp 后缀，非品牌名。每次保存都须取新值——
        // 恒定后缀会让并发 saveConfig 撞同一个 tmp 路径，原子写的隔离性即失效。
        self.persist_canonical_under_write_lock(&canonical)?;
        // LOW-4：只有 `customAppPresets` 的 id 集**实际变化**才跑 read_dir + unlink reconcile。
        // `set_value` 走此汇流点保存**任何**键（mixedPort / 开关 / 规则…），绝大多数与自定义应用无关；
        // 无条件 reconcile 会让每次保存都白遍历一遍 `<userData>/icons/`。先比 id 集，未变即跳过整个
        // 磁盘遍历。变化时行为不变（reconcile 仅删「旧有新无」，共享 / 复用 id 保留，unlink best-effort）。
        if reconcile_icons {
            if let Some(old) = old_ids {
                let new_ids = crate::icon_cache::custom_app_ids(&canonical);
                if old != new_ids {
                    crate::icon_cache::reconcile_removed(
                        &crate::icon_cache::icons_dir(&self.dir),
                        &old,
                        &new_ids,
                    );
                }
            }
        }
        if let Ok(mut guard) = self.cache.write() {
            *guard = Some(canonical.clone());
        }
        Ok(canonical)
    }

    /// The managed branch has stronger durability than legacy atomic saves.
    /// An uncertain directory sync invalidates the old cache even though this
    /// method returns Err and callers must not continue Apply side effects.
    fn persist_canonical_under_write_lock(&self, canonical: &Value) -> Result<(), StoreError> {
        if !mesh_guard::has_managed_fields(canonical) {
            return ConfigStore::save(&StdFs, &self.path, canonical, &random_tmp_suffix());
        }
        let content = serde_json::to_string_pretty(canonical).map_err(StoreError::from)?;
        let result = durable_atomic_write(&self.path, &content, &random_tmp_suffix());
        #[cfg(test)]
        let result = match result {
            Ok(_)
                if self
                    .test_uncertain_after_mesh_publish_once
                    .swap(false, Ordering::SeqCst) =>
            {
                Err(StoreError::CommitUncertain(
                    "injected after managed publish".into(),
                ))
            }
            other => other,
        };
        match result {
            Ok(_) => Ok(()),
            Err(error @ StoreError::CommitUncertain(_)) => {
                let actual = self
                    .raw_disk_for_mesh_under_write_lock()
                    .ok()
                    .filter(|raw| matches!(mesh_guard::validate_raw(raw), Ok(true)));
                if let Ok(mut guard) = self.cache.write() {
                    *guard = actual;
                }
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    fn deferred_deletions_path(&self) -> PathBuf {
        self.dir.join(DEFERRED_DELETIONS_FILE)
    }

    fn load_deferred_deletions_locked(&self) -> Result<DeferredDeletionJournal, StoreError> {
        let path = self.deferred_deletions_path();
        if !StdFs.exists(&path) {
            return Ok(DeferredDeletionJournal::default());
        }
        let text = StdFs.read_to_string(&path)?;
        let journal: DeferredDeletionJournal =
            serde_json::from_str(&text).map_err(StoreError::from_parse)?;
        if journal.version != DEFERRED_DELETIONS_VERSION {
            return Err(StoreError::validation(format!(
                "unsupported deferred deletion journal version: {}",
                journal.version
            )));
        }
        Ok(journal)
    }

    fn save_deferred_deletions_locked(
        &self,
        journal: &DeferredDeletionJournal,
    ) -> Result<(), StoreError> {
        let path = self.deferred_deletions_path();
        if journal.entries.is_empty() {
            return StdFs.remove(&path);
        }
        let content = serde_json::to_string_pretty(journal).map_err(StoreError::from)?;
        polaris_store::atomic_write_plan(&path, &random_tmp_suffix(), &content).execute(&StdFs)
    }

    #[cfg(test)]
    fn stage_deferred_deletions(
        &self,
        current: &Value,
        incoming: &Value,
    ) -> Result<(), StoreError> {
        let _guard = self
            .deferred_delete_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.stage_deferred_deletions_locked(current, incoming)
    }

    #[cfg(test)]
    fn stage_deferred_deletions_locked(
        &self,
        current: &Value,
        incoming: &Value,
    ) -> Result<(), StoreError> {
        self.stage_deferred_deletion_entries_locked(derive_deferred_deletions(current, incoming))
    }

    fn stage_deferred_deletion_entries_locked(
        &self,
        additions: Vec<DeferredConfigDeletion>,
    ) -> Result<(), StoreError> {
        if additions.is_empty() {
            return Ok(());
        }
        let mut journal = self.load_deferred_deletions_locked()?;
        for entry in additions {
            if !journal.entries.contains(&entry) {
                journal.entries.push(entry);
            }
        }
        self.save_deferred_deletions_locked(&journal)
    }

    /// Apply / 冷启动消费延迟删除。每条先按最新配置复核；执行失败保留在日志中，下一次 Apply/启动重试。
    pub(crate) fn process_deferred_deletions(
        &self,
        mut apply: impl FnMut(&DeferredConfigDeletion, &Value) -> Result<(), String>,
    ) -> Result<DeferredDeletionSummary, StoreError> {
        deny_inside_projection("process_deferred_deletions");
        // 锁序与保存腿保持一致：write → deferred-delete。消费期间配置不能被重新加入/删除；否则
        // “复核未取消 → 执行删除”之间仍有竞态。此前反向先拿 deletion 再 cold-current 拿 write，
        // 还会与 save 的 write→deletion 构成冷启动死锁。
        let _write_guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _delete_guard = self
            .deferred_delete_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // 在两把事务锁内重读磁盘真值；不用 cache，外部编辑/迁移同样必须参与取消判定。
        let current = self.load_full_under_write_lock()?;
        let journal = self.load_deferred_deletions_locked()?;
        let mut summary = DeferredDeletionSummary::default();
        let mut remaining = Vec::new();
        for entry in journal.entries {
            if mesh_guard::has_managed_fields(&current)
                && matches!(&entry, DeferredConfigDeletion::TailscaleState { .. })
            {
                // A pre-opt-in journal may still contain old serverId-only
                // entries. Retain, but never execute them under managed mode.
                summary.retrying += 1;
                remaining.push(entry);
                continue;
            }
            if deferred_deletion_is_cancelled(&entry, &current) {
                summary.cancelled += 1;
                continue;
            }
            match apply(&entry, &current) {
                Ok(()) => summary.applied += 1,
                Err(error) => {
                    summary.retrying += 1;
                    log::warn!("延迟配置删除执行失败，将在下次 Apply/启动重试: {error}");
                    remaining.push(entry);
                }
            }
        }
        self.save_deferred_deletions_locked(&DeferredDeletionJournal {
            version: DEFERRED_DELETIONS_VERSION,
            entries: remaining,
        })?;
        Ok(summary)
    }

    /// **原子读改写** —— 把「读一份配置 → 改它 → 落盘」变成一个不可分割的动作。
    ///
    /// # 它修的缺陷
    ///
    /// 历史生产写入点曾普遍采用 `load_full()` / `current()` → mutate → `save_full()` 的**分离**三步，
    /// 中间没有任何互斥。于是：
    ///
    /// - 两个写入者交错 ⇒ **丢更新**（后写的那份基于旧读，把前者的改动整份覆盖掉）；
    /// - `config_save` 的 `baseVersion` 乐观并发闸被**架空**：它在第 1 步比对版本、第 3 步才写，
    ///   任何别的写入者落在这两步之间都能让那次比对失去意义。
    ///
    /// 而这确实可达：订阅自动更新写验证器、诊断抓包恢复、热切 commit 都跑在 tokio 任务里，
    /// 与命令处理天然并发。
    ///
    /// # 闭包返回「写不写」+ **调用方自己的返回值**，而不是 `Result`
    ///
    /// 这里的形状是被全仓 30 个站点的普查逼出来的，别按直觉改回 `Result<Option<T>, E>`：
    /// 「不写」这条出口**既不唯一、也不都是错误**。逐站点数下来，读与写之间有 2–4 条不写的出口，
    /// 而其中好几条是**带不同载荷的成功**：
    ///
    /// - `server_delete_batch` 无命中 → `ApiResponse::ok(0u32)`
    /// - `rule_resources_delete` NotFound → `ApiResponse::ok(json!({…}))`
    /// - `rules_reorder` 净零序 → `ok_void()`（**刻意不 save 不广播**）
    /// - `perform_subscription_update` 内容等价 → `update_ok(0,0,0,true,…)`
    /// - `proxy.rs` 热切 commit → `false`
    ///
    /// 一个笼统的 `Ok(None)` 装不下它们；而把它们塞进 `E` 则要么得给共享 crate 加
    /// `From<StoreError> for String`，要么得借 `StoreError::Validation` 转手 —— 后者的 Display 是
    /// `"config validation failed: {0}"`，会把 `"服务器不存在: xxx"` 污染成
    /// `"config validation failed: 服务器不存在: xxx"`，是真的用户可见变化。
    ///
    /// 故：闭包返回 [`Decision<R>`]，`R` 就是调用方要 return 的那个东西（任意类型）。
    /// 读或写失败 ⇒ `Err(StoreError)`，调用方照它今天的写法映射（30 个站点今天都把读失败与写失败
    /// 收敛成同一句 `ApiResponse::err(format!("{e}"))`，故合并不丢信息）。
    ///
    /// `Write` 腿连**已落盘的那份配置**一起返回（`Some(cfg)`）：调用方拿它去
    /// `broadcast_config_changed`，不必再读一次（再读又是一次可被别人插入的窗口）。
    /// `Skip` 腿给 `None` —— **它必须不广播**：净零改动多发一次 `configChanged` 就多一次
    /// `switch_mode` 评估。
    ///
    /// # 整份替换也走这里（不需要第二个原语）
    ///
    /// 闭包拿到的是 `&mut Value`，故「整份替换」就是 `*cfg = next.clone()`
    /// （备份导入、`config_save` 落用户提交的全量配置都是这一形态）。**不要**为它另开一个跳过读的
    /// 入口：那等于又造一条不持锁的写路径，而本方法存在的全部意义就是「只剩一条」。
    ///
    /// # 读的是锁内 `load_full_under_write_lock`，不是 `current`（勿改）
    ///
    /// 本方法自己拥有那次锁内磁盘重读。有人把它改成基于 `current()` 会**偷偷把不变式换成
    /// 「与缓存一致」** —— 而缓存只由本进程的写刷新，外部改动（用户手改 config.json、另一个进程）
    /// 一律看不见，于是「原子读改写」退化成「原子地基于一份可能过期的快照改写」。
    ///
    /// # 锁的边界：到落盘为止，**不含广播**
    ///
    /// 调用方必须在本方法**返回之后**才 `broadcast_config_changed`。那条广播会
    /// `spawn(switch_mode_with(...))`，而 `switch_mode` 有几条腿回读 `config.current()`；把广播圈进
    /// 临界区（或改成同步等待它）就是把一个会回读配置的调用放进持锁区间。这里是后来者最容易
    /// 好心扩大锁范围的地方。
    ///
    /// # 不重入（静态、全构建配置）
    ///
    /// `write_lock` 是私有字段，临界区内只调用显式的 `*_under_write_lock` 腿，任何被调方都不会
    /// 再次获取它。这是构造性结论，不依赖 debug-only 的 [`reentrancy`] 探针。
    ///
    /// 本方法入口直接调用 `deny_inside_projection`；因此在 `with_current` 闭包里调用 `update` 会在
    /// 获取事务锁之前立刻 panic，而不会等到缓存写锁处自死锁。
    ///
    /// # 锁中毒
    ///
    /// 闭包 panic 会毒化 `write_lock`；此处**恢复**而非传播。落盘是原子的（tmp→rename），
    /// 一次 panic 留下的要么是完整旧文件要么是完整新文件，没有撕裂态；而让一次闭包 panic
    /// 永久锁死此后所有配置写入，比那次 panic 本身糟得多。
    pub fn update<R>(
        &self,
        f: impl FnOnce(&mut Value) -> Decision<R>,
    ) -> Result<(R, Option<Value>), StoreError> {
        self.update_with_cleanup(false, f)
    }

    /// [`Self::update`] 的延迟不可逆清理形态：同一临界区内保留旧配置、生成删除 journal、落新配置。
    pub fn update_deferred_cleanup<R>(
        &self,
        f: impl FnOnce(&mut Value) -> Decision<R>,
    ) -> Result<(R, Option<Value>), StoreError> {
        self.update_with_cleanup(true, f)
    }

    /// 在普通配置差集之外附带显式不可逆动作。用于“重置内置资源”这类配置里没有对应集合实体可供
    /// 自动 diff 的操作；意图与配置仍按同一个 write-ahead 临界区提交，Skip 腿绝不写 journal。
    pub(crate) fn update_with_explicit_deletions<R>(
        &self,
        deletions: Vec<DeferredConfigDeletion>,
        f: impl FnOnce(&mut Value) -> Decision<R>,
    ) -> Result<(R, Option<Value>), StoreError> {
        deny_inside_projection("update_with_explicit_deletions");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cfg = self.load_full_under_write_lock()?;
        let current = cfg.clone();
        match f(&mut cfg) {
            Decision::Skip(r) => Ok((r, None)),
            Decision::Write(r) => {
                let saved = self.save_full_deferred_cleanup_with_explicit_under_write_lock(
                    &current, &cfg, &deletions,
                )?;
                Ok((r, Some(saved)))
            }
        }
    }

    pub fn update_with_cleanup<R>(
        &self,
        defer_cleanup: bool,
        f: impl FnOnce(&mut Value) -> Decision<R>,
    ) -> Result<(R, Option<Value>), StoreError> {
        deny_inside_projection("update_with_cleanup");
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut cfg = self.load_full_under_write_lock()?;
        let current = defer_cleanup.then(|| cfg.clone());
        match f(&mut cfg) {
            // 不写：闭包对 `cfg` 的改动一律丢弃（它拿的是本方法的局部副本）。
            Decision::Skip(r) => Ok((r, None)),
            Decision::Write(r) => {
                let saved = if let Some(current) = current.as_ref() {
                    self.save_full_deferred_cleanup_under_write_lock(current, &cfg)?
                } else {
                    self.save_full_under_write_lock(&cfg)?
                };
                Ok((r, Some(saved)))
            }
        }
    }

    /// 取单键（currentConfig 投影）。上游 `configManager.get(key)`。
    ///
    /// Polaris 的 get 支持 dotted path（如 'servers'）；此处投影顶层键（与 Polaris ConfigManager.get
    /// 主路径一致，复杂路径交由渲染端处理）。
    pub fn get_value(&self, key: &str) -> Result<Value, StoreError> {
        deny_inside_projection("get_value");
        let cfg = self.current()?;
        Ok(cfg.get(key).cloned().unwrap_or(Value::Null))
    }

    /// 置单键（currentConfig 原地改 + 落盘 + 广播由调用方触发）。上游 `configManager.set(key, value)`。
    #[cfg(test)]
    pub fn set_value(&self, key: &str, value: Value) -> Result<Value, StoreError> {
        deny_inside_projection("set_value");
        let (_, saved) = self.update(|cfg| {
            // 原地替换 / 插入顶层键。
            if let Some(obj) = cfg.as_object_mut() {
                obj.insert(key.to_string(), value);
            }
            Decision::Write(())
        })?;
        saved.ok_or_else(|| StoreError::validation("set_value write unexpectedly skipped"))
    }

    /// 取配置目录下某子路径（mesh state / helper token 等复用）。
    #[must_use]
    pub fn join(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.dir.join(relative)
    }
}

fn collection_ids(config: &Value, key: &str) -> std::collections::HashSet<String> {
    config
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn removed_collection_entries<'a>(
    current: &'a Value,
    incoming: &Value,
    key: &str,
) -> Vec<&'a Value> {
    let next_ids = collection_ids(incoming, key);
    current
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    item.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !next_ids.contains(id))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn warp_credentials(server: &Value) -> Option<(&str, &str)> {
    let device = server.get("wireguardSettings")?.get("warpDevice")?;
    Some((
        device.get("deviceId")?.as_str()?,
        device.get("token")?.as_str()?,
    ))
}

fn derive_deferred_deletions(current: &Value, incoming: &Value) -> Vec<DeferredConfigDeletion> {
    let mut out = Vec::new();
    for server in removed_collection_entries(current, incoming, "servers") {
        let Some(server_id) = server.get("id").and_then(Value::as_str) else {
            continue;
        };
        if server
            .get("protocol")
            .and_then(Value::as_str)
            .is_some_and(|protocol| protocol.eq_ignore_ascii_case("tailscale"))
        {
            out.push(DeferredConfigDeletion::TailscaleState {
                server_id: server_id.to_string(),
            });
        }
        if let Some((device_id, token)) = warp_credentials(server) {
            if !device_id.is_empty() && !token.is_empty() {
                out.push(DeferredConfigDeletion::WarpDevice {
                    server_id: server_id.to_string(),
                    device_id: device_id.to_string(),
                    token: token.to_string(),
                });
            }
        }
    }
    for resource in removed_collection_entries(current, incoming, "ruleResources") {
        if let Some(file_name) = resource
            .get("fileName")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
        {
            out.push(DeferredConfigDeletion::RuleResource {
                file_name: file_name.to_string(),
            });
        }
    }
    for preset in removed_collection_entries(current, incoming, "customAppPresets") {
        if let Some(app_id) = preset.get("id").and_then(Value::as_str) {
            out.push(DeferredConfigDeletion::AppIcon {
                app_id: app_id.to_string(),
            });
        }
    }
    out
}

fn deferred_deletion_is_cancelled(entry: &DeferredConfigDeletion, current: &Value) -> bool {
    match entry {
        DeferredConfigDeletion::RuleResource { file_name } => current
            .get("ruleResources")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|item| {
                    item.get("fileName").and_then(Value::as_str) == Some(file_name.as_str())
                })
            }),
        DeferredConfigDeletion::BuiltinRuleResource { tag, .. } => current
            .get("builtinGeoMeta")
            .and_then(Value::as_object)
            .is_some_and(|metadata| metadata.contains_key(tag)),
        DeferredConfigDeletion::AppIcon { app_id } => {
            let removed_stem = crate::icon_cache::sanitize_stem(app_id);
            collection_ids(current, "customAppPresets")
                .iter()
                .any(|current_id| crate::icon_cache::sanitize_stem(current_id) == removed_stem)
        }
        DeferredConfigDeletion::TailscaleState { server_id } => current
            .get("servers")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().any(|server| {
                    server.get("id").and_then(Value::as_str) == Some(server_id.as_str())
                        && server
                            .get("protocol")
                            .and_then(Value::as_str)
                            .is_some_and(|protocol| protocol.eq_ignore_ascii_case("tailscale"))
                })
            }),
        DeferredConfigDeletion::WarpDevice {
            device_id, token, ..
        } => current
            .get("servers")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items
                    .iter()
                    .any(|server| warp_credentials(server) == Some((device_id, token)))
            }),
    }
}

#[cfg(test)]
mod tests;
