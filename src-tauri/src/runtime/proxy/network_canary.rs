//! 网络场景命中态：内核 canary 探针（spec §6.3 方案 2，N4）。
//!
//! 内核没有任何 API 能读出 transport 当前拿到的 DNS 地址或搜索域（spec K9），所以「当前是否处在该网络」
//! 只能让**内核亲自求值**：生成器给每个可用场景写一组 canary DNS 规则 + 一个只听 127.0.0.1 的 UDP
//! `direct` 入站（`builder::network_env::apply_network_canaries`），本模块向该入站发 DNS 查询 ——
//! A 127.0.0.1 = 命中，NXDOMAIN = 未命中，其余（超时 / 核没起 / 应答畸形）= 未知。
//!
//! - **不读核日志**：隐私模式把核日志抬到 ≥warn 后照样工作（spec §6.3 方案 3 的缺陷不在这条路上）。
//! - **周期 + 网络变化即时**：每 [`CANARY_PROBE_INTERVAL`] 探一轮；网络变化先把全部结果置为未知、再立即
//!   探一轮（[`NetworkCanaryState::invalidate`]）。dhcp 源的首次失败是粘滞的（spec K7 / R1），canary
//!   如实报「未命中」—— 这正是 R1 要的可见性。
//! - **运行核为准**：探的是**正在跑的那个核**的配置（起核就绪时 [`NetworkCanaryState::arm`] 换上本次
//!   生成的 canary 表），不是磁盘上的下一份。核停 / 崩溃 ⇒ [`NetworkCanaryState::disarm`]，全部未知。
//!
//! 结果变化时经 `ProxyErrorEmitter::emit_network_profile_match_changed` 发一个**无载荷**信号，渲染端据此
//! 重拉 `network_profile_resolved_sources`（其 `matched` 字段由 [`NetworkCanaryState::matched`] 填入）——
//! 与 `onLifecycle` 等既有推送同一约定：事件是变更信号，payload 不复制易过期的快照。

use std::collections::BTreeMap;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use polaris_config_engine::builder::network_env::{NetworkCanary, NetworkCanaryPlan};
use tokio::sync::Notify;

use super::ProxyRuntime;

/// 周期探测间隔（spec §6.3「例如 5s」）。查询只在回环上完成，不出网。
pub(super) const CANARY_PROBE_INTERVAL: Duration = Duration::from_secs(5);
/// 单次查询等应答的上限。`predefined` 应答不经 transport（spec K10），正常在毫秒级；超时 = 未知。
const CANARY_QUERY_TIMEOUT: Duration = Duration::from_millis(800);

#[derive(Debug, Default)]
struct Inner {
    /// 会话代：每次 arm / disarm 递增。探测任务只服务自己那一代，代不符即退场。
    session: u64,
    /// 轮次代：会话代变化或网络变化都递增。在飞那一轮的结果只在轮次代未变时才写回
    /// （网络变化那一刻还在飞的查询问的是旧网络）。
    round: u64,
    plan: Option<NetworkCanaryPlan>,
    /// 场景 id → 命中态（`None` = 未知）。只含本次 plan 里的场景。
    matches: BTreeMap<String, Option<bool>>,
}

/// canary 命中态的唯一持有者（`ProxyRuntime::network_canary`）。
#[derive(Debug, Default)]
pub(crate) struct NetworkCanaryState {
    inner: Mutex<Inner>,
    /// 唤醒探测任务：网络变化要立即探；disarm 要让它立即退场而不是睡满一个周期。
    wake: Notify,
}

impl NetworkCanaryState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 起核就绪：换上**本次运行核**的 canary 表（`None` = 本次没生成），结果全部置为未知。
    /// 返回 `(本会话代, 对外可见状态是否变了)`；会话代交给本次的探测任务。
    pub(super) fn arm(&self, plan: Option<NetworkCanaryPlan>) -> (u64, bool) {
        let mut inner = self.lock();
        inner.session = inner.session.wrapping_add(1);
        inner.round = inner.round.wrapping_add(1);
        let matches: BTreeMap<String, Option<bool>> = plan
            .iter()
            .flat_map(|p| p.canaries.iter())
            .map(|c| (c.profile_id.clone(), None))
            .collect();
        let changed = inner.matches.values().any(Option::is_some) || inner.matches != matches;
        inner.matches = matches;
        inner.plan = plan;
        let session = inner.session;
        drop(inner);
        self.wake.notify_waiters();
        (session, changed)
    }

    /// 核停止 / 崩溃：旧探测任务退场、清空结果（全部回到未知）。返回对外可见状态是否变了。
    pub(super) fn disarm(&self) -> bool {
        let mut inner = self.lock();
        inner.session = inner.session.wrapping_add(1);
        inner.round = inner.round.wrapping_add(1);
        inner.plan = None;
        let changed = !inner.matches.is_empty();
        inner.matches.clear();
        drop(inner);
        self.wake.notify_waiters();
        changed
    }

    /// 网络变化：已知结果全部作废（未知），唤醒探测任务立即重探。返回对外可见状态是否变了。
    pub(super) fn invalidate(&self) -> bool {
        let mut inner = self.lock();
        inner.round = inner.round.wrapping_add(1);
        let changed = inner.matches.values().any(Option::is_some);
        for value in inner.matches.values_mut() {
            *value = None;
        }
        drop(inner);
        self.wake.notify_waiters();
        changed
    }

    /// 某场景当前的命中态（`None` = 未知 / 本次没有它的 canary）。
    pub(crate) fn matched(&self, profile_id: &str) -> Option<bool> {
        self.lock().matches.get(profile_id).copied().flatten()
    }

    /// `session` 这一代要探的目标：`(轮次代, 端口, canary 表)`；会话已换 / 核已停 ⇒ `None`。
    fn target(&self, session: u64) -> Option<(u64, u16, Vec<NetworkCanary>)> {
        let inner = self.lock();
        if inner.session != session {
            return None;
        }
        let plan = inner.plan.as_ref()?;
        Some((inner.round, plan.port, plan.canaries.clone()))
    }

    /// 写回一轮结果；轮次代已变（期间网络变化 / 核停 / 换核）⇒ 作废。返回对外可见状态是否变了。
    fn record(&self, round: u64, results: BTreeMap<String, Option<bool>>) -> bool {
        let mut inner = self.lock();
        if inner.round != round {
            return false;
        }
        let changed = inner.matches != results;
        inner.matches = results;
        changed
    }
}

/// 探测循环：服务 `session` 这一代，直到下一次 arm / disarm。
///
/// `query` 注入「向回环入站发一次 canary 查询」（生产 = [`query_canary`]；单测注入桩），
/// `on_change` 在对外可见状态变化时调用（生产 = 发无载荷事件）。
pub(super) async fn run_canary_probe<Q, F, C>(
    state: &NetworkCanaryState,
    session: u64,
    interval: Duration,
    query: Q,
    on_change: C,
) where
    Q: Fn(SocketAddr, String) -> F,
    F: Future<Output = Option<bool>>,
    C: Fn(),
{
    loop {
        // 先登记唤醒再取目标：取目标之后才发生的网络变化 / disarm 不会被漏掉。
        let woken = state.wake.notified();
        tokio::pin!(woken);
        woken.as_mut().enable();
        let Some((round, port, canaries)) = state.target(session) else {
            return;
        };
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let mut results = BTreeMap::new();
        for canary in canaries {
            let hit = query(addr, canary.domain).await;
            results.insert(canary.profile_id, hit);
        }
        if state.record(round, results) {
            on_change();
        }
        tokio::select! {
            () = tokio::time::sleep(interval) => {}
            () = &mut woken => {}
        }
    }
}

/// 生产查询：向 `addr`（canary 回环入站）发一次 A 查询。
/// NOERROR 且带答案 ⇒ `Some(true)`；NXDOMAIN ⇒ `Some(false)`；超时 / 发送失败 / 应答不对 ⇒ `None`（未知）。
pub(super) async fn query_canary(addr: SocketAddr, domain: String) -> Option<bool> {
    let socket = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .ok()?;
    let id = CANARY_QUERY_ID;
    socket.send_to(&dns_query(id, &domain), addr).await.ok()?;
    let mut buf = [0u8; 512];
    let len = tokio::time::timeout(CANARY_QUERY_TIMEOUT, socket.recv(&mut buf))
        .await
        .ok()?
        .ok()?;
    parse_canary_answer(id, &buf[..len])
}

/// 查询 id。每次查询用新的临时 socket，只收发给它的回包，固定 id 足够（仍校验，挡住畸形回包）。
const CANARY_QUERY_ID: u16 = 0x4e50;

/// 最小 DNS 查询报文：`id`、RD=1、一个问题 `<domain> IN A`。
fn dns_query(id: u16, domain: &str) -> Vec<u8> {
    let mut packet = Vec::with_capacity(domain.len() + 18);
    packet.extend_from_slice(&id.to_be_bytes());
    packet.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]);
    for label in domain.trim_end_matches('.').split('.') {
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
    }
    packet.extend_from_slice(&[0, 0, 1, 0, 1]);
    packet
}

/// 应答判读：id 对得上且是应答（QR=1）；rcode 0 且 ANCOUNT>0 ⇒ 命中，rcode 3 ⇒ 未命中，其余 ⇒ 未知。
fn parse_canary_answer(id: u16, reply: &[u8]) -> Option<bool> {
    if reply.len() < 12 || u16::from_be_bytes([reply[0], reply[1]]) != id || reply[2] & 0x80 == 0 {
        return None;
    }
    let answers = u16::from_be_bytes([reply[6], reply[7]]);
    match reply[3] & 0x0f {
        0 if answers > 0 => Some(true),
        3 => Some(false),
        _ => None,
    }
}

impl ProxyRuntime {
    /// 发命中态变更信号。未接线（单测 / setup 前）只记日志：发不出事件不影响探测本身。
    fn emit_network_profile_match_changed(&self) {
        match self.error_emitter.get() {
            Some(e) => e.emit_network_profile_match_changed(),
            None => log::debug!("emitter 未接线 → 跳过 networkProfileMatchChanged"),
        }
    }

    /// 起核就绪：换上本次运行核的 canary 表（`None` = 本次没生成），有表就挂探测任务。
    ///
    /// 任务随下一次 arm / disarm 退场（会话代）；另加世代守卫只拦信号 —— 被接管的核不该再替新核发声。
    pub(super) fn arm_network_canary(
        self: &Arc<Self>,
        my_gen: u64,
        plan: Option<NetworkCanaryPlan>,
    ) {
        let has_plan = plan.is_some();
        let (session, changed) = self.network_canary.arm(plan);
        if changed {
            self.emit_network_profile_match_changed();
        }
        if !has_plan {
            return;
        }
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let state = Arc::clone(&me.network_canary);
            run_canary_probe(&state, session, CANARY_PROBE_INTERVAL, query_canary, || {
                if me.gate.generation() == my_gen {
                    me.emit_network_profile_match_changed();
                }
            })
            .await;
            log::debug!("网络场景 canary 探测任务退场（会话 {session}）");
        });
    }

    /// 核停止 / 崩溃：命中态全部回到未知。
    pub(super) fn disarm_network_canary(&self) {
        if self.network_canary.disarm() {
            self.emit_network_profile_match_changed();
        }
    }

    /// 网络变化：先把命中态置为未知（不拿旧网络的结果冒充新网络），再由探测任务立即重探。
    pub(super) fn invalidate_network_canary(&self) {
        if self.network_canary.invalidate() {
            self.emit_network_profile_match_changed();
        }
    }

    /// 某场景当前的命中态（`network_profile_resolved_sources` 的 `matched`）。
    pub(super) fn network_profile_matched(&self, profile_id: &str) -> Option<bool> {
        self.network_canary.matched(profile_id)
    }
}

#[cfg(test)]
mod tests;
