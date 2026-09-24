//! N4 canary 命中态的运行时纯逻辑（spec §11 N4 验收 ⑤）：状态跃迁、探测循环（注入查询函数，不起核、
//! 不发包）、应答判读。起核门（真核回环）在 `crates/config-engine/tests/network_profile_runtime.rs`。

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

fn plan(ids: &[&str]) -> NetworkCanaryPlan {
    NetworkCanaryPlan {
        port: 53999,
        canaries: ids
            .iter()
            .enumerate()
            .map(|(k, id)| NetworkCanary {
                profile_id: (*id).to_string(),
                domain: format!("p{k}.np-canary.polaris.invalid"),
            })
            .collect(),
    }
}

/// 查询桩：按域名回放可变的结果表，并记录被问过几次。
#[derive(Clone, Default)]
struct Stub {
    answers: Arc<Mutex<BTreeMap<String, Option<bool>>>>,
    asked: Arc<AtomicUsize>,
}

impl Stub {
    fn set(&self, domain: &str, value: Option<bool>) {
        self.answers.lock().unwrap().insert(domain.into(), value);
    }
    fn query(&self) -> impl Fn(SocketAddr, String) -> std::future::Ready<Option<bool>> + '_ {
        move |addr, domain| {
            assert_eq!(
                addr,
                SocketAddr::from((Ipv4Addr::LOCALHOST, 53999)),
                "只问回环探针口"
            );
            self.asked.fetch_add(1, Ordering::SeqCst);
            std::future::ready(self.answers.lock().unwrap().get(&domain).copied().flatten())
        }
    }
}

/// 让出执行权直到 `cond` 成立（暂停时钟下 sleep 会自动推进；上限防挂死）。
async fn until(cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("条件在时限内未成立");
}

#[test]
fn state_transitions_unknown_on_arm_invalidate_and_disarm() {
    let state = NetworkCanaryState::default();
    assert_eq!(state.matched("np-a"), None, "从未起核 ⇒ 未知");

    let (session, changed) = state.arm(Some(plan(&["np-a", "np-b"])));
    assert!(changed, "新会话带来新的场景集 ⇒ 对外可见状态变化");
    assert_eq!(state.matched("np-a"), None, "arm 后探完之前一律未知");
    let (round, port, canaries) = state.target(session).expect("本会话有目标");
    assert_eq!(port, 53999);
    assert_eq!(canaries.len(), 2);
    assert!(state.record(
        round,
        BTreeMap::from([("np-a".into(), Some(true)), ("np-b".into(), Some(false))])
    ));
    assert_eq!(state.matched("np-a"), Some(true));
    assert_eq!(state.matched("np-b"), Some(false));

    assert!(state.invalidate(), "网络变化：已知结果作废 ⇒ 状态变化");
    assert_eq!(state.matched("np-a"), None);
    assert_eq!(state.matched("np-b"), None);
    assert!(
        !state.record(round, BTreeMap::from([("np-a".into(), Some(true))])),
        "网络变化前在飞的那一轮结果不许写回"
    );
    assert_eq!(state.matched("np-a"), None);
    assert!(
        state.target(session).is_some(),
        "网络变化不换会话，探测任务继续"
    );

    assert!(state.disarm(), "核停：清空结果");
    assert_eq!(state.matched("np-a"), None);
    assert!(
        state.target(session).is_none(),
        "核停 ⇒ 旧会话无目标，探测任务退场"
    );

    let (next, _) = state.arm(None);
    assert_ne!(next, session);
    assert!(state.target(next).is_none(), "本次没有 canary ⇒ 不探");
}

/// 验收 ⑤：探测循环（注入查询函数）——起核就绪探一轮 → 网络变化立即置未知并重探 → 核停退场；
/// 重启（再 arm）后旧任务不再写回、新任务重新探测。
#[tokio::test(start_paused = true)]
async fn probe_loop_reprobes_on_network_change_and_exits_on_stop() {
    let state = Arc::new(NetworkCanaryState::default());
    let stub = Stub::default();
    stub.set("p0.np-canary.polaris.invalid", Some(true));
    stub.set("p1.np-canary.polaris.invalid", Some(false));
    let changes = Arc::new(AtomicUsize::new(0));

    let (session, _) = state.arm(Some(plan(&["np-a", "np-b"])));
    let task = {
        let (state, stub, changes) = (Arc::clone(&state), stub.clone(), Arc::clone(&changes));
        tokio::spawn(async move {
            run_canary_probe(&state, session, CANARY_PROBE_INTERVAL, stub.query(), || {
                changes.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        })
    };
    until(|| state.matched("np-a") == Some(true)).await;
    assert_eq!(state.matched("np-b"), Some(false));
    assert_eq!(changes.load(Ordering::SeqCst), 1, "首轮结果 ⇒ 一次变更信号");

    // 网络变化：立即置未知，且**不等周期**就重探出新结果。
    stub.set("p0.np-canary.polaris.invalid", Some(false));
    stub.set("p1.np-canary.polaris.invalid", Some(true));
    let asked_before = stub.asked.load(Ordering::SeqCst);
    assert!(state.invalidate());
    assert_eq!(state.matched("np-a"), None, "网络变化那一刻即为未知");
    until(|| state.matched("np-b") == Some(true)).await;
    assert_eq!(state.matched("np-a"), Some(false));
    assert_eq!(
        stub.asked.load(Ordering::SeqCst),
        asked_before + 2,
        "网络变化后只重探一轮（未等 5s 周期）"
    );

    // 周期探测：结果不变不发信号。
    let signals = changes.load(Ordering::SeqCst);
    tokio::time::sleep(CANARY_PROBE_INTERVAL * 2).await;
    until(|| stub.asked.load(Ordering::SeqCst) >= asked_before + 6).await;
    assert_eq!(
        changes.load(Ordering::SeqCst),
        signals,
        "结果不变 ⇒ 不重复发信号"
    );

    // 核停：结果清空，任务退场。
    assert!(state.disarm());
    until(|| task.is_finished()).await;
    assert_eq!(state.matched("np-a"), None);

    // 重启：新会话从未知开始、重新探测。
    let (session2, _) = state.arm(Some(plan(&["np-a"])));
    assert_eq!(state.matched("np-a"), None);
    let task2 = {
        let (state, stub) = (Arc::clone(&state), stub.clone());
        tokio::spawn(async move {
            run_canary_probe(&state, session2, CANARY_PROBE_INTERVAL, stub.query(), || {}).await;
        })
    };
    until(|| state.matched("np-a") == Some(false)).await;
    state.disarm();
    until(|| task2.is_finished()).await;
}

/// 查询超时 / 应答畸形 ⇒ 未知（不当成「未命中」）。
#[tokio::test(start_paused = true)]
async fn unanswered_query_stays_unknown_not_miss() {
    let state = Arc::new(NetworkCanaryState::default());
    let stub = Stub::default();
    stub.set("p0.np-canary.polaris.invalid", None);
    let (session, _) = state.arm(Some(plan(&["np-a"])));
    let task = {
        let (state, stub) = (Arc::clone(&state), stub.clone());
        tokio::spawn(async move {
            run_canary_probe(&state, session, CANARY_PROBE_INTERVAL, stub.query(), || {}).await;
        })
    };
    until(|| stub.asked.load(Ordering::SeqCst) >= 1).await;
    tokio::task::yield_now().await;
    assert_eq!(state.matched("np-a"), None);
    state.disarm();
    until(|| task.is_finished()).await;
}

#[test]
fn canary_answer_parsing() {
    let reply = |id: u16, flags: [u8; 2], answers: u16| {
        let mut b = id.to_be_bytes().to_vec();
        b.extend_from_slice(&flags);
        b.extend_from_slice(&[0, 1]);
        b.extend_from_slice(&answers.to_be_bytes());
        b.extend_from_slice(&[0, 0, 0, 0]);
        b
    };
    let id = CANARY_QUERY_ID;
    assert_eq!(
        parse_canary_answer(id, &reply(id, [0x81, 0x80], 1)),
        Some(true)
    );
    assert_eq!(
        parse_canary_answer(id, &reply(id, [0x81, 0x83], 0)),
        Some(false)
    );
    assert_eq!(
        parse_canary_answer(id, &reply(id, [0x81, 0x80], 0)),
        None,
        "NOERROR 但无答案 ⇒ 未知"
    );
    assert_eq!(
        parse_canary_answer(id, &reply(id, [0x81, 0x82], 0)),
        None,
        "SERVFAIL ⇒ 未知"
    );
    assert_eq!(
        parse_canary_answer(id, &reply(id ^ 1, [0x81, 0x80], 1)),
        None,
        "id 不符"
    );
    assert_eq!(
        parse_canary_answer(id, &reply(id, [0x01, 0x80], 1)),
        None,
        "不是应答（QR=0）"
    );
    assert_eq!(parse_canary_answer(id, &[0; 5]), None, "截断");

    let q = dns_query(id, "p0.np-canary.polaris.invalid.");
    assert_eq!(&q[..2], &id.to_be_bytes());
    assert_eq!(&q[4..6], &[0, 1], "一个问题");
    assert!(q.ends_with(&[7, b'i', b'n', b'v', b'a', b'l', b'i', b'd', 0, 0, 1, 0, 1]));
}

/// 换核（再次 arm 而中间没有 disarm，例如就绪腿被重复走到）：旧会话的探测任务必须退场，
/// 不得与新任务并存、往新会话里写结果。
#[tokio::test(start_paused = true)]
async fn rearm_retires_previous_session_task() {
    let state = Arc::new(NetworkCanaryState::default());
    let stub = Stub::default();
    stub.set("p0.np-canary.polaris.invalid", Some(true));
    let (old, _) = state.arm(Some(plan(&["np-a"])));
    let task = {
        let (state, stub) = (Arc::clone(&state), stub.clone());
        tokio::spawn(async move {
            run_canary_probe(&state, old, CANARY_PROBE_INTERVAL, stub.query(), || {}).await;
        })
    };
    until(|| state.matched("np-a") == Some(true)).await;
    let (new, _) = state.arm(Some(plan(&["np-a"])));
    assert_ne!(new, old);
    until(|| task.is_finished()).await;
    assert_eq!(
        state.matched("np-a"),
        None,
        "新会话从未知开始，旧任务没有写回"
    );
    state.disarm();
}
