//! `test_support` 的自证。
//!
//! # 本文件的位置本身就是判据
//!
//! 它在 `src-tauri/src/test_support/tests/mod.rs` —— 也就是**全仓测试外移之后**每个测试实体
//! 会落到的那个深度。下面所有取材调用写的都是「`src-tauri/src/` 里的哪个文件」，与本文件
//! 在第几层**无关**：`crate_source("tray.rs")` 在这里读到的仍然是 `src-tauri/src/tray.rs`。
//!
//! 换成旧写法就不成立：在这个位置 `include_str!("tray.rs")` 指的是
//! `src-tauri/src/test_support/tests/tray.rs`。今天它不存在（编译期红），但 143 处站点里
//! 有 129 处是同目录裸文件名，搬家后**大量会撞上另一个真实存在的同名文件** —— 那才是
//! 真正的失败模式：编译过、门全绿、扫的却是别的文件。
//!
//! # 跨 crate 那一半
//!
//! `polaris-source-probe` 内部**不读** `env!("CARGO_MANIFEST_DIR")`（读了会解析成
//! `crates/source-probe`）。锚点由本 crate 的 wrapper 传入，而「传进去的确实是 src-tauri」
//! 由 source-probe 自测与 44 处真实调用间接覆盖。

use super::*;

#[cfg(unix)]
#[test]
fn created_directory_under_a_temp_alias_owns_its_canonical_path() {
    use std::os::unix::fs::symlink;

    let base = TestDir::new("polaris-test-support-alias-");
    let real_parent = base.join("real");
    let alias_parent = base.join("alias");
    std::fs::create_dir(&real_parent).unwrap();
    symlink(&real_parent, &alias_parent).unwrap();
    let owned_path;
    {
        let dir = TestDir::new_in("owned-", &alias_parent);
        owned_path = dir.path().to_path_buf();
        assert_eq!(owned_path, std::fs::canonicalize(&owned_path).unwrap());
        assert_eq!(owned_path.parent(), Some(real_parent.as_path()));
        std::fs::write(dir.join("sentinel"), b"owned").unwrap();
    }
    assert!(!owned_path.exists());
    assert!(alias_parent.is_symlink());
    assert!(real_parent.is_dir());
}

#[test]
fn created_directory_supports_file_url_roundtrips() {
    let dir = TestDir::new("polaris-test-support-file-url-");
    let path = dir.join("picked.md");
    std::fs::write(&path, b"picked file").unwrap();

    let picked = crate::commands::picked_file::tests::file_url_of(&path);
    let roundtrip = picked.into_path().unwrap();
    assert_eq!(roundtrip, path);
    assert_eq!(std::fs::read(roundtrip).unwrap(), b"picked file");
}

#[test]
fn retained_rule_file_can_be_read_using_builder_path_string() {
    use polaris_config_engine::builder::custom_rule_files::ext_rule_file_exists;

    let dir = TestDir::new("polaris-test-support-retained-rule-");
    let rules_dir = dir.join("tailnet-rules");
    std::fs::create_dir(&rules_dir).unwrap();
    let path = rules_dir.join("tailnet-ts1.json");
    let bytes = br#"{"version":3,"rules":[{"ip_cidr":["100.64.0.1/32"]}]}"#;
    std::fs::write(&path, bytes).unwrap();

    // The builder receives a directory string and appends '/' to its rule name.
    let builder_path = format!("{}/tailnet-ts1.json", rules_dir.display());
    assert!(ext_rule_file_exists(&builder_path));
    assert_eq!(std::fs::read(&builder_path).unwrap(), bytes);
    std::fs::remove_file(path).unwrap();
    assert!(!ext_rule_file_exists(&builder_path));
}

// ── method_body ──

/// **守卫的守卫**：证明 [`method_body`] 真的截在方法自己的括号上。
///
/// 没有这条，「我按括号配对封了顶」只是一句注释。
#[test]
fn method_body_stops_at_the_methods_own_brace() {
    let src = "impl X {\n    fn target(&self) {\n        if a { inside(); }\n    }\n\n    fn later(&self) {\n        outside();\n    }\n}\n";
    let body = method_body(src, "fn target(");
    assert!(body.contains("inside()"), "必须包含被守方法自己的函数体");
    assert!(
        !body.contains("outside()"),
        "**封顶失效**：切到了同一个 impl 的后续方法 → 守卫可被「删这里、加那里」骗过"
    );

    // 字符串里的花括号（`format!("{x}")` 之类）不得计入深度。
    let with_fmt = "impl X {\n    fn target(&self) {\n        log(\"a {b} c }\");\n        tail();\n    }\n\n    fn later(&self) {\n        outside();\n    }\n}\n";
    let body = method_body(with_fmt, "fn target(");
    assert!(
        body.contains("tail()"),
        "字符串字面量里的右花括号被误当作方法结束"
    );
    assert!(!body.contains("outside()"));

    // 行注释里的花括号同理。
    let with_comment = "impl X {\n    fn target(&self) {\n        // } 这不是结束\n        tail();\n    }\n\n    fn later(&self) {\n        outside();\n    }\n}\n";
    let body = method_body(with_comment, "fn target(");
    assert!(body.contains("tail()"));
    assert!(!body.contains("outside()"));

    // 🔴 **剥注释**：跳过注释里的花括号只决定「切到哪」，不决定「切出来的文本喂谁」。
    // 消费者全是 `contains` / `find` / `count()` 型判据，注释里的同名文本会替生产调用点作证。
    let fed_by_comment =
        "impl X {\n    fn target(&self) {\n        real_call();\n        // real_call() 只出现在整行注释里\n    }\n}\n";
    assert_eq!(
        method_body(fed_by_comment, "fn target(")
            .matches("real_call()")
            .count(),
        1,
        "整行注释里的锚点文本必须被剥掉，否则 `count()==N` 类判据可被注释充数"
    );
    let only_comment =
        "impl X {\n    fn target(&self) {\n        // real_call() 被删了，只剩这行注释\n    }\n}\n";
    assert!(
        !method_body(only_comment, "fn target(").contains("real_call()"),
        "**假绿**：生产调用点删光、只剩注释时，正面 `contains` 必须转红"
    );
}

/// 锚点消失必须 panic（转红），而不是返回空切片让断言恒真。
#[test]
#[should_panic(expected = "锚点消失")]
fn missing_anchor_panics_instead_of_silently_passing() {
    method_body("fn other() {\n}\n", "fn nonexistent(");
}

/// 🔴 [`write_executable_stand_in`] 落下的替身，在别的线程不停 `fork` 时也必须每次都起得来。
///
/// 旁边那些线程就是全量测试里「别的用例正好在 `fork`」的浓缩版。把落盘换回「本进程
/// `std::fs::write` + 加执行位」，本条在同样的轮数里稳定撞出十几次
/// `Text file busy (os error 26)`。
///
/// 并发度与轮数都有上界：子进程只是 `sh -c :`，不碰网络、不留状态。
#[cfg(unix)]
#[test]
fn executable_stand_in_always_starts_while_other_threads_fork() {
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};

    const FORKING_THREADS: usize = 4;
    const ROUNDS: usize = 100;

    let dir = TestDir::new("polaris-stand-in-race-");
    let stop = AtomicBool::new(false);
    let mut failures = Vec::new();
    std::thread::scope(|scope| {
        for _ in 0..FORKING_THREADS {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    let _ = Command::new("/bin/sh")
                        .args(["-c", ":"])
                        .stdin(Stdio::null())
                        .status();
                }
            });
        }
        for round in 0..ROUNDS {
            let stand_in = dir.join(format!("stand-in-{round}"));
            write_executable_stand_in(&stand_in, "#!/bin/sh\nexit 0\n");
            match Command::new(&stand_in).stdin(Stdio::null()).status() {
                Ok(status) if status.success() => {}
                Ok(status) => failures.push(format!("第 {round} 轮：{status}")),
                Err(error) => failures.push(format!("第 {round} 轮：{error}")),
            }
        }
        // 先收齐结果再断言：在这里 panic 会让上面那些线程永远等不到停止信号。
        stop.store(true, Ordering::Relaxed);
    });
    assert!(
        failures.is_empty(),
        "替身没能每次都起来（共 {} 次）：\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
