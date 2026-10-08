//! 读回见证：一轮测速期间订一条内核的连接事件流，按「入站 tag + 本机源端口」记下每条测量连接
//! 实际由哪个出站承载，供测量层在发布结果前对账。
//!
//! - 一轮一条流，准入时建、随轮关闭。流绑定在准入时取到的那个管理端点上，不自动重连：核换代后
//!   它自己结束，不会连到新核。
//! - 流建不起来或中途断开不影响测量本身，只是其后的查询一律查不到记录。
//! - 记录在路由那一刻由内核建立，早于拨号：拨号失败的连接也有记录；被规则拒绝的连接没有。
//!
//! # 表里只有测量连接
//!
//! 测量 socket 一连上本机入站口、还没发出任何字节，就把自己的「入站 tag + 源端口」**登记**进来
//! （[`BindingWitness::expect`]）；内核要读到 CONNECT 之后才路由、才建记录，所以登记恒早于记录。
//! 流上来的连接只有对得上一条登记才进表，查到（或查询超时）即连同登记一起删。于是表的大小以
//! 在飞的测量数为界，与用户流量的强度无关，不需要按容量或时间淘汰。
//!
//! # 首帧的存量与其后的事件分开认
//!
//! - **流建立之后新到的事件**：键对得上登记就认，不比时刻。登记先于这条连接上的任何字节，
//!   而登记期间这个源端口被测量 socket 占着，同键的新事件只可能是它。
//! - **首帧里的存量**（内核当前的活动连接加保留的已关闭连接）：同一个源端口上一次使用留下的旧
//!   记录也在里面，要靠建立时刻区分，见 [`BACKLOG_CLOCK_TOLERANCE_MS`]。
//!
//! 对测量层只暴露 [`BindingWitness`] 这一个窄面，单测注入假实现。

use std::collections::HashMap;
use std::fmt::Display;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::{Stream, StreamExt};
use polaris_config_engine::builder::is_probe_pool_inbound_tag;
use polaris_singbox_grpc::daemon;
use polaris_singbox_grpc::tonic::Status;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio::time::Instant;

/// 一次查询最多等多久。它发生在测量出值之后，不计入测量值，也不占测量的两段预算。
///
/// 取值是估算，没有真机时延数据；连接事件的新增由内核即时推送，记录在第一次 GET 的路由那一刻
/// 就已建立，正常情况下查询时记录早已到达。
pub(crate) const READBACK_WAIT_MS: u64 = 300;

/// 起测前最多等见证流建立多久（只有一次测量的腿才等，见 [`BindingWitness::ready`]）。
/// 取值是估算：建流是回环上的一次连接加一次订阅。等待在测量开始之前，不计入测量值。
pub(crate) const READBACK_OPEN_WAIT_MS: u64 = 500;

/// 首帧存量里「建立时刻不早于登记」这条比较的容差。
///
/// 两个时刻不同源：登记取 App 的墙钟，建立时刻是内核的 `time.Now()`。Windows 上 Go 的墙钟按
/// 系统节拍走，粒度可到 15.6 毫秒；登记到内核建记录只隔回环上一次往返，不加容差时真记录会被
/// 判成旧的。取 250 毫秒：盖住十几个节拍加调度抖动；同时远小于一个源端口被同一目的端口再次
/// 用上所需的时间（主动关闭的一侧要过 TIME_WAIT，各平台都以秒计），不会把上一次使用的旧记录
/// 放进来。
pub(crate) const BACKLOG_CLOCK_TOLERANCE_MS: u64 = 250;

/// 读回用连接事件流的流量增量间隔（纳秒）。读回只看连接的新增，它们即时推送、不受这个值影响；
/// 取大值是为了不让内核为这条流白算流量增量。
pub(crate) const CONNECTION_EVENTS_INTERVAL_NS: i64 = 60_000_000_000;

/// 内核为一条连接建立的记录里，测量层用得到的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnRecord {
    pub(crate) inbound: String,
    /// 连接在本机一侧的源端口（测量 socket 的本地端口）。
    pub(crate) source_port: u16,
    /// 内核建这条记录的时刻（Unix 毫秒）。
    pub(crate) created_at: u64,
    /// 叶子出站：路由选中的出站沿 group 逐级取当前选择，解到底的那一个。
    pub(crate) outbound: String,
    pub(crate) outbound_type: String,
    /// 出站链，从叶子到路由选中的出站。
    pub(crate) chain: Vec<String>,
    /// 命中的路由规则描述；没有命中任何规则时为空。
    pub(crate) rule: String,
    pub(crate) destination: String,
    pub(crate) domain: String,
}

/// 一条腿的测量连接会经过的入站。别的入站上的连接在折帧时就丢掉，不进后面的对账。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WatchedInbounds {
    /// 探针池的各槽（`probe-in-k`）。
    ProbePool,
    /// 回退腿与出口伴测实际用的那一个本机代理入站。
    One(&'static str),
}

impl WatchedInbounds {
    fn contains(self, tag: &str) -> bool {
        match self {
            Self::ProbePool => is_probe_pool_inbound_tag(tag),
            Self::One(watched) => tag == watched,
        }
    }
}

/// 把一帧连接事件折成记录：带连接体、经 `watched` 入站、源地址解析得出端口的那些。
///
/// **已关闭的连接也留**：首帧里内核保留的已关闭连接可能正是一条先于首帧关掉的测量连接；内核的
/// 关闭事件同样带着完整的连接体。它是历史还是本轮的，由见证表按登记判（见模块文档），不在这里判。
pub(crate) fn records_of(
    frame: daemon::ConnectionEvents,
    watched: WatchedInbounds,
) -> Vec<ConnRecord> {
    frame
        .events
        .into_iter()
        .filter_map(|event| event.connection)
        .filter(|conn| watched.contains(&conn.inbound))
        .filter_map(|conn| {
            let source_port = conn.source.rsplit_once(':')?.1.parse().ok()?;
            Some(ConnRecord {
                inbound: conn.inbound,
                source_port,
                created_at: u64::try_from(conn.created_at).unwrap_or_default(),
                outbound: conn.outbound,
                outbound_type: conn.outbound_type,
                chain: conn.chain_list,
                rule: conn.rule,
                destination: conn.destination,
                domain: conn.domain,
            })
        })
        .collect()
}

/// 见证吃的一帧：折出来的记录，以及它是不是首帧的存量。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    /// 内核在订阅那一刻发的全量帧（`reset`）：当前的活动连接加保留的已关闭连接。其后的帧
    /// 都是订阅之后才发生的事件。
    pub(crate) backlog: bool,
    pub(crate) records: Vec<ConnRecord>,
}

/// 把内核的连接事件流接成见证吃的帧流。流报错即结束（不重连），错误的状态码与消息记一行
/// 日志；其后的查询查不到记录。
pub(crate) fn record_stream<S>(
    frames: S,
    watched: WatchedInbounds,
) -> impl Stream<Item = Frame> + Send + Unpin
where
    S: Stream<Item = Result<daemon::ConnectionEvents, Status>> + Send + 'static,
{
    Box::pin(frames.scan((), move |(), frame| {
        std::future::ready(match frame {
            Ok(frame) => Some(Frame {
                backlog: frame.reset,
                records: records_of(frame, watched),
            }),
            Err(status) => {
                // 状态码与消息来自内核；请求侧的凭据在请求头里，不在这两项里。
                log::warn!(
                    "测速读回：连接事件流报错 code={:?} message={}",
                    status.code(),
                    status.message()
                );
                None
            }
        })
    }))
}

/// 见证流的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WitnessState {
    /// 本轮不做读回。
    Off,
    /// 正在建流。
    Connecting,
    Established,
    /// 流没建起来。
    Failed,
    /// 建起来之后中途断开。
    Broken,
    /// 建起来过，已随轮正常关闭。
    Closed,
}

impl WitnessState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Connecting => "connecting",
            Self::Established => "established",
            Self::Failed => "failed",
            Self::Broken => "broken",
            Self::Closed => "closed",
        }
    }

    const fn code(self) -> u8 {
        self as u8
    }

    const fn of(code: u8) -> Self {
        match code {
            1 => Self::Connecting,
            2 => Self::Established,
            3 => Self::Failed,
            4 => Self::Broken,
            5 => Self::Closed,
            _ => Self::Off,
        }
    }
}

type Boxed<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 测量层对读回见证的全部依赖。
pub(crate) trait BindingWitness: Send + Sync {
    /// 等见证流建立（或确定建不起来），最多 [`READBACK_OPEN_WAIT_MS`]。
    fn ready(&self) -> Boxed<'_, ()>;

    /// 登记一条刚连上 `inbound` 入站口、源端口为 `source_port` 的测量连接。必须在这条连接上
    /// 发出任何字节之前调。
    fn expect(&self, inbound: &str, source_port: u16);

    /// 取登记过的那条连接的记录，并撤销登记。限时内没有即 `None`。
    fn lookup<'a>(&'a self, inbound: &'a str, source_port: u16) -> Boxed<'a, Option<ConnRecord>>;

    fn state(&self) -> WitnessState;

    /// 到目前为止单次查询等待的最大值（毫秒）。
    fn max_wait_ms(&self) -> u64;

    /// 关闭见证流并等它真正结束。返回之后不再有属于这一轮的流。
    fn close(&self) -> Boxed<'_, ()>;
}

/// 不做读回的见证：查询立即返回 `None`。
pub(crate) struct NoWitness;

impl BindingWitness for NoWitness {
    fn ready(&self) -> Boxed<'_, ()> {
        Box::pin(async {})
    }

    fn expect(&self, _: &str, _: u16) {}

    fn lookup<'a>(&'a self, _: &'a str, _: u16) -> Boxed<'a, Option<ConnRecord>> {
        Box::pin(async { None })
    }

    fn state(&self) -> WitnessState {
        WitnessState::Off
    }

    fn max_wait_ms(&self) -> u64 {
        0
    }

    fn close(&self) -> Boxed<'_, ()> {
        Box::pin(async {})
    }
}

type Key = (String, u16);

#[derive(Default)]
struct Table {
    /// 已登记、尚未查询的测量连接 → 登记时刻（Unix 毫秒）。
    expected: HashMap<Key, u64>,
    /// 对上了登记的记录。键集恒是 `expected` 键集的子集。
    records: HashMap<Key, ConnRecord>,
}

impl Table {
    /// 收一条流上来的记录：对得上登记才留。首帧的存量另要求建立时刻不早于登记（带容差）：
    /// 早于登记的是同一个源端口上一次使用留下的历史。
    fn offer(&mut self, record: ConnRecord, backlog: bool) -> bool {
        let key = (record.inbound.clone(), record.source_port);
        let Some(expected_at) = self.expected.get(&key) else {
            return false;
        };
        if backlog && record.created_at.saturating_add(BACKLOG_CLOCK_TOLERANCE_MS) < *expected_at {
            return false;
        }
        self.records.insert(key, record);
        true
    }
}

struct Shared {
    table: Mutex<Table>,
    state: AtomicU8,
    /// 有新记录进表，或流的状态变了。
    changed: Notify,
    max_wait_ms: AtomicU64,
    now_ms: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl Shared {
    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_state(&self, state: WitnessState) {
        self.state.store(state.code(), Ordering::Release);
        self.changed.notify_waiters();
    }

    fn state(&self) -> WitnessState {
        WitnessState::of(self.state.load(Ordering::Acquire))
    }

    /// 等到 `done` 给出值或 `deadline` 到期。
    async fn wait_until<T>(&self, deadline: Instant, done: impl Fn() -> Option<T>) -> Option<T> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            // 先登记再看状态：反过来会漏掉两步之间到达的那次通知。
            changed.as_mut().enable();
            if let Some(value) = done() {
                return Some(value);
            }
            if tokio::time::timeout_at(deadline, changed).await.is_err() {
                return None;
            }
        }
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// 生产用的见证：后台任务读一条连接事件流，把对得上登记的记录放进表里。
pub(crate) struct StreamWitness {
    shared: Arc<Shared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl StreamWitness {
    /// 起后台任务。`open` 建流（建流本身不阻塞调用方）。
    pub(crate) fn spawn<Open, S, E>(open: Open) -> Self
    where
        Open: Future<Output = Result<S, E>> + Send + 'static,
        S: Stream<Item = Frame> + Send + Unpin + 'static,
        E: Display,
    {
        Self::spawn_with_clock(open, unix_ms)
    }

    /// [`spawn`](Self::spawn) 的本体：登记时刻用的墙钟可注入。
    pub(crate) fn spawn_with_clock<Open, S, E>(
        open: Open,
        now_ms: impl Fn() -> u64 + Send + Sync + 'static,
    ) -> Self
    where
        Open: Future<Output = Result<S, E>> + Send + 'static,
        S: Stream<Item = Frame> + Send + Unpin + 'static,
        E: Display,
    {
        let shared = Arc::new(Shared {
            table: Mutex::new(Table::default()),
            state: AtomicU8::new(WitnessState::Connecting.code()),
            changed: Notify::new(),
            max_wait_ms: AtomicU64::new(0),
            now_ms: Box::new(now_ms),
        });
        let task = tokio::spawn({
            let shared = Arc::clone(&shared);
            async move {
                let mut stream = match open.await {
                    Ok(stream) => stream,
                    Err(error) => {
                        log::warn!(
                            "测速读回：连接事件流建立失败，本轮结果的承载出站一律记为未验证：{error}"
                        );
                        shared.set_state(WitnessState::Failed);
                        return;
                    }
                };
                shared.set_state(WitnessState::Established);
                while let Some(Frame { backlog, records }) = stream.next().await {
                    let kept = {
                        let mut table = shared.table();
                        let mut kept = false;
                        // 每条都要过一遍：不能在第一条留下之后短路。
                        for record in records {
                            kept |= table.offer(record, backlog);
                        }
                        kept
                    };
                    if kept {
                        shared.changed.notify_waiters();
                    }
                }
                log::warn!("测速读回：连接事件流中途断开，其后结果的承载出站记为未验证");
                shared.set_state(WitnessState::Broken);
            }
        });
        Self {
            shared,
            task: Mutex::new(Some(task)),
        }
    }

    fn take_task(&self) -> Option<JoinHandle<()>> {
        self.task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    /// 表里现有的登记数与记录数。
    #[cfg(test)]
    pub(crate) fn table_len(&self) -> (usize, usize) {
        let table = self.shared.table();
        (table.expected.len(), table.records.len())
    }
}

impl BindingWitness for StreamWitness {
    fn ready(&self) -> Boxed<'_, ()> {
        Box::pin(async move {
            let deadline = Instant::now() + Duration::from_millis(READBACK_OPEN_WAIT_MS);
            self.shared
                .wait_until(deadline, || {
                    (self.shared.state() != WitnessState::Connecting).then_some(())
                })
                .await;
        })
    }

    fn expect(&self, inbound: &str, source_port: u16) {
        let now = (self.shared.now_ms)();
        self.shared
            .table()
            .expected
            .insert((inbound.to_string(), source_port), now);
    }

    fn lookup<'a>(&'a self, inbound: &'a str, source_port: u16) -> Boxed<'a, Option<ConnRecord>> {
        Box::pin(async move {
            let started = Instant::now();
            let deadline = started + Duration::from_millis(READBACK_WAIT_MS);
            let key = (inbound.to_string(), source_port);
            let found = self
                .shared
                .wait_until(deadline, || {
                    if let Some(record) = self.shared.table().records.remove(&key) {
                        return Some(Some(record));
                    }
                    // 流已经不在了：再等也不会有记录。
                    (!matches!(
                        self.shared.state(),
                        WitnessState::Connecting | WitnessState::Established
                    ))
                    .then_some(None)
                })
                .await
                .flatten();
            // 查过即撤销登记：其后再到的同键记录不属于任何在飞的测量。
            {
                let mut table = self.shared.table();
                table.expected.remove(&key);
                table.records.remove(&key);
            }
            let waited = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.shared.max_wait_ms.fetch_max(waited, Ordering::AcqRel);
            found
        })
    }

    fn state(&self) -> WitnessState {
        self.shared.state()
    }

    fn max_wait_ms(&self) -> u64 {
        self.shared.max_wait_ms.load(Ordering::Acquire)
    }

    fn close(&self) -> Boxed<'_, ()> {
        Box::pin(async move {
            if let Some(task) = self.take_task() {
                task.abort();
                // 等任务真正结束：它一结束，流对象随之析构。
                let _ = task.await;
            }
            // 「没建起来」与「中途断开」留着不盖：汇总行要报的是这条流这一轮过得怎么样。
            match self.shared.state() {
                WitnessState::Established => self.shared.set_state(WitnessState::Closed),
                WitnessState::Connecting => self.shared.set_state(WitnessState::Failed),
                _ => {}
            }
        })
    }
}

impl Drop for StreamWitness {
    fn drop(&mut self) {
        if let Some(task) = self.take_task() {
            task.abort();
        }
    }
}
