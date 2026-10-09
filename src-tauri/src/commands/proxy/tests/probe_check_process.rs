//! [`run_probe_check`] 的**真子进程**接线门。
//!
//! 起的是 [`write_sleeping_probe`] 写出来的 shell 探针（睡一会儿、建个见证文件、退出），
//! **不是** sing-box，不碰网络、不碰系统状态。
//!
//! # 守的是什么
//!
//! `run_probe_check` 此前自己写了一遍 `sing-box check` 的子进程接线，超时有、但漏了
//! 原超时腿把 `output()` 的 future 直接丢掉，未保留 Child native wait，留下游离 check。
//! 现在它调用 `core-supervisor::config_gate::run_check_raw`，持久 registry 保留原 Child
//! 到 native wait 和配置副本退休完成；LifecycleUnknown 必须向调用方传播。
//!
//! 退出事实来自该调用成功返回的同 Child native wait；见证文件只辅助观察延迟写入未发生。

use std::time::Duration;

use super::super::{run_probe_check, ProbeCheck};
use crate::test_support::{
    ran_in_isolated_worker, write_executable_stand_in, write_sleeping_probe, TestDir,
};

/// **正向对照**：探针在预算内跑完 ⇒ 判 `Supported`，且见证文件真的出现。
///
/// 没有这一条，下面那条的「见证文件不存在」既可能是「子进程被杀了」，也可能是「路径压根没传对」——
/// 那样断言恒真、零信息量。
#[tokio::test]
async fn supported_and_lets_the_child_finish_when_it_fits_the_budget() {
    if ran_in_isolated_worker(
        module_path!(),
        "supported_and_lets_the_child_finish_when_it_fits_the_budget",
    ) {
        return;
    }
    let dir = TestDir::new("polaris-probe-check-ok-");
    let witness = dir.path().join("ran.txt");
    let probe = write_sleeping_probe(dir.path(), &witness);

    let config = dir.path().join("probe.json");
    std::fs::write(&config, b"{}").unwrap();
    let verdict = run_probe_check(&probe, &config).await.unwrap();
    assert!(
        matches!(verdict, ProbeCheck::Supported),
        "探针 rc=0 ⇒ 必须判 Supported"
    );
    assert!(
        witness.exists(),
        "正向对照失败：探针跑完了却没写见证文件 —— 路径没传对，另一条的断言会恒真"
    );
}

/// 🔴 **变异锁：超时 → `Indeterminate`（failOpen）+ 子进程真的被杀掉**。
///
/// 改动前的这条腿会**留下游离进程**：超时判决是对的，但丢掉 future 并不杀子进程，那个
/// `sing-box check` 会一路跑完并写出见证文件。本测在那份源码上因此转红。
///
/// 使用真实时钟：暂停 Tokio 时钟会同时越过清理 wait 的预算，OS SIGCHLD 尚未调度时
/// 正确得到 CleanupUnknown，不能用它假造成功退出。此探针睡10秒，超过真实8秒预算。
#[tokio::test]
async fn timing_out_is_indeterminate_and_kills_the_child() {
    if ran_in_isolated_worker(
        module_path!(),
        "timing_out_is_indeterminate_and_kills_the_child",
    ) {
        return;
    }
    let dir = TestDir::new("polaris-probe-check-timeout-");
    let witness = dir.path().join("killed.txt");
    let probe = dir.path().join("sleeping-probe.sh");
    write_executable_stand_in(
        &probe,
        format!("#!/bin/sh\nsleep 10\n: > '{}'\n", witness.display()),
    );

    let config = dir.path().join("probe.json");
    std::fs::write(&config, b"{}").unwrap();
    let verdict = run_probe_check(&probe, &config).await.unwrap();
    assert!(
        matches!(verdict, ProbeCheck::Indeterminate),
        "超时是 failOpen：判 Supported 会把没验过的协议说成支持，判 Unsupported 会把一个\
         可能完全正常的协议标红"
    );

    // 真实时钟：等过探针的睡眠时长再看。活着的话这会儿早写完了。
    std::thread::sleep(Duration::from_secs(3));
    assert!(
        !witness.exists(),
        "超时后的探针仍完成延迟写入；原 Child wait/清理接线未守住"
    );
}
