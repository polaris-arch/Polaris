//! 自动选点的决策层：选择意图、择优腿的裁决、首轮锁存与对外状态。
//!
//! 自动选点有两条腿，跑在同一个按核世代挂载的心跳任务里（`runtime::proxy::auto_switch`）：
//! - **故障腿**：连通性连续失败后现测候选并换走。决策机是
//!   [`AutoSwitchMachine`](crate::runtime::auto_switch::AutoSwitchMachine)，自动意图下视同启用、
//!   候选收窄到意图指向的订阅。
//! - **择优腿**：读测速账本，在意图指向的订阅里把出口保持在测得可用且够快的节点上。裁决是本模块
//!   的 [`decide`]。
//!
//! 意图与实际出口分列：意图存顶层键 `selectionIntent`，`selectedServerId` 始终是实际出口（自动
//! 选择下即当前胜出节点）。意图键不在 `UserConfig` 里，改它不重新生成内核配置。
//!
//! # 裁决只有一个
//!
//! 后台的择优、用户点「自动选择」时的首次落点、用户点「立即切换」，目标都由 [`decide`] 给出。
//! 后两个是显式路径（[`Facts::explicit`]）：不受冷却、前后台、首轮与测速在飞这几道闸门约束，
//! 候选也不要求能零重启切过去；但**往哪换**与后台是同一套判断，不会把出口换到更差的节点上。
//!
//! # 「更优」的连胜记的是当前出口，不是某个挑战者
//!
//! 换到更快的节点要付出代价（默认断开既有连接），所以要求「持续明显更优」：当前出口**连续
//! [`WIN_STREAK`] 轮**被候选明显胜过才换。连胜数的是「当前出口被胜过的轮数」，每一轮里胜过它的
//! 可以是不同的节点 —— 订阅里常有几个延迟相近的快节点，逐轮最小的那个会换人，按挑战者记连胜
//! 会反复清零、永远换不了。凑满时目标取**最近一轮**里延迟最小的候选。一轮不再有候选明显胜过
//! 当前出口，连胜清零。两轮须来自不同的测速轮，且相隔不少于 [`WIN_MIN_GAP_MS`]。
//!
//! # 时钟
//!
//! 「相隔多久」一律用单调毫秒（账本入账时记下的那一刻、换点时刻、状况开始持续的时刻）：墙钟会被
//! 回拨或前拨。墙钟只出现在给界面看的时间戳里。
//!
//! 本模块不做 I/O：时刻、账本读数与运行态全部由调用方注入；[`decide`] 的全部跨次状态在
//! [`Memory`] 里。两条腿共用一个冷却时刻与一个在飞标志（都在故障腿的决策机上），不会同时换点。

use std::collections::{BTreeMap, VecDeque};

use polaris_helper_proto::Platform;
use serde::Serialize;
use serde_json::Value;

use crate::runtime::auto_switch::{HEARTBEAT_INTERVAL_MS, SWITCH_COOLDOWN_MS};
use crate::runtime::measurement_ledger::{Candidate, Candidates, Exclusion};

/// 自动选点总开关。`false`：设置意图的命令拒绝，已有的自动意图按手动处理（故障切换回到只看
/// `autoSwitchNode`、候选为全部节点）。回退 = 改这个常量后重新出包。
pub(crate) const AUTO_SELECT_ENABLED: bool = true;
/// 择优腿开关。`false`：不因「当前节点测速失败」或「更优」换点；首次落点与限定在订阅内的故障
/// 切换保留。
pub(crate) const BETTER_LEG_ENABLED: bool = true;

/// 绝对容差（毫秒）。与内核 urltest 组的缺省容差相同（内核 `protocol/group/urltest.go` 的
/// `tolerance` 缺省值 50）。
pub(crate) const TOLERANCE_ABS_MS: u32 = 50;
/// 相对容差：当前出口延迟的百分比。经验取值；与绝对容差取较大者。
pub(crate) const TOLERANCE_REL_PERCENT: u32 = 25;
/// 当前出口须连续这么多轮被候选明显胜过。经验取值。
pub(crate) const WIN_STREAK: u32 = 2;
/// 两次计胜的测量至少相隔这么久。经验取值，略小于周期下限 5 分钟：连点两次手动测速凑不满连胜。
pub(crate) const WIN_MIN_GAP_MS: u64 = 4 * 60_000;
/// 计胜时比较的两个读数（当前出口的与候选的）不是同一轮测得时，测量时刻至多相差这么久。
/// 取计胜间隔的一半（严格小于）：同一个当前出口读数因此配不上两次相隔够远的候选读数，
/// 旧的当前出口读数凑不出连胜。
pub(crate) const PAIR_MAX_SKEW_MS: u64 = WIN_MIN_GAP_MS / 2;
/// 任何一次换点之后，这段时间内不因「更优」换点。经验取值。
pub(crate) const MIN_DWELL_MS: u64 = 10 * 60_000;
/// 因「更优」换点的频度上限：任意连续这么长的窗口内至多这么多次。经验取值。
pub(crate) const BETTER_RATE_WINDOW_MS: u64 = 60 * 60_000;
pub(crate) const BETTER_RATE_MAX: usize = 3;
/// 当前出口连续这么多次测速失败才据此换点。须小于周期测速的退避门槛（连续 3 次），保证在
/// 当前出口被退避之前作出判断。经验取值。
pub(crate) const CURRENT_FAILURE_CONFIRM: u32 = 2;
/// 提交连续失败时的重试间隔上限。间隔从冷却时长起、每失败一次翻倍，到这里封顶。取缺省周期的
/// 长度：再长就比等下一轮测速还慢。经验取值。
pub(crate) const COMMIT_BACKOFF_MAX_MS: u64 = 30 * 60_000;
/// 被故障腿换走的节点，这么多个周期内不作为「更优」的挑战者。取周期测速对连续失败节点的复测
/// 间隔（退避中的节点每这么多轮测一次）：两处回答的是同一个问题 ——「一个刚被判定不可用的节点，
/// 多久之后值得再信一次」。
pub(crate) const FAILOVER_BAR_PERIODS: u64 = 4;
/// 上一项的下限：一个限频窗口。窗口内它回不来，「测速好、连通性差」的节点至多每个窗口被换回
/// 一次，远低于限频上限。
pub(crate) const FAILOVER_BAR_MIN_MS: u64 = BETTER_RATE_WINDOW_MS;
/// 读状态时：距最近一次评估超过这么久即判评估停滞（3 个心跳拍）。经验取值。
pub(crate) const STALL_AFTER_MS: u64 = 3 * HEARTBEAT_INTERVAL_MS;
/// 读状态时：没有数据持续超过订阅周期的这么多倍即判长期无数据。经验取值。
pub(crate) const STARVED_PERIODS: u64 = 3;
/// 读状态时：实际出口不在订阅内持续超过订阅周期的这么多倍即判逾期。经验取值。
pub(crate) const OUTSIDE_PERIODS: u64 = 1;

// 取值之间的约束，写成编译期断言：改了任一侧而破坏约束即编不过。
// 当前出口的失败须在它被周期测速退避之前确认；两次计胜的间隔须小于周期下限，否则最短周期下
// 相邻两轮永远凑不满连胜；故障后的排除期与周期测速的退避复测间隔是同一个量。
const _: () = {
    use crate::runtime::measurement_scheduler::{BACKOFF_AFTER_FAILURES, BACKOFF_SKIP_ROUNDS};
    assert!(CURRENT_FAILURE_CONFIRM < BACKOFF_AFTER_FAILURES);
    assert!(WIN_MIN_GAP_MS < *polaris_store::SPEED_TEST_INTERVAL_MINUTES.start() * 60_000);
    assert!(FAILOVER_BAR_PERIODS == BACKOFF_SKIP_ROUNDS as u64 + 1);
    assert!(PAIR_MAX_SKEW_MS * 2 <= WIN_MIN_GAP_MS);
};

/// 两个回退开关的取值。生产只用 [`Switches::PRODUCTION`]；作参数传入使两种取值都能被单测覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Switches {
    pub(crate) master: bool,
    pub(crate) better_leg: bool,
}

impl Switches {
    pub(crate) const PRODUCTION: Self = Self {
        master: AUTO_SELECT_ENABLED,
        better_leg: BETTER_LEG_ENABLED,
    };
}

/// 本平台是否开放自动选择。自动选择靠周期测速供数，所以跟着周期计划走：周期计划未启用的平台
/// （今天是 iOS）不开放，周期计划在那里打开时一并打开。
pub(crate) const fn platform_open(platform: Platform) -> bool {
    crate::runtime::measurement_scheduler::platform_enabled(platform)
}

/// 选择意图。同一时刻只有一个：用户流量只有一个出口 selector。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Intent {
    /// 手动：出口是用户点的那个节点（或直连、阻断）。
    Manual,
    /// 自动，作用域是一个订阅。
    Auto { subscription_id: String },
    /// 配置里存着一个本版本不认识的意图（更新的版本写的模式或作用域）。原样留在盘上，本版本
    /// 不据它做任何事。
    Unrecognized,
}

impl Intent {
    /// 配置里存着的意图（不看回退开关与平台）。
    pub(crate) fn stored(config: &Value) -> Self {
        match polaris_store::selection_intent_subscription(config) {
            Some(subscription_id) => Self::Auto {
                subscription_id: subscription_id.to_string(),
            },
            None if polaris_store::selection_intent_unrecognized(config) => Self::Unrecognized,
            None => Self::Manual,
        }
    }
}

/// 自动选择在这里不可用的原因：总开关关着，或本平台未开放。可用时为 `None`。
/// 设置意图的命令据此拒绝；已有的自动意图据此按手动处理。
pub(crate) const fn closed(switches: Switches, platform: Platform) -> Option<Gate> {
    if !switches.master {
        Some(Gate::SwitchedOff)
    } else if !platform_open(platform) {
        Some(Gate::PlatformNotOpen)
    } else {
        None
    }
}

/// 生效的自动意图指向的订阅：总开关关着或本平台未开放时为 `None`，两条腿都按手动处理。
pub(crate) fn effective_subscription(
    config: &Value,
    switches: Switches,
    platform: Platform,
) -> Option<&str> {
    polaris_store::selection_intent_subscription(config)
        .filter(|_| closed(switches, platform).is_none())
}

/// 订阅的成员：订阅 id 相同的节点 id，按配置里的顺序。
pub(crate) fn subscription_members(config: &Value, subscription_id: &str) -> Vec<String> {
    config
        .get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|server| {
            server.get("subscriptionId").and_then(Value::as_str) == Some(subscription_id)
        })
        .filter_map(|server| server.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// 哪条腿换的点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Leg {
    Failover,
    Select,
}

/// 换点的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Cause {
    /// 当前出口不在订阅内，落到订阅里延迟最小的候选。
    FirstPlacement,
    /// 当前出口连续几轮被候选明显胜过。
    Better,
    /// 当前出口在账本里连续测速失败。
    CurrentFailed,
    /// 连通性心跳连续失败（故障腿）。
    Failover,
    /// 订阅刷新删掉了当前胜出节点，刷新流程改选同订阅的候选。
    WinnerRemoved,
    /// 用户要求立即切换，而当前出口没有新鲜结果可比。
    Requested,
}

impl Cause {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::FirstPlacement => "firstPlacement",
            Self::Better => "better",
            Self::CurrentFailed => "currentFailed",
            Self::Failover => "failover",
            Self::WinnerRemoved => "winnerRemoved",
            Self::Requested => "requested",
        }
    }
}

/// 择优腿没有评估的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gate {
    /// 意图是手动。
    Manual,
    /// 配置里的意图本版本不认识。
    IntentUnrecognized,
    /// 源码里的总开关关着。
    SwitchedOff,
    PlatformNotOpen,
    CoreNotRunning,
    /// 出口正在后台对账（上一次换点的目标与旧出口都没能自证）。
    Reconciling,
    /// 有已保存未应用的出口变更、或范围未知的未保存草稿在等应用：不在这个缝里另立一个意图。
    ConfigPending,
    /// 手机不在前台。
    Background,
    SubscriptionEmpty,
    /// 有测速在飞：等它收口，不拿半份数据选点。
    Measuring,
    /// 首轮未完成。
    WaitingFirstRound,
    /// 首轮未完成，且周期测速的全局总开关关着：只在手动测速后有数据。
    PeriodicDisabled,
    /// 首轮未完成，且周期测速因计费网络暂停。
    MeteredPaused,
    /// 首轮未完成，且周期测速因省电暂停。
    PowerSave,
}

impl Gate {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::IntentUnrecognized => "intentUnrecognized",
            Self::SwitchedOff => "switchedOff",
            Self::PlatformNotOpen => "platformNotOpen",
            Self::CoreNotRunning => "coreNotRunning",
            Self::Reconciling => "reconciling",
            Self::ConfigPending => "configPending",
            Self::Background => "background",
            Self::SubscriptionEmpty => "subscriptionEmpty",
            Self::Measuring => "measuring",
            Self::WaitingFirstRound => "waitingFirstRound",
            Self::PeriodicDisabled => "periodicDisabled",
            Self::MeteredPaused => "meteredPaused",
            Self::PowerSave => "powerSave",
        }
    }
}

/// 周期测速没在供数的原因（由调度器的状态折算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Starved {
    PeriodicDisabled,
    MeteredPaused,
    PowerSave,
}

impl Starved {
    const fn as_str(self) -> &'static str {
        match self {
            Self::PeriodicDisabled => "periodicDisabled",
            Self::MeteredPaused => "meteredPaused",
            Self::PowerSave => "powerSave",
        }
    }

    const fn gate(self) -> Gate {
        match self {
            Self::PeriodicDisabled => Gate::PeriodicDisabled,
            Self::MeteredPaused => Gate::MeteredPaused,
            Self::PowerSave => Gate::PowerSave,
        }
    }
}

/// 当前出口没有新鲜结果的原因。没有数据不等于差：后台不据此换点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NoResult {
    Expired,
    /// 没有可用于选点的测量（从未测过、只有非候选路径或未连接测量的结果）。
    Unmeasured,
    /// 最近一轮起测前被预筛跳过（未纳入）。
    Skipped,
    /// 测出了值，但读回判定承载它的不是这个节点：量到的不是它。
    Mismatch,
}

impl NoResult {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "currentExpired",
            Self::Unmeasured => "currentUnmeasured",
            Self::Skipped => "currentSkipped",
            Self::Mismatch => "currentMismatch",
        }
    }
}

/// 当前出口在账本里的状况。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Current {
    /// 不属于意图指向的订阅（刚选了自动、被兜底到了别处、直连或阻断、未选）。
    Outside,
    /// 有新鲜的值：延迟、测出它的那一轮、入账的单调时刻。
    Fresh {
        latency_ms: u32,
        run: u64,
        measured_mono: u64,
    },
    /// 最新一条是失败，附连续失败次数。
    Failed {
        consecutive: u32,
    },
    NoResult(NoResult),
}

/// 离换点还差什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lacking {
    /// 连胜轮数不够。
    Streak,
    /// 有新的一轮更优测量，但与上一次计胜相隔太近，没有计入。
    Gap,
    /// 当前出口与候选的读数不是同一轮测得、且时刻相差太远，没法比。
    Pair,
    /// 距上次换点不够久。
    Dwell,
    /// 窗口内因「更优」换点的次数已到上限。
    RateLimit,
    /// 两条腿共用的冷却还没过。
    Cooldown,
}

impl Lacking {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Streak => "streak",
            Self::Gap => "gap",
            Self::Pair => "pair",
            Self::Dwell => "dwell",
            Self::RateLimit => "rateLimit",
            Self::Cooldown => "cooldown",
        }
    }
}

/// 一次提交没有换成的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitFailure {
    /// 目标没能通过运行态读回，已恢复原出口。
    Failed,
    /// 提交那一刻目标已不满足零重启热切的条件。
    NotEligible,
    /// 目标与原出口都没能自证，出口交给了后台对账。
    Reconciling,
    /// 让位给了手动选择、配置变更、生命周期事务或新的核世代。不算失败，不计入连续失败。
    Yielded,
}

impl CommitFailure {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "commitFailed",
            Self::NotEligible => "commitNotEligible",
            Self::Reconciling => "commitReconciling",
            Self::Yielded => "commitYielded",
        }
    }

    /// 是否计入连续失败（并因此拉长重试间隔）。
    pub(crate) const fn counts(self) -> bool {
        !matches!(self, Self::Yielded)
    }
}

/// 评估了、不换的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hold {
    /// 当前出口有新鲜结果，没有明显更优的候选。
    Settled,
    /// 当前出口被候选明显胜过，换点条件还没齐。
    Pending(Lacking),
    /// 订阅里没有可用候选。不转直连，不借别的订阅。
    NoCandidates,
    /// 有候选，但切过去都得整核重启；后台不重启。
    NeedsRestart,
    /// 当前出口测速失败，次数还不到确认门槛。
    CurrentFailedOnce,
    /// 当前出口没有新鲜结果可比。
    CurrentNoResult(NoResult),
    /// 当前出口没有新鲜结果，而周期测速此刻没在供数。
    Starved(Starved),
    /// 该换（首次落点或当前节点测速失败），冷却还没过。
    Cooldown,
    /// 择优腿开关关着。
    BetterLegOff,
    /// 裁决为换点，正在提交。
    Committing,
    /// 裁决为换点，提交没有换成。
    CommitFailed(CommitFailure),
}

impl Hold {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::Pending(_) => "challengerPending",
            Self::NoCandidates => "noCandidates",
            Self::NeedsRestart => "needsRestart",
            Self::CurrentFailedOnce => "currentFailedOnce",
            Self::CurrentNoResult(reason) => reason.as_str(),
            Self::Starved(starved) => starved.as_str(),
            Self::Cooldown => "cooldown",
            Self::BetterLegOff => "betterLegOff",
            Self::Committing => "committing",
            Self::CommitFailed(failure) => failure.as_str(),
        }
    }
}

/// 一次评估的裁决。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    NotEvaluated(Gate),
    Hold(Hold),
    Switch {
        target: String,
        latency_ms: u32,
        cause: Cause,
    },
}

/// 一次评估的全部输入。
pub(crate) struct Facts<'a> {
    /// 配置里存着的意图。
    pub(crate) intent: &'a Intent,
    pub(crate) switches: Switches,
    pub(crate) platform_open: bool,
    pub(crate) core_running: bool,
    /// 显式路径（用户点「自动选择」时的首次落点、「立即切换」）。见模块文档。
    pub(crate) explicit: bool,
    /// 手机是否在前台；桌面恒为真。
    pub(crate) foreground: bool,
    pub(crate) reconciling: bool,
    pub(crate) config_pending: bool,
    /// 单飞闸上有测速在飞。
    pub(crate) measuring: bool,
    /// 订阅的成员数。
    pub(crate) members: usize,
    /// 首轮是否已完成（[`Memory::latch_first_round`] 的结果）。
    pub(crate) first_round_done: bool,
    /// 周期测速此刻没在供数的原因。
    pub(crate) starved: Option<Starved>,
    pub(crate) current: Current,
    /// 当前出口的节点 id（连胜按当前出口记）。
    pub(crate) current_id: Option<&'a str>,
    /// 候选：账本里该订阅的可选点，去掉当前出口与过不了资格筛选的，按配置顺序。
    pub(crate) candidates: &'a [Candidate],
    /// Fresh candidates that require a restart: comparison evidence only in background.
    pub(crate) restart_candidates: &'a [Candidate],
    /// 因「切过去要整核重启」被筛掉的可选点个数。
    pub(crate) restart_blocked: usize,
    /// 单调毫秒。
    pub(crate) now: u64,
    /// 两条腿共用的冷却起点：最近一次换点尝试的时刻（成败都算）。
    pub(crate) last_attempt_at: Option<u64>,
}

/// 连胜记录：当前出口连续被候选明显胜过的轮数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Streak {
    current: String,
    pub(crate) wins: u32,
    /// 最近一轮里胜过当前出口的延迟最小者。
    pub(crate) leader: String,
    /// 最近一次计胜依据的那次测量：运行号与入账的单调时刻。
    last_run: u64,
    last_mono: u64,
    /// 最近一轮更优测量因相隔太近没有计入。
    gap_rejected: bool,
}

/// 记忆量的有效范围：其中任何一项变了，连胜与首轮锁存清零。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Epoch {
    pub(crate) subscription: String,
    pub(crate) generation: u64,
    pub(crate) network_epoch: Option<u64>,
    /// 前台代次；桌面为 `None`。
    pub(crate) foreground_epoch: Option<u64>,
}

/// 故障排除的观测锚点，与择优的四字段有效范围分开。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BarContext {
    known_network_epoch: Option<u64>,
    metered_change_epoch: Option<u64>,
}

/// [`decide`] 的跨次状态。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Memory {
    epoch: Option<Epoch>,
    first_round_done: bool,
    /// 进入当前范围那一刻调度器的轮次序号：此后收尾的一轮才算数。
    round_baseline: u64,
    streak: Option<Streak>,
    /// 最近一次换点成功的时刻（两条腿、首次落点都算）。
    last_switch_at: Option<u64>,
    /// 窗口内因「更优」换点的时刻。
    better_switches: VecDeque<u64>,
    /// 提交连续失败的次数（成功即归零）。
    commit_failures: u32,
    /// 被故障腿换走的节点 → 排除到何时（单调毫秒）。
    barred: BTreeMap<String, u64>,
    bar_context: BarContext,
}

impl Memory {
    /// 对齐有效范围。范围变了即清连胜与首轮锁存；驻留与限频不随它清（它们约束的是换点的频度，
    /// 换核世代不该让频度约束归零）。
    ///
    /// 提交连续失败随核世代清；故障排除只随订阅与确证的网络 / 计费变化清。
    /// 前台代次仍使连胜与首轮锁存失效，但不代表网络变化。
    pub(crate) fn observe_epoch(&mut self, epoch: &Epoch, round_serial: u64) {
        if self
            .epoch
            .as_ref()
            .is_some_and(|previous| previous.subscription != epoch.subscription)
        {
            self.barred.clear();
            self.bar_context = BarContext::default();
        }
        if let Some(network) = epoch.network_epoch {
            if self
                .bar_context
                .known_network_epoch
                .is_some_and(|previous| previous != network)
            {
                self.barred.clear();
            }
            self.bar_context.known_network_epoch = Some(network);
        }
        let Some(previous) = self.epoch.as_ref() else {
            self.enter(epoch, round_serial);
            return;
        };
        if previous == epoch {
            return;
        }
        if previous.generation != epoch.generation {
            self.commit_failures = 0;
        }
        self.enter(epoch, round_serial);
    }

    /// 三条生产消费路径共同对齐。首次计费读数只立基线；旧读数不能倒退水位，
    /// 清除时立即推进水位，避免重复消费同一变化后误清新建的排除。
    pub(crate) fn observe_context(
        &mut self,
        epoch: &Epoch,
        round_serial: u64,
        metered_change_epoch: u64,
    ) {
        self.observe_epoch(epoch, round_serial);
        match self.bar_context.metered_change_epoch {
            Some(previous) if metered_change_epoch > previous => {
                self.barred.clear();
                self.bar_context.metered_change_epoch = Some(metered_change_epoch);
            }
            None => self.bar_context.metered_change_epoch = Some(metered_change_epoch),
            Some(_) => {}
        }
    }

    fn enter(&mut self, epoch: &Epoch, round_serial: u64) {
        self.epoch = Some(epoch.clone());
        self.first_round_done = false;
        self.round_baseline = round_serial;
        self.streak = None;
    }

    /// 意图回到手动（或不再生效）：范围作废，下次进入自动时从头来。
    pub(crate) fn leave(&mut self) {
        self.epoch = None;
        self.first_round_done = false;
        self.streak = None;
        self.commit_failures = 0;
        self.barred.clear();
        self.bar_context = BarContext::default();
    }

    /// 首轮是否完成，满足任一即成立并在当前范围内锁存：账本对订阅成员给出「每个可测的节点都有
    /// 当前结果」；或调度器报告该订阅在进入当前范围之后已有一轮收尾。
    ///
    /// 锁存而不持续要求：一轮之后个别结果过期不该让闸门回落。
    pub(crate) fn latch_first_round(
        &mut self,
        covers_all_testable: bool,
        last_round_serial: Option<u64>,
    ) -> bool {
        if covers_all_testable
            || last_round_serial.is_some_and(|serial| serial > self.round_baseline)
        {
            self.first_round_done = true;
        }
        self.first_round_done
    }

    /// 一次换点成功之后记账。
    pub(crate) fn record_switch(&mut self, cause: Cause, now: u64) {
        self.last_switch_at = Some(now);
        self.streak = None;
        self.commit_failures = 0;
        if cause == Cause::Better {
            self.better_switches.push_back(now);
        }
    }

    /// 一次提交没有换成。
    pub(crate) fn record_commit_failure(&mut self, failure: CommitFailure) {
        if failure.counts() {
            self.commit_failures = self.commit_failures.saturating_add(1);
        }
    }

    /// 故障腿把出口从 `node_id` 换走了：这个节点在一段时间内不作为「更优」的挑战者。
    pub(crate) fn bar(&mut self, node_id: &str, now: u64, period_ms: u64) {
        let length = period_ms
            .saturating_mul(FAILOVER_BAR_PERIODS)
            .max(FAILOVER_BAR_MIN_MS);
        self.barred
            .insert(node_id.to_string(), now.saturating_add(length));
    }

    pub(crate) fn is_barred(&self, node_id: &str, now: u64) -> bool {
        self.barred.get(node_id).is_some_and(|until| now < *until)
    }

    /// 此刻仍被排除的节点与各自剩余的毫秒数。
    pub(crate) fn barred(&self, now: u64) -> Vec<(&str, u64)> {
        self.barred
            .iter()
            .filter(|(_, until)| now < **until)
            .map(|(id, until)| (id.as_str(), *until - now))
            .collect()
    }

    /// 窗口内因「更优」换点的次数。
    pub(crate) fn better_switches_in_window(&self, now: u64) -> usize {
        self.better_switches
            .iter()
            .filter(|at| now.saturating_sub(**at) < BETTER_RATE_WINDOW_MS)
            .count()
    }

    pub(crate) const fn commit_failures(&self) -> u32 {
        self.commit_failures
    }

    pub(crate) fn streak(&self) -> Option<&Streak> {
        self.streak.as_ref()
    }
}

/// 最近一次换点尝试之后要等多久才能再试：平时是两条腿共用的冷却；提交连续失败时每失败一次
/// 翻倍，到 [`COMMIT_BACKOFF_MAX_MS`] 封顶。
pub(crate) fn retry_after_ms(commit_failures: u32) -> u64 {
    SWITCH_COOLDOWN_MS
        .saturating_mul(1u64 << commit_failures.min(16))
        .min(COMMIT_BACKOFF_MAX_MS.max(SWITCH_COOLDOWN_MS))
}

/// 候选按延迟从小到大排；并列时保持入参顺序（即配置顺序）。
pub(crate) fn rank(candidates: &[Candidate]) -> Vec<&Candidate> {
    let mut ranked: Vec<&Candidate> = candidates.iter().collect();
    ranked.sort_by_key(|candidate| candidate.latency_ms);
    ranked
}

/// 候选是否明显更优：延迟比当前出口低出容差（绝对值与相对值取较大者）以上。
pub(crate) fn clearly_better(challenger_ms: u32, current_ms: u32) -> bool {
    let relative = u64::from(current_ms) * u64::from(TOLERANCE_REL_PERCENT) / 100;
    let margin = u64::from(TOLERANCE_ABS_MS).max(relative);
    u64::from(challenger_ms) + margin < u64::from(current_ms)
}

/// **前置闸门**（纯函数）：不满足即不评估。次序即优先级；候选与当前出口的状况不参与。
/// 显式路径只过与「能不能换」有关的几道：对账、待应用配置、前后台、测速在飞与首轮是后台自己
/// 该等的事，不拦用户的显式操作。
pub(crate) fn gate(facts: &Facts<'_>) -> Option<Gate> {
    match facts.intent {
        Intent::Manual => return Some(Gate::Manual),
        Intent::Unrecognized => return Some(Gate::IntentUnrecognized),
        Intent::Auto { .. } => {}
    }
    if !facts.switches.master {
        return Some(Gate::SwitchedOff);
    }
    if !facts.platform_open {
        return Some(Gate::PlatformNotOpen);
    }
    if !facts.core_running {
        return Some(Gate::CoreNotRunning);
    }
    if !facts.explicit {
        if facts.reconciling {
            return Some(Gate::Reconciling);
        }
        if facts.config_pending {
            return Some(Gate::ConfigPending);
        }
        if !facts.foreground {
            return Some(Gate::Background);
        }
    }
    if facts.members == 0 {
        return Some(Gate::SubscriptionEmpty);
    }
    if !facts.explicit {
        if facts.measuring {
            return Some(Gate::Measuring);
        }
        if !facts.first_round_done {
            return Some(facts.starved.map_or(Gate::WaitingFirstRound, Starved::gate));
        }
    }
    None
}

/// **择优的裁决**（除 `memory` 外无副作用）。先过 [`gate`]，再按当前出口的状况分四种情形。
/// 后台与显式路径共用，差别见模块文档与各分支。
///
/// 换点成功与否由调用方执行后回填：成功调 [`Memory::record_switch`]，没换成调
/// [`Memory::record_commit_failure`]；连胜在没换成时保留。
pub(crate) fn decide(facts: &Facts<'_>, memory: &mut Memory) -> Decision {
    if let Some(gate) = gate(facts) {
        return Decision::NotEvaluated(gate);
    }

    let cooling = !facts.explicit
        && facts.last_attempt_at.is_some_and(|at| {
            facts.now.saturating_sub(at) < retry_after_ms(memory.commit_failures)
        });
    let ranked = rank(facts.candidates);
    // 被故障腿换走不久的节点：不作为「更优」的挑战者；别无选择时（首次落点、当前节点已失败）
    // 仍可落到它上面，总比留在订阅外或一个测不通的节点上强。
    let mut trusted = ranked
        .iter()
        .copied()
        .find(|candidate| !memory.is_barred(&candidate.node_id, facts.now));
    let any = trusted.or_else(|| ranked.first().copied());
    // A slow Hot candidate must not erase evidence for a materially better restart-only
    // candidate. Prefer a useful Hot challenger; otherwise keep the same comparable
    // evidence that the explicit command will consume, without admitting a restart.
    if let Current::Fresh { latency_ms, .. } = facts.current {
        if trusted.is_none_or(|candidate| !clearly_better(candidate.latency_ms, latency_ms)) {
            trusted = rank(facts.restart_candidates)
                .into_iter()
                .find(|candidate| {
                    !memory.is_barred(&candidate.node_id, facts.now)
                        && clearly_better(candidate.latency_ms, latency_ms)
                })
                .or(trusted);
        }
    }
    // 没有候选可换：区分「订阅里根本没有可用节点」与「有，但切过去都得重启」。
    let nothing_to_switch_to = if facts.restart_blocked > 0 {
        Hold::NeedsRestart
    } else {
        Hold::NoCandidates
    };
    let switch_to = |candidate: &Candidate, cause: Cause| {
        if !facts.explicit
            && facts
                .restart_candidates
                .iter()
                .any(|restart| restart.node_id == candidate.node_id)
        {
            Decision::Hold(Hold::NeedsRestart)
        } else {
            Decision::Switch {
                target: candidate.node_id.clone(),
                latency_ms: candidate.latency_ms,
                cause,
            }
        }
    };

    let (current_ms, current_run, current_mono) = match facts.current {
        // 当前出口不在订阅内：落到延迟最小的候选，不受连胜与驻留约束。
        Current::Outside => {
            memory.streak = None;
            return match any {
                None => Decision::Hold(nothing_to_switch_to),
                Some(_) if cooling => Decision::Hold(Hold::Cooldown),
                Some(candidate) => switch_to(candidate, Cause::FirstPlacement),
            };
        }
        // 当前出口最新一条是失败：后台要达到确认次数才据此换走；显式路径一次失败即可。
        Current::Failed { consecutive } => {
            memory.streak = None;
            let confirm = if facts.explicit {
                1
            } else {
                CURRENT_FAILURE_CONFIRM
            };
            if consecutive < confirm {
                return Decision::Hold(Hold::CurrentFailedOnce);
            }
            if !facts.switches.better_leg {
                return Decision::Hold(Hold::BetterLegOff);
            }
            return match any {
                None => Decision::Hold(nothing_to_switch_to),
                Some(_) if cooling => Decision::Hold(Hold::Cooldown),
                Some(candidate) => switch_to(candidate, Cause::CurrentFailed),
            };
        }
        // 当前出口没有新鲜结果。后台不换：没有数据不等于差，调度器每轮把当前出口排在最前，
        // 下一轮就会有数据；周期测速此刻没在供数时如实说是它挡住了。显式路径可以换：用户
        // 要求现在就换，而当前出口拿不出任何可比的结果。
        Current::NoResult(reason) => {
            memory.streak = None;
            if !facts.explicit {
                return Decision::Hold(
                    facts
                        .starved
                        .map_or(Hold::CurrentNoResult(reason), Hold::Starved),
                );
            }
            return match any {
                None => Decision::Hold(nothing_to_switch_to),
                Some(candidate) => switch_to(candidate, Cause::Requested),
            };
        }
        Current::Fresh {
            latency_ms,
            run,
            measured_mono,
        } => (latency_ms, run, measured_mono),
    };

    // 当前出口有新鲜的值：须连续几轮被候选明显胜过才换。
    let (Some(leader), Some(current_id)) = (trusted, facts.current_id) else {
        memory.streak = None;
        return Decision::Hold(Hold::Settled);
    };
    if !clearly_better(leader.latency_ms, current_ms) {
        memory.streak = None;
        return Decision::Hold(Hold::Settled);
    }
    // 两个读数要可比：同一轮测得，或测量时刻足够近。当前出口的读数是更早一轮的，不计胜也不
    // 清连胜（等它被重测）。
    if leader.run != current_run && leader.measured_mono.abs_diff(current_mono) >= PAIR_MAX_SKEW_MS
    {
        return Decision::Hold(Hold::Pending(Lacking::Pair));
    }
    match memory
        .streak
        .as_mut()
        .filter(|streak| streak.current == current_id)
    {
        Some(streak) => {
            // 同一轮测量不重复计胜；与上一次计胜相隔太近的也不计。
            if leader.run != streak.last_run {
                if leader.measured_mono.saturating_sub(streak.last_mono) >= WIN_MIN_GAP_MS {
                    streak.wins += 1;
                    streak.last_run = leader.run;
                    streak.last_mono = leader.measured_mono;
                    streak.gap_rejected = false;
                } else {
                    streak.gap_rejected = true;
                }
            }
            streak.leader.clone_from(&leader.node_id);
        }
        None => {
            memory.streak = Some(Streak {
                current: current_id.to_string(),
                wins: 1,
                leader: leader.node_id.clone(),
                last_run: leader.run,
                last_mono: leader.measured_mono,
                gap_rejected: false,
            });
        }
    }
    if !facts.switches.better_leg {
        return Decision::Hold(Hold::BetterLegOff);
    }
    let streak = memory.streak.as_ref();
    if streak.is_none_or(|streak| streak.wins < WIN_STREAK) {
        return Decision::Hold(Hold::Pending(
            if streak.is_some_and(|streak| streak.gap_rejected) {
                Lacking::Gap
            } else {
                Lacking::Streak
            },
        ));
    }
    if memory
        .last_switch_at
        .is_some_and(|at| facts.now.saturating_sub(at) < MIN_DWELL_MS)
    {
        return Decision::Hold(Hold::Pending(Lacking::Dwell));
    }
    memory
        .better_switches
        .retain(|at| facts.now.saturating_sub(*at) < BETTER_RATE_WINDOW_MS);
    if memory.better_switches.len() >= BETTER_RATE_MAX {
        return Decision::Hold(Hold::Pending(Lacking::RateLimit));
    }
    if cooling {
        return Decision::Hold(Hold::Pending(Lacking::Cooldown));
    }
    switch_to(leader, Cause::Better)
}

/// 一次评估的摘要：订阅成员在账本里的分布。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Counts {
    /// 可选点（含当前出口，若它可选）。
    pub(crate) selectable: usize,
    pub(crate) failed: usize,
    /// 过期（核换代、节点参数改过、切网、超过新鲜期上限、前台代次变过）。
    pub(crate) stale: usize,
    /// 没有可用于选点的测量（从未测过、非候选路径、未连接测量、读回判定量到的不是它）。
    pub(crate) unmeasured: usize,
    /// 最近一轮起测前被预筛跳过（未纳入）。
    pub(crate) skipped: usize,
    /// 可选点里承载出站没有被读回证实的个数。它们照常可选，只计数。
    pub(crate) unverified: usize,
    /// 可选点里因「切过去要整核重启」不进候选的个数。
    pub(crate) needs_restart: usize,
}

impl Counts {
    pub(crate) fn of(ledger: &Candidates, needs_restart: usize) -> Self {
        let mut counts = Self {
            selectable: ledger.selectable.len(),
            unverified: ledger.unverified.len(),
            needs_restart,
            ..Self::default()
        };
        for (_, exclusion) in &ledger.excluded {
            match exclusion {
                Exclusion::Failed { .. } => counts.failed += 1,
                Exclusion::Stale(_) => counts.stale += 1,
                Exclusion::Skipped(_) => counts.skipped += 1,
                Exclusion::NeverMeasured
                | Exclusion::NonCandidatePath
                | Exclusion::BindingMismatch
                | Exclusion::Disconnected => counts.unmeasured += 1,
            }
        }
        counts
    }

    /// 可选点全部没有被读回证实：读回通道在本机很可能不可用。
    pub(crate) const fn all_unverified(&self) -> bool {
        self.selectable > 0 && self.unverified == self.selectable
    }
}

/// 当前出口在账本里的状况。`ledger` 是对订阅成员取的候选；出口不在成员里即 [`Current::Outside`]。
pub(crate) fn classify_current(
    current_id: Option<&str>,
    members: &[String],
    ledger: &Candidates,
) -> Current {
    let Some(current_id) = current_id.filter(|id| members.iter().any(|member| member == id)) else {
        return Current::Outside;
    };
    if let Some(candidate) = ledger
        .selectable
        .iter()
        .find(|candidate| candidate.node_id == current_id)
    {
        return Current::Fresh {
            latency_ms: candidate.latency_ms,
            run: candidate.run,
            measured_mono: candidate.measured_mono,
        };
    }
    match ledger
        .excluded
        .iter()
        .find(|(id, _)| id == current_id)
        .map(|(_, exclusion)| exclusion)
    {
        Some(Exclusion::Failed { consecutive, .. }) => Current::Failed {
            consecutive: *consecutive,
        },
        Some(Exclusion::Stale(_)) => Current::NoResult(NoResult::Expired),
        Some(Exclusion::Skipped(_)) => Current::NoResult(NoResult::Skipped),
        Some(Exclusion::BindingMismatch) => Current::NoResult(NoResult::Mismatch),
        Some(Exclusion::NeverMeasured | Exclusion::NonCandidatePath | Exclusion::Disconnected)
        | None => Current::NoResult(NoResult::Unmeasured),
    }
}

// ════════════════ 对外状态（状态命令与状态事件共用一个载荷）════════════════

/// 一次换点的记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SwitchRecord {
    /// Unix 毫秒（只用于展示）。
    pub(crate) at: u64,
    pub(crate) leg: Leg,
    pub(crate) cause: Cause,
    pub(crate) from_id: Option<String>,
    pub(crate) from_name: Option<String>,
    pub(crate) to_id: String,
    pub(crate) to_name: String,
    /// 换点那一刻两者的延迟；旧出口没有新鲜的值时为空。
    pub(crate) from_latency_ms: Option<u32>,
    pub(crate) to_latency_ms: Option<u32>,
    /// 胜出依据是否没有被读回证实；故障腿现测的结果不带这个结论，为空。
    pub(crate) unverified: Option<bool>,
}

/// 最近一次没有换成的提交。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitRecord {
    /// Unix 毫秒（只用于展示）。
    pub(crate) at: u64,
    pub(crate) to_id: String,
    /// `commitFailed` / `commitNotEligible` / `commitReconciling` / `commitYielded`。
    pub(crate) outcome: &'static str,
}

/// 最近一次评估。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evaluation {
    /// Unix 毫秒（只用于展示）。
    pub(crate) at: u64,
    /// 单调毫秒：停滞与「持续多久」据它算。
    pub(crate) at_mono: u64,
    pub(crate) subscription: String,
    pub(crate) decision: Decision,
    pub(crate) counts: Counts,
}

/// 进程级的状态槽：不随核世代清空，不落盘。
#[derive(Debug, Default)]
pub(crate) struct Slot {
    pub(crate) memory: Memory,
    pub(crate) last_switch: Option<SwitchRecord>,
    pub(crate) last_commit_failure: Option<CommitRecord>,
    pub(crate) evaluation: Option<Evaluation>,
    /// 本次运行以来两条腿各自的换点次数。
    pub(crate) failover_switches: u64,
    pub(crate) select_switches: u64,
    /// 「没有数据可据以选点」「实际出口不在订阅内」各自从何时起持续（单调毫秒）。
    pub(crate) starved_since: Option<u64>,
    pub(crate) outside_since: Option<u64>,
    /// 当前这个自动意图被写入的时刻（单调毫秒）：还没有任何一次评估时，停滞从它算起。
    pub(crate) intent_set_mono: Option<u64>,
    /// 最近一次记过日志的裁决（去重）。
    pub(crate) logged: Option<String>,
}

impl Slot {
    /// 意图不再是（这个）自动选择：清掉决策记忆里随范围走的部分与最近一次评估。返回是否确有残留。
    /// 上次换点、计数、驻留与限频不清：它们记的是本次运行以来发生过的事。
    pub(crate) fn leave(&mut self) -> bool {
        let had = self.evaluation.is_some() || self.memory.epoch.is_some();
        self.memory.leave();
        self.evaluation = None;
        self.last_commit_failure = None;
        self.starved_since = None;
        self.outside_since = None;
        self.intent_set_mono = None;
        had
    }

    /// 记下一次评估的裁决，并维护两个「从何时起持续」的时刻。
    ///
    /// 「没有数据可据以选点」覆盖：首轮未完成、订阅里没有候选、当前出口没有新鲜结果（过期、
    /// 未测、未纳入、读回不符）、周期测速没在供数。
    pub(crate) fn record_evaluation(&mut self, evaluation: Evaluation, outside: bool) {
        let starved = matches!(
            evaluation.decision,
            Decision::NotEvaluated(
                Gate::WaitingFirstRound
                    | Gate::PeriodicDisabled
                    | Gate::MeteredPaused
                    | Gate::PowerSave
            ) | Decision::Hold(Hold::NoCandidates | Hold::CurrentNoResult(_) | Hold::Starved(_))
        );
        self.starved_since = starved.then(|| self.starved_since.unwrap_or(evaluation.at_mono));
        self.outside_since = outside.then(|| self.outside_since.unwrap_or(evaluation.at_mono));
        self.evaluation = Some(evaluation);
    }

    /// 一次换点成功之后记账：换点记录、计数与决策记忆。评估里「正在提交」的裁决落成换点。
    pub(crate) fn record_switch(&mut self, record: SwitchRecord, now_mono: u64) {
        match record.leg {
            Leg::Failover => self.failover_switches += 1,
            Leg::Select => self.select_switches += 1,
        }
        self.memory.record_switch(record.cause, now_mono);
        self.last_commit_failure = None;
        if let Some(evaluation) = self.evaluation.as_mut() {
            if evaluation.decision == Decision::Hold(Hold::Committing) {
                evaluation.decision = Decision::Switch {
                    target: record.to_id.clone(),
                    latency_ms: record.to_latency_ms.unwrap_or_default(),
                    cause: record.cause,
                };
            }
        }
        self.last_switch = Some(record);
    }

    /// 一次提交没有换成：评估里「正在提交」的裁决落成没换成的原因，连续失败计数随之推进。
    pub(crate) fn record_commit_failure(&mut self, failure: CommitFailure, to_id: &str, at: u64) {
        self.memory.record_commit_failure(failure);
        self.last_commit_failure = Some(CommitRecord {
            at,
            to_id: to_id.to_string(),
            outcome: failure.as_str(),
        });
        if let Some(evaluation) = self.evaluation.as_mut() {
            if evaluation.decision == Decision::Hold(Hold::Committing) {
                evaluation.decision = Decision::Hold(Hold::CommitFailed(failure));
            }
        }
    }
}

/// 当前出口的依据。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Basis {
    /// `fresh` / `failed` / `expired` / `unmeasured` / `skipped` / `mismatch`。
    pub(crate) state: &'static str,
    pub(crate) latency_ms: Option<u32>,
    pub(crate) measured_at: Option<u64>,
    /// 读回结论：`confirmed` / `unverified`；没有新鲜的值时为空。
    pub(crate) binding: Option<&'static str>,
}

impl Basis {
    /// 某个节点在账本里的依据。`ledger` 须是对含该节点的列表取的候选。
    pub(crate) fn of(node_id: &str, ledger: &Candidates) -> Self {
        if let Some(candidate) = ledger
            .selectable
            .iter()
            .find(|candidate| candidate.node_id == node_id)
        {
            return Self {
                state: "fresh",
                latency_ms: Some(candidate.latency_ms),
                measured_at: Some(candidate.measured_at),
                binding: Some(if ledger.unverified.iter().any(|id| id == node_id) {
                    "unverified"
                } else {
                    "confirmed"
                }),
            };
        }
        let state = match ledger
            .excluded
            .iter()
            .find(|(id, _)| id == node_id)
            .map(|(_, exclusion)| exclusion)
        {
            Some(Exclusion::Failed { .. }) => "failed",
            Some(Exclusion::Stale(_)) => "expired",
            Some(Exclusion::Skipped(_)) => "skipped",
            Some(Exclusion::BindingMismatch) => "mismatch",
            _ => "unmeasured",
        };
        Self {
            state,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct IntentView {
    /// `manual` / `auto` / `unrecognized`。
    pub(crate) mode: &'static str,
    /// 自动选择的作用域；今天只有 `subscription`。
    pub(crate) scope: Option<&'static str>,
    pub(crate) subscription_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExitView {
    /// 实际出口：节点 id、`__direct__`、`__block__`，或未选。
    pub(crate) server_id: Option<String>,
    /// 是否属于意图指向的订阅；非自动意图下为空。
    pub(crate) in_subscription: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EvaluationView {
    pub(crate) at: u64,
    /// `held` / `switch` / `notEvaluated`。
    pub(crate) outcome: &'static str,
    pub(crate) reason: &'static str,
    pub(crate) counts: Counts,
}

/// 当前出口被候选明显胜过的进度。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChallengerView {
    /// 最近一轮里胜过当前出口的延迟最小者；凑满条件时换到的就是当轮的这一个。
    pub(crate) server_id: String,
    /// 当前出口已连续被胜过的轮数，与须凑满的轮数。
    pub(crate) wins: u32,
    pub(crate) wins_required: u32,
    /// 离换点还差什么：`streak` / `gap` / `pair` / `dwell` / `rateLimit` / `cooldown`。
    pub(crate) lacking: Option<&'static str>,
}

/// 因近期被故障切换换走而暂时不作为挑战者的节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BarredView {
    pub(crate) server_id: String,
    pub(crate) remaining_ms: u64,
}

/// 提交的状况。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitView {
    /// 连续没有换成的次数（成功即归零；让位不计）。
    pub(crate) consecutive_failures: u32,
    /// 连续失败时，下一次重试前要等的间隔（毫秒）；每失败一次翻倍，有上界。
    pub(crate) retry_after_ms: Option<u64>,
    pub(crate) last_failure: Option<CommitRecord>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Flags {
    /// 该评估而长时间没有评估。
    pub(crate) stalled: bool,
    /// 没有数据可据以选点已持续过久。
    pub(crate) starved: bool,
    pub(crate) exit_outside_subscription: bool,
    /// 实际出口不在订阅内已持续过久。
    pub(crate) exit_outside_overdue: bool,
    pub(crate) all_unverified: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Counters {
    pub(crate) failover_switches: u64,
    pub(crate) select_switches: u64,
    /// 最近一个限频窗口内因「更优」换点的次数与上限。
    pub(crate) better_switches_in_window: usize,
    pub(crate) better_switches_max: usize,
}

/// 周期测速一侧的状况：自动选择没有数据时，原因从这里追。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DataSource {
    /// 周期计划没在执行的原因（调度器的未启用或暂停原因）；在执行时为空。
    pub(crate) blocked_by: Option<&'static str>,
    /// 意图指向的订阅最近一次到期未执行的原因。
    pub(crate) last_skip: Option<&'static str>,
}

/// 选择状态。字段在非自动意图下除意图与实际出口外全部为空或零值，不留陈旧值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Status {
    pub(crate) intent: IntentView,
    pub(crate) exit: ExitView,
    /// `manual` / `settled` / `waitingFirstRound` / `noCandidates` / `noData` / `notEvaluated` /
    /// `needsRestart` / `rateLimited` / `commitFailed`。
    pub(crate) mode: &'static str,
    /// 模式的细分原因。
    pub(crate) reason: Option<&'static str>,
    pub(crate) basis: Option<Basis>,
    /// 本次运行以来的上一次换点。
    pub(crate) last_switch: Option<SwitchRecord>,
    pub(crate) last_evaluation: Option<EvaluationView>,
    pub(crate) challenger: Option<ChallengerView>,
    pub(crate) barred: Vec<BarredView>,
    pub(crate) commit: CommitView,
    /// 没有数据可据以选点已持续的毫秒数；有数据时为空。
    pub(crate) no_data_for_ms: Option<u64>,
    pub(crate) flags: Flags,
    pub(crate) counters: Counters,
    pub(crate) data_source: DataSource,
}

/// 读状态那一刻的对照面。
pub(crate) struct StatusInputs<'a> {
    pub(crate) intent: &'a Intent,
    pub(crate) switches: Switches,
    pub(crate) platform_open: bool,
    pub(crate) core_running: bool,
    /// 核就绪距今多久（毫秒）；不知道时为空。
    pub(crate) core_ready_ago_ms: Option<u64>,
    pub(crate) foreground: bool,
    pub(crate) exit_id: Option<&'a str>,
    /// 实际出口是否属于意图指向的订阅。
    pub(crate) exit_in_subscription: bool,
    pub(crate) basis: Option<Basis>,
    /// 意图指向的订阅的周期（毫秒）。
    pub(crate) period_ms: u64,
    pub(crate) data_source: DataSource,
    /// 单调毫秒。
    pub(crate) now: u64,
}

impl Status {
    /// 除意图与实际出口外全部为空的状态。
    fn blank(
        intent: IntentView,
        exit: ExitView,
        mode: &'static str,
        reason: Option<&'static str>,
    ) -> Self {
        Self {
            intent,
            exit,
            mode,
            reason,
            basis: None,
            last_switch: None,
            last_evaluation: None,
            challenger: None,
            barred: Vec::new(),
            commit: CommitView::default(),
            no_data_for_ms: None,
            flags: Flags::default(),
            counters: Counters::default(),
            data_source: DataSource::default(),
        }
    }
}

/// **选择状态的读时投影**（纯函数）。停滞、长期无数据、出口逾期只依赖槽里的几个单调时刻：评估
/// 任务即使已经不在了，读取方也看得出来。
pub(crate) fn project_status(inputs: &StatusInputs<'_>, slot: &Slot) -> Status {
    let exit = |in_subscription| ExitView {
        server_id: inputs.exit_id.map(str::to_string),
        in_subscription,
    };
    let subscription_id = match inputs.intent {
        Intent::Auto { subscription_id } => subscription_id,
        Intent::Manual => {
            return Status::blank(
                IntentView {
                    mode: "manual",
                    scope: None,
                    subscription_id: None,
                },
                exit(None),
                "manual",
                None,
            );
        }
        Intent::Unrecognized => {
            return Status::blank(
                IntentView {
                    mode: "unrecognized",
                    scope: None,
                    subscription_id: None,
                },
                exit(None),
                "notEvaluated",
                Some(Gate::IntentUnrecognized.as_str()),
            );
        }
    };
    // 读的这一刻就能判的闸门先判：评估任务只在核运行期间存在，核停了之后槽里留着的是旧裁决。
    let live_gate = if !inputs.switches.master {
        Some(Gate::SwitchedOff)
    } else if !inputs.platform_open {
        Some(Gate::PlatformNotOpen)
    } else if !inputs.core_running {
        Some(Gate::CoreNotRunning)
    } else {
        None
    };
    let evaluation = slot
        .evaluation
        .as_ref()
        .filter(|evaluation| &evaluation.subscription == subscription_id);
    let decision = match (live_gate, evaluation) {
        (Some(gate), _) => Some(Decision::NotEvaluated(gate)),
        (None, Some(evaluation)) => Some(evaluation.decision.clone()),
        (None, None) => None,
    };
    let active = live_gate.is_none();
    let commit_failures = slot.memory.commit_failures();
    let (mode, reason) = match &decision {
        // 意图刚写入、还没有轮到一次评估。
        None => ("notEvaluated", Some("notYetEvaluated")),
        Some(Decision::NotEvaluated(Gate::WaitingFirstRound)) => ("waitingFirstRound", None),
        Some(Decision::NotEvaluated(gate)) => ("notEvaluated", Some(gate.as_str())),
        Some(Decision::Hold(Hold::NoCandidates)) => ("noCandidates", None),
        Some(Decision::Hold(Hold::NeedsRestart)) => ("needsRestart", None),
        Some(Decision::Hold(Hold::Pending(Lacking::RateLimit))) => ("rateLimited", None),
        // 没有数据可据以选点：当前出口没有新鲜结果，或周期测速没在供数。
        Some(Decision::Hold(hold @ (Hold::CurrentNoResult(_) | Hold::Starved(_)))) => {
            ("noData", Some(hold.as_str()))
        }
        Some(Decision::Hold(hold @ Hold::CommitFailed(_))) => ("commitFailed", Some(hold.as_str())),
        // 提交连续失败之后在等下一次重试：仍是「提交失败」，不报成已选定。
        Some(Decision::Hold(Hold::Cooldown | Hold::Pending(Lacking::Cooldown)))
            if commit_failures > 0 =>
        {
            ("commitFailed", Some("retryPending"))
        }
        Some(Decision::Hold(hold)) => ("settled", Some(hold.as_str())),
        Some(Decision::Switch { cause, .. }) => ("settled", Some(cause.as_str())),
    };
    let counts = evaluation.map(|evaluation| evaluation.counts);
    let challenger = slot
        .memory
        .streak()
        .filter(|_| active && evaluation.is_some())
        .map(|streak| ChallengerView {
            server_id: streak.leader.clone(),
            wins: streak.wins,
            wins_required: WIN_STREAK,
            lacking: match &decision {
                Some(Decision::Hold(Hold::Pending(lacking))) => Some(lacking.as_str()),
                _ => None,
            },
        });
    // 最近一次「有人看过」距今多久：评估、意图写入、核就绪三者中最近的那个。三者都不知道
    // （进程重启后意图已在盘上，还没评估过）时无从证明评估任务活着，按停滞处理。
    let since = |at: Option<u64>| at.map(|at| inputs.now.saturating_sub(at));
    let quiet_for = [
        since(evaluation.map(|evaluation| evaluation.at_mono)),
        since(slot.intent_set_mono),
        inputs.core_ready_ago_ms,
    ]
    .into_iter()
    .flatten()
    .min();
    let lasting = |at: Option<u64>, periods: u64| {
        since(at).is_some_and(|elapsed| elapsed > inputs.period_ms.saturating_mul(periods))
    };
    Status {
        intent: IntentView {
            mode: "auto",
            scope: Some("subscription"),
            subscription_id: Some(subscription_id.clone()),
        },
        exit: exit(Some(inputs.exit_in_subscription)),
        mode,
        reason,
        basis: inputs.basis.clone(),
        last_switch: slot.last_switch.clone(),
        last_evaluation: evaluation.map(|evaluation| {
            let (outcome, reason) = match &evaluation.decision {
                Decision::NotEvaluated(gate) => ("notEvaluated", gate.as_str()),
                Decision::Hold(hold) => ("held", hold.as_str()),
                Decision::Switch { cause, .. } => ("switch", cause.as_str()),
            };
            EvaluationView {
                at: evaluation.at,
                outcome,
                reason,
                counts: evaluation.counts,
            }
        }),
        challenger,
        barred: slot
            .memory
            .barred(inputs.now)
            .into_iter()
            .map(|(server_id, remaining_ms)| BarredView {
                server_id: server_id.to_string(),
                remaining_ms,
            })
            .collect(),
        commit: CommitView {
            consecutive_failures: commit_failures,
            retry_after_ms: (commit_failures > 0).then(|| retry_after_ms(commit_failures)),
            last_failure: slot.last_commit_failure.clone(),
        },
        no_data_for_ms: since(slot.starved_since).filter(|_| active),
        flags: Flags {
            stalled: active
                && inputs.foreground
                && quiet_for.is_none_or(|elapsed| elapsed > STALL_AFTER_MS),
            starved: active && lasting(slot.starved_since, STARVED_PERIODS),
            exit_outside_subscription: !inputs.exit_in_subscription,
            exit_outside_overdue: active
                && !inputs.exit_in_subscription
                && lasting(slot.outside_since, OUTSIDE_PERIODS),
            all_unverified: active && counts.is_some_and(|counts| counts.all_unverified()),
        },
        counters: Counters {
            failover_switches: slot.failover_switches,
            select_switches: slot.select_switches,
            better_switches_in_window: slot.memory.better_switches_in_window(inputs.now),
            better_switches_max: BETTER_RATE_MAX,
        },
        data_source: inputs.data_source.clone(),
    }
}

#[cfg(test)]
mod tests;
