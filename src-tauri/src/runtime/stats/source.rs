//! 两条长驻流的**传输层**，以及它唯一的平台分叉。
//!
//! `runtime/stats` 的其余部分（活动连接表、已结束历史环、四条 emit 闸门、签名去重、订阅门、
//! 五条渲染层事件）在两个平台上是**同一份代码**。会分叉的只有「帧从哪来」：
//!
//! | | 桌面 | Android |
//! |---|---|---|
//! | 传输 | clash 管理 API 上的 daemon gRPC（loopback HTTP/2 + secret） | libbox `CommandClient`（**同进程** unix socket，无端口无 secret） |
//! | 建流 | `SingBoxApiClient::connect` + `SubscribeConnections` / `SubscribeStatus` | Kotlin `PolarisVpnPlugin` 的 `connectionsOpen` / `statsOpen` |
//! | 收帧 | `ReconnectingStream::recv` | `connectionsPoll` / `statsPoll` 长轮询（游标交接） |
//! | 帧形状 | `daemon::ConnectionEvents` / `daemon::Status` | 逐字段同形的 JSON（见 `BridgeConnection` 的头注） |
//!
//! 两侧的帧最后都变成同一对纯逻辑类型（`SingBoxConnectionEvents` / `SingBoxStatus`），
//! 于是 `relay.rs` 的循环体两个平台共用一份 —— 「Android 上重新发明一套数据面」这件事从形态上
//! 就不成立。
//!
//! # 🔴 Android 的数据面**不经过管理 API**
//!
//! libbox 的命令服务跑在**本进程内**，用的是 app 私有目录下的 unix socket：没有端口、没有
//! `clashApiSecret`、没有任何回环 TCP 监听。故本模块的 Android 腿一次都不读
//! `ProxyStatus::clash_api_port`，也一次都不读 `clashApiSecret`（`read_clash_secret` 整个函数
//! 是 `cfg(not(android))` 的）。
//!
//! 这是 handoff §4 U1 那条未决裁定（「Android 上管理 API 仍开在共享 loopback、`has_management_api`
//! 无平台门控」）**收口时的一块拼图**：连接列表与流量统计曾是「管理 API 在 Android 上还有谁在用」
//! 的答案之一，现在不是了。**本批不动 `has_management_api`、不关管理 API** —— 那是产品裁定，
//! 且就绪门的 TCP 探测、测速回退、订阅更新等仍挂在它上面。这里只登记：数据面这一条依赖没了。
//!
//! # 为什么 Android 是「Rust 拉」不是「Kotlin 推」
//!
//! 见 `StatsBridge.kt` 的类文档（同一个判断的两面）。一句话：推要直接依赖 `jni` crate，而
//! lock 里同时有 0.21.1 与 0.22.4 两个传递版本，挑错是链接期才炸；拉复用 K4 已定稿的插件命令面，
//! 零新增依赖，且现成的契约门直接覆盖新命令。
//!
//! # 🔴 桥的 future **绝不允许被丢弃**
//!
//! `PluginHandle::run_mobile_plugin_async` 的实现里，Kotlin 回执经
//! `tx.send(response).unwrap()` 交给 future（`tauri-2.11.5/src/plugin/mobile.rs:307`）——
//! 若 future 已被丢弃，那个 `unwrap` 在 JNI 回调线程上 **panic**。故：
//!
//! - 本模块的每个桥调用都跑在**专属的 pump 任务**里，一路 `await` 到底，**不套 `select!`、
//!   不套 `tokio::time::timeout`**（后者超时即丢弃 future，同样触发）；
//! - relay 循环拿到的是 [`tokio::sync::mpsc::Receiver`]，`recv()` 本身 cancel-safe，
//!   于是 `relay.rs` 的 `select!` 形态两个平台一字不改；
//! - 「桥不回应」由 **Kotlin 侧**封顶（`MAX_WAIT_MS` 900ms 必回一帧，可能是空批），
//!   而不是由 Rust 侧超时。超时判据搬到了不会丢弃 future 的那一侧。

use polaris_stats_engine::{SingBoxConnectionEvents, SingBoxStatus};

use crate::runtime::proxy::ProxyStatus;

#[cfg(not(target_os = "android"))]
use polaris_singbox_grpc::{
    daemon, Endpoint, ReconnectConfig, ReconnectingStream, SingBoxApiClient,
};
#[cfg(not(target_os = "android"))]
use serde_json::Value;

use crate::runtime::config::ConfigManager;

// ══════════════════════════════════════════════════════════════════════════════
// 通道契约：Kotlin 订阅的 libbox 通道 ⇄ 前端消费的五条 topic
//
// 这两组常量是**给人和给门看的声明**，不是运行期开关：Kotlin 侧的
// `StatsBridge.statsStream` / `connectionsStream` 各自 `addCommand` 哪个通道，与这里逐字对拍
// （`scripts/check-android-bridge.mjs` 的 A6）。摘掉 Kotlin 侧任意一个 `addCommand`，
// 门会红在**那一条通道所供的 topic 上**，而不是笼统地说「桥不一致」。
// ══════════════════════════════════════════════════════════════════════════════

/// `stats` topic 在 Android 上的供数通道（Kotlin：`options.addCommand(Libbox.CommandStatus)`）。
#[cfg(any(target_os = "android", test))]
pub(super) const STATS_LIBBOX_COMMAND: &str = "CommandStatus";

/// 四条连接需求在 Android 上的供数通道（Kotlin：`options.addCommand(Libbox.CommandConnections)`）。
#[cfg(any(target_os = "android", test))]
pub(super) const CONNECTIONS_LIBBOX_COMMAND: &str = "CommandConnections";

/// 前端五条 topic（`ui/src/domain/ipc-channels.ts` 的 `STATS_TOPIC_EVENT`）→ 供数通道。
///
/// **少一条 = 某个屏在 Android 上没有数据源**，而那件事今天没有任何编译期证据：
/// 五条 topic 的 emit 都在共享的 relay 里，Android 侧只要少订一个 libbox 通道，
/// 对应的屏就安静地一直空着。故把对应关系写成声明，由契约门 A6 双向对拍。
#[cfg(any(target_os = "android", test))]
pub(super) const TOPIC_SOURCE: &[(&str, &str)] = &[
    ("stats", STATS_LIBBOX_COMMAND),
    ("aggregate", CONNECTIONS_LIBBOX_COMMAND),
    ("topology", CONNECTIONS_LIBBOX_COMMAND),
    ("detail", CONNECTIONS_LIBBOX_COMMAND),
    ("closed", CONNECTIONS_LIBBOX_COMMAND),
];

// ══════════════════════════════════════════════════════════════════════════════
// 平台无关的判据面（三条谓词）
// ══════════════════════════════════════════════════════════════════════════════

/// 核是否处在「可以建数据流」的状态。
///
/// 桌面还要求管理口已知（`clash_api_port != 0`）—— 那是 gRPC 的落点。
/// Android 上没有这个落点：libbox 命令服务随内核一起在本进程内起来，核在即可订。
pub(super) fn stream_ready(status: &ProxyStatus) -> bool {
    #[cfg(not(target_os = "android"))]
    {
        status.running && status.clash_api_port != 0
    }
    #[cfg(target_os = "android")]
    {
        status.running
    }
}

/// 建流那一刻的「流身份」。桌面 = 管理口端口号；Android 无此概念，恒 0（**只用于日志**，
/// 不参与任何判据，见 [`stream_still_valid`]）。
pub(super) fn stream_port(status: &ProxyStatus) -> u16 {
    #[cfg(not(target_os = "android"))]
    {
        status.clash_api_port
    }
    #[cfg(target_os = "android")]
    {
        let _ = status;
        0
    }
}

/// 已建的流是否仍指向同一个核。
///
/// 桌面：换核 / 重启动态口会换管理口，端口一变即断流重来（`ReconnectingStream` 自己发现不了）。
/// Android：核在进程内，换核必然经过一次停核（`VpnBridge.finishStop` 清 `CORE_STARTED`），
/// 故只看 `running`；而 libbox 那一侧真断了会经 `disconnected()` 把流标成 closed → `recv` 返 None。
pub(super) fn stream_still_valid(status: &ProxyStatus, port: u16) -> bool {
    #[cfg(not(target_os = "android"))]
    {
        status.running && status.clash_api_port == port
    }
    #[cfg(target_os = "android")]
    {
        let _ = port;
        status.running
    }
}

/// 一条已订阅数据流是否仍属于当前运行核。`running` 在停核桥回执前仍为 true，
/// 因而还须核对起停世代与生命周期事务；旧流的关闭尾帧不能进入聚合或缺能力告警。
pub(super) fn stream_session_current(
    status: &ProxyStatus,
    port: u16,
    stream_generation: u64,
    current_generation: u64,
    lifecycle_busy: bool,
) -> bool {
    stream_generation == current_generation && !lifecycle_busy && stream_still_valid(status, port)
}

/// 建流日志里的流标识（桌面 `port=NNNN`；Android 上没有端口这回事，别印一个假的 0）。
pub(super) fn stream_label(port: u16) -> String {
    #[cfg(not(target_os = "android"))]
    {
        format!("port={port}")
    }
    #[cfg(target_os = "android")]
    {
        let _ = port;
        "libbox 命令通道".to_string()
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 桌面腿：daemon gRPC
// ══════════════════════════════════════════════════════════════════════════════

/// currentConfig.clashApiSecret（对齐 proxy.rs `management_api()` 的读法）。
///
/// **整个函数 `cfg(not(android))`**：Android 的数据面不经过管理 API，也就没有 secret 可读。
/// 留一个恒返空串的跨平台版本，等于给「Android 也在读管理 API」留了一处看起来无害的接线。
#[cfg(not(target_os = "android"))]
fn read_clash_secret(config: &ConfigManager) -> String {
    config
        .current()
        .ok()
        .and_then(|c| {
            c.get("clashApiSecret")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// 连接事件长驻流（平台各自的传输，同一种帧）。
pub(super) struct ConnectionsStream {
    #[cfg(not(target_os = "android"))]
    inner: ReconnectingStream<daemon::ConnectionEvents>,
    #[cfg(target_os = "android")]
    inner: android::Pump<SingBoxConnectionEvents>,
}

/// Status 长驻流（同上）。
pub(super) struct StatusStream {
    #[cfg(not(target_os = "android"))]
    inner: ReconnectingStream<daemon::Status>,
    #[cfg(target_os = "android")]
    inner: android::Pump<SingBoxStatus>,
}

impl ConnectionsStream {
    /// 下一帧；`None` = 流终止（桌面：`ReconnectingStream` 内部终止；Android：桥断/溢出）→ 重建。
    pub(super) async fn recv(&mut self) -> Option<SingBoxConnectionEvents> {
        #[cfg(not(target_os = "android"))]
        {
            let ev = self.inner.recv().await?;
            Some(super::relay::daemon_events_to_engine(&ev))
        }
        #[cfg(target_os = "android")]
        {
            self.inner.recv().await
        }
    }
}

impl StatusStream {
    /// 下一帧；`None` 的语义同 [`ConnectionsStream::recv`]。
    pub(super) async fn recv(&mut self) -> Option<SingBoxStatus> {
        #[cfg(not(target_os = "android"))]
        {
            let st = self.inner.recv().await?;
            Some(super::relay::daemon_status_to_engine(&st))
        }
        #[cfg(target_os = "android")]
        {
            self.inner.recv().await
        }
    }
}

/// 订阅连接事件流。`None` = 这一轮建不起来（调用方退避一拍重试，与桌面既有腿同语义）。
pub(super) async fn subscribe_connections(
    config: &ConfigManager,
    port: u16,
) -> Option<ConnectionsStream> {
    #[cfg(not(target_os = "android"))]
    {
        let secret = read_clash_secret(config);
        let client = match SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret).await
        {
            Ok(c) => c,
            Err(e) => {
                log::debug!("连接流：管理 API 连接失败 {e}");
                return None;
            }
        };
        Some(ConnectionsStream {
            inner: client.subscribe_connections(
                super::CONNECTIONS_STREAM_INTERVAL_NS,
                ReconnectConfig::default(),
            ),
        })
    }
    #[cfg(target_os = "android")]
    {
        let _ = (config, port);
        android::open_connections()
            .await
            .map(|inner| ConnectionsStream { inner })
    }
}

/// 订阅 Status 流。`None` 的语义同 [`subscribe_connections`]。
pub(super) async fn subscribe_status(config: &ConfigManager, port: u16) -> Option<StatusStream> {
    #[cfg(not(target_os = "android"))]
    {
        let secret = read_clash_secret(config);
        let client = match SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), secret).await
        {
            Ok(c) => c,
            Err(e) => {
                log::debug!("Status 流：管理 API 连接失败 {e}");
                return None;
            }
        };
        Some(StatusStream {
            inner: client
                .subscribe_status(super::STATS_STREAM_INTERVAL_NS, ReconnectConfig::default()),
        })
    }
    #[cfg(target_os = "android")]
    {
        let _ = (config, port);
        android::open_status()
            .await
            .map(|inner| StatusStream { inner })
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Android 腿：libbox CommandClient（经 K4 的 Tauri plugin 命令面）
// ══════════════════════════════════════════════════════════════════════════════

/// Keep the entire native handshake alive when its relay is aborted. The guard is
/// transferred to the pump, so an old close cannot tear down a newer subscription.
#[cfg(any(target_os = "android", test))]
async fn detached_open<T, F, Fut>(lock: &'static tokio::sync::Mutex<()>, open: F) -> Option<T>
where
    T: Send + 'static,
    F: FnOnce(tokio::sync::MutexGuard<'static, ()>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Option<T>> + Send + 'static,
{
    let (ready, result) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let guard = lock.lock().await;
        if ready.is_closed() {
            return;
        }
        // Once native work begins, await every callback even if the caller leaves.
        let stream = open(guard).await;
        // Failed delivery drops the pump, triggering its normal cooperative close.
        let _ = ready.send(stream);
    });
    result.await.ok().flatten()
}

#[cfg(target_os = "android")]
mod android {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use polaris_stats_engine::{
        ConnectionEventType, SingBoxConnection, SingBoxConnectionEvent, SingBoxConnectionEvents,
        SingBoxProcessInfo, SingBoxStatus,
    };
    use tokio::sync::{mpsc, Mutex, MutexGuard};

    static CONNECTION_STREAM_LOCK: Mutex<()> = Mutex::const_new(());
    static STATUS_STREAM_LOCK: Mutex<()> = Mutex::const_new(());

    use crate::runtime::proxy::android_bridge;

    /// 帧队列深度。Kotlin 侧一帧一秒、这边即取即喂，常态深度 0–1；给 64 是让一次积压
    /// （relay 正忙于一次大聚合）不至于把 pump 卡在 `send` 上。
    const CHANNEL_DEPTH: usize = 64;

    /// 「建流成功但一帧都没来」多久算异常（单条流生命周期内）。
    const SILENT_STREAM_WARN: Duration = Duration::from_secs(5);

    /// 连续多少条**空手而归**的流生命周期算异常（跨代累计）。
    ///
    /// 单代 5s 的判据在一种真实工况下会漏报：内核处在起停循环时，每条流活不到 5 秒就被
    /// `BoxService.closeAll()` 收掉（本轮实测就是这样）。而「一直没数据」恰恰是那种时候最该喊的。
    /// 故再加一条跨代计数：连续 5 条流一帧未收 ⇒ 喊。两条判据的失效面互补，缺一条都留窗口。
    const SILENT_STREAK_WARN: u32 = 5;

    /// 心跳日志的间隔（帧数）。1s 一帧 ⇒ 半分钟一行。
    const HEARTBEAT_EVERY: u64 = 30;

    /// 一条 pump 的可观测性记账。
    ///
    /// # 🔴 它守的是本条链路上**唯一**不可见的失效形态
    ///
    /// 「订阅根本没接上」与「此刻真的零连接、零流量」在界面上逐像素相同：空列表 + 0 B/s +
    /// 零报错。日志里默认也一样 —— 都是「什么都没发生」。而这两件事的下一步动作完全相反。
    ///
    /// 判据取「建流后 [`SILENT_STREAM_WARN`] 内一帧都没有」：libbox 两条通道都在订阅当刻推首帧
    /// （Status 每 tick 必推；Connections 首帧是 `reset=true` 全量表，**表空也推**），
    /// 故「一帧都没有」不可能是「没有数据」。反向对照同样在这里：真的零连接时，首帧照到，
    /// 心跳照走，只是内容是 0 —— 两种形态在日志里长得不一样。
    struct PumpLog {
        name: &'static str,
        since: Instant,
        frames: u64,
        warned: bool,
        /// 该条流所属通道的**跨代**空手计数（见 [`SILENT_STREAK_WARN`]）。
        streak: &'static AtomicU32,
    }

    impl PumpLog {
        fn new(name: &'static str, streak: &'static AtomicU32) -> Self {
            Self {
                name,
                since: Instant::now(),
                frames: 0,
                warned: false,
                streak,
            }
        }

        /// 记一批帧。首帧与每 [`HEARTBEAT_EVERY`] 帧各报一次内容摘要。
        fn note(&mut self, count: usize, detail: impl FnOnce() -> String) {
            if count == 0 {
                return;
            }
            let before = self.frames;
            self.frames += count as u64;
            if before == 0 {
                // 跨代计数在**拿到第一帧那一刻**就归零，不等 [`Self::retire`]：pump 有一条
                // 「消费方已走 ⇒ 直接 return」的早退路径不经过 retire，把归零押在退场上，
                // 那条路径会让一次**成功**的生命周期在计数里留下一笔账。
                self.streak.store(0, Ordering::SeqCst);
                log::info!("{}：libbox 首帧已到（{}）", self.name, detail());
            } else if before / HEARTBEAT_EVERY != self.frames / HEARTBEAT_EVERY {
                log::debug!("{}：已收 {} 帧（{}）", self.name, self.frames, detail());
            }
        }

        /// 静默自曝（见结构体文档）。每条流最多喊一次。
        fn check_silence(&mut self) {
            if self.frames > 0 || self.warned || self.since.elapsed() < SILENT_STREAM_WARN {
                return;
            }
            self.warned = true;
            log::warn!(
                "{}：建流已 {}s，libbox 一帧都没推过来。这**不是**「当前没有数据」——\
                 空表也会有首帧 —— 而是订阅面没接上（Kotlin 侧 addCommand 缺失，或回调没把帧落进缓冲）。\
                 本流供数的界面会一直空着且不报任何错。",
                self.name,
                self.since.elapsed().as_secs()
            );
        }

        /// 本条流退场时结账跨代计数（见 [`SILENT_STREAK_WARN`]）。
        fn retire(&self) {
            if self.frames > 0 {
                self.streak.store(0, Ordering::SeqCst);
                return;
            }
            note_empty_generation(self.name, self.streak);
        }
    }

    /// 一次「建流之后一帧都没拿到」的结账（含**建流即作废**那条路）。
    ///
    /// 🔴 建流失败也必须计数，这是本轮实测补上的：摘掉 Kotlin 侧 `addCommand` 之后，握手取帧会
    /// 一直等到 900ms 封顶，而内核起停循环里那 900ms 内往往正好被 `closeAll()` 收流 ⇒ 每次都走
    /// 「建流即作废」早退、pump 根本没起来 ⇒ 只在 pump 里记账的话，**订阅没接上这件事一次都不会
    /// 被喊出来**。判据要盖住的是「本流从没拿到过数据」，而不是「pump 跑过但空手」。
    fn note_empty_generation(name: &str, streak: &'static AtomicU32) {
        let n = streak.fetch_add(1, Ordering::SeqCst) + 1;
        if n == SILENT_STREAK_WARN {
            log::warn!(
                "{name}：连续 {n} 次建流都没拿到任何帧。两种成因都会让本流供数的界面一直空着\
                 且不报任何错：① 订阅面没接上（Kotlin 侧 addCommand 缺失，或回调没把帧落进缓冲）；\
                 ② 内核根本没稳定运行过（起停循环）。注意这**不是**「当前没有数据」—— 空表也会有首帧。"
            );
        }
    }

    /// 两条通道各自的跨代空手计数（`retire` 结账）。
    static CONNECTIONS_EMPTY_STREAK: AtomicU32 = AtomicU32::new(0);
    static STATUS_EMPTY_STREAK: AtomicU32 = AtomicU32::new(0);

    /// 一条流的 Rust 半边：pump 任务 + 帧通道。
    ///
    /// `Drop` 只翻停止位 —— pump 会在**当前这次桥往返走完之后**退出并调 `*Close`。
    /// 不在 `Drop` 里 abort：abort 会丢弃在飞的桥 future，而丢弃它正是那条 panic 路径（见模块文档）。
    pub(super) struct Pump<T> {
        rx: mpsc::Receiver<T>,
        stop: Arc<AtomicBool>,
    }

    impl<T> Pump<T> {
        pub(super) async fn recv(&mut self) -> Option<T> {
            self.rx.recv().await
        }
    }

    impl<T> Drop for Pump<T> {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    /// 桥的取帧请求。`after` 的语义（以及它为什么是承重的）见 Kotlin 侧 `PollArgs`。
    #[derive(serde::Serialize)]
    #[serde(rename_all = "camelCase")]
    struct PollArgs {
        after: u64,
    }

    /// 一次取帧的回包（两条流同形，只有 `frames` 的元素类型不同）。
    ///
    /// 两个具体化别名（[`ConnectionsBatch`] / [`StatusBatch`]）不是装饰：契约门的 A1 正则
    /// 只认 `run_mobile_plugin_async::<[^>]*>(`，调用点写成 `::<Batch<BridgeStatus>>` 会因为
    /// 内层的 `>` 匹配不上而漏掉那条命令。漏掉的失效方式是**响亮**的（A5 FLOOR 立刻转红，
    /// 本批实测过一次），但没有理由把它留给下一个人。
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Batch<T> {
        /// 本批最后一帧的序号；空批时原样回传请求里的 `after`。
        seq: u64,
        /// 真 = 这条流已废（对端断开 / Kotlin 侧缓冲溢出），调用方须重订阅。
        #[serde(default)]
        closed: bool,
        /// 作废的因由；`org.json` 的 `put(key, null)` 会删键，故这里必须能缺席。
        #[serde(default)]
        reason: Option<String>,
        /// **刻意不带 `serde(default)`**：Kotlin 侧每次回包都放 `frames`（空批就是空数组），
        /// 给它一个默认空 vec 会把「Kotlin 侧回包结构变了」静默兑成「这一批没有帧」。
        /// 另外 `#[serde(default)]` 在泛型字段上会给 `T` 加一条 `Default` 约束（实测编译期报错），
        /// 而帧类型本就不该有「默认帧」这种东西。
        frames: Vec<T>,
    }

    /// 连接流的回包（A1 正则要一个不含嵌套泛型的具体类型名，见 [`Batch`] 的头注）。
    type ConnectionsBatch = Batch<BridgeConnectionEvents>;

    /// Status 流的回包（同上）。
    type StatusBatch = Batch<BridgeStatus>;

    /// libbox `StatusMessage` 的九个字段，与 proto `message Status` 同形。
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BridgeStatus {
        memory: u64,
        goroutines: i32,
        connections_in: i32,
        connections_out: i32,
        traffic_available: bool,
        uplink: i64,
        downlink: i64,
        uplink_total: i64,
        downlink_total: i64,
    }

    /// libbox `ConnectionEvents` 帧。
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BridgeConnectionEvents {
        reset: bool,
        events: Vec<BridgeConnectionEvent>,
    }

    /// libbox `ConnectionEvent`。`kind` 取 libbox 的 `ConnectionEventNew/Update/Closed`（0/1/2），
    /// 与 proto `enum ConnectionEventType` 同值。
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BridgeConnectionEvent {
        kind: i32,
        id: String,
        connection: Option<BridgeConnection>,
        uplink_delta: i64,
        downlink_delta: i64,
        closed_at: i64,
    }

    /// libbox `Connection` 的**桌面同款子集**。
    ///
    /// 🔴 字段集与 `relay.rs::daemon_conn_to_engine` 逐字一致，一个不多一个不少
    /// （契约门 A9 双向对拍，A8 再对拍 Kotlin 编码器那一侧）。桌面那段明写「刻意不加字段：
    /// 这里映射哪些字段决定了 aggregate / detail 的输出」—— 在 Android 上多映一个，就是让同一个
    /// 界面在两个平台显示不同的东西。
    ///
    /// **无一字段带 `serde(default)`**：Kotlin 少发一个键 ⇒ 反序列化当场报错、整条流转红，
    /// 而不是安静地填一个看起来对的零。
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BridgeConnection {
        id: String,
        inbound: String,
        inbound_type: String,
        network: String,
        source: String,
        destination: String,
        domain: String,
        created_at: i64,
        closed_at: i64,
        uplink_total: i64,
        downlink_total: i64,
        rule: String,
        chain_list: Vec<String>,
        process_info: BridgeProcessInfo,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct BridgeProcessInfo {
        process_path: String,
    }

    impl From<BridgeStatus> for SingBoxStatus {
        fn from(s: BridgeStatus) -> Self {
            SingBoxStatus {
                memory: s.memory,
                goroutines: s.goroutines,
                connections_in: s.connections_in,
                connections_out: s.connections_out,
                traffic_available: s.traffic_available,
                uplink: s.uplink,
                downlink: s.downlink,
                uplink_total: s.uplink_total,
                downlink_total: s.downlink_total,
            }
        }
    }

    impl From<BridgeConnection> for SingBoxConnection {
        fn from(c: BridgeConnection) -> Self {
            SingBoxConnection {
                id: c.id,
                inbound: c.inbound,
                inbound_type: c.inbound_type,
                network: c.network,
                source: c.source,
                destination: c.destination,
                domain: c.domain,
                created_at: c.created_at,
                closed_at: c.closed_at,
                uplink_total: c.uplink_total,
                downlink_total: c.downlink_total,
                rule: c.rule,
                chain_list: c.chain_list,
                process_info: SingBoxProcessInfo {
                    process_path: c.process_info.process_path,
                    ..Default::default()
                },
                ..Default::default()
            }
        }
    }

    impl From<BridgeConnectionEvents> for SingBoxConnectionEvents {
        fn from(ev: BridgeConnectionEvents) -> Self {
            SingBoxConnectionEvents {
                reset: ev.reset,
                events: ev
                    .events
                    .into_iter()
                    .map(|e| SingBoxConnectionEvent {
                        // 未知值兜底成 `New`，与 `relay.rs::daemon_events_to_engine` 同一条理由
                        // （当 NEW 处理最多多一条连接，当 CLOSED 处理会误删一条活连接）。
                        kind: match e.kind {
                            1 => ConnectionEventType::Update,
                            2 => ConnectionEventType::Closed,
                            _ => ConnectionEventType::New,
                        },
                        id: e.id,
                        connection: e.connection.map(Into::into),
                        uplink_delta: e.uplink_delta,
                        downlink_delta: e.downlink_delta,
                        closed_at: e.closed_at,
                    })
                    .collect(),
            }
        }
    }

    /// 收连接流。
    ///
    /// 两条腿各一个函数而不是「一个函数 + 命令名参数」：后者要么把命令名做成变量（契约门 A1 的
    /// 正则只认字面量，那一条命令会从对拍面里消失），要么在函数里按串 `match` 再选字面量 ——
    /// 而 `match` 的兜底臂会让一个拼错的串**静默去收另一条流**。两个三行函数没有这个失效面。
    async fn close_connections_stream(plugin: &'static tauri::plugin::PluginHandle<tauri::Wry>) {
        if let Err(e) = plugin
            .run_mobile_plugin_async::<()>("connectionsClose", ())
            .await
        {
            log::debug!("连接流：收流失败 {e}");
        }
    }

    /// 收 Status 流（理由同 [`close_connections_stream`]）。
    async fn close_status_stream(plugin: &'static tauri::plugin::PluginHandle<tauri::Wry>) {
        if let Err(e) = plugin.run_mobile_plugin_async::<()>("statsClose", ()).await {
            log::debug!("Status 流：收流失败 {e}");
        }
    }

    /// 心跳/首帧日志里的连接帧摘要。
    fn describe_connections(frames: &[BridgeConnectionEvents]) -> String {
        let last = frames.last();
        format!(
            "reset={} 本帧事件数={}",
            last.map(|f| f.reset).unwrap_or(false),
            last.map(|f| f.events.len()).unwrap_or(0),
        )
    }

    /// 心跳/首帧日志里的 Status 帧摘要。
    fn describe_status(frames: &[BridgeStatus]) -> String {
        let last = frames.last();
        format!(
            "活连接={} 累计上行={} 累计下行={} trafficAvailable={}",
            last.map(|f| f.connections_in).unwrap_or(0),
            last.map(|f| f.uplink_total).unwrap_or(0),
            last.map(|f| f.downlink_total).unwrap_or(0),
            last.map(|f| f.traffic_available).unwrap_or(false),
        )
    }

    pub(super) async fn open_connections() -> Option<Pump<SingBoxConnectionEvents>> {
        super::detached_open(&CONNECTION_STREAM_LOCK, open_connections_inner).await
    }

    async fn open_connections_inner(
        guard: MutexGuard<'static, ()>,
    ) -> Option<Pump<SingBoxConnectionEvents>> {
        let plugin = android_bridge::plugin()?;
        if let Err(e) = plugin
            .run_mobile_plugin_async::<()>("connectionsOpen", ())
            .await
        {
            log::debug!("连接流：libbox 命令通道建流失败 {e}");
            return None;
        }
        // 🔴 建流握手：`*Open` 之后**立刻**取一次帧，作废就当建流失败。
        //
        // Android 上 `stream_ready` 只能问桥的记账位（`CORE_STARTED` 是「起核回执」不是存活探测，
        // 射程见 `android_bridge`），而内核可能正在重启 —— 那一刻 libbox 的 `SubscribeConnections`
        // 会被内核的 `waitForStarted` 以 `os.ErrInvalid`（文案就是 "invalid argument"）拒收。
        // 没有这次握手，relay 会「建流成功 → 秒死 → 立刻重建」忙转：实测约 1000 次/秒，
        // 且每一次都经 Android 主线程 dispatch。返 `None` 让调用方退避一拍（同桌面「管理 API
        // 连不上」那条腿），是唯一不引入第二套重试状态机的收法。
        let first = match plugin
            .run_mobile_plugin_async::<ConnectionsBatch>("connectionsPoll", PollArgs { after: 0 })
            .await
        {
            Ok(b) => b,
            Err(e) => {
                log::debug!("连接流：建流握手取帧失败 {e}");
                close_connections_stream(plugin).await;
                return None;
            }
        };
        if first.closed {
            log::debug!(
                "连接流：建流即作废（{}）—— 内核多半正在重启，退避一拍重试",
                first.reason.as_deref().unwrap_or("无因由")
            );
            note_empty_generation("连接流", &CONNECTIONS_EMPTY_STREAK);
            close_connections_stream(plugin).await;
            return None;
        }
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        tauri::async_runtime::spawn(async move {
            let _stream_guard = guard;
            let mut after = first.seq;
            let mut pump = PumpLog::new("连接流", &CONNECTIONS_EMPTY_STREAK);
            pump.note(first.frames.len(), || describe_connections(&first.frames));
            for frame in first.frames {
                if tx.send(frame.into()).await.is_err() {
                    close_connections_stream(plugin).await;
                    return;
                }
            }
            loop {
                if task_stop.load(Ordering::SeqCst) {
                    break;
                }
                // 🔴 一路 await 到底：不 select、不 timeout（丢弃这个 future = tauri 侧 panic）。
                let batch = match plugin
                    .run_mobile_plugin_async::<ConnectionsBatch>(
                        "connectionsPoll",
                        PollArgs { after },
                    )
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        log::warn!("连接流：桥取帧失败 {e}");
                        break;
                    }
                };
                after = batch.seq;
                pump.note(batch.frames.len(), || describe_connections(&batch.frames));
                pump.check_silence();
                let mut disconnected = batch.closed;
                for frame in batch.frames {
                    if tx.send(frame.into()).await.is_err() {
                        disconnected = true; // 消费方走了
                        break;
                    }
                }
                if disconnected {
                    if let Some(reason) = batch.reason {
                        log::debug!("连接流：libbox 命令通道已作废（{reason}）");
                    }
                    break;
                }
            }
            pump.retire();
            // 收流。这一条同样一路 await 到底。
            close_connections_stream(plugin).await;
        });
        Some(Pump { rx, stop })
    }

    pub(super) async fn open_status() -> Option<Pump<SingBoxStatus>> {
        super::detached_open(&STATUS_STREAM_LOCK, open_status_inner).await
    }

    async fn open_status_inner(guard: MutexGuard<'static, ()>) -> Option<Pump<SingBoxStatus>> {
        let plugin = android_bridge::plugin()?;
        if let Err(e) = plugin.run_mobile_plugin_async::<()>("statsOpen", ()).await {
            log::debug!("Status 流：libbox 命令通道建流失败 {e}");
            return None;
        }
        // 建流握手，理由与连接流那条逐字相同（见 `open_connections`）。
        let first = match plugin
            .run_mobile_plugin_async::<StatusBatch>("statsPoll", PollArgs { after: 0 })
            .await
        {
            Ok(b) => b,
            Err(e) => {
                log::debug!("Status 流：建流握手取帧失败 {e}");
                close_status_stream(plugin).await;
                return None;
            }
        };
        if first.closed {
            log::debug!(
                "Status 流：建流即作废（{}）—— 内核多半正在重启，退避一拍重试",
                first.reason.as_deref().unwrap_or("无因由")
            );
            note_empty_generation("Status 流", &STATUS_EMPTY_STREAK);
            close_status_stream(plugin).await;
            return None;
        }
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = stop.clone();
        tauri::async_runtime::spawn(async move {
            let _stream_guard = guard;
            let mut after = first.seq;
            let mut pump = PumpLog::new("Status 流", &STATUS_EMPTY_STREAK);
            pump.note(first.frames.len(), || describe_status(&first.frames));
            for frame in first.frames {
                if tx.send(frame.into()).await.is_err() {
                    close_status_stream(plugin).await;
                    return;
                }
            }
            loop {
                if task_stop.load(Ordering::SeqCst) {
                    break;
                }
                let batch = match plugin
                    .run_mobile_plugin_async::<StatusBatch>("statsPoll", PollArgs { after })
                    .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        log::warn!("Status 流：桥取帧失败 {e}");
                        break;
                    }
                };
                after = batch.seq;
                pump.note(batch.frames.len(), || describe_status(&batch.frames));
                pump.check_silence();
                let mut disconnected = batch.closed;
                for frame in batch.frames {
                    if tx.send(frame.into()).await.is_err() {
                        disconnected = true;
                        break;
                    }
                }
                if disconnected {
                    if let Some(reason) = batch.reason {
                        log::debug!("Status 流：libbox 命令通道已作废（{reason}）");
                    }
                    break;
                }
            }
            pump.retire();
            close_status_stream(plugin).await;
        });
        Some(Pump { rx, stop })
    }
}

#[cfg(test)]
mod tests;
