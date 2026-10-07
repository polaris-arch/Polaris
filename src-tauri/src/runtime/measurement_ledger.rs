//! 测速结果账本：所有来源的逐节点结果按存放键留最新一条，读取时现算它还能不能用于选点。
//!
//! - **入账点只有一个**：[`RunEvents`](crate::commands::speedtest::RunEvents) 给逐节点结果盖章的
//!   那一处。手动、故障切换、周期计划、出口伴测、停止态临时核的结果都进账；入账时不筛资格，
//!   能否进选点由读取时的判据决定。
//! - **不落盘**：运行号是进程内序列，结果的当前性绑定核世代，进程重启后两者都从头开始。
//! - **不随核世代清空**：旧世代的结果留着用于显示，读取时判为过期。
//!
//! 读取判据是测量层那一组纯函数（存放键、新旧比较、过期原因、可否选点）再叠加两条只属于账本的
//! 时间判据：新鲜期上限与前台代次（见 [`LedgerStale`]）。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::Notify;

use crate::commands::speedtest::{
    is_selectable, result_key, stale_reason, supersedes_stored, CoreInstance, CurrentState,
    MeasureFailure, MeasurePath, Measured, ResultIdentity, ResultKey, SpeedTestOrigin, StaleReason,
};

/// 条数硬上限。按每条约 300 字节估，合计约 2.5MB。
const HARD_CAP: usize = 8192;

/// 软上限相对当前配置节点数的倍数：两条路径乘以至多两个测速 URL。
const SOFT_CAP_PER_NODE: usize = 4;

/// 账本里的一条记录：某个存放键下最新的那一次测量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LedgerEntry {
    pub(crate) measured: Measured,
    pub(crate) identity: ResultIdentity,
    /// 入账那一刻的前台代次。
    pub(crate) foreground_epoch: u64,
    /// 到这一条为止的连续失败次数（成功即归零；手动测过即从头数）。
    pub(crate) consecutive_failures: u32,
    /// 这个键最近一次成功的时刻（Unix 毫秒）。
    pub(crate) last_ok_at: Option<u64>,
}

/// 一条结果不再可用于选点的原因。前一项来自测量层的三条身份判据，后两项只属于账本。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LedgerStale {
    Identity(StaleReason),
    /// 超过新鲜期上限。
    Expired,
    /// 入账之后前台代次变过（只在移动端判）。
    ForegroundEpoch,
}

/// 节点没有进可选点列表的原因。「失败」与其余各项分开：只有它是节点本身的负面证据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Exclusion {
    NeverMeasured,
    Failed {
        failure: MeasureFailure,
        consecutive: u32,
    },
    Stale(LedgerStale),
    /// 最近一轮起测前即被预筛跳过，附原因。
    Skipped(&'static str),
    /// 只有经本机代理入站测得的结果。
    NonCandidatePath,
    /// 只有停止态临时核测得的结果。
    Disconnected,
}

/// 可选点的一个候选。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) node_id: String,
    pub(crate) latency_ms: u32,
    pub(crate) measured_at: u64,
    pub(crate) run: u64,
}

/// 取候选的返回：可选点的，与被排除的及其原因。两个列表都按入参顺序。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Candidates {
    pub(crate) selectable: Vec<Candidate>,
    pub(crate) excluded: Vec<(String, Exclusion)>,
}

/// 读取时的对照面，由读取方在读的那一刻取。账本不缓存任何判定结果。
pub(crate) struct ReadContext<'a> {
    /// 当前主核世代；核不在运行时为 `None`。
    pub(crate) main_generation: Option<u64>,
    /// 当前配置里逐节点的参数指纹。
    pub(crate) fingerprints: &'a BTreeMap<String, String>,
    /// 当前网络代次；未知时为 `None`。
    pub(crate) network_epoch: Option<u64>,
    /// 当前测速 URL 的摘要。
    pub(crate) url_digest: &'a str,
    pub(crate) now_ms: u64,
    /// 当前前台代次。桌面传 `None`：桌面有网络代次，不靠它。
    pub(crate) foreground_epoch: Option<u64>,
    /// 逐节点的新鲜期上限（毫秒）。
    pub(crate) freshness_cap_ms: &'a dyn Fn(&str) -> u64,
}

/// 淘汰时的对照面。
pub(crate) struct EvictContext<'a> {
    /// 当前配置里的全部节点 id。
    pub(crate) nodes: &'a BTreeSet<String>,
    pub(crate) url_digest: &'a str,
    pub(crate) now_ms: u64,
    pub(crate) freshness_cap_ms: &'a dyn Fn(&str) -> u64,
}

impl Candidates {
    /// 这组节点是否已被一轮完整覆盖：每个可测的节点都有当前结果（测出了值，或真测了没通）。
    /// 起测前被预筛跳过的不算欠账；从未测过、已过期、只有非候选结果的都算没覆盖。
    /// 「首轮是否完成」的判据：被抢占后为否，补发完成后为是。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn covers_all_testable(&self) -> bool {
        self.excluded.iter().all(|(_, exclusion)| {
            matches!(exclusion, Exclusion::Failed { .. } | Exclusion::Skipped(_))
        })
    }
}

#[derive(Default)]
struct Inner {
    entries: BTreeMap<ResultKey, LedgerEntry>,
    /// 最近一轮起测前被预筛跳过的节点及原因；节点一旦有新结果入账即移除。
    skipped: BTreeMap<String, &'static str>,
    /// 因运行号或序号不比已有记录新而被拒的入账次数。
    rejected_not_newer: u64,
}

pub(crate) struct MeasurementLedger {
    inner: Mutex<Inner>,
    version: AtomicU64,
    changed: Notify,
    foreground_epoch: AtomicU64,
}

static LEDGER: MeasurementLedger = MeasurementLedger::new();

/// 进程级的那一本账。
pub(crate) fn global() -> &'static MeasurementLedger {
    &LEDGER
}

impl MeasurementLedger {
    pub(crate) const fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                entries: BTreeMap::new(),
                skipped: BTreeMap::new(),
                rejected_not_newer: 0,
            }),
            version: AtomicU64::new(0),
            changed: Notify::const_new(),
            foreground_epoch: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn bump(&self) {
        self.version.fetch_add(1, Ordering::AcqRel);
        self.changed.notify_waiters();
    }

    /// 账本版本：每次入账或淘汰后加一。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    /// 等到版本越过 `seen` 为止，返回新版本。消费方据此重新评估，不必轮询。
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) async fn changed_since(&self, seen: u64) -> u64 {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            // 先登记再看状态：反过来会漏掉两步之间到达的那次通知。
            notified.as_mut().enable();
            let version = self.version();
            if version > seen {
                return version;
            }
            notified.await;
        }
    }

    pub(crate) fn foreground_epoch(&self) -> u64 {
        self.foreground_epoch.load(Ordering::Acquire)
    }

    /// 调度器把它的前台代次同步过来。代次变了之后，此前入账的结果在移动端不再可用于选点。
    pub(crate) fn set_foreground_epoch(&self, epoch: u64) {
        self.foreground_epoch.store(epoch, Ordering::Release);
    }

    /// 入账一条盖过章的逐节点结果。返回是否取代了已有记录。
    pub(crate) fn record(
        &self,
        node_id: &str,
        measured: Measured,
        identity: ResultIdentity,
    ) -> bool {
        let key = result_key(node_id, &identity);
        let mut inner = self.lock();
        let stored = inner.entries.get(&key);
        if !supersedes_stored(&identity, stored.map(|entry| &entry.identity)) {
            inner.rejected_not_newer += 1;
            return false;
        }
        let manual = identity.origin == SpeedTestOrigin::Manual;
        let previous_failures = stored
            .filter(|_| !manual)
            .map_or(0, |entry| entry.consecutive_failures);
        let entry = LedgerEntry {
            consecutive_failures: if measured.is_ok() {
                0
            } else {
                previous_failures + 1
            },
            last_ok_at: if measured.is_ok() {
                Some(identity.measured_at)
            } else {
                stored.and_then(|entry| entry.last_ok_at)
            },
            foreground_epoch: self.foreground_epoch(),
            measured,
            identity,
        };
        inner.entries.insert(key, entry);
        inner.skipped.remove(node_id);
        if inner.entries.len() > HARD_CAP {
            // 这里没有配置可对照，只按测量时刻淘汰最旧的一条；按配置的淘汰在每轮收尾时做。
            if let Some(oldest) = inner
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.identity.measured_at)
                .map(|(key, _)| key.clone())
            {
                inner.entries.remove(&oldest);
            }
        }
        drop(inner);
        self.bump();
        true
    }

    /// 登记一轮起测前被预筛跳过的节点。`members` 是这一轮的全部目标：它们旧的跳过标记先清掉。
    pub(crate) fn set_skipped(&self, members: &[String], skipped: &[(String, &'static str)]) {
        let mut inner = self.lock();
        for id in members {
            inner.skipped.remove(id);
        }
        for (id, reason) in skipped {
            inner.skipped.insert(id.clone(), reason);
        }
    }

    /// 按软上限淘汰（每轮收尾时调）。返回淘汰的条数。
    pub(crate) fn evict(&self, context: &EvictContext<'_>) -> usize {
        let mut inner = self.lock();
        let soft_cap = (context.nodes.len() * SOFT_CAP_PER_NODE).min(HARD_CAP);
        let excess = inner.entries.len().saturating_sub(soft_cap);
        let victims: Vec<ResultKey> = eviction_order(&inner.entries, context)
            .into_iter()
            .take(excess)
            .collect();
        for key in &victims {
            inner.entries.remove(key);
        }
        drop(inner);
        if !victims.is_empty() {
            self.bump();
        }
        victims.len()
    }

    /// 取候选：`node_ids` 里哪些现在可以进选点，其余各因为什么不行。
    pub(crate) fn candidates(&self, node_ids: &[String], context: &ReadContext<'_>) -> Candidates {
        let inner = self.lock();
        let mut out = Candidates::default();
        for id in node_ids {
            match classify(&inner, id, context) {
                Ok(candidate) => out.selectable.push(candidate),
                Err(exclusion) => out.excluded.push((id.clone(), exclusion)),
            }
        }
        out
    }

    /// `node_ids` 里已有当前结果的节点：测出了值，或确实测过而没通。过期、未测、被跳过的不算。
    pub(crate) fn current_results(
        &self,
        node_ids: &[String],
        context: &ReadContext<'_>,
    ) -> BTreeSet<String> {
        let Candidates {
            selectable,
            excluded,
        } = self.candidates(node_ids, context);
        selectable
            .into_iter()
            .map(|candidate| candidate.node_id)
            .chain(
                excluded
                    .into_iter()
                    .filter(|(_, exclusion)| matches!(exclusion, Exclusion::Failed { .. }))
                    .map(|(id, _)| id),
            )
            .collect()
    }

    /// 某个节点在候选路径、当前测速 URL 下的记录（排序与退避的取材面）。
    pub(crate) fn candidate_entry(&self, node_id: &str, url_digest: &str) -> Option<LedgerEntry> {
        self.lock()
            .entries
            .iter()
            .find(|(key, _)| {
                key.node_id == node_id
                    && key.path == MeasurePath::Candidate
                    && key.url_digest == url_digest
            })
            .map(|(_, entry)| entry.clone())
    }

    /// 被拒的入账次数（运行号或序号不比已有记录新）。
    #[cfg(test)]
    pub(crate) fn rejected_not_newer(&self) -> u64 {
        self.lock().rejected_not_newer
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().entries.len()
    }
}

/// 一条记录是否已不可用于选点。先判测量层的三条身份判据，再判账本自己的两条时间判据。
fn ledger_stale(
    entry: &LedgerEntry,
    node_id: &str,
    context: &ReadContext<'_>,
) -> Option<LedgerStale> {
    let current = CurrentState {
        main_generation: context.main_generation,
        node_fingerprint: context.fingerprints.get(node_id).map(String::as_str),
        network_epoch: context.network_epoch,
    };
    if let Some(reason) = stale_reason(&entry.identity, &current) {
        return Some(LedgerStale::Identity(reason));
    }
    if context.now_ms.saturating_sub(entry.identity.measured_at)
        > (context.freshness_cap_ms)(node_id)
    {
        return Some(LedgerStale::Expired);
    }
    if context
        .foreground_epoch
        .is_some_and(|epoch| epoch != entry.foreground_epoch)
    {
        return Some(LedgerStale::ForegroundEpoch);
    }
    None
}

fn classify(
    inner: &Inner,
    node_id: &str,
    context: &ReadContext<'_>,
) -> Result<Candidate, Exclusion> {
    let of_path = |path: MeasurePath| {
        inner.entries.iter().find(|(key, _)| {
            key.node_id == node_id && key.path == path && key.url_digest == context.url_digest
        })
    };
    let Some((_, entry)) = of_path(MeasurePath::Candidate) else {
        return Err(match inner.skipped.get(node_id) {
            Some(reason) => Exclusion::Skipped(reason),
            None if of_path(MeasurePath::System).is_some() => Exclusion::NonCandidatePath,
            None => Exclusion::NeverMeasured,
        });
    };
    if entry.identity.instance == CoreInstance::Temp {
        return Err(Exclusion::Disconnected);
    }
    if let Some(stale) = ledger_stale(entry, node_id, context) {
        return Err(Exclusion::Stale(stale));
    }
    let current = CurrentState {
        main_generation: context.main_generation,
        node_fingerprint: context.fingerprints.get(node_id).map(String::as_str),
        network_epoch: context.network_epoch,
    };
    match entry.measured {
        Ok(latency_ms) if is_selectable(&entry.measured, &entry.identity, &current) => {
            Ok(Candidate {
                node_id: node_id.to_string(),
                latency_ms,
                measured_at: entry.identity.measured_at,
                run: entry.identity.run,
            })
        }
        Ok(_) => Err(Exclusion::NonCandidatePath),
        Err(failure) => Err(Exclusion::Failed {
            failure,
            consecutive: entry.consecutive_failures,
        }),
    }
}

/// 淘汰顺序：节点已不在配置里的键；URL 不是当前测速 URL 且已超过新鲜期上限的键；
/// 其余按测量时刻从旧到新。
fn eviction_order(
    entries: &BTreeMap<ResultKey, LedgerEntry>,
    context: &EvictContext<'_>,
) -> Vec<ResultKey> {
    let mut ranked: Vec<(u8, u64, &ResultKey)> = entries
        .iter()
        .map(|(key, entry)| {
            let measured_at = entry.identity.measured_at;
            let class = if !context.nodes.contains(&key.node_id) {
                0
            } else if key.url_digest != context.url_digest
                && context.now_ms.saturating_sub(measured_at)
                    > (context.freshness_cap_ms)(&key.node_id)
            {
                1
            } else {
                2
            };
            (class, measured_at, key)
        })
        .collect();
    ranked.sort();
    ranked.into_iter().map(|(_, _, key)| key.clone()).collect()
}

#[cfg(test)]
mod tests;
