//! 随包 `sing-box` 定位的单一事实源。
//!
//! 这些集成测试在不同平台各自执行；macOS 不能按「哪个文件先存在」选核，
//! 因为打包目录同时带 arm64/x64 两份二进制。package 优先传入打包目标，不能只看 runner 架构。
//!
//! 本模块同时持有**随包核平台枚举**（[`CORE_MATRIX`]）：凡「按平台把盘上的核看一遍」的门
//! 都必须从它派生覆盖轴。两个门各写一份平台名单的后果不是「两份都在」，而是其中一份
//! 悄悄少一格 —— 而且失败信息会把人指向错误的方向（表里没有的平台，红不出来）。

//! # 为什么整模块 `allow(dead_code, unused_imports)`
//!
//! `tests/support/` 是**菜单**，不是库：Cargo 把它编进每一个 `tests/*.rs` 集成测试二进制，
//! 而每个门只点自己要的那几样。于是「本二进制没用到某个 helper」是这个模块的常态，
//! 不是缺陷信号 —— 加一个新门就会让其余没用到的项在 `-D warnings` 下全体转红
//! （CI 跑的正是 `cargo clippy --workspace --all-targets -- -D warnings`）。
//!
//! 代价如实记：真正废弃的 helper 不会再被 lint 抓到，只能靠改动时人眼看。
//! 换成逐项 `#[allow]` 并不更强 —— 那样每加一个门仍要去补一轮 attribute，
//! 漏补的表现同样是 CI 红，而不是「发现了死代码」。
#![allow(dead_code, unused_imports)]

use std::path::{Path, PathBuf};
use std::process::Command;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("CARGO_MANIFEST_DIR 应形如 <repo>/crates/config-engine")
        .to_path_buf()
}

/// 随包核平台枚举里的一行。
///
/// 一行 = 一个平台的全部身份：两套标签（`key` / `gate_target`）、盘上路径、以及构建面期望值。
/// 构建面那三个字段（`goos` / `goarch` / `cgo`）与 `extra_tags` 只被 `core_build_matrix` 消费，
/// 但它们属于**同一行**，拆出去就又变成两份名单。
pub struct CoreBuild {
    /// 与 `scripts/fetch-core.mjs` 的 `TARGETS[].key` 同名。
    pub key: &'static str,
    /// 与 `POLARIS_KERNEL_GATE_TARGET` / package job 的目标标签同名（拼写与 `key` 不同）。
    pub gate_target: &'static str,
    pub rel: &'static str,
    pub goos: &'static str,
    pub goarch: &'static str,
    pub cgo: &'static str,
    /// 逐平台额外的 build tag。今天只有一条：`with_purego` 只出现在 linux / windows，
    /// mac 两份没有（mac 走 `CGO_ENABLED=1`，用不着 purego 那套无 cgo 兜底）。
    pub extra_tags: &'static [&'static str],
}

/// 随包核的四个平台 —— **全仓唯一一份名单**。
///
/// 消费点：`core_build_matrix`（构建面逐平台比对）、`core_dep_fingerprint`（依赖指纹逐平台比对）、
/// [`bundled_core_candidates_for`]（当前打包目标那一份的路径）。
pub const CORE_MATRIX: &[CoreBuild] = &[
    CoreBuild {
        key: "linux",
        gate_target: "linux",
        rel: "resources/linux/sing-box",
        goos: "linux",
        goarch: "amd64",
        cgo: "0",
        extra_tags: &["with_purego"],
    },
    CoreBuild {
        key: "win",
        gate_target: "windows",
        rel: "resources/win/sing-box.exe",
        goos: "windows",
        goarch: "amd64",
        cgo: "0",
        extra_tags: &["with_purego"],
    },
    CoreBuild {
        key: "mac-arm64",
        gate_target: "macos-arm64",
        rel: "resources/mac-arm64/sing-box",
        goos: "darwin",
        goarch: "arm64",
        cgo: "1",
        extra_tags: &[],
    },
    CoreBuild {
        key: "mac-x64",
        gate_target: "macos-x64",
        rel: "resources/mac-x64/sing-box",
        goos: "darwin",
        goarch: "amd64",
        cgo: "1",
        extra_tags: &[],
    },
];

pub fn kernel_gate_required() -> bool {
    std::env::var("POLARIS_REQUIRE_KERNEL_GATE").is_ok_and(|v| v == "1")
}

/// 盘上真实存在的那几份核（按 [`CORE_MATRIX`] 顺序），只 stat 不读字节。
///
/// 故意**不**在这里解析二进制：构建面门要 buildinfo 设置区、依赖指纹门要 modinfo，
/// 各读各的（单份 ~50MB，读完即弃，不驻留 4 份）。共享的是**覆盖轴**，不是解析器。
pub fn present_cores() -> Vec<(&'static CoreBuild, PathBuf)> {
    let root = repo_root();
    CORE_MATRIX
        .iter()
        .filter_map(|c| {
            let path = root.join(c.rel);
            path.is_file().then_some((c, path))
        })
        .collect()
}

/// 缺核时的统一处置：`POLARIS_REQUIRE_KERNEL_GATE=1` 下缺一即红，否则返回 `false` 让调用门打 warn。
///
/// `what` 填进失败信息末尾的括号，用来说明是哪一道门没跑全。
pub fn require_all_present(present: &[(&'static CoreBuild, PathBuf)], what: &str) -> bool {
    if present.len() == CORE_MATRIX.len() {
        return true;
    }
    let missing: Vec<&str> = CORE_MATRIX
        .iter()
        .filter(|c| !present.iter().any(|(p, _)| p.key == c.key))
        .map(|c| c.key)
        .collect();
    assert!(
        !kernel_gate_required(),
        "POLARIS_REQUIRE_KERNEL_GATE=1 但盘上缺这些平台的随包核：{} —— \
         打包腿的 `node scripts/fetch-core.mjs`（不传 --platform = 全平台）是不是失败了？\
         （{what}）",
        missing.join(", ")
    );
    false
}

/// 打包工作流传入的目标标签优先；本地/非 package 调用才按宿主推断。
pub fn kernel_gate_target() -> String {
    if let Ok(target) = std::env::var("POLARIS_KERNEL_GATE_TARGET") {
        assert!(
            matches!(target.as_str(), "linux" | "windows" | "macos-arm64" | "macos-x64"),
            "POLARIS_KERNEL_GATE_TARGET 必须是 linux/windows/macos-arm64/macos-x64，收到 {target:?}"
        );
        return target;
    }
    if cfg!(target_os = "windows") {
        "windows".to_owned()
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "macos-arm64".to_owned()
    } else if cfg!(target_os = "macos") {
        "macos-x64".to_owned()
    } else {
        "linux".to_owned()
    }
}

/// 打包目标 → 唯一允许执行/读取的随包核相对路径。
///
/// 返回 slice 是为调用端保留「候选」的统一接口；每个目标只有一个元素，绝不允许跨架构回退。
/// 路径从 [`CORE_MATRIX`] 取，不在这里再写一遍 —— 两份路径表分叉的表现是「门读错了文件」。
pub fn bundled_core_candidates_for(target: &str) -> &'static [&'static str] {
    let build = CORE_MATRIX
        .iter()
        .find(|c| c.gate_target == target)
        .unwrap_or_else(|| panic!("未知随包核目标：{target}"));
    std::slice::from_ref(&build.rel)
}

/// 当前打包目标对应的随包核；缺失时不尝试另一架构。
pub fn bundled_core() -> Option<PathBuf> {
    bundled_core_candidates_for(&kernel_gate_target())
        .iter()
        .map(|path| repo_root().join(path))
        .find(|path| path.is_file())
}

/// macOS x64 package job 实际在 arm64 runner 上，只有它需要 Rosetta。
pub fn target_needs_rosetta(target: &str) -> bool {
    match target {
        "linux" | "windows" | "macos-arm64" => false,
        "macos-x64" => true,
        _ => panic!("未知随包核目标：{target}"),
    }
}

/// 为显式目标核构造命令。macOS x64 交叉构建 job 运行在 arm64 runner，必须经 Rosetta。
pub fn command_for_core_target(target: &str, core: &Path) -> Command {
    if target_needs_rosetta(target) {
        let mut command = Command::new("/usr/bin/arch");
        command.arg("-x86_64").arg(core);
        command
    } else {
        Command::new(core)
    }
}

/// 为当前 package/local 目标核构造命令。
#[allow(dead_code)] // core_dep_fingerprint 单独编译此模块，只读二进制而不执行它。
pub fn command_for_core(core: &Path) -> Command {
    command_for_core_target(&kernel_gate_target(), core)
}

/// 缺核时的统一处置：硬化门红，开发机明确跳过。
pub fn core_or_skip(what: &str) -> Option<PathBuf> {
    if let Some(core) = bundled_core() {
        return Some(core);
    }
    let required = std::env::var("POLARIS_REQUIRE_KERNEL_GATE").is_ok_and(|value| value == "1");
    assert!(
        !required,
        "POLARIS_REQUIRE_KERNEL_GATE=1 但盘上没有当前目标对应的随包核 —— \\
         打包腿的 `node scripts/fetch-core.mjs` 是不是失败了？（{what} 未执行）"
    );
    eprintln!(
        "⚠ 跳过 {what}：盘上没有当前目标对应的随包核（`.gitignore` 的 /resources/*）。\\
         跑 `node scripts/fetch-core.mjs` 后本门自动生效；打包腿带 POLARIS_REQUIRE_KERNEL_GATE=1 强制生效。"
    );
    None
}
