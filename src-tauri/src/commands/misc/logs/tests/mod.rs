use super::*;
use crate::test_support::crate_code;

fn temp_log_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "polaris-misc-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn core_log_tail_keeps_legacy_startup_and_managed_sources_bounded() {
    let dir = temp_log_dir("core-tail");
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    std::fs::write(dir.join(LEGACY_SINGBOX_LOG), b"legacy-line\n").unwrap();
    std::fs::write(dir.join(STARTUP_SINGBOX_LOG), b"fatal-line\n").unwrap();
    std::fs::write(dir.join("logs").join(MANAGED_SINGBOX_LOG), b"live-line\n").unwrap();

    let tail = read_core_log_tail(&dir, 192);
    assert!(tail.contains("legacy-line"));
    assert!(tail.contains("fatal-line"));
    assert!(tail.contains("live-line"));
    assert!(tail.len() <= 192, "三路兼容不得让导出重新变成无界");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn exported_tail_redacts_helper_and_legacy_native_vpn_credentials() {
    let dir = temp_log_dir("core-tail-redaction");
    std::fs::create_dir_all(dir.join("logs")).unwrap();
    std::fs::write(
        dir.join(STARTUP_SINGBOX_LOG),
        b"Authorization: Bearer STARTUP_TOKEN\nprivate_key=WG_PRIVATE\n",
    )
    .unwrap();
    std::fs::write(
        dir.join(LEGACY_SINGBOX_LOG),
        b"open URL https://vpn.example/sso/PATH_TOKEN?code=QUERY_TOKEN\n",
    )
    .unwrap();

    let tail = read_core_log_tail(&dir, 2_048);
    for secret in ["STARTUP_TOKEN", "WG_PRIVATE", "PATH_TOKEN", "QUERY_TOKEN"] {
        assert!(!tail.contains(secret), "导出日志泄漏 {secret}: {tail}");
    }
    assert!(tail.contains("<redacted>"));
    assert!(tail.contains("https://vpn.example/<redacted>"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn legacy_archive_commits_destination_before_removing_source() {
    let dir = temp_log_dir("archive");
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join(LEGACY_SINGBOX_LOG);
    let destination = dir.join("archive").join("saved.log");
    std::fs::write(&source, b"historical evidence").unwrap();

    assert_eq!(archive_legacy_log(&source, &destination).unwrap(), 19);
    assert!(!source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), b"historical evidence");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn legacy_archive_replaces_an_approved_existing_target() {
    let dir = temp_log_dir("archive-replace");
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join(LEGACY_SINGBOX_LOG);
    let destination = dir.join("saved.log");
    std::fs::write(&source, b"new evidence").unwrap();
    std::fs::write(&destination, b"old archive").unwrap();

    assert_eq!(archive_legacy_log(&source, &destination).unwrap(), 12);
    assert!(!source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), b"new evidence");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn legacy_archive_refuses_in_place_destination() {
    let dir = temp_log_dir("archive-same");
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join(LEGACY_SINGBOX_LOG);
    std::fs::write(&source, b"keep me").unwrap();
    assert!(archive_legacy_log(&source, &source).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"keep me");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn legacy_archive_refuses_a_path_alias_to_the_source() {
    let dir = temp_log_dir("archive-alias");
    std::fs::create_dir_all(dir.join("alias")).unwrap();
    let source = dir.join(LEGACY_SINGBOX_LOG);
    let destination = dir.join("alias").join("..").join(LEGACY_SINGBOX_LOG);
    std::fs::write(&source, b"keep me too").unwrap();
    assert!(archive_legacy_log(&source, &destination).is_err());
    assert_eq!(std::fs::read(&source).unwrap(), b"keep me too");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn legacy_delete_is_explicit_regular_file_only_and_idempotent() {
    let dir = temp_log_dir("delete");
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join(LEGACY_SINGBOX_LOG);
    std::fs::write(&source, b"obsolete evidence").unwrap();

    assert_eq!(delete_legacy_log(&source).unwrap(), Some(17));
    assert!(!source.exists());
    assert_eq!(delete_legacy_log(&source).unwrap(), None);

    std::fs::create_dir(&source).unwrap();
    assert!(delete_legacy_log(&source).is_err());
    assert!(source.is_dir(), "非普通文件不得被删除");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn log_subscription_tokens_protect_new_mount_from_stale_cleanup() {
    let mut registry = LogSubscriberRegistry::default();
    assert!(registry.register("main", "mount-1"));
    assert!(!registry.register("main", "mount-2"));
    assert!(
        !registry.unregister("main", "mount-1"),
        "旧页面 cleanup 不得删除同一窗口的新页面订阅"
    );
    assert_eq!(registry.windows(), vec!["main"]);
    assert!(registry.unregister("main", "mount-2"));
    assert!(registry.windows().is_empty());
}

#[test]
fn log_subscription_window_cleanup_is_idempotent() {
    let mut registry = LogSubscriberRegistry::default();
    registry.register("main", "mount-1");
    assert!(registry.clear_window("main"));
    assert!(!registry.clear_window("main"));
}

#[test]
fn log_emitter_reuses_non_blocking_main_window_visibility() {
    let body = crate::commands::guard_scan::top_level_fn_body(
        &crate_code("commands/misc/logs.rs"),
        "fn visible_log_windows(",
    );
    assert!(
        body.contains("runtime.stats().window_visible(app)"),
        "日志 emitter 必须复用 stats 的主线程缓存式可见性真值"
    );
    for forbidden in ["is_visible(", "is_minimized(", "get_webview_window("] {
        assert!(
            !body.contains(forbidden),
            "后台日志 emitter 不得直接调用平台窗口 getter `{forbidden}`"
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════
// 核在跑的真实日志级别：级别名投影
// ══════════════════════════════════════════════════════════════════════════

/// **必须是小写**，不只是「好看」：渲染端拿这个串直接与 `config.logLevel`（恒小写）比对来判
/// 「核在跑的级别是否与我写下的值分叉」。返回 `WARN` 的话每一次比对都不相等 ⇒ 徽标恒亮分叉告警，
/// 一个天天喊狼来了的自证等于没有自证。
///
/// **变异锁**：去掉 `to_ascii_lowercase()` → 转红。
#[test]
fn runtime_level_name_is_lowercase_matching_config_log_level() {
    use polaris_singbox_grpc::daemon::LogLevel;
    assert_eq!(runtime_level_name(LogLevel::Warn), "warn");
    assert_eq!(runtime_level_name(LogLevel::Info), "info");
    assert_eq!(runtime_level_name(LogLevel::Debug), "debug");
    // sing-box 独有的两档（本仓生成侧永不写入，但读侧必须能原样说出来）。
    assert_eq!(runtime_level_name(LogLevel::Panic), "panic");
    assert_eq!(runtime_level_name(LogLevel::Trace), "trace");
}

// ══════════════════════════════════════════════════════════════════════════
// R2 出口无效直判终态的**载荷折叠**（`fold_proxy_blocked`，1:1 上游
// `IpInfoService.markProxyBlocked` :187-197）。纯逻辑、不碰任何进程级 static ⇒ 可并行跑，
// 不受 `stale_probe_leg_must_not_overwrite_newer_leg` 那条「唯一碰 static 的测试」约束。
// ══════════════════════════════════════════════════════════════════════════

fn log_rec(seq: u64, msg: &str) -> crate::logging::LogRecord {
    crate::logging::LogRecord {
        seq,
        ts_ms: 1_700_000_000_000,
        level: "info",
        target: "app".into(),
        message: msg.into(),
    }
}

/// `_id` 必须随每条日志出境，且**原样**是后端的单调 seq。
///
/// 打断这条（不发 `_id` / 改用 timestamp 派生）→ 渲染端只能退回 `timestamp-index` 作 key：
/// 环形缓冲一滑动全列换身份（滚动期全量重渲 + 打断选区），且水合与增量流那 ≤150ms 的重叠窗口
/// 无从去重（同一条日志渲染两遍）。
#[test]
fn log_entry_carries_monotonic_id() {
    let a = log_record_to_entry(&log_rec(41, "first"));
    let b = log_record_to_entry(&log_rec(42, "second"));
    assert_eq!(a["_id"], json!(41), "_id 必须原样带出后端 seq");
    assert!(
        a["_id"].as_u64() < b["_id"].as_u64(),
        "_id 必须单调递增——去重键靠「≤ 已见最大值即丢」，非单调即漏行/重放"
    );
    // 其余契约字段不得因加 _id 而漂。
    assert_eq!(b["level"], json!("info"));
    assert_eq!(b["message"], json!("second"));
    assert_eq!(b["source"], json!("app"));
    assert!(b["timestamp"].as_str().is_some_and(|s| s.contains('T')));
}

/// trace → debug 的归并不受 `_id` 改动影响（渲染端 `LogLevel` 无 trace 档）。
#[test]
fn log_entry_level_still_folds_trace_into_debug() {
    let mut r = log_rec(1, "x");
    r.level = "trace";
    assert_eq!(log_record_to_entry(&r)["level"], json!("debug"));
}

/// 单批截断取**尾部**（丢最旧、保最新）。取头部 = UI 永远显示最老的那 500 条，
/// 真机上与「日志流卡死」几乎不可分辨，故显式钉住方向。
#[test]
fn tail_capped_keeps_newest_and_drops_oldest() {
    let v: Vec<u32> = (0..10).collect();
    assert_eq!(tail_capped(&v, 3), &[7, 8, 9], "保最新三条");
    assert_eq!(tail_capped(&v, 10), &v[..], "不超容量 → 原样");
    assert_eq!(tail_capped(&v, 99), &v[..], "cap 超量 → 原样");
    assert!(tail_capped(&v, 0).is_empty(), "cap=0 → 空");
    let empty: [u32; 0] = [];
    assert!(tail_capped(&empty, 5).is_empty(), "空输入不 panic");
}

/// 截断上限与渲染端缓冲同量：补推多于渲染端能留的行数只是白费一次序列化 + 一次 webview 唤醒。
#[test]
fn pending_batch_cap_matches_renderer_buffer() {
    assert_eq!(
        MAX_PENDING_LOG_BATCH, 500,
        "与 LogsScreen MAX_BUFFER 同量（改一边须同步另一边）"
    );
}

/* ══════════════ content-URI 往返门（日志 / 诊断导出这一组）══════════════ */
//
// 门的本体与射程自曝在 `commands::picked_file::tests` 的头注里。这里驱动的是**导出腿自己的
// 那道缝**（`finish_export`）—— 它回的就是 `logs_export` / `diagnostic_export` 交给前端的那个
// 信封，故「不落 cancelled」在这里是逐字可断言的。

use crate::commands::picked_file::tests::{
    desktop_target, file_url_of, saf_file_path, RecordingGateway, SAF_URI,
};
use crate::test_support::TestDir;

#[test]
fn exporting_to_a_content_uri_is_not_reported_as_cancelled_and_really_writes() {
    let dir = TestDir::new("polaris-logs-export-uri");
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));

    let payload = finish_export(&gateway, Some(saf_file_path()), "# Polaris 日志导出\n");

    // ① 不落 cancelled —— W-18 之前 content URI 恰恰会落在这里。
    assert_eq!(payload["success"], true);
    assert!(
        payload.get("error").is_none(),
        "URI 目标不许被判成失败/取消"
    );
    // ② 回执给 URI 原文（桌面那侧仍是路径，见下一条）。
    assert_eq!(payload["filePath"], SAF_URI);
    // ③ 字节真的写出去了 ——「什么都没发生」同样不会落 cancelled，只有这一条抓得住它。
    assert_eq!(gateway.contents().as_deref(), Some("# Polaris 日志导出\n"));
    assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);
}

#[test]
fn exporting_to_a_content_uri_overwrites_a_longer_existing_document() {
    // 上一条只在空文件上写一次 ⇒ `overwrite_opts()` 的 `truncate` 删掉它照样绿（实测过）。
    // 而 `truncate` 是 Android 上唯一决定 `android_mode()` 出 "wt" 而不是 "w" 的那一位：
    // 允许「替换现有文件」的 provider 交回更长的旧文档时，报告会带着旧尾巴落盘、导出仍报成功。
    let dir = TestDir::new("polaris-logs-export-truncate");
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));
    gateway.seed("# 上一次导出的报告，比这次长得多，长得多，长得多\n");

    let payload = finish_export(&gateway, Some(saf_file_path()), "# 新报告\n");

    assert_eq!(payload["success"], true);
    assert_eq!(
        gateway.contents().as_deref(),
        Some("# 新报告\n"),
        "必须逐字相等 —— 带着旧尾巴同样能让 contains / success 通过"
    );
}

#[test]
fn exporting_to_a_file_url_goes_through_the_gateway_and_echoes_the_url() {
    // iOS 形态（`saveFileDialog` 的 `.exportToService` 回的就是它）。射程：iOS 真机未验，
    // 见 `picked_file::tests` 头注射程自曝 4。
    let dir = TestDir::new("polaris-logs-export-file-url");
    let gateway = RecordingGateway::new(dir.path().join("ios-doc"));
    let target = file_url_of(&dir.path().join("polaris-diagnostic.md"));

    let payload = finish_export(&gateway, Some(target.clone()), "ios-report");

    assert_eq!(payload["success"], true);
    assert_eq!(payload["filePath"], target.to_string());
    assert_eq!(gateway.contents().as_deref(), Some("ios-report"));
    assert_eq!(gateway.calls(), vec![target.to_string()]);
}

#[test]
fn exporting_to_a_local_path_still_goes_through_std_fs() {
    // 桌面负侧：只喂**裸路径** —— 桌面的对话框只产得出这一种（见 `file_url_of` 的文档）。
    let dir = TestDir::new("polaris-logs-export-path");
    let out = dir.path().join("polaris-diagnostic.md");
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));

    let payload = finish_export(&gateway, Some(desktop_target(&out)), "desktop-report");
    assert_eq!(payload["success"], true);
    // 桌面回执逐字不变：仍是 `path.to_string_lossy()`，不是 URL。
    assert_eq!(payload["filePath"], out.to_string_lossy().as_ref());
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "desktop-report");
    assert!(
        gateway.calls().is_empty(),
        "桌面导出必须仍走 std::fs 原路，一次都不该碰插件网关"
    );
}

#[test]
fn only_a_real_cancel_is_reported_as_cancelled_and_failures_say_why() {
    // 正向对照：上面两条全是成功路径。这一条证明判据分得清取消、失败与成功三态。
    let dir = TestDir::new("polaris-logs-export-negative");
    let gateway = RecordingGateway::new(dir.path().join("no-such-dir").join("doc"));

    let cancelled = finish_export(&gateway, None, "body");
    assert_eq!(cancelled["success"], false);
    assert_eq!(cancelled["error"], "cancelled");

    let failed = finish_export(&gateway, Some(saf_file_path()), "body");
    assert_eq!(failed["success"], false);
    assert_ne!(
        failed["error"], "cancelled",
        "打不开必须报出原因，不许再被吃成「用户取消了」"
    );
}

#[test]
fn archiving_the_legacy_log_refuses_a_content_uri_instead_of_pretending_to_be_cancelled() {
    // `logs_archive_legacy` 的落盘是同目录临时文件 + rename 事务，content URI 上表达不出来。
    // 处置是**显式报错**：这里钉住那条分派判据本身 —— URI 目标不得被 `classify` 判成路径，
    // 否则命令会带着一个不存在的路径往下走 `archive_legacy_log`。
    assert!(
        matches!(classify(&saf_file_path()), PickedTarget::Uri(_)),
        "content URI 必须落进 URI 侧 ⇒ 命令那一支返回显式错误，而不是 cancelled"
    );
    let dir = TestDir::new("polaris-logs-archive-path");
    let dest = dir.path().join("legacy.log");
    assert_eq!(
        classify(&desktop_target(&dest)),
        PickedTarget::Path(dest),
        "桌面归档腿必须仍拿到本机路径（rename 事务的前提）"
    );
    // `file:` URL 在本批之后落 URI 侧 ⇒ 归档腿对它同样是**显式报错**。桌面产不出这种形态，
    // 故这不是桌面回归；如实登记：iOS 上归档旧日志今天**不支持**，只是不假装成取消。
    assert!(matches!(
        classify(&file_url_of(&dir.path().join("ios.log"))),
        PickedTarget::Uri(_)
    ));
}
