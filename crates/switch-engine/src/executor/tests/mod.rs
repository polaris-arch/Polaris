#![allow(clippy::too_many_lines)]

use super::*;
use polaris_config_engine::builder::hotswitch::{HotSwitchKind, HotSwitchPut};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::task::Poll;
use tokio::sync::Semaphore;

fn global_plan() -> HotSwitchPlan {
    HotSwitchPlan {
        kind: HotSwitchKind::Global,
        puts: vec![HotSwitchPut {
            selector_tag: "proxy-selector".into(),
            member_tag: "tagB".into(),
            old_member_tag: Some("tagA".into()),
        }],
        must_restart: false,
    }
}

fn conn(id: &str, chains: &[&str], closed_at: i64) -> ConnectionSnapshot {
    ConnectionSnapshot {
        id: id.into(),
        chains: chains.iter().map(|s| s.to_string()).collect(),
        closed_at,
    }
}

#[tokio::test]
async fn execute_applies_all_puts_when_success() {
    let api = MockManagementApi::new();
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &global_plan(), false).await;
    match outcome {
        HotSwitchOutcome::Applied { disconnect } => {
            assert!(disconnect.is_none()); // 开关关
        }
        _ => panic!("expected Applied"),
    }
    // PUT 被调用。
    let calls = api.put_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], ("proxy-selector".into(), "tagB".into()));
}

#[tokio::test]
async fn execute_triggers_precision_disconnect_when_switch_on() {
    // 开关开 + 有命中旧成员的连接 → 断连。
    let api = MockManagementApi::new().with_connections(vec![
        conn("old-global", &["tagA", "proxy-selector", "rule-sel-r1"], 0),
        conn("rule-fixed", &["tagA", "rule-sel-r2"], 0), // 不含 proxy-selector
        conn("new", &["tagB", "proxy-selector"], 0),     // 新成员连接
    ]);
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &global_plan(), true).await;
    match outcome {
        HotSwitchOutcome::Applied {
            disconnect: Some(d),
        } => {
            assert_eq!(d.closed_ids, vec!["old-global".to_string()]);
        }
        _ => panic!("expected Applied with disconnect"),
    }
    // close_connection 被调用一次（关 old-global）。
    let close_calls = api.close_calls.lock().unwrap();
    assert_eq!(*close_calls, vec!["old-global".to_string()]);
}

#[tokio::test]
async fn execute_skips_disconnect_when_switch_off() {
    let api =
        MockManagementApi::new().with_connections(vec![conn("x", &["tagA", "proxy-selector"], 0)]);
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &global_plan(), false).await;
    match outcome {
        HotSwitchOutcome::Applied { disconnect } => assert!(disconnect.is_none()),
        _ => panic!("expected Applied"),
    }
    // 开关关 → 不查连接、不关连接。
    assert!(api.close_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn execute_returns_failed_on_put_error() {
    // 第二个 PUT 失败 → Failed（退回重启兜底）。第一个 PUT 仍执行（break 在失败处）。
    let api = MockManagementApi::new().with_put(
        "proxy-selector",
        "tagB",
        Err(ManagementError::Call("boom".into())),
    );
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &global_plan(), true).await;
    match outcome {
        HotSwitchOutcome::Failed {
            failed_selector,
            failed_member,
            error,
        } => {
            assert_eq!(failed_selector, "proxy-selector");
            assert_eq!(failed_member, "tagB");
            assert!(error.contains("boom"));
        }
        _ => panic!("expected Failed"),
    }
    // 失败后不执行断连。
    assert!(api.close_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn execute_returns_client_not_ready() {
    let api = MockManagementApi::new().not_ready();
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &global_plan(), true).await;
    assert_eq!(outcome, HotSwitchOutcome::ClientNotReady);
}

#[tokio::test]
async fn execute_first_put_failure_stops_loop() {
    // 两条 PUT，第一条失败 → 不尝试第二条（Polaris break）。
    let plan = HotSwitchPlan {
        kind: HotSwitchKind::Both,
        puts: vec![
            HotSwitchPut {
                selector_tag: "proxy-selector".into(),
                member_tag: "tagB".into(),
                old_member_tag: Some("tagA".into()),
            },
            HotSwitchPut {
                selector_tag: "rule-sel-r1".into(),
                member_tag: "tagX".into(),
                old_member_tag: Some("tagY".into()),
            },
        ],
        must_restart: false,
    };
    let api = MockManagementApi::new().with_put(
        "proxy-selector",
        "tagB",
        Err(ManagementError::Call("fail".into())),
    );
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &plan, true).await;
    assert!(matches!(outcome, HotSwitchOutcome::Failed { .. }));
    // 只调了第一条 PUT（break）。
    let calls = api.put_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "proxy-selector");
}

#[tokio::test]
async fn execute_both_kind_closes_global_and_rule_following_connections() {
    // kind=Both：global pair + rule pair 都断各自的旧成员连接。
    let plan = HotSwitchPlan {
        kind: HotSwitchKind::Both,
        puts: vec![
            HotSwitchPut {
                selector_tag: "proxy-selector".into(),
                member_tag: "tagB".into(),
                old_member_tag: Some("tagA".into()),
            },
            HotSwitchPut {
                selector_tag: "rule-sel-r1".into(),
                member_tag: "tagX".into(),
                old_member_tag: Some("tagA".into()),
            },
        ],
        must_restart: false,
    };
    let api = MockManagementApi::new().with_connections(vec![
        // 跟全局走旧节点A的规则连接 → 命中 global pair。
        conn("c1", &["tagA", "proxy-selector", "rule-sel-r1"], 0),
        // r1 规则固定走旧节点A → 命中 rule pair。
        conn("c2", &["tagA", "rule-sel-r1"], 0),
        // r2 规则连接 → 不命中（selector 不同）。
        conn("c3", &["tagA", "rule-sel-r2"], 0),
        // 死连接 → 跳过。
        conn("c4", &["tagA", "proxy-selector"], 999),
    ]);
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &plan, true).await;
    match outcome {
        HotSwitchOutcome::Applied {
            disconnect: Some(d),
        } => {
            // c1 + c2 命中（c3 不命中、c4 死连接跳过）。
            assert_eq!(d.closed_ids.len(), 2);
            assert!(d.closed_ids.contains(&"c1".to_string()));
            assert!(d.closed_ids.contains(&"c2".to_string()));
        }
        _ => panic!("expected Applied"),
    }
}

#[tokio::test]
async fn execute_no_disconnect_when_pairs_empty() {
    // puts 有 PUT 但 oldMemberTag 全缺 / old==new → pairs 空 → 不查连接、不关。
    let plan = HotSwitchPlan {
        kind: HotSwitchKind::Global,
        puts: vec![HotSwitchPut {
            selector_tag: "proxy-selector".into(),
            member_tag: "tagB".into(),
            old_member_tag: None, // 无旧成员 → 无 pair
        }],
        must_restart: false,
    };
    let api =
        MockManagementApi::new().with_connections(vec![conn("x", &["tagA", "proxy-selector"], 0)]);
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &plan, true).await;
    match outcome {
        HotSwitchOutcome::Applied {
            disconnect: Some(d),
        } => {
            assert_eq!(d.closed_count(), 0); // 无 pair → 无断连
        }
        _ => panic!("expected Applied"),
    }
    // pairs 空 → 不关连接。
    assert!(api.close_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn execute_empty_puts_applies_without_disconnect() {
    // 空 plan（无 puts）→ Applied，无 PUT、无断连。
    let plan = HotSwitchPlan::default();
    let api = MockManagementApi::new();
    let exec = SwitchExecutor;
    let outcome = exec.execute(&api, &plan, true).await;
    match outcome {
        HotSwitchOutcome::Applied { disconnect } => {
            // 空 puts → pairs 空 → disconnect 是 Some(空 outcome)（开关开但无 pair）。
            assert!(disconnect.is_some());
            assert_eq!(disconnect.unwrap().closed_count(), 0);
        }
        _ => panic!("expected Applied"),
    }
    assert!(api.put_calls.lock().unwrap().is_empty());
}

// 每条 close 真正挂起，只有测试释放其 permit 才完成；无需 spawn 或墙钟等待。
// guard 同时记录在途峰值和 future drop，避免“最后关了 40 条”冒充并发/取消证明。
#[derive(Default)]
struct BlockingCloseState {
    puts: Vec<(String, String)>,
    puts_before_snapshot: Vec<(String, String)>,
    snapshot_calls: usize,
    attempted: Vec<String>,
    entered: Vec<String>,
    completed: Vec<String>,
    dropped: Vec<String>,
    rejected: Vec<String>,
    failed: Vec<String>,
    active: usize,
    peak: usize,
}

struct BlockingManagementApi {
    snapshot: Vec<ConnectionSnapshot>,
    close_gates: HashMap<String, Semaphore>,
    state: Mutex<BlockingCloseState>,
    generation_valid: AtomicBool,
    fail_close: Option<String>,
    fail_put: Option<String>,
    fail_snapshot: bool,
}

impl BlockingManagementApi {
    fn new(snapshot: Vec<ConnectionSnapshot>) -> Self {
        let close_gates = snapshot
            .iter()
            .map(|c| (c.id.clone(), Semaphore::new(0)))
            .collect();
        Self {
            snapshot,
            close_gates,
            state: Mutex::new(BlockingCloseState::default()),
            generation_valid: AtomicBool::new(true),
            fail_close: None,
            fail_put: None,
            fail_snapshot: false,
        }
    }

    fn release(&self, id: &str) {
        self.close_gates.get(id).unwrap().add_permits(1);
    }

    fn release_all(&self) {
        for gate in self.close_gates.values() {
            gate.add_permits(1);
        }
    }
}

struct ActiveClose<'a> {
    state: &'a Mutex<BlockingCloseState>,
    id: String,
    completed: bool,
}

impl Drop for ActiveClose<'_> {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        state.active -= 1;
        if self.completed {
            state.completed.push(self.id.clone());
        } else {
            state.dropped.push(self.id.clone());
        }
    }
}

#[async_trait]
impl ManagementApi for BlockingManagementApi {
    async fn select_outbound(
        &self,
        selector_tag: &str,
        member_tag: &str,
    ) -> Result<(), ManagementError> {
        self.state
            .lock()
            .unwrap()
            .puts
            .push((selector_tag.into(), member_tag.into()));
        if self.fail_put.as_deref() == Some(selector_tag) {
            return Err(ManagementError::Call("selected PUT failed".into()));
        }
        Ok(())
    }

    async fn first_connection_snapshot(&self) -> Result<Vec<ConnectionSnapshot>, ManagementError> {
        let mut state = self.state.lock().unwrap();
        state.snapshot_calls += 1;
        state.puts_before_snapshot = state.puts.clone();
        if self.fail_snapshot {
            return Err(ManagementError::SnapshotTimeout);
        }
        Ok(self.snapshot.clone())
    }

    async fn close_connection(&self, id: &str) -> Result<(), ManagementError> {
        {
            let mut state = self.state.lock().unwrap();
            assert_eq!(state.snapshot_calls, 1, "close must follow the snapshot");
            state.attempted.push(id.into());
            // 模拟收到的 API wrapper 在每次 RPC 入场时复核 generation/intent。
            if !self.generation_valid.load(Ordering::SeqCst) {
                state.rejected.push(id.into());
                return Err(ManagementError::NotReady);
            }
            state.entered.push(id.into());
            state.active += 1;
            state.peak = state.peak.max(state.active);
        }
        let mut active = ActiveClose {
            state: &self.state,
            id: id.into(),
            completed: false,
        };
        let permit = self.close_gates.get(id).unwrap().acquire().await.unwrap();
        permit.forget();
        active.completed = true;
        if self.fail_close.as_deref() == Some(id) {
            self.state.lock().unwrap().failed.push(id.into());
            return Err(ManagementError::Call("selected close failed".into()));
        }
        Ok(())
    }
}

fn selected_snapshot(count: usize) -> Vec<ConnectionSnapshot> {
    (0..count)
        .map(|i| conn(&format!("selected-{i}"), &["tagA", "proxy-selector"], 0))
        .collect()
}

fn selected_ids(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("selected-{i}")).collect()
}

fn applied_ids(outcome: HotSwitchOutcome) -> Vec<String> {
    match outcome {
        HotSwitchOutcome::Applied {
            disconnect: Some(disconnect),
        } => disconnect.closed_ids,
        _ => panic!("expected Applied with disconnect"),
    }
}

fn assert_same_ids(actual: &[String], expected: &[String]) {
    let mut actual = actual.to_vec();
    let mut expected = expected.to_vec();
    actual.sort();
    expected.sort();
    assert_eq!(actual, expected);
}

fn assert_send<T: Send>(_: &T) {}

#[tokio::test]
async fn execute_bounds_blocking_closes_at_sixteen_and_drains_later_waves() {
    // 字面值 16 是契约，不从实现常量反推，否则把上限改错也会同步“通过”。
    for count in [0, 1, 16, 17, 40] {
        let api = BlockingManagementApi::new(selected_snapshot(count));
        let exec = SwitchExecutor;
        let plan = global_plan();
        let mut execution = Box::pin(exec.execute(&api, &plan, true));
        // 外层 runtime 需要 Send；只写静态契约，不创建 detached task。
        assert_send(&execution);
        let first_poll = futures::poll!(execution.as_mut());
        let outcome = if count == 0 {
            match first_poll {
                Poll::Ready(outcome) => outcome,
                Poll::Pending => panic!("empty selection must not wait for a close"),
            }
        } else {
            assert!(first_poll.is_pending());
            {
                let state = api.state.lock().unwrap();
                assert_eq!(state.entered.len(), count.min(16));
                assert_eq!(state.active, count.min(16));
                assert_eq!(state.peak, count.min(16));
                assert!(state.completed.is_empty());
            }
            api.release_all();
            execution.await
        };
        assert_eq!(applied_ids(outcome), selected_ids(count));
        let state = api.state.lock().unwrap();
        assert_same_ids(&state.entered, &selected_ids(count));
        assert_same_ids(&state.completed, &selected_ids(count));
        assert_eq!(state.active, 0);
        assert!(state.peak <= 16);
        assert!(state.dropped.is_empty());
    }
}

#[tokio::test]
async fn execute_keeps_selected_order_and_best_effort_with_unordered_closes() {
    let mut snapshot = selected_snapshot(40);
    snapshot.extend([
        conn("ghost", &["tagA", "proxy-selector"], 1),
        conn("new-member", &["tagB", "proxy-selector"], 0),
        conn("fixed-old-member", &["tagA", "rule-sel-fixed"], 0),
        conn("direct", &["direct"], 0),
        conn("unknown-chain", &[], 0),
    ]);
    let mut api = BlockingManagementApi::new(snapshot);
    api.fail_close = Some("selected-3".into());
    let exec = SwitchExecutor;
    let plan = global_plan();
    let mut execution = Box::pin(exec.execute(&api, &plan, true));
    assert!(futures::poll!(execution.as_mut()).is_pending());
    // 先完成第一波的最后一条；第二波必须能入场，不等最早那条完成。
    api.release("selected-15");
    assert!(futures::poll!(execution.as_mut()).is_pending());
    {
        let state = api.state.lock().unwrap();
        assert_eq!(state.completed, vec!["selected-15".to_string()]);
        assert_eq!(state.entered.len(), 17);
        assert!(state.entered.contains(&"selected-16".to_string()));
        assert_eq!(state.active, 16);
    }
    api.release_all();
    let HotSwitchOutcome::Applied {
        disconnect: Some(result),
    } = execution.await
    else {
        panic!("close failures must not roll back selector selection");
    };
    assert_eq!(result.selected_ids, selected_ids(40));
    assert_eq!(result.failed_ids, vec!["selected-3".to_string()]);
    assert_eq!(
        result.closed_ids,
        selected_ids(40)
            .into_iter()
            .filter(|id| id != "selected-3")
            .collect::<Vec<_>>()
    );
    let state = api.state.lock().unwrap();
    assert_same_ids(&state.attempted, &selected_ids(40));
    assert_same_ids(&state.completed, &selected_ids(40));
    assert_eq!(state.failed, vec!["selected-3".to_string()]);
    assert_eq!(state.active, 0);
    assert_eq!(state.peak, 16);
}

#[tokio::test]
async fn execute_rechecks_received_api_gate_for_later_close_waves() {
    let api = BlockingManagementApi::new(selected_snapshot(40));
    let exec = SwitchExecutor;
    let plan = global_plan();
    let mut execution = Box::pin(exec.execute(&api, &plan, true));
    assert!(futures::poll!(execution.as_mut()).is_pending());
    assert_eq!(api.state.lock().unwrap().entered.len(), 16);
    api.generation_valid.store(false, Ordering::SeqCst);
    api.release_all();
    // wrapper 已发出的第一波不能撤回；后续 24 条只能被 wrapper 拒绝。
    // Only the first wave actually received successful RPC acknowledgements.
    let HotSwitchOutcome::Applied {
        disconnect: Some(result),
    } = execution.await
    else {
        panic!("expected applied selector outcome");
    };
    assert_eq!(result.closed_ids, selected_ids(16));
    assert_eq!(result.failed_ids, selected_ids(40)[16..]);
    let state = api.state.lock().unwrap();
    assert_same_ids(&state.attempted, &selected_ids(40));
    assert_same_ids(&state.entered, &selected_ids(16));
    assert_same_ids(&state.completed, &selected_ids(16));
    assert_same_ids(&state.rejected, &selected_ids(40)[16..]);
    assert_eq!(state.active, 0);
    assert_eq!(state.peak, 16);
}

#[tokio::test]
async fn dropping_execute_drops_in_flight_closes_without_starting_stragglers() {
    let api = BlockingManagementApi::new(selected_snapshot(40));
    let exec = SwitchExecutor;
    let plan = global_plan();
    let mut execution = Box::pin(exec.execute(&api, &plan, true));
    assert!(futures::poll!(execution.as_mut()).is_pending());
    assert_eq!(api.state.lock().unwrap().active, 16);
    drop(execution);
    {
        let state = api.state.lock().unwrap();
        assert_eq!(state.active, 0, "close futures must drop with their owner");
        assert_same_ids(&state.dropped, &selected_ids(16));
        assert_same_ids(&state.attempted, &selected_ids(16));
        assert!(state.completed.is_empty());
    }
    api.release_all();
    tokio::task::yield_now().await;
    let state = api.state.lock().unwrap();
    assert_same_ids(&state.attempted, &selected_ids(16));
    assert_same_ids(&state.dropped, &selected_ids(16));
    assert!(state.completed.is_empty());
    assert_eq!(state.active, 0);
}

#[tokio::test]
async fn execute_finishes_ordered_puts_before_snapshot_and_close() {
    let mut plan = global_plan();
    plan.kind = HotSwitchKind::Both;
    plan.puts.push(HotSwitchPut {
        selector_tag: "rule-sel-r1".into(),
        member_tag: "tagX".into(),
        old_member_tag: Some("tagY".into()),
    });
    let expected_puts = vec![
        ("proxy-selector".to_string(), "tagB".to_string()),
        ("rule-sel-r1".to_string(), "tagX".to_string()),
    ];
    let api = BlockingManagementApi::new(selected_snapshot(17));
    let exec = SwitchExecutor;
    let mut execution = Box::pin(exec.execute(&api, &plan, true));
    assert!(futures::poll!(execution.as_mut()).is_pending());
    {
        let state = api.state.lock().unwrap();
        assert_eq!(state.puts, expected_puts);
        assert_eq!(state.puts_before_snapshot, expected_puts);
        assert_eq!(state.snapshot_calls, 1);
        assert_eq!(state.entered.len(), 16);
    }
    api.release_all();
    assert_eq!(applied_ids(execution.await), selected_ids(17));

    let mut api = BlockingManagementApi::new(selected_snapshot(17));
    api.fail_put = Some("proxy-selector".into());
    let outcome = exec.execute(&api, &plan, true).await;
    assert!(matches!(outcome, HotSwitchOutcome::Failed { .. }));
    let state = api.state.lock().unwrap();
    assert_eq!(state.puts, expected_puts[..1]);
    assert_eq!(state.snapshot_calls, 0);
    assert!(state.attempted.is_empty());
}

#[tokio::test]
async fn execute_skip_and_snapshot_failure_paths_never_start_close_futures() {
    for scenario in [
        "switch-off",
        "no-old-member",
        "unchanged-member",
        "snapshot-fails",
    ] {
        let mut api = BlockingManagementApi::new(selected_snapshot(17));
        let mut plan = global_plan();
        let interrupt = scenario != "switch-off";
        match scenario {
            "no-old-member" => plan.puts[0].old_member_tag = None,
            "unchanged-member" => plan.puts[0].old_member_tag = Some("tagB".into()),
            "snapshot-fails" => api.fail_snapshot = true,
            _ => {}
        }
        let exec = SwitchExecutor;
        let mut execution = Box::pin(exec.execute(&api, &plan, interrupt));
        let outcome = match futures::poll!(execution.as_mut()) {
            Poll::Ready(outcome) => outcome,
            Poll::Pending => panic!("{scenario} must not wait for a close"),
        };
        if interrupt {
            assert!(applied_ids(outcome).is_empty());
        } else {
            assert_eq!(outcome, HotSwitchOutcome::Applied { disconnect: None });
        }
        let state = api.state.lock().unwrap();
        assert_eq!(
            state.snapshot_calls,
            usize::from(scenario == "snapshot-fails")
        );
        assert!(state.attempted.is_empty());
        assert_eq!(state.active, 0);
    }
}

mod audit_regressions;

#[tokio::test]
async fn all_close_failures_remain_applied_but_never_count_as_acknowledged() {
    let api = BlockingManagementApi::new(selected_snapshot(3));
    api.generation_valid.store(false, Ordering::SeqCst);
    let HotSwitchOutcome::Applied {
        disconnect: Some(result),
    } = SwitchExecutor.execute(&api, &global_plan(), true).await
    else {
        panic!("best-effort close failures must not undo a successful selector PUT");
    };
    assert!(result.closed_ids.is_empty());
    assert_eq!(result.failed_ids, selected_ids(3));
    assert_eq!(result.selected_ids, selected_ids(3));
}
