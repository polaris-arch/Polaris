use super::*;
use std::cell::{Cell, RefCell};

#[derive(Default)]
struct SessionExec {
    ending: std::cell::Cell<bool>,
    exec: MockExec,
}

impl FlushExec for SessionExec {
    fn windows_session_ending(&self) -> bool {
        self.ending.get()
    }

    fn windows_flush(&self, timeout: Duration) -> Result<(), String> {
        self.exec.windows_flush(timeout)
    }

    fn exec(&self, cmd: &FlushCommand, timeout: Duration) -> Result<(), String> {
        self.exec.exec(cmd, timeout)
    }
}

#[test]
fn queued_windows_flush_rechecks_the_session_before_contacting_helper() {
    let exec = SessionExec::default();
    // The task was queued while open, then actually ran during session end.
    exec.ending.set(true);
    let helper_calls = std::cell::Cell::new(0);
    let flushed = flush_os_dns_cache(
        Platform::Win,
        &exec,
        Some(&|| {
            helper_calls.set(helper_calls.get() + 1);
            HelperFlushResult {
                ok: true,
                ..Default::default()
            }
        }),
        true,
        &mut |_| {},
    );
    assert!(!flushed);
    assert_eq!(helper_calls.get(), 0);
    assert!(exec.exec.calls.borrow().is_empty());
    assert_eq!(exec.exec.native_calls.get(), 0);
}

#[test]
fn session_end_while_helper_is_in_flight_blocks_local_fallback() {
    let exec = SessionExec::default();
    let helper_calls = std::cell::Cell::new(0);
    let flushed = flush_os_dns_cache(
        Platform::Win,
        &exec,
        Some(&|| {
            helper_calls.set(helper_calls.get() + 1);
            exec.ending.set(true);
            HelperFlushResult {
                error: Some("service stopped".into()),
                ..Default::default()
            }
        }),
        true,
        &mut |_| {},
    );
    assert!(!flushed);
    assert_eq!(helper_calls.get(), 1);
    assert!(exec.exec.calls.borrow().is_empty());
    assert_eq!(exec.exec.native_calls.get(), 0);
}

#[test]
fn cancelled_session_end_allows_the_next_ordinary_flush() {
    let exec = SessionExec::default();
    exec.ending.set(true);
    assert!(!flush_os_dns_cache(
        Platform::Win,
        &exec,
        None,
        false,
        &mut |_| {}
    ));
    assert!(exec.exec.calls.borrow().is_empty());
    assert_eq!(exec.exec.native_calls.get(), 0);
    exec.ending.set(false);
    assert!(flush_os_dns_cache(
        Platform::Win,
        &exec,
        None,
        false,
        &mut |_| {}
    ));
    assert!(exec.exec.calls.borrow().is_empty());
    assert_eq!(exec.exec.native_calls.get(), 1);
}

#[derive(Default)]
struct MockExec {
    calls: RefCell<Vec<FlushCommand>>,
    fail: bool,
    native_calls: Cell<usize>,
}
impl FlushExec for MockExec {
    fn windows_flush(&self, timeout: Duration) -> Result<(), String> {
        assert_eq!(timeout, EXEC_TIMEOUT);
        self.native_calls.set(self.native_calls.get() + 1);
        if self.fail {
            Err("native API denied".into())
        } else {
            Ok(())
        }
    }

    fn exec(&self, cmd: &FlushCommand, _timeout: Duration) -> Result<(), String> {
        self.calls.borrow_mut().push(cmd.clone());
        if self.fail {
            Err("exec failed".into())
        } else {
            Ok(())
        }
    }
}

#[test]
fn mac_user_flush_command_shape() {
    let c = mac_user_flush_command();
    assert_eq!(c.program, "/usr/bin/dscacheutil");
    assert_eq!(c.args, vec!["-flushcache".to_string()]);
}

#[test]
fn linux_flush_command_shape() {
    let c = linux_flush_command();
    assert_eq!(c.program, "resolvectl");
    assert_eq!(c.args, vec!["flush-caches".to_string()]);
}

#[test]
fn mac_uses_helper_when_ok() {
    let exec = MockExec::default();
    let mut warned = String::new();
    flush_os_dns_cache(
        Platform::Mac,
        &exec,
        Some(&|| HelperFlushResult {
            ok: true,
            partial: None,
            error: None,
        }),
        true,
        &mut |m| warned = m.into(),
    );
    // helper ok → 不走 exec。
    assert!(exec.calls.borrow().is_empty());
    assert!(warned.is_empty());
}

#[test]
fn mac_partial_warns_no_degrade() {
    let exec = MockExec::default();
    let mut warned = String::new();
    flush_os_dns_cache(
        Platform::Mac,
        &exec,
        Some(&|| HelperFlushResult {
            ok: true,
            partial: Some("HUP mDNSResponder failed".into()),
            error: None,
        }),
        true,
        &mut |m| warned = m.into(),
    );
    // partial → 不降级（不 exec），仅 warn。
    assert!(exec.calls.borrow().is_empty());
    assert!(warned.contains("partial"));
}

#[test]
fn mac_helper_unavailable_degrades_to_user_level() {
    let exec = MockExec::default();
    let mut warned = String::new();
    flush_os_dns_cache(
        Platform::Mac,
        &exec,
        Some(&|| HelperFlushResult {
            ok: false,
            partial: None,
            error: Some("ERR unknown".into()),
        }),
        true,
        &mut |m| warned = m.into(),
    );
    // helper 不可用 → 降级 dscacheutil。
    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "/usr/bin/dscacheutil");
    assert!(warned.contains("降级"));
}

#[test]
fn mac_no_helper_degrades_directly() {
    let exec = MockExec::default();
    let warned = String::new();
    flush_os_dns_cache(Platform::Mac, &exec, None, false, &mut |_m| {});
    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "/usr/bin/dscacheutil");
    assert!(warned.is_empty());
}

#[test]
fn mac_exec_failure_warns_not_throws() {
    let exec = MockExec {
        fail: true,
        ..Default::default()
    };
    let mut warned = String::new();
    assert!(!flush_os_dns_cache(
        Platform::Mac,
        &exec,
        None,
        false,
        &mut |m| { warned = m.into() }
    ));
    assert!(warned.contains("失败（忽略）"));
}

#[test]
fn windows_uses_only_native_api_and_reports_failure_without_a_child_fallback() {
    for fail in [false, true] {
        let exec = MockExec {
            fail,
            ..Default::default()
        };
        let mut warned = String::new();
        assert_eq!(
            flush_os_dns_cache(Platform::Win, &exec, None, false, &mut |m| warned =
                m.into()),
            !fail
        );
        assert_eq!(exec.native_calls.get(), 1);
        assert!(exec.calls.borrow().is_empty());
        assert_eq!(warned.contains("native API denied"), fail);
    }
}

/// D4：装了 helper 就走 SYSTEM 那条，app 侧**不再**跑注定 rc=1 的 Medium IL API。
#[test]
fn windows_uses_helper_when_available() {
    let exec = MockExec::default();
    let mut warned = String::new();
    let ok = flush_os_dns_cache(
        Platform::Win,
        &exec,
        Some(&|| HelperFlushResult {
            ok: true,
            partial: None,
            error: None,
        }),
        true,
        &mut |m| warned = m.into(),
    );
    assert!(ok);
    assert_eq!(exec.native_calls.get(), 0);
    assert!(exec.calls.borrow().is_empty(), "helper 成功还跑了本地命令");
    assert!(warned.is_empty());
}

/// 旧 helper（回 `ERR unknown`）/ 未装 → 回退本地原生 API，且降级原因可见。
///
/// 这是兼容矩阵「新 app × 旧 helper」那一格：只调用新原生能力；旧 helper 不得收到旧 flush-dns 命令。
#[test]
fn windows_degrades_to_local_native_api_when_helper_unavailable() {
    let exec = MockExec::default();
    let mut warned = String::new();
    flush_os_dns_cache(
        Platform::Win,
        &exec,
        Some(&|| HelperFlushResult {
            ok: false,
            partial: None,
            error: Some("ERR unknown".into()),
        }),
        true,
        &mut |m| warned = m.into(),
    );
    assert_eq!(exec.native_calls.get(), 1);
    assert!(exec.calls.borrow().is_empty());
    assert!(warned.contains("降级"), "{warned}");
    assert!(warned.contains("ERR unknown"), "降级原因不可见：{warned}");
}

/// B4：`helper_ready=false`（没装 helper）⇒ Windows 腿**一次都不去问** helper，也不发降级 warn。
///
/// 这是修的那个噪音：没装 helper 的机器每次起停核都吃一条「helper flush-dns 不可用」，而那条
/// 结论恒定、用户无从行动。判据咬的是**调用次数**不是日志字数 —— 只断言「没 warn」的话，把
/// warn 悄悄降成 debug 也能骗过去，而那并没有省掉那次注定失败的 IPC。
///
/// 正面断言不可省：跳过 helper 之后**必须**照常调用本地原生 API（跳过的是特权腿，不是这件事）。
#[test]
fn windows_skips_the_helper_when_it_is_not_ready() {
    let exec = MockExec::default();
    let consulted = std::cell::Cell::new(0usize);
    let mut warned = String::new();
    flush_os_dns_cache(
        Platform::Win,
        &exec,
        Some(&|| {
            consulted.set(consulted.get() + 1);
            HelperFlushResult {
                ok: false,
                partial: None,
                error: Some("ERR unknown".into()),
            }
        }),
        false,
        &mut |m| warned = m.into(),
    );
    assert_eq!(consulted.get(), 0, "没装 helper 还去问了一次 —— 噪音没省掉");
    assert!(warned.is_empty(), "不该发降级 warn：{warned}");
    assert_eq!(exec.native_calls.get(), 1);
    assert!(exec.calls.borrow().is_empty());
}

/// B4 的**反向对照**：`helper_ready=true` 时 helper 照常被问，失败照常出声。
///
/// 没有这一条，上一条可以被「Windows 腿干脆别调 helper 了」这种改法同样满足 —— 那是把 D4 整条
/// 关掉，而不是消噪音。本条钉住「真失败一格都不吞」：问过了、失败了、原因进了 warn、降级照走。
#[test]
fn windows_still_consults_a_ready_helper_and_reports_its_failure() {
    let exec = MockExec::default();
    let consulted = std::cell::Cell::new(0usize);
    let mut warned = String::new();
    flush_os_dns_cache(
        Platform::Win,
        &exec,
        Some(&|| {
            consulted.set(consulted.get() + 1);
            HelperFlushResult {
                ok: false,
                partial: None,
                error: Some("native DNS API denied".into()),
            }
        }),
        true,
        &mut |m| warned = m.into(),
    );
    assert_eq!(consulted.get(), 1, "装了 helper 就必须真的去问一次");
    assert!(warned.contains("降级"), "{warned}");
    assert!(
        warned.contains("denied"),
        "helper 的真失败原因被吞掉了：{warned}"
    );
    assert_eq!(exec.native_calls.get(), 1);
    assert!(exec.calls.borrow().is_empty());
}

#[test]
fn linux_runs_resolvectl() {
    let exec = MockExec::default();
    flush_os_dns_cache(Platform::Linux, &exec, None, false, &mut |_| {});
    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "resolvectl");
}

#[test]
fn linux_flush_failure_is_observable_without_throwing() {
    let exec = MockExec {
        fail: true,
        ..Default::default()
    };
    let mut warned = String::new();
    let ok = flush_os_dns_cache(Platform::Linux, &exec, None, false, &mut |message| {
        warned = message.to_owned();
    });
    assert!(!ok);
    assert!(warned.contains("失败（忽略）"));
}

/// 未知平台：一条命令都不跑，且**返 `false`**（= 「本次没有刷」，2026-09-05 由 true 改）。
///
/// 与紧邻的 Android 那条构成对照组：两条都不 spawn，但返回值**必须不同** —— Android 的 `true`
/// 建立在一条已知事实上（该平台没有应用可刷的系统缓存，故「刷完了」为真），`Other` 拿不到同一条
/// 事实，返 `true` 就是把「没做」报成「做完了」。只断言「不 spawn」会让这两条无法区分。
#[test]
fn other_platform_is_an_honest_not_flushed_noop() {
    let exec = MockExec::default();
    let ok = flush_os_dns_cache(Platform::Other, &exec, None, false, &mut |_| {});
    assert!(
        !ok,
        "未知平台上没跑任何刷缓存命令 ⇒ 只能报「没刷」；报「刷完了」是本轮要拆掉的乐观兜底"
    );
    assert!(exec.calls.borrow().is_empty());
}

/// Android：**一条命令都不许跑**，且返 true（本平台上「刷缓存」这件事已完成）。
///
/// 正对照在同文件的 mac/win/linux 三条：那三条断言**确实跑了**对应命令。只写「Android 不跑」
/// 会被「MockExec 记账坏了、谁都记不到」骗过。
#[test]
fn android_flush_is_a_true_noop_and_spawns_nothing() {
    let exec = MockExec::default();
    let ok = flush_os_dns_cache(Platform::Android, &exec, None, false, &mut |_| {});
    assert!(ok, "Android 上没有应用可刷的系统 DNS 缓存 ⇒ no-op 即完成");
    assert!(
        exec.calls.borrow().is_empty(),
        "Android 上不得 spawn 任何刷缓存命令（`resolvectl`/`ipconfig`/`dscacheutil` 一个都没有）"
    );
}

#[test]
fn current_platform_matches_target() {
    // current() 由编译 target 决定，按 target 断言（对齐 helper-proto
    // platform_current_matches_compile_target），三平台 CI 均成立。
    let cur = Platform::current();
    if cfg!(target_os = "macos") {
        assert_eq!(cur, Platform::Mac);
    } else if cfg!(target_os = "windows") {
        assert_eq!(cur, Platform::Win);
    } else if cfg!(target_os = "android") {
        assert_eq!(cur, Platform::Android);
    } else if cfg!(target_os = "linux") {
        assert_eq!(cur, Platform::Linux);
    } else {
        assert_eq!(cur, Platform::Other);
    }
}
