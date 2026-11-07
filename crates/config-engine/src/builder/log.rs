//! sing-box 日志配置生成；SC02 在 disabled 时保留可信 host 输出锚点元数据。
//!
//! 纯函数：读 UserConfig 子集 + privacyMode + 平台 + 日志文件路径（注入，不硬编码）。
//! 当前 producer 字节契约由 golden/config-snapshot 守卫验证；本 builder 不执行文件 I/O。

#![forbid(unsafe_code)]

use crate::singbox::LogConfig;
use crate::user_config::{LogLevel, ProxyModeType};
// Platform 作单一真值（polaris-helper-proto）：本 builder 不再自定义平台枚举。
// 历史的 Darwin/Win32/Linux 三变体统一映射为 proto 的 Mac/Win/Linux；Other 视同 Linux（Unix 路径日志）。
use polaris_helper_proto::Platform;

/// buildLogConfig 依赖注入：日志文件路径（TUN 模式 output 字段值，由调用方提供——
/// 生产环境 = UserData/singbox.log，对拍 fixture = 固定假路径）。
#[derive(Debug, Clone)]
pub struct LogBuildDeps<'a> {
    pub privacy_mode: bool,
    pub platform: Platform,
    /// TUN 的可信固定输出锚点。None = 不让 Core 持固定 fd；实际 App 使用 SubscribeLog + bounded sink。
    pub log_file_path: Option<&'a str>,
}

/// UserConfig 中 buildLogConfig 消费的子集（上游 `singbox-log-builder.ts:15` 入参 config 的投影）。
#[derive(Debug, Clone, Default)]
pub struct LogConfigInput {
    pub log_level: LogLevel,
    pub disable_log_file: bool,
    pub proxy_mode_type: ProxyModeType,
}

/// 生成日志配置；disabled 保留已有可信 host 锚点，启用无需重新授权任意路径。
///
/// 行为契约（对拍锁）：
/// 1. level = effectiveLogLevel(config.logLevel || 'info', privacyMode) —— 隐私模式抬 ≥warn。
/// 2. timestamp 恒 true。
/// 3. disableLogFile → disabled=true；仍保留可信 host 注入的固定 output 元数据（核不打开它）。
/// 4. TUN 模式（三平台）→ output = 日志文件路径；manual/systemProxy 模式不写文件（stderr 直喂）。
///
/// `output` 仅来自 host 注入的依赖，不从 renderer 用户配置读取路径。
/// disabled 只保留这个名字，Core 的稳定 logger owner 不在构造/启动时打开它。
/// 实际 App 启动固定使用 None（W26）：SubscribeLog relay 将核日志交给 App 的 bounded writer，
/// 不能为了热启用而恢复一个无法响应轮转的固定 Core fd。显式 Some 仍受 Core 原路径/文件存在
/// 检查；本 builder 本身不创建目录、文件或占位日志。
pub fn build_log_config(input: &LogConfigInput, deps: &LogBuildDeps) -> LogConfig {
    // 1. level（隐私模式从源头不让 sing-box 记录连接明细）。
    let level = input.log_level.effective(deps.privacy_mode);
    let mut cfg = LogConfig {
        level: level_to_string(level),
        timestamp: true,
        output: None,
        disabled: None,
    };

    // Disabled suppresses emission/opening, not the immutable host output anchor.
    // This builder does no I/O; Core keeps the name inert until explicit enable.
    if input.disable_log_file {
        cfg.disabled = Some(true);
    }

    // 3. TUN 模式（三平台）→ output 写文件。
    // Polaris 谓词：isTunMode && (darwin || win32 || linux)。三平台全覆盖 → 实质 = isTunMode，
    // 但保留平台枚举检查忠实移植（防未来平台分支差异）。
    // Platform::Other 视同 Linux（未知平台按 Unix 路径保守写文件，TUN 下 stdout 不可捕获）。
    //
    // Platform::Android **必须在列**（2026-09-04 K10）：Android 上核是进程内 libbox，stdout 更加
    // 不可捕获（没有子进程可接管道），日志落盘是导出诊断报告拿到核原文的唯一途径。它此前是靠
    // `Platform::parse("android") == Other` 顺带盖住的 —— 那条腿在本次给 Android 具名之后就断了，
    // 不在这里补上就是一次由本次改动**引入**的回归（而不是原有缺陷）。
    // 全变体皆 true 仍保留枚举检查：忠实移植 + 让未来的平台差异有地方落。
    //
    // 接管方式取**本平台生效值**（[`ProxyModeType::effective_on`]）而不是磁盘上存的那个：Android 的
    // `proxy_mode_type` 缺省是 `SystemProxy`，照裸值判会让「全新安装 / 备份恢复的 Android 客户端
    // 一条核日志都不落盘」，而上一段刚说过落盘是那里拿到核原文的**唯一**途径 —— 两条合起来正好
    // 抵消，且症状是「导出诊断里核日志是空的」，离成因很远。
    //
    // Platform::Ios **同样必须在列**（2026-09-06，加 `Platform::Ios` 变体时补）：这一格与上面
    // Android 那一段是**逐字同一条回归**，且成因形态更隐蔽 —— `matches!` 漏一个变体只是求值
    // `false`，**编译器一句话都不说**（本批 40 处穷举 match 里一处都拦不住它）。
    // iOS 的依据比 Android 更强一档：核跑在 NE 扩展进程里，app 进程连它的 stdout 都不在同一个
    // 进程树上，落盘（写进 App Group 共享容器）是导出诊断拿到核原文的唯一途径。
    // 漏掉它的症状与 Android 那次相同：导出诊断里核日志是空的，离成因很远。
    //
    // 本行由 `crates/config-engine/src/builder/tests/log.rs` 的
    // `log_output_allowlist_covers_every_platform_variant` 逐变体钉死（变异 B：删掉这里的
    // `Platform::Ios` 即红）。
    let writes_log_to_file = input.proxy_mode_type.effective_on(deps.platform).is_tun()
        && matches!(
            deps.platform,
            Platform::Mac
                | Platform::Win
                | Platform::Linux
                | Platform::Android
                | Platform::Ios
                | Platform::Other
        );

    if writes_log_to_file {
        if let Some(path) = deps.log_file_path {
            cfg.output = Some(path.to_string());
        }
    }

    cfg
}

/// `LogLevel` → sing-box JSON 字符串（serde lowercase rename 的手动镜像）。
///
/// 放宽到 `pub` 而不是让第二个调用方另写一份映射：测速临时核（`src-tauri` 的
/// `commands::speedtest::temp_core_log_level`）要下发的是**同一个** sing-box `log.level` 字段，
/// 两份映射一旦漂移，表现是「主核按用户档记日志、临时核记的是另一档」而两边都「能跑」。
#[must_use]
pub fn level_to_string(level: LogLevel) -> String {
    match level {
        LogLevel::Debug => "debug",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
        LogLevel::Fatal => "fatal",
    }
    .to_string()
}

#[cfg(test)]
mod tests;
