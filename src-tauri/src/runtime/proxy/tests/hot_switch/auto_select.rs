//! 自动选点的 I/O 编排：择优腿取读数 → 裁决 → 经故障切换那一个提交事务换点，两条显式命令的
//! 落盘，心跳循环的唤醒，以及它们与手动选择、订阅范围、平台闸之间的边界。
//!
//! 账本、调度器读数、单飞闸与两个时钟一律注入：不碰进程级的那一份，也不读真实时间。

use std::ops::ControlFlow;
use std::time::Duration;

use super::*;
use crate::commands::server::{
    auto_select_enable_core, auto_select_switch_now_core, finish_placement_core,
    server_switch_core, AutoSelectError, Placement,
};
use crate::commands::speedtest::{
    resolve_speed_test_url, BindingVerdict, CoreInstance, FailKind, FailPhase, MeasureFailure,
    MeasurePath, ResultIdentity, SpeedTestOrigin,
};
use crate::runtime::auto_select::{
    retry_after_ms, Cause, CommitFailure, Decision, Gate, Hold, Lacking, Leg, SwitchRecord,
    Switches,
};
use crate::runtime::auto_switch::{AutoSwitchMachine, HEARTBEAT_INTERVAL_MS, SWITCH_COOLDOWN_MS};
use crate::runtime::measurement_ledger::MeasurementLedger;
use crate::runtime::measurement_scheduler::{Idle, Pause, Signals, Verdict};
use crate::runtime::proxy::auto_switch::{
    run_heartbeat, HeartbeatLegs, SelectCache, SelectReadings,
};

const MINUTE: u64 = 60_000;
/// 注入的两个时钟的起点。两者毫无关系：用错时钟的比较会立刻露出来。
const MONO: u64 = 7 * 60 * MINUTE;
const WALL: u64 = 1_700_000_000_000;

/// 订阅 `sub` 含 `node-b`、`node-c`；`node-a` 是自建节点。
fn auto_config(selected: &str, intent: Option<&str>) -> Value {
    let mut cfg = two_node_config(7890, selected);
    cfg["servers"]
        .as_array_mut()
        .unwrap()
        .push(ss_node("node-c", "Node C", 18003));
    cfg["servers"][1]["subscriptionId"] = serde_json::json!("sub");
    cfg["servers"][2]["subscriptionId"] = serde_json::json!("sub");
    cfg["subscriptions"] = serde_json::json!([
        { "id": "sub", "name": "S", "url": "https://a.example/x" },
        { "id": "other", "name": "O", "url": "https://b.example/x" },
    ]);
    if let Some(subscription) = intent {
        cfg["selectionIntent"] = polaris_store::selection_intent_auto(subscription);
    }
    cfg
}

/// 落盘并把运行时标成「这份配置正在运行」。返回落盘后的规范形。
fn running(rt: &ProxyRuntime, cfg: &Value) -> Value {
    rt.config.save_full(cfg).expect("seed D");
    let saved = rt.config.current().unwrap();
    mark_running_with_snapshot(rt, &saved);
    rt.switch_snapshot
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .id_to_tag
        .insert("node-c".to_string(), "Node C".to_string());
    saved
}

/// 一条当前核世代、探针槽路径测得的结果的身份块；测量的墙钟时刻就是注入的「现在」。
fn identity(rt: &ProxyRuntime, cfg: &Value, run: u64) -> ResultIdentity {
    ResultIdentity {
        run,
        seq: 1,
        origin: SpeedTestOrigin::Schedule,
        scope: None,
        path: MeasurePath::Candidate,
        url_digest: polaris_updater::sha256_hex(resolve_speed_test_url(cfg).as_bytes()),
        instance: CoreInstance::Main {
            generation: rt.core_generation(),
            start_time: None,
        },
        config_digest: None,
        node_fingerprint: None,
        network_epoch: None,
        measured_at: WALL,
        binding: Some(BindingVerdict::Confirmed),
    }
}

/// 一轮测速的结果入账：`(节点, 延迟)`，同一个运行号、同一个入账时刻。
fn round(
    ledger: &MeasurementLedger,
    rt: &ProxyRuntime,
    cfg: &Value,
    run: u64,
    at: u64,
    results: &[(&str, u32)],
) {
    for (node, latency_ms) in results {
        ledger.record_at(node, Ok(*latency_ms), identity(rt, cfg, run), at);
    }
}

fn readings_at(ledger: &MeasurementLedger, now: u64) -> SelectReadings<'_> {
    SelectReadings {
        ledger,
        signals: Signals::idle(),
        measuring: false,
        now,
        now_wall: WALL,
    }
}

fn readings(ledger: &MeasurementLedger) -> SelectReadings<'_> {
    readings_at(ledger, MONO)
}

async fn assess(
    rt: &Arc<ProxyRuntime>,
    readings: &SelectReadings<'_>,
    last_attempt_at: Option<u64>,
) -> Option<SelectPlan> {
    rt.auto_select_assess(
        rt.core_generation(),
        last_attempt_at,
        &mut SelectCache::default(),
        readings,
    )
    .await
}

async fn commit(
    rt: &Arc<ProxyRuntime>,
    machine: &mut AutoSwitchMachine,
    plan: &SelectPlan,
    api: &AttestingManagementApi,
    readings: &SelectReadings<'_>,
) -> bool {
    rt.auto_select_commit(
        rt.core_generation(),
        machine,
        plan,
        api,
        &mut SelectCache::default(),
        readings,
    )
    .await
}

fn last_decision(rt: &ProxyRuntime) -> Decision {
    rt.auto_select
        .lock()
        .unwrap()
        .evaluation
        .as_ref()
        .expect("应已评估过")
        .decision
        .clone()
}

fn intent_on_disk(rt: &ProxyRuntime) -> Option<String> {
    polaris_store::selection_intent_subscription(&rt.config.current().unwrap()).map(str::to_string)
}

fn exit_on_disk(rt: &ProxyRuntime) -> String {
    rt.config.current().unwrap()["selectedServerId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ── 择优腿：评估与提交 ────────────────────────────────────────────────────────

/// 首次落点：出口不在订阅内，账本里订阅成员都有当前结果 → 落到延迟最小者。提交走后台事务：
/// 磁盘侧只改实际出口，意图不变，内核不重启，只换 `proxy-selector`。评估时槽里记的是
/// 「正在提交」，提交成功才落成换点。
#[tokio::test]
async fn the_select_leg_lands_on_the_fastest_member_and_keeps_the_intent() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    // 别的订阅、自建节点再快也不是候选。
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80), ("node-a", 5)],
    );
    let reads = readings(&ledger);

    let plan = assess(&rt, &reads, None).await.expect("应换点");
    assert_eq!(plan.candidate.id, "node-c");
    assert_eq!((plan.cause, plan.latency_ms), (Cause::FirstPlacement, 80));
    assert_eq!(plan.subscription, "sub");
    assert_eq!(last_decision(&rt), Decision::Hold(Hold::Committing));

    let api = AttestingManagementApi::new("Node A");
    let mut machine = AutoSwitchMachine::new();
    assert!(commit(&rt, &mut machine, &plan, &api, &reads).await);
    assert_eq!(exit_on_disk(&rt), "node-c");
    assert_eq!(
        intent_on_disk(&rt).as_deref(),
        Some("sub"),
        "系统代选不清意图"
    );
    assert_eq!(rt.status().pid, 424242, "后台事务不重启内核");
    assert_eq!(
        *api.puts.lock().unwrap(),
        vec![("proxy-selector".to_string(), "Node C".to_string())]
    );
    assert!(!machine.is_switching(), "在飞标志已释放");
    assert_eq!(
        machine.last_switch_time(),
        Some(MONO),
        "两条腿共用的冷却已起"
    );
    assert_eq!(
        last_decision(&rt),
        Decision::Switch {
            target: "node-c".to_string(),
            latency_ms: 80,
            cause: Cause::FirstPlacement
        }
    );
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!((slot.select_switches, slot.failover_switches), (1, 0));
    let record = slot.last_switch.as_ref().unwrap();
    assert_eq!(
        (
            record.leg,
            record.cause,
            record.to_id.as_str(),
            record.unverified,
            record.at
        ),
        (
            Leg::Select,
            Cause::FirstPlacement,
            "node-c",
            Some(false),
            WALL
        )
    );
    assert_eq!(record.from_id.as_deref(), Some("node-a"));
}

#[tokio::test]
async fn production_tick_passes_the_shared_cooldown_to_assessment() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    let mut machine = AutoSwitchMachine::new();
    machine.begin_switch(MONO);
    machine.end_switch();
    let mut cache = SelectCache::default();
    rt.auto_select_tick_from(
        rt.core_generation(),
        &mut machine,
        &mut cache,
        &readings_at(&ledger, MONO + SWITCH_COOLDOWN_MS - 1),
        async { AttestingManagementApi::new("Node A") },
    )
    .await;
    assert_eq!(last_decision(&rt), Decision::Hold(Hold::Cooldown));
    assert_eq!(exit_on_disk(&rt), "node-a");
    let boundary = readings_at(&ledger, MONO + SWITCH_COOLDOWN_MS);
    assert!(
        rt.auto_select_assess(
            rt.core_generation(),
            machine.last_switch_time(),
            &mut cache,
            &boundary
        )
        .await
        .is_some(),
        "the same candidate becomes eligible at the exact cooldown boundary"
    );
}

#[tokio::test]
async fn production_restart_evidence_survives_background_ticks_and_allows_explicit_placement() {
    for mixed in [false, true] {
        let (rt, _dir) = test_runtime();
        let mut initial = auto_config("node-b", Some("sub"));
        initial["servers"][2] = valid_force_route_wg_node("node-c", "Node C");
        initial["servers"][2]["subscriptionId"] = serde_json::json!("sub");
        if mixed {
            initial["servers"][0]["subscriptionId"] = serde_json::json!("sub");
        }
        let cfg = running(&rt, &initial);
        let ledger = MeasurementLedger::new();
        let mut cache = SelectCache::default();
        for (run, at) in [(1, MONO), (2, MONO + 5 * MINUTE)] {
            let mut results = vec![("node-b", 400), ("node-c", 10)];
            if mixed {
                results.push(("node-a", 390));
            }
            round(&ledger, &rt, &cfg, run, at, &results);
            for beat in [at, at + HEARTBEAT_INTERVAL_MS] {
                assert!(rt
                    .auto_select_assess(
                        rt.core_generation(),
                        None,
                        &mut cache,
                        &readings_at(&ledger, beat)
                    )
                    .await
                    .is_none());
                assert_eq!(
                    last_decision(&rt),
                    Decision::Hold(if run == 1 {
                        Hold::Pending(Lacking::Streak)
                    } else {
                        Hold::NeedsRestart
                    }),
                    "mixed={mixed}"
                );
            }
        }
        assert_eq!(rt.auto_select.lock().unwrap().select_switches, 0);
        assert_eq!(exit_on_disk(&rt), "node-b");
        let (saved, placement) = switch_now(
            &rt,
            &readings_at(&ledger, MONO + 5 * MINUTE + HEARTBEAT_INTERVAL_MS),
        )
        .expect("explicit command inherits the two comparable wins");
        assert_eq!(placement.to, "node-c");
        assert_eq!(saved["selectedServerId"], "node-c");
    }
}

#[tokio::test]
async fn refresh_records_only_a_accepted_runtime_exit_change() {
    use crate::commands::subscription::record_auto_exit_after_removal;
    for outcome in [
        None,
        Some(SwitchOutcome::NotRunning),
        Some(SwitchOutcome::Pending),
        Some(SwitchOutcome::Deferred),
        Some(SwitchOutcome::NoOp),
        Some(SwitchOutcome::Unchanged),
        Some(SwitchOutcome::HotSwitched),
        Some(SwitchOutcome::Restarting),
    ] {
        let (rt, _dir) = test_runtime();
        let cfg = auto_config("node-c", Some("sub"));
        rt.auto_select
            .lock()
            .unwrap()
            .memory
            .record_commit_failure(CommitFailure::Failed);
        record_auto_exit_after_removal(&rt, &cfg, Some("removed-node"), outcome);
        let slot = rt.auto_select.lock().unwrap();
        let accepted = matches!(
            outcome,
            Some(SwitchOutcome::HotSwitched | SwitchOutcome::Restarting)
        );
        assert_eq!(slot.select_switches, u64::from(accepted), "{outcome:?}");
        assert_eq!(slot.last_switch.is_some(), accepted);
        assert_eq!(slot.memory.commit_failures(), u32::from(!accepted));
    }
}

#[tokio::test]
async fn production_refresh_broadcast_settles_rejected_and_accepted_apply() {
    use crate::commands::config::apply_config_broadcast_core;
    use crate::commands::subscription::record_auto_exit_after_removal;
    for case in ["stopped", "pending", "superseded", "stale", "restart"] {
        let (rt, _dir) = test_runtime();
        let mut cfg = if case == "stopped" {
            auto_config("node-b", Some("sub"))
        } else {
            running(&rt, &auto_config("node-b", Some("sub")))
        };
        // Every branch really replaces removed B with C; a rejected Apply must reach
        // the receipt guard instead of returning early because the exit did not change.
        cfg["servers"]
            .as_array_mut()
            .unwrap()
            .retain(|server| server["id"] != "node-b");
        cfg["selectedServerId"] = serde_json::json!("node-c");
        rt.config.save_full(&cfg).unwrap();
        rt.auto_select
            .lock()
            .unwrap()
            .memory
            .record_commit_failure(CommitFailure::Failed);
        let generation = rt.register_selector_intent();
        if case == "pending" {
            rt.gate.begin();
        }
        if case == "superseded" {
            rt.register_selector_intent();
        }
        let mut candidate = cfg.clone();
        if case == "stale" {
            candidate["selectedServerId"] = serde_json::json!("node-a");
        }
        apply_config_broadcast_core(
            &rt,
            candidate,
            false,
            generation,
            |_| {},
            |proxy, outcome| {
                if case == "restart" {
                    assert!(matches!(outcome, Some(SwitchOutcome::Restarting)));
                } else {
                    assert!(!matches!(
                        outcome,
                        Some(SwitchOutcome::HotSwitched | SwitchOutcome::Restarting)
                    ));
                }
                record_auto_exit_after_removal(proxy, &cfg, Some("node-b"), outcome);
            },
        )
        .await;
        let slot = rt.auto_select.lock().unwrap();
        assert_eq!(slot.select_switches, u64::from(case == "restart"), "{case}");
        assert_eq!(slot.last_switch.is_some(), case == "restart", "{case}");
        assert_eq!(
            slot.memory.commit_failures(),
            u32::from(case != "restart"),
            "{case}"
        );
    }
}

/// 账本里不能进候选的结果一条都换不动出口：停止态临时核测得的（未连接测量）、读回判定量到的
/// 不是它的、超过新鲜期上限的、别的核世代测的。订阅里没有候选时出口原样保留：不转直连，不借
/// 别的订阅里更快的节点。
#[tokio::test]
async fn results_that_may_not_be_selected_never_move_the_exit() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let good = |run| identity(&rt, &cfg, run);
    let cases: [(&str, ResultIdentity); 4] = [
        (
            "未连接测量",
            ResultIdentity {
                instance: CoreInstance::Temp,
                ..good(1)
            },
        ),
        (
            "读回不符",
            ResultIdentity {
                binding: Some(BindingVerdict::Mismatch),
                ..good(1)
            },
        ),
        (
            "超过新鲜期上限",
            ResultIdentity {
                measured_at: WALL - 24 * 60 * MINUTE,
                ..good(1)
            },
        ),
        (
            "别的核世代",
            ResultIdentity {
                instance: CoreInstance::Main {
                    generation: rt.core_generation() + 1,
                    start_time: None,
                },
                ..good(1)
            },
        ),
    ];
    for (label, bad) in cases {
        let ledger = MeasurementLedger::new();
        ledger.record_at("node-b", Ok(10), bad, MONO);
        // `node-c` 真测了没通。
        ledger.record_at(
            "node-c",
            Err(MeasureFailure::new(FailPhase::Measure, FailKind::Timeout)),
            good(1),
            MONO,
        );
        ledger.record_at("node-a", Ok(5), good(1), MONO);
        let mut reads = readings(&ledger);
        // 读回不符的结果算「有当前结果」，其余三种不算：用调度器的一轮收尾来过首轮闸门。
        reads
            .signals
            .subscriptions
            .insert("sub".to_string(), (Some(1), None));
        assert_eq!(assess(&rt, &reads, None).await, None, "{label}");
        assert_eq!(
            last_decision(&rt),
            Decision::Hold(Hold::NoCandidates),
            "{label}"
        );
        assert_eq!(exit_on_disk(&rt), "node-a", "{label}：无候选时出口不动");
        // 正向对照：同一个节点换成一条合格的结果，立刻成为落点。
        ledger.record_at("node-b", Ok(10), good(2), MONO);
        let plan = assess(&rt, &reads, None).await.expect(label);
        assert_eq!(plan.candidate.id, "node-b", "{label}");
    }
}

/// 首轮未完成不落点：只测了一部分成员、调度器也没有报告一轮收尾时，出口不动；不取成员表第一个，
/// 不取半份数据里的最优。调度器报告该订阅有一轮收尾后才评估。周期测速的全局总开关关着时，
/// 原因如实报成「没有周期数据」。
#[tokio::test]
async fn nothing_is_placed_before_the_first_round_completes() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(&ledger, &rt, &cfg, 1, MONO, &[("node-b", 120)]);

    let mut reads = readings(&ledger);
    assert_eq!(assess(&rt, &reads, None).await, None);
    assert_eq!(
        last_decision(&rt),
        Decision::NotEvaluated(Gate::WaitingFirstRound)
    );

    reads.signals.verdict = Verdict::Idle(Idle::UserDisabled);
    assert_eq!(assess(&rt, &reads, None).await, None);
    assert_eq!(
        last_decision(&rt),
        Decision::NotEvaluated(Gate::PeriodicDisabled),
        "总开关关闭时不择优，原因可读"
    );
    assert_eq!(exit_on_disk(&rt), "node-a");

    // 调度器的一轮收尾（序号大于进入本范围时的 0）。
    reads.signals.verdict = Verdict::Waiting;
    reads.signals.round_serial = 1;
    reads
        .signals
        .subscriptions
        .insert("sub".to_string(), (Some(1), None));
    let plan = assess(&rt, &reads, None).await.expect("首轮收尾后落点");
    assert_eq!(plan.candidate.id, "node-b");

    // 有测速在飞：不拿中途的数据选点。
    reads.measuring = true;
    assert_eq!(assess(&rt, &reads, None).await, None);
    assert_eq!(last_decision(&rt), Decision::NotEvaluated(Gate::Measuring));
}

/// 首轮完成之后数据源停掉：当前出口的结果过期，而周期测速被总开关关掉、因计费暂停或因省电暂停。
/// 状态是「没有数据」，原因是被什么挡住；出口不动。读回长期不符同理，原因是读回不符。
#[tokio::test]
async fn a_stopped_data_source_after_the_first_round_is_reported_not_settled() {
    for (verdict, reason) in [
        (Verdict::Idle(Idle::UserDisabled), "periodicDisabled"),
        (Verdict::Paused(Pause::Metered), "meteredPaused"),
        (Verdict::Paused(Pause::PowerSave), "powerSave"),
        (Verdict::Waiting, "currentExpired"),
    ] {
        let (rt, _dir) = test_runtime();
        let cfg = running(&rt, &auto_config("node-b", Some("sub")));
        let ledger = MeasurementLedger::new();
        round(
            &ledger,
            &rt,
            &cfg,
            1,
            MONO,
            &[("node-b", 100), ("node-c", 90)],
        );
        // 首轮完成，出口已选定。
        assert_eq!(assess(&rt, &readings(&ledger), None).await, None);
        assert_eq!(last_decision(&rt), Decision::Hold(Hold::Settled));
        // 一天之后：结果早已过期，周期测速没在跑。
        let mut later = readings_at(&ledger, MONO + 24 * 60 * MINUTE);
        later.now_wall = WALL + 24 * 60 * MINUTE;
        later.signals.verdict = verdict;
        assert_eq!(assess(&rt, &later, None).await, None);
        let status = rt.auto_select_status_from(&later);
        assert_eq!(
            (status.mode, status.reason),
            ("noData", Some(reason)),
            "{verdict:?}"
        );
        assert_eq!(status.no_data_for_ms, Some(0));
        assert_eq!(exit_on_disk(&rt), "node-b");
        // 持续三个周期以上：长期无数据置位。
        let mut much_later = readings_at(&ledger, later.now + 91 * MINUTE);
        much_later.now_wall = later.now_wall + 91 * MINUTE;
        much_later.signals.verdict = verdict;
        assess(&rt, &much_later, None).await;
        let status = rt.auto_select_status_from(&much_later);
        assert!(status.flags.starved, "{verdict:?}");
        assert_eq!(status.no_data_for_ms, Some(91 * MINUTE));
    }

    // 当前出口读回长期不符：每一轮都有它的「结果」，但没有一条可用于选点。
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-b", Some("sub")));
    let ledger = MeasurementLedger::new();
    for (run, at) in [(1, MONO), (2, MONO + 30 * MINUTE)] {
        ledger.record_at(
            "node-b",
            Ok(40),
            ResultIdentity {
                binding: Some(BindingVerdict::Mismatch),
                ..identity(&rt, &cfg, run)
            },
            at,
        );
        round(&ledger, &rt, &cfg, run, at, &[("node-c", 10)]);
        assert_eq!(assess(&rt, &readings_at(&ledger, at), None).await, None);
    }
    let status = rt.auto_select_status_from(&readings_at(&ledger, MONO + 30 * MINUTE));
    assert_eq!(
        (status.mode, status.reason),
        ("noData", Some("currentMismatch"))
    );
    assert_eq!(status.no_data_for_ms, Some(30 * MINUTE));
    assert_eq!(exit_on_disk(&rt), "node-b");
}

/// 换点没换成（管理面拒绝目标，事务已恢复原出口）：不记换点，实际出口与意图不变，内核不重启；
/// 结果回写状态 —— 模式是「提交失败」，带原因与连续失败次数。重试间隔随连续失败翻倍：失败一次
/// 后 60 秒时还不重试，120 秒时才重试；换成之后清零。
#[tokio::test]
async fn a_failed_commit_is_recorded_and_retried_with_a_growing_interval() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    let reads = readings(&ledger);
    let plan = assess(&rt, &reads, None).await.unwrap();

    let rejecting = AttestingManagementApi::rejecting("Node A", "Node C");
    let mut machine = AutoSwitchMachine::new();
    assert!(!commit(&rt, &mut machine, &plan, &rejecting, &reads).await);
    assert_eq!(exit_on_disk(&rt), "node-a");
    assert_eq!(intent_on_disk(&rt).as_deref(), Some("sub"));
    assert_eq!(rt.status().pid, 424242);
    assert!(!machine.is_switching(), "失败路径同样释放在飞标志");
    assert_eq!(machine.last_switch_time(), Some(MONO), "失败也进冷却");
    assert_eq!(
        last_decision(&rt),
        Decision::Hold(Hold::CommitFailed(CommitFailure::Failed)),
        "槽里不留一个实际没换的换点"
    );
    {
        let slot = rt.auto_select.lock().unwrap();
        assert_eq!(slot.select_switches, 0, "回滚的换点不算换点");
        assert_eq!(slot.last_switch, None);
        assert_eq!(slot.memory.commit_failures(), 1);
    }
    let status = rt.auto_select_status_from(&reads);
    assert_eq!(
        (status.mode, status.reason),
        ("commitFailed", Some("commitFailed"))
    );
    assert_eq!(status.commit.consecutive_failures, 1);
    assert_eq!(status.commit.retry_after_ms, Some(2 * SWITCH_COOLDOWN_MS));
    assert_eq!(
        status
            .commit
            .last_failure
            .as_ref()
            .map(|f| f.to_id.as_str()),
        Some("node-c")
    );

    // 60 秒后：平时的冷却已过，但失败过一次，间隔翻倍，还不重试；状态仍是提交失败。
    let attempted = machine.last_switch_time();
    let at_60 = readings_at(&ledger, MONO + SWITCH_COOLDOWN_MS);
    assert_eq!(assess(&rt, &at_60, attempted).await, None);
    assert_eq!(last_decision(&rt), Decision::Hold(Hold::Cooldown));
    let status = rt.auto_select_status_from(&at_60);
    assert_eq!(
        (status.mode, status.reason),
        ("commitFailed", Some("retryPending"))
    );
    // 120 秒后重试；再失败，间隔到 240 秒。
    let at_120 = readings_at(&ledger, MONO + retry_after_ms(1));
    let plan = assess(&rt, &at_120, attempted).await.expect("到点重试");
    assert!(!commit(&rt, &mut machine, &plan, &rejecting, &at_120).await);
    assert_eq!(rt.auto_select.lock().unwrap().memory.commit_failures(), 2);
    let at_239 = readings_at(&ledger, at_120.now + retry_after_ms(2) - 1);
    assert_eq!(assess(&rt, &at_239, machine.last_switch_time()).await, None);
    // 管理面恢复：到点重试并换成，失败计数清零，状态回到已选定。
    let at_240 = readings_at(&ledger, at_120.now + retry_after_ms(2));
    let plan = assess(&rt, &at_240, machine.last_switch_time())
        .await
        .unwrap();
    let api = AttestingManagementApi::new("Node A");
    assert!(commit(&rt, &mut machine, &plan, &api, &at_240).await);
    let status = rt.auto_select_status_from(&at_240);
    assert_eq!(
        (status.mode, status.reason),
        ("settled", Some("firstPlacement"))
    );
    assert_eq!(status.commit.consecutive_failures, 0);
    assert_eq!(status.commit.last_failure, None);
    assert_eq!(exit_on_disk(&rt), "node-c");
}

/// 起点是阻断哨兵：切到任何节点都要整核重启（阻断由规则级 reject 表达）。后台不重启：候选全被
/// 剔除，状态为「需重启才能切换」，没有任何 selector 调用。
#[tokio::test]
async fn a_start_that_needs_a_restart_is_reported_and_never_restarted_in_the_background() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config(BLOCK_SERVER_ID, Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    assert_eq!(assess(&rt, &readings(&ledger), None).await, None);
    assert_eq!(last_decision(&rt), Decision::Hold(Hold::NeedsRestart));
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!(slot.evaluation.as_ref().unwrap().counts.needs_restart, 2);
    assert_eq!(rt.status().pid, 424242);
    assert_eq!(exit_on_disk(&rt), BLOCK_SERVER_ID);
}

#[tokio::test]
async fn production_assessment_invalidates_cached_eligibility_on_exit_or_generation_change() {
    for change_generation in [false, true] {
        let (rt, _dir) = test_runtime();
        let mut initial = auto_config(
            if change_generation {
                "node-a"
            } else {
                BLOCK_SERVER_ID
            },
            Some("sub"),
        );
        // A new kernel may change a candidate's routing requirements while keeping
        // the same selected exit. The generation is then the sole cache-key change.
        if change_generation {
            initial["servers"][2] = valid_force_route_wg_node("node-c", "Node C");
            initial["servers"][2]["subscriptionId"] = serde_json::json!("sub");
        }
        initial["servers"][1]["subscriptionId"] = serde_json::json!("other");
        let cfg = running(&rt, &initial);
        let ledger = MeasurementLedger::new();
        round(&ledger, &rt, &cfg, 1, MONO, &[("node-c", 80)]);
        let mut cache = SelectCache::default();
        assert!(rt
            .auto_select_assess(rt.core_generation(), None, &mut cache, &readings(&ledger))
            .await
            .is_none());
        assert_eq!(last_decision(&rt), Decision::Hold(Hold::NeedsRestart));
        // Same key keeps the actual cached verdicts, rather than a fresh cache per call.
        assert!(rt
            .auto_select_assess(rt.core_generation(), None, &mut cache, &readings(&ledger))
            .await
            .is_none());
        if change_generation {
            rt.gate.bump_generation();
        }
        let mut next_config = auto_config(
            if change_generation {
                "node-a"
            } else {
                DIRECT_SERVER_ID
            },
            Some("sub"),
        );
        next_config["servers"][1]["subscriptionId"] = serde_json::json!("other");
        let live = running(&rt, &next_config);
        round(&ledger, &rt, &live, 2, MONO + 1, &[("node-c", 80)]);
        let next = rt
            .auto_select_assess(rt.core_generation(), None, &mut cache, &readings(&ledger))
            .await;
        assert_eq!(
            next.unwrap_or_else(|| panic!(
                "generation change={change_generation}, decision={:?}",
                last_decision(&rt)
            ))
            .candidate
            .id,
            "node-c",
            "generation change={change_generation}"
        );
    }
}

/// 起点是直连哨兵：`direct` 恒是 `proxy-selector` 的成员，首次落点可经后台事务热切；事务失败时
/// 恢复的旧成员是 `direct`。
#[tokio::test]
async fn a_direct_start_is_placed_by_the_background_transaction() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("__direct__", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    let reads = readings(&ledger);
    let plan = assess(&rt, &reads, None).await.expect("直连起点可热切");
    assert_eq!(plan.current_id, "__direct__");

    let rejecting = AttestingManagementApi::rejecting("direct", "Node C");
    let mut machine = AutoSwitchMachine::new();
    assert!(!commit(&rt, &mut machine, &plan, &rejecting, &reads).await);
    assert_eq!(
        *rejecting.puts.lock().unwrap(),
        vec![
            ("proxy-selector".to_string(), "Node C".to_string()),
            ("proxy-selector".to_string(), "direct".to_string()),
        ],
        "失败后恢复到 direct"
    );
    assert_eq!(exit_on_disk(&rt), "__direct__");

    let api = AttestingManagementApi::new("direct");
    assert!(commit(&rt, &mut machine, &plan, &api, &reads).await);
    assert_eq!(exit_on_disk(&rt), "node-c");
    assert_eq!(intent_on_disk(&rt).as_deref(), Some("sub"));
}

/// 择优腿经真实账本读数凑满连胜：当前出口 `node-b` 连续两轮被明显胜过，两轮里领先的可以是同一个
/// 节点；只胜一轮不换；当前出口的读数停在旧的一轮时（只重测了候选）不计胜。
#[tokio::test]
async fn the_select_leg_takes_a_better_node_after_two_comparable_rounds() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-b", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 300), ("node-c", 100)],
    );
    assert_eq!(assess(&rt, &readings(&ledger), None).await, None);
    assert_eq!(
        last_decision(&rt),
        Decision::Hold(Hold::Pending(Lacking::Streak))
    );
    // 半小时后只有候选被单独重测：当前出口没有同轮的读数可比。
    let later = MONO + 30 * MINUTE;
    round(&ledger, &rt, &cfg, 2, later, &[("node-c", 100)]);
    assert_eq!(assess(&rt, &readings_at(&ledger, later), None).await, None);
    assert_eq!(
        last_decision(&rt),
        Decision::Hold(Hold::Pending(Lacking::Pair))
    );
    // 当前出口也被重测（同一轮）：第二胜，换。
    round(&ledger, &rt, &cfg, 2, later, &[("node-b", 300)]);
    let plan = assess(&rt, &readings_at(&ledger, later), None)
        .await
        .expect("两轮可比的明显更优");
    assert_eq!(
        (plan.candidate.id.as_str(), plan.cause),
        ("node-c", Cause::Better)
    );
}

// ── 与手动选择、订阅范围、平台闸的边界 ────────────────────────────────────────

/// 竞态：评估已给出换点、提交之前用户点了节点 —— 点的就是当前出口，实际出口没变、只有意图
/// 回到了手动（经真实的 `server:switch` 落盘）。这笔换点在落盘那一刻让位：不发任何 selector
/// 调用，出口与手动意图都不动；让位不计入提交失败。
#[tokio::test]
async fn a_manual_selection_between_assess_and_commit_voids_the_commit() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    let reads = readings(&ledger);
    let plan = assess(&rt, &reads, None).await.unwrap();

    let (_, exit_changed, _, intent_cleared) = server_switch_core(
        &rt.config,
        "node-a",
        |_| Ok(()),
        || rt.register_selector_intent(),
    )
    .expect("用户重选当前出口");
    assert!(!exit_changed && intent_cleared);

    let api = AttestingManagementApi::new("Node A");
    let mut machine = AutoSwitchMachine::new();
    assert!(!commit(&rt, &mut machine, &plan, &api, &reads).await);
    assert!(
        api.puts.lock().unwrap().is_empty(),
        "让位发生在任何 selector 调用之前"
    );
    assert_eq!(exit_on_disk(&rt), "node-a");
    assert_eq!(intent_on_disk(&rt), None);
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!(slot.select_switches, 0);
    assert_eq!(slot.memory.commit_failures(), 0);
    assert_eq!(
        slot.last_commit_failure.as_ref().map(|f| f.outcome),
        Some("commitYielded")
    );
}

/// 提交事务落盘时刻的意图复核：凭「自动（订阅甲）」发起的换点，在意图已是手动、或已指向别的
/// 订阅时让位；手动意图下的故障切换（不带这项要求）不受影响。
#[tokio::test]
async fn the_transaction_yields_when_the_auto_intent_it_acted_on_is_gone() {
    let candidate = RuntimeCandidate {
        id: "node-b".to_string(),
        name: "Node B".to_string(),
        tag: "Node B".to_string(),
    };
    for (intent, required, expected) in [
        (None, Some("sub"), AutoHotSwitchOutcome::Superseded),
        (Some("other"), Some("sub"), AutoHotSwitchOutcome::Superseded),
        (Some("sub"), Some("sub"), AutoHotSwitchOutcome::Applied),
        (None, None, AutoHotSwitchOutcome::Applied),
        (Some("sub"), None, AutoHotSwitchOutcome::Applied),
    ] {
        let (rt, _dir) = test_runtime();
        let cfg = running(&rt, &auto_config("node-a", intent));
        let fingerprint = current_server_fingerprints(&cfg).remove("node-b").unwrap();
        let api = AttestingManagementApi::new("Node A");
        let outcome = rt
            .auto_hot_switch_transaction_with_api(
                rt.core_generation(),
                "node-a",
                &candidate,
                &fingerprint,
                required,
                &api,
            )
            .await;
        assert_eq!(outcome, expected, "意图 {intent:?}，要求 {required:?}");
        assert_eq!(
            intent_on_disk(&rt).as_deref(),
            intent,
            "事务只改实际出口，从不动意图"
        );
        let moved = expected == AutoHotSwitchOutcome::Applied;
        assert_eq!(exit_on_disk(&rt), if moved { "node-b" } else { "node-a" });
        assert_eq!(api.puts.lock().unwrap().is_empty(), !moved);
    }
}

/// iOS 本轮不开放：配置里即使存着自动意图也不生效 —— 故障腿不因它启用，择优腿不评估并如实报
/// 「本平台未开放」，显式路径的裁决同样不评估，设置意图的入口据同一个判据拒绝。同一份配置在
/// Android 上生效。
#[tokio::test]
async fn ios_never_acts_on_a_stored_auto_intent() {
    let (rt, _dir) = test_runtime_on(Platform::Ios);
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    let reads = readings(&ledger);
    assert!(!rt.auto_select_active());
    assert_eq!(rt.auto_select_closed(), Some(Gate::PlatformNotOpen));
    assert_eq!(rt.auto_select_fallback(&cfg), None);
    assert_eq!(assess(&rt, &reads, None).await, None);
    assert_eq!(
        last_decision(&rt),
        Decision::NotEvaluated(Gate::PlatformNotOpen)
    );
    assert_eq!(
        rt.auto_select_explicit_from(&cfg, "sub", &reads),
        Decision::NotEvaluated(Gate::PlatformNotOpen)
    );
    let status = rt.auto_select_status_from(&reads);
    assert_eq!(
        (status.intent.mode, status.mode, status.reason),
        ("auto", "notEvaluated", Some("platformNotOpen"))
    );

    let (rt, _dir) = test_runtime_on(Platform::Android);
    running(&rt, &auto_config("node-a", Some("sub")));
    assert!(rt.auto_select_active());
    assert_eq!(rt.auto_select_closed(), None);
    assert_eq!(
        Switches::PRODUCTION,
        Switches {
            master: true,
            better_leg: true
        }
    );
}

/// 没开自动选择的用户：择优腿每次被唤醒都什么也不做 —— 不留评估、不发状态事件，故障腿不因它
/// 启用；故障腿换点成功之后也不往择优的状态里记任何东西。
#[tokio::test]
async fn a_manual_intent_leaves_no_trace() {
    let (rt, _dir) = test_runtime();
    let events = Arc::new(Mutex::new(Vec::new()));
    rt.set_error_emitter(Box::new(RecordingErrorEmitter {
        auto_select_status: Arc::clone(&events),
        ..Default::default()
    }));
    let cfg = running(&rt, &auto_config("node-a", None));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    for _ in 0..3 {
        assert_eq!(assess(&rt, &readings(&ledger), None).await, None);
    }
    assert!(!rt.auto_select_active());
    // 故障腿在手动意图下换了点：`auto_period_ms` 为空。
    rt.auto_select_note_failover(None, failover_record("node-a", "node-b"), MONO);
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!(slot.evaluation, None);
    assert_eq!(slot.last_switch, None);
    assert_eq!((slot.failover_switches, slot.select_switches), (0, 0));
    assert_eq!(
        slot.memory,
        crate::runtime::auto_select::Memory::default(),
        "手动意图下不留任何择优状态"
    );
    assert_eq!(*rt.auto_select_announced.lock().unwrap(), None);
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(rt.config.current().unwrap(), cfg);
}

fn failover_record(from: &str, to: &str) -> SwitchRecord {
    SwitchRecord {
        at: WALL,
        leg: Leg::Failover,
        cause: Cause::Failover,
        from_id: Some(from.to_string()),
        from_name: Some(from.to_string()),
        to_id: to.to_string(),
        to_name: to.to_string(),
        from_latency_ms: None,
        to_latency_ms: Some(120),
        unverified: None,
    }
}

/// 自动意图下故障腿把出口从 `node-c` 换到了 `node-b`：记一次换点，并把 `node-c` 暂时排除。
/// 此后 `node-c` 的测速结果再好，择优腿也不把它当挑战者换回去；状态里读得出它被排除、还剩多久。
/// 排除期满后它才重新被考虑。
#[tokio::test]
async fn a_node_left_by_the_failover_leg_is_not_switched_back_to_while_barred() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-b", Some("sub")));
    let ledger = MeasurementLedger::new();
    let period = 30 * MINUTE;
    rt.auto_select_note_failover(Some(period), failover_record("node-c", "node-b"), MONO);
    {
        let slot = rt.auto_select.lock().unwrap();
        assert_eq!(slot.failover_switches, 1);
        assert!(slot.memory.is_barred("node-c", MONO));
    }
    // 两轮里 node-c 都远好于当前出口。
    for (run, at) in [(1, MONO + 11 * MINUTE), (2, MONO + 41 * MINUTE)] {
        round(
            &ledger,
            &rt,
            &cfg,
            run,
            at,
            &[("node-b", 400), ("node-c", 60)],
        );
        assert_eq!(assess(&rt, &readings_at(&ledger, at), None).await, None);
        assert_eq!(last_decision(&rt), Decision::Hold(Hold::Settled));
    }
    let status = rt.auto_select_status_from(&readings_at(&ledger, MONO + 41 * MINUTE));
    assert_eq!(status.barred.len(), 1);
    assert_eq!(status.barred[0].server_id, "node-c");
    assert_eq!(status.barred[0].remaining_ms, 4 * period - 41 * MINUTE);
    assert_eq!(status.challenger, None);

    // 排除期满之后的两轮：照常计胜、换回。
    let after = MONO + 4 * period;
    round(
        &ledger,
        &rt,
        &cfg,
        3,
        after,
        &[("node-b", 400), ("node-c", 60)],
    );
    assert_eq!(assess(&rt, &readings_at(&ledger, after), None).await, None);
    assert_eq!(
        last_decision(&rt),
        Decision::Hold(Hold::Pending(Lacking::Streak))
    );
    round(
        &ledger,
        &rt,
        &cfg,
        4,
        after + period,
        &[("node-b", 400), ("node-c", 60)],
    );
    let plan = assess(&rt, &readings_at(&ledger, after + period), None)
        .await
        .expect("排除期满后可以换回");
    assert_eq!(plan.candidate.id, "node-c");
}

/// 状态事件：结论变了发一次，同一结论的重复评估不重发（「已持续多久」这类自己会走的量不算
/// 变化）；意图回到手动后发一次手动态并清掉残留。
#[tokio::test]
async fn the_status_event_fires_on_change_only() {
    let (rt, _dir) = test_runtime();
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    rt.set_error_emitter(Box::new(RecordingErrorEmitter {
        auto_select_status: Arc::clone(&events),
        ..Default::default()
    }));
    running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    for beat in 0..3 {
        assess(
            &rt,
            &readings_at(&ledger, MONO + beat * HEARTBEAT_INTERVAL_MS),
            None,
        )
        .await;
    }
    {
        let seen = events.lock().unwrap();
        assert_eq!(seen.len(), 1, "三次评估结论相同，只发一次");
        assert_eq!(seen[0]["intent"]["subscriptionId"], "sub");
        assert_eq!(seen[0]["exit"]["inSubscription"], false);
        assert_eq!(seen[0]["flags"]["exitOutsideSubscription"], true);
    }

    server_switch_core(
        &rt.config,
        "node-a",
        |_| Ok(()),
        || rt.register_selector_intent(),
    )
    .unwrap();
    let reads = readings(&ledger);
    assess(&rt, &reads, None).await;
    assess(&rt, &reads, None).await;
    let seen = events.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[1]["mode"], "manual");
    assert_eq!(seen[1]["lastEvaluation"], Value::Null);
    assert_eq!(rt.auto_select.lock().unwrap().evaluation, None);
}

/// 并发播报不乱序：取状态、比对、发射在同一把锁里。多个线程同时播报、其间状态在变，收到的
/// 事件序列里每个状态至多出现一次连续段 —— 不会出现「新状态之后又来一条旧状态」。
#[test]
fn concurrent_announcements_never_deliver_an_older_status_after_a_newer_one() {
    let (rt, _dir) = test_runtime();
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    rt.set_error_emitter(Box::new(RecordingErrorEmitter {
        auto_select_status: Arc::clone(&events),
        ..Default::default()
    }));
    running(&rt, &auto_config("node-a", Some("sub")));
    let ledger: &'static MeasurementLedger = Box::leak(Box::new(MeasurementLedger::new()));
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..200 {
                    rt.auto_select_announce_from(&readings(ledger));
                }
            });
        }
        scope.spawn(|| {
            for count in 1..=50 {
                rt.auto_select.lock().unwrap().failover_switches = count;
                rt.auto_select_announce_from(&readings(ledger));
            }
        });
    });
    let seen: Vec<u64> = events
        .lock()
        .unwrap()
        .iter()
        .map(|status| status["counters"]["failoverSwitches"].as_u64().unwrap())
        .collect();
    assert!(seen.len() >= 2);
    assert!(
        seen.windows(2).all(|pair| pair[0] < pair[1]),
        "计数只增：每个值恰好发一次、按序到达，{seen:?}"
    );
}

/// 接线（源码级）：故障腿在自动意图下把候选收窄到该订阅、并把那个订阅交给提交事务复核；
/// 心跳里故障腿在自动意图生效时视同启用；换点成功后只在自动意图下记择优的状态。这几处的效果
/// 要真起核真探测才观测得到，这里守的是取材与传参没有被改掉；候选范围与记账本身的行为由别的
/// 用例守。
#[test]
fn the_failover_leg_is_scoped_and_enabled_by_an_effective_auto_intent() {
    let source = module_code("runtime/proxy");
    let body = method_body(
        &source,
        "    async fn do_switch_io(self: &Arc<Self>, machine: &mut AutoSwitchMachine, \
         reason: &str) -> bool {",
    );
    let scope = body
        .find("auto_select::effective_subscription(")
        .expect("候选范围取自生效的自动意图");
    let plan = body.find("plan_runtime_candidates(").expect("候选规划锚点");
    assert!(scope < plan);
    assert!(
        body.contains("scope.as_ref(),"),
        "候选规划必须带上范围；传 None 即回到全部节点"
    );
    let squeezed: String = body.split_whitespace().collect();
    assert!(
        squeezed.contains("expected_fingerprint,auto_subscription,).await"),
        "提交事务必须拿到发起所凭的订阅，否则手动选择之后在飞的故障切换仍会换点"
    );
    assert!(
        squeezed.contains("self.auto_select_note_failover(auto_subscription.map(|subscription|{"),
        "换点成功后的记账以「自动意图是否生效」为条件"
    );
    assert!(
        !body.contains("auto_select_slot()"),
        "故障腿不直接写择优的状态槽"
    );
    let beat = method_body(&source, "    async fn failover_beat(");
    assert!(beat
        .contains("let want_enabled = self.auto_switch_enabled() || self.auto_select_active();"));
}

/// 意图键不进内核配置：只差一个意图键的两份配置，解析出的 `UserConfig` 逐字段相同（生成内核配置
/// 的全部输入），对运行核的切换判定是「无操作」—— 写入意图既不重新生成配置，也不触发热切或重启。
#[test]
fn the_intent_key_never_reaches_the_kernel_config() {
    let (rt, _dir) = test_runtime();
    let manual = running(&rt, &auto_config("node-b", None));
    let mut auto = manual.clone();
    auto["selectionIntent"] = polaris_store::selection_intent_auto("sub");
    let typed = |config: &Value| {
        serde_json::to_value(serde_json::from_value::<UserConfig>(config.clone()).unwrap()).unwrap()
    };
    assert_eq!(typed(&manual), typed(&auto));
    assert!(!UserConfig::FIELD_NAMES.contains(&polaris_store::SELECTION_INTENT_KEY));
    let classified = rt.classify_staged(&auto);
    assert_eq!(
        (classified.decision, classified.restart_required),
        ("noOp", false)
    );
    // 对照：换一个真的进内核配置的键，判定就不是无操作。
    let mut other = manual;
    other["mixedPort"] = serde_json::json!(7999);
    assert_ne!(rt.classify_staged(&other).decision, "noOp");
}

// ── 两条显式命令的落盘：目标取自同一个裁决，账本读数是真的 ────────────────────

fn enable(
    rt: &ProxyRuntime,
    reads: &SelectReadings<'_>,
) -> Result<(Value, Option<crate::commands::server::Placement>), AutoSelectError> {
    auto_select_enable_core(
        &rt.config,
        "sub",
        rt.auto_select_closed(),
        |cfg, subscription| rt.auto_select_explicit_from(cfg, subscription, reads),
        |_| Ok(()),
        || rt.register_selector_intent(),
    )
}

fn switch_now(
    rt: &ProxyRuntime,
    reads: &SelectReadings<'_>,
) -> Result<(Value, crate::commands::server::Placement), AutoSelectError> {
    auto_select_switch_now_core(
        &rt.config,
        rt.auto_select_closed(),
        |cfg, subscription| rt.auto_select_explicit_from(cfg, subscription, reads),
        |_| Ok(()),
        || rt.register_selector_intent(),
    )
}

/// 当前出口已是订阅成员时点「自动选择」：只写意图，出口不动 —— 不论它是订阅里最快的、最慢的，
/// 还是没有结果；连点多少次都一样。「立即切换」在出口已是最快的那个时拒绝，同样不动出口。
#[tokio::test]
async fn enabling_auto_on_a_member_exit_never_moves_it() {
    for (b_ms, c_ms, label) in [
        (40, 300, "当前出口是订阅里最快的"),
        (300, 40, "当前出口是订阅里最慢的"),
    ] {
        let (rt, _dir) = test_runtime();
        let cfg = running(&rt, &auto_config("node-b", None));
        let ledger = MeasurementLedger::new();
        round(
            &ledger,
            &rt,
            &cfg,
            1,
            MONO,
            &[("node-b", b_ms), ("node-c", c_ms)],
        );
        let reads = readings(&ledger);
        for attempt in 0..3 {
            let (saved, placement) = enable(&rt, &reads).expect("写意图");
            assert_eq!(placement, None, "{label}，第 {attempt} 次");
            assert_eq!(saved["selectedServerId"], "node-b", "{label}");
            assert_eq!(intent_on_disk(&rt).as_deref(), Some("sub"));
        }
        assert!(saved_has_no_recent(&rt));
        if b_ms < c_ms {
            // 出口已是最快的：立即切换没有落点。
            for _ in 0..2 {
                assert_eq!(
                    switch_now(&rt, &reads).unwrap_err(),
                    AutoSelectError::NoTarget("settled"),
                    "{label}"
                );
                assert_eq!(exit_on_disk(&rt), "node-b");
            }
        } else {
            // 出口明显更慢，但只有一轮读数：连胜没凑满，立即切换同样不换。
            assert_eq!(
                switch_now(&rt, &reads).unwrap_err(),
                AutoSelectError::NoTarget("challengerPending")
            );
            assert_eq!(exit_on_disk(&rt), "node-b");
        }
    }
    // 成员出口没有任何结果：点「自动选择」也只写意图。
    let (rt, _dir) = test_runtime();
    running(&rt, &auto_config("node-b", None));
    let ledger = MeasurementLedger::new();
    let (saved, placement) = enable(&rt, &readings(&ledger)).unwrap();
    assert_eq!(
        (placement, saved["selectedServerId"].as_str()),
        (None, Some("node-b"))
    );
}

fn saved_has_no_recent(rt: &ProxyRuntime) -> bool {
    rt.config
        .current()
        .unwrap()
        .get("recentServerIds")
        .is_none()
}

/// 当前出口不属于该订阅时点「自动选择」：有新鲜候选即在同一次写里落到延迟最小者（首次落点），
/// 没有则只写意图、出口不动（不取成员表第一个）。落点不进最近节点列表。核不在运行时不落点。
#[tokio::test]
async fn enabling_auto_from_outside_the_subscription_places_on_the_fastest_member() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", None));
    let ledger = MeasurementLedger::new();
    let (saved, placement) = enable(&rt, &readings(&ledger)).expect("只写意图");
    assert_eq!(placement, None);
    assert_eq!(saved["selectedServerId"], "node-a");

    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80), ("node-a", 5)],
    );
    let (saved, placement) = enable(&rt, &readings(&ledger)).expect("写意图并落点");
    let placement = placement.expect("首次落点");
    assert_eq!(
        (
            placement.to.as_str(),
            placement.latency_ms,
            placement.cause,
            placement.from.as_deref(),
            placement.exit_changed
        ),
        ("node-c", 80, Cause::FirstPlacement, Some("node-a"), true)
    );
    assert_eq!(saved["selectedServerId"], "node-c");
    assert_eq!(intent_on_disk(&rt).as_deref(), Some("sub"));
    assert!(saved_has_no_recent(&rt));
    // 再点一次：出口已在订阅内，不再动。
    assert_eq!(enable(&rt, &readings(&ledger)).unwrap().1, None);
    assert_eq!(exit_on_disk(&rt), "node-c");

    // 核不在运行：账本里没有当前结果，只写意图。
    let (stopped, _dir) = test_runtime();
    stopped
        .config
        .save_full(&auto_config("node-a", None))
        .unwrap();
    let (saved, placement) = enable(&stopped, &readings(&ledger)).unwrap();
    assert_eq!(
        (placement, saved["selectedServerId"].as_str()),
        (None, Some("node-a"))
    );
}

/// 「立即切换」给出落点的情形，目标都是裁决认定的那一个：后台因须整核重启换不了的首次落点
/// （起点是阻断）；当前出口测速失败；当前出口没有新鲜结果；当前出口已连续两轮被明显胜过。
/// 意图保留。意图是手动时拒绝。
#[tokio::test]
async fn switch_now_goes_exactly_where_the_decision_points() {
    // 后台报「需重启才能切换」的那个场景：显式路径换得过去。
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config(BLOCK_SERVER_ID, Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    let reads = readings(&ledger);
    assert_eq!(assess(&rt, &reads, None).await, None);
    assert_eq!(last_decision(&rt), Decision::Hold(Hold::NeedsRestart));
    let (saved, placement) = switch_now(&rt, &reads).expect("立即切换");
    assert_eq!(
        (placement.to.as_str(), placement.cause),
        ("node-c", Cause::FirstPlacement)
    );
    assert_eq!(saved["selectedServerId"], "node-c");
    assert_eq!(
        intent_on_disk(&rt).as_deref(),
        Some("sub"),
        "显式的立即切换不是手动选择"
    );

    // 当前出口测速失败（一次即可）与没有新鲜结果。
    for (seed, cause) in [(true, Cause::CurrentFailed), (false, Cause::Requested)] {
        let (rt, _dir) = test_runtime();
        let cfg = running(&rt, &auto_config("node-b", Some("sub")));
        let ledger = MeasurementLedger::new();
        if seed {
            ledger.record_at(
                "node-b",
                Err(MeasureFailure::new(FailPhase::Measure, FailKind::Timeout)),
                identity(&rt, &cfg, 1),
                MONO,
            );
        }
        round(&ledger, &rt, &cfg, 1, MONO, &[("node-c", 80)]);
        let (_, placement) = switch_now(&rt, &readings(&ledger)).expect("立即切换");
        assert_eq!((placement.to.as_str(), placement.cause), ("node-c", cause));
    }

    // 已凑满连胜的更优者。
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-b", Some("sub")));
    let ledger = MeasurementLedger::new();
    for (run, at) in [(1, MONO), (2, MONO + 30 * MINUTE)] {
        round(
            &ledger,
            &rt,
            &cfg,
            run,
            at,
            &[("node-b", 300), ("node-c", 100)],
        );
        if run == 1 {
            assert_eq!(assess(&rt, &readings_at(&ledger, at), None).await, None);
        }
    }
    let (_, placement) =
        switch_now(&rt, &readings_at(&ledger, MONO + 30 * MINUTE)).expect("立即切换");
    assert_eq!(
        (placement.to.as_str(), placement.cause),
        ("node-c", Cause::Better)
    );

    // 意图是手动。
    let (rt, _dir) = test_runtime();
    running(&rt, &auto_config("node-b", None));
    let ledger = MeasurementLedger::new();
    assert_eq!(
        switch_now(&rt, &readings(&ledger)).unwrap_err(),
        AutoSelectError::NotAuto
    );
}

// ── 心跳循环：何时醒、醒了做什么（假时钟）──────────────────────────────────────

/// 记录两条腿各自被调用的时刻（距循环起点的毫秒）。故障腿每拍耗时 `work`。
struct RecordingLegs {
    started: tokio::time::Instant,
    work: Duration,
    data: Arc<tokio::sync::Notify>,
    alive: Arc<std::sync::atomic::AtomicBool>,
    failovers: Arc<Mutex<Vec<u64>>>,
    selects: Arc<Mutex<Vec<u64>>>,
    /// 第几拍之后故障腿要求任务退场。
    break_after: Option<usize>,
}

impl RecordingLegs {
    fn elapsed(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap()
    }
}

impl HeartbeatLegs for RecordingLegs {
    fn current(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn data_settled(&self) {
        self.data.notified().await;
    }

    async fn failover(&mut self) -> ControlFlow<()> {
        let at = self.elapsed();
        self.failovers.lock().unwrap().push(at);
        tokio::time::sleep(self.work).await;
        if self
            .break_after
            .is_some_and(|after| self.failovers.lock().unwrap().len() >= after)
        {
            return ControlFlow::Break(());
        }
        ControlFlow::Continue(())
    }

    async fn select(&mut self) {
        let at = self.elapsed();
        self.selects.lock().unwrap().push(at);
    }
}

struct Loop {
    data: Arc<tokio::sync::Notify>,
    intent: Arc<tokio::sync::Notify>,
    alive: Arc<std::sync::atomic::AtomicBool>,
    failovers: Arc<Mutex<Vec<u64>>>,
    selects: Arc<Mutex<Vec<u64>>>,
    task: tokio::task::JoinHandle<()>,
}

const TICK: Duration = Duration::from_millis(HEARTBEAT_INTERVAL_MS);

/// 在当前（暂停的）时钟上起一个心跳循环。
fn spawn_loop(work: Duration, break_after: Option<usize>) -> Loop {
    let data = Arc::new(tokio::sync::Notify::new());
    let intent = Arc::new(tokio::sync::Notify::new());
    let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let failovers = Arc::new(Mutex::new(Vec::new()));
    let selects = Arc::new(Mutex::new(Vec::new()));
    let mut legs = RecordingLegs {
        started: tokio::time::Instant::now(),
        work,
        data: Arc::clone(&data),
        alive: Arc::clone(&alive),
        failovers: Arc::clone(&failovers),
        selects: Arc::clone(&selects),
        break_after,
    };
    let intent_wake = Arc::clone(&intent);
    let task = tokio::spawn(async move { run_heartbeat(&mut legs, TICK, &intent_wake).await });
    Loop {
        data,
        intent,
        alive,
        failovers,
        selects,
        task,
    }
}

/// 让循环跑到假时钟的 `ms` 处（从当前时刻起再走这么多毫秒）。
async fn advance(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    tokio::task::yield_now().await;
}

/// 没有别的唤醒时（意图缺席的用户就是这样：择优腿的一步什么都不做），连通性探测的节奏与只有
/// 故障腿的原循环相同：「睡一拍 → 干活（耗时 d）→ 再睡一拍」，第 n 次探测在 n·拍长 + (n−1)·d。
#[tokio::test(start_paused = true)]
async fn the_probe_cadence_matches_sleep_then_work() {
    let work = 1_700;
    let ran = spawn_loop(Duration::from_millis(work), None);
    advance(5 * (HEARTBEAT_INTERVAL_MS + work) + 1).await;
    let expected: Vec<u64> = (0..5)
        .map(|n| (n + 1) * HEARTBEAT_INTERVAL_MS + n * work)
        .collect();
    assert_eq!(*ran.failovers.lock().unwrap(), expected);
    // 每拍之后择优腿走一步，在故障腿做完的那一刻。
    assert_eq!(
        *ran.selects.lock().unwrap(),
        expected.iter().map(|at| at + work).collect::<Vec<_>>()
    );
    ran.task.abort();
}

/// 账本变化与意图写入的唤醒只走择优腿，不提前也不推迟连通性探测：拍与拍之间无论插进多少次
/// 这样的唤醒，探测的时刻一毫秒都不动。
#[tokio::test(start_paused = true)]
async fn data_and_intent_wakes_run_only_the_select_leg_and_never_move_the_beat() {
    let work = 500;
    let ran = spawn_loop(Duration::from_millis(work), None);
    // 第一拍之前插两次，第一拍与第二拍之间插三次（其中一次紧贴着下一拍）。
    advance(1_000).await;
    ran.data.notify_one();
    advance(9_000).await;
    ran.intent.notify_one();
    advance(HEARTBEAT_INTERVAL_MS - 10_000 + work + 1_000).await;
    ran.data.notify_one();
    advance(15_000).await;
    ran.intent.notify_one();
    advance(HEARTBEAT_INTERVAL_MS - 16_000 - 1).await;
    ran.data.notify_one();
    advance(2 * HEARTBEAT_INTERVAL_MS).await;

    let beats = ran.failovers.lock().unwrap().clone();
    assert_eq!(
        &beats[..3],
        [
            HEARTBEAT_INTERVAL_MS,
            2 * HEARTBEAT_INTERVAL_MS + work,
            3 * HEARTBEAT_INTERVAL_MS + 2 * work
        ],
        "与没有这些唤醒时逐毫秒相同"
    );
    let selects = ran.selects.lock().unwrap().clone();
    let extra: Vec<u64> = selects
        .iter()
        .copied()
        .filter(|at| !beats.iter().any(|beat| beat + work == *at))
        .collect();
    assert_eq!(
        extra,
        [
            1_000,
            10_000,
            HEARTBEAT_INTERVAL_MS + work + 1_000,
            HEARTBEAT_INTERVAL_MS + work + 16_000,
            2 * HEARTBEAT_INTERVAL_MS + work - 1
        ],
        "五次唤醒各走一步择优腿，当场走"
    );
    ran.task.abort();
}

/// 世代守卫与退场：核被接管后，下一次唤醒（哪一路都算）即退出，两条腿都不再走；故障腿要求
/// 退场时当拍退出、不再走择优腿。
#[tokio::test(start_paused = true)]
async fn the_loop_exits_on_a_generation_change_or_a_break() {
    let ran = spawn_loop(Duration::ZERO, None);
    advance(HEARTBEAT_INTERVAL_MS).await;
    assert_eq!(ran.failovers.lock().unwrap().len(), 1);
    ran.alive.store(false, Ordering::SeqCst);
    ran.data.notify_one();
    advance(1).await;
    assert!(ran.task.is_finished(), "数据唤醒也过世代守卫");
    advance(3 * HEARTBEAT_INTERVAL_MS).await;
    assert_eq!(ran.failovers.lock().unwrap().len(), 1);
    assert_eq!(ran.selects.lock().unwrap().len(), 1);

    let ran = spawn_loop(Duration::ZERO, Some(2));
    advance(5 * HEARTBEAT_INTERVAL_MS).await;
    assert!(ran.task.is_finished());
    assert_eq!(ran.failovers.lock().unwrap().len(), 2);
    assert_eq!(
        ran.selects.lock().unwrap().len(),
        1,
        "要求退场的那一拍不再走择优腿"
    );
}

/// 接线（源码级）：生产的心跳任务跑的就是这个循环，循环之外没有第二处睡眠或唤醒。
#[test]
fn the_production_heartbeat_runs_this_loop() {
    let source = module_code("runtime/proxy");
    let spawn = method_body(&source, "    pub(super) fn spawn_auto_switch_heartbeat(");
    assert!(spawn.contains("run_heartbeat(&mut legs, tick, &me.auto_select_wake).await;"));
    assert!(spawn.contains("let tick = Duration::from_millis(HEARTBEAT_INTERVAL_MS);"));
    let settled = method_body(&source, "    async fn data_settled(&self) {");
    assert!(settled.contains("speed_test_data_settled(self.seen).await;"));
    let tick = method_body(&source, "    async fn auto_select_tick(");
    let tick: String = tick.split_whitespace().collect();
    assert!(tick.contains(
        "self.auto_select_tick_from(generation,machine,cache,&readings,self.management_api())"
    ));
    for forbidden in ["tokio::time::sleep", "tokio::select!", "loop {"] {
        assert!(
            !spawn.contains(forbidden),
            "挂载处出现了 `{forbidden}`：节奏只能由 run_heartbeat 决定"
        );
    }
}

fn explicit_placement(rt: &ProxyRuntime, target: &str) -> Placement {
    Placement {
        from: Some("node-a".into()),
        to: target.into(),
        latency_ms: 20,
        cause: Cause::Better,
        exit_changed: true,
        intent_generation: rt.register_selector_intent(),
    }
}

#[tokio::test]
async fn explicit_placement_runtime_error_does_not_record_a_switch() {
    let (rt, _dir) = test_runtime();
    rt.config
        .save_full(&auto_config("node-b", Some("sub")))
        .unwrap();
    mark_running(&rt); // No runtime baseline: the real switch path must return Err.
    let saved = rt.config.current().unwrap();
    let placement = explicit_placement(&rt, "node-b");
    let result = serde_json::to_value(finish_placement_core(&rt, &saved, placement).await).unwrap();
    assert_eq!(result["status"], "failed");
    assert!(result["error"].as_str().unwrap().contains("当前配置基准"));
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!(slot.select_switches, 0);
    assert!(slot.last_switch.is_none());
}

#[tokio::test]
async fn explicit_placement_superseded_or_stopped_does_not_record_a_switch() {
    for superseded in [false, true] {
        let (rt, _dir) = test_runtime();
        rt.config
            .save_full(&auto_config("node-b", Some("sub")))
            .unwrap();
        let saved = rt.config.current().unwrap();
        let placement = explicit_placement(&rt, "node-b");
        if superseded {
            rt.register_selector_intent();
        }
        let result =
            serde_json::to_value(finish_placement_core(&rt, &saved, placement).await).unwrap();
        assert_eq!(
            result["status"],
            if superseded {
                "superseded"
            } else {
                "notRunning"
            }
        );
        let slot = rt.auto_select.lock().unwrap();
        assert_eq!(slot.select_switches, 0);
        assert!(slot.last_switch.is_none());
    }
}

#[tokio::test]
async fn explicit_placement_deferred_does_not_record_but_applied_does() {
    for deferred in [false, true] {
        let (rt, _dir, sink, mut baseline) = explicit_selection_fixture();
        if deferred {
            baseline["proxyMode"] = serde_json::json!("direct");
            mark_running_with_snapshot(&rt, &baseline);
        }
        if !deferred {
            *sink.groups.lock().unwrap() = Some(vec![group(PROXY_SELECTOR_TAG, "Node B")]);
        }
        let target = if deferred { BLOCK_SERVER_ID } else { "node-b" };
        let mut saved = baseline;
        saved["selectedServerId"] = serde_json::json!(target);
        rt.config.save_full(&saved).unwrap();
        let placement = explicit_placement(&rt, target);
        let result =
            serde_json::to_value(finish_placement_core(&rt, &saved, placement).await).unwrap();
        assert_eq!(
            result["status"],
            if deferred { "deferred" } else { "applied" }
        );
        let slot = rt.auto_select.lock().unwrap();
        assert_eq!(slot.select_switches, u64::from(!deferred));
        assert_eq!(slot.last_switch.is_some(), !deferred);
        assert_eq!(sink.calls().is_empty(), deferred);
    }
}

#[tokio::test]
async fn explicit_placement_pending_does_not_record_a_switch() {
    let (rt, _dir, _sink, baseline) = explicit_selection_fixture();
    let mut saved = baseline;
    saved["selectedServerId"] = serde_json::json!("node-b");
    rt.config.save_full(&saved).unwrap();
    rt.gate.begin();
    let placement = explicit_placement(&rt, "node-b");
    let result = serde_json::to_value(finish_placement_core(&rt, &saved, placement).await).unwrap();
    assert_eq!(result["status"], "pending");
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!(slot.select_switches, 0);
    assert!(slot.last_switch.is_none());
}

#[tokio::test]
async fn explicit_placement_confirmed_restart_records_the_accepted_switch() {
    let (rt, _dir, sink, mut baseline) = explicit_selection_fixture();
    baseline["proxyMode"] = serde_json::json!("smart");
    mark_running_with_snapshot(&rt, &baseline);
    *rt.startup_snapshot.write().unwrap() = Some(baseline.clone());
    let mut saved = baseline;
    saved["selectedServerId"] = serde_json::json!(BLOCK_SERVER_ID);
    rt.config.save_full(&saved).unwrap();
    let placement = explicit_placement(&rt, BLOCK_SERVER_ID);
    let result = serde_json::to_value(finish_placement_core(&rt, &saved, placement).await).unwrap();
    assert_eq!(result["status"], "restarting");
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!(slot.select_switches, 1);
    assert_eq!(slot.last_switch.as_ref().unwrap().to_id, BLOCK_SERVER_ID);
    assert!(sink.calls().is_empty());
    assert!(rt.pending_force_restart.read().unwrap().is_some());
}

#[tokio::test]
async fn production_intent_arming_clears_evaluation_and_wakes_the_heartbeat() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    round(
        &ledger,
        &rt,
        &cfg,
        1,
        MONO,
        &[("node-b", 120), ("node-c", 80)],
    );
    assert!(assess(&rt, &readings(&ledger), None).await.is_some());
    assert!(rt.auto_select.lock().unwrap().evaluation.is_some());
    rt.auto_select_armed();
    {
        let slot = rt.auto_select.lock().unwrap();
        assert!(slot.evaluation.is_none());
        assert!(slot.intent_set_mono.is_some());
    }
    tokio::time::timeout(Duration::from_secs(1), rt.auto_select_wake.notified())
        .await
        .expect("writing an automatic intent must wake the live heartbeat");
    assert!(assess(&rt, &readings(&ledger), None).await.is_some());
    rt.auto_select_disarmed();
    let slot = rt.auto_select.lock().unwrap();
    assert!(slot.evaluation.is_none());
    assert!(slot.intent_set_mono.is_none());
}

#[tokio::test]
async fn production_failover_bookkeeping_starts_better_leg_dwell() {
    let (rt, _dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-b", Some("sub")));
    let ledger = MeasurementLedger::new();
    // The failed former node is outside this subscription, leaving C eligible.
    rt.auto_select_note_failover(
        Some(30 * MINUTE),
        failover_record("failed-old", "node-b"),
        MONO,
    );
    for (run, at) in [(1, MONO), (2, MONO + 5 * MINUTE)] {
        round(
            &ledger,
            &rt,
            &cfg,
            run,
            at,
            &[("node-b", 400), ("node-c", 10)],
        );
        assert!(assess(&rt, &readings_at(&ledger, at), None).await.is_none());
    }
    assert_eq!(
        last_decision(&rt),
        Decision::Hold(Hold::Pending(Lacking::Dwell))
    );
    assert_eq!(rt.auto_select.lock().unwrap().failover_switches, 1);
}

#[test]
fn production_refresh_fallback_uses_current_receipts_and_surviving_members() {
    let (rt, _dir) = test_runtime_on(Platform::Linux);
    let mut config = auto_config("node-a", Some("sub"));
    // Unique IDs isolate this production-global ledger fixture from parallel tests.
    config["servers"][1]["id"] = serde_json::json!("me04-refresh-slow");
    config["servers"][2]["id"] = serde_json::json!("me04-refresh-fast");
    let saved = running(&rt, &config);
    let ledger = crate::runtime::measurement_ledger::global();
    for (id, latency) in [("me04-refresh-slow", 80), ("me04-refresh-fast", 20)] {
        let mut receipt = identity(&rt, &saved, 1);
        receipt.measured_at = now_ms();
        assert!(ledger.record(id, Ok(latency), receipt));
    }
    assert_eq!(
        rt.auto_select_fallback(&saved).unwrap().node_id,
        "me04-refresh-fast"
    );
    rt.auto_select_note_failover(
        Some(30 * MINUTE),
        failover_record("me04-refresh-fast", "node-a"),
        crate::runtime::proxy::lifecycle::monotonic_now_ms(),
    );
    assert_eq!(
        rt.auto_select_fallback(&saved).unwrap().node_id,
        "me04-refresh-slow",
        "the live production wrapper must consume the bar, not only the injected seam"
    );
    let mut refreshed = saved;
    refreshed["servers"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["id"] != "me04-refresh-fast");
    assert_eq!(
        rt.auto_select_fallback(&refreshed).unwrap().node_id,
        "me04-refresh-slow"
    );
    refreshed.as_object_mut().unwrap().remove("selectionIntent");
    assert!(rt.auto_select_fallback(&refreshed).is_none());
}

// Refresh uses the same production entry, with only the ledger and clocks injected.
fn refresh_fixture() -> (Arc<ProxyRuntime>, TestDir, Value, MeasurementLedger) {
    let (rt, dir) = test_runtime_on(Platform::Linux);
    let cfg = running(&rt, &auto_config("node-a", Some("sub")));
    let ledger = MeasurementLedger::new();
    refresh_receipts(&rt, &cfg, &ledger, 1, &[("node-b", 80), ("node-c", 20)]);
    (rt, dir, cfg, ledger)
}

fn refresh_receipts(
    rt: &ProxyRuntime,
    cfg: &Value,
    ledger: &MeasurementLedger,
    run: u64,
    results: &[(&str, u32)],
) {
    let fingerprints = crate::commands::speedtest::current_server_fingerprints(cfg);
    for (node, latency) in results {
        let mut receipt = identity(rt, cfg, run);
        receipt.node_fingerprint = fingerprints.get(*node).cloned();
        assert!(ledger.record_at(node, Ok(*latency), receipt, MONO));
    }
}

#[test]
fn refresh_fallback_prefers_unbarred_eligible_nodes_and_restores_them_at_expiry() {
    let (rt, _dir, cfg, ledger) = refresh_fixture();
    let period = 30 * MINUTE;
    rt.auto_select_note_failover(Some(period), failover_record("node-c", "node-a"), MONO);
    let end = MONO + 4 * period;
    for at in [MONO, end - 1] {
        assert_eq!(
            rt.auto_select_fallback_from(&cfg, &readings_at(&ledger, at))
                .unwrap()
                .node_id,
            "node-b",
            "the fresh fastest node is barred, so prefer the fresh slower node"
        );
    }
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings_at(&ledger, end))
            .unwrap()
            .node_id,
        "node-c",
        "the bar uses the injected monotonic clock, independently of receipt freshness"
    );
    assert_eq!(
        rt.config.current().unwrap(),
        cfg,
        "fallback must not write config or intent"
    );
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!((slot.failover_switches, slot.select_switches), (1, 0));
    assert_eq!(slot.last_switch.as_ref().unwrap().to_id.as_str(), "node-a");
}

#[test]
fn refresh_fallback_rescues_a_barred_node_only_when_other_nodes_are_ineligible() {
    for other in ["deleted", "expired", "changed", "failed", "unmeasured"] {
        let (rt, _dir, mut cfg, ledger) = refresh_fixture();
        rt.auto_select_note_failover(Some(30 * MINUTE), failover_record("node-c", "node-a"), MONO);
        match other {
            "deleted" => cfg["servers"]
                .as_array_mut()
                .unwrap()
                .retain(|node| node["id"] != "node-b"),
            "changed" => cfg["servers"][1]["port"] = serde_json::json!(19001),
            "expired" => {
                let mut old = identity(&rt, &cfg, 2);
                old.measured_at = WALL - 61 * MINUTE;
                assert!(ledger.record_at("node-b", Ok(1), old, MONO));
            }
            "failed" => {
                assert!(ledger.record_at(
                    "node-b",
                    Err(MeasureFailure::new(FailPhase::Measure, FailKind::Timeout)),
                    identity(&rt, &cfg, 2),
                    MONO
                ));
            }
            "unmeasured" => {
                cfg["servers"][1]["id"] = serde_json::json!("never-measured");
            }
            _ => unreachable!(),
        }
        assert_eq!(
            rt.auto_select_fallback_from(&cfg, &readings(&ledger))
                .unwrap()
                .node_id,
            "node-c",
            "{other}: no eligible unbarred node remains"
        );
    }
}

#[test]
fn refresh_fallback_never_rescues_an_ineligible_barred_node() {
    for invalid in [
        "expired",
        "changed",
        "deleted",
        "generation",
        "stopped",
        "mismatch",
        "disconnected",
    ] {
        let (rt, _dir, mut cfg, ledger) = refresh_fixture();
        cfg["servers"]
            .as_array_mut()
            .unwrap()
            .retain(|node| node["id"] != "node-b");
        rt.auto_select_note_failover(Some(30 * MINUTE), failover_record("node-c", "node-a"), MONO);
        match invalid {
            "expired" => {
                let mut old = identity(&rt, &cfg, 2);
                old.measured_at = WALL - 61 * MINUTE;
                assert!(ledger.record_at("node-c", Ok(1), old, MONO));
            }
            "changed" => cfg["servers"][1]["port"] = serde_json::json!(19002),
            "deleted" => cfg["servers"]
                .as_array_mut()
                .unwrap()
                .retain(|node| node["id"] != "node-c"),
            "generation" => {
                rt.gate.bump_generation();
            }
            "stopped" => rt.status.write().unwrap().running = false,
            "mismatch" | "disconnected" => {
                let mut bad = identity(&rt, &cfg, 2);
                if invalid == "mismatch" {
                    bad.binding = Some(BindingVerdict::Mismatch);
                } else {
                    bad.instance = CoreInstance::Temp;
                }
                assert!(ledger.record_at("node-c", Ok(1), bad, MONO));
            }
            _ => unreachable!(),
        }
        assert_eq!(
            rt.auto_select_fallback_from(&cfg, &readings(&ledger)),
            None,
            "{invalid}"
        );
    }
}

#[test]
fn refresh_fallback_does_not_cross_subscriptions_even_when_only_barred_members_remain() {
    let (rt, _dir, mut cfg, ledger) = refresh_fixture();
    cfg["servers"][0]["subscriptionId"] = serde_json::json!("other");
    refresh_receipts(&rt, &cfg, &ledger, 2, &[("node-a", 1)]);
    rt.auto_select_note_failover(Some(30 * MINUTE), failover_record("node-c", "node-a"), MONO);
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings(&ledger))
            .unwrap()
            .node_id,
        "node-b"
    );
    cfg["servers"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["id"] != "node-b");
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings(&ledger))
            .unwrap()
            .node_id,
        "node-c"
    );
    cfg["servers"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["id"] != "node-c");
    assert_eq!(rt.auto_select_fallback_from(&cfg, &readings(&ledger)), None);
}

#[test]
fn refresh_fallback_requires_an_effective_subscription_intent_and_open_platform() {
    let (rt, _dir, cfg, ledger) = refresh_fixture();
    for invalid in ["manual", "missing", "empty", "all", "malformed"] {
        let mut config = cfg.clone();
        match invalid {
            "manual" => {
                config.as_object_mut().unwrap().remove("selectionIntent");
            }
            "missing" => config["selectionIntent"] = polaris_store::selection_intent_auto("absent"),
            "empty" => config["selectionIntent"] = polaris_store::selection_intent_auto("other"),
            "all" => config["selectionIntent"] = serde_json::json!({"mode":"auto","scope":"all"}),
            "malformed" => config["selectionIntent"] = serde_json::json!(true),
            _ => unreachable!(),
        }
        assert_eq!(
            rt.auto_select_fallback_from(&config, &readings(&ledger)),
            None,
            "{invalid}"
        );
    }
    let (ios, _dir) = test_runtime_on(Platform::Ios);
    let config = running(&ios, &cfg);
    assert_eq!(
        ios.auto_select_fallback_from(&config, &readings(&ledger)),
        None
    );
}

#[test]
fn refresh_fallback_aligns_existing_epoch_before_reading_the_bar() {
    let (rt, _dir, mut cfg, ledger) = refresh_fixture();
    rt.auto_select_fallback_from(&cfg, &readings(&ledger));
    rt.auto_select_note_failover(Some(30 * MINUTE), failover_record("node-c", "node-a"), MONO);
    rt.gate.bump_generation();
    refresh_receipts(&rt, &cfg, &ledger, 2, &[("node-b", 80), ("node-c", 20)]);
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings(&ledger))
            .unwrap()
            .node_id,
        "node-b",
        "the existing epoch contract retains the bar across a core generation change"
    );
    cfg["selectionIntent"] = polaris_store::selection_intent_auto("other");
    cfg["servers"][1]["subscriptionId"] = serde_json::json!("other");
    cfg["servers"][2]["subscriptionId"] = serde_json::json!("other");
    refresh_receipts(&rt, &cfg, &ledger, 3, &[("node-b", 80), ("node-c", 20)]);
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings(&ledger))
            .unwrap()
            .node_id,
        "node-c"
    );
    assert!(
        rt.auto_select
            .lock()
            .unwrap()
            .memory
            .barred(MONO)
            .is_empty(),
        "bar belongs to the previous subscription"
    );
}

#[test]
fn refresh_fallback_after_current_deletion_keeps_same_tier_latency_and_tie_order() {
    let (rt, _dir, mut cfg, ledger) = refresh_fixture();
    cfg["servers"]
        .as_array_mut()
        .unwrap()
        .retain(|node| node["id"] != "node-a");
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings(&ledger))
            .unwrap()
            .node_id,
        "node-c"
    );
    for id in ["node-b", "node-c"] {
        rt.auto_select_note_failover(Some(30 * MINUTE), failover_record(id, "node-a"), MONO);
    }
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings(&ledger))
            .unwrap()
            .node_id,
        "node-c",
        "all eligible members barred: rescue the fastest"
    );
    refresh_receipts(&rt, &cfg, &ledger, 2, &[("node-b", 20), ("node-c", 20)]);
    assert_eq!(
        rt.auto_select_fallback_from(&cfg, &readings(&ledger))
            .unwrap()
            .node_id,
        "node-b",
        "tie order remains configuration order"
    );
}
