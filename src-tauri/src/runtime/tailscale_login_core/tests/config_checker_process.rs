//! [`SingBoxConfigChecker`] 的**真子进程**接线门。
//!
//! 与本目录其余测试相反，这两条真起一个子进程 —— 起的是 [`write_sleeping_probe`] 写出来的
//! shell 探针（睡一会儿、建个见证文件、退出），**不是** sing-box，不碰网络、不碰系统状态。
//!
//! # 守的是什么
//!
//! `SingBoxConfigChecker::check` 此前自己写了一遍 `sing-box check` 的子进程接线，而那一份**没有
//! 超时**：`output()` 会一直等到子进程自己退出。它挂在瞬态登录核与测速临时核的起核前置位上，
//! 于是 check 一旦挂住（慢盘、杀软扫描、核二进制半损坏），整条登录/测速流程跟着永久挂起。
//! 现在它改调 `core-supervisor::config_gate::run_check_raw` —— 全仓唯一那份带超时与
//! 原生 Child 保管与退出确认的实现。本文件验本调用点确实走了那条实现，保留真实进程
//! 完成/超时行为与见证文件；文件缺席自身不能签退出，退出事实来自共用 custody 的 native wait。
//!
//! # 为什么只有 unix
//!
//! 见 [`write_sleeping_probe`] 的文档：跨平台的那一半（超时与原生退出确认）由
//! `crates/core-supervisor/tests/config_gate_process.rs` 用 Rust 探针在三平台各跑一遍；本处只验
//! 本包这个调用点接到了那条实现上，而这件事与平台无关。

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::super::{ConfigChecker, SingBoxConfigChecker};
use crate::test_support::{
    ran_in_isolated_worker, sleeping_probe_script, write_executable_stand_in, write_sleeping_probe,
    TestDir, PROBE_SLEEP_MILLIS,
};

fn config_fixture(dir: &Path) -> PathBuf {
    let config = dir.join("config.json");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&config)
        .expect("real check input fixture");
    file.write_all(b"{}").unwrap();
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    config
}

/// **正向对照**：探针在预算内跑完 ⇒ 判 `Ok`，且见证文件真的出现。
///
/// 没有这一条，下面那条的「见证文件不存在」既可能是「子进程被杀了」，也可能是「路径压根没传对、
/// 这条腿从来就写不出文件」—— 那样断言恒真、零信息量。
#[tokio::test]
async fn accepts_and_lets_the_child_finish_when_it_fits_the_budget() {
    if ran_in_isolated_worker(
        module_path!(),
        "accepts_and_lets_the_child_finish_when_it_fits_the_budget",
    ) {
        return;
    }
    let dir = TestDir::new("polaris-login-cfgcheck-ok-");
    let witness = dir.path().join("ran.txt");
    let probe = write_sleeping_probe(dir.path(), &witness);
    let config = config_fixture(dir.path());

    SingBoxConfigChecker
        .check(&probe, &config)
        .await
        .expect("探针 rc=0 ⇒ 必须判 Ok");

    assert!(
        witness.exists(),
        "正向对照失败：探针跑完了却没写见证文件 —— 路径没传对，另一条的断言会恒真"
    );
}

/// 🔴 **变异锁：check 挂住时必须超时返回，且子进程真的被杀掉**。
///
/// 改动前这一整条腿是不存在的：`SingBoxConfigChecker::check` 没有任何超时，喂它一个不退出的
/// 子进程，`await` 永不返回 —— 本测在那份源码上跑不完（挂死），而不是失败一次就结束。
///
/// 时钟用 `start_paused`：超时预算是写死的 [`CONFIG_CHECK_TIMEOUT`](polaris_core_supervisor::CONFIG_CHECK_TIMEOUT)
/// （5 s）。等探针的启动见证后只推进检查期限，随即恢复真实时钟，让 OS 原生 wait 有机会
/// 完成。不能让虚时再次跳过收割预算，也不能把 CleanupUnknown 当成已确认的超时诊断。
#[tokio::test(start_paused = true)]
async fn times_out_instead_of_hanging_and_kills_the_child() {
    if ran_in_isolated_worker(
        module_path!(),
        "times_out_instead_of_hanging_and_kills_the_child",
    ) {
        return;
    }
    let dir = TestDir::new("polaris-login-cfgcheck-timeout-");
    let witness = dir.path().join("killed.txt");
    let probe = dir.path().join("sleeping-probe.sh");
    let config = config_fixture(dir.path());
    let started = dir.path().join("started.txt");
    let script = sleeping_probe_script(&witness);
    let (shebang, body) = script.split_once('\n').unwrap();
    write_executable_stand_in(
        &probe,
        format!("{shebang}\n: > '{}'\n{body}", started.display()),
    );

    let check = tokio::spawn(async move { SingBoxConfigChecker.check(&probe, &config).await });
    let start_deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !started.exists() {
        assert!(
            std::time::Instant::now() < start_deadline,
            "probe did not start"
        );
        // Keep this paused runtime runnable until the real OS process has started.
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!witness.exists(), "probe must still be inside its sleep");
    tokio::time::advance(polaris_core_supervisor::CONFIG_CHECK_TIMEOUT + Duration::from_millis(1))
        .await;
    tokio::time::resume();

    let err = check
        .await
        .unwrap()
        .expect_err("超时必须报错，而不是把一份没验过的配置当成通过");
    assert!(
        err.contains("超时"),
        "超时腿的文案要说得出是超时（调用方会把它原样呈给用户）；实得：{err}"
    );

    // 真实时钟：等过探针的睡眠时长再看。活着的话这会儿早写完了。
    std::thread::sleep(Duration::from_millis(PROBE_SLEEP_MILLIS + 300));
    assert!(
        !witness.exists(),
        "超时后探针仍写了见证文件，原生关闭/退休没有在返回前收口"
    );
}
