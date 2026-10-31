use super::*;
use std::sync::{Arc, Mutex};

/// 测试用 executor：按预设返回 (stderr, code)。
type MockResults = Arc<Mutex<Vec<(String, i32)>>>;

struct MockExecutor {
    results: MockResults,
}

impl Executor for MockExecutor {
    fn execute(&self, _argv: &[String]) -> Result<(String, i32), ClientError> {
        let mut guard = self.results.lock().unwrap();
        if guard.is_empty() {
            return Ok((String::new(), 0));
        }
        Ok(guard.remove(0))
    }
}

fn executor_with(results: Vec<(String, i32)>) -> (MockExecutor, MockResults) {
    let results: MockResults = Arc::new(Mutex::new(results));
    let e = MockExecutor {
        results: results.clone(),
    };
    (e, results)
}

#[test]
fn osascript_escalation_argv_structure() {
    // 对照 HelperManager.ts:888-892: osascript -e 'do shell script "/bin/bash <path>" with admin'
    let esc = osascript_escalation("/tmp/install.sh");
    assert_eq!(esc.method, PrivilegeMethod::Osascript);
    assert_eq!(esc.argv[0], "/usr/bin/osascript");
    assert_eq!(esc.argv[1], "-e");
    assert!(esc.argv[2].contains("do shell script"));
    assert!(esc.argv[2].contains("with administrator privileges"));
    assert!(esc.argv[2].contains("/tmp/install.sh"));
}

#[test]
fn pkexec_escalation_argv_structure() {
    // 对照 PlatformPrivilegeService.ts:162: pkexec /bin/bash <path>
    let esc = pkexec_escalation("/tmp/setcap.sh");
    assert_eq!(esc.method, PrivilegeMethod::Pkexec);
    assert_eq!(
        esc.argv,
        vec!["/usr/bin/pkexec", "/bin/bash", "/tmp/setcap.sh"]
    );
}

#[test]
fn uac_escalation_argv_structure() {
    assert!(uac_escalation(r"C:\temp\install.ps1").is_err());
    let source = polaris_source_probe::crate_source!("privilege.rs");
    let source = polaris_source_probe::mask_comments(&source);
    let executor = source
        .split_once("impl Executor for StdExecutor {")
        .unwrap()
        .1
        .split_once("\n}\n")
        .unwrap()
        .0;
    let construct = executor.find("Command::new(&argv[0])").unwrap();
    let isolate = executor.find("cmd.env_remove(\"PSModulePath\")").unwrap();
    let spawn = executor.find(".output()").unwrap();
    assert!(construct < isolate && isolate < spawn);
    assert!(executor[isolate..spawn].contains("creation_flags(0x0800_0000)"));
    assert!(!executor.contains("std::env::remove_var"));
}

#[test]
fn uac_script_path_preserves_native_quotes_and_powershell_literals() {
    for path in [
        r"C:\Users\O'Brien $home\App Data\安装.ps1",
        r"C:\temp\install.ps1",
    ] {
        assert!(uac_escalation(path).is_err());
    }
}

#[test]
fn osascript_path_with_spaces_quoted() {
    // 含空格家目录不应击穿引号（Polaris osaShellArg 两层转义，HelperManager.ts:854）
    let esc = osascript_escalation("/Users/some user/App Data/install.sh");
    // argv[2] 是 AppleScript 字符串，路径应被 shell 单引号包裹
    assert!(esc.argv[2].contains("'/Users/some user/App Data/install.sh'"));
}

#[test]
fn osascript_path_with_apostrophe_escaped() {
    // 含撇号家目录：shell_quote 用 '\'' 关闭再开单引号（防 shell 注入，Polaris shq）
    // 直接测 shell_quote（安全关键层）—— applescript_escape 后反斜杠会翻倍，故不查 argv
    let quoted = shell_quote("/Users/o'brien/install.sh");
    // shq 输出: '/Users/o'\''brien/install.sh'（撇号用 '\'' 转义）
    assert!(quoted.contains("'\\''"), "撇号须被 '\\'' 转义: {quoted}");
    assert!(quoted.starts_with('\'') && quoted.ends_with('\''));
}

#[test]
fn is_cancelled_osascript_minus_128() {
    // HelperManager.ts:903: code === -128 或 stderr 含 -128 / "User canceled"
    assert!(is_user_cancelled(PrivilegeMethod::Osascript, -128, ""));
    assert!(is_user_cancelled(
        PrivilegeMethod::Osascript,
        1,
        "User canceled"
    ));
    assert!(is_user_cancelled(
        PrivilegeMethod::Osascript,
        1,
        "some -128 error"
    ));
    assert!(!is_user_cancelled(
        PrivilegeMethod::Osascript,
        1,
        "mkdir failed"
    ));
}

#[test]
fn pkexec_only_classifies_126_as_user_cancelled() {
    // pkexec：126 = 用户取消；127 = 当前会话没有认证代理，二者不能共用用户文案。
    assert!(is_user_cancelled(PrivilegeMethod::Pkexec, 126, ""));
    assert!(!is_user_cancelled(PrivilegeMethod::Pkexec, 127, ""));
    // 3 = 缺 setcap（非取消）
    assert!(!is_user_cancelled(PrivilegeMethod::Pkexec, 3, ""));
}

#[test]
fn uac_cancellation_uses_machine_marker_not_localized_text() {
    assert!(is_user_cancelled(
        PrivilegeMethod::Uac,
        1223,
        "UAC_ERROR_CANCELLED_1223"
    ));
    assert!(!is_user_cancelled(
        PrivilegeMethod::Uac,
        1,
        "UAC_ERROR_CANCELLED_1223"
    ));
    assert!(!is_user_cancelled(
        PrivilegeMethod::Uac,
        1223,
        "Der Vorgang wurde vom Benutzer abgebrochen."
    ));
    assert!(!is_user_cancelled(
        PrivilegeMethod::Uac,
        1,
        "installer cancel cleanup failed"
    ));
}

#[test]
fn run_uac_escalation_keeps_cancelled_and_failure_distinct() {
    let esc = Escalation {
        method: PrivilegeMethod::Uac,
        argv: vec!["classifier-only".into()],
    };
    let (cancelled, _) = executor_with(vec![("UAC_ERROR_CANCELLED_1223".to_owned(), 1223)]);
    assert_eq!(
        run_escalation(&esc, &cancelled).unwrap(),
        EscalationOutcome::Cancelled
    );

    let (failed, _) = executor_with(vec![("installer cancel cleanup failed".to_owned(), 1)]);
    assert_eq!(
        run_escalation(&esc, &failed).unwrap(),
        EscalationOutcome::Failed {
            stderr: "installer cancel cleanup failed".to_owned(),
            code: 1,
        }
    );
}

#[test]
fn classify_success() {
    let status = exit_status_for(0);
    assert_eq!(
        classify_outcome(PrivilegeMethod::Pkexec, status, ""),
        EscalationOutcome::Success
    );
}

#[test]
fn classify_cancelled() {
    let status = exit_status_for(126);
    assert_eq!(
        classify_outcome(PrivilegeMethod::Pkexec, status, ""),
        EscalationOutcome::Cancelled
    );
}

#[test]
fn classify_pkexec_without_authentication_agent_as_failed() {
    let status = exit_status_for(127);
    assert_eq!(
        classify_outcome(
            PrivilegeMethod::Pkexec,
            status,
            "No authentication agent found"
        ),
        EscalationOutcome::Failed {
            stderr: "No authentication agent found".to_owned(),
            code: 127,
        }
    );
}

#[test]
fn classify_failed() {
    let status = exit_status_for(1);
    assert_eq!(
        classify_outcome(PrivilegeMethod::Osascript, status, "mkdir: error"),
        EscalationOutcome::Failed {
            stderr: "mkdir: error".to_owned(),
            code: 1
        }
    );
}

#[test]
fn run_escalation_success_via_executor() {
    let esc = pkexec_escalation("/tmp/x.sh");
    let (exec, _) = executor_with(vec![("".to_owned(), 0)]);
    let outcome = run_escalation(&esc, &exec).unwrap();
    assert_eq!(outcome, EscalationOutcome::Success);
}

#[test]
fn run_escalation_cancelled_via_executor() {
    let esc = pkexec_escalation("/tmp/x.sh");
    let (exec, _) = executor_with(vec![("canceled".to_owned(), 126)]);
    let outcome = run_escalation(&esc, &exec).unwrap();
    assert_eq!(outcome, EscalationOutcome::Cancelled);
}

#[test]
fn run_escalation_failed_via_executor() {
    let esc = pkexec_escalation("/tmp/x.sh");
    let (exec, _) = executor_with(vec![("setcap: error".to_owned(), 3)]);
    let outcome = run_escalation(&esc, &exec).unwrap();
    assert_eq!(
        outcome,
        EscalationOutcome::Failed {
            stderr: "setcap: error".to_owned(),
            code: 3
        }
    );
}

#[test]
fn shell_quote_basic() {
    assert_eq!(shell_quote("simple"), "'simple'");
    assert_eq!(shell_quote("with space"), "'with space'");
}

#[test]
fn shell_quote_apostrophe() {
    // shq: o'brien -> 'o'\''brien'
    assert_eq!(shell_quote("o'brien"), "'o'\\''brien'");
}

#[test]
fn applescript_escape_doubles_backslash_and_quote() {
    assert_eq!(applescript_escape(r"a\b"), r"a\\b");
    assert_eq!(applescript_escape(r#"a"b"#), r#"a\"b"#);
}

#[test]
fn privilege_method_current_platform_is_one_of_three() {
    let m = PrivilegeMethod::for_current_platform();
    assert!(matches!(
        m,
        PrivilegeMethod::Osascript | PrivilegeMethod::Uac | PrivilegeMethod::Pkexec
    ));
}

/// 构造一个测试用 ExitStatus（正常退出码 N，非信号）。
///
/// unix 的 wait status 编码：高 8 位 = 正常退出码，低 7 位 = 信号（0 = 正常退出）。
/// `from_raw(code << 8)` 让 `.code()` 正确返回 `Some(code)`（避免 from_raw(126) 被当信号 → None）。
fn exit_status_for(code: i32) -> ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        ExitStatus::from_raw(code << 8)
    }
    #[cfg(not(unix))]
    {
        // 非 unix（Windows CI 腿）：用 `cmd /c exit N` 构造真 ExitStatus。
        //
        // **必须原样透传 code**：早先这里写 `if code == 0 { "0" } else { "1" }`，把一切非零
        // 压成 1 —— 而 126 恰是 pkexec「用户取消」的判据码（`is_user_cancelled`），于是
        // `classify_cancelled` 在 Windows 上拿到的是 `Failed { code: 1 }`，断言必红。
        // `cmd /c exit N` 对任意 0..=255 保真，无需降级。
        let exit_arg = code.to_string();
        Command::new("cmd")
            .args(["/c", "exit", &exit_arg])
            .status()
            .unwrap()
    }
}

// 非 unix 占位 trait 已移除（from_exit_code 在 unix 上正确工作，本 crate 测试在 linux 跑）。

/// Windows UAC 提权 argv 的三条硬约束。本函数此前**零测试覆盖** —— 改坏了不会红。
///
/// 判据落在**可执行形态**（ArgumentList 里的实参、退出码回传语句）上，不是「注释里提没提过」。
#[test]
fn windows_product_retires_policy_override_and_preserves_native_exit_custody() {
    assert!(uac_escalation(r"C:\temp\install.ps1").is_err());
    let source = polaris_source_probe::crate_source!("privilege.rs");
    let source = polaris_source_probe::mask_comments(&source);
    assert!(!source.contains("Start-Process"));
    assert!(!source.contains("EncodedCommand"));
    assert!(!source.contains("Bypass"));
    let native = polaris_source_probe::repo_file!("crates/windows-cleaner/src/windows/install.rs");
    for need in [
        "ShellExecuteExW",
        "SEE_MASK_NOCLOSEPROCESS",
        "GetExitCodeProcess",
        "ERROR_CANCELLED",
        "WaitForSingleObject",
        "image.check()",
        "hash(&image.file)",
    ] {
        assert!(native.contains(need), "{need}");
    }
    assert!(!native.contains("TerminateProcess"));
}

/// 路径里的单引号不得击穿 PowerShell 字符串（`''` 是 PS 单引号串里的转义写法）。
#[test]
fn uac_argv_escapes_single_quotes_in_the_script_path() {
    assert!(uac_escalation(r"C:\Users\o'brien\x.ps1").is_err());
}
