//! Native Auto ownership tests plus retained generic failover/heartbeat receipts.
//! Retired App-ranking policy stays covered by runtime::auto_select pure tests;
//! production tests below exercise native group state through the existing PUT harness.
use super::*;
use crate::commands::server::{finish_placement_core, server_switch_core, Placement};
use crate::runtime::auto_select::{Cause, Leg, SwitchRecord};
use crate::runtime::auto_switch::HEARTBEAT_INTERVAL_MS;
use crate::runtime::proxy::auto_switch::{run_heartbeat, HeartbeatLegs};
use polaris_config_engine::user_config::auto_select::{self as native, AutoSelectTarget};
use polaris_singbox_grpc::daemon::{Group, GroupItem};
use polaris_switch_engine::ConnectionSnapshot;
use std::ops::ControlFlow;
use std::time::Duration;
const MINUTE: u64 = 60_000;
const MONO: u64 = 7 * 60 * MINUTE;
const WALL: u64 = 1_700_000_000_000;

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

fn running(rt: &ProxyRuntime, cfg: &Value) -> Value {
    rt.config.save_full(cfg).expect("seed D");
    let saved = rt.config.current().unwrap();
    mark_running_with_snapshot(rt, &saved);
    *rt.test_measurement_core_sha256.lock().unwrap() = Some((rt.core_generation(), "a".repeat(64)));
    rt.switch_snapshot
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .id_to_tag
        .insert("node-c".to_string(), "Node C".to_string());
    saved
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

// Explicit barriers are coordinator inputs, independent of the retired ranker.
fn failover_fixture() -> (
    Arc<ProxyRuntime>,
    TestDir,
    Value,
    Vec<crate::runtime::auto_switch::RuntimeCandidate>,
    BTreeSet<String>,
) {
    let (rt, dir) = test_runtime();
    let cfg = running(&rt, &auto_config("node-a", None));
    rt.auto_select_note_failover(Some(30 * MINUTE), failover_record("node-c", "node-a"), MONO);
    let barred = BTreeSet::from(["node-c".to_owned()]);
    let snapshot = rt.switch_snapshot.read().unwrap().clone().unwrap();
    let scope = BTreeSet::from(["node-b".to_owned(), "node-c".to_owned()]);
    let mut plan = crate::runtime::auto_switch::plan_runtime_candidates(
        &cfg,
        Some("node-a"),
        &snapshot.id_to_tag,
        &snapshot.measurement_fingerprints,
        &crate::runtime::node_fingerprints::measurement_table_json(&cfg),
        &BTreeSet::new(),
        &BTreeSet::new(),
        Some(&scope),
    );
    assert!(rt.retain_hot_switchable_candidates(rt.core_generation(), &cfg, &mut plan));
    assert_eq!(plan.candidates.len(), 2);
    (rt, dir, cfg, plan.candidates, barred)
}

fn failover_batch(
    entries: &[(&str, crate::runtime::auto_switch::CandidateProbe)],
) -> crate::commands::speedtest::RuntimeProbeBatch {
    crate::commands::speedtest::RuntimeProbeBatch::Completed(
        entries
            .iter()
            .map(|(id, outcome)| (id.to_string(), *outcome))
            .collect(),
    )
}

fn failover_ids(input: &[(String, String)]) -> Vec<String> {
    input.iter().map(|(id, _)| id.clone()).collect()
}

fn failover_best(verdict: Option<crate::runtime::auto_switch::ProbeVerdict>) -> (String, u32) {
    match verdict {
        Some(crate::runtime::auto_switch::ProbeVerdict::Best(best)) => {
            (best.id, best.latency_ms.unwrap())
        }
        other => panic!("expected a measured candidate, got {other:?}"),
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
async fn explicit_placement_structural_change_defers_without_recording_or_implicit_restart() {
    let (rt, _dir, sink, mut baseline) = explicit_selection_fixture();
    baseline["proxyMode"] = serde_json::json!("smart");
    mark_running_with_snapshot(&rt, &baseline);
    *rt.startup_snapshot.write().unwrap() = Some(baseline.clone());
    let runtime = rt.current_config_snapshot();
    let mut saved = baseline;
    saved["selectedServerId"] = serde_json::json!(BLOCK_SERVER_ID);
    rt.config.save_full(&saved).unwrap();
    let placement = explicit_placement(&rt, BLOCK_SERVER_ID);
    let result = serde_json::to_value(finish_placement_core(&rt, &saved, placement).await).unwrap();
    assert_eq!(result["status"], "pending");
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!(slot.select_switches, 0);
    assert!(slot.last_switch.is_none());
    assert!(sink.calls().is_empty());
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    assert_eq!(rt.current_config_snapshot(), runtime);
}

#[tokio::test]
async fn failover_fallback_prefers_unbarred_without_probing_the_faster_barred_node() {
    use crate::runtime::auto_switch::CandidateProbe::Measured;
    let (rt, _dir, cfg, candidates, barred) = failover_fixture();
    let mut calls = Vec::new();
    let result = rt
        .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |input| {
            calls.push(failover_ids(&input));
            // Even a foreign/late result for the other tier cannot participate in this tier's judge.
            std::future::ready(failover_batch(&[
                ("node-b", Measured(80)),
                ("node-c", Measured(1)),
            ]))
        })
        .await;
    assert_eq!(failover_best(result), ("node-b".to_string(), 80));
    assert_eq!(calls, [vec!["node-b"]]);
    assert_eq!(rt.config.current().unwrap(), cfg);
    let slot = rt.auto_select.lock().unwrap();
    assert_eq!((slot.failover_switches, slot.select_switches), (1, 0));
    assert!(slot.memory.is_barred("node-c", MONO));
}

#[tokio::test]
async fn failover_fallback_rescues_only_after_an_explicit_all_failed_first_tier() {
    use crate::runtime::auto_switch::CandidateProbe::{Failed, Measured};
    let (rt, _dir, _cfg, candidates, barred) = failover_fixture();
    let mut calls = Vec::new();
    let result = rt
        .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |input| {
            calls.push(failover_ids(&input));
            let entries = if calls.len() == 1 {
                vec![("node-b", Failed), ("node-c", Measured(1))]
            } else {
                vec![("node-c", Measured(20))]
            };
            std::future::ready(failover_batch(&entries))
        })
        .await;
    assert_eq!(failover_best(result), ("node-c".to_string(), 20));
    assert_eq!(calls, [vec!["node-b"], vec!["node-c"]]);
}

#[tokio::test]
async fn failover_fallback_probes_barred_tier_directly_when_no_unbarred_candidate_is_eligible() {
    use crate::runtime::auto_switch::CandidateProbe::Measured;
    let (rt, _dir, _cfg, mut candidates, barred) = failover_fixture();
    candidates.retain(|candidate| candidate.id == "node-c");
    let mut calls = Vec::new();
    let result = rt
        .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |input| {
            calls.push(failover_ids(&input));
            std::future::ready(failover_batch(&[("node-c", Measured(20))]))
        })
        .await;
    assert_eq!(failover_best(result), ("node-c".to_string(), 20));
    assert_eq!(calls, [vec!["node-c"]]);
}

#[tokio::test]
async fn failover_fallback_does_not_rescue_after_mismatch_or_mix_tier_results() {
    use crate::runtime::auto_switch::{
        CandidateProbe::{Measured, MismatchSkipped},
        ProbeVerdict,
    };
    let (rt, _dir, _cfg, candidates, barred) = failover_fixture();
    let mut calls = Vec::new();
    let result = rt
        .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |input| {
            calls.push(failover_ids(&input));
            std::future::ready(failover_batch(&[
                ("node-b", MismatchSkipped),
                ("node-c", Measured(1)),
            ]))
        })
        .await;
    assert_eq!(
        result,
        Some(ProbeVerdict::Inconclusive {
            mismatch_skipped: 1
        })
    );
    assert_eq!(calls, [vec!["node-b"]]);
}

#[tokio::test]
async fn failover_fallback_busy_or_interrupted_in_either_tier_never_means_all_failed() {
    use crate::commands::speedtest::RuntimeProbeBatch;
    use crate::runtime::auto_switch::CandidateProbe::Failed;
    for second_tier in [false, true] {
        for interrupted in [false, true] {
            let (rt, _dir, _cfg, candidates, barred) = failover_fixture();
            let mut calls = Vec::new();
            let result = rt
                .probe_failover_candidates_from(
                    rt.core_generation(),
                    &candidates,
                    &barred,
                    |input| {
                        calls.push(failover_ids(&input));
                        std::future::ready(if second_tier && calls.len() == 1 {
                            failover_batch(&[("node-b", Failed)])
                        } else if interrupted {
                            RuntimeProbeBatch::Interrupted
                        } else {
                            RuntimeProbeBatch::Busy
                        })
                    },
                )
                .await;
            assert_eq!(result, None);
            let expected = if second_tier {
                vec![vec!["node-b"], vec!["node-c"]]
            } else {
                vec![vec!["node-b"]]
            };
            assert_eq!(calls, expected);
        }
    }
}

#[tokio::test]
async fn failover_fallback_generation_change_or_stop_prevents_initial_or_rescue_probe() {
    use crate::runtime::auto_switch::CandidateProbe::Failed;
    for before_probe in [false, true] {
        for stopped in [false, true] {
            let (rt, _dir, _cfg, candidates, barred) = failover_fixture();
            let generation = rt.core_generation();
            let invalidate = || {
                if stopped {
                    rt.status.write().unwrap().running = false;
                } else {
                    rt.gate.bump_generation();
                }
            };
            if before_probe {
                invalidate();
            }
            let mut calls = Vec::new();
            let result = rt
                .probe_failover_candidates_from(generation, &candidates, &barred, |input| {
                    calls.push(failover_ids(&input));
                    invalidate();
                    std::future::ready(failover_batch(&[("node-b", Failed)]))
                })
                .await;
            assert_eq!(result, None);
            let expected = if before_probe {
                vec![]
            } else {
                vec![vec!["node-b"]]
            };
            assert_eq!(calls, expected);
        }
    }
}

#[tokio::test]
async fn failover_fallback_discards_a_measured_rescue_when_its_generation_has_changed() {
    use crate::runtime::auto_switch::CandidateProbe::{Failed, Measured};
    let (rt, _dir, _cfg, candidates, barred) = failover_fixture();
    let generation = rt.core_generation();
    let mut calls = Vec::new();
    let result = rt
        .probe_failover_candidates_from(generation, &candidates, &barred, |input| {
            calls.push(failover_ids(&input));
            let batch = if calls.len() == 1 {
                failover_batch(&[("node-b", Failed)])
            } else {
                rt.gate.bump_generation();
                failover_batch(&[("node-c", Measured(20))])
            };
            std::future::ready(batch)
        })
        .await;
    assert_eq!(result, None);
    assert_eq!(calls, [vec!["node-b"], vec!["node-c"]]);
}

#[tokio::test]
async fn failover_fallback_barred_tier_reports_its_own_all_failed_or_inconclusive_result() {
    use crate::runtime::auto_switch::{
        CandidateProbe::{Failed, MismatchSkipped},
        ProbeVerdict,
    };
    for mismatch in [false, true] {
        let (rt, _dir, _cfg, candidates, barred) = failover_fixture();
        let mut calls = Vec::new();
        let result = rt
            .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |input| {
                calls.push(failover_ids(&input));
                let entries = if calls.len() == 1 {
                    vec![("node-b", Failed)]
                } else {
                    vec![("node-c", if mismatch { MismatchSkipped } else { Failed })]
                };
                std::future::ready(failover_batch(&entries))
            })
            .await;
        let expected = if mismatch {
            ProbeVerdict::Inconclusive {
                mismatch_skipped: 1,
            }
        } else {
            ProbeVerdict::AllFailed
        };
        assert_eq!(result, Some(expected));
        assert_eq!(calls, [vec!["node-b"], vec!["node-c"]]);
    }
}

#[tokio::test]
async fn failover_fallback_preserves_same_tier_latency_and_configuration_tie_order() {
    use crate::runtime::auto_switch::{CandidateProbe::Measured, RuntimeCandidate};
    for all_barred in [false, true] {
        let (rt, _dir, _cfg, mut candidates, _) = failover_fixture();
        candidates.push(RuntimeCandidate {
            id: "node-d".to_string(),
            name: "Node D".to_string(),
            tag: "Node D".to_string(),
        });
        let barred = if all_barred {
            candidates.iter().map(|c| c.id.clone()).collect()
        } else {
            BTreeSet::new()
        };
        let mut calls = Vec::new();
        let result = rt
            .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |input| {
                calls.push(failover_ids(&input));
                std::future::ready(failover_batch(&[
                    ("node-b", Measured(80)),
                    ("node-c", Measured(20)),
                    ("node-d", Measured(20)),
                ]))
            })
            .await;
        assert_eq!(failover_best(result), ("node-c".to_string(), 20));
        assert_eq!(calls, [vec!["node-b", "node-c", "node-d"]]);
    }
}

#[tokio::test]
async fn failover_fallback_rescue_never_reintroduces_scoped_or_runtime_ineligible_nodes() {
    use crate::runtime::auto_switch::{plan_runtime_candidates, CandidateProbe::Measured};
    for invalid in [
        "deleted",
        "other-subscription",
        "not-loaded",
        "dirty",
        "staged",
        "not-ready",
    ] {
        let (rt, _dir, mut cfg, _candidates, barred) = failover_fixture();
        let baseline = cfg.clone();
        let mut snapshot = rt.switch_snapshot.read().unwrap().clone().unwrap();
        let mut staged = BTreeSet::new();
        let mut not_ready = BTreeSet::new();
        match invalid {
            "deleted" => cfg["servers"]
                .as_array_mut()
                .unwrap()
                .retain(|n| n["id"] != "node-b"),
            "other-subscription" => {
                cfg["servers"][1]["subscriptionId"] = serde_json::json!("other")
            }
            "not-loaded" => {
                snapshot.id_to_tag.remove("node-b");
            }
            "dirty" => cfg["servers"][1]["port"] = serde_json::json!(19001),
            "staged" => {
                staged.insert("node-b".to_string());
            }
            "not-ready" => {
                not_ready.insert("node-b".to_string());
            }
            _ => unreachable!(),
        }
        let scope = crate::runtime::auto_select::subscription_members(&cfg, "sub")
            .into_iter()
            .collect();
        let mut plan = plan_runtime_candidates(
            &cfg,
            Some("node-a"),
            &snapshot.id_to_tag,
            &snapshot.measurement_fingerprints,
            &crate::runtime::node_fingerprints::measurement_table_json(&cfg),
            &staged,
            &not_ready,
            Some(&scope),
        );
        assert!(rt.retain_hot_switchable_candidates(rt.core_generation(), &baseline, &mut plan));
        let mut calls = Vec::new();
        let result = rt
            .probe_failover_candidates_from(
                rt.core_generation(),
                &plan.candidates,
                &barred,
                |input| {
                    calls.push(failover_ids(&input));
                    std::future::ready(failover_batch(&[
                        ("node-c", Measured(20)),
                        ("node-b", Measured(1)),
                        ("node-a", Measured(0)),
                    ]))
                },
            )
            .await;
        assert_eq!(
            failover_best(result),
            ("node-c".to_string(), 20),
            "{invalid}"
        );
        assert_eq!(calls, [vec!["node-c"]], "{invalid}");
    }
}

#[tokio::test]
async fn failover_fallback_a_late_batch_cannot_beat_a_new_generation_retry() {
    use crate::runtime::auto_switch::CandidateProbe::Measured;
    let (rt, _dir, cfg, candidates, barred) = failover_fixture();
    let generation = rt.core_generation();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel();
    let old_rt = Arc::clone(&rt);
    let old_candidates = candidates.clone();
    let old_barred = barred.clone();
    let old = tokio::spawn(async move {
        let mut started = Some(started_tx);
        let mut result = Some(result_rx);
        old_rt
            .probe_failover_candidates_from(generation, &old_candidates, &old_barred, |input| {
                started.take().unwrap().send(failover_ids(&input)).unwrap();
                let result = result.take().unwrap();
                async move { result.await.unwrap() }
            })
            .await
    });
    let started = started_rx.await.unwrap();
    rt.gate.bump_generation();
    let new_result = rt
        .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |_| {
            std::future::ready(failover_batch(&[
                ("node-b", Measured(80)),
                ("node-c", Measured(1)),
            ]))
        })
        .await;
    result_tx
        .send(failover_batch(&[
            ("node-b", Measured(1)),
            ("node-c", Measured(0)),
        ]))
        .unwrap();
    assert_eq!(old.await.unwrap(), None);
    assert_eq!(failover_best(new_result), ("node-b".to_string(), 80));
    assert_eq!(started, ["node-b"]);
    assert_eq!(rt.config.current().unwrap(), cfg);
}

#[test]
fn failover_fallback_production_uses_layered_coordinator_after_candidate_qualification() {
    let body = method_body(&module_code("runtime/proxy"),
        "    async fn do_switch_io(self: &Arc<Self>, machine: &mut AutoSwitchMachine, reason: &str) -> bool {");
    let qualify = body.find("self.retain_hot_switchable_candidates(").unwrap();
    let bars = body.find("self.auto_select_failover_barred_from(").unwrap();
    let layered = body.find(".probe_failover_candidates_from(").unwrap();
    let probe = body.find("probe_runtime_candidates(").unwrap();
    let commit = body.find(".auto_hot_switch_transaction(").unwrap();
    assert!(qualify < bars && bars < layered && layered < probe && probe < commit);
    assert!(body.contains("&candidate_plan.candidates,"));
    assert!(body.contains("probe_runtime_candidates(self, targets, &probe_input, url)"));
    assert!(
        !body.contains("judge_probes("),
        "the caller must not re-judge mixed tiers"
    );
}

#[tokio::test]
async fn failover_fallback_empty_candidates_or_missing_failure_receipts_do_not_prove_all_failed() {
    use crate::runtime::auto_switch::CandidateProbe::Failed;
    for invalid in ["empty", "missing-first", "missing-rescue"] {
        let (rt, _dir, _cfg, mut candidates, barred) = failover_fixture();
        if invalid == "empty" {
            candidates.clear();
        }
        let mut calls = Vec::new();
        let result = rt
            .probe_failover_candidates_from(rt.core_generation(), &candidates, &barred, |input| {
                calls.push(failover_ids(&input));
                std::future::ready(if invalid == "missing-rescue" && calls.len() == 1 {
                    failover_batch(&[("node-b", Failed)])
                } else {
                    failover_batch(&[])
                })
            })
            .await;
        assert_eq!(result, None, "{invalid}");
        let expected = match invalid {
            "empty" => vec![],
            "missing-first" => vec![vec!["node-b"]],
            "missing-rescue" => vec![vec!["node-b"], vec!["node-c"]],
            _ => unreachable!(),
        };
        assert_eq!(calls, expected, "{invalid}");
    }
}

fn native_fixture(
    default: Option<AutoSelectTarget>,
    rule_scope: bool,
) -> (Arc<ProxyRuntime>, TestDir, Arc<TestPutSink>, Value) {
    let (rt, dir) = test_runtime();
    let mut config = auto_config("node-a", None);
    config["autoSelectEnabled"] = serde_json::json!(true);
    if let Some(target) = default {
        config["selectionIntent"] = polaris_store::auto_select::intent(&target);
    }
    if rule_scope {
        config["appRules"] = serde_json::json!([{"appId":"independent","enabled":true,"action":"proxy","targetAutoSelect":{"scope":"subscription","subscriptionId":"sub"}}]);
    }
    let config = running(&rt, &config);
    let typed: UserConfig = serde_json::from_value(config.clone()).unwrap();
    let nodes = rt
        .switch_snapshot
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .id_to_tag
        .clone();
    let now = (crate::runtime::subscription_scheduler::now_ms() / 1000) as i64;
    let item = |tag: String, time: i64| GroupItem {
        tag,
        url_test_time: time,
        url_test_delay: 20,
        ..Default::default()
    };
    let mut groups = Vec::new();
    let mut proxy_members: Vec<GroupItem> =
        nodes.values().map(|tag| item(tag.clone(), 0)).collect();
    for target in native::referenced_targets(&typed) {
        let wrapper = native::wrapper_tag(&target);
        let test = native::urltest_tag(&target);
        let members = native::eligible_members(&typed, &target);
        let selected = if target == AutoSelectTarget::Global {
            "node-a"
        } else {
            "node-c"
        };
        let mut wrapper_members = vec![item(test.clone(), 0)];
        wrapper_members.extend(members.iter().map(|id| item(nodes[id].clone(), 0)));
        groups.push(Group {
            tag: wrapper.clone(),
            selected: test.clone(),
            items: wrapper_members,
            ..Default::default()
        });
        groups.push(Group {
            tag: test,
            selected: nodes[selected].clone(),
            items: members
                .iter()
                .map(|id| item(nodes[id].clone(), now))
                .collect(),
            ..Default::default()
        });
        proxy_members.push(item(wrapper, 0));
    }
    let selected = polaris_store::auto_select::target(&config)
        .map(|target| native::wrapper_tag(&target))
        .unwrap_or_else(|| nodes["node-a"].clone());
    groups.push(Group {
        tag: PROXY_SELECTOR_TAG.into(),
        selected,
        items: proxy_members,
        ..Default::default()
    });
    let sink = Arc::new(TestPutSink::default());
    sink.follow_puts.store(true, Ordering::SeqCst);
    *sink.groups.lock().unwrap() = Some(
        groups
            .iter()
            .map(|group| GroupSelection {
                tag: group.tag.clone(),
                selected: group.selected.clone(),
            })
            .collect(),
    );
    *sink.native_groups.lock().unwrap() = Some(groups);
    *rt.management_api_stub.lock().unwrap() = Some(Arc::clone(&sink));
    (rt, dir, sink, config)
}

fn subscription_target() -> AutoSelectTarget {
    AutoSelectTarget::Subscription {
        subscription_id: "sub".into(),
    }
}

#[tokio::test]
async fn native_groups_own_production_and_legacy_writer_cannot_override_a_reference() {
    let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
    assert_eq!(
        rt.auto_select_closed(),
        Some(crate::runtime::auto_select::Gate::SwitchedOff)
    );
    assert!(!rt.auto_select_active());
    let status = rt.native_auto_status().await;
    assert_eq!(status.native_auto_groups.len(), 2);
    assert!(status
        .native_auto_groups
        .iter()
        .all(|group| group.confirmed));
    assert_eq!(status.actual_server_id.as_deref(), Some("node-a"));
    assert_eq!(rt.config.current().unwrap(), config);
    assert!(sink.calls().is_empty());
    let source = module_code("runtime/proxy");
    let body=method_body(&source,"    async fn do_switch_io(self: &Arc<Self>, machine: &mut AutoSwitchMachine, reason: &str) -> bool {");
    assert!(
        body.find("polaris_store::auto_select::target").unwrap()
            < body.find("plan_runtime_candidates(").unwrap()
    );
}

#[tokio::test]
async fn native_status_observes_runtime_and_rejects_stop_or_old_generation() {
    let (rt, _dir, _sink, mut desired) = native_fixture(Some(AutoSelectTarget::Global), true);
    desired["selectedServerId"] = serde_json::json!("node-c");
    rt.config.save_full(&desired).unwrap();
    let status = rt.native_auto_status().await;
    assert_eq!(status.actual_server_id.as_deref(), Some("node-a"));
    assert_eq!(rt.status().actual_server_id.as_deref(), Some("node-a"));
    rt.gate.bump_generation();
    assert_eq!(rt.native_auto_cached_status().actual_server_id, None);
    rt.status.write().unwrap().running = false;
    assert_eq!(rt.status().actual_server_id, None);
    assert!(rt
        .native_auto_cached_status()
        .native_auto_groups
        .iter()
        .all(|group| !group.confirmed));
}

#[tokio::test]
async fn native_default_same_reference_is_noop_and_independent_rule_group_is_preserved() {
    let (rt, _dir, sink, mut desired) = native_fixture(Some(AutoSelectTarget::Global), true);
    desired["selectedServerId"] = serde_json::json!("node-c");
    rt.config.save_full(&desired).unwrap();
    let status = rt
        .native_auto_select_default(AutoSelectTarget::Global)
        .await
        .unwrap();
    assert_eq!(status.actual_server_id.as_deref(), Some("node-a"));
    assert_eq!(rt.config.current().unwrap()["selectedServerId"], "node-a");
    assert!(sink.calls().is_empty());
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    let status = rt
        .native_auto_select_default(subscription_target())
        .await
        .unwrap();
    assert_eq!(status.actual_server_id.as_deref(), Some("node-c"));
    assert_eq!(
        rt.config.current().unwrap()["appRules"],
        desired["appRules"]
    );
    assert!(rt.pending_force_restart.read().unwrap().is_none());
}

#[tokio::test]
async fn native_manual_pin_same_leaf_clears_only_default_without_restart() {
    for target in ["node-a", "node-b"] {
        let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
        let (_, _, intent, cleared) = server_switch_core(
            &rt.config,
            target,
            |_| Ok(()),
            || rt.register_selector_intent(),
        )
        .unwrap();
        assert!(cleared);
        let result = rt
            .switch_selected_server_if_current(target, intent)
            .await
            .unwrap();
        assert_eq!(result, Some(SwitchOutcome::HotSwitched));
        assert!(rt
            .config
            .current()
            .unwrap()
            .get("selectionIntent")
            .is_none());
        assert_eq!(rt.config.current().unwrap()["appRules"], config["appRules"]);
        assert_eq!(
            rt.native_auto_status().await.actual_server_id.as_deref(),
            Some(target)
        );
        assert_eq!(sink.calls().len(), 1);
        assert!(rt.pending_force_restart.read().unwrap().is_none());
    }
}

fn native_connection(id: &str, chains: &[&str]) -> ConnectionSnapshot {
    ConnectionSnapshot {
        id: id.into(),
        chains: chains.iter().map(|tag| (*tag).into()).collect(),
        closed_at: 0,
    }
}

fn set_native_selected(sink: &TestPutSink, tag: &str, member: &str) {
    for group in sink.native_groups.lock().unwrap().as_mut().unwrap() {
        if group.tag == tag {
            group.selected = member.into();
        }
    }
    for group in sink.groups.lock().unwrap().as_mut().unwrap() {
        if group.tag == tag {
            group.selected = member.into();
        }
    }
}

fn seed_logical_exit_connections(sink: &TestPutSink, old_chain: &[&str]) {
    let mut followed = old_chain.to_vec();
    followed.push("rule-sel-follow-default");
    let wrapper = native::wrapper_tag(&AutoSelectTarget::Global);
    let test = native::urltest_tag(&AutoSelectTarget::Global);
    let mut closed = native_connection("already-closed", old_chain);
    closed.closed_at = 1;
    *sink.connections.lock().unwrap() = vec![
        native_connection("old-default", old_chain),
        native_connection("old-follow-default", &followed),
        native_connection("fixed-same-node", &["Node A", "rule-sel-fixed"]),
        native_connection(
            "independent-same-auto",
            &["Node A", &test, &wrapper, "rule-sel-independent-auto"],
        ),
        native_connection("direct", &["direct"]),
        native_connection("different-member", &["Node C", PROXY_SELECTOR_TAG]),
        native_connection("unknown-chain", &[]),
        closed,
    ];
}

fn assert_logical_exit_closures(sink: &TestPutSink, interrupt: bool) {
    let mut actual = sink.closed_ids.lock().unwrap().clone();
    actual.sort();
    let expected: Vec<String> = if interrupt {
        vec!["old-default".into(), "old-follow-default".into()]
    } else {
        Vec::new()
    };
    assert_eq!(actual, expected);
    assert_eq!(
        sink.connection_snapshot_calls.load(Ordering::SeqCst),
        u32::from(interrupt),
    );
}

#[tokio::test]
async fn native_auto_internal_winner_changes_preserve_connections_with_either_preference() {
    for interrupt in [false, true] {
        let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
        config["interruptConnectionsOnSwitch"] = serde_json::json!(interrupt);
        let config = running(&rt, &config);
        let wrapper = native::wrapper_tag(&AutoSelectTarget::Global);
        let test = native::urltest_tag(&AutoSelectTarget::Global);
        seed_logical_exit_connections(&sink, &["Node A", &test, &wrapper, PROXY_SELECTOR_TAG]);
        assert_eq!(
            rt.native_auto_status().await.actual_server_id.as_deref(),
            Some("node-a"),
        );
        // The native kernel changes a URLTest winner; the App observes it only.
        set_native_selected(&sink, &test, "Node B");
        assert_eq!(
            rt.native_auto_status().await.actual_server_id.as_deref(),
            Some("node-b"),
        );
        assert_eq!(rt.config.current().unwrap(), config);
        assert!(sink.calls().is_empty());
        assert_logical_exit_closures(&sink, false);
    }
}

#[tokio::test]
async fn native_auto_to_node_obeys_preference_even_for_same_physical_leaf() {
    for interrupt in [false, true] {
        for target in ["node-a", "node-b"] {
            let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
            config["interruptConnectionsOnSwitch"] = serde_json::json!(interrupt);
            let config = running(&rt, &config);
            let wrapper = native::wrapper_tag(&AutoSelectTarget::Global);
            let test = native::urltest_tag(&AutoSelectTarget::Global);
            seed_logical_exit_connections(&sink, &["Node A", &test, &wrapper, PROXY_SELECTOR_TAG]);
            let at_put = Arc::clone(&sink);
            *sink.on_put.lock().unwrap() = Some(Box::new(move || {
                assert_eq!(
                    at_put.connection_snapshot_calls.load(Ordering::SeqCst),
                    u32::from(interrupt),
                    "old IDs must be captured before the first selector PUT",
                );
                // A late snapshot must not acquire IDs created at/after PUT.
                at_put.connections.lock().unwrap().push(native_connection(
                    "after-snapshot",
                    &["Node A", &test, &wrapper, PROXY_SELECTOR_TAG],
                ));
            }));
            if interrupt {
                let owner = Arc::clone(&rt);
                *sink.on_close.lock().unwrap() = Some(Box::new(move || {
                    let runtime = owner.current_config_snapshot().unwrap();
                    assert!(polaris_store::auto_select::target(&runtime).is_none());
                    assert_eq!(runtime["selectedServerId"], target);
                    assert_eq!(owner.native_auto_cached_status().actual_server_id.as_deref(), Some(target));
                }));
            }
            let (_, _, intent, cleared) = server_switch_core(
                &rt.config,
                target,
                |_| Ok(()),
                || rt.register_selector_intent(),
            )
            .unwrap();
            assert!(cleared);
            assert_eq!(
                rt.switch_selected_server_if_current(target, intent).await.unwrap(),
                Some(SwitchOutcome::HotSwitched),
            );
            assert_logical_exit_closures(&sink, interrupt);
            assert_eq!(sink.calls().len(), 1);
            assert_eq!(rt.config.current().unwrap()["appRules"], config["appRules"]);
            assert_eq!(
                rt.native_auto_status().await.actual_server_id.as_deref(),
                Some(target),
            );
            assert!(rt.pending_force_restart.read().unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn native_node_to_auto_obeys_preference_and_excludes_new_same_leaf_connections() {
    for interrupt in [false, true] {
        for winner in ["Node A", "Node B"] {
            let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
            // Keep the already-emitted Auto groups but begin with a concrete default.
            config.as_object_mut().unwrap().remove("selectionIntent");
            config["interruptConnectionsOnSwitch"] = serde_json::json!(interrupt);
            let config = running(&rt, &config);
            set_native_selected(&sink, PROXY_SELECTOR_TAG, "Node A");
            let wrapper = native::wrapper_tag(&AutoSelectTarget::Global);
            let test = native::urltest_tag(&AutoSelectTarget::Global);
            set_native_selected(&sink, &test, winner);
            seed_logical_exit_connections(&sink, &["Node A", PROXY_SELECTOR_TAG]);
            let at_put = Arc::clone(&sink);
            *sink.on_put.lock().unwrap() = Some(Box::new(move || {
                assert_eq!(
                    at_put.connection_snapshot_calls.load(Ordering::SeqCst),
                    u32::from(interrupt),
                    "old IDs must be captured before the first selector PUT",
                );
                // Auto's new chain can still contain the old physical node.
                // Pair filtering alone would close this ID if read after PUT.
                at_put.connections.lock().unwrap().push(native_connection(
                    "new-auto-same-leaf",
                    &["Node A", &test, &wrapper, PROXY_SELECTOR_TAG],
                ));
            }));
            if interrupt {
                let owner = Arc::clone(&rt);
                *sink.on_close.lock().unwrap() = Some(Box::new(move || {
                    assert_eq!(
                        polaris_store::auto_select::target(&owner.current_config_snapshot().unwrap()),
                        Some(AutoSelectTarget::Global),
                    );
                    assert_eq!(
                        owner.native_auto_cached_status().actual_server_id.as_deref(),
                        Some(if winner == "Node A" { "node-a" } else { "node-b" }),
                    );
                }));
            }
            let status = rt.native_auto_select_default(AutoSelectTarget::Global).await.unwrap();
            assert_eq!(
                status.actual_server_id.as_deref(),
                Some(if winner == "Node A" { "node-a" } else { "node-b" }),
            );
            assert_logical_exit_closures(&sink, interrupt);
            assert_eq!(sink.calls().len(), 2);
            assert_eq!(rt.config.current().unwrap()["appRules"], config["appRules"]);
            assert!(rt.pending_force_restart.read().unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn native_node_to_auto_rejects_partial_put_or_stale_intent_without_closing_connections() {
    for race in ["put", "intent", "stop", "readback", "readback-intent", "readback-stop", "readback-publication"] {
        let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
        config.as_object_mut().unwrap().remove("selectionIntent");
        config["interruptConnectionsOnSwitch"] = serde_json::json!(true);
        let config = running(&rt, &config);
        set_native_selected(&sink, PROXY_SELECTOR_TAG, "Node A");
        seed_logical_exit_connections(&sink, &["Node A", PROXY_SELECTOR_TAG]);
        if race == "put" {
            sink.fail_at.lock().unwrap().insert(2);
        } else if race == "readback" {
            sink.follow_puts.store(false, Ordering::SeqCst);
        } else if race.starts_with("readback-") {
            let owner = Arc::clone(&rt);
            let at_put = Arc::clone(&sink);
            *sink.on_put.lock().unwrap() = Some(Box::new(move || {
                // The next group read is after both selector PUT acknowledgements.
                let at_read = Arc::clone(&at_put);
                *at_put.on_groups.lock().unwrap() = Some(Box::new(move || {
                    if race == "readback-publication" {
                        // A newer observation wins before this RPC response publishes.
                        let groups = at_read.native_groups.lock().unwrap().clone().unwrap();
                        assert!(owner.publish_native_groups(
                            owner.core_generation(), owner.native_observation_revision(), groups,
                        ).is_some());
                    } else if race == "readback-stop" {
                        owner.gate.bump_generation();
                        owner.status.write().unwrap().running = false;
                    } else {
                        owner.register_selector_intent();
                    }
                }));
            }));
        } else {
            let owner = Arc::clone(&rt);
            *sink.on_put.lock().unwrap() = Some(Box::new(move || {
                if race == "stop" {
                    owner.gate.bump_generation();
                    owner.status.write().unwrap().running = false;
                } else {
                    owner.register_selector_intent();
                }
            }));
        }
        assert!(rt.native_auto_select_default(AutoSelectTarget::Global).await.is_err());
        assert!(sink.closed_ids.lock().unwrap().is_empty());
        assert_eq!(sink.connection_snapshot_calls.load(Ordering::SeqCst), 1);
        assert_eq!(rt.current_config_snapshot().unwrap(), config);
        assert_eq!(
            rt.config.current().unwrap()["selectionIntent"],
            polaris_store::auto_select::intent(&AutoSelectTarget::Global),
        );
        assert!(rt.pending_force_restart.read().unwrap().is_none());
        if race == "stop" || race == "intent" {
            assert_eq!(sink.calls().len(), 1, "a superseded rollback cannot PUT");
        } else if race == "readback-stop" || race == "readback-intent" {
            assert_eq!(sink.calls().len(), 2);
        } else {
            assert_eq!(
                rt.native_auto_status().await.actual_server_id.as_deref(),
                Some("node-a"),
            );
        }
    }
}

#[tokio::test]
async fn native_logical_disconnect_keeps_snapshot_error_and_guards_late_close() {
    use polaris_config_engine::builder::hotswitch::{HotSwitchKind, HotSwitchPlan, HotSwitchPut};
    for failure in ["snapshot", "intent", "stop"] {
        let (rt, _dir, sink, _config) = native_fixture(Some(AutoSelectTarget::Global), true);
        let wrapper = native::wrapper_tag(&AutoSelectTarget::Global);
        seed_logical_exit_connections(&sink, &["Node A", &wrapper, PROXY_SELECTOR_TAG]);
        if failure == "snapshot" {
            *sink.connection_snapshot_error.lock().unwrap() =
                Some(polaris_switch_engine::ManagementError::SnapshotTimeout);
        }
        let plan = HotSwitchPlan {
            kind: HotSwitchKind::Global,
            must_restart: false,
            puts: vec![HotSwitchPut {
                selector_tag: PROXY_SELECTOR_TAG.into(),
                member_tag: "Node A".into(),
                old_member_tag: Some(wrapper),
            }],
        };
        let generation = rt.core_generation();
        let intent = rt.register_selector_intent();
        let (outcome, captured) = rt
            .execute_hot_switch_plan_with_pre_switch_snapshot(&plan, true, generation, intent)
            .await;
        assert!(matches!(outcome, polaris_switch_engine::HotSwitchOutcome::Applied { .. }));
        assert!(sink.closed_ids.lock().unwrap().is_empty(), "PUT ACK alone never closes");
        if failure == "intent" {
            rt.register_selector_intent();
        } else if failure == "stop" {
            rt.gate.bump_generation();
            rt.status.write().unwrap().running = false;
        }
        let receipt = rt.finish_logical_exit_disconnect(captured, generation, intent).await.unwrap();
        assert!(receipt.closed_ids.is_empty());
        assert!(sink.closed_ids.lock().unwrap().is_empty());
        assert_eq!(sink.connection_snapshot_calls.load(Ordering::SeqCst), 1);
        if failure == "snapshot" {
            assert!(receipt.snapshot_error.is_some());
            assert!(receipt.selected_ids.is_empty());
        } else {
            assert_eq!(receipt.selected_ids.len(), 2);
            assert_eq!(receipt.failed_ids.len(), 2);
        }
    }
}

#[tokio::test]
async fn native_off_requires_every_scope_before_first_write_and_never_guesses_desired() {
    for missing in ["history", "group", "outside"] {
        let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
        let sub_test = native::urltest_tag(&subscription_target());
        {
            let mut groups = sink.native_groups.lock().unwrap();
            let groups = groups.as_mut().unwrap();
            match missing {
                "history" => {
                    for item in groups.iter_mut().flat_map(|group| &mut group.items) {
                        if item.tag == "Node C" {
                            item.url_test_time = 0;
                        }
                    }
                }
                "group" => groups.retain(|group| group.tag != sub_test),
                "outside" => {
                    groups
                        .iter_mut()
                        .find(|group| group.tag == sub_test)
                        .unwrap()
                        .selected = "Node A".into()
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(
            rt.native_auto_set_enabled(false).await.unwrap_err(),
            "AUTO_SELECT_NO_ACTUAL_EXIT",
            "{missing}"
        );
        assert!(sink.calls().is_empty());
        assert_eq!(rt.config.current().unwrap(), config);
    }
}

#[tokio::test]
async fn native_off_partial_failure_rolls_back_and_does_not_commit_flag_or_pins() {
    for failure in ["second-put", "default-put", "readback", "persist", "publication"] {
        let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
        match failure {
            "second-put" => {
                sink.fail_at.lock().unwrap().insert(2);
            }
            "default-put" => { sink.fail_at.lock().unwrap().insert(3); }
            "readback" => sink.follow_puts.store(false, Ordering::SeqCst),
            "persist" => rt.config.fail_next_config_persist_for_test(),
            "publication" => {
                let owner = Arc::clone(&rt);
                let at_put = Arc::clone(&sink);
                *sink.on_put.lock().unwrap() = Some(Box::new(move || {
                    let at_read = Arc::clone(&at_put);
                    *at_put.on_groups.lock().unwrap() = Some(Box::new(move || {
                        let groups = at_read.native_groups.lock().unwrap().clone().unwrap();
                        assert!(owner.publish_native_groups(
                            owner.core_generation(), owner.native_observation_revision(), groups,
                        ).is_some());
                    }));
                }));
            }
            _ => unreachable!(),
        }
        assert_eq!(
            rt.native_auto_set_enabled(false).await.unwrap_err(),
            "AUTO_SELECT_SWITCH_FAILED",
            "{failure}"
        );
        assert_eq!(rt.config.current().unwrap(), config);
        let groups = sink.native_groups.lock().unwrap();
        for target in [AutoSelectTarget::Global, subscription_target()] {
            assert_eq!(
                groups
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|group| group.tag == native::wrapper_tag(&target))
                    .unwrap()
                    .selected,
                native::urltest_tag(&target)
            );
        }
        assert!(rt.selector_reconcile.is_required());
        assert!(rt.pending_force_restart.read().unwrap().is_none());
    }
}

#[tokio::test]
async fn native_off_stop_or_new_intent_at_put_cannot_commit_an_obsolete_receipt() {
    for stop in [false, true] {
        let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
        let owner = Arc::clone(&rt);
        *sink.on_put.lock().unwrap() = Some(Box::new(move || {
            if stop {
                owner.gate.bump_generation();
                owner.status.write().unwrap().running = false;
            } else {
                owner.register_selector_intent();
            }
        }));
        assert!(rt.native_auto_set_enabled(false).await.is_err());
        assert_eq!(rt.config.current().unwrap(), config);
        assert!(rt.pending_force_restart.read().unwrap().is_none());
        assert_eq!(
            sink.calls().len(), 1,
            "rollback cannot write after a replaced core or a newer intent",
        );
    }
}

#[tokio::test]
async fn native_off_pins_each_scope_and_makes_default_concrete_without_disconnect() {
    for interrupt in [false, true] {
        for default in [AutoSelectTarget::Global, subscription_target()] {
            let (rt, _dir, sink, mut config) = native_fixture(Some(default.clone()), true);
            config["interruptConnectionsOnSwitch"] = serde_json::json!(interrupt);
            let config = running(&rt, &config);
            let actual = if default == AutoSelectTarget::Global { "node-a" } else { "node-c" };
            let tag = if actual == "node-a" { "Node A" } else { "Node C" };
            let wrapper = native::wrapper_tag(&default);
            seed_logical_exit_connections(&sink, &[tag, &wrapper, PROXY_SELECTOR_TAG]);
            let status = rt.native_auto_set_enabled(false).await.unwrap();
            assert_eq!(status.apply_outcome, Some("applied"));
            assert!(!status.feature_enabled);
            assert!(status.native_auto_groups.iter().all(|group| group.confirmed && group.state == "pinned"));
            assert_eq!(status.actual_server_id.as_deref(), Some(actual));
            let saved = rt.config.current().unwrap();
            assert!(saved.get("selectionIntent").is_none());
            assert_eq!(saved["selectedServerId"], actual);
            assert_eq!(saved["appRules"], config["appRules"]);
            let runtime = rt.current_config_snapshot().unwrap();
            assert!(runtime.get("selectionIntent").is_none());
            assert_eq!(runtime["selectedServerId"], actual);
            let typed: UserConfig = serde_json::from_value(saved).unwrap();
            assert_eq!(native::pin(&typed, &default), Some(actual));
            assert_eq!(native::pin(&typed, &subscription_target()), Some("node-c"));
            assert_eq!(sink.calls().last(), Some(&(PROXY_SELECTOR_TAG.into(), tag.into())));
            assert_logical_exit_closures(&sink, false);
            assert!(rt.pending_force_restart.read().unwrap().is_none());

            // The shared rule wrapper resumes Auto, but its winner cannot pull
            // the now-concrete default back into Auto, even when scope is shared.
            set_native_selected(&sink, &native::urltest_tag(&default), "Node B");
            let calls_before = sink.calls().len();
            let status = rt.native_auto_set_enabled(true).await.unwrap();
            assert!(status.feature_enabled);
            assert_eq!(status.actual_server_id.as_deref(), Some(actual));
            assert!(status.native_auto_groups.iter().all(|group| group.confirmed && group.state == "auto"));
            assert!(rt.config.current().unwrap().get("selectionIntent").is_none());
            assert!(sink.calls()[calls_before..].iter().all(|(tag, _)| tag != PROXY_SELECTOR_TAG));
            assert_logical_exit_closures(&sink, false);
            assert!(rt.pending_force_restart.read().unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn native_off_then_manual_selection_then_on_keeps_that_manual_default() {
    for interrupt in [false, true] {
        let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
        config["interruptConnectionsOnSwitch"] = serde_json::json!(interrupt);
        running(&rt, &config);
        rt.native_auto_set_enabled(false).await.unwrap();
        let (_, _, intent, _) = server_switch_core(
            &rt.config, "node-b", |_| Ok(()), || rt.register_selector_intent(),
        ).unwrap();
        rt.switch_selected_server_if_current("node-b", intent).await.unwrap();
        assert_eq!(rt.native_auto_status().await.actual_server_id.as_deref(), Some("node-b"));
        let calls_before = sink.calls().len();
        let snapshots_before = sink.connection_snapshot_calls.load(Ordering::SeqCst);
        let closed_before = sink.closed_ids.lock().unwrap().clone();
        let status = rt.native_auto_set_enabled(true).await.unwrap();
        assert_eq!(status.apply_outcome, Some("applied"));
        assert_eq!(status.actual_server_id.as_deref(), Some("node-b"));
        assert!(rt.config.current().unwrap().get("selectionIntent").is_none());
        assert_eq!(rt.current_config_snapshot().unwrap()["selectedServerId"], "node-b");
        assert!(sink.calls()[calls_before..].iter().all(|(tag, _)| tag != PROXY_SELECTOR_TAG));
        assert_eq!(sink.connection_snapshot_calls.load(Ordering::SeqCst), snapshots_before);
        assert_eq!(*sink.closed_ids.lock().unwrap(), closed_before);
        assert!(rt.pending_force_restart.read().unwrap().is_none());
    }
}

#[tokio::test]
async fn native_stopped_off_requires_verified_pins_but_empty_manual_mode_needs_no_node() {
    let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
    rt.status.write().unwrap().running = false;
    assert_eq!(
        rt.native_auto_set_enabled(false).await.unwrap_err(),
        "AUTO_SELECT_NO_ACTUAL_EXIT"
    );
    assert_eq!(rt.config.current().unwrap(), config);
    assert!(sink.calls().is_empty());
    rt.status.write().unwrap().running = true;
    rt.native_auto_set_enabled(false).await.unwrap();
    rt.native_auto_set_enabled(true).await.unwrap();
    rt.status.write().unwrap().running = false;
    assert!(
        !rt.native_auto_set_enabled(false)
            .await
            .unwrap()
            .feature_enabled
    );
    let (empty, _dir) = test_runtime();
    empty
        .config
        .save_full(&polaris_store::default_config())
        .unwrap();
    assert!(
        !empty
            .native_auto_set_enabled(false)
            .await
            .unwrap()
            .feature_enabled
    );
}

#[tokio::test]
async fn native_enable_without_references_keeps_manual_default_and_app_http_ownership() {
    let (rt, _dir, sink, mut config) = native_fixture(None, false);
    config["autoSelectEnabled"] = serde_json::json!(false);
    rt.config.save_full(&config).unwrap();
    mark_running_with_snapshot(&rt, &config);
    rt.native_auto_status().await;
    let status = rt.native_auto_set_enabled(true).await.unwrap();
    assert!(status.feature_enabled);
    assert!(status.native_auto_groups.is_empty());
    assert_eq!(status.actual_server_id.as_deref(), Some("node-a"));
    assert!(sink.calls().is_empty());
    assert!(!rt.native_owns_any(&["node-a".into(), "node-b".into()]));
    assert!(rt.pending_force_restart.read().unwrap().is_none());
}

#[tokio::test]
async fn native_dirty_or_empty_scope_is_refused_before_intent_or_runtime_changes() {
    for dirty in [false, true] {
        let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
        if dirty {
            config["servers"][1]["port"] = serde_json::json!(19002);
        }
        rt.config.save_full(&config).unwrap();
        let before = rt.config.current().unwrap();
        let target = if dirty {
            subscription_target()
        } else {
            AutoSelectTarget::Subscription {
                subscription_id: "absent".into(),
            }
        };
        assert_eq!(
            rt.native_auto_select_default(target).await.unwrap_err(),
            if dirty {
                "AUTO_SELECT_REQUIRES_APPLY"
            } else {
                "AUTO_SELECT_SCOPE_EMPTY"
            }
        );
        assert_eq!(rt.config.current().unwrap(), before);
        assert!(sink.calls().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn native_new_graph_defers_without_restart_and_preserves_unrelated_desired_edits() {
    let (rt, _dir, sink, mut desired) = native_fixture(None, true);
    let runtime = rt.current_config_snapshot();
    desired["mixedPort"] = serde_json::json!(7999);
    desired["selectedServerId"] = serde_json::json!("node-c");
    rt.config.save_full(&desired).unwrap();
    let status = rt.native_auto_select_default(AutoSelectTarget::Global).await.unwrap();
    assert_eq!(status.actual_server_id.as_deref(), Some("node-a"));
    assert!(status.native_auto_groups.iter()
        .find(|group| group.target == AutoSelectTarget::Global)
        .is_some_and(|group| !group.confirmed));
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    assert!(rt.restart_deferred.load(Ordering::SeqCst));
    assert_eq!(rt.current_config_snapshot(), runtime);
    assert_eq!(rt.current_config_snapshot().unwrap()["mixedPort"], 7890);
    assert_eq!(rt.config.current().unwrap()["mixedPort"], 7999);
    assert_eq!(rt.config.current().unwrap()["selectedServerId"], "node-a");
    assert!(sink.calls().is_empty());
}

#[tokio::test]
async fn native_obsolete_read_or_busy_lifecycle_cannot_save_a_new_reference() {
    for race in ["busy", "intent", "generation"] {
        let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
        if race == "busy" {
            rt.gate.begin();
        } else {
            let owner = Arc::clone(&rt);
            *sink.on_groups.lock().unwrap() = Some(Box::new(move || {
                if race == "generation" {
                    owner.gate.bump_generation();
                } else {
                    owner.register_selector_intent();
                }
            }));
        }
        assert!(
            rt.native_auto_select_default(subscription_target())
                .await
                .is_err(),
            "{race}"
        );
        assert_eq!(rt.config.current().unwrap(), config, "{race}");
        assert!(sink.calls().is_empty(), "{race}");
        assert!(rt.pending_force_restart.read().unwrap().is_none(), "{race}");
    }
}

#[tokio::test]
async fn native_default_partial_apply_or_intent_race_preserves_runtime_and_pending_reference() {
    for race in ["put", "intent", "stop"] {
        let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
        if race == "put" {
            sink.fail_at.lock().unwrap().insert(2);
        } else {
            let owner = Arc::clone(&rt);
            *sink.on_put.lock().unwrap() = Some(Box::new(move || {
                if race == "stop" {
                    owner.gate.bump_generation();
                    owner.status.write().unwrap().running = false;
                } else {
                    owner.register_selector_intent();
                }
            }));
        }
        assert!(
            rt.native_auto_select_default(subscription_target())
                .await
                .is_err(),
            "{race}"
        );
        assert_eq!(
            rt.current_config_snapshot().unwrap()["selectionIntent"],
            config["selectionIntent"],
            "R must not claim a rejected apply"
        );
        assert_eq!(
            rt.config.current().unwrap()["selectionIntent"],
            polaris_store::auto_select::intent(&subscription_target()),
            "the requested reference remains retryable"
        );
        assert!(rt.pending_force_restart.read().unwrap().is_none());
        if race == "stop" {
            assert_eq!(sink.calls().len(), 1);
        } else {
            assert_eq!(
                rt.native_auto_status().await.actual_server_id.as_deref(),
                Some("node-a")
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn native_mode_projection_preserves_confirmed_queue_and_unapplied_desired_fields() {
    let (rt, _dir, sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
    let mut desired = config.clone();
    desired["mixedPort"] = serde_json::json!(7999);
    rt.config.save_full(&desired).unwrap();
    let mut pending = config.clone();
    pending["selectionIntent"] = polaris_store::auto_select::intent(&subscription_target());
    let intent = rt.register_selector_intent();
    // Seed an already explicitly confirmed snapshot, rather than using an
    // unconfirmed projection helper as implicit restart authorization.
    *rt.pending_force_restart.write().unwrap() = Some((77, pending.clone(),
        ForceRestartSource::Confirmed { intent_generation: intent }));
    let status = rt.native_auto_set_enabled(false).await.unwrap();
    assert_eq!(status.apply_outcome, Some("applied"));
    let queued = rt.pending_force_restart.read().unwrap();
    let (id, queued, source) = queued.as_ref().unwrap();
    assert_eq!(*id, 77);
    assert_eq!(queued, &pending, "later mode changes cannot rewrite confirmed snapshot");
    assert!(matches!(source, ForceRestartSource::Confirmed { .. }));
    assert_eq!(rt.config.current().unwrap()["mixedPort"], 7999);
    let runtime = rt.current_config_snapshot().unwrap();
    assert!(runtime.get("selectionIntent").is_none());
    assert_eq!(runtime["selectedServerId"], "node-a");
    assert_eq!(runtime["mixedPort"], 7890);
    assert_eq!(runtime["autoSelectEnabled"], false);
    assert_eq!(sink.calls().len(), 3, "two scope freezes plus the same-leaf concrete default");
    rt.gate.bump_generation();
}

#[tokio::test]
async fn native_wrong_endpoint_dirty_connection_and_missing_history_stay_pending() {
    for source in ["url", "connection", "history", "missing"] {
        let (rt, _dir, sink, mut desired) = native_fixture(Some(AutoSelectTarget::Global), true);
        match source {
            "url" => {
                desired["speedTestUrl"] = serde_json::json!("https://other.example/generate_204")
            }
            "connection" => desired["servers"][0]["port"] = serde_json::json!(19009),
            "history" => {
                for group in sink.native_groups.lock().unwrap().as_mut().unwrap() {
                    for item in &mut group.items {
                        item.url_test_time = 0;
                    }
                }
            }
            "missing" => *sink.native_groups.lock().unwrap() = None,
            _ => unreachable!(),
        }
        rt.config.save_full(&desired).unwrap();
        let status = rt.native_auto_status().await;
        let global = status
            .native_auto_groups
            .iter()
            .find(|group| group.target == AutoSelectTarget::Global)
            .unwrap();
        assert!(!global.confirmed, "{source}");
        assert_eq!(global.state, "pending", "{source}");
        assert!(sink.calls().is_empty());
        if source == "missing" {
            assert_eq!(status.actual_server_id, None);
        } else {
            assert_eq!(status.actual_server_id.as_deref(), Some("node-a"));
        }
        assert!(rt.native_auto_set_enabled(false).await.is_err(), "{source}");
        assert!(rt.config.current().unwrap()["autoSelectEnabled"]
            .as_bool()
            .unwrap());
    }
}

#[tokio::test]
async fn native_status_event_fires_on_change_only_and_tracks_confirmed_actual_exit() {
    let (rt, _dir, sink, _config) = native_fixture(Some(AutoSelectTarget::Global), true);
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    rt.set_error_emitter(Box::new(RecordingErrorEmitter {
        auto_select_status: Arc::clone(&events),
        ..Default::default()
    }));
    for _ in 0..3 {
        rt.native_auto_status().await;
    }
    assert_eq!(events.lock().unwrap().len(), 1);
    let global = native::urltest_tag(&AutoSelectTarget::Global);
    sink.native_groups
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .iter_mut()
        .find(|group| group.tag == global)
        .unwrap()
        .selected = "Node B".into();
    rt.native_auto_status().await;
    rt.native_auto_status().await;
    let seen = events.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0]["actualServerId"], "node-a");
    assert_eq!(seen[1]["actualServerId"], "node-b");
    assert!(seen[1]["nativeAutoGroups"][0]["confirmed"]
        .as_bool()
        .unwrap());
}

#[test]
fn native_concurrent_announcements_never_deliver_an_old_generation_after_a_new_one() {
    let (rt, _dir, _sink, _config) = native_fixture(Some(AutoSelectTarget::Global), true);
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    rt.set_error_emitter(Box::new(RecordingErrorEmitter {
        auto_select_status: Arc::clone(&events),
        ..Default::default()
    }));
    rt.auto_select_announce();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                for _ in 0..200 {
                    rt.auto_select_announce();
                }
            });
        }
        scope.spawn(|| {
            for _ in 1..=50 {
                rt.gate.bump_generation();
                rt.auto_select_announce();
            }
        });
    });
    let seen: Vec<u64> = events
        .lock()
        .unwrap()
        .iter()
        .map(|status| status["generation"].as_u64().unwrap())
        .collect();
    assert!(seen.len() >= 2);
    assert!(seen.windows(2).all(|pair| pair[0] < pair[1]), "{seen:?}");
}

#[test]
fn native_route_projection_uses_running_emitted_members_and_clears_unknown_or_stopped() {
    let (rt, _dir, _sink, config) = native_fixture(Some(AutoSelectTarget::Global), true);
    let mut desired = config.clone();
    desired["servers"][0]["address"] = serde_json::json!("unapplied.example");
    rt.config.save_full(&desired).unwrap();
    rt.switch_snapshot
        .write()
        .unwrap()
        .as_mut()
        .unwrap()
        .id_to_tag
        .remove("node-c");
    let projected = rt.emitted_exit_route_config().unwrap();
    assert_eq!(projected.servers.len(), 2);
    assert!(!projected.servers.iter().any(|node| node.id == "node-c"));
    assert_eq!(
        projected.servers[0].address,
        config["servers"][0]["address"].as_str().unwrap()
    );
    *rt.switch_snapshot.write().unwrap() = None;
    assert!(rt.emitted_exit_route_config().is_none());
    rt.status.write().unwrap().running = false;
    assert!(rt.emitted_exit_route_config().is_none());
}

#[cfg(not(target_os = "ios"))]
#[test]
fn native_runtime_snapshot_excludes_configured_but_unemitted_nodes() {
    let (rt, _dir) = test_runtime();
    let mut raw = two_node_config(7890, "node-a");
    raw["autoSelectEnabled"] = serde_json::json!(true);
    raw["selectionIntent"] = polaris_store::auto_select::intent(&AutoSelectTarget::Global);
    let typed: UserConfig = serde_json::from_value(raw.clone()).unwrap();
    let deps = rt.generate_deps(30_101, 0, 0, None, &[20_010], &raw, false);
    let mut emitted =
        polaris_config_engine::builder::generate_sing_box_config(&typed, &BTreeMap::new(), &deps)
            .unwrap();
    assert!(serde_json::to_value(&emitted)
        .unwrap()
        .get("selectionIntent")
        .is_none());
    assert!(emitted
        .outbounds
        .iter()
        .any(|outbound| outbound.tag == native::wrapper_tag(&AutoSelectTarget::Global)));
    emitted
        .outbounds
        .retain(|outbound| outbound.tag != "Node B");
    let snapshot = ProxyRuntime::build_switch_snapshot(&typed, &emitted, &deps);
    assert!(snapshot.id_to_tag.contains_key("node-a"));
    assert!(!snapshot.id_to_tag.contains_key("node-b"));
}

#[tokio::test]
async fn native_manual_unattested_put_does_not_fall_through_to_structural_restart() {
    for target in ["node-a", "node-b"] {
        let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
        config["interruptConnectionsOnSwitch"] = serde_json::json!(true);
        let config = running(&rt, &config);
        let wrapper = native::wrapper_tag(&AutoSelectTarget::Global);
        seed_logical_exit_connections(&sink, &["Node A", &wrapper, PROXY_SELECTOR_TAG]);
        sink.follow_puts.store(false, Ordering::SeqCst);
        let (_, _, intent, _) = server_switch_core(
            &rt.config,
            target,
            |_| Ok(()),
            || rt.register_selector_intent(),
        )
        .unwrap();
        assert_eq!(
            rt.switch_selected_server_if_current(target, intent).await.unwrap(),
            Some(SwitchOutcome::Pending),
        );
        assert_eq!(
            rt.current_config_snapshot().unwrap()["selectionIntent"],
            config["selectionIntent"],
        );
        assert_eq!(
            rt.native_auto_status().await.actual_server_id.as_deref(),
            Some("node-a"),
        );
        assert!(sink.closed_ids.lock().unwrap().is_empty());
        assert_eq!(sink.connection_snapshot_calls.load(Ordering::SeqCst), 1);
        assert!(rt.pending_force_restart.read().unwrap().is_none());
        assert!(rt.selector_reconcile.is_required());
    }
}

#[tokio::test]
async fn native_auto_to_node_failed_publication_or_stale_readback_never_closes() {
    for target in ["node-a", "node-b"] {
        for race in ["put", "publication", "intent", "stop"] {
            let (rt, _dir, sink, mut config) = native_fixture(Some(AutoSelectTarget::Global), true);
            config["interruptConnectionsOnSwitch"] = serde_json::json!(true);
            let config = running(&rt, &config);
            let wrapper = native::wrapper_tag(&AutoSelectTarget::Global);
            seed_logical_exit_connections(&sink, &["Node A", &wrapper, PROXY_SELECTOR_TAG]);
            if race == "put" {
                sink.fail_at.lock().unwrap().insert(1);
            } else {
                let owner = Arc::clone(&rt);
                let at_put = Arc::clone(&sink);
                *sink.on_put.lock().unwrap() = Some(Box::new(move || {
                    let at_read = Arc::clone(&at_put);
                    *at_put.on_groups.lock().unwrap() = Some(Box::new(move || {
                        if race == "publication" {
                            let groups = at_read.native_groups.lock().unwrap().clone().unwrap();
                            assert!(owner.publish_native_groups(
                                owner.core_generation(), owner.native_observation_revision(), groups,
                            ).is_some());
                        } else if race == "stop" {
                            owner.gate.bump_generation();
                            owner.status.write().unwrap().running = false;
                        } else {
                            owner.register_selector_intent();
                        }
                    }));
                }));
            }
            let (_, _, intent, _) = server_switch_core(
                &rt.config, target, |_| Ok(()), || rt.register_selector_intent(),
            ).unwrap();
            assert_eq!(
                rt.switch_selected_server_if_current(target, intent).await.unwrap(),
                Some(SwitchOutcome::Pending),
            );
            assert_eq!(rt.current_config_snapshot().unwrap(), config);
            assert!(rt.config.current().unwrap().get("selectionIntent").is_none());
            assert_eq!(rt.config.current().unwrap()["selectedServerId"], target);
            assert!(sink.closed_ids.lock().unwrap().is_empty());
            assert_eq!(sink.connection_snapshot_calls.load(Ordering::SeqCst), 1);
            assert_eq!(sink.calls().len(), if race == "publication" { 2 } else { 1 });
            assert!(rt.pending_force_restart.read().unwrap().is_none());
        }
    }
}

#[tokio::test]
async fn native_off_refuses_divergent_default_reference_even_with_the_same_scope_set() {
    let (rt, _dir, sink, original) = native_fixture(Some(AutoSelectTarget::Global), true);
    let mut desired = original.clone();
    desired["selectionIntent"] = polaris_store::auto_select::intent(&subscription_target());
    // Keep Global referenced independently so target-set equality alone cannot
    // mistake an unapplied default change for the current operation's scope.
    desired["appRules"].as_array_mut().unwrap().push(serde_json::json!({
        "appId":"global-rule", "enabled":true, "action":"proxy",
        "targetAutoSelect":{"scope":"global"},
    }));
    let mut runtime = original.clone();
    runtime["appRules"] = desired["appRules"].clone();
    mark_running_with_snapshot(&rt, &runtime);
    rt.config.save_full(&desired).unwrap();
    assert_eq!(rt.native_auto_set_enabled(false).await.unwrap_err(), "AUTO_SELECT_REQUIRES_APPLY");
    assert!(sink.calls().is_empty());
    assert_eq!(rt.current_config_snapshot().unwrap(), runtime);
    assert_eq!(rt.config.current().unwrap()["selectionIntent"], desired["selectionIntent"]);
}

#[tokio::test]
async fn native_real_config_writer_invalidates_edited_pins_without_fabricating_refresh_winners() {
    let (rt, _dir, _sink, _config) = native_fixture(Some(AutoSelectTarget::Global), true);
    rt.native_auto_set_enabled(false).await.unwrap();
    let mut desired = rt.config.current().unwrap();
    desired["mixedPort"] = serde_json::json!(7999);
    rt.config.save_full(&desired).unwrap();
    let unchanged: UserConfig = serde_json::from_value(rt.config.current().unwrap()).unwrap();
    assert_eq!(
        unchanged.auto_select_pins.len(),
        2,
        "unrelated pending edits cannot discard verified pins"
    );
    desired["servers"][2]["address"] = serde_json::json!("replacement.example");
    rt.config.save_full(&desired).unwrap();
    let changed: UserConfig = serde_json::from_value(rt.config.current().unwrap()).unwrap();
    assert_eq!(
        native::pin(&changed, &AutoSelectTarget::Global),
        Some("node-a")
    );
    assert_eq!(native::pin(&changed, &subscription_target()), None);
    let status = rt.native_auto_status().await;
    assert_eq!(status.actual_server_id.as_deref(), Some("node-a"));
    assert!(
        !status
            .native_auto_groups
            .iter()
            .find(|group| group.target == subscription_target())
            .unwrap()
            .confirmed
    );
    rt.status.write().unwrap().running = false;
    assert_eq!(
        rt.native_auto_set_enabled(false).await.unwrap_err(),
        "AUTO_SELECT_NO_ACTUAL_EXIT"
    );
}

#[test]
fn native_retest_requests_http_without_an_app_winner_write() {
    let body = method_body(
        &module_code("runtime/proxy"),
        "    pub(crate) async fn native_auto_retest(",
    );
    assert!(body.contains(".url_test("));
    assert!(!body.contains("native_put(") && !body.contains("auto_select_explicit"));
    assert!(
        body.contains("self.native_auto_status().await"),
        "ACK is not a winner receipt"
    );
}

#[tokio::test]
async fn native_off_rejects_unapplied_runtime_only_or_retargeted_references_before_any_put() {
    for change in ["remove-rule", "empty-desired", "retarget-rule"] {
        let (rt, _dir, sink, original) = native_fixture(Some(AutoSelectTarget::Global), true);
        let runtime = rt.current_config_snapshot().unwrap();
        let mut desired = original.clone();
        if change == "retarget-rule" {
            desired["appRules"][0]["targetAutoSelect"] = serde_json::json!({"scope":"manual"});
        } else {
            desired["appRules"] = serde_json::json!([]);
        }
        if change == "empty-desired" {
            desired.as_object_mut().unwrap().remove("selectionIntent");
        }
        rt.config.save_full(&desired).unwrap();
        let saved = rt.config.current().unwrap();
        assert_eq!(
            rt.native_auto_set_enabled(false).await.unwrap_err(),
            "AUTO_SELECT_REQUIRES_APPLY",
            "{change}"
        );
        assert!(sink.calls().is_empty());
        assert_eq!(rt.config.current().unwrap(), saved);
        assert_eq!(rt.current_config_snapshot().unwrap(), runtime);
    }
}

#[tokio::test]
async fn native_mode_read_race_cannot_write_even_before_first_put() {
    for change in ["saved", "intent", "generation"] {
        let (rt, _dir, sink, _) = native_fixture(Some(AutoSelectTarget::Global), true);
        let owner = Arc::clone(&rt);
        *sink.on_groups.lock().unwrap() = Some(Box::new(move || match change {
            "saved" => {
                let mut config = owner.config.current().unwrap();
                config["appRules"] = serde_json::json!([]);
                owner.config.save_full(&config).unwrap();
            }
            "intent" => {
                owner.register_selector_intent();
            }
            _ => {
                owner.gate.bump_generation();
            }
        }));
        assert!(rt.native_auto_set_enabled(false).await.is_err());
        assert!(sink.calls().is_empty());
        assert_eq!(rt.config.current().unwrap()["autoSelectEnabled"], true);
    }
}

#[tokio::test(start_paused = true)]
async fn native_enable_absent_graph_reports_deferred_without_restart_and_preserves_runtime_flag() {
    let (rt, _dir, sink, _) = native_fixture(Some(AutoSelectTarget::Global), true);
    let off = rt.native_auto_set_enabled(false).await.unwrap();
    assert_eq!(off.apply_outcome, Some("applied"));
    let runtime = rt.current_config_snapshot();
    let saved_pins = rt.config.current().unwrap()["autoSelectPins"].clone();
    let calls_before = sink.calls().len();
    sink.native_groups.lock().unwrap().as_mut().unwrap()
        .retain(|group| !group.tag.ends_with("-test"));
    let groups_before = sink.native_groups.lock().unwrap().clone();
    let status = rt.native_auto_set_enabled(true).await.unwrap();
    assert_eq!(status.apply_outcome, Some("deferred"));
    assert_eq!(rt.config.current().unwrap()["autoSelectEnabled"], true);
    assert_eq!(rt.config.current().unwrap()["autoSelectPins"], saved_pins);
    assert_eq!(rt.current_config_snapshot(), runtime);
    assert_eq!(rt.current_config_snapshot().unwrap()["autoSelectEnabled"], false);
    assert_eq!(*sink.native_groups.lock().unwrap(), groups_before);
    assert_eq!(sink.calls().len(), calls_before, "missing groups cannot trigger partial PUTs");
    assert!(rt.pending_force_restart.read().unwrap().is_none());
    assert!(rt.restart_deferred.load(Ordering::SeqCst));
    rt.gate.bump_generation();
    assert_eq!(status.apply_outcome, Some("deferred"));
}

#[tokio::test]
async fn native_stopped_enable_returns_saved_not_runtime_applied() {
    let (rt, _dir, _sink, _) = native_fixture(None, false);
    rt.status.write().unwrap().running = false;
    let status = rt.native_auto_set_enabled(true).await.unwrap();
    assert_eq!(status.apply_outcome, Some("saved"));
    assert_eq!(status.actual_server_id, None);
}

#[tokio::test]
async fn native_mode_scheduler_failure_is_deferred_and_obsolete_claim_is_rejected() {
    let (rt, _dir, _sink, config) = native_fixture(None, false);
    let generation = rt.core_generation();
    let intent = rt.selector_reconcile.intent_generation();
    let owner = Arc::clone(&rt);
    let _ = std::thread::spawn(move || {
        let _lock = owner.pending_force_restart.write().unwrap();
        panic!("poison restart storage");
    })
    .join();
    assert_eq!(
        rt.commit_native_mode_projection(&config, generation, intent, true),
        Some(SwitchOutcome::Deferred)
    );
    rt.gate.bump_generation();
    assert_eq!(
        rt.commit_native_mode_projection(&config, generation, intent, true),
        None
    );
}

#[tokio::test]
async fn native_cancel_before_admission_or_during_status_never_uses_cached_completed_results() {
    use crate::commands::speedtest::SpeedTestCancel;
    use crate::runtime::speedtest::InterruptReason;
    for before in [true, false] {
        let (rt, _dir, sink, _) = native_fixture(Some(AutoSelectTarget::Global), true);
        let cancel = SpeedTestCancel::default();
        let observed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_by_read = Arc::clone(&observed);
        let during_read = cancel.clone();
        *sink.on_groups.lock().unwrap() = Some(Box::new(move || {
            observed_by_read.store(true, Ordering::SeqCst);
            if !before {
                during_read.cancel(InterruptReason::Preempted);
            }
        }));
        if before {
            cancel.cancel(InterruptReason::Cancelled);
        }
        assert!(rt
            .native_measure_requested(&["node-a".into()], &cancel)
            .await
            .is_empty());
        assert_eq!(observed.load(Ordering::SeqCst), !before);
        assert!(cancel.reason().is_some());
        assert!(sink.calls().is_empty());
        assert!(rt.pending_force_restart.read().unwrap().is_none());
    }
}

#[tokio::test]
async fn native_manual_sentinels_are_runtime_attested_despite_pending_saved_selection() {
    for id in ["__direct__", "__block__"] {
        let (rt, _dir, sink, mut applied) = native_fixture(None, false);
        applied["selectedServerId"] = serde_json::json!(id);
        applied["proxyMode"] = serde_json::json!("smart");
        mark_running_with_snapshot(&rt, &applied);
        *rt.startup_snapshot.write().unwrap() = Some(applied.clone());
        // Desired remains a different ordinary node; it cannot provide actual identity.
        assert_ne!(rt.config.current().unwrap()["selectedServerId"], id);
        {
            let mut groups = sink.native_groups.lock().unwrap();
            let proxy = groups
                .as_mut()
                .unwrap()
                .iter_mut()
                .find(|group| group.tag == PROXY_SELECTOR_TAG)
                .unwrap();
            proxy.selected = "direct".into();
            proxy.items.push(GroupItem {
                tag: "direct".into(),
                ..Default::default()
            });
        }
        assert_eq!(
            rt.native_auto_status().await.actual_server_id.as_deref(),
            Some(id)
        );
        assert_eq!(rt.status().actual_server_id.as_deref(), Some(id));
        if id == "__direct__" {
            *sink.native_groups.lock().unwrap() = None;
        } else {
            *rt.startup_snapshot.write().unwrap() = None;
        }
        assert_eq!(rt.native_auto_status().await.actual_server_id, None);
        assert_eq!(rt.status().actual_server_id, None);
    }
}

/// Deterministic suspension points: capture the one-shot ticket and A first,
/// accept B, then release the old success/failure. No timers or network involved.
#[tokio::test]
async fn native_observation_fences_old_success_failure_and_relay_queue() {
    let (rt, _dir, sink, _) = native_fixture(Some(AutoSelectTarget::Global), false);
    let events = Arc::new(Mutex::new(Vec::<Value>::new()));
    rt.set_error_emitter(Box::new(RecordingErrorEmitter {
        auto_select_status: Arc::clone(&events),
        ..Default::default()
    }));
    let generation = rt.core_generation();
    let a = sink.native_groups.lock().unwrap().clone().unwrap();
    let mut b = a.clone();
    let tag = rt
        .switch_snapshot
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .id_to_tag["node-b"]
        .clone();
    b.iter_mut()
        .find(|group| group.tag == PROXY_SELECTOR_TAG)
        .unwrap()
        .selected = tag;
    let suspended = rt.native_observation_revision();
    let relay = rt
        .publish_native_groups(generation, suspended, b.clone())
        .unwrap();
    assert_eq!(
        rt.native_auto_cached_status().actual_server_id.as_deref(),
        Some("node-b")
    );
    assert!(rt
        .publish_native_groups(generation, suspended, a.clone())
        .is_none());
    assert!(rt
        .publish_native_groups(generation, suspended, Vec::new())
        .is_none());
    assert_eq!(
        rt.native_auto_cached_status().actual_server_id.as_deref(),
        Some("node-b")
    );

    // A selector readback wins while the relay has an old A frame queued. Its
    // old connection ticket must be rejected, forcing a fresh subscription.
    let selector = rt.publish_native_groups(generation, relay, b).unwrap();
    assert!(rt
        .publish_native_groups(generation, relay, a.clone())
        .is_none());
    assert_eq!(
        rt.native_auto_cached_status().actual_server_id.as_deref(),
        Some("node-b")
    );
    assert_eq!(
        events.lock().unwrap().len(),
        1,
        "rejected frames must not announce stale state"
    );
    assert_eq!(events.lock().unwrap()[0]["actualServerId"], "node-b");
    rt.bump_generation();
    assert!(rt.publish_native_groups(generation, selector, a).is_none());
    assert_eq!(rt.native_auto_cached_status().actual_server_id, None);
}

#[tokio::test]
async fn native_status_inflight_response_cannot_replace_a_newer_observation() {
    for failed in [false, true] {
        let (rt, _dir, sink, _) = native_fixture(Some(AutoSelectTarget::Global), false);
        let mut b = sink.native_groups.lock().unwrap().clone().unwrap();
        let tag = rt
            .switch_snapshot
            .read()
            .unwrap()
            .as_ref()
            .unwrap()
            .id_to_tag["node-b"]
            .clone();
        b.iter_mut()
            .find(|group| group.tag == PROXY_SELECTOR_TAG)
            .unwrap()
            .selected = tag;
        if failed {
            *sink.native_groups.lock().unwrap() = None;
        }
        let newer = Arc::clone(&rt);
        // Existing request seam fires after the status ticket is captured, before
        // the request returns its independently held A (or failure).
        *sink.on_groups.lock().unwrap() = Some(Box::new(move || {
            let revision = newer.native_observation_revision();
            assert!(newer
                .publish_native_groups(newer.core_generation(), revision, b)
                .is_some());
        }));
        let status = rt.native_auto_status().await;
        assert_eq!(
            status.actual_server_id.as_deref(),
            Some("node-b"),
            "failed={failed}"
        );
    }
}

#[test]
fn native_measure_frame_rechecks_current_identity_and_membership_without_publishing() {
    for change in ["fingerprint", "remove"] {
        let (rt, _dir, sink, request_config) =
            native_fixture(Some(AutoSelectTarget::Global), false);
        let generation = rt.core_generation();
        let url = crate::commands::speedtest::resolve_speed_test_url(&request_config);
        let frame = sink.native_groups.lock().unwrap().clone().unwrap();
        let revision = rt.native_observation_revision();
        let initial = rt
            .native_measure_frame_status(generation, &url, &frame)
            .unwrap();
        assert!(initial
            .native_measurements
            .iter()
            .any(|sample| sample.server_id == "node-b"));

        // Request and native frame still describe the old emitted connection.
        // D changes during its wait without changing G, Auto, or test URL.
        let mut latest = request_config.clone();
        let servers = latest["servers"].as_array_mut().unwrap();
        if change == "fingerprint" {
            servers
                .iter_mut()
                .find(|node| node["id"] == "node-b")
                .unwrap()["port"] = serde_json::json!(19002);
        } else {
            servers.retain(|node| node["id"] != "node-b");
        }
        rt.config.save_full(&latest).unwrap();
        assert_eq!(rt.core_generation(), generation);
        // This is the production frame-processing seam, which loads D itself.
        let current = rt
            .native_measure_frame_status(generation, &url, &frame)
            .unwrap();
        assert!(
            !current
                .native_measurements
                .iter()
                .any(|sample| sample.server_id == "node-b"),
            "{change}"
        );
        assert!(
            current
                .native_measurements
                .iter()
                .any(|sample| sample.server_id == "node-a"),
            "unchanged clean sample: {change}"
        );
        assert_eq!(
            rt.native_observation_revision(),
            revision,
            "measurement frames stay local"
        );
    }
}
