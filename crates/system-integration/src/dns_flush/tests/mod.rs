use super::*;
use std::cell::RefCell;

#[derive(Default)]
struct MockExec {
    calls: RefCell<Vec<FlushCommand>>,
    fail: bool,
}
impl FlushExec for MockExec {
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
fn windows_flush_command_shape() {
    let c = windows_flush_command();
    // System32 绝对路径而非裸 `ipconfig`：部分设备 PATH 缺 System32 → 裸命令报「不是内部或外部
    // 命令」（上游 `ipconfigExe = system32('ipconfig.exe')` 同因）。本机非 Windows 时 env 无
    // SystemRoot → 回落 C:\Windows，故断言以 System32 路径结尾。
    assert!(
        c.program.ends_with("\\System32\\ipconfig.exe"),
        "须用 System32 绝对路径，实际 {}",
        c.program
    );
    assert_eq!(c.args, vec!["/flushdns".to_string()]);
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
fn windows_runs_ipconfig_flushdns() {
    let exec = MockExec::default();
    flush_os_dns_cache(Platform::Win, &exec, None, false, &mut |_| {});
    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].program.ends_with("ipconfig.exe"), "{:?}", calls[0]);
    assert_eq!(calls[0].args, vec!["/flushdns".to_string()]);
}

/// D4：装了 helper 就走 SYSTEM 那条，app 侧**不再**跑注定 rc=1 的 Medium IL ipconfig。
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
    assert!(exec.calls.borrow().is_empty(), "helper 成功还跑了本地命令");
    assert!(warned.is_empty());
}

/// 旧 helper（回 `ERR unknown`）/ 未装 → 回退本地 ipconfig，且降级原因可见。
///
/// 这是兼容矩阵「新 app × 旧 helper」那一格：必须回到改动前的行为，不 brick、不静默。
#[test]
fn windows_degrades_to_local_ipconfig_when_helper_unavailable() {
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
    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].program.ends_with("ipconfig.exe"), "{:?}", calls[0]);
    assert!(warned.contains("降级"), "{warned}");
    assert!(warned.contains("ERR unknown"), "降级原因不可见：{warned}");
}

/// B4：`helper_ready=false`（没装 helper）⇒ Windows 腿**一次都不去问** helper，也不发降级 warn。
///
/// 这是修的那个噪音：没装 helper 的机器每次起停核都吃一条「helper flush-dns 不可用」，而那条
/// 结论恒定、用户无从行动。判据咬的是**调用次数**不是日志字数 —— 只断言「没 warn」的话，把
/// warn 悄悄降成 debug 也能骗过去，而那并没有省掉那次注定失败的 IPC。
///
/// 正面断言不可省：跳过 helper 之后**必须**照常跑本地 ipconfig（跳过的是特权腿，不是这件事）。
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
    let calls = exec.calls.borrow();
    assert_eq!(calls.len(), 1, "跳过 helper 后仍必须跑本地 ipconfig");
    assert!(calls[0].program.ends_with("ipconfig.exe"), "{:?}", calls[0]);
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
                error: Some("ipconfig /flushdns exit 1: denied".into()),
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
    assert_eq!(exec.calls.borrow().len(), 1, "失败后必须降级本地 ipconfig");
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

#[test]
fn other_platform_noop() {
    let exec = MockExec::default();
    flush_os_dns_cache(Platform::Other, &exec, None, false, &mut |_| {});
    assert!(exec.calls.borrow().is_empty());
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
    } else if cfg!(target_os = "linux") {
        assert_eq!(cur, Platform::Linux);
    } else {
        assert_eq!(cur, Platform::Other);
    }
}
