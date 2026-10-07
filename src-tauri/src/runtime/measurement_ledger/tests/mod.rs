use super::*;
use crate::commands::speedtest::{FailKind, FailPhase};

const URL: &str = "url-digest";
const CAP_MS: u64 = 60 * 60_000;

fn identity(run: u64, seq: u64, measured_at: u64) -> ResultIdentity {
    ResultIdentity {
        run,
        seq,
        origin: SpeedTestOrigin::Schedule,
        scope: None,
        path: MeasurePath::Candidate,
        url_digest: URL.to_string(),
        instance: CoreInstance::Main {
            generation: 7,
            start_time: None,
        },
        config_digest: None,
        node_fingerprint: Some("fp".to_string()),
        network_epoch: Some(3),
        measured_at,
    }
}

const TIMED_OUT: MeasureFailure = MeasureFailure::new(FailPhase::Measure, FailKind::Timeout);

fn fingerprints(ids: &[&str]) -> BTreeMap<String, String> {
    ids.iter()
        .map(|id| ((*id).to_string(), "fp".to_string()))
        .collect()
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// 读取对照面：世代 7、网络代次 3、此刻 `now_ms`，新鲜期上限一小时。
fn context<'a>(
    fingerprints: &'a BTreeMap<String, String>,
    now_ms: u64,
    foreground_epoch: Option<u64>,
) -> ReadContext<'a> {
    ReadContext {
        main_generation: Some(7),
        fingerprints,
        network_epoch: Some(3),
        url_digest: URL,
        now_ms,
        foreground_epoch,
        freshness_cap_ms: &|_| CAP_MS,
    }
}

fn exclusion_of(candidates: &Candidates, id: &str) -> Exclusion {
    candidates
        .excluded
        .iter()
        .find(|(node, _)| node == id)
        .map(|(_, exclusion)| exclusion.clone())
        .unwrap_or_else(|| panic!("{id} 不在被排除的列表里：{candidates:?}"))
}

/// 入账：运行号更大的取代已有记录；更旧的被拒并计数，已有记录原样保留。
#[test]
fn an_older_run_is_rejected_and_counted() {
    let ledger = MeasurementLedger::new();
    assert!(ledger.record("a", Ok(120), identity(5, 1, 1_000)));
    assert!(
        !ledger.record("a", Ok(999), identity(4, 9, 2_000)),
        "旧运行号"
    );
    assert!(
        !ledger.record("a", Ok(999), identity(5, 1, 2_000)),
        "同号同序"
    );
    assert_eq!(ledger.rejected_not_newer(), 2);
    assert_eq!(ledger.candidate_entry("a", URL).unwrap().measured, Ok(120));
    assert!(
        ledger.record("a", Ok(80), identity(5, 2, 3_000)),
        "同号序号更大"
    );
    assert_eq!(ledger.len(), 1);
}

/// 版本：入账与淘汰各加一，被拒的入账不加；等待方在版本越过所见值时被唤醒。
#[tokio::test]
async fn the_version_advances_on_every_accepted_record() {
    let ledger: &'static MeasurementLedger = Box::leak(Box::new(MeasurementLedger::new()));
    assert_eq!(ledger.version(), 0);
    let waiter = tokio::spawn(ledger.changed_since(0));
    tokio::task::yield_now().await;
    ledger.record("a", Ok(1), identity(2, 1, 0));
    assert_eq!(waiter.await.unwrap(), 1);
    ledger.record("a", Ok(1), identity(1, 1, 0));
    assert_eq!(ledger.version(), 1, "被拒的入账不改版本");
}

/// 读取判据表：同一批记录，对照面每变一项，对应节点从可选点变成带原因的排除。
#[test]
fn reading_classifies_every_node_with_a_reason() {
    let ledger = MeasurementLedger::new();
    ledger.record("ok", Ok(120), identity(1, 1, 10_000));
    ledger.record("failed", Err(TIMED_OUT), identity(1, 2, 10_000));
    ledger.record(
        "system",
        Ok(50),
        ResultIdentity {
            path: MeasurePath::System,
            ..identity(1, 3, 10_000)
        },
    );
    ledger.record(
        "temp",
        Ok(60),
        ResultIdentity {
            instance: CoreInstance::Temp,
            ..identity(1, 4, 10_000)
        },
    );
    ledger.set_skipped(&ids(&["skipped"]), &[("skipped".to_string(), "notInPool")]);
    let all = ids(&["ok", "failed", "system", "temp", "skipped", "never"]);
    let fps = fingerprints(&["ok", "failed", "system", "temp", "skipped", "never"]);

    let now = ledger.candidates(&all, &context(&fps, 20_000, None));
    assert_eq!(
        now.selectable,
        vec![Candidate {
            node_id: "ok".to_string(),
            latency_ms: 120,
            measured_at: 10_000,
            run: 1,
        }],
        "停止态与 system 路径的结果不进可选点"
    );
    // 失败与没测是两类：只有真测了没通的才带失败次数。
    assert_eq!(
        exclusion_of(&now, "failed"),
        Exclusion::Failed {
            failure: TIMED_OUT,
            consecutive: 1
        }
    );
    assert_eq!(exclusion_of(&now, "never"), Exclusion::NeverMeasured);
    assert_eq!(
        exclusion_of(&now, "skipped"),
        Exclusion::Skipped("notInPool")
    );
    assert_eq!(exclusion_of(&now, "system"), Exclusion::NonCandidatePath);
    assert_eq!(exclusion_of(&now, "temp"), Exclusion::Disconnected);

    let only = ids(&["ok"]);
    // 核换代：旧世代的结果过期，不可选点。
    let regenerated = ReadContext {
        main_generation: Some(8),
        ..context(&fps, 20_000, None)
    };
    assert_eq!(
        exclusion_of(&ledger.candidates(&only, &regenerated), "ok"),
        Exclusion::Stale(LedgerStale::Identity(StaleReason::Instance))
    );
    // 桌面切网：网络代次变了。
    let moved = ReadContext {
        network_epoch: Some(4),
        ..context(&fps, 20_000, None)
    };
    assert_eq!(
        exclusion_of(&ledger.candidates(&only, &moved), "ok"),
        Exclusion::Stale(LedgerStale::Identity(StaleReason::NetworkEpoch))
    );
    // 新鲜期上限：刚好到上限仍可用，超过一毫秒即过期。
    let at_cap = ledger.candidates(&only, &context(&fps, 10_000 + CAP_MS, None));
    assert_eq!(at_cap.selectable.len(), 1);
    let past_cap = ledger.candidates(&only, &context(&fps, 10_001 + CAP_MS, None));
    assert_eq!(
        exclusion_of(&past_cap, "ok"),
        Exclusion::Stale(LedgerStale::Expired)
    );
}

/// 前台代次只在给出时判（移动端）：代次变了，此前入账的结果不可选点；桌面不看它。
#[test]
fn a_foreground_epoch_change_voids_earlier_results_on_mobile_only() {
    let ledger = MeasurementLedger::new();
    let fps = fingerprints(&["a"]);
    let only = ids(&["a"]);
    ledger.record("a", Ok(120), identity(1, 1, 10_000));
    assert_eq!(
        ledger
            .candidates(&only, &context(&fps, 20_000, Some(0)))
            .selectable
            .len(),
        1
    );
    ledger.set_foreground_epoch(1);
    assert_eq!(
        exclusion_of(
            &ledger.candidates(&only, &context(&fps, 20_000, Some(1))),
            "a"
        ),
        Exclusion::Stale(LedgerStale::ForegroundEpoch)
    );
    assert_eq!(
        ledger
            .candidates(&only, &context(&fps, 20_000, None))
            .selectable
            .len(),
        1,
        "桌面不按前台代次判"
    );
    // 新代次下重测即恢复。
    ledger.record("a", Ok(90), identity(2, 1, 20_000));
    assert_eq!(
        ledger
            .candidates(&only, &context(&fps, 20_000, Some(1)))
            .selectable[0]
            .latency_ms,
        90
    );
}

/// 连续失败计数：失败累加，成功归零；手动测过即从头数。
#[test]
fn consecutive_failures_reset_on_success_or_a_manual_run() {
    let ledger = MeasurementLedger::new();
    let failures = |ledger: &MeasurementLedger| {
        ledger
            .candidate_entry("a", URL)
            .unwrap()
            .consecutive_failures
    };
    for run in 1..=3 {
        ledger.record("a", Err(TIMED_OUT), identity(run, 1, run));
    }
    assert_eq!(failures(&ledger), 3);
    ledger.record(
        "a",
        Err(TIMED_OUT),
        ResultIdentity {
            origin: SpeedTestOrigin::Manual,
            ..identity(4, 1, 4)
        },
    );
    assert_eq!(failures(&ledger), 1, "手动测过：从头数");
    ledger.record("a", Ok(100), identity(5, 1, 5));
    assert_eq!(failures(&ledger), 0);
    ledger.record("a", Err(TIMED_OUT), identity(6, 1, 6));
    let entry = ledger.candidate_entry("a", URL).unwrap();
    assert_eq!(
        (entry.consecutive_failures, entry.last_ok_at),
        (1, Some(5)),
        "最近一次成功的时刻跨过随后的失败保留"
    );
}

/// 已有当前结果的节点 = 测出了值的 + 真测了没通的；过期、被跳过、从未测过的都不算。
#[test]
fn current_results_exclude_stale_skipped_and_unmeasured_nodes() {
    let ledger = MeasurementLedger::new();
    ledger.record("ok", Ok(120), identity(1, 1, 10_000));
    ledger.record("failed", Err(TIMED_OUT), identity(1, 2, 10_000));
    ledger.record("old", Ok(120), identity(1, 3, 0));
    let all = ids(&["ok", "failed", "old", "never"]);
    let fps = fingerprints(&["ok", "failed", "old", "never"]);
    let current = ledger.current_results(&all, &context(&fps, CAP_MS + 5_000, None));
    assert_eq!(current, ids(&["failed", "ok"]).into_iter().collect());
}

/// 淘汰顺序：先是节点已不在配置里的键，再是旧测速 URL 且已过新鲜期的键，最后按测量时刻从旧到新。
#[test]
fn eviction_follows_the_documented_order() {
    let ledger = MeasurementLedger::new();
    let other_url = |run, at| ResultIdentity {
        url_digest: "old-url".to_string(),
        ..identity(run, 1, at)
    };
    ledger.record("gone", Ok(1), identity(1, 1, 9_000_000));
    ledger.record("a", Ok(1), other_url(2, 0)); // 旧 URL，已过新鲜期
    ledger.record("a", Ok(1), identity(3, 1, 5_000_000));
    ledger.record("b", Ok(1), other_url(4, 9_000_000)); // 旧 URL，但还新鲜
    ledger.record("b", Ok(1), identity(5, 1, 4_000_000));
    let nodes: BTreeSet<String> = ids(&["a", "b"]).into_iter().collect();
    let context = EvictContext {
        nodes: &nodes,
        url_digest: URL,
        now_ms: 9_000_000,
        freshness_cap_ms: &|_| CAP_MS,
    };
    let order: Vec<(String, String, u64)> = {
        let inner = ledger.lock();
        eviction_order(&inner.entries, &context)
            .into_iter()
            .map(|key| {
                let at = inner.entries[&key].identity.measured_at;
                (key.node_id, key.url_digest, at)
            })
            .collect()
    };
    assert_eq!(
        order,
        vec![
            ("gone".to_string(), URL.to_string(), 9_000_000),
            ("a".to_string(), "old-url".to_string(), 0),
            ("b".to_string(), URL.to_string(), 4_000_000),
            ("a".to_string(), URL.to_string(), 5_000_000),
            ("b".to_string(), "old-url".to_string(), 9_000_000),
        ]
    );

    // 软上限 = 节点数的 4 倍 = 8，五条都留；节点只剩一个时软上限为 4，按上面的顺序淘汰一条。
    assert_eq!(ledger.evict(&context), 0);
    let one: BTreeSet<String> = ids(&["a"]).into_iter().collect();
    let version = ledger.version();
    assert_eq!(
        ledger.evict(&EvictContext {
            nodes: &one,
            ..context
        }),
        1
    );
    assert_eq!(ledger.len(), 4);
    // 此时 b 与 gone 都已不在配置里，同属第一类；类内按测量时刻，最旧的是 b 的那条。
    assert!(ledger.candidate_entry("b", URL).is_none());
    assert!(ledger.candidate_entry("gone", URL).is_some());
    assert_eq!(ledger.version(), version + 1, "淘汰后版本加一");
}

/// 条数不超过硬上限：越过时淘汰测量时刻最旧的一条。
#[test]
fn the_ledger_never_grows_past_the_hard_cap() {
    let ledger = MeasurementLedger::new();
    for index in 0..=HARD_CAP as u64 {
        ledger.record(&format!("n{index}"), Ok(1), identity(index + 1, 1, index));
    }
    assert_eq!(ledger.len(), HARD_CAP);
    assert!(
        ledger.candidate_entry("n0", URL).is_none(),
        "最旧的一条被淘汰"
    );
    assert!(ledger.candidate_entry("n1", URL).is_some());
}

/// 首轮是否完成：一轮被抢占（还有节点没测到）为否，补发完成后为是。起测前被预筛跳过的
/// 节点不算欠账；失败也算覆盖到了。
#[test]
fn a_round_covers_a_subscription_only_once_every_testable_member_has_a_result() {
    let ledger = MeasurementLedger::new();
    let members = ids(&["a", "b", "c", "skipped"]);
    let fps = fingerprints(&["a", "b", "c", "skipped"]);
    let covered = |ledger: &MeasurementLedger| {
        ledger
            .candidates(&members, &context(&fps, 20_000, Some(0)))
            .covers_all_testable()
    };
    ledger.set_skipped(&members, &[("skipped".to_string(), "dirty")]);
    assert!(!covered(&ledger), "还没测过");
    // 周期一轮测完 a 之后被手动测速抢占。
    ledger.record("a", Ok(120), identity(1, 1, 10_000));
    assert!(!covered(&ledger), "被抢占：b、c 还没有结果");
    // 补发完成（b 没通也算测到了）。
    ledger.record("b", Err(TIMED_OUT), identity(2, 1, 11_000));
    ledger.record("c", Ok(90), identity(2, 2, 11_000));
    assert!(covered(&ledger));
    // 前台代次变了：旧结果不再算数，要重新覆盖。
    ledger.set_foreground_epoch(1);
    assert!(!ledger
        .candidates(&members, &context(&fps, 20_000, Some(1)))
        .covers_all_testable());
}
