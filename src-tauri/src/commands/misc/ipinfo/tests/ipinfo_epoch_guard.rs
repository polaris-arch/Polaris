use crate::commands::guard_scan::top_level_fn_body;
use crate::test_support::crate_code;

fn body(signature: &str) -> String {
    top_level_fn_body(&crate_code("commands/misc/ipinfo.rs"), signature)
}

#[test]
fn refresh_entrypoints_preserve_recovery_semantics() {
    assert!(body("pub fn schedule_ipinfo_refresh(app:")
        .contains("schedule_ipinfo_refresh_inner(app, delay_ms, false)"));
    assert!(body("pub fn schedule_network_recovery_refresh(app:")
        .contains("schedule_ipinfo_refresh_inner(app, IPINFO_SETTLE_DELAY_MS, true)"));
}

#[test]
fn manual_and_scheduled_probes_share_the_commit_gate() {
    let manual = body("pub async fn ipinfo_get(");
    let scheduler = body("fn schedule_ipinfo_refresh_inner(");
    assert!(manual.contains("begin_manual_probe()") && manual.contains("probe_publish_ipinfo("));
    assert!(
        scheduler.contains("begin_scheduled_probe(ticket, delay_ms > 0)")
            && scheduler.contains("probe_publish_ipinfo(")
    );
    let begin = body("fn begin_manual_probe(");
    let lock = begin.find("ipinfo_state().lock()").unwrap();
    let seq = begin.find("declare_schedule_locked()").unwrap();
    let epoch = begin.find("IPINFO_REFRESH_EPOCH.fetch_add").unwrap();
    assert!(lock < seq && seq < epoch, "手点宣告与领世代须同锁且按序");
}

#[test]
fn pending_stop_and_commit_are_serialized_under_one_lock() {
    let declare = body("fn declare_scheduled_refresh(");
    let lock = declare.find("ipinfo_state().lock()").unwrap();
    let seq = declare.find("declare_schedule_locked()").unwrap();
    let pending = declare
        .find("state.publish(pending_ipinfo_snapshot(), false)")
        .unwrap();
    let stop = declare.find("state.publish(stopped, true)").unwrap();
    assert!(lock < seq && seq < pending && seq < stop);
    assert!(
        declare.contains("state.pending_seq = None")
            && declare.contains("stopped[\"proxy\"] = Value::Null")
    );

    let begin = body("fn begin_scheduled_probe(");
    assert!(
        begin.contains("state.pending_seq != Some(ticket)")
            && begin.find("state.pending_seq != Some(ticket)").unwrap()
                < begin.find("IPINFO_REFRESH_EPOCH.fetch_add").unwrap()
    );
    let commit = body("fn commit_ipinfo_snapshot(");
    assert!(
        commit.find("ipinfo_state().lock()").unwrap()
            < commit.find("ipinfo_probe_is_current(epoch, seq)").unwrap()
    );
    assert!(
        commit.find("ipinfo_probe_is_current(epoch, seq)").unwrap()
            < commit.find("state.publish(snap.clone(), true)").unwrap()
    );
    let sequencer = body("fn next_ipinfo_schedule_seq(");
    assert!(
        sequencer.find("ipinfo_state().lock()").unwrap()
            < sequencer.find("IPINFO_SCHEDULE_SEQ.fetch_add").unwrap()
    );
}

#[test]
fn scheduler_publishes_before_sleep_then_finishes_its_own_ticket() {
    let scheduler = body("fn schedule_ipinfo_refresh_inner(");
    let declare = scheduler
        .find("declare_scheduled_refresh(delay_ms)")
        .unwrap();
    let broadcast = scheduler.find("crate::events::broadcast(").unwrap();
    let spawn = scheduler.find("tauri::async_runtime::spawn").unwrap();
    let sleep = scheduler.find("tokio::time::sleep(").unwrap();
    let begin = scheduler
        .find("begin_scheduled_probe(ticket, delay_ms > 0)")
        .unwrap();
    let probe = scheduler.find("probe_publish_ipinfo(").unwrap();
    let finish = scheduler.find("finish_scheduled_probe(ticket)").unwrap();
    assert!(
        declare < broadcast
            && broadcast < spawn
            && spawn < sleep
            && sleep < begin
            && begin < probe
            && probe < finish
    );
    assert!(
        !scheduler.contains("return") && !scheduler.contains('?'),
        "排程任务不能跳过自己的收尾"
    );
    assert!(
        scheduler.contains("maybe_recheck_unlock_after_exit_recovery(")
            && scheduler.contains("schedule_unreachable_ipinfo_recheck(")
    );
}

#[test]
fn blocked_state_invalidates_old_probe_before_publishing() {
    let blocked = body("fn commit_proxy_blocked_snapshot(");
    let lock = blocked.find("ipinfo_state().lock()").unwrap();
    let declare = blocked.find("declare_schedule_locked()").unwrap();
    let fold = blocked
        .find("fold_proxy_blocked(state.cache.clone(), reason)")
        .unwrap();
    let publish = blocked.find("state.publish(snap, true)").unwrap();
    assert!(lock < declare && declare < fold && fold < publish);
    assert!(blocked.contains("state.pending_seq = None"));
    let mark = body("pub(crate) fn mark_ipinfo_proxy_blocked(");
    assert!(
        mark.find("commit_proxy_blocked_snapshot(reason)").unwrap()
            < mark.find("crate::events::broadcast(").unwrap()
    );
}

#[test]
fn stale_result_cannot_broadcast_or_start_warm_probe() {
    let publish = body("async fn probe_publish_ipinfo(");
    let gate = publish
        .find("commit_ipinfo_snapshot(epoch, seq, &snap)")
        .unwrap();
    let stale = publish
        .find("return (peek_ipinfo_snapshot(), false)")
        .unwrap();
    let broadcast = publish.find("crate::events::broadcast(").unwrap();
    let warm = publish.find("spawn_warm_rtt_probe(").unwrap();
    assert!(gate < stale && stale < broadcast && broadcast < warm);
    assert!(publish[warm..].contains("epoch,") && publish[warm..].contains("seq,"));
}

#[test]
fn unreachable_retry_keeps_warning_visible_and_claims_current_schedule() {
    let retry = body("fn schedule_unreachable_ipinfo_recheck(");
    assert!(
        retry.contains("tauri::async_runtime::spawn")
            && retry.contains("claim_unreachable_retry_probe(expected_seq)")
            && retry.contains("probe_publish_ipinfo(")
            && retry.contains("maybe_recheck_unlock_after_exit_recovery(")
    );
    assert!(!retry.contains("pending_ipinfo_snapshot()"));
    assert!(body("fn claim_schedule_seq(").contains("compare_exchange"));
    let claim = body("fn claim_unreachable_retry_probe(");
    assert!(
        claim.find("ipinfo_state().lock()").unwrap()
            < claim
                .find("claim_schedule_seq(&IPINFO_SCHEDULE_SEQ, expected)")
                .unwrap()
    );
    assert!(
        claim
            .find("claim_schedule_seq(&IPINFO_SCHEDULE_SEQ, expected)")
            .unwrap()
            < claim.find("IPINFO_REFRESH_EPOCH.fetch_add").unwrap()
    );
}
