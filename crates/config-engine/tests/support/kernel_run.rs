//! 起核（`sing-box run`）用例的**唯一**入口：本机禁起核的跳过判定 + 唯一拼 `run` 子命令的地方。
//!
//! # 为什么要有它（陈先生决策 2026-09-25）
//!
//! 本机硬约束「不许起 `sing-box run`」与 `cargo test --workspace` 里的真起核门冲突
//! （`network_profile_runtime` / `subscription_update_guard_runtime` 在盘上有随包核时默认就会真起核）。
//! 决策：**本机门禁跳过起核用例，覆盖交给 CI**。本文件就是那个开关的唯一实现。
//!
//! # 环境变量语义
//!
//! - `POLARIS_NO_KERNEL_RUN=1`：禁起核。起核用例入口的 `kernel_run_or_skip` 返回 `false`
//!   并打印一行跳过说明；若设了 `POLARIS_KERNEL_RUN_SKIP_LOG=<文件>`，再往该文件追加一行用例名，
//!   供 `scripts/gate-rust.sh` 汇总成「跳过 N 个起核用例」（`cargo test` 会吞掉通过用例的 stderr，
//!   只靠 `eprintln!` 在门输出里看不见 —— 那就成了静默空绿）。
//!   未设 / `0` / 空串 = 照常起核（CI 的语义）；其它取值当场红，不猜。
//! - 与 `POLARIS_REQUIRE_KERNEL_GATE=1` 同时设 ⇒ 当场红：「要求内核门必须生效」与「禁起核」
//!   互斥，放任其一胜出都会造出「要求跑却被跳过」的空跑绿。
//!
//! # 两道闸
//!
//! 1. `kernel_run_or_skip`：用例入口第一行调用，禁起核时整条用例跳过（不做半截）。
//! 2. `with_run`：**全仓测试源码里唯一允许拼 `run` 子命令的地方**（源码级门
//!    `tests/kernel_run_single_entry.rs` 钉死）。禁起核时它直接 panic —— 漏调第 1 道闸的新用例
//!    在本机表现为红，而不是悄悄把核起起来。
//!
//! 本文件只依赖 std：`src-tauri` 的真核用例经 `#[path]` 引入同一份，不各写一份判定。
#![allow(dead_code)]

use std::io::Write;
use std::process::Command;

/// 禁起核开关（本机 `scripts/gate-rust.sh` 在非 CI 环境 export 为 `1`）。
pub const NO_KERNEL_RUN_ENV: &str = "POLARIS_NO_KERNEL_RUN";
/// 可选：跳过记账文件路径，每跳过一条用例追加一行。
pub const SKIP_LOG_ENV: &str = "POLARIS_KERNEL_RUN_SKIP_LOG";
/// 打包 / release-risk 腿「内核门必须生效」的开关（语义见 `core_locator::core_or_skip`）。
const REQUIRE_ENV: &str = "POLARIS_REQUIRE_KERNEL_GATE";

/// 当前环境是否禁起核。与 `POLARIS_REQUIRE_KERNEL_GATE=1` 冲突、或取值不认识 ⇒ panic。
pub fn kernel_run_forbidden() -> bool {
    let forbidden = match std::env::var(NO_KERNEL_RUN_ENV).as_deref() {
        Err(_) | Ok("") | Ok("0") => false,
        Ok("1") => true,
        Ok(other) => panic!("{NO_KERNEL_RUN_ENV} 只认 1 / 0 / 未设，收到 {other:?}"),
    };
    let required = std::env::var(REQUIRE_ENV).is_ok_and(|v| v == "1");
    assert!(
        !(forbidden && required),
        "{NO_KERNEL_RUN_ENV}=1 与 {REQUIRE_ENV}=1 同时设置：一个禁起核、一个要求内核门必须生效，\
         二者互斥 —— 放任任一胜出都是「要求跑却被跳过」的空跑绿。去掉其中一个再跑。"
    );
    forbidden
}

/// 起核用例入口第一行调用：`true` = 可以起核；`false` = 本机禁起核，已打印并记账，调用方直接 `return`。
pub fn kernel_run_or_skip(what: &str) -> bool {
    if !kernel_run_forbidden() {
        return true;
    }
    eprintln!(
        "⛔ 跳过起核用例「{what}」：{NO_KERNEL_RUN_ENV}=1（本机禁起 sing-box run，覆盖交给 CI）"
    );
    if let Ok(path) = std::env::var(SKIP_LOG_ENV) {
        if !path.is_empty() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("{SKIP_LOG_ENV}={path} 打不开：{e}"));
            // 单次 write 一整行（O_APPEND）：并行用例各写各的行，不交错。
            file.write_all(format!("{what}\n").as_bytes())
                .unwrap_or_else(|e| panic!("{SKIP_LOG_ENV}={path} 写入失败：{e}"));
        }
    }
    false
}

/// 断言当前允许起核；禁起核时 panic。给不经 `Command` 起核的路径（如把真核注入运行时）用。
pub fn forbid_kernel_run(what: &str) {
    assert!(
        !kernel_run_forbidden(),
        "{what}：{NO_KERNEL_RUN_ENV}=1 下仍走到了起核路径 —— 该用例入口漏调了 kernel_run_or_skip，\
         或它本就不该在禁起核的环境里跑"
    );
}

/// 往 `cmd` 末尾追加 `run` 子命令。**全仓测试源码里唯一允许拼 `run` 的地方**。
pub fn with_run(mut cmd: Command) -> Command {
    forbid_kernel_run("拼 sing-box run 子命令");
    cmd.arg("run");
    cmd
}
