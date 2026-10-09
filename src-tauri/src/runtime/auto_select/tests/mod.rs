use super::*;
use crate::commands::speedtest::{FailKind, FailPhase, MeasureFailure};
use crate::runtime::measurement_ledger::LedgerStale;
use serde_json::json;

const MINUTE: u64 = 60_000;
/// 远离零点的起始时刻（单调毫秒）：冷却、驻留的「距今多久」不会被零点截断。
const T0: u64 = 100 * 60 * MINUTE;
/// 一个与单调时刻毫无关系的墙钟读数：用例里凡是「相隔多久」都不该读它。
const WALL: u64 = 1_700_000_000_000;

fn auto() -> Intent {
    Intent::Auto {
        subscription_id: "sub".to_string(),
    }
}

const ON: Switches = Switches {
    master: true,
    better_leg: true,
};

/// 一个可选点：`run` 是测出它的那一轮，`mono` 是入账的单调时刻。墙钟时刻故意取一个无关的值。
fn candidate(id: &str, latency_ms: u32, run: u64, mono: u64) -> Candidate {
    Candidate {
        node_id: id.to_string(),
        latency_ms,
        measured_at: WALL,
        measured_mono: mono,
        run,
    }
}

/// 当前出口有新鲜的值，且与 `with` 同一轮测得。
fn fresh(latency_ms: u32, with: &Candidate) -> Current {
    Current::Fresh {
        latency_ms,
        run: with.run,
        measured_mono: with.measured_mono,
    }
}

/// 闸门全开的一份后台输入。
fn facts<'a>(intent: &'a Intent, current: Current, candidates: &'a [Candidate]) -> Facts<'a> {
    Facts {
        intent,
        switches: ON,
        platform_open: true,
        core_running: true,
        explicit: false,
        foreground: true,
        reconciling: false,
        config_pending: false,
        measuring: false,
        members: 3,
        first_round_done: true,
        starved: None,
        current,
        current_id: Some("cur"),
        candidates,
        restart_blocked: 0,
        restart_candidates: &[],
        now: T0,
        last_attempt_at: None,
    }
}

fn switch(target: &str, latency_ms: u32, cause: Cause) -> Decision {
    Decision::Switch {
        target: target.to_string(),
        latency_ms,
        cause,
    }
}

const NO_RESULTS: [NoResult; 4] = [
    NoResult::Expired,
    NoResult::Unmeasured,
    NoResult::Skipped,
    NoResult::Mismatch,
];

// ── 意图与平台闸 ──────────────────────────────────────────────────────────────

/// 意图键的读取：只认规范形；总开关关着或本平台未开放时不生效。结构完好而取值不认识的是
/// 「本版本不认识的意图」，损坏的与缺席的一样是手动。
#[test]
fn the_intent_is_effective_only_when_switched_on_and_the_platform_is_open() {
    let off = Switches {
        master: false,
        better_leg: true,
    };
    let config = json!({ "selectionIntent": polaris_store::selection_intent_auto("sub") });
    assert_eq!(
        effective_subscription(&config, ON, Platform::Linux),
        Some("sub")
    );
    assert_eq!(effective_subscription(&config, off, Platform::Linux), None);
    assert_eq!(
        effective_subscription(&json!({}), ON, Platform::Linux),
        None
    );
    assert_eq!(Intent::stored(&config), auto());
    assert_eq!(Intent::stored(&json!({})), Intent::Manual);
    for (value, stored) in [
        (json!("sub"), Intent::Manual),
        (
            json!({ "mode": "auto", "subscriptionId": "sub" }),
            Intent::Manual,
        ),
        (
            json!({ "mode": "auto", "scope": "subscription", "subscriptionId": "" }),
            Intent::Manual,
        ),
        (
            json!({ "mode": "auto", "scope": "global" }),
            Intent::Unrecognized,
        ),
        (
            json!({ "mode": "pinned", "scope": "subscription", "subscriptionId": "sub" }),
            Intent::Unrecognized,
        ),
    ] {
        let config = json!({ "selectionIntent": value });
        assert_eq!(
            effective_subscription(&config, ON, Platform::Linux),
            None,
            "{config}"
        );
        assert_eq!(Intent::stored(&config), stored, "{config}");
    }
}

/// iOS 本轮不开放：意图在那里不生效，设置意图的命令据 [`closed`] 拒绝。其余四个平台开放。
#[test]
fn ios_is_closed_and_every_other_platform_is_open() {
    let config = json!({ "selectionIntent": polaris_store::selection_intent_auto("sub") });
    assert_eq!(closed(ON, Platform::Ios), Some(Gate::PlatformNotOpen));
    assert_eq!(effective_subscription(&config, ON, Platform::Ios), None);
    for platform in [
        Platform::Mac,
        Platform::Win,
        Platform::Linux,
        Platform::Android,
    ] {
        assert_eq!(closed(ON, platform), None, "{platform:?}");
        assert_eq!(
            effective_subscription(&config, ON, platform),
            Some("sub"),
            "{platform:?}"
        );
    }
    // 总开关先于平台：两者都不满足时报总开关。
    let off = Switches {
        master: false,
        better_leg: true,
    };
    assert_eq!(closed(off, Platform::Ios), Some(Gate::SwitchedOff));
    assert_eq!(closed(off, Platform::Linux), Some(Gate::SwitchedOff));
}

/// 订阅成员按配置顺序取，不含别的订阅与自建节点。
#[test]
fn members_are_the_nodes_of_that_subscription_in_config_order() {
    let config = json!({ "servers": [
        { "id": "b", "subscriptionId": "sub" },
        { "id": "own" },
        { "id": "x", "subscriptionId": "other" },
        { "id": "a", "subscriptionId": "sub" },
    ]});
    assert_eq!(subscription_members(&config, "sub"), ["b", "a"]);
    assert!(subscription_members(&config, "none").is_empty());
}

// ── 择优裁决的真值表 ──────────────────────────────────────────────────────────

/// 条目 1：意图为手动（或本版本不认识），任何输入都不评估，后台与显式路径都一样。
#[test]
fn a_manual_intent_is_never_evaluated() {
    let far_better = [candidate("a", 1, 1, T0)];
    for (intent, gate) in [
        (Intent::Manual, Gate::Manual),
        (Intent::Unrecognized, Gate::IntentUnrecognized),
    ] {
        for current in [
            Current::Outside,
            fresh(900, &far_better[0]),
            Current::Failed { consecutive: 9 },
            Current::NoResult(NoResult::Expired),
        ] {
            for explicit in [false, true] {
                let mut memory = Memory::default();
                let mut input = facts(&intent, current, &far_better);
                input.explicit = explicit;
                assert_eq!(decide(&input, &mut memory), Decision::NotEvaluated(gate));
                assert_eq!(memory, Memory::default(), "不生效的意图下不留任何记忆");
            }
        }
    }
}

/// 前置闸门逐项与次序：每关掉一项得到对应原因；同时关两项时报排在前面的那一项。
#[test]
fn every_gate_reports_its_own_reason_in_priority_order() {
    let intent = auto();
    let best = [candidate("a", 10, 1, T0)];
    let open = || facts(&intent, Current::Outside, &best);
    type Close = fn(&mut Facts<'_>);
    let gates: [(Gate, Close); 12] = [
        (Gate::SwitchedOff, |f| f.switches.master = false),
        (Gate::PlatformNotOpen, |f| f.platform_open = false),
        (Gate::CoreNotRunning, |f| f.core_running = false),
        (Gate::Reconciling, |f| f.reconciling = true),
        (Gate::ConfigPending, |f| f.config_pending = true),
        (Gate::Background, |f| f.foreground = false),
        (Gate::SubscriptionEmpty, |f| f.members = 0),
        (Gate::Measuring, |f| f.measuring = true),
        (Gate::WaitingFirstRound, |f| f.first_round_done = false),
        (Gate::PeriodicDisabled, |f| {
            f.first_round_done = false;
            f.starved = Some(Starved::PeriodicDisabled);
        }),
        (Gate::MeteredPaused, |f| {
            f.first_round_done = false;
            f.starved = Some(Starved::MeteredPaused);
        }),
        (Gate::PowerSave, |f| {
            f.first_round_done = false;
            f.starved = Some(Starved::PowerSave);
        }),
    ];
    assert_eq!(gate(&open()), None);
    for (index, (expected, close)) in gates.iter().enumerate() {
        let mut one = open();
        close(&mut one);
        assert_eq!(gate(&one), Some(*expected));
        assert_eq!(
            decide(&one, &mut Memory::default()),
            Decision::NotEvaluated(*expected)
        );
        // 再关掉排在后面的每一项：结论不变。最后三项是同一道闸的三种原因，互相不比。
        if index < 9 {
            for (_, later) in gates.iter().take(9).skip(index + 1) {
                let mut two = open();
                close(&mut two);
                later(&mut two);
                assert_eq!(gate(&two), Some(*expected), "{expected:?} 应先于后面的闸门");
            }
        }
    }
    // 显式路径：对账、待应用配置、前后台、测速在飞、首轮都不拦；其余照拦。
    for (index, (expected, close)) in gates.iter().enumerate() {
        let mut explicit = open();
        explicit.explicit = true;
        close(&mut explicit);
        let applies = matches!(index, 0..=2 | 6);
        assert_eq!(
            gate(&explicit),
            applies.then_some(*expected),
            "{expected:?}"
        );
    }
}

/// 条目 23：手机不在前台不评估；桌面的同一份输入（恒在前台）照常评估。
#[test]
fn a_phone_in_the_background_is_not_evaluated_while_a_desktop_is() {
    let intent = auto();
    let best = [candidate("a", 80, 1, T0)];
    let mut phone = facts(&intent, Current::Outside, &best);
    phone.foreground = false;
    assert_eq!(
        decide(&phone, &mut Memory::default()),
        Decision::NotEvaluated(Gate::Background)
    );
    let desktop = facts(&intent, Current::Outside, &best);
    assert_eq!(
        decide(&desktop, &mut Memory::default()),
        switch("a", 80, Cause::FirstPlacement)
    );
}

/// 条目 22：有测速在飞时不评估，收口后评估一次。
#[test]
fn nothing_is_decided_while_a_measurement_is_in_flight() {
    let intent = auto();
    let best = [candidate("a", 80, 1, T0)];
    let mut flying = facts(&intent, Current::Outside, &best);
    flying.measuring = true;
    let mut memory = Memory::default();
    assert_eq!(
        decide(&flying, &mut memory),
        Decision::NotEvaluated(Gate::Measuring)
    );
    flying.measuring = false;
    assert_eq!(
        decide(&flying, &mut memory),
        switch("a", 80, Cause::FirstPlacement)
    );
}

/// 条目 2、3、14：当前出口不在订阅内 → 落到延迟最小的候选，不要求连胜与驻留；并列取配置顺序
/// 靠前者；候选为空不换（不转直连，不借别的订阅）。
#[test]
fn an_exit_outside_the_subscription_lands_on_the_fastest_candidate() {
    let intent = auto();
    let two = [candidate("slow", 120, 1, T0), candidate("fast", 80, 1, T0)];
    let mut memory = Memory::default();
    // 刚换过点（驻留未满）也照落：首次落点不受驻留约束。
    memory.record_switch(Cause::Failover, T0 - MINUTE - SWITCH_COOLDOWN_MS);
    let mut outside = facts(&intent, Current::Outside, &two);
    outside.last_attempt_at = Some(T0 - MINUTE - SWITCH_COOLDOWN_MS);
    assert_eq!(
        decide(&outside, &mut memory),
        switch("fast", 80, Cause::FirstPlacement)
    );

    let tied = [
        candidate("first", 80, 1, T0),
        candidate("second", 80, 1, T0),
    ];
    assert_eq!(
        decide(
            &facts(&intent, Current::Outside, &tied),
            &mut Memory::default()
        ),
        switch("first", 80, Cause::FirstPlacement),
        "延迟并列取配置顺序靠前者"
    );

    assert_eq!(
        decide(
            &facts(&intent, Current::Outside, &[]),
            &mut Memory::default()
        ),
        Decision::Hold(Hold::NoCandidates)
    );
}

/// 条目 33 的后半：有可选点、但切过去都得整核重启 → 「需重启才能切换」，不是「无可用候选」。
#[test]
fn candidates_that_all_need_a_restart_are_reported_as_such() {
    let intent = auto();
    let mut blocked = facts(&intent, Current::Outside, &[]);
    blocked.restart_blocked = 2;
    assert_eq!(
        decide(&blocked, &mut Memory::default()),
        Decision::Hold(Hold::NeedsRestart)
    );
    // 当前出口有新鲜值时不算问题：没有挑战者而已。
    let peer = candidate("x", 100, 1, T0);
    let mut settled = facts(&intent, fresh(100, &peer), &[]);
    settled.restart_blocked = 2;
    assert_eq!(
        decide(&settled, &mut Memory::default()),
        Decision::Hold(Hold::Settled)
    );
}

/// 条目 4–7：容差是绝对值与相对值中较大者。
#[test]
fn a_challenger_must_beat_the_larger_of_the_two_tolerances() {
    for (current, challenger, counts, why) in [
        (200, 160, false, "差 40，不到绝对容差 50"),
        (200, 140, true, "差 60，超过 max(50, 50)"),
        (200, 150, false, "差恰为容差：须严格更优"),
        (800, 700, false, "差 100，不到 800 的 25%"),
        (800, 599, true, "差 201，超过 800 的 25%"),
        (40, 28, false, "差 12，超过 25% 但不到绝对容差 50"),
        (40, 1, false, "当前出口已低于绝对容差：再快也不算明显更优"),
    ] {
        assert_eq!(clearly_better(challenger, current), counts, "{why}");
        let intent = auto();
        let list = [candidate("ch", challenger, 1, T0)];
        let mut memory = Memory::default();
        let decision = decide(
            &facts(&intent, fresh(current, &list[0]), &list),
            &mut memory,
        );
        if counts {
            assert_eq!(
                decision,
                Decision::Hold(Hold::Pending(Lacking::Streak)),
                "{why}"
            );
            assert_eq!(memory.streak().map(|streak| streak.wins), Some(1), "{why}");
        } else {
            assert_eq!(decision, Decision::Hold(Hold::Settled), "{why}");
            assert_eq!(memory.streak(), None, "{why}");
        }
    }
}

/// 一轮读数下的裁决：当前出口 300 毫秒，与 `list[0]` 同轮测得。
fn round(intent: &Intent, memory: &mut Memory, list: &[Candidate], now: u64) -> Decision {
    let mut input = facts(intent, fresh(300, &list[0]), list);
    input.now = now;
    decide(&input, memory)
}

/// 条目 8：当前出口连续两轮被明显胜过（运行号不同、相隔不少于 4 分钟）才换；间隔不足不计，
/// 运行号相同不计。间隔按入账的单调时刻算：用例里所有读数的墙钟时刻都相同。
#[test]
fn a_switch_needs_two_wins_from_distinct_runs_far_enough_apart() {
    let intent = auto();
    let first = [candidate("ch", 100, 1, T0)];

    // 运行号不同、相隔 5 分钟：换。
    let mut memory = Memory::default();
    assert_eq!(
        round(&intent, &mut memory, &first, T0),
        Decision::Hold(Hold::Pending(Lacking::Streak))
    );
    let later = [candidate("ch", 100, 2, T0 + 5 * MINUTE)];
    assert_eq!(
        round(&intent, &mut memory, &later, T0 + 5 * MINUTE),
        switch("ch", 100, Cause::Better)
    );

    // 相隔 3 分钟：不计，原因标成间隔不足；等满 4 分钟后的又一次测量才计。
    let mut memory = Memory::default();
    round(&intent, &mut memory, &first, T0);
    let too_soon = [candidate("ch", 100, 2, T0 + 3 * MINUTE)];
    assert_eq!(
        round(&intent, &mut memory, &too_soon, T0 + 3 * MINUTE),
        Decision::Hold(Hold::Pending(Lacking::Gap))
    );
    assert_eq!(memory.streak().map(|streak| streak.wins), Some(1));
    let enough = [candidate("ch", 100, 3, T0 + WIN_MIN_GAP_MS)];
    assert_eq!(
        round(&intent, &mut memory, &enough, T0 + WIN_MIN_GAP_MS),
        switch("ch", 100, Cause::Better)
    );

    // 运行号相同：同一次测量被反复评估（心跳拍复查）不重复计胜。
    let mut memory = Memory::default();
    for beat in 0..20 {
        assert_eq!(
            round(&intent, &mut memory, &first, T0 + beat * MINUTE),
            Decision::Hold(Hold::Pending(Lacking::Streak))
        );
    }
    assert_eq!(memory.streak().map(|streak| streak.wins), Some(1));
}

/// 连胜记的是当前出口被胜过的轮数，不是某个挑战者：两轮里领先的是不同的节点，照样凑满，
/// 目标取最近一轮里延迟最小的那个。中途有一轮没有候选明显胜过当前出口，连胜清零。
#[test]
fn the_streak_counts_rounds_the_exit_was_beaten_whoever_led() {
    let intent = auto();
    let mut memory = Memory::default();
    let first = [candidate("b", 100, 1, T0), candidate("c", 110, 1, T0)];
    assert_eq!(
        round(&intent, &mut memory, &first, T0),
        Decision::Hold(Hold::Pending(Lacking::Streak))
    );
    assert_eq!(memory.streak().map(|s| s.leader.as_str()), Some("b"));
    let second = [
        candidate("b", 110, 2, T0 + 30 * MINUTE),
        candidate("c", 100, 2, T0 + 30 * MINUTE),
    ];
    assert_eq!(
        round(&intent, &mut memory, &second, T0 + 30 * MINUTE),
        switch("c", 100, Cause::Better),
        "第二轮领先者换成了 c：连胜不清零，换到当轮最小者"
    );

    // 领先者中途有一轮不再明显更优：清零，从头数。
    let mut memory = Memory::default();
    round(&intent, &mut memory, &first, T0);
    let lapse = [
        candidate("b", 290, 2, T0 + 30 * MINUTE),
        candidate("c", 280, 2, T0 + 30 * MINUTE),
    ];
    assert_eq!(
        round(&intent, &mut memory, &lapse, T0 + 30 * MINUTE),
        Decision::Hold(Hold::Settled)
    );
    assert_eq!(memory.streak(), None);
    let third = [
        candidate("b", 100, 3, T0 + 60 * MINUTE),
        candidate("c", 110, 3, T0 + 60 * MINUTE),
    ];
    assert_eq!(
        round(&intent, &mut memory, &third, T0 + 60 * MINUTE),
        Decision::Hold(Hold::Pending(Lacking::Streak)),
        "清零之后的这一轮只是第一胜"
    );
}

/// 计胜时两个读数要可比：同一轮测得，或入账时刻相差小于计胜间隔的一半。当前出口的读数是更早
/// 一轮的（只重测了候选）不计胜也不清连胜，如实说读数没法比。
#[test]
fn a_win_needs_the_two_readings_to_be_comparable() {
    let intent = auto();
    let decide_with = |memory: &mut Memory, current: Current, list: &[Candidate]| {
        let mut input = facts(&intent, current, list);
        input.now = list[0].measured_mono;
        decide(&input, memory)
    };
    let stale_current = Current::Fresh {
        latency_ms: 300,
        run: 1,
        measured_mono: T0,
    };
    // 当前出口只在第 1 轮测过；候选此后被单独重测了两次，相隔够远。
    let mut memory = Memory::default();
    let same_round = [candidate("ch", 100, 1, T0)];
    assert_eq!(
        decide_with(&mut memory, stale_current, &same_round),
        Decision::Hold(Hold::Pending(Lacking::Streak)),
        "同一轮：可比，第一胜"
    );
    for (run, later) in [(2, 5 * MINUTE), (3, 10 * MINUTE), (4, 60 * MINUTE)] {
        let retested = [candidate("ch", 100, run, T0 + later)];
        assert_eq!(
            decide_with(&mut memory, stale_current, &retested),
            Decision::Hold(Hold::Pending(Lacking::Pair)),
            "当前出口的读数停在第 1 轮"
        );
        assert_eq!(memory.streak().map(|streak| streak.wins), Some(1));
    }
    // 不同轮但入账时刻足够近：可比。恰在边界上不可比。
    let near = Current::Fresh {
        latency_ms: 300,
        run: 9,
        measured_mono: T0 + 5 * MINUTE + PAIR_MAX_SKEW_MS - 1,
    };
    let retested = [candidate("ch", 100, 2, T0 + 5 * MINUTE)];
    assert_eq!(
        decide_with(&mut memory, near, &retested),
        switch("ch", 100, Cause::Better)
    );
    let edge = Current::Fresh {
        latency_ms: 300,
        run: 9,
        measured_mono: T0 + PAIR_MAX_SKEW_MS,
    };
    assert_eq!(
        decide_with(&mut Memory::default(), edge, &same_round),
        Decision::Hold(Hold::Pending(Lacking::Pair))
    );
}

/// 连胜已满的记忆：当前出口 `cur` 已连续两轮被明显胜过。返回记忆与第二轮的读数。
fn two_wins(intent: &Intent) -> (Memory, [Candidate; 1]) {
    let mut memory = Memory::default();
    round(
        intent,
        &mut memory,
        &[candidate("ch", 100, 1, T0 - 5 * MINUTE)],
        T0 - 5 * MINUTE,
    );
    let list = [candidate("ch", 100, 2, T0)];
    (memory, list)
}

/// 条目 9：驻留。上次换点在 9 分钟前不换，11 分钟前换；任何一次换点（含故障腿）都起驻留。
#[test]
fn nothing_better_is_taken_within_the_dwell_after_any_switch() {
    let intent = auto();
    for cause in [Cause::Failover, Cause::FirstPlacement, Cause::Better] {
        let mut memory = Memory::default();
        memory.record_switch(cause, T0 - 9 * MINUTE);
        round(
            &intent,
            &mut memory,
            &[candidate("ch", 100, 1, T0 - 5 * MINUTE)],
            T0 - 5 * MINUTE,
        );
        let list = [candidate("ch", 100, 2, T0)];
        assert_eq!(
            round(&intent, &mut memory, &list, T0),
            Decision::Hold(Hold::Pending(Lacking::Dwell)),
            "{cause:?}"
        );
        assert_eq!(
            round(&intent, &mut memory, &list, T0 + 2 * MINUTE),
            switch("ch", 100, Cause::Better),
            "{cause:?}：11 分钟后驻留已满"
        );
    }
}

/// 条目 10：限频。窗口内已有 3 次因「更优」的换点 → 不换，结论为已限频；最早一次滑出窗口后恢复。
/// 故障腿与首次落点不占这个额度。
#[test]
fn better_switches_are_rate_limited_per_window() {
    let intent = auto();
    let mut memory = Memory::default();
    for minutes_ago in [58, 40, 20] {
        memory.record_switch(Cause::Better, T0 - minutes_ago * MINUTE);
    }
    memory.record_switch(Cause::Failover, T0 - 15 * MINUTE);
    memory.record_switch(Cause::FirstPlacement, T0 - 12 * MINUTE);
    assert_eq!(memory.better_switches_in_window(T0), 3);
    round(
        &intent,
        &mut memory,
        &[candidate("ch", 100, 1, T0 - 5 * MINUTE)],
        T0 - 5 * MINUTE,
    );
    let list = [candidate("ch", 100, 2, T0)];
    assert_eq!(
        round(&intent, &mut memory, &list, T0),
        Decision::Hold(Hold::Pending(Lacking::RateLimit))
    );
    // 58 分钟前的那一次在 2 分钟后滑出窗口。
    assert_eq!(
        round(&intent, &mut memory, &list, T0 + 2 * MINUTE + 1),
        switch("ch", 100, Cause::Better)
    );
}

/// 冷却由两条腿共用：最近一次换点尝试（成败都算）60 秒内，哪条规则都不换。
#[test]
fn the_shared_cooldown_holds_every_rule() {
    let intent = auto();
    let best = [candidate("a", 80, 2, T0)];
    for current in [Current::Outside, Current::Failed { consecutive: 2 }] {
        let mut cooling = facts(&intent, current, &best);
        cooling.last_attempt_at = Some(T0 - SWITCH_COOLDOWN_MS + 1);
        assert_eq!(
            decide(&cooling, &mut Memory::default()),
            Decision::Hold(Hold::Cooldown)
        );
        cooling.last_attempt_at = Some(T0 - SWITCH_COOLDOWN_MS);
        assert!(matches!(
            decide(&cooling, &mut Memory::default()),
            Decision::Switch { .. }
        ));
    }
    let (mut memory, list) = two_wins(&intent);
    let mut cooling = facts(&intent, fresh(300, &list[0]), &list);
    cooling.last_attempt_at = Some(T0 - 1_000);
    assert_eq!(
        decide(&cooling, &mut memory),
        Decision::Hold(Hold::Pending(Lacking::Cooldown))
    );
}

/// 条目 11：当前出口换人、有效范围（核世代、网络代次、前台代次、订阅）任一变化，连胜清零。
#[test]
fn the_streak_resets_when_the_exit_or_the_epoch_changes() {
    let intent = auto();
    let epoch = Epoch {
        subscription: "sub".to_string(),
        generation: 1,
        network_epoch: Some(1),
        foreground_epoch: Some(1),
    };
    let armed = || {
        let mut memory = Memory::default();
        memory.observe_epoch(&epoch, 0);
        round(
            &intent,
            &mut memory,
            &[candidate("ch", 100, 1, T0 - 5 * MINUTE)],
            T0 - 5 * MINUTE,
        );
        assert_eq!(memory.streak().map(|streak| streak.wins), Some(1));
        memory
    };

    // 当前出口换人：从头算。
    let mut memory = armed();
    let same = [candidate("ch", 100, 2, T0)];
    let mut moved = facts(&intent, fresh(300, &same[0]), &same);
    moved.current_id = Some("elsewhere");
    assert_eq!(
        decide(&moved, &mut memory),
        Decision::Hold(Hold::Pending(Lacking::Streak))
    );

    // 有效范围的四个分量各变一次。
    for changed in [
        Epoch {
            generation: 2,
            ..epoch.clone()
        },
        Epoch {
            network_epoch: Some(2),
            ..epoch.clone()
        },
        Epoch {
            foreground_epoch: Some(2),
            ..epoch.clone()
        },
        Epoch {
            subscription: "another".to_string(),
            ..epoch.clone()
        },
    ] {
        let mut memory = armed();
        memory.observe_epoch(&epoch, 5);
        assert!(memory.streak().is_some(), "范围没变不清");
        memory.observe_epoch(&changed, 5);
        assert_eq!(memory.streak(), None, "{changed:?}");
    }
}

/// 条目 12：当前出口最新为失败。连续 1 次不换；连续 2 次且有候选即换（不要求连胜与驻留）；
/// 连续 2 次而无候选不换。
#[test]
fn a_failing_exit_is_left_after_two_consecutive_failures() {
    let intent = auto();
    let best = [candidate("slow", 900, 1, T0), candidate("ok", 400, 1, T0)];
    assert_eq!(
        decide(
            &facts(&intent, Current::Failed { consecutive: 1 }, &best),
            &mut Memory::default()
        ),
        Decision::Hold(Hold::CurrentFailedOnce)
    );
    let mut memory = Memory::default();
    memory.record_switch(Cause::Better, T0 - 2 * MINUTE);
    assert_eq!(
        decide(
            &facts(&intent, Current::Failed { consecutive: 2 }, &best),
            &mut memory
        ),
        switch("ok", 400, Cause::CurrentFailed),
        "驻留未满也换：当前节点已经不通"
    );
    assert_eq!(
        decide(
            &facts(&intent, Current::Failed { consecutive: 2 }, &[]),
            &mut Memory::default()
        ),
        Decision::Hold(Hold::NoCandidates)
    );
}

/// 条目 13：当前出口过期、未测、未纳入、读回不符 → 后台不换，结论各带原因。没有数据不等于差。
/// 周期测速此刻没在供数时，原因是它被什么挡住了。
#[test]
fn an_exit_without_a_fresh_result_is_never_left_in_the_background() {
    let intent = auto();
    let far_better = [candidate("a", 1, 2, T0)];
    for (reason, text) in NO_RESULTS.into_iter().zip([
        "currentExpired",
        "currentUnmeasured",
        "currentSkipped",
        "currentMismatch",
    ]) {
        let hold = Hold::CurrentNoResult(reason);
        assert_eq!(
            decide(
                &facts(&intent, Current::NoResult(reason), &far_better),
                &mut Memory::default()
            ),
            Decision::Hold(hold)
        );
        assert_eq!(hold.as_str(), text);
        for (starved, text) in [
            (Starved::PeriodicDisabled, "periodicDisabled"),
            (Starved::MeteredPaused, "meteredPaused"),
            (Starved::PowerSave, "powerSave"),
        ] {
            let mut blocked = facts(&intent, Current::NoResult(reason), &far_better);
            blocked.starved = Some(starved);
            let hold = Hold::Starved(starved);
            assert_eq!(
                decide(&blocked, &mut Memory::default()),
                Decision::Hold(hold)
            );
            assert_eq!(hold.as_str(), text);
        }
    }
    // 当前出口有新鲜值时，周期测速停着不改变裁决：有数据就照常判。
    let mut still_fresh = facts(&intent, fresh(100, &far_better[0]), &[]);
    still_fresh.starved = Some(Starved::PeriodicDisabled);
    assert_eq!(
        decide(&still_fresh, &mut Memory::default()),
        Decision::Hold(Hold::Settled)
    );
}

/// 条目 36：择优腿开关关着时，「当前节点测速失败」与「更优」都不换，首次落点仍换。
/// 总开关关着时整条腿不评估。
#[test]
fn the_two_fallback_switches_cut_what_they_say() {
    let intent = auto();
    let best = [candidate("a", 80, 2, T0)];
    let no_better_leg = |current| {
        let mut input = facts(&intent, current, &best);
        input.switches.better_leg = false;
        input
    };
    assert_eq!(
        decide(&no_better_leg(Current::Outside), &mut Memory::default()),
        switch("a", 80, Cause::FirstPlacement)
    );
    assert_eq!(
        decide(
            &no_better_leg(Current::Failed { consecutive: 5 }),
            &mut Memory::default()
        ),
        Decision::Hold(Hold::BetterLegOff)
    );
    let (mut memory, list) = two_wins(&intent);
    let mut input = facts(&intent, fresh(300, &list[0]), &list);
    input.switches.better_leg = false;
    assert_eq!(
        decide(&input, &mut memory),
        Decision::Hold(Hold::BetterLegOff)
    );

    let mut switched_off = facts(&intent, Current::Outside, &best);
    switched_off.switches.master = false;
    assert_eq!(
        decide(&switched_off, &mut Memory::default()),
        Decision::NotEvaluated(Gate::SwitchedOff)
    );
    // 生产取值：两个都开。
    const { assert!(AUTO_SELECT_ENABLED && BETTER_LEG_ENABLED) };
}

// ── 显式路径：与后台同一个裁决 ────────────────────────────────────────────────

/// 显式路径往哪换与后台是同一套判断：当前出口有新鲜值且是订阅里最快的，裁决是不换 —— 候选再
/// 多也不会把出口换到更慢的节点上，连着问多少次都一样。只比当前出口略快、或只明显更优了一轮的，
/// 同样不换。
#[test]
fn the_explicit_path_never_moves_a_fresh_exit_to_a_node_that_is_not_better() {
    let intent = auto();
    let slower = [
        candidate("second", 300, 1, T0),
        candidate("third", 500, 1, T0),
    ];
    let mut memory = Memory::default();
    for _ in 0..3 {
        let mut input = facts(&intent, fresh(40, &slower[0]), &slower);
        input.explicit = true;
        assert_eq!(decide(&input, &mut memory), Decision::Hold(Hold::Settled));
    }
    let slightly = [candidate("near", 180, 1, T0)];
    let mut input = facts(&intent, fresh(200, &slightly[0]), &slightly);
    input.explicit = true;
    assert_eq!(
        decide(&input, &mut Memory::default()),
        Decision::Hold(Hold::Settled)
    );
    let once = [candidate("ch", 100, 1, T0)];
    let mut input = facts(&intent, fresh(300, &once[0]), &once);
    input.explicit = true;
    assert_eq!(
        decide(&input, &mut Memory::default()),
        Decision::Hold(Hold::Pending(Lacking::Streak)),
        "只胜了一轮：显式路径也不换"
    );
}

/// 显式路径给出落点的四种情形：当前出口不在订阅内（首次落点）；当前出口测速失败（一次即可）；
/// 当前出口没有新鲜结果（后台不换，显式可换）；当前出口已连续两轮被明显胜过。冷却不拦它；
/// 没有候选时如实说没有。
#[test]
fn the_explicit_path_switches_exactly_where_the_decision_has_a_target() {
    let intent = auto();
    let best = [candidate("a", 80, 2, T0), candidate("b", 90, 2, T0)];
    let explicit = |current, candidates: &'static [Candidate]| {
        let mut input = facts(&intent, current, candidates);
        input.explicit = true;
        input.last_attempt_at = Some(T0 - 1);
        input.first_round_done = false;
        input.measuring = true;
        input
    };
    let best: &'static [Candidate] = Box::leak(Box::new(best));
    assert_eq!(
        decide(&explicit(Current::Outside, best), &mut Memory::default()),
        switch("a", 80, Cause::FirstPlacement)
    );
    assert_eq!(
        decide(
            &explicit(Current::Failed { consecutive: 1 }, best),
            &mut Memory::default()
        ),
        switch("a", 80, Cause::CurrentFailed)
    );
    for reason in NO_RESULTS {
        assert_eq!(
            decide(
                &explicit(Current::NoResult(reason), best),
                &mut Memory::default()
            ),
            switch("a", 80, Cause::Requested)
        );
        assert_eq!(
            decide(
                &explicit(Current::NoResult(reason), &[]),
                &mut Memory::default()
            ),
            Decision::Hold(Hold::NoCandidates)
        );
    }
    let (mut memory, list) = two_wins(&intent);
    let mut input = facts(&intent, fresh(300, &list[0]), &list);
    input.explicit = true;
    input.last_attempt_at = Some(T0 - 1);
    assert_eq!(
        decide(&input, &mut memory),
        switch("ch", 100, Cause::Better)
    );
}

// ── 提交失败的重试间隔 ────────────────────────────────────────────────────────

/// 条目 33 的前半：换点没换成不记换点，连胜保留。重试间隔从冷却时长起、每连续失败一次翻倍，
/// 到上界封顶；让位不计；换成一次即归零。
#[test]
fn failed_commits_back_off_with_a_bound_and_keep_the_streak() {
    assert_eq!(retry_after_ms(0), SWITCH_COOLDOWN_MS);
    assert_eq!(retry_after_ms(1), 2 * SWITCH_COOLDOWN_MS);
    assert_eq!(retry_after_ms(4), 16 * SWITCH_COOLDOWN_MS);
    assert_eq!(retry_after_ms(5), COMMIT_BACKOFF_MAX_MS);
    assert_eq!(retry_after_ms(u32::MAX), COMMIT_BACKOFF_MAX_MS);

    let intent = auto();
    let (mut memory, list) = two_wins(&intent);
    assert_eq!(
        round(&intent, &mut memory, &list, T0),
        switch("ch", 100, Cause::Better)
    );
    // 提交失败：调用方记一次失败，并把尝试时刻记在共用的冷却起点上。
    let before = memory.clone();
    memory.record_commit_failure(CommitFailure::Yielded);
    assert_eq!(memory, before, "让位不计入连续失败");
    memory.record_commit_failure(CommitFailure::Failed);
    assert_eq!(memory.commit_failures(), 1);
    assert_eq!(memory.streak(), before.streak(), "连胜保留");
    assert_eq!(
        memory.better_switches_in_window(T0),
        0,
        "没换成不占限频额度"
    );
    let at = |memory: &mut Memory, elapsed: u64| {
        let mut input = facts(&intent, fresh(300, &list[0]), &list);
        input.last_attempt_at = Some(T0);
        input.now = T0 + elapsed;
        decide(&input, memory)
    };
    let waiting = Decision::Hold(Hold::Pending(Lacking::Cooldown));
    assert_eq!(
        at(&mut memory, SWITCH_COOLDOWN_MS),
        waiting,
        "失败一次：间隔翻倍"
    );
    assert_eq!(at(&mut memory, 2 * SWITCH_COOLDOWN_MS - 1), waiting);
    assert_eq!(
        at(&mut memory, 2 * SWITCH_COOLDOWN_MS),
        switch("ch", 100, Cause::Better)
    );
    for _ in 0..20 {
        memory.record_commit_failure(CommitFailure::NotEligible);
    }
    assert_eq!(at(&mut memory, COMMIT_BACKOFF_MAX_MS - 1), waiting);
    assert_eq!(
        at(&mut memory, COMMIT_BACKOFF_MAX_MS),
        switch("ch", 100, Cause::Better),
        "失败再多，间隔也不超过上界"
    );
    memory.record_switch(Cause::Better, T0);
    assert_eq!(memory.commit_failures(), 0);
}

// ── 被故障腿换走的节点 ────────────────────────────────────────────────────────

/// 被故障腿换走的节点在排除期内不作为「更优」的挑战者：它测速再好也不计胜，轮到的是下一个
/// 未被排除的候选。排除期 = 周期的 4 倍、不短于一个限频窗口；到期后恢复。首次落点与当前节点
/// 已失败时别无选择，仍可落到它上面。
#[test]
fn a_node_the_failover_leg_just_left_is_not_a_challenger_for_a_while() {
    let intent = auto();
    let period = 30 * MINUTE;
    let mut memory = Memory::default();
    memory.bar("flaky", T0, period);
    let length = period * FAILOVER_BAR_PERIODS;
    assert_eq!(memory.barred(T0), [("flaky", length)]);
    assert!(memory.is_barred("flaky", T0 + length - 1));
    assert!(!memory.is_barred("flaky", T0 + length));
    let mut short = Memory::default();
    short.bar("flaky", T0, 5 * MINUTE);
    assert_eq!(
        short.barred(T0),
        [("flaky", FAILOVER_BAR_MIN_MS)],
        "短周期下不短于一个限频窗口"
    );

    let only_flaky = [candidate("flaky", 50, 1, T0)];
    assert_eq!(
        round(&intent, &mut memory, &only_flaky, T0),
        Decision::Hold(Hold::Settled),
        "只有它明显更优：不计胜"
    );
    assert_eq!(memory.streak(), None);
    let with_other = [
        candidate("flaky", 50, 1, T0),
        candidate("steady", 120, 1, T0),
    ];
    assert_eq!(
        round(&intent, &mut memory, &with_other, T0),
        Decision::Hold(Hold::Pending(Lacking::Streak))
    );
    assert_eq!(memory.streak().map(|s| s.leader.as_str()), Some("steady"));

    // 别无选择时仍可落到它上面；有别的候选时优先别的。
    for current in [Current::Outside, Current::Failed { consecutive: 2 }] {
        assert!(matches!(
            decide(&facts(&intent, current, &only_flaky), &mut memory.clone()),
            Decision::Switch { ref target, .. } if target == "flaky"
        ));
        assert!(matches!(
            decide(&facts(&intent, current, &with_other), &mut memory.clone()),
            Decision::Switch { ref target, .. } if target == "steady"
        ));
    }

    // 排除随网络与订阅清，不随核世代清。
    let epoch = |generation, network| Epoch {
        subscription: "sub".to_string(),
        generation,
        network_epoch: Some(network),
        foreground_epoch: None,
    };
    let mut memory = Memory::default();
    memory.observe_epoch(&epoch(1, 1), 0);
    memory.bar("flaky", T0, period);
    memory.observe_epoch(&epoch(2, 1), 0);
    assert!(
        memory.is_barred("flaky", T0),
        "重启内核不会让不通的节点变通"
    );
    memory.observe_epoch(&epoch(2, 2), 0);
    assert!(!memory.is_barred("flaky", T0), "换了网络值得重新看");
}

// ── 账本读数到裁决输入 ────────────────────────────────────────────────────────

const TIMED_OUT: MeasureFailure = MeasureFailure::new(FailPhase::Measure, FailKind::Timeout);

fn failed(consecutive: u32) -> Exclusion {
    Exclusion::Failed {
        failure: TIMED_OUT,
        consecutive,
    }
}

fn ledger(
    selectable: &[Candidate],
    excluded: &[(&str, Exclusion)],
    unverified: &[&str],
) -> Candidates {
    Candidates {
        selectable: selectable.to_vec(),
        excluded: excluded
            .iter()
            .map(|(id, exclusion)| ((*id).to_string(), exclusion.clone()))
            .collect(),
        unverified: unverified.iter().map(|id| (*id).to_string()).collect(),
    }
}

fn members(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_string()).collect()
}

/// 当前出口在账本里的状况：不在订阅内、有新鲜值（带它的运行号与入账时刻）、失败、以及四种
/// 「没有新鲜结果」。读回不符单列；非候选路径、未连接测量归未测。
#[test]
fn the_exit_is_classified_from_the_ledger_read() {
    let all = members(&["cur", "a"]);
    let read = |exclusion: Exclusion| ledger(&[], &[("cur", exclusion)], &[]);
    assert_eq!(
        classify_current(
            Some("cur"),
            &all,
            &ledger(&[candidate("cur", 70, 4, T0 + 9)], &[], &[])
        ),
        Current::Fresh {
            latency_ms: 70,
            run: 4,
            measured_mono: T0 + 9
        }
    );
    for (exclusion, expected) in [
        (failed(2), Current::Failed { consecutive: 2 }),
        (
            Exclusion::Stale(LedgerStale::Expired),
            Current::NoResult(NoResult::Expired),
        ),
        (
            Exclusion::Stale(LedgerStale::ForegroundEpoch),
            Current::NoResult(NoResult::Expired),
        ),
        (
            Exclusion::Skipped("dirty"),
            Current::NoResult(NoResult::Skipped),
        ),
        (
            Exclusion::NeverMeasured,
            Current::NoResult(NoResult::Unmeasured),
        ),
        (
            Exclusion::BindingMismatch,
            Current::NoResult(NoResult::Mismatch),
        ),
        (
            Exclusion::NonCandidatePath,
            Current::NoResult(NoResult::Unmeasured),
        ),
        (
            Exclusion::Disconnected,
            Current::NoResult(NoResult::Unmeasured),
        ),
    ] {
        assert_eq!(
            classify_current(Some("cur"), &all, &read(exclusion.clone())),
            expected,
            "{exclusion:?}"
        );
    }
    let empty = ledger(&[], &[], &[]);
    for outside in [
        Some("elsewhere"),
        Some("__direct__"),
        Some("__block__"),
        None,
    ] {
        assert_eq!(classify_current(outside, &all, &empty), Current::Outside);
    }
}

/// 条目 15：未被读回证实的可选点与证实的同权参与比较；摘要里的计数等于其个数；全部未证实时
/// 标记置位。
#[test]
fn unverified_candidates_compete_equally_and_are_counted() {
    let intent = auto();
    let selectable = [
        candidate("confirmed", 120, 1, T0),
        candidate("unverified", 80, 1, T0),
    ];
    let read = ledger(&selectable, &[], &["unverified"]);
    assert_eq!(
        decide(
            &facts(&intent, Current::Outside, &read.selectable),
            &mut Memory::default()
        ),
        switch("unverified", 80, Cause::FirstPlacement)
    );
    let counts = Counts::of(&read, 0);
    assert_eq!((counts.selectable, counts.unverified), (2, 1));
    assert!(!counts.all_unverified());
    assert!(
        Counts::of(&ledger(&selectable, &[], &["confirmed", "unverified"]), 0).all_unverified()
    );
    assert!(
        !Counts::of(&ledger(&[], &[], &[]), 0).all_unverified(),
        "没有可选点时不置位"
    );
}

/// 条目 16：失败与未测分属两类计数；把一个未测节点改成失败不改变裁决，除非它是当前出口。
#[test]
fn a_failure_is_negative_evidence_only_for_the_current_exit() {
    let intent = auto();
    let all = members(&["cur", "other", "good"]);
    let good = [candidate("cur", 100, 1, T0), candidate("good", 90, 1, T0)];
    let unmeasured = ledger(&good, &[("other", Exclusion::NeverMeasured)], &[]);
    let other_failed = ledger(&good, &[("other", failed(9))], &[]);
    let decide_on = |read: &Candidates| {
        let candidates: Vec<Candidate> = read
            .selectable
            .iter()
            .filter(|candidate| candidate.node_id != "cur")
            .cloned()
            .collect();
        let current = classify_current(Some("cur"), &all, read);
        decide(
            &facts(&intent, current, &candidates),
            &mut Memory::default(),
        )
    };
    assert_eq!(decide_on(&unmeasured), decide_on(&other_failed));
    assert_eq!(
        (
            Counts::of(&unmeasured, 0).unmeasured,
            Counts::of(&unmeasured, 0).failed
        ),
        (1, 0)
    );
    assert_eq!(
        (
            Counts::of(&other_failed, 0).unmeasured,
            Counts::of(&other_failed, 0).failed
        ),
        (0, 1)
    );
    // 同样的改动落在当前出口上：裁决变了。
    let good_only = [candidate("good", 90, 1, T0)];
    let current_unmeasured = ledger(&good_only, &[("cur", Exclusion::NeverMeasured)], &[]);
    let current_failed = ledger(&good_only, &[("cur", failed(2))], &[]);
    assert_eq!(
        decide_on(&current_unmeasured),
        Decision::Hold(Hold::CurrentNoResult(NoResult::Unmeasured))
    );
    assert_eq!(
        decide_on(&current_failed),
        switch("good", 90, Cause::CurrentFailed)
    );
}

/// 各类排除落进哪一个计数。
#[test]
fn counts_bucket_every_exclusion() {
    let read = ledger(
        &[candidate("ok", 50, 1, T0)],
        &[
            ("f", failed(1)),
            ("s1", Exclusion::Stale(LedgerStale::Expired)),
            ("k", Exclusion::Skipped("dirty")),
            ("n", Exclusion::NeverMeasured),
            ("p", Exclusion::NonCandidatePath),
            ("m", Exclusion::BindingMismatch),
            ("d", Exclusion::Disconnected),
        ],
        &[],
    );
    assert_eq!(
        Counts::of(&read, 4),
        Counts {
            selectable: 1,
            failed: 1,
            stale: 1,
            unmeasured: 4,
            skipped: 1,
            unverified: 0,
            needs_restart: 4,
        }
    );
}

// ── 首轮锁存 ──────────────────────────────────────────────────────────────────

/// 条目 21：两个判据都不成立时不算完成；任一成立即锁存，此后判据回落（一个成员的失败记录过期）
/// 不使闸门回落；有效范围变化后锁存清除，且进入新范围之前收尾的那一轮不算数。
#[test]
fn the_first_round_latches_and_clears_with_the_epoch() {
    let epoch = |generation| Epoch {
        subscription: "sub".to_string(),
        generation,
        network_epoch: None,
        foreground_epoch: None,
    };
    let mut memory = Memory::default();
    memory.observe_epoch(&epoch(1), 7);
    assert!(!memory.latch_first_round(false, None));
    assert!(
        !memory.latch_first_round(false, Some(7)),
        "序号 7 的那一轮在进入本范围之前就收尾了"
    );
    assert!(
        memory.latch_first_round(false, Some(8)),
        "调度器报告本范围内有一轮收尾"
    );
    assert!(
        memory.latch_first_round(false, None),
        "锁存：判据回落不回落"
    );

    memory.observe_epoch(&epoch(1), 9);
    assert!(memory.latch_first_round(false, None), "范围没变，锁存还在");
    memory.observe_epoch(&epoch(2), 9);
    assert!(
        !memory.latch_first_round(false, Some(9)),
        "换核世代：从头等"
    );
    assert!(memory.latch_first_round(true, None), "账本给出完整覆盖");
    assert!(memory.latch_first_round(false, None));

    memory.leave();
    memory.observe_epoch(&epoch(2), 9);
    assert!(
        !memory.latch_first_round(false, None),
        "意图离开过自动：从头等"
    );
}

// ── 回放 ──────────────────────────────────────────────────────────────────────

/// 一轮的读数。
struct Round {
    nodes: Vec<(&'static str, u32)>,
    /// 当前出口这一轮测速失败（连续失败次数由回放累计）。
    current_failed: bool,
    /// 这一轮收尾之后，连通性心跳判当前出口不通，故障腿把出口换到这个节点。
    failover_to: Option<&'static str>,
}

fn nodes(nodes: Vec<(&'static str, u32)>) -> Round {
    Round {
        nodes,
        current_failed: false,
        failover_to: None,
    }
}

/// 假时钟回放：每 `period` 一轮，一轮收尾时评估一次，其后每 30 秒按同一份读数复查（心跳拍），
/// 裁决为换点即视为提交成功。返回每次换点的 (轮次, 距开始的毫秒, 目标, 原因)。
fn replay(
    start: &'static str,
    period: u64,
    rounds: usize,
    round_of: &dyn Fn(usize, &str) -> Round,
) -> Vec<(usize, u64, String, Cause)> {
    let intent = auto();
    let mut memory = Memory::default();
    let mut current = start.to_string();
    let mut consecutive_failures = 0;
    let mut last_attempt_at = None;
    let mut switches = Vec::new();
    for index in 0..rounds {
        let round = round_of(index, &current);
        let measured = T0 + index as u64 * period;
        consecutive_failures = if round.current_failed {
            consecutive_failures + 1
        } else {
            0
        };
        let mut now = measured;
        while now < measured + period {
            let state = if round.current_failed {
                Current::Failed {
                    consecutive: consecutive_failures,
                }
            } else {
                round.nodes.iter().find(|(id, _)| *id == current).map_or(
                    Current::Outside,
                    |(_, latency_ms)| Current::Fresh {
                        latency_ms: *latency_ms,
                        run: index as u64 + 1,
                        measured_mono: measured,
                    },
                )
            };
            let candidates: Vec<Candidate> = round
                .nodes
                .iter()
                .filter(|(id, _)| *id != current)
                .map(|(id, latency_ms)| candidate(id, *latency_ms, index as u64 + 1, measured))
                .collect();
            let mut input = facts(&intent, state, &candidates);
            input.current_id = Some(&current);
            input.now = now;
            input.last_attempt_at = last_attempt_at;
            if let Decision::Switch { target, cause, .. } = decide(&input, &mut memory) {
                memory.record_switch(cause, now);
                last_attempt_at = Some(now);
                switches.push((index, now - T0, target.clone(), cause));
                current = target;
                consecutive_failures = 0;
            }
            // 故障腿在这一轮收尾后的第三个心跳拍换点（连通性连续 3 拍失败）。
            if let Some(to) = round
                .failover_to
                .filter(|_| now == measured + 3 * HEARTBEAT_INTERVAL_MS)
            {
                memory.bar(&current, now, period);
                memory.record_switch(Cause::Failover, now);
                last_attempt_at = Some(now);
                switches.push((index, now - T0, to.to_string(), Cause::Failover));
                current = to.to_string();
            }
            now += HEARTBEAT_INTERVAL_MS;
        }
    }
    switches
}

/// 条目 17：两个节点的延迟在容差内交替领先 48 轮，换点 0 次。
#[test]
fn replay_two_nodes_trading_the_lead_within_tolerance_never_switch() {
    let switches = replay("a", 30 * MINUTE, 48, &|index, _| {
        nodes(if index % 2 == 0 {
            vec![("a", 200), ("b", 170)]
        } else {
            vec![("a", 170), ("b", 200)]
        })
    });
    assert_eq!(switches, []);
}

/// 条目 18：一个节点从第 10 轮起持续比当前快 100 毫秒，恰好换 1 次，发生在第 11 轮收尾的那一刻。
#[test]
fn replay_a_steadily_better_node_is_taken_once_on_the_second_round() {
    let period = 30 * MINUTE;
    let switches = replay("a", period, 30, &|index, _| {
        // 轮次从 0 数：下标 9 是第 10 轮。
        nodes(vec![("a", 300), ("b", if index >= 9 { 200 } else { 290 })])
    });
    assert_eq!(
        switches,
        [(10, 10 * period, "b".to_string(), Cause::Better)]
    );
}

/// 多个延迟相近的快节点：当前 300 毫秒，b 与 c 逐轮交替领先。当前出口每一轮都被明显胜过，
/// 第二轮收尾时换到当轮最小者，此后不再换（b、c 之间的差在容差内）。
#[test]
fn replay_alternating_leaders_still_get_the_slow_exit_replaced_on_the_second_round() {
    let period = 30 * MINUTE;
    let switches = replay("a", period, 24, &|index, _| {
        nodes(if index % 2 == 0 {
            vec![("a", 300), ("b", 100), ("c", 110)]
        } else {
            vec![("a", 300), ("b", 110), ("c", 100)]
        })
    });
    assert_eq!(switches, [(1, period, "c".to_string(), Cause::Better)]);
}

/// 领先者中途有一轮不再明显更优：连胜清零，换点推迟到其后重新凑满的那一轮。
#[test]
fn replay_a_lapse_in_the_lead_restarts_the_streak() {
    let period = 30 * MINUTE;
    let switches = replay("a", period, 8, &|index, _| {
        nodes(vec![
            ("a", 300),
            // 第 1 轮明显更优，第 2 轮回到容差内，第 3、4 轮再次明显更优。
            ("b", if index == 1 { 290 } else { 100 }),
        ])
    });
    assert_eq!(switches, [(3, 3 * period, "b".to_string(), Cause::Better)]);
}

/// 条目 19：对抗轨迹跑 24 小时。每 5 分钟一轮（周期下限），每轮都有一个不同的节点大幅领先。
/// 任意连续 60 分钟内因「更优」的换点不超过 3 次，相邻两次至少隔一个驻留期；换点时刻钉死。
#[test]
fn replay_an_adversarial_trace_is_bounded_by_the_rate_limit_and_the_dwell() {
    let period = 5 * MINUTE;
    let rounds = (24 * 60 * MINUTE / period) as usize;
    let switches = replay("n0", period, rounds, &|_, current| {
        // 三个节点轮着大幅领先：领先者永远不是当前出口。
        let next = match current {
            "n0" => "n1",
            "n1" => "n2",
            _ => "n0",
        };
        nodes(
            ["n0", "n1", "n2"]
                .into_iter()
                .map(|id| (id, if id == next { 50 } else { 900 }))
                .collect(),
        )
    });
    assert!(switches
        .iter()
        .all(|(_, _, _, cause)| *cause == Cause::Better));
    let times: Vec<u64> = switches.iter().map(|(_, at, _, _)| *at).collect();
    for (index, at) in times.iter().enumerate() {
        let in_window = times[index..]
            .iter()
            .take_while(|later| **later - at < BETTER_RATE_WINDOW_MS)
            .count();
        assert!(
            in_window <= BETTER_RATE_MAX,
            "从 {at} 起的一小时内换了 {in_window} 次"
        );
    }
    assert!(times
        .windows(2)
        .all(|pair| pair[1] - pair[0] >= MIN_DWELL_MS));
    // 第一小时：第 2 轮收尾时凑满连胜即换，此后每隔一个驻留期一次，第 4 次被限频挡到窗口滑出。
    assert_eq!(
        &times[..4],
        [5 * MINUTE, 15 * MINUTE, 25 * MINUTE, 65 * MINUTE]
    );
    assert_eq!(
        switches.len(),
        72,
        "24 小时的硬上界是每小时 3 次；本轨迹每小时都顶到上界"
    );
}

/// 条目 20：当前出口每隔一轮测速失败一次，换点 0 次（连续失败从未达到 2）。
#[test]
fn replay_an_exit_failing_every_other_round_is_never_left() {
    let switches = replay("a", 30 * MINUTE, 48, &|index, _| Round {
        current_failed: index % 2 == 1,
        ..nodes(vec![("a", 200), ("b", 190)])
    });
    assert_eq!(switches, []);
}

/// 对照：当前出口连续两轮失败，第二轮收尾时换走，原因是当前节点测速失败。
#[test]
fn replay_an_exit_failing_twice_in_a_row_is_left_on_the_second_round() {
    let period = 30 * MINUTE;
    let switches = replay("a", period, 6, &|index, current| Round {
        current_failed: current == "a" && index >= 2,
        // 换走之后 `a` 仍然测不通：轨迹里它不再以可选点出现。
        ..nodes(if current == "a" {
            vec![("a", 200), ("b", 400)]
        } else {
            vec![("b", 400)]
        })
    });
    assert_eq!(
        switches,
        [(3, 3 * period, "b".to_string(), Cause::CurrentFailed)]
    );
}

/// 测速好、连通性差的节点：a 的测速延迟一直远好于 b，但出口一落到 a 上，连通性心跳就判它不通，
/// 故障腿换到 b。没有故障记忆时，择优腿两轮之后把出口换回 a，如此往复。有了它，a 在排除期内
/// 不作为挑战者：24 小时里故障腿只换了那一次，此后每个排除期（周期的 4 倍）至多被换回一次。
#[test]
fn replay_a_node_that_measures_well_but_fails_connectivity_does_not_loop() {
    let period = 30 * MINUTE;
    let rounds = 48;
    let switches = replay("a", period, rounds, &|_, current| Round {
        failover_to: (current == "a").then_some("b"),
        ..nodes(vec![("a", 60), ("b", 400)])
    });
    let back_to_a: Vec<u64> = switches
        .iter()
        .filter(|(_, _, target, cause)| target == "a" && *cause == Cause::Better)
        .map(|(_, at, _, _)| *at)
        .collect();
    let bar = period * FAILOVER_BAR_PERIODS;
    assert!(
        back_to_a.windows(2).all(|pair| pair[1] - pair[0] >= bar),
        "换回 a 的间隔不短于排除期：{back_to_a:?}"
    );
    assert!(
        back_to_a
            .iter()
            .all(|at| *at >= 3 * HEARTBEAT_INTERVAL_MS + bar),
        "第一次换回不早于排除期满"
    );
    // 24 小时 = 48 个周期，排除期 4 个周期（外加凑连胜的时间）：至多 12 次往返，实际更少。
    assert!(back_to_a.len() <= 9, "{}", back_to_a.len());
    assert!(!back_to_a.is_empty(), "排除有期限：到期后它仍会被再信一次");
}

// ── 对外状态 ──────────────────────────────────────────────────────────────────

fn inputs(intent: &Intent) -> StatusInputs<'_> {
    StatusInputs {
        intent,
        switches: ON,
        platform_open: true,
        core_running: true,
        core_ready_ago_ms: Some(60 * MINUTE),
        foreground: true,
        exit_id: Some("cur"),
        exit_in_subscription: true,
        basis: Some(Basis {
            state: "fresh",
            latency_ms: Some(120),
            measured_at: Some(WALL - MINUTE),
            binding: Some("confirmed"),
        }),
        period_ms: 30 * MINUTE,
        data_source: DataSource::default(),
        now: T0,
    }
}

fn evaluation(decision: Decision, at_mono: u64) -> Evaluation {
    Evaluation {
        at: WALL,
        at_mono,
        subscription: "sub".to_string(),
        decision,
        counts: Counts::default(),
    }
}

fn evaluated(decision: Decision, seconds_ago: u64) -> Slot {
    let mut slot = Slot::default();
    slot.record_evaluation(evaluation(decision, T0 - seconds_ago * 1_000), false);
    slot
}

fn switch_record(leg: Leg, cause: Cause) -> SwitchRecord {
    SwitchRecord {
        at: WALL - 9 * MINUTE,
        leg,
        cause,
        from_id: Some("old".to_string()),
        from_name: Some("Old".to_string()),
        to_id: "cur".to_string(),
        to_name: "Cur".to_string(),
        from_latency_ms: Some(300),
        to_latency_ms: Some(120),
        unverified: Some(false),
    }
}

/// 条目 35：载荷的形状。字段齐全；手动意图下除意图与实际出口外全部为空或零值，槽里留着的
/// 上次换点、评估、排除与计数不漏出来。
#[test]
fn the_status_payload_has_every_field_and_is_blank_under_a_manual_intent() {
    let mut slot = evaluated(Decision::Hold(Hold::Pending(Lacking::Dwell)), 5);
    slot.record_switch(switch_record(Leg::Select, Cause::Better), T0 - 9 * MINUTE);
    slot.memory.bar("flaky", T0, 30 * MINUTE);
    let intent = auto();
    let leader = [candidate("ch", 100, 1, T0)];
    decide(
        &facts(&intent, fresh(300, &leader[0]), &leader),
        &mut slot.memory,
    );

    let auto_status = serde_json::to_value(project_status(&inputs(&intent), &slot)).unwrap();
    let mut keys: Vec<&str> = auto_status
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "barred",
            "basis",
            "challenger",
            "commit",
            "counters",
            "dataSource",
            "exit",
            "flags",
            "intent",
            "lastEvaluation",
            "lastSwitch",
            "mode",
            "noDataForMs",
            "reason",
        ]
    );
    assert_eq!(
        auto_status["intent"],
        json!({ "mode": "auto", "scope": "subscription", "subscriptionId": "sub" })
    );
    assert_eq!(
        auto_status["exit"],
        json!({ "serverId": "cur", "inSubscription": true })
    );
    assert_eq!(auto_status["mode"], "settled");
    assert_eq!(auto_status["reason"], "challengerPending");
    assert_eq!(
        auto_status["challenger"],
        json!({ "serverId": "ch", "wins": 1, "winsRequired": 2, "lacking": "dwell" })
    );
    assert_eq!(
        auto_status["barred"],
        json!([{ "serverId": "flaky", "remainingMs": 120 * MINUTE }])
    );
    assert_eq!(
        auto_status["commit"],
        json!({ "consecutiveFailures": 0, "retryAfterMs": null, "lastFailure": null })
    );
    assert_eq!(auto_status["noDataForMs"], Value::Null);
    assert_eq!(
        auto_status["lastSwitch"],
        json!({
            "at": WALL - 9 * MINUTE, "leg": "select", "cause": "better",
            "fromId": "old", "fromName": "Old", "toId": "cur", "toName": "Cur",
            "fromLatencyMs": 300, "toLatencyMs": 120, "unverified": false,
        })
    );
    assert_eq!(auto_status["lastEvaluation"]["outcome"], "held");
    assert_eq!(auto_status["lastEvaluation"]["at"], WALL);
    assert_eq!(
        auto_status["lastEvaluation"]["counts"],
        json!({
            "selectable": 0, "failed": 0, "stale": 0, "unmeasured": 0, "skipped": 0,
            "unverified": 0, "needsRestart": 0,
        })
    );
    assert_eq!(
        auto_status["counters"],
        json!({
            "failoverSwitches": 0, "selectSwitches": 1,
            "betterSwitchesInWindow": 1, "betterSwitchesMax": 3,
        })
    );
    assert_eq!(
        auto_status["flags"],
        json!({
            "stalled": false, "starved": false, "exitOutsideSubscription": false,
            "exitOutsideOverdue": false, "allUnverified": false,
        })
    );
    assert_eq!(
        auto_status["dataSource"],
        json!({ "blockedBy": null, "lastSkip": null })
    );
    assert_eq!(auto_status["basis"]["binding"], "confirmed");

    let blank = |intent_mode: &str, mode: &str, reason: Value| {
        json!({
            "intent": { "mode": intent_mode, "scope": null, "subscriptionId": null },
            "exit": { "serverId": "cur", "inSubscription": null },
            "mode": mode,
            "reason": reason,
            "basis": null,
            "lastSwitch": null,
            "lastEvaluation": null,
            "challenger": null,
            "barred": [],
            "commit": { "consecutiveFailures": 0, "retryAfterMs": null, "lastFailure": null },
            "noDataForMs": null,
            "flags": {
                "stalled": false, "starved": false, "exitOutsideSubscription": false,
                "exitOutsideOverdue": false, "allUnverified": false,
            },
            "counters": {
                "failoverSwitches": 0, "selectSwitches": 0,
                "betterSwitchesInWindow": 0, "betterSwitchesMax": 0,
            },
            "dataSource": { "blockedBy": null, "lastSkip": null },
        })
    };
    let manual = Intent::Manual;
    assert_eq!(
        serde_json::to_value(project_status(&inputs(&manual), &slot)).unwrap(),
        blank("manual", "manual", Value::Null)
    );
    // 本版本不认识的意图：如实说不认识，其余同样留空。
    let unrecognized = Intent::Unrecognized;
    assert_eq!(
        serde_json::to_value(project_status(&inputs(&unrecognized), &slot)).unwrap(),
        blank("unrecognized", "notEvaluated", json!("intentUnrecognized"))
    );
}

/// 每一种「显示自动选择但实际没在选」的原因都能从状态里读出：模式与原因的映射。
#[test]
fn every_reason_auto_selection_is_not_selecting_is_readable_from_the_status() {
    let intent = auto();
    let cases: Vec<(Decision, &str, Option<&str>)> = vec![
        (
            Decision::NotEvaluated(Gate::WaitingFirstRound),
            "waitingFirstRound",
            None,
        ),
        (
            Decision::NotEvaluated(Gate::PeriodicDisabled),
            "notEvaluated",
            Some("periodicDisabled"),
        ),
        (
            Decision::NotEvaluated(Gate::MeteredPaused),
            "notEvaluated",
            Some("meteredPaused"),
        ),
        (
            Decision::NotEvaluated(Gate::PowerSave),
            "notEvaluated",
            Some("powerSave"),
        ),
        (
            Decision::NotEvaluated(Gate::Background),
            "notEvaluated",
            Some("background"),
        ),
        (
            Decision::NotEvaluated(Gate::Reconciling),
            "notEvaluated",
            Some("reconciling"),
        ),
        (
            Decision::NotEvaluated(Gate::ConfigPending),
            "notEvaluated",
            Some("configPending"),
        ),
        (
            Decision::NotEvaluated(Gate::Measuring),
            "notEvaluated",
            Some("measuring"),
        ),
        (
            Decision::NotEvaluated(Gate::SubscriptionEmpty),
            "notEvaluated",
            Some("subscriptionEmpty"),
        ),
        (Decision::Hold(Hold::NoCandidates), "noCandidates", None),
        (Decision::Hold(Hold::NeedsRestart), "needsRestart", None),
        (
            Decision::Hold(Hold::Pending(Lacking::RateLimit)),
            "rateLimited",
            None,
        ),
        (Decision::Hold(Hold::Settled), "settled", Some("settled")),
        (
            Decision::Hold(Hold::Pending(Lacking::Streak)),
            "settled",
            Some("challengerPending"),
        ),
        (
            Decision::Hold(Hold::CurrentFailedOnce),
            "settled",
            Some("currentFailedOnce"),
        ),
        (
            Decision::Hold(Hold::CurrentNoResult(NoResult::Expired)),
            "noData",
            Some("currentExpired"),
        ),
        (
            Decision::Hold(Hold::CurrentNoResult(NoResult::Unmeasured)),
            "noData",
            Some("currentUnmeasured"),
        ),
        (
            Decision::Hold(Hold::CurrentNoResult(NoResult::Skipped)),
            "noData",
            Some("currentSkipped"),
        ),
        (
            Decision::Hold(Hold::CurrentNoResult(NoResult::Mismatch)),
            "noData",
            Some("currentMismatch"),
        ),
        (
            Decision::Hold(Hold::Starved(Starved::PeriodicDisabled)),
            "noData",
            Some("periodicDisabled"),
        ),
        (
            Decision::Hold(Hold::Starved(Starved::MeteredPaused)),
            "noData",
            Some("meteredPaused"),
        ),
        (
            Decision::Hold(Hold::Starved(Starved::PowerSave)),
            "noData",
            Some("powerSave"),
        ),
        (Decision::Hold(Hold::Cooldown), "settled", Some("cooldown")),
        (
            Decision::Hold(Hold::BetterLegOff),
            "settled",
            Some("betterLegOff"),
        ),
        (
            Decision::Hold(Hold::Committing),
            "settled",
            Some("committing"),
        ),
        (
            Decision::Hold(Hold::CommitFailed(CommitFailure::Failed)),
            "commitFailed",
            Some("commitFailed"),
        ),
        (
            Decision::Hold(Hold::CommitFailed(CommitFailure::NotEligible)),
            "commitFailed",
            Some("commitNotEligible"),
        ),
        (
            Decision::Hold(Hold::CommitFailed(CommitFailure::Reconciling)),
            "commitFailed",
            Some("commitReconciling"),
        ),
        (
            Decision::Hold(Hold::CommitFailed(CommitFailure::Yielded)),
            "commitFailed",
            Some("commitYielded"),
        ),
        (switch("x", 1, Cause::Better), "settled", Some("better")),
        (
            switch("x", 1, Cause::FirstPlacement),
            "settled",
            Some("firstPlacement"),
        ),
    ];
    for (decision, mode, reason) in cases {
        let status = project_status(&inputs(&intent), &evaluated(decision.clone(), 5));
        assert_eq!((status.mode, status.reason), (mode, reason), "{decision:?}");
    }

    // 读的那一刻就能判的三道闸门盖过槽里留着的旧裁决：核停了之后评估任务已经不在。
    let settled = evaluated(Decision::Hold(Hold::Settled), 5);
    type Close = fn(&mut StatusInputs<'_>);
    let live: [(&str, Close); 3] = [
        ("switchedOff", |i| i.switches.master = false),
        ("platformNotOpen", |i| i.platform_open = false),
        ("coreNotRunning", |i| i.core_running = false),
    ];
    for (reason, close) in live {
        let mut closed = inputs(&intent);
        close(&mut closed);
        let status = project_status(&closed, &settled);
        assert_eq!((status.mode, status.reason), ("notEvaluated", Some(reason)));
        assert_eq!(status.challenger, None);
        assert!(!status.flags.stalled, "没在评估是有原因的，不算停滞");
    }

    // 意图刚写入、还没有轮到一次评估；别的订阅留下的评估不算数。
    let armed = Slot {
        intent_set_mono: Some(T0 - 1_000),
        ..Slot::default()
    };
    let status = project_status(&inputs(&intent), &armed);
    assert_eq!(
        (status.mode, status.reason),
        ("notEvaluated", Some("notYetEvaluated"))
    );
    let mut other = evaluated(Decision::Hold(Hold::Settled), 5);
    other.evaluation.as_mut().unwrap().subscription = "another".to_string();
    let status = project_status(&inputs(&intent), &other);
    assert_eq!(status.reason, Some("notYetEvaluated"));
    assert_eq!(status.last_evaluation, None);
}

/// 首轮锁存之后数据源停掉、或当前出口长期读回不符：状态是「没有数据」而不是「已选定」，原因
/// 说得出是被什么挡住；持续的时长读得到，超过三个周期后「长期无数据」置位。逐一覆盖周期测速
/// 总开关被关、计费暂停、省电暂停、读回长期不符，以及过期、未测、未纳入。
#[test]
fn losing_the_data_after_the_first_round_is_reported_as_no_data_and_timed() {
    let intent = auto();
    let period = 30 * MINUTE;
    let far_better = [candidate("a", 1, 2, T0)];
    let mut cases: Vec<(Option<Starved>, NoResult, &str)> = vec![
        (
            Some(Starved::PeriodicDisabled),
            NoResult::Expired,
            "periodicDisabled",
        ),
        (
            Some(Starved::MeteredPaused),
            NoResult::Expired,
            "meteredPaused",
        ),
        (Some(Starved::PowerSave), NoResult::Expired, "powerSave"),
    ];
    for (reason, text) in NO_RESULTS.into_iter().zip([
        "currentExpired",
        "currentUnmeasured",
        "currentSkipped",
        "currentMismatch",
    ]) {
        cases.push((None, reason, text));
    }
    for (starved, no_result, reason) in cases {
        // 首轮已完成；此后每拍评估，当前出口一直拿不出新鲜结果。
        let mut slot = Slot::default();
        let mut memory = Memory::default();
        let began = T0;
        let mut last = began;
        for beat in 0..=(STARVED_PERIODS * period / HEARTBEAT_INTERVAL_MS + 1) {
            last = began + beat * HEARTBEAT_INTERVAL_MS;
            let mut input = facts(&intent, Current::NoResult(no_result), &far_better);
            input.starved = starved;
            input.now = last;
            let decision = decide(&input, &mut memory);
            assert!(matches!(decision, Decision::Hold(_)), "{reason}：不换");
            slot.record_evaluation(evaluation(decision, last), false);
        }
        assert_eq!(slot.starved_since, Some(began), "{reason}：从第一拍起算");
        let mut read = inputs(&intent);
        read.now = began + HEARTBEAT_INTERVAL_MS;
        let early = project_status(&read, &slot);
        assert_eq!((early.mode, early.reason), ("noData", Some(reason)));
        assert_eq!(early.no_data_for_ms, Some(HEARTBEAT_INTERVAL_MS));
        assert!(!early.flags.starved, "{reason}：还没到三个周期");
        read.now = last;
        let late = project_status(&read, &slot);
        assert_eq!((late.mode, late.reason), ("noData", Some(reason)));
        assert!(late.flags.starved, "{reason}：超过三个周期");
        assert!(!late.flags.stalled, "评估任务活着");
        // 数据回来之后，计时与标记清掉。
        slot.record_evaluation(evaluation(Decision::Hold(Hold::Settled), last + 1), false);
        read.now = last + 1;
        let back = project_status(&read, &slot);
        assert_eq!((back.mode, back.no_data_for_ms), ("settled", None));
        assert!(!back.flags.starved);
    }
}

/// 提交没换成的状态：模式是「提交失败」、带原因与连续失败次数、下一次重试的间隔；等重试期间
/// 不报成「已选定」。换成之后清掉。「正在提交」的裁决由提交的结果落成换点或失败原因，槽里不会
/// 留着一个实际没换的「换点」。
#[test]
fn a_failed_commit_is_visible_in_the_status_until_one_succeeds() {
    let intent = auto();
    let mut slot = evaluated(Decision::Hold(Hold::Committing), 0);
    slot.record_commit_failure(CommitFailure::Failed, "target", WALL);
    assert_eq!(
        slot.evaluation.as_ref().unwrap().decision,
        Decision::Hold(Hold::CommitFailed(CommitFailure::Failed))
    );
    let status = project_status(&inputs(&intent), &slot);
    assert_eq!(
        (status.mode, status.reason),
        ("commitFailed", Some("commitFailed"))
    );
    assert_eq!(
        serde_json::to_value(&status.commit).unwrap(),
        json!({
            "consecutiveFailures": 1,
            "retryAfterMs": 2 * SWITCH_COOLDOWN_MS,
            "lastFailure": { "at": WALL, "toId": "target", "outcome": "commitFailed" },
        })
    );
    assert_eq!((slot.select_switches, slot.last_switch.clone()), (0, None));

    // 下一拍的裁决是「冷却中」：仍报提交失败。
    for waiting in [Hold::Cooldown, Hold::Pending(Lacking::Cooldown)] {
        slot.record_evaluation(evaluation(Decision::Hold(waiting), T0), false);
        let status = project_status(&inputs(&intent), &slot);
        assert_eq!(
            (status.mode, status.reason),
            ("commitFailed", Some("retryPending"))
        );
    }
    // 再失败一次（另一种原因）：计数与间隔推进。
    slot.record_evaluation(evaluation(Decision::Hold(Hold::Committing), T0), false);
    slot.record_commit_failure(CommitFailure::NotEligible, "target", WALL + 1);
    let status = project_status(&inputs(&intent), &slot);
    assert_eq!(status.reason, Some("commitNotEligible"));
    assert_eq!(status.commit.consecutive_failures, 2);
    assert_eq!(status.commit.retry_after_ms, Some(4 * SWITCH_COOLDOWN_MS));
    // 让位：如实记下，不推进计数。
    slot.record_evaluation(evaluation(Decision::Hold(Hold::Committing), T0), false);
    slot.record_commit_failure(CommitFailure::Yielded, "target", WALL + 2);
    assert_eq!(slot.memory.commit_failures(), 2);

    // 换成一次：落成换点，失败记录清掉。
    slot.record_evaluation(evaluation(Decision::Hold(Hold::Committing), T0), false);
    slot.record_switch(switch_record(Leg::Select, Cause::Better), T0);
    assert_eq!(
        slot.evaluation.as_ref().unwrap().decision,
        switch("cur", 120, Cause::Better)
    );
    let status = project_status(&inputs(&intent), &slot);
    assert_eq!((status.mode, status.reason), ("settled", Some("better")));
    assert_eq!(status.commit, CommitView::default());
    // 没有失败在案时，冷却就是冷却。
    slot.record_evaluation(evaluation(Decision::Hold(Hold::Cooldown), T0), false);
    assert_eq!(
        project_status(&inputs(&intent), &slot).reason,
        Some("cooldown")
    );
}

/// 条目 34：最近一次评估在 91 秒前 → 评估停滞；89 秒前不置位。这是读时投影：槽里只有时刻，
/// 评估任务不在运行时同样读得出来。手机不在前台时不算停滞。三个时刻都不知道（进程重启后意图
/// 已在盘上、还没评估过、核就绪时刻未知）时无从证明评估任务活着，按停滞处理。
#[test]
fn a_stalled_evaluation_is_projected_at_read_time() {
    let intent = auto();
    let at = |seconds_ago| {
        project_status(
            &inputs(&intent),
            &evaluated(Decision::Hold(Hold::Settled), seconds_ago),
        )
        .flags
        .stalled
    };
    assert!(at(91));
    assert!(!at(89));
    assert_eq!(STALL_AFTER_MS, 90_000);

    let mut background = inputs(&intent);
    background.foreground = false;
    assert!(
        !project_status(&background, &evaluated(Decision::Hold(Hold::Settled), 600))
            .flags
            .stalled
    );

    // 还没有任何一次评估：从核就绪与意图写入中较近的那个算起。
    let mut never = inputs(&intent);
    never.core_ready_ago_ms = Some(91_000);
    assert!(project_status(&never, &Slot::default()).flags.stalled);
    let armed_recently = Slot {
        intent_set_mono: Some(T0 - 10_000),
        ..Slot::default()
    };
    assert!(!project_status(&never, &armed_recently).flags.stalled);
    // 核刚换代、新任务还没评估过：旧评估再老也不算停滞。
    let mut restarted = inputs(&intent);
    restarted.core_ready_ago_ms = Some(5_000);
    assert!(
        !project_status(&restarted, &evaluated(Decision::Hold(Hold::Settled), 600))
            .flags
            .stalled
    );
    // 三者皆空。
    let mut unknown = inputs(&intent);
    unknown.core_ready_ago_ms = None;
    let status = project_status(&unknown, &Slot::default());
    assert!(status.flags.stalled);
    assert_eq!(status.reason, Some("notYetEvaluated"));
    unknown.foreground = false;
    assert!(!project_status(&unknown, &Slot::default()).flags.stalled);
}

/// 「持续多久」只看单调时刻：评估带着的墙钟时刻被回拨一天或前拨一天，停滞、长期无数据、出口
/// 逾期的判定都不变。
#[test]
fn wall_clock_jumps_do_not_move_any_duration() {
    let intent = auto();
    let period = 30 * MINUTE;
    let day = 24 * 60 * MINUTE;
    for wall in [WALL, WALL - day, WALL + day] {
        let mut slot = Slot::default();
        let began = T0 - STARVED_PERIODS * period - 1;
        for (at_mono, at) in [(began, WALL), (T0 - 1_000, wall)] {
            slot.record_evaluation(
                Evaluation {
                    at,
                    ..evaluation(Decision::Hold(Hold::NoCandidates), at_mono)
                },
                true,
            );
        }
        let mut outside = inputs(&intent);
        outside.exit_in_subscription = false;
        let status = project_status(&outside, &slot);
        assert!(status.flags.starved && status.flags.exit_outside_overdue);
        assert!(!status.flags.stalled);
        assert_eq!(status.no_data_for_ms, Some(STARVED_PERIODS * period + 1));
        assert_eq!(status.last_evaluation.unwrap().at, wall, "展示的时刻照给");
    }
    // 计胜间隔同理：两次测量的墙钟时刻倒着走，照样按入账的单调时刻计。
    let mut memory = Memory::default();
    let mut first = candidate("ch", 100, 1, T0);
    first.measured_at = WALL + day;
    let mut second = candidate("ch", 100, 2, T0 + WIN_MIN_GAP_MS);
    second.measured_at = WALL - day;
    round(&intent, &mut memory, &[first], T0);
    assert_eq!(
        round(&intent, &mut memory, &[second], T0 + WIN_MIN_GAP_MS),
        switch("ch", 100, Cause::Better)
    );
    // 反过来：墙钟上相隔一天、单调时刻只隔一分钟的两次测量凑不满。
    let mut memory = Memory::default();
    let mut first = candidate("ch", 100, 1, T0);
    first.measured_at = WALL;
    let mut second = candidate("ch", 100, 2, T0 + MINUTE);
    second.measured_at = WALL + day;
    round(&intent, &mut memory, &[first], T0);
    assert_eq!(
        round(&intent, &mut memory, &[second], T0 + MINUTE),
        Decision::Hold(Hold::Pending(Lacking::Gap))
    );
}

/// 长期无数据与出口逾期：从状况第一次出现起算，超过订阅周期的约定倍数才置位；状况消失即清。
#[test]
fn starvation_and_an_outside_exit_are_timed_from_when_they_began() {
    let intent = auto();
    let period = 30 * MINUTE;
    let mut slot = Slot::default();
    let began = T0 - STARVED_PERIODS * period - 1;
    slot.record_evaluation(
        evaluation(Decision::NotEvaluated(Gate::WaitingFirstRound), began),
        true,
    );
    // 中间换了一种「没有数据」的结论：起点不重置。
    slot.record_evaluation(
        evaluation(Decision::Hold(Hold::NoCandidates), T0 - 1_000),
        true,
    );
    assert_eq!(
        (slot.starved_since, slot.outside_since),
        (Some(began), Some(began))
    );

    let mut outside = inputs(&intent);
    outside.exit_in_subscription = false;
    let flags = project_status(&outside, &slot).flags;
    assert!(flags.starved && flags.exit_outside_subscription && flags.exit_outside_overdue);

    let mut just_under = inputs(&intent);
    just_under.exit_in_subscription = false;
    just_under.now = began + OUTSIDE_PERIODS * period;
    let flags = project_status(&just_under, &slot).flags;
    assert!(!flags.starved && !flags.exit_outside_overdue);
    assert!(
        flags.exit_outside_subscription,
        "不在订阅内是即时标记，逾期才看时长"
    );

    slot.record_evaluation(evaluation(Decision::Hold(Hold::Settled), T0), false);
    assert_eq!((slot.starved_since, slot.outside_since), (None, None));
    assert!(!project_status(&inputs(&intent), &slot).flags.starved);
}

/// 可选点全部未被读回证实时标记置位。
#[test]
fn the_all_unverified_flag_follows_the_last_evaluation() {
    let intent = auto();
    let mut slot = evaluated(Decision::Hold(Hold::Settled), 5);
    slot.evaluation.as_mut().unwrap().counts = Counts {
        selectable: 3,
        unverified: 3,
        ..Counts::default()
    };
    assert!(project_status(&inputs(&intent), &slot).flags.all_unverified);
    slot.evaluation.as_mut().unwrap().counts.unverified = 2;
    assert!(!project_status(&inputs(&intent), &slot).flags.all_unverified);
}

/// 槽的记账：换点计数按腿分开；离开自动选择时清评估、连胜、排除与提交失败，保留上次换点与计数。
#[test]
fn the_slot_counts_switches_per_leg_and_forgets_only_scoped_state_on_leave() {
    let mut slot = evaluated(Decision::Hold(Hold::Settled), 5);
    slot.memory.observe_epoch(
        &Epoch {
            subscription: "sub".to_string(),
            generation: 1,
            network_epoch: None,
            foreground_epoch: None,
        },
        0,
    );
    slot.record_switch(switch_record(Leg::Failover, Cause::Failover), T0);
    slot.record_switch(switch_record(Leg::Select, Cause::Better), T0 + 1);
    slot.memory.bar("flaky", T0, 30 * MINUTE);
    slot.record_commit_failure(CommitFailure::Failed, "x", WALL);
    assert_eq!((slot.failover_switches, slot.select_switches), (1, 1));
    assert!(slot.leave());
    assert_eq!(slot.evaluation, None);
    assert_eq!(slot.last_commit_failure, None);
    assert_eq!(slot.memory.commit_failures(), 0);
    assert!(slot.memory.barred(T0).is_empty());
    assert!(slot.last_switch.is_some());
    assert_eq!((slot.failover_switches, slot.select_switches), (1, 1));
    assert_eq!(
        slot.memory.better_switches_in_window(T0 + 2),
        1,
        "限频额度不因离开而归还"
    );
    assert!(!slot.leave(), "没有残留时如实说没有");
}
