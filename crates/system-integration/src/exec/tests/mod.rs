use super::*;
use crate::exec::exec_tests_helpers::MockRunner;
use std::cell::RefCell;

#[test]
fn command_new_builds_argv() {
    let c = Command::new(
        "gsettings",
        ["set", "org.gnome.system.proxy", "mode", "none"],
    );
    assert_eq!(c.program, "gsettings");
    assert_eq!(c.args.len(), 4);
    assert_eq!(c.args[3], "none");
}

#[test]
fn command_poll_interval_backs_off_but_keeps_the_original_ceiling() {
    let mut interval = INITIAL_POLL_INTERVAL;
    let mut observed = vec![interval];
    for _ in 0..5 {
        interval = next_poll_interval(interval, MAX_POLL_INTERVAL);
        observed.push(interval);
    }
    assert_eq!(
        observed,
        [1, 2, 4, 8, 10, 10].map(Duration::from_millis).to_vec()
    );
}

#[test]
fn mock_runner_records_and_returns_queued_stdout() {
    let r = MockRunner {
        stdouts: RefCell::new(vec!["hello".into()]),
        ..Default::default()
    };
    let out = r.run(&Command::new("x", [] as [&str; 0]), Duration::from_secs(1));
    assert_eq!(out.unwrap().stdout, "hello");
    assert_eq!(r.calls.borrow().len(), 1);
}

#[test]
fn mock_runner_fails_listed_program() {
    let r = MockRunner {
        fail_programs: vec!["boom".into()],
        ..Default::default()
    };
    assert!(r
        .run(
            &Command::new("boom", [] as [&str; 0]),
            Duration::from_secs(1)
        )
        .is_err());
}

// ── StdCommandRunner：只验「执行器」本身（不碰网络/代理/DNS，仅无害的本地命令）──
//
// 真进程 smoke 按宿主用 `#[cfg]` 选择可执行文件；紧邻的 system32 纯函数仍保持全平台可测。

// 这些烟测只验证 runner 的 stdout/stderr/exit-code 契约。Windows 用 System32 的 cmd.exe
// 避免 PowerShell 在并行 CI 中偶发地超过整个命令预算；不改变 runner 的超时契约。
#[cfg(unix)]
const COMMAND_SMOKE_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(windows)]
const COMMAND_SMOKE_TIMEOUT: Duration = Duration::from_secs(15);

// ── system32（纯函数，Linux 上得 Windows 路径 —— 零 cfg 可测性的样板）──

#[test]
fn system_root_prefers_systemroot_then_windir_then_default() {
    assert_eq!(system_root(Some("D:\\Win"), Some("E:\\w")), "D:\\Win");
    assert_eq!(system_root(None, Some("E:\\w")), "E:\\w");
    assert_eq!(system_root(None, None), "C:\\Windows");
    // 空串视同缺失（上游 `env.SystemRoot || env.windir || ...` 的 falsy 语义）。
    assert_eq!(system_root(Some(""), Some("E:\\w")), "E:\\w");
    assert_eq!(system_root(Some(""), Some("")), "C:\\Windows");
}

#[test]
fn system32_builds_backslash_absolute_path() {
    assert_eq!(
        system32("reg.exe", None, None),
        "C:\\Windows\\System32\\reg.exe"
    );
    assert_eq!(
        system32("netsh.exe", Some("D:\\Win"), None),
        "D:\\Win\\System32\\netsh.exe"
    );
    // 尾斜杠不产生双分隔符。
    assert_eq!(
        system32("ipconfig.exe", Some("D:\\Win\\"), None),
        "D:\\Win\\System32\\ipconfig.exe"
    );
}

#[test]
fn std_runner_ok_on_zero_exit() {
    #[cfg(unix)]
    let cmd = Command::new("/bin/sh", ["-c", "printf out; printf err >&2"]);
    #[cfg(windows)]
    let cmd = Command::new(
        system32_from_env("cmd.exe"),
        ["/D", "/Q", "/C", "echo out&echo err>&2"],
    );
    let out = StdCommandRunner.run(&cmd, COMMAND_SMOKE_TIMEOUT);
    let out = out.expect("zero exit → Ok");
    #[cfg(unix)]
    assert_eq!(out.stdout, "out");
    #[cfg(unix)]
    assert_eq!(out.stderr, "err");
    #[cfg(windows)]
    assert_eq!(out.stdout, "out\r\n");
    #[cfg(windows)]
    assert_eq!(out.stderr, "err\r\n");
}

#[test]
fn std_runner_err_on_nonzero_exit_carries_stderr() {
    #[cfg(unix)]
    let cmd = Command::new("/bin/sh", ["-c", "echo boom >&2; exit 3"]);
    #[cfg(windows)]
    let cmd = Command::new(
        system32_from_env("cmd.exe"),
        ["/D", "/Q", "/C", "echo boom>&2&exit /b 3"],
    );
    let e = StdCommandRunner
        .run(&cmd, COMMAND_SMOKE_TIMEOUT)
        .expect_err("非零退出 → Err（对齐 execFileAsync reject）");
    assert!(e.contains('3'), "错误须带退出码: {e}");
    assert!(e.contains("boom"), "错误须带 stderr: {e}");
}

#[test]
fn std_runner_err_on_missing_program() {
    let e = StdCommandRunner
        .run(
            &Command::new("polaris-no-such-binary-xyz", [] as [&str; 0]),
            Duration::from_secs(5),
        )
        .expect_err("二进制缺失 → Err");
    assert!(e.contains("启动失败"), "{e}");
}

/// 硬超时是 [`FlushExec`](crate::dns_flush::FlushExec) 契约的一部分（上游 EXEC_TIMEOUT_MS=3s，
/// 防挂起命令拖住 fire-and-forget 链）。若 runner 忽略 timeout 参数，本测试转红。
#[test]
fn std_runner_kills_on_timeout() {
    #[cfg(unix)]
    let cmd = Command::new("/bin/sh", ["-c", "sleep 30"]);
    #[cfg(windows)]
    let marker_dir = tempfile::Builder::new()
        .prefix("polaris-exec-")
        .tempdir_in(".")
        .expect("创建启动标记目录");
    #[cfg(windows)]
    let marker = marker_dir.path().join("started");
    #[cfg(windows)]
    let script = format!(
        "echo started>{}\\started&for /L %i in (1,1,2147483647) do @rem",
        marker_dir.path().file_name().unwrap().to_string_lossy()
    );
    #[cfg(windows)]
    let cmd = Command::new(
        system32_from_env("cmd.exe"),
        ["/D", "/Q", "/C", script.as_str()],
    );
    // Windows 的 cmd 内部循环不启动后代进程；标记证明命令体已执行，避免把冷启动超时当作杀进程通过。
    #[cfg(unix)]
    let timeout = Duration::from_millis(150);
    #[cfg(windows)]
    let timeout = Duration::from_secs(2);
    let started = Instant::now();
    let e = StdCommandRunner.run(&cmd, timeout).expect_err("超时 → Err");
    assert!(e.contains("超时"), "{e}");
    #[cfg(windows)]
    assert!(
        marker.exists(),
        "超时前命令体须已执行: {}",
        marker.display()
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "须在超时后即刻返回，实际 {:?}",
        started.elapsed()
    );
}

#[test]
fn std_runner_drains_large_output_without_deadlock() {
    #[cfg(unix)]
    let cmd = Command::new("/bin/sh", ["-c", "yes polaris | head -c 300000"]);
    #[cfg(windows)]
    let script = format!(
        "(for /L %i in (1,1,300) do @<nul set /p ={})&exit /b 0",
        "x".repeat(1000)
    );
    #[cfg(windows)]
    let cmd = Command::new(
        system32_from_env("cmd.exe"),
        ["/D", "/Q", "/C", script.as_str()],
    );
    // 远超管道缓冲（64KB）：若不起排空线程，此处会与 try_wait 轮询互等 → 超时失败。
    // 复用平台烟测预算；此测试验证「能排空并退出」。
    let out = StdCommandRunner
        .run(&cmd, COMMAND_SMOKE_TIMEOUT)
        .expect("大输出须正常收完");
    assert_eq!(out.stdout.len(), 300_000);
}

#[test]
fn observed_spawn_failure_is_distinct_from_unknown_cleanup() {
    let observed = StdCommandRunner.run_observed(
        &Command::new("polaris-no-such-binary-xyz", [] as [&str; 0]),
        COMMAND_SMOKE_TIMEOUT,
    );
    assert_eq!(observed.cleanup(), CommandCleanup::NotSpawned);
    assert!(observed
        .operation()
        .unwrap()
        .unwrap_err()
        .contains("启动失败"));
    assert!(observed.into_pending().is_none());
}

#[test]
fn observed_nonzero_operation_still_requires_both_actual_reader_returns() {
    #[cfg(unix)]
    let cmd = Command::new("/bin/sh", ["-c", "printf out; printf err >&2; exit 3"]);
    #[cfg(windows)]
    let cmd = Command::new(
        system32_from_env("cmd.exe"),
        ["/D", "/Q", "/C", "echo out&echo err>&2&exit /b 3"],
    );
    let observed = StdCommandRunner.run_observed(&cmd, COMMAND_SMOKE_TIMEOUT);
    let mut pending = observed.into_pending().expect("original child custody");
    assert_eq!(
        pending.poll_cleanup(COMMAND_SMOKE_TIMEOUT),
        CommandCleanup::NativeAndReadersReturned,
    );
    assert!(pending.native_status().is_some());
    assert!(
        matches!(pending.stdout_disposition(), CommandReaderDisposition::Returned(output) if output.contains("out"))
    );
    assert!(
        matches!(pending.stderr_disposition(), CommandReaderDisposition::Returned(output) if output.contains("err"))
    );
    let error = pending.operation().unwrap().unwrap_err();
    assert!(error.contains('3') && error.contains("err"));
}

#[cfg(unix)]
#[test]
fn observed_timeout_retains_same_child_and_reader_handles_until_actual_return() {
    let cmd = Command::new("/bin/sh", ["-c", "exec sleep 30"]);
    let observed = StdCommandRunner.run_observed(&cmd, Duration::from_millis(30));
    let mut pending = observed
        .into_pending()
        .expect("timeout keeps original resources");
    let child_id = pending.child.id();
    assert!(pending.operation().unwrap().unwrap_err().contains("超时"));
    assert_eq!(
        pending.poll_cleanup(Duration::from_secs(2)),
        CommandCleanup::NativeAndReadersReturned,
    );
    assert_eq!(
        pending.child.id(),
        child_id,
        "poll cannot replace original child"
    );
    assert!(pending.native_status().is_some());
    assert!(pending.stdout.handle.is_none() && pending.stderr.handle.is_none());
    assert!(
        pending.operation().unwrap().unwrap_err().contains("超时"),
        "cleanup cannot rewrite the failed operation"
    );
}

#[cfg(unix)]
#[test]
fn exited_child_does_not_imply_inherited_reader_eof() {
    let cmd = Command::new("/bin/sh", ["-c", "sleep 0.2 & printf out; printf err >&2"]);
    let observed = StdCommandRunner.run_observed(&cmd, COMMAND_SMOKE_TIMEOUT);
    let mut pending = observed.into_pending().unwrap();
    assert!(pending.native_status().is_some());
    assert_eq!(pending.cleanup(), CommandCleanup::Unknown);
    assert!(pending.stdout.handle.is_some() || pending.stderr.handle.is_some());
    assert!(pending.operation().is_none());
    assert_eq!(
        pending.poll_cleanup(Duration::ZERO),
        CommandCleanup::Unknown
    );
    assert_eq!(
        pending.poll_cleanup(Duration::from_secs(2)),
        CommandCleanup::NativeAndReadersReturned
    );
    assert_eq!(pending.operation().unwrap().unwrap().stdout, "out");
}

#[test]
fn actual_reader_join_preserves_read_error_and_missing_pipe() {
    struct FailingRead;
    impl Read for FailingRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "fixture read denied",
            ))
        }
    }
    let mut reader = CommandReader::start(Some(FailingRead));
    reader.join();
    assert!(
        matches!(reader.disposition, CommandReaderDisposition::ReadFailed(ref error) if error.contains("fixture read denied"))
    );
    assert!(reader.observed_output().is_err());
    assert_eq!(
        reader.legacy_output(),
        "",
        "legacy conversion provides no completion"
    );
    let mut missing = CommandReader::start(None::<io::Cursor<Vec<u8>>>);
    missing.join();
    assert!(matches!(
        missing.disposition,
        CommandReaderDisposition::ReadFailed(_)
    ));
}

#[test]
fn pending_reader_is_retained_and_successor_joins_same_thread() {
    struct HeldRead(std::sync::mpsc::Receiver<()>);
    impl Read for HeldRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            self.0.recv().unwrap();
            Ok(0)
        }
    }
    let (release, receiver) = std::sync::mpsc::channel();
    let mut reader = CommandReader::start(Some(HeldRead(receiver)));
    let thread_id = reader.handle.as_ref().unwrap().thread().id();
    reader.join_returned();
    assert_eq!(reader.disposition, CommandReaderDisposition::Pending);
    assert_eq!(reader.handle.as_ref().unwrap().thread().id(), thread_id);
    release.send(()).unwrap();
    reader.join();
    assert_eq!(
        reader.disposition,
        CommandReaderDisposition::Returned(String::new())
    );
    assert!(reader.handle.is_none());
}

#[test]
fn reader_panic_does_not_skip_second_join_or_mint_cleanup() {
    #[cfg(unix)]
    let cmd = Command::new("/bin/sh", ["-c", "printf out; printf err >&2"]);
    #[cfg(windows)]
    let cmd = Command::new(
        system32_from_env("cmd.exe"),
        ["/D", "/Q", "/C", "echo out&echo err>&2"],
    );
    let mut pending = StdCommandRunner
        .run_observed(&cmd, COMMAND_SMOKE_TIMEOUT)
        .into_pending()
        .unwrap();
    assert_eq!(
        pending.poll_cleanup(COMMAND_SMOKE_TIMEOUT),
        CommandCleanup::NativeAndReadersReturned
    );
    // Negative-only fault: original native wait is real; injected readers cannot create a positive receipt.
    pending.stdout = CommandReader {
        handle: Some(std::thread::spawn(|| panic!("fixture reader panic"))),
        disposition: CommandReaderDisposition::Pending,
    };
    pending.stderr = CommandReader::start(Some(io::Cursor::new(b"second".to_vec())));
    assert_eq!(
        pending.poll_cleanup(Duration::from_millis(100)),
        CommandCleanup::Unknown
    );
    assert_eq!(
        pending.stdout.disposition,
        CommandReaderDisposition::Panicked
    );
    assert_eq!(
        pending.stderr.disposition,
        CommandReaderDisposition::Returned("second".to_owned())
    );
    assert!(pending.operation().unwrap().unwrap_err().contains("panic"));
    assert_eq!(
        pending.poll_cleanup(Duration::ZERO),
        CommandCleanup::Unknown
    );
}
