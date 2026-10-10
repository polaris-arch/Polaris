//! 周期测速调度器：应用运行期间，按订阅各自的周期对它的节点发一轮周期来源的测速。
//!
//! 调度器不持有测量逻辑，只做三件事：
//! - 决定何时对哪些节点发一轮（[`Planner`]：每拍裁决、到期与重试、补测、冻结检测）；
//! - 把目标排好序交给测量层（[`order_targets`]），结果由测量层的发布口入账
//!   （[`crate::runtime::measurement_ledger`]）；
//! - 对外报告自己的状态（[`Planner::status`]）。
//!
//! 形态与订阅自动更新调度器相同：进程级单例，装配时启动一次，自己按拍读核世代、运行态与配置，
//! 不挂在起核流程上。核不在运行即未启用；周期来源在任何平台都不启动代理。
//!
//! **纯决策 / 计时分离**：[`decide_tick`] 与 [`Planner`] 不做任何 I/O，时刻全部由调用方注入；
//! 定时器、事件接线与起一轮是 [`MeasurementScheduler`] 这层薄壳。
//!
//! 两个时钟各管一件事：到期用单调时刻，冻结检测与离开时长只比墙钟（各平台单调时钟在进程被
//! 冻结期间是否前进并不一致）。墙钟被回拨时冻结会漏判一次，后果是一批可能的假失败，由下一轮覆盖。

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::FutureExt;
use polaris_helper_proto::Platform;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Listener, Manager};
use tokio::sync::Notify;

use crate::commands::speedtest::{
    current_server_fingerprints, resolve_speed_test_url, run_scheduled_round, RoundAbort,
    RoundReport, ScheduledRound, SpeedTestOrigin,
};
use crate::events::channel::{
    EVENT_CONFIG_CHANGED, EVENT_PROXY_STARTED, EVENT_SPEED_TEST_SCHEDULE,
};
use crate::runtime::measurement_ledger::{
    self, EvictContext, LedgerEntry, MeasurementLedger, ReadContext,
};
use crate::runtime::speedtest::InterruptReason;
use crate::runtime::subscription_scheduler::now_ms;
use crate::runtime::{auto_select, AppRuntime};

/// 调度器总开关。`false`：不发起任何一轮，状态恒为未启用（结果入账照常，无行为影响）。
/// 回退 = 改这个常量后重新出包；两种取值各有单测。
const SCHEDULER_ENABLED: bool = true;

/// 运行与等待态的拍长。
const TICK_ACTIVE_MS: u64 = 5_000;
/// 暂停与未启用态的拍长。生命周期事件、配置变更与一轮收尾会立即唤醒，不等整拍。
const TICK_IDLE_MS: u64 = 30_000;
/// 相邻两拍的墙钟间隔超过拍长的这么多倍，判为进程被冻结过。读状态时同一个倍数用来判调度器停滞。
const TICK_GAP_FACTOR: u64 = 3;
/// 每个核世代的首轮在核就绪后这么久发起（让开起核后的选择器校正与出口探测）。
const FIRST_ROUND_DELAY_MS: u64 = 15_000;
/// 闸被占用时的重试间隔与同一次到期的重试上限；用完后并入下一个到期。
const BUSY_RETRY_MS: u64 = 60_000;
const BUSY_RETRY_MAX: u8 = 3;
/// 被更高优先级抢占后，隔这么久只对仍缺当前结果的目标补发。
const PREEMPT_RESEND_MS: u64 = 30_000;
/// 离开前台不少于这么久，回来时前台代次加一。
const AWAY_EPOCH_MS: u64 = 5 * 60_000;
/// 一轮内相邻两条结果的墙钟间隔超过它即判设备冻结：单节点最坏耗时 12 秒加 3 秒余量。
/// 取 `u64::MAX` 即关掉这条判据。
const FREEZE_GAP_MS: u64 = 15_000;
/// iOS 失去活跃后的静默期：不发起新的一轮，过后不需要回前台信号即可继续。
const IOS_QUIET_MS: u64 = 30_000;
/// 桌面网络代次变化后，等它这么久不再变化才补测；两次由切网触发的补测之间的最小间隔。
const NET_SETTLE_MS: u64 = 10_000;
const NET_RETEST_MIN_GAP_MS: u64 = 5 * 60_000;
/// 没有订阅到期时，设备状况（计费、省电）隔这么久查一次，用来发现翻转。
const CONDITIONS_POLL_MS: u64 = 60_000;
/// 配置快照的最长使用时间（配置变更事件会提前作废它）。
const CONFIG_REFRESH_MS: u64 = 30_000;
/// 计费网络下选「降频」时周期乘的倍数。
const METERED_REDUCED_FACTOR: u64 = 4;
/// 单轮时间预算的上限；实际取它与周期一半中较小者。
const ROUND_BUDGET_MAX_MS: u64 = 10 * 60_000;
/// 到期时刻相差不超过合并窗的订阅并入同一轮：周期的十分之一，至多这么久。
const MERGE_WINDOW_MAX_MS: u64 = 3 * 60_000;
/// 新鲜期上限：节点实际测量间隔的 2 倍，不低于这个值。
const FRESHNESS_FLOOR_MS: u64 = 10 * 60_000;
/// 连续这么多轮失败的节点进入退避：此后每跳过这么多轮测一次。
pub(crate) const BACKOFF_AFTER_FAILURES: u32 = 3;
pub(crate) const BACKOFF_SKIP_ROUNDS: u32 = 3;
/// 连续这么多轮被时间预算截断即置「周期过短」。
const SHORT_PERIOD_STREAK: u8 = 3;
/// 读状态时：距上次完整轮次超过周期的这么多倍即逾期。
const OVERDUE_FACTOR: u64 = 3;

/// 一个时刻的两个读数：单调毫秒（到期）与墙钟 Unix 毫秒（冻结检测、对外时间）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Now {
    pub(crate) mono: u64,
    pub(crate) wall: u64,
}

/// 当前网络是否计费。「不可得」是独立的一态，不当作非计费，按「照常」处理并如实报出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Metered {
    Yes,
    No,
    Unavailable,
}

impl Metered {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Yes => "metered",
            Self::No => "unmetered",
            Self::Unavailable => "unavailable",
        }
    }
}

/// 每轮准入前拉一次的设备状况。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeviceConditions {
    pub(crate) metered: Metered,
    /// 省电模式。信号不可得时按未开启处理。
    pub(crate) power_save: bool,
}

impl DeviceConditions {
    const UNAVAILABLE: Self = Self {
        metered: Metered::Unavailable,
        power_save: false,
    };
}

/// 计费网络下周期测速的行为（全局一项，五个平台同一套取值与缺省）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeteredPolicy {
    Pause,
    /// 周期乘 [`METERED_REDUCED_FACTOR`]。
    Reduced,
    Normal,
}

impl MeteredPolicy {
    fn parse(value: Option<&str>) -> Self {
        match value.unwrap_or(polaris_store::SPEED_TEST_METERED_POLICY_DEFAULT) {
            "pause" => Self::Pause,
            "normal" => Self::Normal,
            _ => Self::Reduced,
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Reduced => "reduced",
            Self::Normal => "normal",
        }
    }
}

/// 没有可执行的计划的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Idle {
    /// 源码里的总开关关着。
    SwitchedOff,
    /// 用户在设置里关掉了周期测速。
    UserDisabled,
    /// 本平台未启用。
    PlatformNotEnabled,
    CoreNotRunning,
    /// 没有任何订阅开启周期测速。
    NoSubscription,
}

impl Idle {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SwitchedOff => "switchedOff",
            Self::UserDisabled => "userDisabled",
            Self::PlatformNotEnabled => "platformNotEnabled",
            Self::CoreNotRunning => "coreNotRunning",
            Self::NoSubscription => "noSubscription",
        }
    }
}

/// 有计划、但到期也不执行的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pause {
    Background,
    Metered,
    PowerSave,
}

impl Pause {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Metered => "metered",
            Self::PowerSave => "powerSave",
        }
    }
}

/// 一拍的裁决。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    Idle(Idle),
    Paused(Pause),
    /// 一轮在飞。
    Running,
    /// 有计划，未到期（或到期后在等重试 / 静默期）。
    Waiting,
    /// 发一轮。
    Start,
}

/// 一拍裁决的全部输入。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TickFacts {
    pub(crate) switched_on: bool,
    pub(crate) user_enabled: bool,
    pub(crate) platform_enabled: bool,
    pub(crate) core_running: bool,
    pub(crate) has_plan: bool,
    pub(crate) foreground: bool,
    pub(crate) round_in_flight: bool,
    pub(crate) power_save: bool,
    pub(crate) metered: Metered,
    pub(crate) policy: MeteredPolicy,
    pub(crate) due: bool,
    /// iOS 失去活跃后的静默期内。
    pub(crate) quiet: bool,
}

/// **每拍裁决**（纯函数）。次序即优先级：先判有没有计划，再判该不该暂停，最后才看到没到期。
pub(crate) const fn decide_tick(facts: &TickFacts) -> Verdict {
    if !facts.switched_on {
        return Verdict::Idle(Idle::SwitchedOff);
    }
    if !facts.user_enabled {
        return Verdict::Idle(Idle::UserDisabled);
    }
    if !facts.platform_enabled {
        return Verdict::Idle(Idle::PlatformNotEnabled);
    }
    if !facts.core_running {
        return Verdict::Idle(Idle::CoreNotRunning);
    }
    if !facts.has_plan {
        return Verdict::Idle(Idle::NoSubscription);
    }
    if !facts.foreground {
        return Verdict::Paused(Pause::Background);
    }
    if facts.round_in_flight {
        return Verdict::Running;
    }
    if facts.power_save {
        return Verdict::Paused(Pause::PowerSave);
    }
    // 计费状态不可得时不暂停：按「照常」处理。
    if matches!(facts.metered, Metered::Yes) && matches!(facts.policy, MeteredPolicy::Pause) {
        return Verdict::Paused(Pause::Metered);
    }
    if !facts.due || facts.quiet {
        return Verdict::Waiting;
    }
    Verdict::Start
}

/// 本平台是否启用周期计划。iOS 的原生侧（屏幕状态接线与设备状况查询）尚未就位：设备第一次
/// 休眠后内核会一直停在设备暂停态，经 WireGuard 类节点的探测会挂到超时并被记成失败。在那之前
/// iOS 不建计划。
pub(crate) const fn platform_enabled(platform: Platform) -> bool {
    match platform {
        Platform::Mac | Platform::Win | Platform::Linux | Platform::Android => true,
        Platform::Ios => false,
        Platform::Other => true,
    }
}

/// 只有手机按前后台判定；桌面以「进程活着且未休眠」为准。
pub(crate) const fn is_mobile(platform: Platform) -> bool {
    match platform {
        Platform::Android | Platform::Ios => true,
        Platform::Mac | Platform::Win | Platform::Linux | Platform::Other => false,
    }
}

/// 一个进入计划的订阅。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubPlan {
    pub(crate) period_ms: u64,
    /// 订阅 id 相同的节点，按配置里的顺序。
    pub(crate) members: Vec<String>,
}

/// 逐订阅的周期（毫秒）：缺省或非法值取缺省周期。
fn period_ms_of(subscription: &Value) -> u64 {
    subscription
        .get("speedTestIntervalMinutes")
        .and_then(Value::as_u64)
        .filter(|minutes| polaris_store::SPEED_TEST_INTERVAL_MINUTES.contains(minutes))
        .unwrap_or(polaris_store::SPEED_TEST_INTERVAL_MINUTES_DEFAULT)
        * 60_000
}

/// 从原始配置现算计划：开了周期测速的订阅，加上被自动选择意图指向的订阅（`auto_intent`）。
/// 一个节点只属于一个订阅，各订阅的成员集合天然不相交。
pub(crate) fn plan_subscriptions(
    config: &Value,
    auto_intent: &BTreeSet<String>,
) -> BTreeMap<String, SubPlan> {
    let servers = config.get("servers").and_then(Value::as_array);
    config
        .get("subscriptions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|subscription| {
            let id = subscription.get("id").and_then(Value::as_str)?;
            let enabled = subscription
                .get("periodicSpeedTest")
                .and_then(Value::as_bool)
                .unwrap_or(polaris_store::PERIODIC_SPEED_TEST_SUBSCRIPTION_DEFAULT);
            if !enabled && !auto_intent.contains(id) {
                return None;
            }
            let members = servers
                .into_iter()
                .flatten()
                .filter(|server| server.get("subscriptionId").and_then(Value::as_str) == Some(id))
                .filter_map(|server| server.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            Some((
                id.to_string(),
                SubPlan {
                    period_ms: period_ms_of(subscription),
                    members,
                },
            ))
        })
        .collect()
}

/// 节点的新鲜期上限：它实际测量间隔的 2 倍，不低于 10 分钟。
///
/// 实际测量间隔是所属订阅的周期，再乘上调度器自己安排的两种拉长：计费网络降频时每
/// [`METERED_REDUCED_FACTOR`] 个周期才发一轮（`period_factor`，取那条结果**入账时**的倍数，记在
/// 账本条目上：一条结果的新鲜期在它产生时就定下，此后倍数变大不会让已过期的结果重新变新鲜）；
/// 连续失败达到退避门槛的节点每 `BACKOFF_SKIP_ROUNDS + 1` 轮才测一次（`consecutive_failures`
/// 是那条记录的连续失败次数）。上限若不跟着放宽，结果会在调度器按计划不去测它的那段时间里
/// 过期：降频期间约一半时间没有可选点，退避中的节点约一半时间被读成「没有当前结果」。
///
/// 不属于任何计划内订阅的节点（只被手动测过）按缺省周期算。
pub(crate) fn freshness_cap_ms(
    plan: &BTreeMap<String, SubPlan>,
    node_id: &str,
    period_factor: u64,
    consecutive_failures: u32,
) -> u64 {
    let period = plan
        .values()
        .find(|sub| sub.members.iter().any(|member| member == node_id))
        .map_or(
            polaris_store::SPEED_TEST_INTERVAL_MINUTES_DEFAULT * 60_000,
            |sub| sub.period_ms,
        );
    let backoff = if consecutive_failures >= BACKOFF_AFTER_FAILURES {
        u64::from(BACKOFF_SKIP_ROUNDS) + 1
    } else {
        1
    };
    (period * period_factor * backoff * 2).max(FRESHNESS_FLOOR_MS)
}

/// 账本里一条记录的新鲜期上限：降频倍数与连续失败次数都取自这条记录本身。
pub(crate) fn entry_freshness_cap_ms(
    plan: &BTreeMap<String, SubPlan>,
    node_id: &str,
    entry: &LedgerEntry,
) -> u64 {
    freshness_cap_ms(
        plan,
        node_id,
        entry.period_factor,
        entry.consecutive_failures,
    )
}

/// 用户是否开着周期测速的全局总开关。
fn user_enabled(config: &Value) -> bool {
    config
        .get("periodicSpeedTestEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(polaris_store::PERIODIC_SPEED_TEST_ENABLED_DEFAULT)
}

/// 到期而未执行的原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkipReason {
    Background,
    Metered,
    PowerSave,
    /// 闸被占用，附占用方来源。
    GateBusy(Option<SpeedTestOrigin>),
    Preempted,
    /// 超出单轮时间预算。
    Budget,
    Frozen,
    PoolUnavailable,
    /// 本轮零可测，附预筛分布。
    NothingTestable(BTreeMap<&'static str, usize>),
    /// 本轮没有目标（都已有当前结果，或都在退避里）。
    NoTargets,
    /// 这一轮的任务异常终止，没有回执。
    Crashed,
}

impl SkipReason {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Metered => "metered",
            Self::PowerSave => "powerSave",
            Self::GateBusy(_) => "gateBusy",
            Self::Preempted => "preempted",
            Self::Budget => "budget",
            Self::Frozen => "frozen",
            Self::PoolUnavailable => "poolUnavailable",
            Self::NothingTestable(_) => "nothingTestable",
            Self::NoTargets => "noTargets",
            Self::Crashed => "crashed",
        }
    }

    fn to_json(&self, at: u64) -> Value {
        let mut skip = json!({ "at": at, "reason": self.as_str() });
        match self {
            Self::GateBusy(Some(holder)) => skip["holder"] = json!(holder.as_str()),
            Self::NothingTestable(skipped) => skip["skipped"] = json!(skipped),
            _ => {}
        }
        skip
    }
}

/// 调度器为什么取消了在飞的一轮（测量层只知道「被取消」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AbortCause {
    Background,
    Frozen,
    Budget,
    /// 核换代、核停止或计划被关：不记账，状态随后整体重置。
    Reset,
}

#[derive(Debug, Clone, Default)]
struct SubState {
    period_ms: u64,
    members: Vec<String>,
    /// 下次到期的单调时刻。
    next_due: u64,
    /// 同一次到期因闸被占用已重试的次数。
    busy_retries: u8,
    /// 下一轮只对仍缺当前结果的目标补发（上一轮被抢占 / 被打断）。
    resend: bool,
    /// 最近一次以完成收尾、覆盖本订阅全部可测成员的一轮的结束时刻（墙钟）。
    last_full_round: Option<u64>,
    last_round: Option<Value>,
    last_skip: Option<(u64, SkipReason)>,
    /// 当前这次到期已经为同一个原因记过日志。
    skip_logged: Option<SkipReason>,
    truncated_streak: u8,
    period_too_short: bool,
    /// 本订阅最近一次收尾的一轮的序号（见 [`Planner::round_serial`]）。
    last_round_serial: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveRound {
    subs: Vec<String>,
    started_mono: u64,
    budget_ms: u64,
    /// 准入那一刻的降频倍数（1 或 [`METERED_REDUCED_FACTOR`]）。
    period_factor: u64,
    abort_cause: Option<AbortCause>,
}

/// 调度器发一轮时交给外壳的指令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RoundStart {
    /// 并入这一轮的订阅 id（已排序）。
    pub(crate) subs: Vec<String>,
    /// 到期订阅 id 以逗号连接。
    pub(crate) scope: String,
    /// 只测仍缺当前结果的目标。
    pub(crate) only_missing: bool,
}

/// 一拍的输入。
pub(crate) struct TickInput<'a> {
    pub(crate) now: Now,
    pub(crate) user_enabled: bool,
    /// 当前主核世代；核不在运行为 `None`。
    pub(crate) generation: Option<u64>,
    pub(crate) plan: &'a BTreeMap<String, SubPlan>,
    pub(crate) network_epoch: Option<u64>,
    /// 这一拍刚拉到的设备状况；没拉则沿用上一次的。
    pub(crate) conditions: Option<DeviceConditions>,
    pub(crate) policy: MeteredPolicy,
    /// 换核世代或切网时，这本账里的连续失败数清零。
    pub(crate) ledger: &'a MeasurementLedger,
}

/// 一拍的输出。
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct TickOutput {
    /// 取消在飞的一轮。
    pub(crate) abort: bool,
    pub(crate) start: Option<RoundStart>,
    pub(crate) sleep_ms: u64,
}

/// 调度器的状态机（不做 I/O；时刻、配置与平台信号全部注入）。
pub(crate) struct Planner {
    switched_on: bool,
    platform: Platform,
    platform_enabled: bool,
    generation: Option<u64>,
    ready: Now,
    subs: BTreeMap<String, SubState>,
    foreground: bool,
    /// 离开前台的墙钟时刻。
    left_at: Option<u64>,
    quiet_until: u64,
    foreground_epoch: u64,
    /// 最近一拍的墙钟时刻，与那一拍计划睡多久。
    last_tick: (u64, u64),
    network_epoch: Option<u64>,
    network_changed_at: Option<u64>,
    last_network_retest: Option<u64>,
    conditions: DeviceConditions,
    conditions_at: Option<u64>,
    /// 最近一次**已知**的计费状态（是 / 否）。中间隔着「不可得」的迁移也据它判翻转。
    last_known_metered: Option<bool>,
    /// 已观测的已知计费翻转序号：只供故障排除消费，不改变择优有效范围。
    metered_change_epoch: u64,
    round: Option<ActiveRound>,
    /// 上一轮被时间预算截断时没测到的节点：下一轮排在最前。
    carry_over: Vec<String>,
    /// 退避中的节点已经跳过的轮数。
    backoff_skips: BTreeMap<String, u32>,
    /// 出过换核世代或切网（含视同切网的计费翻转、久离返回）而账本的连续失败数还没清零。
    forgive_failures: bool,
    verdict: Verdict,
    /// 最近一拍生效的计费网络策略。
    policy: MeteredPolicy,
    /// 上一轮开始时的波宽，以及那一轮内是否减半过。
    last_width: Option<(usize, bool)>,
    /// 已收尾的轮次数（进程内单调，不随核世代清零）。一轮算收尾：测完了、被时间预算截断、
    /// 没有目标、或零可测；被抢占、被离开前台或冻结打断的不算，它们还要补发。
    round_serial: u64,
}

/// 调度器给自动选点读的几个量（见 [`signals`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Signals {
    /// 手机是否在前台；桌面恒为真。
    pub(crate) foreground: bool,
    /// 最近一拍的裁决：计划没在执行时，原因在这里。
    pub(crate) verdict: Verdict,
    /// 已收尾的轮次数。
    pub(crate) round_serial: u64,
    /// 进程内单调的已知计费翻转序号，不随核世代或前台变化清零。
    pub(crate) metered_change_epoch: u64,
    /// 逐订阅：最近一次收尾的一轮的序号，与最近一次到期未执行的原因。
    pub(crate) subscriptions: BTreeMap<String, (Option<u64>, Option<&'static str>)>,
}

impl Signals {
    pub(crate) const fn idle() -> Self {
        Self {
            foreground: true,
            verdict: Verdict::Idle(Idle::CoreNotRunning),
            round_serial: 0,
            metered_change_epoch: 0,
            subscriptions: BTreeMap::new(),
        }
    }
}

impl Signals {
    /// 周期计划没在执行的原因（未启用或暂停）；在执行时为 `None`。
    pub(crate) const fn blocked_by(&self) -> Option<&'static str> {
        match self.verdict {
            Verdict::Idle(idle) => Some(idle.as_str()),
            Verdict::Paused(pause) => Some(pause.as_str()),
            Verdict::Running | Verdict::Waiting | Verdict::Start => None,
        }
    }
}

static SIGNALS: Mutex<Signals> = Mutex::new(Signals::idle());

/// 调度器最近一拍对外公布的读数。调度器没启动时是初值：在前台、不降频、没有任何一轮。
pub(crate) fn signals() -> Signals {
    SIGNALS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Resolve the production intent before building the subscription plan.
fn effective_plan(config: &Value, platform: Platform) -> BTreeMap<String, SubPlan> {
    let auto_intent =
        auto_select::effective_subscription(config, auto_select::Switches::PRODUCTION, platform)
            .map(str::to_string)
            .into_iter()
            .collect();
    plan_subscriptions(config, &auto_intent)
}

impl Planner {
    pub(crate) fn new(platform: Platform, now: Now) -> Self {
        Self::with_switch(SCHEDULER_ENABLED, platform, now)
    }

    /// 总开关作参数传入，使两种取值都能被单测覆盖；生产只经 [`Planner::new`]。
    fn with_switch(switched_on: bool, platform: Platform, now: Now) -> Self {
        Self {
            switched_on,
            platform,
            platform_enabled: platform_enabled(platform),
            generation: None,
            ready: now,
            subs: BTreeMap::new(),
            foreground: true,
            left_at: None,
            quiet_until: 0,
            foreground_epoch: 0,
            last_tick: (now.wall, TICK_IDLE_MS),
            network_epoch: None,
            network_changed_at: None,
            last_network_retest: None,
            conditions: DeviceConditions::UNAVAILABLE,
            conditions_at: None,
            last_known_metered: None,
            metered_change_epoch: 0,
            round: None,
            carry_over: Vec::new(),
            backoff_skips: BTreeMap::new(),
            forgive_failures: false,
            verdict: Verdict::Idle(Idle::CoreNotRunning),
            policy: MeteredPolicy::parse(None),
            last_width: None,
            round_serial: 0,
        }
    }

    /// 当前的降频倍数：计费网络且策略为降频时每 [`METERED_REDUCED_FACTOR`] 个周期发一轮。
    /// 准入时定下一轮的到期间隔；同步给账本后，此后入账的结果带着它定各自的新鲜期上限。
    pub(crate) const fn period_factor(&self) -> u64 {
        if matches!(self.conditions.metered, Metered::Yes)
            && matches!(self.policy, MeteredPolicy::Reduced)
        {
            METERED_REDUCED_FACTOR
        } else {
            1
        }
    }

    /// 给自动选点读的几个量。
    pub(crate) fn signals(&self) -> Signals {
        Signals {
            foreground: self.foreground,
            verdict: self.verdict,
            round_serial: self.round_serial,
            metered_change_epoch: self.metered_change_epoch,
            subscriptions: self
                .subs
                .iter()
                .map(|(id, sub)| {
                    (
                        id.clone(),
                        (
                            sub.last_round_serial,
                            sub.last_skip.as_ref().map(|(_, reason)| reason.as_str()),
                        ),
                    )
                })
                .collect(),
        }
    }

    pub(crate) const fn foreground_epoch(&self) -> u64 {
        self.foreground_epoch
    }

    /// 这一拍之前要不要先拉一次设备状况：有订阅到期（准入前必查），或距上次查询已久。
    pub(crate) fn wants_conditions(&self, now: Now) -> bool {
        if self.subs.is_empty() || self.round.is_some() {
            return false;
        }
        self.subs.values().any(|sub| sub.next_due <= now.mono)
            || self
                .conditions_at
                .is_none_or(|at| now.mono.saturating_sub(at) >= CONDITIONS_POLL_MS)
    }

    fn due_now(&mut self, now: Now) {
        for sub in self.subs.values_mut() {
            sub.next_due = sub.next_due.min(now.mono);
        }
    }

    /// 回到前台（或解冻）后的评估：离开够久则前台代次加一并立即补测。手机上旧结果随代次不可用于
    /// 选点；桌面休眠醒来后网络多半已变。这一次计入切网补测的最小间隔，紧随其后的网络代次变化
    /// 不再立刻另发一轮。离开不久、且没有订阅到期时沿用原到期时刻。
    fn returned(&mut self, now: Now, away_ms: u64) {
        if away_ms < AWAY_EPOCH_MS {
            return;
        }
        self.foreground_epoch += 1;
        self.forgive_failures = true;
        self.last_network_retest = Some(now.mono);
        self.due_now(now);
    }
}

/// 前后台信号只在手机上有来源；桌面构型里这一组没有调用方。
#[cfg_attr(not(mobile), allow(dead_code))]
impl Planner {
    /// 离开前台。桌面不按前后台判定，忽略。
    ///
    /// Android 的暂停与恢复成对出现：进入暂停，等回前台才退出。iOS 的这个信号来自「即将失去
    /// 活跃」，下拉控制中心也会触发而不会有对应的回前台信号，所以只作打断：取消在飞的一轮并
    /// 静默一段时间，不进入需要回前台信号才能退出的状态。
    pub(crate) fn on_suspended(&mut self, now: Now) -> bool {
        match self.platform {
            Platform::Android => self.foreground = false,
            Platform::Ios => self.quiet_until = now.mono + IOS_QUIET_MS,
            Platform::Mac | Platform::Win | Platform::Linux | Platform::Other => return false,
        }
        self.left_at.get_or_insert(now.wall);
        self.abort(AbortCause::Background)
    }

    /// 回到前台。
    pub(crate) fn on_resumed(&mut self, now: Now) {
        self.foreground = true;
        if let Some(left_at) = self.left_at.take() {
            self.returned(now, now.wall.saturating_sub(left_at));
        }
    }
}

impl Planner {
    fn abort(&mut self, cause: AbortCause) -> bool {
        match self.round.as_mut() {
            Some(round) => {
                // 重置盖过已标而未收尾的成因：那一轮属于旧世代，收尾时不得按旧成因给新计划记账。
                if cause == AbortCause::Reset {
                    round.abort_cause = Some(cause);
                } else {
                    round.abort_cause.get_or_insert(cause);
                }
                true
            }
            None => false,
        }
    }

    fn skip(&mut self, id: &str, now: Now, reason: SkipReason) {
        let Some(sub) = self.subs.get_mut(id) else {
            return;
        };
        // 同一原因连续出现只在首次记一行，不每拍重复。
        if sub.skip_logged.as_ref().map(SkipReason::as_str) != Some(reason.as_str()) {
            log::info!("周期测速：订阅 {id} 到期未执行，原因 {}", reason.as_str());
            sub.skip_logged = Some(reason.clone());
        }
        sub.last_skip = Some((now.wall, reason));
    }

    /// Publish admission conditions to subsequent measurement receipts in the
    /// same production step as planning, so reduced rounds retain their factor.
    fn tick_and_sync_ledger(&mut self, input: &TickInput<'_>) -> TickOutput {
        let output = self.tick(input);
        input.ledger.set_foreground_epoch(self.foreground_epoch());
        input.ledger.set_period_factor(self.period_factor());
        output
    }

    /// **一拍**：推进状态机，返回要外壳做的事。
    pub(crate) fn tick(&mut self, input: &TickInput<'_>) -> TickOutput {
        let now = input.now;
        let mut out = TickOutput::default();

        // 相邻两拍的墙钟间隔远超拍长 ⇒ 进程被冻结过（休眠 / 被系统挂起 / 定时器被推迟）。
        let (last_wall, last_sleep) = self.last_tick;
        let gap = now.wall.saturating_sub(last_wall);
        if gap > last_sleep * TICK_GAP_FACTOR {
            log::warn!("周期测速：两拍相隔 {gap}ms，判定进程被冻结过");
            out.abort |= self.abort(AbortCause::Frozen);
            if self.left_at.is_none() {
                self.returned(now, gap);
            }
        }

        // 核世代变化（含停核）：到期时刻、重试计数、轮转与退避全部清空，按新世代重来。
        // 旧世代的结果留在账本里，读取时判为过期。
        let generation = input
            .generation
            .filter(|_| self.switched_on && self.platform_enabled && input.user_enabled);
        if generation != self.generation {
            out.abort |= self.abort(AbortCause::Reset);
            self.generation = generation;
            self.ready = now;
            self.subs.clear();
            self.carry_over.clear();
            self.backoff_skips.clear();
            self.forgive_failures = true;
            self.network_epoch = input.network_epoch;
            self.network_changed_at = None;
            if generation.is_some() {
                if input.plan.is_empty() {
                    log::info!("周期测速：未启用（没有订阅开启周期测速）");
                } else {
                    let summary: Vec<String> = input
                        .plan
                        .iter()
                        .map(|(id, sub)| {
                            format!(
                                "{id}（每 {} 分钟，{} 个节点）",
                                sub.period_ms / 60_000,
                                sub.members.len()
                            )
                        })
                        .collect();
                    log::info!(
                        "周期测速：本次连接的计划含 {} 个订阅：{}",
                        summary.len(),
                        summary.join("、")
                    );
                }
            }
        }
        self.policy = input.policy;

        // 计划每拍现算：订阅的开关与周期读自原始配置，不重启内核，下一拍生效。
        if self.generation.is_some() {
            self.subs.retain(|id, _| input.plan.contains_key(id));
            let period_factor = self.period_factor();
            let first_due = (self.ready.mono + FIRST_ROUND_DELAY_MS).max(now.mono);
            for (id, plan) in input.plan {
                let sub = self.subs.entry(id.clone()).or_insert_with(|| SubState {
                    next_due: first_due,
                    ..SubState::default()
                });
                sub.period_ms = plan.period_ms;
                sub.members.clone_from(&plan.members);
                // 周期被改短时不必等旧的到期时刻。
                // 降频解除（含计费状态变成不可得）时同理：到期时刻不留在 4 倍周期之外。
                sub.next_due = sub.next_due.min(now.mono + plan.period_ms * period_factor);
            }
            if self.subs.is_empty() {
                out.abort |= self.abort(AbortCause::Reset);
            }
        }

        // 桌面切网：代次变了先等它稳定，再补测一轮；两次补测之间有最小间隔。核换代时由新世代的
        // 首轮接管（上面已把观察到的代次对齐）。在飞的一轮不必处理，跨代次的节点测量层已单独作废。
        if let (Some(seen), Some(current)) = (self.network_epoch, input.network_epoch) {
            if seen != current {
                self.network_changed_at = Some(now.mono);
                self.forgive_failures = true;
            }
        }
        self.network_epoch = input.network_epoch;
        if self.network_changed_at.is_some_and(|at| {
            now.mono.saturating_sub(at) >= NET_SETTLE_MS
                && self
                    .last_network_retest
                    .is_none_or(|last| now.mono.saturating_sub(last) >= NET_RETEST_MIN_GAP_MS)
        }) {
            self.network_changed_at = None;
            self.last_network_retest = Some(now.mono);
            self.due_now(now);
        }

        // 计费状态翻转：这一次已知的取值与上一次已知的取值相反（中间隔着「不可得」也算，手机上
        // Wi-Fi 断开到蜂窝接上之间常有一段读不到）。多半是 Wi-Fi 与蜂窝互换，视同切网，立即补测；
        // 手机上没有网络代次，另把前台代次加一使旧结果作废。补测与切网共用最小间隔：间隔内的翻转
        // 记作一次待处理的切网，满间隔后补一轮，来回翻转绕不过降频。
        if let Some(conditions) = input.conditions {
            let known = match conditions.metered {
                Metered::Yes => Some(true),
                Metered::No => Some(false),
                Metered::Unavailable => None,
            };
            let flipped = known.is_some()
                && self.last_known_metered.is_some()
                && known != self.last_known_metered;
            if known.is_some() {
                self.last_known_metered = known;
            }
            self.conditions = conditions;
            self.conditions_at = Some(now.mono);
            if flipped {
                // 先记录事实，再判断补测节流；间隔内来回翻转也不能丢失。
                self.metered_change_epoch = self.metered_change_epoch.saturating_add(1);
                self.forgive_failures = true;
                if is_mobile(self.platform) {
                    self.foreground_epoch += 1;
                }
                if self
                    .last_network_retest
                    .is_none_or(|last| now.mono.saturating_sub(last) >= NET_RETEST_MIN_GAP_MS)
                {
                    self.last_network_retest = Some(now.mono);
                    self.due_now(now);
                } else {
                    self.network_changed_at = Some(now.mono);
                }
            }
        }

        // 换核世代或切网之后，此前的连续失败不再作数：清零，否则断网期间进了退避的节点在补测轮
        // 里一个也排不上。先于本拍可能发起的一轮，排目标时读到的已是清零后的计数。
        if std::mem::take(&mut self.forgive_failures) {
            input.ledger.forgive_failures();
        }

        // 一轮超出时间预算即取消，没测到的节点轮转到下一轮最前。
        if self
            .round
            .as_ref()
            .is_some_and(|round| now.mono.saturating_sub(round.started_mono) > round.budget_ms)
        {
            out.abort |= self.abort(AbortCause::Budget);
        }

        let due: Vec<String> = self
            .subs
            .iter()
            .filter(|(_, sub)| sub.next_due <= now.mono)
            .map(|(id, _)| id.clone())
            .collect();
        self.verdict = decide_tick(&TickFacts {
            switched_on: self.switched_on,
            user_enabled: input.user_enabled,
            platform_enabled: self.platform_enabled,
            core_running: input.generation.is_some(),
            has_plan: !self.subs.is_empty(),
            foreground: self.foreground,
            round_in_flight: self.round.is_some(),
            power_save: self.conditions.power_save,
            metered: self.conditions.metered,
            policy: input.policy,
            due: !due.is_empty(),
            quiet: now.mono < self.quiet_until,
        });
        match self.verdict {
            Verdict::Start => {
                // 合并：到期时刻落在合并窗内的订阅并入同一轮，只抢一次闸。
                let mut subs: Vec<String> = self
                    .subs
                    .iter()
                    .filter(|(_, sub)| {
                        sub.next_due <= now.mono + (sub.period_ms / 10).min(MERGE_WINDOW_MAX_MS)
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                subs.sort();
                let included = |id: &String| self.subs.get(id);
                let only_missing = subs.iter().filter_map(included).all(|sub| sub.resend);
                let budget_ms = subs
                    .iter()
                    .filter_map(included)
                    .map(|sub| sub.period_ms / 2)
                    .min()
                    .unwrap_or(ROUND_BUDGET_MAX_MS)
                    .min(ROUND_BUDGET_MAX_MS);
                for id in &subs {
                    if let Some(sub) = self.subs.get_mut(id) {
                        if sub.skip_logged.take().is_some() {
                            log::info!("周期测速：订阅 {id} 恢复执行");
                        }
                    }
                }
                self.round = Some(ActiveRound {
                    subs: subs.clone(),
                    started_mono: now.mono,
                    budget_ms,
                    period_factor: self.period_factor(),
                    abort_cause: None,
                });
                out.start = Some(RoundStart {
                    scope: subs.join(","),
                    subs,
                    only_missing,
                });
                self.verdict = Verdict::Running;
            }
            Verdict::Paused(pause) => {
                let reason = match pause {
                    Pause::Background => SkipReason::Background,
                    Pause::Metered => SkipReason::Metered,
                    Pause::PowerSave => SkipReason::PowerSave,
                };
                for id in &due {
                    self.skip(id, now, reason.clone());
                }
            }
            Verdict::Idle(_) | Verdict::Running | Verdict::Waiting => {}
        }

        out.sleep_ms = match self.verdict {
            Verdict::Idle(_) | Verdict::Paused(_) => TICK_IDLE_MS,
            Verdict::Running | Verdict::Waiting | Verdict::Start => TICK_ACTIVE_MS,
        };
        self.last_tick = (now.wall, out.sleep_ms);
        out
    }

    /// 一轮收尾：按结局安排各订阅的下次到期，并记下观测量。
    pub(crate) fn on_round_end(&mut self, now: Now, outcome: &ScheduledRound) {
        let Some(round) = self.round.take() else {
            return;
        };
        if round.abort_cause == Some(AbortCause::Reset) {
            return;
        }
        let next_regular =
            |sub: &SubState| round.started_mono + sub.period_ms * round.period_factor;
        match outcome {
            // 闸被手动或故障切换占着：没有任何节点被测。隔一会儿重试，次数有上界。
            ScheduledRound::Busy(holder) => {
                for id in &round.subs {
                    self.skip(id, now, SkipReason::GateBusy(*holder));
                    let Some(sub) = self.subs.get_mut(id) else {
                        continue;
                    };
                    sub.busy_retries += 1;
                    if sub.busy_retries > BUSY_RETRY_MAX {
                        sub.busy_retries = 0;
                        sub.next_due = now.mono + sub.period_ms;
                    } else {
                        sub.next_due = now.mono + BUSY_RETRY_MS;
                    }
                }
            }
            ScheduledRound::PoolUnavailable => {
                for id in &round.subs {
                    self.skip(id, now, SkipReason::PoolUnavailable);
                    if let Some(sub) = self.subs.get_mut(id) {
                        sub.next_due = next_regular(sub);
                    }
                }
            }
            ScheduledRound::Ran(report) => {
                self.last_width = Some((report.width, report.halved));
                let frozen = report.frozen || round.abort_cause == Some(AbortCause::Frozen);
                let skip = if frozen {
                    Some(SkipReason::Frozen)
                } else {
                    match (report.interrupted, round.abort_cause) {
                        (Some(InterruptReason::Preempted), _) => Some(SkipReason::Preempted),
                        (Some(_), Some(AbortCause::Budget)) => Some(SkipReason::Budget),
                        (Some(_), Some(AbortCause::Background)) => Some(SkipReason::Background),
                        // 核换代 / 停核打断的一轮：状态在下一拍整体重置，这里不记账。
                        (Some(_), _) => return,
                        (None, _)
                            if report.measured.is_empty()
                                && report.unmeasured.is_empty()
                                && report.skipped.is_empty() =>
                        {
                            Some(SkipReason::NoTargets)
                        }
                        (None, _) if report.measured.is_empty() && report.unmeasured.is_empty() => {
                            let mut skipped = BTreeMap::new();
                            for (_, reason) in &report.skipped {
                                *skipped.entry(*reason).or_insert(0) += 1;
                            }
                            Some(SkipReason::NothingTestable(skipped))
                        }
                        (None, _) => None,
                    }
                };
                // 这一轮算不算收尾（首轮完成的判据之一）：被抢占或被打断的还要补发，不算。
                if matches!(
                    skip,
                    None | Some(
                        SkipReason::Budget | SkipReason::NoTargets | SkipReason::NothingTestable(_)
                    )
                ) {
                    self.round_serial += 1;
                    for id in &round.subs {
                        if let Some(sub) = self.subs.get_mut(id) {
                            sub.last_round_serial = Some(self.round_serial);
                        }
                    }
                }
                // 被时间预算截断：没测到的节点下一轮排在最前。
                if skip == Some(SkipReason::Budget) {
                    self.carry_over.clone_from(&report.unmeasured);
                } else if report.interrupted.is_none() {
                    self.carry_over.clear();
                }
                for id in &round.subs {
                    if let Some(reason) = &skip {
                        self.skip(id, now, reason.clone());
                    }
                    let Some(sub) = self.subs.get_mut(id) else {
                        continue;
                    };
                    sub.busy_retries = 0;
                    let summary = round_summary(report, &sub.members);
                    sub.last_round = Some(summary);
                    match &skip {
                        Some(SkipReason::Preempted) => {
                            sub.resend = true;
                            sub.next_due = now.mono + PREEMPT_RESEND_MS;
                        }
                        // 被离开前台或冻结打断：到期时刻不动，回来后只补仍缺的。
                        Some(SkipReason::Background | SkipReason::Frozen) => sub.resend = true,
                        Some(SkipReason::Budget) => {
                            sub.resend = false;
                            sub.next_due = next_regular(sub);
                            sub.truncated_streak = sub.truncated_streak.saturating_add(1);
                            if sub.truncated_streak >= SHORT_PERIOD_STREAK && !sub.period_too_short
                            {
                                sub.period_too_short = true;
                                log::warn!(
                                    "周期测速：订阅 {id} 连续 {} 轮在时间预算内测不完，周期过短",
                                    sub.truncated_streak
                                );
                            }
                        }
                        _ => {
                            sub.resend = false;
                            sub.truncated_streak = 0;
                            sub.period_too_short = false;
                            sub.next_due = next_regular(sub);
                            // 完整轮次要有成员真被测到：空轮与成员全被预筛跳过的一轮不算。
                            let measured = sub
                                .members
                                .iter()
                                .any(|member| report.measured.contains_key(member));
                            if measured
                                && !sub
                                    .members
                                    .iter()
                                    .any(|member| report.unmeasured.contains(member))
                            {
                                sub.last_full_round = Some(report.ended_at);
                            }
                        }
                    }
                }
            }
        }
    }

    /// 一轮的任务异常终止，没有回执：记下原因，按正常周期排下次到期，不立即重试。
    pub(crate) fn on_round_crashed(&mut self, now: Now) {
        let Some(round) = self.round.take() else {
            return;
        };
        if round.abort_cause == Some(AbortCause::Reset) {
            return;
        }
        for id in &round.subs {
            self.skip(id, now, SkipReason::Crashed);
            if let Some(sub) = self.subs.get_mut(id) {
                sub.next_due = round.started_mono + sub.period_ms * round.period_factor;
            }
        }
    }

    /// 给一轮排目标（见 [`order_targets`]）；退避计数随之推进。
    pub(crate) fn round_targets(
        &mut self,
        start: &RoundStart,
        selected: Option<&str>,
        view: &dyn Fn(&str) -> Option<EntryView>,
        current: &BTreeSet<String>,
    ) -> Vec<String> {
        let mut members: Vec<String> = Vec::new();
        let mut seen = BTreeSet::new();
        for id in start.subs.iter().filter_map(|id| self.subs.get(id)) {
            for member in &id.members {
                // 按节点 id 去重：今天各订阅的成员不相交，这一步是为全局自动选择预留的。
                if seen.insert(member.clone()) {
                    members.push(member.clone());
                }
            }
        }
        order_targets(
            &members,
            selected,
            &self.carry_over,
            view,
            start.only_missing.then_some(current),
            &mut self.backoff_skips,
        )
    }

    /// 计划状态（读时投影）。逾期与停滞只依赖几个时间戳，调度器任务即使已经停了，读取方也看得出来。
    ///
    /// `plan` 是配置里要求周期测速的订阅。计划没建起来时（全局总开关关着、本平台未启用、核不在
    /// 运行），这些订阅照样列出并带上 `blockedBy`：订阅里开着却不测，要让人看得出为什么。
    pub(crate) fn status(
        &self,
        now: Now,
        core_running: bool,
        plan: &BTreeMap<String, SubPlan>,
    ) -> Value {
        let (state, reason) = match self.verdict {
            Verdict::Idle(idle) => ("disabled", Some(idle.as_str())),
            Verdict::Paused(pause) => ("paused", Some(pause.as_str())),
            Verdict::Running | Verdict::Start => ("running", None),
            Verdict::Waiting => ("waiting", None),
        };
        let active = matches!(self.verdict, Verdict::Running | Verdict::Waiting);
        let blocked = plan
            .iter()
            .filter(|(id, _)| !self.subs.contains_key(*id))
            .map(|(id, sub)| {
                (
                    id.as_str(),
                    json!({ "periodMinutes": sub.period_ms / 60_000, "blockedBy": reason }),
                )
            });
        let subscriptions: BTreeMap<&str, Value> = self
            .subs
            .iter()
            .map(|(id, sub)| {
                let since = sub.last_full_round.unwrap_or(self.ready.wall);
                (
                    id.as_str(),
                    json!({
                        "periodMinutes": sub.period_ms / 60_000,
                        "lastFullRoundAt": sub.last_full_round,
                        "lastRound": sub.last_round,
                        "lastSkip": sub.last_skip.as_ref().map(|(at, reason)| reason.to_json(*at)),
                        "nextDueAt": now.wall + sub.next_due.saturating_sub(now.mono),
                        "periodTooShort": sub.period_too_short,
                        "overdue": active
                            && now.wall.saturating_sub(since) > sub.period_ms * OVERDUE_FACTOR,
                    }),
                )
            })
            .chain(blocked)
            .collect();
        let (last_tick, last_sleep) = self.last_tick;
        json!({
            "state": state,
            "reason": reason,
            "subscriptions": subscriptions,
            "concurrency": self.last_width.map(|(width, halved)| json!({ "width": width, "halved": halved })),
            // 当前网络的计费状态与生效的策略：两者合起来才说得清是「降频」「暂停」还是「照常」。
            "metered": self.conditions.metered.as_str(),
            "meteredPolicy": self.policy.as_str(),
            "lastTickAt": last_tick,
            "stalled": core_running
                && self.foreground
                && now.wall.saturating_sub(last_tick) > last_sleep * TICK_GAP_FACTOR,
            // 设置项的取值范围与缺省值：界面从这里取，不自己写死（并发上限随平台不同）。
            "limits": {
                "concurrencyMin": polaris_store::SPEED_TEST_CONCURRENCY_MIN,
                "concurrencyMax": polaris_store::speed_test_slot_cap(self.platform),
                "intervalMinutesMin": polaris_store::SPEED_TEST_INTERVAL_MINUTES.start(),
                "intervalMinutesMax": polaris_store::SPEED_TEST_INTERVAL_MINUTES.end(),
                "intervalMinutesDefault": polaris_store::SPEED_TEST_INTERVAL_MINUTES_DEFAULT,
                "enabledDefault": polaris_store::PERIODIC_SPEED_TEST_ENABLED_DEFAULT,
                "subscriptionDefault": polaris_store::PERIODIC_SPEED_TEST_SUBSCRIPTION_DEFAULT,
                "meteredPolicyDefault": polaris_store::SPEED_TEST_METERED_POLICY_DEFAULT,
            },
        })
    }
}

/// 上一轮在某个订阅上的摘要。
fn round_summary(report: &RoundReport, members: &[String]) -> Value {
    let ok = members
        .iter()
        .filter(|id| report.measured.get(*id) == Some(&true))
        .count();
    let failed = members
        .iter()
        .filter(|id| report.measured.get(*id) == Some(&false))
        .count();
    let mut skipped: BTreeMap<&str, usize> = BTreeMap::new();
    for (id, reason) in &report.skipped {
        if members.contains(id) {
            *skipped.entry(reason).or_insert(0) += 1;
        }
    }
    json!({
        "run": report.run,
        "startedAt": report.started_at,
        "endedAt": report.ended_at,
        "ok": ok,
        "failed": failed,
        "unmeasured": members.iter().filter(|id| report.unmeasured.contains(id)).count(),
        "skipped": skipped,
    })
}

/// 排序与退避要看的那一点账本内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EntryView {
    /// 这条结果是不是本核世代测的。
    pub(crate) this_generation: bool,
    pub(crate) ok: bool,
    pub(crate) measured_at: u64,
    pub(crate) consecutive_failures: u32,
}

/// **一轮的目标顺序**（纯函数）。分波按顺序切块，所以顺序决定谁和谁同波：
/// 当前选中的节点；上一轮被截断没测到的；本核世代还没有结果的；上次成功的（测量时刻从旧到新）；
/// 上次失败的。上次失败的集中到最后几波，免得每一波都被一个不可达节点拖到超时。
///
/// 退避：连续失败达到阈值的节点，此后每跳过若干轮才测一次；成功、被手动测过、换核世代或切网后
/// 连续失败数归零，即恢复每轮都测。`only_missing` 给出时是补发：已有当前结果的节点不重测，退避计数不动。
pub(crate) fn order_targets(
    members: &[String],
    selected: Option<&str>,
    carry_over: &[String],
    view: &dyn Fn(&str) -> Option<EntryView>,
    only_missing: Option<&BTreeSet<String>>,
    backoff_skips: &mut BTreeMap<String, u32>,
) -> Vec<String> {
    let mut ranked: Vec<((u8, u64), &String)> = Vec::new();
    for (index, id) in members.iter().enumerate() {
        let entry = view(id);
        if only_missing.is_some_and(|current| current.contains(id)) {
            continue;
        }
        if entry.is_some_and(|entry| entry.consecutive_failures >= BACKOFF_AFTER_FAILURES) {
            let skips = backoff_skips.entry(id.clone()).or_insert(0);
            if only_missing.is_some() {
                continue;
            }
            if *skips < BACKOFF_SKIP_ROUNDS {
                *skips += 1;
                continue;
            }
            *skips = 0;
        } else {
            backoff_skips.remove(id);
        }
        let carried = carry_over.iter().position(|carried| carried == id);
        let rank = if selected == Some(id.as_str()) {
            (0, 0)
        } else if let Some(position) = carried {
            (1, position as u64)
        } else {
            match entry {
                Some(entry) if entry.this_generation && entry.ok => (3, entry.measured_at),
                Some(entry) if entry.this_generation => (4, index as u64),
                _ => (2, index as u64),
            }
        };
        ranked.push((rank, id));
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, id)| id.clone()).collect()
}

/// NetworkManager 全局 `Metered` 属性（`busctl get-property` 的输出，形如 `u 4`）→ 三态。
/// 1 / 3 是计费与猜测计费，2 / 4 是非计费与猜测非计费，0 是未知。
pub(crate) fn parse_network_manager_metered(output: &str) -> Metered {
    match output.split_whitespace().nth(1) {
        Some("1" | "3") => Metered::Yes,
        Some("2" | "4") => Metered::No,
        _ => Metered::Unavailable,
    }
}

/// Linux：问 NetworkManager 主连接是否计费。没有 NetworkManager（或没有 `busctl`）的系统
/// 一律「不可得」。主连接由 NetworkManager 按它自己管理的连接选出，代理的 TUN 网卡不归它管，
/// 所以读到的是底层网络。
async fn linux_metered() -> Metered {
    let query = tokio::process::Command::new("busctl")
        .args([
            "--system",
            "get-property",
            "org.freedesktop.NetworkManager",
            "/org/freedesktop/NetworkManager",
            "org.freedesktop.NetworkManager",
            "Metered",
        ])
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(Duration::from_secs(3), query).await {
        Ok(Ok(output)) if output.status.success() => {
            parse_network_manager_metered(&String::from_utf8_lossy(&output.stdout))
        }
        _ => Metered::Unavailable,
    }
}

/// 系统原生查询的回答（`None` 是不可得）→ 三态。
pub(crate) const fn metered_of(answer: Option<bool>) -> Metered {
    match answer {
        Some(true) => Metered::Yes,
        Some(false) => Metered::No,
        None => Metered::Unavailable,
    }
}

/// Windows 与 macOS：问系统的联网成本接口。查询会阻塞，放到阻塞线程上并限时；超时或任务异常
/// 一律「不可得」。
async fn native_metered() -> Metered {
    let query = tokio::task::spawn_blocking(polaris_system_integration::network_cost::metered);
    match tokio::time::timeout(Duration::from_secs(3), query).await {
        Ok(Ok(answer)) => metered_of(answer),
        _ => Metered::Unavailable,
    }
}

#[cfg(target_os = "android")]
async fn android_conditions() -> DeviceConditions {
    match crate::runtime::proxy::android_bridge::device_conditions().await {
        Ok(conditions) => DeviceConditions {
            metered: if conditions.metered {
                Metered::Yes
            } else {
                Metered::No
            },
            power_save: conditions.power_save,
        },
        Err(error) => {
            log::debug!("周期测速：设备状况不可得：{error}");
            DeviceConditions::UNAVAILABLE
        }
    }
}

#[cfg(not(target_os = "android"))]
async fn android_conditions() -> DeviceConditions {
    DeviceConditions::UNAVAILABLE
}

/// 拉一次设备状况。拿不到的平台如实报「不可得」，不当作非计费。
async fn device_conditions(platform: Platform) -> DeviceConditions {
    match platform {
        Platform::Android => android_conditions().await,
        Platform::Linux => DeviceConditions {
            metered: linux_metered().await,
            power_save: false,
        },
        Platform::Win | Platform::Mac => DeviceConditions {
            metered: native_metered().await,
            power_save: false,
        },
        // 原生侧的查询尚未就位（本平台的计划也还没有启用）。
        Platform::Ios => DeviceConditions::UNAVAILABLE,
        Platform::Other => DeviceConditions::UNAVAILABLE,
    }
}

/// 等一轮跑完。任务 panic 时返回 `None` 而不把 panic 传出去：收尾必须照常发生，否则在飞标记与
/// 取消槽一直留着，调度器到进程退出都不再发任何一轮。
async fn settle(round: impl Future<Output = ScheduledRound>) -> Option<ScheduledRound> {
    let outcome = AssertUnwindSafe(round).catch_unwind().await;
    if outcome.is_err() {
        log::error!("周期测速：一轮的任务异常终止");
    }
    outcome.ok()
}

struct Shared {
    planner: Planner,
    /// 在飞一轮的取消槽。
    abort: Option<Arc<RoundAbort>>,
    plan: BTreeMap<String, SubPlan>,
    /// 最近一次发给前端的（状态，原因）。
    announced: Option<(String, Option<String>)>,
}

/// 周期测速调度器（进程级单例，`lib.rs` 装配时启动一次并 `manage`）。
pub struct MeasurementScheduler {
    shared: Mutex<Shared>,
    wake: Notify,
    config_stale: AtomicBool,
    started: AtomicBool,
    epoch: tokio::time::Instant,
}

impl Default for MeasurementScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl MeasurementScheduler {
    #[must_use]
    pub fn new() -> Self {
        let epoch = tokio::time::Instant::now();
        Self {
            shared: Mutex::new(Shared {
                planner: Planner::new(
                    Platform::current(),
                    Now {
                        mono: 0,
                        wall: now_ms(),
                    },
                ),
                abort: None,
                plan: BTreeMap::new(),
                announced: None,
            }),
            wake: Notify::new(),
            config_stale: AtomicBool::new(true),
            started: AtomicBool::new(false),
            epoch,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> Now {
        Now {
            mono: u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX),
            wall: now_ms(),
        }
    }

    /// 启动拍循环，并订上会让计划立即变化的两个事件。幂等。
    pub fn start(self: &Arc<Self>, app: AppHandle) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        for event in [EVENT_CONFIG_CHANGED, EVENT_PROXY_STARTED] {
            let this = Arc::clone(self);
            app.listen(event, move |_| {
                this.config_stale.store(true, Ordering::SeqCst);
                this.wake.notify_one();
            });
        }
        let this = Arc::clone(self);
        tauri::async_runtime::spawn(async move { this.run(app).await });
    }
}

#[cfg(mobile)]
impl MeasurementScheduler {
    /// 应用离开前台。在飞的一轮立即取消。
    pub fn on_suspended(&self) {
        let now = self.now();
        let mut shared = self.lock();
        if shared.planner.on_suspended(now) {
            if let Some(abort) = shared.abort.as_ref() {
                abort.abort();
            }
        }
        publish(&shared.planner);
        drop(shared);
        self.wake.notify_one();
    }

    /// 应用回到前台。
    pub fn on_resumed(&self) {
        let now = self.now();
        let mut shared = self.lock();
        shared.planner.on_resumed(now);
        publish(&shared.planner);
        drop(shared);
        self.wake.notify_one();
    }
}

impl MeasurementScheduler {
    async fn run(self: Arc<Self>, app: AppHandle) {
        let ledger = measurement_ledger::global();
        let platform = Platform::current();
        let mut config = Value::Null;
        let mut config_at = None;
        loop {
            let now = self.now();
            let (generation, network_epoch) = {
                let state = app.state::<AppRuntime>();
                let proxy = state.proxy();
                if self.config_stale.swap(false, Ordering::SeqCst)
                    || config_at
                        .is_none_or(|at: u64| now.mono.saturating_sub(at) >= CONFIG_REFRESH_MS)
                {
                    if let Ok(current) = state.config().current() {
                        config = current;
                        config_at = Some(now.mono);
                        // 被自动选择意图指向的订阅即使没开周期测速也进计划。
                        self.lock().plan = effective_plan(&config, platform);
                    }
                }
                (
                    proxy.status().running.then(|| proxy.core_generation()),
                    proxy.network_epoch(),
                )
            };
            let conditions = if self.lock().planner.wants_conditions(now) {
                Some(device_conditions(platform).await)
            } else {
                None
            };
            let (output, targets) = {
                let mut shared = self.lock();
                let Shared { planner, plan, .. } = &mut *shared;
                let output = planner.tick_and_sync_ledger(&TickInput {
                    now: self.now(),
                    user_enabled: user_enabled(&config),
                    generation,
                    plan,
                    network_epoch,
                    conditions,
                    policy: MeteredPolicy::parse(
                        config.get("speedTestMeteredPolicy").and_then(Value::as_str),
                    ),
                    ledger,
                });
                publish(planner);
                if output.abort {
                    if let Some(abort) = shared.abort.as_ref() {
                        abort.abort();
                    }
                }
                let targets = output.start.as_ref().map(|start| {
                    let context = LedgerView::new(&config, generation, network_epoch, platform);
                    let Shared { planner, plan, .. } = &mut *shared;
                    let cap =
                        |id: &str, entry: &LedgerEntry| entry_freshness_cap_ms(plan, id, entry);
                    let current = ledger.current_results(
                        &start
                            .subs
                            .iter()
                            .filter_map(|id| plan.get(id))
                            .flat_map(|sub| sub.members.iter().cloned())
                            .collect::<Vec<_>>(),
                        &context.read(&cap, ledger, now.wall),
                    );
                    let targets = planner.round_targets(
                        start,
                        config.get("selectedServerId").and_then(Value::as_str),
                        &|id| context.entry(ledger, id),
                        &current,
                    );
                    let abort = Arc::new(RoundAbort::default());
                    shared.abort = Some(Arc::clone(&abort));
                    (targets, start.scope.clone(), abort)
                });
                (output, targets)
            };
            if let Some((targets, scope, abort)) = targets {
                let this = Arc::clone(&self);
                let app = app.clone();
                let config = config.clone();
                tauri::async_runtime::spawn(async move {
                    let outcome = settle(async {
                        if targets.is_empty() {
                            // 没有要测的（都已有当前结果，或都在退避里）：按完成收尾。
                            let at = now_ms();
                            ScheduledRound::Ran(RoundReport {
                                started_at: at,
                                ended_at: at,
                                ..RoundReport::default()
                            })
                        } else {
                            run_scheduled_round(
                                &app,
                                ledger,
                                &targets,
                                scope,
                                &abort,
                                FREEZE_GAP_MS,
                            )
                            .await
                        }
                    })
                    .await;
                    let now = this.now();
                    let mut shared = this.lock();
                    shared.abort = None;
                    match &outcome {
                        Some(outcome) => shared.planner.on_round_end(now, outcome),
                        None => shared.planner.on_round_crashed(now),
                    }
                    publish(&shared.planner);
                    let plan = shared.plan.clone();
                    drop(shared);
                    // 每轮收尾时按软上限淘汰一次。
                    ledger.evict(&EvictContext {
                        nodes: &current_server_fingerprints(&config).into_keys().collect(),
                        url_digest: &polaris_updater::sha256_hex(
                            resolve_speed_test_url(&config).as_bytes(),
                        ),
                        now_ms: now.wall,
                        freshness_cap_ms: &|id, entry| entry_freshness_cap_ms(&plan, id, entry),
                    });
                    this.announce(&app, true);
                    this.wake.notify_one();
                });
            }
            self.announce(&app, false);
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(output.sleep_ms)) => {}
                () = self.wake.notified() => {}
            }
        }
    }

    /// 状态（或其原因）变了、或一轮刚收尾时，把计划状态发给前端。
    fn announce(&self, app: &AppHandle, force: bool) {
        let status = status(app);
        let key = (
            status["state"].as_str().unwrap_or_default().to_string(),
            status["reason"].as_str().map(str::to_string),
        );
        let mut shared = self.lock();
        if force || shared.announced.as_ref() != Some(&key) {
            shared.announced = Some(key);
            drop(shared);
            let _ = app.emit(EVENT_SPEED_TEST_SCHEDULE, status);
        }
    }
}

/// 把状态机的读数公布给 [`signals`] 的读取方。
fn publish(planner: &Planner) {
    *SIGNALS.lock().unwrap_or_else(PoisonError::into_inner) = planner.signals();
}

/// 读账本要用的对照面（由一份配置快照与当前运行态现算）。调度器与自动选点共用这一份：
/// 一条结果还能不能用于选点，只有这一套判据。
pub(crate) struct LedgerView {
    generation: Option<u64>,
    fingerprints: BTreeMap<String, String>,
    network_epoch: Option<u64>,
    pub(crate) url_digest: String,
    mobile: bool,
}

impl LedgerView {
    pub(crate) fn new(
        config: &Value,
        generation: Option<u64>,
        network_epoch: Option<u64>,
        platform: Platform,
    ) -> Self {
        Self {
            generation,
            fingerprints: current_server_fingerprints(config),
            network_epoch,
            url_digest: polaris_updater::sha256_hex(resolve_speed_test_url(config).as_bytes()),
            mobile: is_mobile(platform),
        }
    }

    /// 当前配置里逐节点的参数指纹。
    pub(crate) const fn fingerprints(&self) -> &BTreeMap<String, String> {
        &self.fingerprints
    }

    pub(crate) fn read<'a>(
        &'a self,
        freshness_cap_ms: &'a dyn Fn(&str, &LedgerEntry) -> u64,
        ledger: &MeasurementLedger,
        now_ms: u64,
    ) -> ReadContext<'a> {
        ReadContext {
            main_generation: self.generation,
            fingerprints: &self.fingerprints,
            network_epoch: self.network_epoch,
            url_digest: &self.url_digest,
            now_ms,
            foreground_epoch: self.mobile.then(|| ledger.foreground_epoch()),
            freshness_cap_ms,
        }
    }

    pub(crate) fn entry(&self, ledger: &MeasurementLedger, node_id: &str) -> Option<EntryView> {
        let entry = ledger.candidate_entry(node_id, &self.url_digest)?;
        Some(EntryView {
            this_generation: matches!(
                entry.identity.instance,
                crate::commands::speedtest::CoreInstance::Main { generation, .. }
                    if Some(generation) == self.generation
            ),
            ok: entry.measured.is_ok(),
            measured_at: entry.identity.measured_at,
            consecutive_failures: entry.consecutive_failures,
        })
    }
}

/// 计划状态（状态命令与状态事件共用）。调度器没装上时也答得出来：如实报停滞。
pub(crate) fn status(app: &AppHandle) -> Value {
    let core_running = app.state::<AppRuntime>().proxy().status().running;
    match app.try_state::<Arc<MeasurementScheduler>>() {
        Some(scheduler) => {
            let now = scheduler.now();
            let shared = scheduler.lock();
            shared.planner.status(now, core_running, &shared.plan)
        }
        None => json!({ "state": "disabled", "reason": null, "stalled": core_running }),
    }
}

#[cfg(test)]
mod tests;
