use super::*;

#[test]
fn visible_window_resets_countdown() {
    assert_eq!(next_hidden_secs(570, TICK_SECS, true, true), 0);
}

#[test]
fn hidden_window_accumulates_and_saturates() {
    assert_eq!(next_hidden_secs(0, TICK_SECS, false, true), 30);
    assert_eq!(next_hidden_secs(570, TICK_SECS, false, true), 600);
    assert_eq!(next_hidden_secs(u64::MAX, TICK_SECS, false, true), u64::MAX);
}

#[test]
fn reclaim_boundary_is_exactly_twenty_ticks() {
    assert_eq!(HIDDEN_RECLAIM_SECS, 10 * 60);
    let mut elapsed = 0;
    for tick in 1..=20 {
        elapsed = next_hidden_secs(elapsed, TICK_SECS, false, true);
        assert_eq!(elapsed >= HIDDEN_RECLAIM_SECS, tick == 20);
    }
}

#[test]
fn one_visible_tick_restarts_full_countdown() {
    let elapsed = next_hidden_secs(590, TICK_SECS, true, true);
    assert!(next_hidden_secs(elapsed, TICK_SECS, false, true) < HIDDEN_RECLAIM_SECS);
}

#[test]
fn no_tray_never_accumulates_or_reclaims_a_minimized_window() {
    let mut elapsed = 0;
    for _ in 0..40 {
        elapsed = next_hidden_secs(elapsed, TICK_SECS, false, false);
        assert_eq!(elapsed, 0);
        assert!(elapsed < HIDDEN_RECLAIM_SECS);
    }
}

#[test]
fn lost_return_anchor_resets_pending_reclaim_and_restoration_starts_fresh() {
    assert_eq!(next_hidden_secs(570, TICK_SECS, false, false), 0);
    assert_eq!(
        next_hidden_secs(HIDDEN_RECLAIM_SECS, TICK_SECS, false, false),
        0
    );
    let mut elapsed = next_hidden_secs(570, TICK_SECS, false, false);
    for tick in 1..=20 {
        elapsed = next_hidden_secs(elapsed, TICK_SECS, false, true);
        assert_eq!(elapsed >= HIDDEN_RECLAIM_SECS, tick == 20);
    }
}

#[test]
fn queued_automatic_transition_rechecks_actual_return_anchor_before_destruction() {
    // Source-only wiring fixture: the live tray lookup must remain inside the UI callback,
    // ahead of the shared destructive transition. A pre-queue snapshot is insufficient.
    let source = include_str!("../../idle_lightweight.rs");
    let callback = source
        .split_once("let post = app.run_on_main_thread(move || {")
        .unwrap()
        .1;
    let (before_transition, _) = callback
        .split_once("crate::tray::enter_lightweight_transition(app_for_main.clone());")
        .unwrap();
    assert!(before_transition.contains("if !crate::tray::tray_present(&app_for_main)"));
    assert!(before_transition.contains("|| !auto_lightweight_enabled(&app_for_main)"));
    let (_, after_anchor_check) = before_transition
        .split_once("if !crate::tray::tray_present(&app_for_main)")
        .unwrap();
    let (guard, _) = after_anchor_check.split_once('}').unwrap();
    assert!(guard.contains("return;"));
}
