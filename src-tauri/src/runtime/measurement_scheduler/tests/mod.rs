use super::*;

const WALL: u64 = 1_700_000_000_000;
const MINUTE: u64 = 60_000;

/// 单调时刻 `mono`，墙钟不动（两拍之间没有冻结）。
fn at(mono: u64) -> Now {
    Now { mono, wall: WALL }
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

fn plan(subs: &[(&str, u64, &[&str])]) -> BTreeMap<String, SubPlan> {
    subs.iter()
        .map(|(id, minutes, members)| {
            (
                (*id).to_string(),
                SubPlan {
                    period_ms: minutes * MINUTE,
                    members: ids(members),
                },
            )
        })
        .collect()
}

/// 不关心连续失败数的用例共用的空账本。
static NO_LEDGER: MeasurementLedger = MeasurementLedger::new();

fn input(now: Now, generation: Option<u64>, plan: &BTreeMap<String, SubPlan>) -> TickInput<'_> {
    TickInput {
        now,
        user_enabled: true,
        generation,
        plan,
        network_epoch: None,
        conditions: None,
        policy: MeteredPolicy::Reduced,
        ledger: &NO_LEDGER,
    }
}

/// 一轮以完成收尾的回执。
fn completed(measured: &[(&str, bool)]) -> ScheduledRound {
    ScheduledRound::Ran(RoundReport {
        run: "1".to_string(),
        started_at: WALL,
        ended_at: WALL + 1_000,
        measured: measured
            .iter()
            .map(|(id, ok)| ((*id).to_string(), *ok))
            .collect(),
        width: 16,
        ..RoundReport::default()
    })
}

/// 一轮被打断的回执：`measured` 已入账，`unmeasured` 没有结果。
fn interrupted(reason: InterruptReason, measured: &[&str], unmeasured: &[&str]) -> ScheduledRound {
    ScheduledRound::Ran(RoundReport {
        measured: measured
            .iter()
            .map(|id| ((*id).to_string(), true))
            .collect(),
        unmeasured: ids(unmeasured),
        interrupted: Some(reason),
        ..RoundReport::default()
    })
}

/// 核就绪后把首轮发出去，返回 (planner, 首轮开始的单调时刻)。
fn started(platform: Platform, plan: &BTreeMap<String, SubPlan>) -> (Planner, u64) {
    let mut planner = Planner::new(platform, at(0));
    planner.platform_enabled = true;
    assert_eq!(planner.tick(&input(at(0), Some(1), plan)).start, None);
    let first = planner.tick(&input(at(FIRST_ROUND_DELAY_MS), Some(1), plan));
    assert!(first.start.is_some(), "首轮应在就绪后约定延迟到期");
    (planner, FIRST_ROUND_DELAY_MS)
}

fn no_entries(_: &str) -> Option<EntryView> {
    None
}

// ── 每拍裁决的真值表 ──────────────────────────────────────────────────────────

const READY: TickFacts = TickFacts {
    switched_on: true,
    user_enabled: true,
    platform_enabled: true,
    core_running: true,
    has_plan: true,
    foreground: true,
    round_in_flight: false,
    power_save: false,
    metered: Metered::No,
    policy: MeteredPolicy::Reduced,
    due: true,
    quiet: false,
};

#[test]
fn decide_tick_truth_table() {
    let cases = [
        ("到期且无任何阻碍", READY, Verdict::Start),
        (
            "源码总开关关着",
            TickFacts {
                switched_on: false,
                ..READY
            },
            Verdict::Idle(Idle::SwitchedOff),
        ),
        (
            "用户关了全局总开关",
            TickFacts {
                user_enabled: false,
                ..READY
            },
            Verdict::Idle(Idle::UserDisabled),
        ),
        (
            "本平台未启用",
            TickFacts {
                platform_enabled: false,
                ..READY
            },
            Verdict::Idle(Idle::PlatformNotEnabled),
        ),
        (
            "核未运行",
            TickFacts {
                core_running: false,
                ..READY
            },
            Verdict::Idle(Idle::CoreNotRunning),
        ),
        (
            "启用集合为空",
            TickFacts {
                has_plan: false,
                ..READY
            },
            Verdict::Idle(Idle::NoSubscription),
        ),
        (
            "不在前台",
            TickFacts {
                foreground: false,
                ..READY
            },
            Verdict::Paused(Pause::Background),
        ),
        (
            "不在前台时即使一轮在飞也是暂停",
            TickFacts {
                foreground: false,
                round_in_flight: true,
                ..READY
            },
            Verdict::Paused(Pause::Background),
        ),
        (
            "一轮在飞",
            TickFacts {
                round_in_flight: true,
                ..READY
            },
            Verdict::Running,
        ),
        (
            "省电",
            TickFacts {
                power_save: true,
                ..READY
            },
            Verdict::Paused(Pause::PowerSave),
        ),
        (
            "计费网络，策略为暂停",
            TickFacts {
                metered: Metered::Yes,
                policy: MeteredPolicy::Pause,
                ..READY
            },
            Verdict::Paused(Pause::Metered),
        ),
        (
            "计费网络，策略为降频：照发，周期在收尾时拉长",
            TickFacts {
                metered: Metered::Yes,
                policy: MeteredPolicy::Reduced,
                ..READY
            },
            Verdict::Start,
        ),
        (
            "计费网络，策略为照常",
            TickFacts {
                metered: Metered::Yes,
                policy: MeteredPolicy::Normal,
                ..READY
            },
            Verdict::Start,
        ),
        (
            "计费状态不可得：即使策略为暂停也按照常处理",
            TickFacts {
                metered: Metered::Unavailable,
                policy: MeteredPolicy::Pause,
                ..READY
            },
            Verdict::Start,
        ),
        (
            "未到期",
            TickFacts {
                due: false,
                ..READY
            },
            Verdict::Waiting,
        ),
        (
            "静默期内",
            TickFacts {
                quiet: true,
                ..READY
            },
            Verdict::Waiting,
        ),
    ];
    for (name, facts, expected) in cases {
        assert_eq!(decide_tick(&facts), expected, "{name}");
    }
}

// ── 回退开关与平台开关 ────────────────────────────────────────────────────────

/// 总开关两种取值：关着时永不发起、状态恒为未启用；开着时照常发首轮。
#[test]
fn the_scheduler_switch_gates_every_round() {
    let plan = plan(&[("s1", 30, &["a"])]);
    for (switched_on, expect_start) in [(false, false), (true, true)] {
        let mut planner = Planner::with_switch(switched_on, Platform::Linux, at(0));
        planner.tick(&input(at(0), Some(1), &plan));
        let out = planner.tick(&input(at(FIRST_ROUND_DELAY_MS), Some(1), &plan));
        assert_eq!(
            out.start.is_some(),
            expect_start,
            "switched_on={switched_on}"
        );
        if !switched_on {
            let status = planner.status(at(FIRST_ROUND_DELAY_MS), true, &plan);
            assert_eq!(status["state"], "disabled");
            assert_eq!(status["reason"], "switchedOff");
        }
    }
    const { assert!(SCHEDULER_ENABLED, "出包取值") };
}

/// iOS 的原生侧未就位：不建计划，状态如实报「本平台未启用」。其余平台启用。
#[test]
fn ios_builds_no_schedule_and_says_so() {
    for platform in Platform::ALL {
        assert_eq!(
            platform_enabled(*platform),
            *platform != Platform::Ios,
            "{platform:?}"
        );
    }
    let plan = plan(&[("s1", 30, &["a"])]);
    let mut planner = Planner::new(Platform::Ios, at(0));
    planner.tick(&input(at(0), Some(1), &plan));
    let out = planner.tick(&input(at(10 * MINUTE), Some(1), &plan));
    assert_eq!(out.start, None);
    let status = planner.status(at(10 * MINUTE), true, &plan);
    assert_eq!(status["state"], "disabled");
    assert_eq!(status["reason"], "platformNotEnabled");
    assert_eq!(
        status["subscriptions"]["s1"]["blockedBy"], "platformNotEnabled",
        "订阅里开着却不测，要说得出原因"
    );
}

/// 用户关掉全局总开关：任何订阅都不建计划；开了周期测速的订阅如实标「被全局开关关闭」。
/// 重新打开后按新一轮核就绪处理。
#[test]
fn the_global_switch_blocks_every_subscription_and_says_so() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let mut planner = Planner::new(Platform::Linux, at(0));
    let off = |now| TickInput {
        user_enabled: false,
        ..input(now, Some(1), &plan)
    };
    planner.tick(&off(at(0)));
    assert_eq!(planner.tick(&off(at(10 * MINUTE))).start, None);
    let status = planner.status(at(10 * MINUTE), true, &plan);
    assert_eq!(status["state"], "disabled");
    assert_eq!(status["reason"], "userDisabled");
    assert_eq!(status["subscriptions"]["s1"]["blockedBy"], "userDisabled");

    planner.tick(&input(at(10 * MINUTE), Some(1), &plan));
    let reopened = planner.tick(&input(
        at(10 * MINUTE + FIRST_ROUND_DELAY_MS),
        Some(1),
        &plan,
    ));
    assert!(reopened.start.is_some());
    assert!(planner.status(at(0), true, &plan)["subscriptions"]["s1"]["blockedBy"].is_null());
}

// ── 计划 ──────────────────────────────────────────────────────────────────────

/// 只有开了周期测速、或被自动选择意图指向的订阅进入计划；其余订阅的节点不出现在任何目标里。
#[test]
fn only_enabled_or_auto_selected_subscriptions_enter_the_plan() {
    let config = json!({
        "subscriptions": [
            { "id": "on", "periodicSpeedTest": true, "speedTestIntervalMinutes": 10 },
            { "id": "off", "periodicSpeedTest": false },
            { "id": "unset" },
            { "id": "auto" },
        ],
        "servers": [
            { "id": "a", "subscriptionId": "on" },
            { "id": "b", "subscriptionId": "off" },
            { "id": "c", "subscriptionId": "unset" },
            { "id": "d", "subscriptionId": "auto" },
            { "id": "e", "subscriptionId": "on" },
            { "id": "manual" },
        ],
    });
    let none = plan_subscriptions(&config, &BTreeSet::new());
    assert_eq!(none, plan(&[("on", 10, &["a", "e"])]));
    // 「unset」不进计划依赖每订阅开关缺省为关。
    const { assert!(!polaris_store::PERIODIC_SPEED_TEST_SUBSCRIPTION_DEFAULT) };

    let with_auto = plan_subscriptions(&config, &BTreeSet::from(["auto".to_string()]));
    assert_eq!(
        with_auto,
        plan(&[("auto", 30, &["d"]), ("on", 10, &["a", "e"])])
    );
    let planned: BTreeSet<&String> = with_auto.values().flat_map(|sub| &sub.members).collect();
    for outsider in ["b", "c", "manual"] {
        assert!(!planned.contains(&outsider.to_string()), "{outsider}");
    }
}

/// 周期：5 到 360 分钟的整数；缺省或非法值取 30。新鲜期上限是周期的 2 倍，不低于 10 分钟。
#[test]
fn the_period_falls_back_to_the_default_and_bounds_freshness() {
    for (value, minutes) in [
        (json!(5), 5),
        (json!(360), 360),
        (json!(4), 30),
        (json!(361), 30),
        (json!("15"), 30),
        (json!(12.5), 30),
        (Value::Null, 30),
    ] {
        assert_eq!(
            period_ms_of(&json!({ "speedTestIntervalMinutes": value })),
            minutes * MINUTE,
            "{value}"
        );
    }
    let plan = plan(&[("fast", 5, &["a"]), ("slow", 120, &["b"])]);
    assert_eq!(freshness_cap_ms(&plan, "a"), 10 * MINUTE, "下限 10 分钟");
    assert_eq!(freshness_cap_ms(&plan, "b"), 240 * MINUTE);
    assert_eq!(
        freshness_cap_ms(&plan, "manual-only"),
        60 * MINUTE,
        "不属于任何计划内订阅的节点按缺省周期算"
    );
}

/// 首轮在核就绪后约定延迟到期；一轮完成后下次到期 = 本轮开始时刻加周期。
#[test]
fn the_first_round_waits_then_rounds_repeat_every_period() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let mut planner = Planner::new(Platform::Linux, at(0));
    assert_eq!(planner.tick(&input(at(0), Some(1), &plan)).start, None);
    assert_eq!(
        planner
            .tick(&input(at(FIRST_ROUND_DELAY_MS - 1), Some(1), &plan))
            .start,
        None
    );
    let first = planner.tick(&input(at(FIRST_ROUND_DELAY_MS), Some(1), &plan));
    assert_eq!(
        first.start,
        Some(RoundStart {
            subs: ids(&["s1"]),
            scope: "s1".to_string(),
            only_missing: false,
        })
    );
    assert_eq!(first.sleep_ms, TICK_ACTIVE_MS);
    // 一轮在飞时不再发第二轮。
    assert_eq!(
        planner
            .tick(&input(at(FIRST_ROUND_DELAY_MS + 5_000), Some(1), &plan))
            .start,
        None
    );
    planner.on_round_end(
        at(FIRST_ROUND_DELAY_MS + 9_000),
        &completed(&[("a", true), ("b", false)]),
    );
    let next_due = FIRST_ROUND_DELAY_MS + 30 * MINUTE;
    assert_eq!(
        planner.tick(&input(at(next_due - 1), Some(1), &plan)).start,
        None
    );
    assert!(planner
        .tick(&input(at(next_due), Some(1), &plan))
        .start
        .is_some());

    let status = planner.status(at(next_due), true, &plan);
    assert_eq!(status["state"], "running");
    let sub = &status["subscriptions"]["s1"];
    assert_eq!(sub["lastFullRoundAt"], WALL + 1_000);
    assert_eq!(sub["lastRound"]["ok"], 1);
    assert_eq!(sub["lastRound"]["failed"], 1);
    assert_eq!(status["concurrency"]["width"], 16);
}

/// 同拍到期的两个订阅合并成一轮：目标无重复，`scope` 含两个订阅 id。到期时刻落在合并窗内的
/// 订阅一并并入，窗外的不并。
#[test]
fn subscriptions_due_together_merge_into_one_round() {
    // 两个订阅故意共有节点 b：今天不会出现，去重是为全局自动选择预留的。
    let plan = plan(&[
        ("s1", 30, &["a", "b"]),
        ("s2", 30, &["b", "c"]),
        ("s3", 30, &["d"]),
    ]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    planner.on_round_end(at(t0 + 1_000), &completed(&[]));
    // 错开三个订阅的到期：s1 到期，s2 晚 2 分钟（窗内：周期的十分之一 = 3 分钟），s3 晚 4 分钟。
    let due = t0 + 30 * MINUTE;
    planner.subs.get_mut("s2").unwrap().next_due = due + 2 * MINUTE;
    planner.subs.get_mut("s3").unwrap().next_due = due + 4 * MINUTE;
    let out = planner.tick(&input(at(due), Some(1), &plan));
    let start = out.start.expect("到期应发一轮");
    assert_eq!(start.subs, ids(&["s1", "s2"]));
    assert_eq!(start.scope, "s1,s2");
    let targets = planner.round_targets(&start, None, &no_entries, &BTreeSet::new());
    assert_eq!(
        targets,
        ids(&["a", "b", "c"]),
        "按节点 id 去重，s3 的节点不在内"
    );
}

// ── 与手动测速、故障切换的并存 ────────────────────────────────────────────────

/// 周期一轮被抢占：回到等待并安排补发；补发只测仍缺当前结果的节点。
#[test]
fn a_preempted_round_is_resent_for_missing_targets_only() {
    let plan = plan(&[("s1", 30, &["a", "b", "c"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    let cut = t0 + 2_000;
    planner.on_round_end(
        at(cut),
        &interrupted(InterruptReason::Preempted, &["a"], &["b", "c"]),
    );
    let status = planner.status(at(cut), true, &plan);
    assert_eq!(status["state"], "running", "读状态不推进状态机");
    assert_eq!(
        status["subscriptions"]["s1"]["lastSkip"]["reason"],
        "preempted"
    );
    assert!(status["subscriptions"]["s1"]["lastFullRoundAt"].is_null());

    assert_eq!(
        planner
            .tick(&input(at(cut + PREEMPT_RESEND_MS - 1), Some(1), &plan))
            .start,
        None
    );
    let resend = planner
        .tick(&input(at(cut + PREEMPT_RESEND_MS), Some(1), &plan))
        .start
        .expect("约定间隔后补发");
    assert!(resend.only_missing);
    // 手动一轮已经测出了 a 与 b 的当前结果：补发的目标只剩 c。
    let current: BTreeSet<String> = ids(&["a", "b"]).into_iter().collect();
    assert_eq!(
        planner.round_targets(&resend, None, &no_entries, &current),
        ids(&["c"])
    );
    // 补发完成后回到常规节拍，下一轮不再是「只补缺的」。
    planner.on_round_end(
        at(cut + PREEMPT_RESEND_MS + 1_000),
        &completed(&[("c", true)]),
    );
    let regular = planner
        .tick(&input(
            at(cut + PREEMPT_RESEND_MS + 30 * MINUTE),
            Some(1),
            &plan,
        ))
        .start
        .unwrap();
    assert!(!regular.only_missing);
}

/// 闸被占用：记「跳过：闸被占用」及占用方，按约定间隔重试；同一次到期至多重试 3 次，
/// 之后并入下一个到期。
#[test]
fn a_busy_gate_is_retried_a_bounded_number_of_times() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let (mut planner, mut now) = started(Platform::Linux, &plan);
    let busy = ScheduledRound::Busy(Some(SpeedTestOrigin::Manual));
    for retry in 1..=BUSY_RETRY_MAX {
        planner.on_round_end(at(now), &busy);
        let skip = &planner.status(at(now), true, &plan)["subscriptions"]["s1"]["lastSkip"];
        assert_eq!(skip["reason"], "gateBusy");
        assert_eq!(skip["holder"], "manual");
        assert_eq!(
            planner
                .tick(&input(at(now + BUSY_RETRY_MS - 1), Some(1), &plan))
                .start,
            None,
            "第 {retry} 次重试之前"
        );
        now += BUSY_RETRY_MS;
        assert!(
            planner
                .tick(&input(at(now), Some(1), &plan))
                .start
                .is_some(),
            "第 {retry} 次重试"
        );
    }
    // 第 3 次重试仍然忙：不再按重试间隔发，等一个周期。
    planner.on_round_end(at(now), &busy);
    assert_eq!(
        planner
            .tick(&input(at(now + BUSY_RETRY_MS), Some(1), &plan))
            .start,
        None
    );
    assert_eq!(
        planner
            .tick(&input(at(now + 30 * MINUTE - 1), Some(1), &plan))
            .start,
        None
    );
    assert!(planner
        .tick(&input(at(now + 30 * MINUTE), Some(1), &plan))
        .start
        .is_some());
}

// ── 核世代 ────────────────────────────────────────────────────────────────────

/// 核世代变化：在飞的一轮被取消，到期时刻、轮转与退避清空；新世代首轮在约定延迟后到期。
/// 停核即回到未启用。
#[test]
fn a_new_core_generation_resets_the_schedule() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    planner.carry_over = ids(&["b"]);
    planner.backoff_skips.insert("a".to_string(), 2);

    let restarted = planner.tick(&input(at(t0 + 5_000), Some(2), &plan));
    assert!(restarted.abort, "旧世代在飞的一轮被取消");
    assert_eq!(restarted.start, None);
    assert!(planner.carry_over.is_empty() && planner.backoff_skips.is_empty());
    // 旧一轮的收尾迟到：不得把它记到新世代头上。
    planner.on_round_end(
        at(t0 + 5_500),
        &interrupted(InterruptReason::Superseded, &["a"], &["b"]),
    );
    assert!(planner.subs["s1"].last_round.is_none());
    assert_eq!(
        planner
            .tick(&input(
                at(t0 + 5_000 + FIRST_ROUND_DELAY_MS - 1),
                Some(2),
                &plan
            ))
            .start,
        None
    );
    assert!(planner
        .tick(&input(
            at(t0 + 5_000 + FIRST_ROUND_DELAY_MS),
            Some(2),
            &plan
        ))
        .start
        .is_some());

    let stopped = planner.tick(&input(at(t0 + 60_000), None, &plan));
    assert!(stopped.abort);
    let status = planner.status(at(t0 + 60_000), false, &plan);
    assert_eq!(status["state"], "disabled");
    assert_eq!(status["reason"], "coreNotRunning");
}

// ── 前后台 ────────────────────────────────────────────────────────────────────

/// Android：离开前台即取消在飞的一轮并进入暂停，一直保持到回前台信号到达。
/// 一轮已被标成超预算而尚未收尾时核换代（或计划被清空）：重置盖过旧成因，收尾不给新世代记账，
/// 新世代的首轮照就绪后的约定延迟发出。
#[test]
fn a_reset_overrides_an_earlier_abort_cause() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let cut = || interrupted(InterruptReason::Cancelled, &["a"], &["b"]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    let over = t0 + ROUND_BUDGET_MAX_MS + 1;
    assert!(planner.tick(&input(at(over), Some(1), &plan)).abort);
    planner.tick(&input(at(over + 1_000), Some(2), &plan));
    planner.on_round_end(at(over + 2_000), &cut());
    assert!(planner.carry_over.is_empty());
    assert_eq!(
        planner.status(at(over + 2_000), true, &plan)["subscriptions"]["s1"]["lastSkip"],
        Value::Null
    );
    let ready = over + 1_000;
    assert_eq!(
        planner
            .tick(&input(at(ready + FIRST_ROUND_DELAY_MS - 1), Some(2), &plan))
            .start,
        None
    );
    let first = planner
        .tick(&input(at(ready + FIRST_ROUND_DELAY_MS), Some(2), &plan))
        .start
        .expect("新世代的首轮不被旧世代那一轮的成因推迟");
    assert!(!first.only_missing);

    // 计划被清空同理。
    let (mut planner, t0) = started(Platform::Linux, &plan);
    let over = t0 + ROUND_BUDGET_MAX_MS + 1;
    planner.tick(&input(at(over), Some(1), &plan));
    planner.tick(&input(at(over + 1_000), Some(1), &BTreeMap::new()));
    planner.on_round_end(at(over + 2_000), &cut());
    assert!(planner.carry_over.is_empty());
}

async fn panicking_round() -> ScheduledRound {
    panic!("测量层 panic")
}

/// 一轮的任务 panic：收尾照常发生，在飞标记清掉并记下原因，下一个周期照发。
#[tokio::test]
async fn a_crashed_round_task_is_settled_and_the_schedule_continues() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    assert_eq!(settle(panicking_round()).await, None);
    planner.on_round_crashed(at(t0 + 1_000));
    assert_eq!(
        planner.status(at(t0 + 1_000), true, &plan)["subscriptions"]["s1"]["lastSkip"]["reason"],
        "crashed"
    );
    assert_eq!(
        planner
            .tick(&input(at(t0 + 30 * MINUTE - 1), Some(1), &plan))
            .start,
        None,
        "不立即重试"
    );
    assert!(planner
        .tick(&input(at(t0 + 30 * MINUTE), Some(1), &plan))
        .start
        .is_some());

    // 正常跑完的回执原样带出。
    assert_eq!(
        settle(async { ScheduledRound::PoolUnavailable }).await,
        Some(ScheduledRound::PoolUnavailable)
    );
}

#[test]
fn android_stays_paused_until_resumed() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let (mut planner, t0) = started(Platform::Android, &plan);
    assert!(planner.on_suspended(at(t0 + 1_000)), "在飞的一轮要被取消");
    planner.on_round_end(
        at(t0 + 1_500),
        &interrupted(InterruptReason::Cancelled, &["a"], &["b"]),
    );
    for later in [2_000, IOS_QUIET_MS + 5_000, 20 * MINUTE] {
        let out = planner.tick(&input(at(t0 + later), Some(1), &plan));
        assert_eq!(out.start, None, "回前台之前不发起（+{later}ms）");
        assert_eq!(out.sleep_ms, TICK_IDLE_MS);
    }
    let status = planner.status(at(t0 + 20 * MINUTE), true, &plan);
    assert_eq!(status["state"], "paused");
    assert_eq!(status["reason"], "background");
    assert_eq!(
        status["subscriptions"]["s1"]["lastSkip"]["reason"],
        "background"
    );
    assert_eq!(status["stalled"], false, "不在前台时不判停滞");

    planner.on_resumed(at(t0 + 20 * MINUTE));
    let back = planner
        .tick(&input(at(t0 + 20 * MINUTE), Some(1), &plan))
        .start
        .expect("回到前台且已到期：先补一轮");
    assert!(back.only_missing, "被打断的一轮只补仍缺的");
}

/// iOS：离开前台只作打断，取消在飞的一轮并静默一段时间；静默期过后不需要回前台信号即回到等待
/// 并照常发起。（iOS 的计划本轮整体关闭，这里绕开平台开关单测这条分支。）
#[test]
fn ios_resumes_after_the_quiet_period_without_a_resume_signal() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let (mut planner, t0) = started(Platform::Ios, &plan);
    let left = t0 + 1_000;
    assert!(planner.on_suspended(at(left)));
    planner.on_round_end(
        at(left + 500),
        &interrupted(InterruptReason::Cancelled, &["a"], &["b"]),
    );
    let quiet = planner.tick(&input(at(left + IOS_QUIET_MS - 1), Some(1), &plan));
    assert_eq!(quiet.start, None);
    assert_eq!(
        planner.status(at(left), true, &plan)["state"],
        "waiting",
        "不进入需要回前台信号才能退出的暂停"
    );
    assert!(planner
        .tick(&input(at(left + IOS_QUIET_MS), Some(1), &plan))
        .start
        .is_some());
}

/// 桌面不按前后台判定：离开前台的信号被忽略。
#[test]
fn desktop_ignores_foreground_signals() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    assert!(!planner.on_suspended(at(t0 + 1_000)));
    assert_eq!(
        planner.status(at(t0 + 1_000), true, &plan)["state"],
        "running"
    );
}

/// 回到前台：离开 4 分钟前台代次不变、沿用原到期时刻；离开 6 分钟加一并立即发起一轮。
#[test]
fn returning_after_a_long_absence_bumps_the_foreground_epoch_and_retests() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let (mut planner, t0) = started(Platform::Android, &plan);
    planner.on_round_end(at(t0 + 1_000), &completed(&[("a", true)]));

    let leave = Now {
        mono: t0 + 2_000,
        wall: WALL,
    };
    planner.on_suspended(leave);
    let short = Now {
        mono: leave.mono + 4 * MINUTE,
        wall: WALL + 4 * MINUTE,
    };
    planner.on_resumed(short);
    assert_eq!(planner.foreground_epoch(), 0);
    planner.last_tick.0 = short.wall;
    assert_eq!(planner.tick(&input(short, Some(1), &plan)).start, None);

    planner.on_suspended(short);
    let long = Now {
        mono: short.mono + 6 * MINUTE,
        wall: short.wall + 6 * MINUTE,
    };
    planner.on_resumed(long);
    assert_eq!(planner.foreground_epoch(), 1);
    planner.last_tick.0 = long.wall;
    assert!(
        planner.tick(&input(long, Some(1), &plan)).start.is_some(),
        "旧结果已不可用于选点：立即补测"
    );
}

/// 两拍之间墙钟跳了很久（休眠 / 被挂起）：在飞的一轮按「设备冻结」取消，未出值的节点不记失败，
/// 解冻后只补仍缺的。
#[test]
fn a_tick_gap_is_treated_as_a_freeze() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    let thawed = Now {
        mono: t0 + 5_000,
        wall: WALL + TICK_ACTIVE_MS * TICK_GAP_FACTOR + 1,
    };
    let out = planner.tick(&input(thawed, Some(1), &plan));
    assert!(out.abort);
    planner.on_round_end(
        thawed,
        &interrupted(InterruptReason::Cancelled, &["a"], &["b"]),
    );
    assert_eq!(
        planner.status(thawed, true, &plan)["subscriptions"]["s1"]["lastSkip"]["reason"],
        "frozen"
    );
    let again = Now {
        mono: thawed.mono + 5_000,
        wall: thawed.wall + 5_000,
    };
    let resend = planner
        .tick(&input(again, Some(1), &plan))
        .start
        .expect("解冻后补测");
    assert!(resend.only_missing);

    // 间隔恰在阈值上：不判冻结。
    let (mut steady, t0) = started(Platform::Linux, &plan);
    let on_time = Now {
        mono: t0 + 5_000,
        wall: WALL + TICK_ACTIVE_MS * TICK_GAP_FACTOR,
    };
    assert!(!steady.tick(&input(on_time, Some(1), &plan)).abort);
}

/// 桌面合盖休眠再唤醒：离开够久即立即补一轮，不等原到期时刻；这一次计入切网补测的最小间隔。
#[test]
fn a_desktop_wake_from_a_long_sleep_retests_at_once() {
    let plan = plan(&[("s1", 60, &["a"])]);
    let on = |now, epoch| TickInput {
        network_epoch: Some(epoch),
        ..input(now, Some(1), &plan)
    };
    let slept = |away| {
        let mut planner = Planner::new(Platform::Linux, at(0));
        planner.tick(&on(at(0), 5));
        assert!(planner
            .tick(&on(at(FIRST_ROUND_DELAY_MS), 5))
            .start
            .is_some());
        planner.on_round_end(at(20_000), &completed(&[("a", true)]));
        assert_eq!(planner.tick(&on(at(25_000), 5)).start, None);
        let woke = Now {
            mono: 30_000,
            wall: WALL + away,
        };
        let start = planner.tick(&on(woke, 5)).start;
        (planner, woke, start)
    };

    let (_, _, start) = slept(AWAY_EPOCH_MS - 1);
    assert_eq!(start, None, "离开不久：沿用原到期时刻");

    let (mut planner, woke, start) = slept(AWAY_EPOCH_MS);
    assert!(!start.expect("唤醒后立即补测").only_missing);
    let later = |ms| Now {
        mono: woke.mono + ms,
        wall: woke.wall,
    };
    planner.on_round_end(later(1_000), &completed(&[("a", true)]));

    // 唤醒后网络代次跟着变：稳定了也要等到距这次补测满下限。
    assert_eq!(planner.tick(&on(later(2_000), 6)).start, None);
    assert_eq!(
        planner.tick(&on(later(NET_RETEST_MIN_GAP_MS - 1), 6)).start,
        None
    );
    assert!(planner
        .tick(&on(later(NET_RETEST_MIN_GAP_MS), 6))
        .start
        .is_some());
}

/// 测量层自己在结果间隔上检出冻结（回执带 `frozen`）时同样记「设备冻结」并只补仍缺的。
#[test]
fn a_frozen_report_is_recorded_as_a_freeze() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    planner.on_round_end(
        at(t0 + 20_000),
        &ScheduledRound::Ran(RoundReport {
            unmeasured: ids(&["a", "b"]),
            interrupted: Some(InterruptReason::Cancelled),
            frozen: true,
            ..RoundReport::default()
        }),
    );
    let sub = &planner.status(at(t0 + 20_000), true, &plan)["subscriptions"]["s1"];
    assert_eq!(sub["lastSkip"]["reason"], "frozen");
    assert_eq!(sub["lastRound"]["failed"], 0, "没有任何节点被记成失败");
    assert_eq!(sub["lastRound"]["unmeasured"], 2);
}

// ── 切网、计费、省电 ──────────────────────────────────────────────────────────

/// 桌面网络代次变化：等它稳定后补测一轮；两次由切网触发的补测之间不小于约定下限。
#[test]
fn a_desktop_network_change_triggers_a_settled_rate_limited_retest() {
    let plan = plan(&[("s1", 60, &["a"])]);
    let on = |mono, epoch| TickInput {
        network_epoch: Some(epoch),
        ..input(at(mono), Some(1), &plan)
    };
    let mut planner = Planner::new(Platform::Linux, at(0));
    planner.tick(&on(0, 5));
    assert!(planner.tick(&on(FIRST_ROUND_DELAY_MS, 5)).start.is_some());
    planner.on_round_end(at(20_000), &completed(&[("a", true)]));

    // 代次连变两次：以最后一次为准重新等稳定。
    let changed = 100_000;
    assert_eq!(planner.tick(&on(changed, 6)).start, None);
    assert_eq!(planner.tick(&on(changed + 4_000, 7)).start, None);
    assert_eq!(
        planner
            .tick(&on(changed + 4_000 + NET_SETTLE_MS - 1, 7))
            .start,
        None
    );
    let first_retest = changed + 4_000 + NET_SETTLE_MS;
    assert!(planner.tick(&on(first_retest, 7)).start.is_some());
    planner.on_round_end(at(first_retest + 1_000), &completed(&[("a", true)]));

    // 紧接着又变：稳定了也要等到距上次补测满下限。
    assert_eq!(planner.tick(&on(first_retest + 30_000, 8)).start, None);
    assert_eq!(
        planner
            .tick(&on(first_retest + NET_RETEST_MIN_GAP_MS - 1, 8))
            .start,
        None
    );
    assert!(planner
        .tick(&on(first_retest + NET_RETEST_MIN_GAP_MS, 8))
        .start
        .is_some());
}

/// 计费网络三种策略：暂停不发并记原因；降频照发、下次到期按 4 倍周期；照常不变。
/// 「不可得」按照常处理，状态里如实标出。
#[test]
fn the_metered_policy_pauses_reduces_or_ignores() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let run = |metered, policy| {
        let mut planner = Planner::new(Platform::Android, at(0));
        let tick = |planner: &mut Planner, mono| {
            planner.tick(&TickInput {
                conditions: Some(DeviceConditions {
                    metered,
                    power_save: false,
                }),
                policy,
                ..input(at(mono), Some(1), &plan)
            })
        };
        tick(&mut planner, 0);
        let first = tick(&mut planner, FIRST_ROUND_DELAY_MS).start.is_some();
        if first {
            planner.on_round_end(at(FIRST_ROUND_DELAY_MS + 1_000), &completed(&[("a", true)]));
        }
        let at_one_period = tick(&mut planner, FIRST_ROUND_DELAY_MS + 30 * MINUTE)
            .start
            .is_some();
        let status = planner.status(at(FIRST_ROUND_DELAY_MS + 30 * MINUTE), true, &plan);
        (first, at_one_period, status)
    };

    let (first, _, status) = run(Metered::Yes, MeteredPolicy::Pause);
    assert!(!first);
    assert_eq!(status["state"], "paused");
    assert_eq!(status["reason"], "metered");
    assert_eq!(
        status["subscriptions"]["s1"]["lastSkip"]["reason"],
        "metered"
    );
    assert_eq!(status["metered"], "metered");

    let (first, at_one_period, status) = run(Metered::Yes, MeteredPolicy::Reduced);
    assert!(first, "降频：首轮照发");
    assert!(!at_one_period, "降频：一个周期后还不到期");
    assert_eq!(
        (
            &status["state"],
            &status["metered"],
            &status["meteredPolicy"]
        ),
        (&json!("waiting"), &json!("metered"), &json!("reduced")),
        "降频不是暂停：状态里靠计费状态加策略说明"
    );
    assert_eq!(
        status["subscriptions"]["s1"]["nextDueAt"],
        WALL + METERED_REDUCED_FACTOR * 30 * MINUTE - 30 * MINUTE,
        "下次到期 = 本轮开始加 4 倍周期"
    );

    for (metered, policy, label) in [
        (Metered::Yes, MeteredPolicy::Normal, "metered"),
        (Metered::No, MeteredPolicy::Pause, "unmetered"),
        (Metered::Unavailable, MeteredPolicy::Pause, "unavailable"),
    ] {
        let (first, at_one_period, status) = run(metered, policy);
        assert!(first && at_one_period, "{label}");
        assert_eq!(status["metered"], label);
    }
    assert_eq!(
        MeteredPolicy::parse(None),
        MeteredPolicy::Reduced,
        "缺省降频"
    );
    assert_eq!(MeteredPolicy::parse(Some("pause")), MeteredPolicy::Pause);
    assert_eq!(MeteredPolicy::parse(Some("normal")), MeteredPolicy::Normal);
    assert_eq!(MeteredPolicy::parse(Some("bogus")), MeteredPolicy::Reduced);
}

/// 省电：下一轮起不再发起；解除后继续。计费状态翻转视同切网：立即补测，手机上前台代次加一。
#[test]
fn power_save_pauses_and_a_metered_flip_retests() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let with = |mono, metered, power_save| TickInput {
        conditions: Some(DeviceConditions {
            metered,
            power_save,
        }),
        ..input(at(mono), Some(1), &plan)
    };
    let mut planner = Planner::new(Platform::Android, at(0));
    planner.tick(&with(0, Metered::No, true));
    assert_eq!(
        planner
            .tick(&with(FIRST_ROUND_DELAY_MS, Metered::No, true))
            .start,
        None
    );
    assert_eq!(
        planner.status(at(FIRST_ROUND_DELAY_MS), true, &plan)["reason"],
        "powerSave"
    );
    let resumed = FIRST_ROUND_DELAY_MS + 5_000;
    assert!(planner
        .tick(&with(resumed, Metered::No, false))
        .start
        .is_some());
    planner.on_round_end(at(resumed + 1_000), &completed(&[("a", true)]));

    // Wi-Fi → 蜂窝：远未到期也立即补测。
    assert_eq!(planner.foreground_epoch(), 0);
    assert!(planner
        .tick(&with(resumed + 60_000, Metered::Yes, false))
        .start
        .is_some());
    assert_eq!(planner.foreground_epoch(), 1);
}

/// 计费状态来回翻转（弱 Wi-Fi 与蜂窝互换）：补测与切网共用最小间隔，间隔内的翻转并成满间隔后的
/// 一轮。前台代次照旧每次翻转加一。
#[test]
fn metered_flips_share_the_network_retest_gap() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let with = |mono, metered| TickInput {
        conditions: Some(DeviceConditions {
            metered,
            power_save: false,
        }),
        ..input(at(mono), Some(1), &plan)
    };
    let mut planner = Planner::new(Platform::Android, at(0));
    planner.tick(&with(0, Metered::No));
    assert!(planner
        .tick(&with(FIRST_ROUND_DELAY_MS, Metered::No))
        .start
        .is_some());
    planner.on_round_end(at(FIRST_ROUND_DELAY_MS + 1_000), &completed(&[("a", true)]));

    let first = FIRST_ROUND_DELAY_MS + 60_000;
    assert!(planner.tick(&with(first, Metered::Yes)).start.is_some());
    planner.on_round_end(at(first + 1_000), &completed(&[("a", true)]));

    assert_eq!(planner.tick(&with(first + 30_000, Metered::No)).start, None);
    assert_eq!(
        planner.tick(&with(first + 60_000, Metered::Yes)).start,
        None
    );
    assert_eq!(planner.foreground_epoch(), 3);
    assert_eq!(
        planner
            .tick(&with(first + NET_RETEST_MIN_GAP_MS - 1, Metered::Yes))
            .start,
        None
    );
    assert!(planner
        .tick(&with(first + NET_RETEST_MIN_GAP_MS, Metered::Yes))
        .start
        .is_some());
}

/// 拉设备状况的时机：有订阅到期时（准入前必查）；否则隔一段查一次；一轮在飞或没有计划时不查。
#[test]
fn device_conditions_are_polled_before_admission_and_periodically() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let mut planner = Planner::new(Platform::Android, at(0));
    assert!(!planner.wants_conditions(at(0)), "没有计划");
    planner.tick(&TickInput {
        conditions: Some(DeviceConditions::UNAVAILABLE),
        ..input(at(0), Some(1), &plan)
    });
    assert!(!planner.wants_conditions(at(1_000)), "刚查过且未到期");
    assert!(planner.wants_conditions(at(FIRST_ROUND_DELAY_MS)), "到期");
    assert!(planner
        .tick(&input(at(FIRST_ROUND_DELAY_MS), Some(1), &plan))
        .start
        .is_some());
    assert!(
        !planner.wants_conditions(at(FIRST_ROUND_DELAY_MS)),
        "一轮在飞"
    );
    planner.on_round_end(at(FIRST_ROUND_DELAY_MS + 1_000), &completed(&[("a", true)]));
    assert!(!planner.wants_conditions(at(CONDITIONS_POLL_MS - 1)));
    assert!(
        planner.wants_conditions(at(CONDITIONS_POLL_MS)),
        "距上次查询已久"
    );
}

#[test]
fn network_manager_metered_property_maps_to_three_states() {
    for (output, expected) in [
        ("u 1\n", Metered::Yes),
        ("u 3\n", Metered::Yes),
        ("u 2\n", Metered::No),
        ("u 4\n", Metered::No),
        ("u 0\n", Metered::Unavailable),
        ("", Metered::Unavailable),
        ("Failed to get property", Metered::Unavailable),
    ] {
        assert_eq!(
            parse_network_manager_metered(output),
            expected,
            "{output:?}"
        );
    }
}

#[test]
fn a_native_answer_maps_to_three_states() {
    assert_eq!(metered_of(Some(true)), Metered::Yes);
    assert_eq!(metered_of(Some(false)), Metered::No);
    assert_eq!(metered_of(None), Metered::Unavailable);
}

// ── 排序、退避、时间预算 ──────────────────────────────────────────────────────

fn view_of<'a>(
    entries: &'a [(&'static str, EntryView)],
) -> impl Fn(&str) -> Option<EntryView> + 'a {
    move |id| {
        entries
            .iter()
            .find(|(node, _)| *node == id)
            .map(|(_, entry)| *entry)
    }
}

const fn entry(ok: bool, measured_at: u64, consecutive_failures: u32) -> EntryView {
    EntryView {
        this_generation: true,
        ok,
        measured_at,
        consecutive_failures,
    }
}

/// 目标顺序：选中的节点；上一轮没测到的；本核世代还没有结果的；上次成功的（从旧到新）；上次失败的。
#[test]
fn targets_are_ordered_so_failures_share_the_last_waves() {
    let entries = [
        ("failed", entry(false, 900, 1)),
        ("ok-new", entry(true, 800, 0)),
        ("ok-old", entry(true, 100, 0)),
        ("selected", entry(true, 500, 0)),
        (
            "old-generation",
            EntryView {
                this_generation: false,
                ..entry(true, 50, 0)
            },
        ),
        ("carried", entry(true, 700, 0)),
    ];
    let members = ids(&[
        "failed",
        "ok-new",
        "never",
        "ok-old",
        "selected",
        "old-generation",
        "carried",
    ]);
    let ordered = order_targets(
        &members,
        Some("selected"),
        &ids(&["carried"]),
        &view_of(&entries),
        None,
        &mut BTreeMap::new(),
    );
    assert_eq!(
        ordered,
        ids(&[
            "selected",
            "carried",
            "never",
            "old-generation",
            "ok-old",
            "ok-new",
            "failed",
        ])
    );
}

/// 退避：连续 3 轮失败的节点在随后 3 轮不出现，第 4 轮出现；一次成功后恢复每轮出现。
/// 补发（只补缺的）不推进退避计数。
#[test]
fn a_node_failing_three_rounds_is_tested_every_fourth_round() {
    let members = ids(&["good", "bad"]);
    let mut skips = BTreeMap::new();
    let rounds = |entries: &[(&'static str, EntryView)], skips: &mut BTreeMap<String, u32>| {
        order_targets(&members, None, &[], &view_of(entries), None, skips)
            .contains(&"bad".to_string())
    };
    let failing = [("good", entry(true, 1, 0)), ("bad", entry(false, 1, 3))];
    let below = [("good", entry(true, 1, 0)), ("bad", entry(false, 1, 2))];
    assert!(rounds(&below, &mut skips), "连续 2 轮失败：照测");
    let appeared: Vec<bool> = (0..8).map(|_| rounds(&failing, &mut skips)).collect();
    assert_eq!(
        appeared,
        [false, false, false, true, false, false, false, true]
    );

    // 补发不算一轮：计数不动，退避中的节点也不进补发目标。
    let before = skips.clone();
    let resend = order_targets(
        &members,
        None,
        &[],
        &view_of(&failing),
        Some(&BTreeSet::new()),
        &mut skips,
    );
    assert_eq!(resend, ids(&["good"]));
    assert_eq!(skips, before);

    let recovered = [("good", entry(true, 1, 0)), ("bad", entry(true, 2, 0))];
    assert!(
        (0..3).all(|_| rounds(&recovered, &mut skips)),
        "成功后每轮都测"
    );
    assert!(skips.is_empty());
}

/// 换核世代、切网、计费翻转、久离返回之后，补测轮要测到此前在退避里的节点：这些事件把账本的
/// 连续失败数清零。没有这些事件时，下一个周期轮照旧跳过退避中的节点。
#[test]
fn a_new_generation_or_network_forgives_failures_so_the_retest_covers_backed_off_nodes() {
    use crate::commands::speedtest::{
        CoreInstance, FailKind, FailPhase, MeasureFailure, MeasurePath, ResultIdentity,
    };

    let plan = plan(&[("s1", 60, &["good", "bad"])]);
    let first_round = FIRST_ROUND_DELAY_MS;
    let idle = first_round + 100_000;
    // 一拍：(时刻, 核世代, 网络代次, 计费状态)。返回这一拍若发了一轮，它的目标。
    let tick = |planner: &mut Planner,
                ledger: &MeasurementLedger,
                now: Now,
                generation: u64,
                epoch: u64,
                metered: Metered| {
        let start = planner
            .tick(&TickInput {
                network_epoch: Some(epoch),
                conditions: Some(DeviceConditions {
                    metered,
                    power_save: false,
                }),
                ledger,
                ..input(now, Some(generation), &plan)
            })
            .start?;
        let view = LedgerView::new(&Value::Null, Some(generation), Some(epoch), Platform::Linux);
        Some(planner.round_targets(&start, None, &|id| view.entry(ledger, id), &BTreeSet::new()))
    };
    // 首轮跑完后，`bad` 已连续失败 3 轮（进了退避），`good` 是通的。
    let backed_off = || {
        let ledger = MeasurementLedger::new();
        let mut planner = Planner::new(Platform::Linux, at(0));
        assert_eq!(tick(&mut planner, &ledger, at(0), 1, 5, Metered::No), None);
        assert_eq!(
            tick(&mut planner, &ledger, at(first_round), 1, 5, Metered::No),
            Some(ids(&["good", "bad"]))
        );
        let url_digest =
            polaris_updater::sha256_hex(resolve_speed_test_url(&Value::Null).as_bytes());
        let identity = |run| ResultIdentity {
            run,
            seq: 1,
            origin: SpeedTestOrigin::Schedule,
            scope: None,
            path: MeasurePath::Candidate,
            url_digest: url_digest.clone(),
            instance: CoreInstance::Main {
                generation: 1,
                start_time: None,
            },
            config_digest: None,
            node_fingerprint: None,
            network_epoch: Some(5),
            measured_at: WALL,
        };
        ledger.record("good", Ok(50), identity(1));
        for run in 1..=u64::from(BACKOFF_AFTER_FAILURES) {
            ledger.record(
                "bad",
                Err(MeasureFailure::new(FailPhase::Measure, FailKind::Timeout)),
                identity(run),
            );
        }
        planner.on_round_end(
            at(first_round + 5_000),
            &completed(&[("good", true), ("bad", false)]),
        );
        assert_eq!(
            tick(&mut planner, &ledger, at(idle), 1, 5, Metered::No),
            None
        );
        (planner, ledger)
    };
    let all = Some(ids(&["good", "bad"]));

    // 对照：没有事件，下一个周期轮不测 `bad`。
    let (mut planner, ledger) = backed_off();
    let next_period = at(first_round + 60 * MINUTE);
    assert_eq!(
        tick(&mut planner, &ledger, next_period, 1, 5, Metered::No),
        Some(ids(&["good"]))
    );

    // 网络代次变化：稳定后的补测轮。
    let (mut planner, ledger) = backed_off();
    assert_eq!(
        tick(&mut planner, &ledger, at(idle + 5_000), 1, 6, Metered::No),
        None
    );
    let settled = at(idle + 5_000 + NET_SETTLE_MS);
    assert_eq!(
        tick(&mut planner, &ledger, settled, 1, 6, Metered::No),
        all,
        "切网"
    );

    // 换核世代：新世代的首轮。
    let (mut planner, ledger) = backed_off();
    assert_eq!(
        tick(&mut planner, &ledger, at(idle + 5_000), 2, 5, Metered::No),
        None
    );
    let first_of_new = at(idle + 5_000 + FIRST_ROUND_DELAY_MS);
    assert_eq!(
        tick(&mut planner, &ledger, first_of_new, 2, 5, Metered::No),
        all,
        "换核世代"
    );

    // 计费状态翻转：当拍补测。
    let (mut planner, ledger) = backed_off();
    assert_eq!(
        tick(&mut planner, &ledger, at(idle + 5_000), 1, 5, Metered::Yes),
        all,
        "计费翻转"
    );

    // 久离返回（桌面休眠醒来）：当拍补测。
    let (mut planner, ledger) = backed_off();
    let woke = Now {
        mono: idle + 5_000,
        wall: WALL + AWAY_EPOCH_MS,
    };
    assert_eq!(
        tick(&mut planner, &ledger, woke, 1, 5, Metered::No),
        all,
        "久离返回"
    );
}

/// 时间预算：取周期的一半与 10 分钟中较小者。超出即取消本轮，没测到的节点下一轮排在最前；
/// 连续 3 次后置「周期过短」，完整跑完一轮后清除。
#[test]
fn a_round_over_budget_is_cut_and_rotated() {
    let plan = plan(&[("s1", 6, &["a", "b", "c", "d"])]);
    let (mut planner, mut start) = started(Platform::Linux, &plan);
    let budget = 3 * MINUTE;
    for streak in 1..=SHORT_PERIOD_STREAK {
        assert!(
            !planner
                .tick(&input(at(start + budget), Some(1), &plan))
                .abort,
            "预算之内不取消"
        );
        assert!(
            planner
                .tick(&input(at(start + budget + 1), Some(1), &plan))
                .abort
        );
        planner.on_round_end(
            at(start + budget + 100),
            &interrupted(InterruptReason::Cancelled, &["a", "b"], &["c", "d"]),
        );
        let sub = &planner.status(at(start), true, &plan)["subscriptions"]["s1"];
        assert_eq!(sub["lastSkip"]["reason"], "budget");
        assert_eq!(
            sub["periodTooShort"],
            streak >= SHORT_PERIOD_STREAK,
            "第 {streak} 次截断"
        );
        // 下次到期按本轮开始加周期算。
        start += 6 * MINUTE;
        let next = planner
            .tick(&input(at(start), Some(1), &plan))
            .start
            .expect("下一轮");
        assert!(!next.only_missing);
        assert_eq!(
            planner.round_targets(&next, None, &no_entries, &BTreeSet::new()),
            ids(&["c", "d", "a", "b"]),
            "没测到的排在最前"
        );
    }
    planner.on_round_end(at(start + 1_000), &completed(&[("a", true)]));
    assert_eq!(
        planner.status(at(start), true, &plan)["subscriptions"]["s1"]["periodTooShort"],
        false
    );

    let long = self::plan(&[("s1", 360, &["a"])]);
    let (mut planner, start) = started(Platform::Linux, &long);
    assert!(
        !planner
            .tick(&input(at(start + ROUND_BUDGET_MAX_MS), Some(1), &long))
            .abort
    );
    assert!(
        planner
            .tick(&input(at(start + ROUND_BUDGET_MAX_MS + 1), Some(1), &long))
            .abort,
        "周期再长，单轮也不超过 10 分钟"
    );
}

/// 零可测与探针池不可用：各记原因，按常规周期再试，没有任何节点被记成失败。
#[test]
fn rounds_that_measured_nothing_are_recorded_with_a_reason() {
    let plan = plan(&[("s1", 30, &["a", "b"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    planner.on_round_end(
        at(t0 + 100),
        &ScheduledRound::Ran(RoundReport {
            skipped: vec![
                ("a".to_string(), "notInPool"),
                ("b".to_string(), "notInPool"),
            ],
            ..RoundReport::default()
        }),
    );
    let skip = &planner.status(at(t0), true, &plan)["subscriptions"]["s1"]["lastSkip"];
    assert_eq!(skip["reason"], "nothingTestable");
    assert_eq!(skip["skipped"]["notInPool"], 2);

    assert!(planner
        .tick(&input(at(t0 + 30 * MINUTE), Some(1), &plan))
        .start
        .is_some());
    planner.on_round_end(at(t0 + 30 * MINUTE), &ScheduledRound::PoolUnavailable);
    assert_eq!(
        planner.status(at(t0), true, &plan)["subscriptions"]["s1"]["lastSkip"]["reason"],
        "poolUnavailable"
    );
}

/// 完整轮次要有成员真被测到：成员全被预筛跳过的订阅、以及没有目标的空轮，都不刷新完整轮次时间，
/// 逾期照样亮得起来。
#[test]
fn only_a_round_that_measured_members_counts_as_a_full_round() {
    let plan = plan(&[("s1", 30, &["a"]), ("s2", 30, &["x"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    let not_in_pool = |id: &str| (id.to_string(), "notInPool");
    let subs =
        |planner: &Planner, mono| planner.status(at(mono), true, &plan)["subscriptions"].clone();

    // 合并的一轮：s1 测到了，s2 的成员全被预筛跳过。
    planner.on_round_end(
        at(t0 + 1_000),
        &ScheduledRound::Ran(RoundReport {
            ended_at: WALL + 1_000,
            measured: BTreeMap::from([("a".to_string(), true)]),
            skipped: vec![not_in_pool("x")],
            ..RoundReport::default()
        }),
    );
    let after = subs(&planner, t0 + 1_000);
    assert_eq!(after["s1"]["lastFullRoundAt"], WALL + 1_000);
    assert_eq!(after["s2"]["lastFullRoundAt"], Value::Null);

    // 空轮（没有目标）：单独记原因。
    let second = t0 + 30 * MINUTE;
    assert!(planner
        .tick(&input(at(second), Some(1), &plan))
        .start
        .is_some());
    planner.on_round_end(
        at(second),
        &ScheduledRound::Ran(RoundReport {
            started_at: WALL + 30 * MINUTE,
            ended_at: WALL + 30 * MINUTE,
            ..RoundReport::default()
        }),
    );
    let after = subs(&planner, second);
    assert_eq!(after["s1"]["lastFullRoundAt"], WALL + 1_000);
    assert_eq!(after["s1"]["lastSkip"]["reason"], "noTargets");
    assert_eq!(after["s2"]["lastFullRoundAt"], Value::Null);

    // 全部被预筛跳过的一轮。
    let third = t0 + 60 * MINUTE;
    assert!(planner
        .tick(&input(at(third), Some(1), &plan))
        .start
        .is_some());
    planner.on_round_end(
        at(third),
        &ScheduledRound::Ran(RoundReport {
            ended_at: WALL + 60 * MINUTE,
            skipped: vec![not_in_pool("a"), not_in_pool("x")],
            ..RoundReport::default()
        }),
    );
    let after = subs(&planner, third);
    assert_eq!(after["s1"]["lastFullRoundAt"], WALL + 1_000);
    assert_eq!(after["s2"]["lastFullRoundAt"], Value::Null);
    assert_eq!(after["s2"]["lastSkip"]["reason"], "nothingTestable");

    // s2 从没有过完整轮次：距就绪超过 3 个周期即逾期。
    let late = planner.status(
        Now {
            mono: third,
            wall: WALL + OVERDUE_FACTOR * 30 * MINUTE + 1,
        },
        true,
        &plan,
    );
    assert_eq!(late["subscriptions"]["s2"]["overdue"], true);
}

// ── 观测：读时投影 ────────────────────────────────────────────────────────────

/// 逾期与停滞是读时投影：不驱动调度器，只拨时钟。
#[test]
fn overdue_and_stalled_are_projected_at_read_time() {
    let plan = plan(&[("s1", 30, &["a"])]);
    let (mut planner, t0) = started(Platform::Linux, &plan);
    planner.on_round_end(at(t0 + 1_000), &completed(&[("a", true)]));
    planner.tick(&input(at(t0 + 2_000), Some(1), &plan));
    let last_full = WALL + 1_000;

    let read = |wall| {
        planner.status(
            Now {
                mono: t0 + 2_000,
                wall,
            },
            true,
            &plan,
        )
    };
    let on_time = read(last_full + OVERDUE_FACTOR * 30 * MINUTE);
    assert_eq!(on_time["subscriptions"]["s1"]["overdue"], false);
    let late = read(last_full + OVERDUE_FACTOR * 30 * MINUTE + 1);
    assert_eq!(late["subscriptions"]["s1"]["overdue"], true);

    // 最近一拍的时刻变旧：停滞。核不在运行时不判。
    assert_eq!(
        read(WALL + TICK_ACTIVE_MS * TICK_GAP_FACTOR)["stalled"],
        false
    );
    let stale = WALL + TICK_ACTIVE_MS * TICK_GAP_FACTOR + 1;
    assert_eq!(read(stale)["stalled"], true);
    assert_eq!(read(stale)["lastTickAt"], WALL);
    assert_eq!(
        planner.status(
            Now {
                mono: t0,
                wall: stale
            },
            false,
            &plan
        )["stalled"],
        false
    );
}

/// 状态里带本平台的并发上限与每订阅开关的缺省值：界面从这里取，不自己写死。
#[test]
fn status_carries_the_platform_cap_and_the_subscription_default() {
    let plan = BTreeMap::new();
    for (platform, cap) in [
        (Platform::Linux, 64),
        (Platform::Android, 32),
        (Platform::Ios, 16),
    ] {
        let status = Planner::new(platform, at(0)).status(at(0), false, &plan);
        let limits = &status["limits"];
        assert_eq!(limits["concurrencyMax"], cap, "{platform:?}");
        assert_eq!(limits["concurrencyMin"], 4);
        assert_eq!(
            (
                &limits["intervalMinutesMin"],
                &limits["intervalMinutesMax"],
                &limits["intervalMinutesDefault"]
            ),
            (&json!(5), &json!(360), &json!(30))
        );
        assert_eq!(limits["enabledDefault"], true);
        assert_eq!(limits["subscriptionDefault"], false);
        assert_eq!(limits["meteredPolicyDefault"], "reduced");
        assert_eq!(status["metered"], "unavailable");
    }
}
