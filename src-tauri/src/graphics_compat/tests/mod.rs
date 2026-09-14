use super::*;

#[test]
fn explicit_false_disables() {
    let raw = r#"{"hardwareAcceleration":false}"#;
    assert!(should_disable_hardware_acceleration(Some(raw)));
}

#[test]
fn explicit_true_keeps_default_on() {
    let raw = r#"{"hardwareAcceleration":true}"#;
    assert!(!should_disable_hardware_acceleration(Some(raw)));
}

/// 正向语义核心：字段缺失 = 默认开 = 不禁。存量配置（升级前落盘的）必须逐字节行为不变。
#[test]
fn missing_field_keeps_default_on() {
    let raw = r#"{"language":"zh-CN"}"#;
    assert!(!should_disable_hardware_acceleration(Some(raw)));
}

/// 容错第一：绝不因脏值误禁用户的硬件加速（"false"/0/null 都不是 JSON false）。
#[test]
fn dirty_values_never_disable() {
    for raw in [
        r#"{"hardwareAcceleration":"false"}"#,
        r#"{"hardwareAcceleration":0}"#,
        r#"{"hardwareAcceleration":null}"#,
        r#"{"hardwareAcceleration":[]}"#,
        r#"{"hardwareAcceleration":{}}"#,
    ] {
        assert!(
            !should_disable_hardware_acceleration(Some(raw)),
            "脏值 {raw} 不得触发禁用"
        );
    }
}

/// 容错第一：文件缺失 / 空 / 损坏 / 顶层非对象 → 一律不禁，且绝不 panic。
#[test]
fn broken_config_never_disables_and_never_panics() {
    for raw in [
        None,
        Some(""),
        Some("   "),
        Some("{"),
        Some("not json at all"),
        Some("[1,2,3]"),
        Some("null"),
        Some(r#"{"hardwareAcceleration":fals"#),
    ] {
        assert!(!should_disable_hardware_acceleration(raw));
    }
}

/// 单向依赖：`windowEffects` 现已有行为消费（门控 vibrancy/Mica），但它**不得反向**影响
/// hardwareAcceleration 的 GPU 环境变量判定 —— 关特效不该连带禁掉硬件加速。
#[test]
fn window_effects_field_does_not_affect_hardware_acceleration() {
    let raw = r#"{"hardwareAcceleration":true,"windowEffects":false}"#;
    assert!(!should_disable_hardware_acceleration(Some(raw)));
    let raw = r#"{"hardwareAcceleration":false,"windowEffects":true}"#;
    assert!(should_disable_hardware_acceleration(Some(raw)));
}

/// 直接开关：`windowEffects === false` → 不上特效。
#[test]
fn window_effects_explicit_false_skips_effects() {
    assert!(!should_apply_window_effects(Some(
        r#"{"windowEffects":false}"#
    )));
}

/// 正向语义：显式 true / 字段缺失 → 上特效（存量配置逐字节不变）。
#[test]
fn window_effects_true_or_missing_applies_effects() {
    assert!(should_apply_window_effects(Some(
        r#"{"windowEffects":true}"#
    )));
    assert!(should_apply_window_effects(Some(r#"{"language":"zh-CN"}"#)));
}

/// 第二个否决位：逃生门开着（hardwareAcceleration=false）→ 即便 windowEffects 为 true / 缺失也不上特效
/// （vibrancy/Mica 本身就是合成层负载，正是用户要躲的白屏向量）。
#[test]
fn hardware_acceleration_off_also_skips_effects() {
    assert!(!should_apply_window_effects(Some(
        r#"{"hardwareAcceleration":false,"windowEffects":true}"#
    )));
    assert!(!should_apply_window_effects(Some(
        r#"{"hardwareAcceleration":false}"#
    )));
}

/// 真值表全枚举：两个否决位的 4 种组合，仅「两个都不是 false」才上特效。
/// 覆盖所有逃逸路径 —— 把实现改成任一单字段判定、或把 && 换成 ||，都会有格子翻红。
#[test]
fn window_effects_truth_table() {
    for (hw, we, expect) in [
        (true, true, true),
        (true, false, false),
        (false, true, false),
        (false, false, false),
    ] {
        let raw = format!(r#"{{"hardwareAcceleration":{hw},"windowEffects":{we}}}"#);
        assert_eq!(
            should_apply_window_effects(Some(&raw)),
            expect,
            "hardwareAcceleration={hw} windowEffects={we} 应 apply_effects={expect}"
        );
    }
}

/// 容错第一：配置缺失 / 空 / 损坏 / 顶层非对象 → 回落默认开 = 上特效，且绝不 panic。
/// （逃生门自己崩了就没救了；脏值绝不误关用户的窗口特效。）
#[test]
fn broken_config_keeps_effects_on_and_never_panics() {
    for raw in [
        None,
        Some(""),
        Some("   "),
        Some("{"),
        Some("not json at all"),
        Some("[1,2,3]"),
        Some("null"),
        Some(r#"{"windowEffects":"false"}"#),
        Some(r#"{"windowEffects":0}"#),
        Some(r#"{"windowEffects":null}"#),
    ] {
        assert!(should_apply_window_effects(raw), "{raw:?} 不得关掉窗口特效");
    }
}

#[test]
fn read_config_raw_missing_file_is_none() {
    let dir = std::env::temp_dir().join("polaris-graphics-compat-absent");
    assert!(read_config_raw(&dir).is_none());
}

// ── Windows：经 WebView2 API 下发的启动参数 ──────────────────────────────────────────────

/// wry 在 Polaris 建窗属性下自己拼出的默认参数，**逐项独立抄录**（不引用实现里的常量）：实现串漏掉任何
/// 一项，下面的断言能点名缺的是哪一项。出处见 `graphics_compat.rs` 的推导注释块；随依赖升级重读时一并核对。
const EXPECTED_WRY_DEFAULT_ARGS: [&str; 2] = [
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection",
    "--autoplay-policy=no-user-gesture-required",
];

/// 开启（默认路径）→ `None`：不替换 wry 默认串，行为与改动前逐字节相同。
#[test]
fn hardware_acceleration_on_passes_no_browser_args() {
    assert_eq!(webview2_browser_args_for(false), None);
}

/// 关闭 → 恰好一个 `--disable-gpu`，且 wry 默认串的每一项都在（`Some` 会整体替换默认串，漏一项 = 静默丢设置）。
#[test]
fn hardware_acceleration_off_keeps_every_wry_default_and_adds_disable_gpu_once() {
    let args = webview2_browser_args_for(true).expect("关硬件加速必须下发启动参数");
    let tokens: Vec<&str> = args.split_whitespace().collect();

    assert_eq!(
        tokens.iter().filter(|t| **t == "--disable-gpu").count(),
        1,
        "`--disable-gpu` 必须恰好出现一次：{args}"
    );
    for item in EXPECTED_WRY_DEFAULT_ARGS {
        assert!(
            tokens.contains(&item),
            "关硬件加速的启动参数缺了 wry 默认项 `{item}`（`additional_browser_args` 为 Some 时 wry 不再拼默认串，\
             缺项 = 关硬件加速的用户静默丢掉这一项）。实际：{args}"
        );
    }
    assert_eq!(
        tokens.len(),
        EXPECTED_WRY_DEFAULT_ARGS.len() + 1,
        "启动参数里出现了默认串与 `--disable-gpu` 之外的项（例如代理）：{args}"
    );
}

/// 进程级入口与纯判定同口径：定格后读到的就是纯判定对定格值的结果。
///
/// 进程级 `OnceLock` 在测试二进制里全局共享，别的测试不碰它；这里只断言「入口 == 纯判定(定格值)」，
/// 不假设定格的是哪一个值（并行跑时谁先定格不可控）。
#[test]
fn process_level_args_follow_the_frozen_switch() {
    let frozen = freeze_hardware_acceleration(Some(r#"{"hardwareAcceleration":false}"#));
    assert_eq!(
        webview_additional_browser_args(),
        webview2_browser_args_for(frozen)
    );
    // 重复定格不改判定。
    assert_eq!(
        freeze_hardware_acceleration(Some(r#"{"hardwareAcceleration":true}"#)),
        frozen
    );
    assert_eq!(freeze_hardware_acceleration(None), frozen);
}

/// `Cargo.lock` 里 `name` 包的全部版本（`[[package]]` 块内紧随 `name` 的 `version` 行）。
fn locked_versions(lock: &str, name: &str) -> Vec<String> {
    let name_line = format!("name = \"{name}\"");
    let mut out = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line.trim() == name_line {
            let version = lines
                .next()
                .and_then(|l| l.trim().strip_prefix("version = \""))
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or_else(|| panic!("Cargo.lock 里 `{name_line}` 的下一行不是 version"));
            out.push(version.to_string());
        }
    }
    out
}

/// 🔴 **漂移门**：推导默认串所依赖的 crate 版本一动就红，逼人回去重读源码。
#[test]
fn webview2_default_args_were_reviewed_against_the_locked_versions() {
    let lock = crate::test_support::repo_file("Cargo.lock");
    for (name, reviewed) in WEBVIEW2_DEFAULT_ARGS_REVIEWED_AGAINST {
        let locked = locked_versions(&lock, name);
        assert_eq!(
            locked,
            [reviewed],
            "\n`{name}` 在 Cargo.lock 里的版本与 `graphics_compat::WEBVIEW2_DEFAULT_ARGS_REVIEWED_AGAINST` 记录的 \
             `{reviewed}` 不一致（或锁了多个版本）。\n\
             关硬件加速时 Polaris 手工传的 WebView2 启动参数会**整体替换** wry 的默认串。放行前：重读新版 \
             `wry/src/webview2/mod.rs` 的 `create_environment` 里拼默认串的那段（`additional_browser_args` 为 None \
             时），以及 tauri-runtime-wry 建 `WebViewBuilder` 时是否新调 `with_autoplay` / 新喂 `proxy_config`；\
             逐项核对 `WEBVIEW2_ARGS_HARDWARE_ACCELERATION_OFF` 与本测试的 `EXPECTED_WRY_DEFAULT_ARGS`，\
             有差异先改串，**最后**才改版本号。只改版本号 = 关硬件加速的用户静默丢掉 msSmartScreenProtection 之类的设置。"
        );
    }
}

/// 版本提取器自检：夹具逐形态对差（没有这一格，漂移门的绿可能只是提取器什么都没读到）。
#[test]
fn locked_versions_self_check() {
    let lock = "[[package]]\nname = \"wry\"\nversion = \"0.55.1\"\nsource = \"x\"\n\n\
                [[package]]\nname = \"wry-extra\"\nversion = \"9.9.9\"\n\n\
                [[package]]\nname = \"tauri-runtime-wry\"\nversion = \"2.11.4\"\n";
    assert_eq!(
        locked_versions(lock, "wry"),
        ["0.55.1"],
        "前缀同名包不得误算"
    );
    assert_eq!(locked_versions(lock, "tauri-runtime-wry"), ["2.11.4"]);
    assert!(locked_versions(lock, "absent").is_empty());
    let twice = format!("{lock}\n[[package]]\nname = \"wry\"\nversion = \"0.56.0\"\n");
    assert_eq!(locked_versions(&twice, "wry"), ["0.55.1", "0.56.0"]);
}

/// 🔴 conf 不得声明 `additionalBrowserArgs`：主窗 `from_config` 会带上它（`tauri-runtime-2.11.3/src/webview.rs:483-485`），
/// 开启态另外三个窗走 wry 默认 ⇒ 同一 data directory 参数不一致 ⇒ 后建的窗建环境失败；关闭态又会被建窗点
/// 覆盖掉 —— 两态都不是声明者想要的。启动参数的唯一真值是 `graphics_compat::webview_additional_browser_args`。
#[test]
fn tauri_conf_does_not_declare_additional_browser_args() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut scanned = Vec::new();
    for entry in std::fs::read_dir(dir).expect("读 src-tauri 目录") {
        let name = entry
            .expect("目录项")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if !(name.starts_with("tauri") && name.ends_with(".json")) {
            continue;
        }
        let text = crate::test_support::crate_file(&name);
        for key in ["additionalBrowserArgs", "additional-browser-args"] {
            assert!(
                !text.contains(key),
                "`src-tauri/{name}` 声明了 `{key}`。WebView2 启动参数必须由 \
                 `graphics_compat::webview_additional_browser_args` 统一下发（四个建窗点同值），conf 里另写一份\
                 会让主窗与其它窗参数不一致 ⇒ 建环境失败。"
            );
        }
        scanned.push(name);
    }
    assert!(
        scanned.iter().any(|n| n == "tauri.conf.json"),
        "取材面里没有 tauri.conf.json —— 选择器失效，本门恒绿。实际扫到：{scanned:?}"
    );
}
