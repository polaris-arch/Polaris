//! 🔴 起核单入口门（源码级）：全仓**测试源码**里拼 `sing-box run` 子命令的调用，只允许出现在
//! `tests/support/kernel_run.rs` 的 `with_run` 一处。
//!
//! # 守的是什么
//!
//! 本机门禁禁起核（`POLARIS_NO_KERNEL_RUN=1`，陈先生决策 2026-09-25）靠的是「所有起核都经同一个
//! helper」：helper 在禁起核时跳过 / panic。新写一个绕过 helper 的 `.arg("run")`，本机 `gate-rust.sh`
//! 就会**真起核**，而且什么都不报 —— 那正是本门存在的理由。「起核走不走 helper」没有任何运行期
//! 表征（CI 上两种写法都照常起核、照常绿），唯一的观察面是源码本身。
//!
//! # 判据形状
//!
//! - 取材面：`crates/` 与 `src-tauri/` 下路径含 `/tests/` 段的全部 `.rs`（仓内「`<dir>/tests/` 恒为
//!   测试」由 `src-tauri/tests/no_inline_test_mods.rs` 钉死，故按路径取测试面是完备的）。
//! - 命中 = 代码里的 `.arg(…)` / `.args(…)` 调用，其**实参区间**里出现字面量 `"run"`。
//!   括号配对在「注释+字面量全抹」的净化面上做（字面量里的括号不干扰配对），`"run"` 在「只抹注释」的
//!   净化面上认（两份净化等长、偏移一致）。于是注释、文档、错误消息模板里写的 `.arg("run")` 都不命中。
//! - 生产代码（helper 各平台 spawner）不在取材面：那是产品起核路径，不是测试。
//!
//! # 已知射程上限（如实记）
//!
//! 把 `"run"` 先存进变量 / 常量再传给 `.arg(x)`、或经非 `arg/args` 的 API 起核，本门看不见。
//! 兜底是 helper 的第二道闸之外的人眼 review；`with_run` 的注释与本门失败信息都写明了唯一入口。

use polaris_source_probe::{mask_comments, mask_comments_and_strings, repo_dir_files_in};

const HELPER: &str = "crates/config-engine/tests/support/kernel_run.rs";

/// `open` 处是 `(`，返回配对的 `)` 偏移（输入须是已抹字面量/注释的纯代码面）。
fn match_paren(code: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, &c) in code.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn line_of(src: &str, off: usize) -> usize {
    src[..off].bytes().filter(|&b| b == b'\n').count() + 1
}

/// 源码里拼 `run` 子命令的调用点（行号，升序）。
fn run_sites(src: &str) -> Vec<usize> {
    let code = mask_comments_and_strings(src);
    let lits = mask_comments(src);
    assert_eq!(code.len(), src.len(), "净化面必须与原文等长（偏移守恒）");
    assert_eq!(lits.len(), src.len(), "净化面必须与原文等长（偏移守恒）");
    let mut sites = Vec::new();
    for needle in [".arg(", ".args("] {
        let mut from = 0;
        while let Some(k) = code[from..].find(needle) {
            let open = from + k + needle.len() - 1;
            let close = match_paren(code.as_bytes(), open).unwrap_or(code.len());
            if lits[open..close].contains("\"run\"") {
                sites.push(line_of(src, open));
            }
            from = open + 1;
        }
    }
    sites.sort_unstable();
    sites
}

fn test_face() -> Vec<(String, String)> {
    let mut face = Vec::new();
    for root in ["crates", "src-tauri"] {
        face.extend(
            repo_dir_files_in(env!("CARGO_MANIFEST_DIR"), root, "rs")
                .into_iter()
                .filter(|(rel, _)| rel.contains("/tests/")),
        );
    }
    face
}

fn read_repo(rel: &str) -> String {
    polaris_source_probe::repo_file_in(env!("CARGO_MANIFEST_DIR"), rel)
}

#[test]
fn every_kernel_run_in_test_sources_goes_through_the_helper() {
    let face = test_face();
    // 取材面自检：空/过窄的面会让下面的循环恒真。今天实测 ~300 个文件，下限留余量。
    assert!(
        face.len() > 200,
        "取材面只有 {} 个文件 —— 路径过滤坏了",
        face.len()
    );
    assert!(
        face.iter().any(|(rel, _)| rel == HELPER),
        "取材面里没有 helper 本身（{HELPER}）—— 面取错了"
    );

    let mut helper_sites = Vec::new();
    let mut bypasses = Vec::new();
    for (rel, src) in &face {
        let sites = run_sites(src);
        if rel == HELPER {
            helper_sites = sites;
        } else {
            bypasses.extend(sites.into_iter().map(|l| format!("  · {rel}:{l}")));
        }
    }
    // 正面断言：检测器在真实代码上认得出唯一那处（否则下面的「零绕过」可能只是检测器瞎了）。
    assert_eq!(
        helper_sites.len(),
        1,
        "{HELPER} 里应恰有 1 处拼 run 的调用（with_run），实测 {helper_sites:?}"
    );
    assert!(
        bypasses.is_empty(),
        "测试源码里出现了绕过 kernel_run::with_run 的 `run` 子命令拼接 —— 本机禁起核（POLARIS_NO_KERNEL_RUN=1）\
         对它无效，gate-rust.sh 会真起核：\n{}\n改成 `with_run(<cmd>)`，并在用例入口先调 `kernel_run_or_skip`。",
        bypasses.join("\n")
    );
}

/// 已知起核用例全部接上 helper（正面断言：防「删掉起核调用」也能让上一条变绿的误读）。
#[test]
fn known_kernel_run_tests_are_wired_to_the_helper() {
    // (文件, 用例入口必须调 kernel_run_or_skip 的次数 = 该文件的起核用例数)
    let skip_wired = [
        ("crates/config-engine/tests/network_profile_runtime.rs", 5),
        (
            "crates/config-engine/tests/subscription_update_guard_runtime.rs",
            1,
        ),
    ];
    for (rel, n) in skip_wired {
        let code = mask_comments_and_strings(&read_repo(rel));
        let tests = code.matches("#[test]").count();
        let skips = code.matches("kernel_run_or_skip(").count();
        assert_eq!(
            tests, n,
            "{rel}：起核用例数漂移（{tests} ≠ {n}），先确认新用例也经 helper 跳过"
        );
        assert_eq!(
            skips, n,
            "{rel}：{n} 个起核用例应各调一次 kernel_run_or_skip，实测 {skips}"
        );
        assert!(code.contains("with_run("), "{rel}：起核不再经 with_run");
    }

    let ps = mask_comments_and_strings(&read_repo(
        "src-tauri/src/runtime/proxy/tests/process_supervision.rs",
    ));
    assert_eq!(
        ps.matches("kernel_run::with_run(").count(),
        2,
        "process_supervision 的两个直起核孤儿都应经 with_run"
    );

    // src-tauri 经 ProxyRuntime 起核的真核用例统一走 inject_real_core_for_test：闸必须在那里。
    let proxy = mask_comments_and_strings(&read_repo("src-tauri/src/runtime/proxy.rs"));
    let at = proxy
        .find("fn inject_real_core_for_test(")
        .expect("inject_real_core_for_test 改名/删了，先确认真核注入点再动本门");
    let body_end = proxy[at..].find("\n    }\n").expect("函数没有收口") + at;
    assert!(
        proxy[at..body_end].contains("kernel_run::forbid_kernel_run("),
        "inject_real_core_for_test 不再经 forbid_kernel_run —— ProxyRuntime 真核用例在禁起核时会真起核"
    );

    // 单一事实源：src-tauri 引入的是同一份文件，不是另写一份判定。
    let runtime = mask_comments(&read_repo("src-tauri/src/runtime.rs"));
    assert!(
        runtime.contains(
            "#[path = \"../../crates/config-engine/tests/support/kernel_run.rs\"]\npub(crate) mod kernel_run;"
        ),
        "src-tauri 必须经 #[path] 引入 {HELPER}，不得另写一份跳过判定"
    );
}

/// 本机门禁设开关、CI 不设：开关只能出现在 gate-rust.sh 的非 CI 分支里。
#[test]
fn only_the_local_gate_sets_the_no_kernel_run_switch() {
    let gate = read_repo("scripts/gate-rust.sh");
    let code: String = gate
        .lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for needle in [
        "if [ -z \"${GITHUB_ACTIONS:-}\" ]; then",
        "export POLARIS_NO_KERNEL_RUN=1",
        "export POLARIS_KERNEL_RUN_SKIP_LOG=",
        "跳过 ${",
    ] {
        assert!(
            code.contains(needle),
            "scripts/gate-rust.sh 缺少 `{needle}`"
        );
    }
    // 取材面 = 工作流目录下全部 .yml（不写死名单：main 与 mobile 的工作流集合不同，
    // 写死会在一侧读到不存在的文件，也会漏掉以后新增的工作流）。
    let workflows = repo_dir_files_in(env!("CARGO_MANIFEST_DIR"), ".github/workflows", "yml");
    assert!(
        workflows.iter().any(|(rel, _)| rel.ends_with("ci.yml")),
        "工作流取材面没有读到 ci.yml —— 目录或扩展名判据坏了（防空跑）"
    );
    for (rel, text) in &workflows {
        assert!(
            !text.contains("POLARIS_NO_KERNEL_RUN"),
            "{rel} 出现了 POLARIS_NO_KERNEL_RUN —— CI 必须照常起核（覆盖交给 CI 的前提）"
        );
    }
}

/// 切点自检：注释 / 字符串 / 原始字符串里的 `.arg("run")` 不命中；真调用（单参与数组）命中；
/// 其它参数与 `"run"` 出现在非 arg 调用里（如 argv 夹具）不命中。
#[test]
fn run_site_detector_slice_self_check() {
    let src = r####"
// cmd.arg("run")
/// 文档：.args(["run", "-c"])
/* 块注释 .arg("run") /* 嵌套 */ */
let msg = "请用 .arg(\"run\") 起核";
let raw = r#".arg("run")"#;
let argv = cmd(&["/usr/bin/sing-box", "run", "-c", "a"]);
let v = vec!["run".to_string()];
x.arg("--disable-color").arg("check");
a.arg("run");
b.args(["run", "-c", cfg]);
c.args([
    "run",
    "-c",
]);
d.arg(if paren(1) { "run" } else { "check" });
"####;
    assert_eq!(run_sites(src), vec![10, 11, 12, 16]);
}
