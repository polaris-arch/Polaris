use super::*;

#[test]
fn backup_failure_exposes_only_the_stable_error_code() {
    let payload = backup_failure("writeFailed");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["errorCode"], "writeFailed");
    assert!(payload.get("error").is_none());
    assert!(payload.get("diagnostic").is_none());
}

#[test]
fn selected_invalid_mesh_policy_fails_export_before_file_picker() {
    let mut config = polaris_store::store::default_config();
    config["meshRoutePolicy"] = json!({
        "schemaVersion": 1,
        "candidateOrder": ["ts-a"],
        "authKey": "must-not-export"
    });
    assert_eq!(
        pick_checked_categories(&config, &[BackupCategory::MeshRouting]),
        Err("meshRoutingInvalidPolicy")
    );
    assert!(
        pick_checked_categories(&config, &[BackupCategory::GeneralSettings])
            .unwrap()
            .get("meshRoutePolicy")
            .is_none()
    );
}

/* ══════════════ content-URI 往返门（备份导出 / 导入这一组）══════════════ */
//
// 门的本体与射程自曝在 `commands::picked_file::tests` 的头注里。这里驱动的是**备份腿自己的
// 那两道缝**（`finish_export` / `read_import_source`）——它们回的就是命令回给前端的那个信封，
// 所以「不落 cancelled」在这里是逐字可断言的，而不是靠推。

use crate::commands::picked_file::tests::{
    desktop_target, file_url_of, saf_file_path, RecordingGateway, SAF_URI,
};
use crate::test_support::TestDir;

mod restore_integration;

#[test]
fn exporting_to_a_content_uri_is_not_reported_as_cancelled_and_really_writes() {
    let dir = TestDir::new("polaris-backup-export-uri");
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));

    let payload = finish_export(&gateway, Some(saf_file_path()), "{\"version\":\"1.1\"}");

    // ① 不落 cancelled —— 这正是 W-18 修掉的那个静默形态。
    assert_eq!(payload["success"], true);
    assert!(
        payload.get("errorCode").is_none(),
        "URI 目标不许被判成失败/取消"
    );
    // ② 回执是 URI 原文：前端把它原样喂回 `backup_import_apply` 才能二次打开同一个文档。
    assert_eq!(payload["filePath"], SAF_URI);
    // ③ 字节真的写出去了 ——「什么都没发生」不会落 cancelled，只有这一条抓得住它。
    assert_eq!(gateway.contents().as_deref(), Some("{\"version\":\"1.1\"}"));
    assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);
}

#[test]
fn exporting_a_backup_to_a_content_uri_overwrites_a_longer_existing_document() {
    // 上一条只在空文件上写一次 ⇒ 把 `overwrite_opts()` 的 `truncate` 删掉整批门照绿（实测过）。
    // 备份是本仓受害最重的那条腿：没有截断时，新备份带着旧文档的尾巴落盘，**导出当场报成功**，
    // 一直到用户按恢复、`parse_backup_content` 判 invalidFormat 才暴露。
    let dir = TestDir::new("polaris-backup-export-truncate");
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));
    gateway.seed("{\"version\":\"1.0\",\"config\":{\"OLD\":\"这份旧备份比新的长得多得多\"}}");

    let payload = finish_export(&gateway, Some(saf_file_path()), "{\"v\":\"1.1\"}");

    assert_eq!(payload["success"], true);
    assert_eq!(
        gateway.contents().as_deref(),
        Some("{\"v\":\"1.1\"}"),
        "必须逐字相等 —— 留下旧尾巴同样会让 success / contains 通过，却写出一份解析不了的备份"
    );
}

#[test]
fn exporting_a_backup_to_a_file_url_goes_through_the_gateway() {
    // iOS 形态。射程：iOS 真机未验，见 `picked_file::tests` 头注射程自曝 4。
    let dir = TestDir::new("polaris-backup-export-file-url");
    let gateway = RecordingGateway::new(dir.path().join("ios-doc"));
    let target = file_url_of(&dir.path().join("out.polaris-backup"));

    let payload = finish_export(&gateway, Some(target.clone()), "{\"v\":\"1.1\"}");

    assert_eq!(payload["success"], true);
    assert_eq!(payload["filePath"], target.to_string());
    assert_eq!(gateway.contents().as_deref(), Some("{\"v\":\"1.1\"}"));
    assert_eq!(gateway.calls(), vec![target.to_string()]);
}

#[test]
fn exporting_to_a_local_path_still_goes_through_std_fs() {
    // 桌面负侧：只喂**裸路径** —— 桌面的对话框只产得出这一种（见 `file_url_of` 的文档）。
    let dir = TestDir::new("polaris-backup-export-path");
    let out = dir.path().join("polaris-backup.polaris-backup");
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));

    let payload = finish_export(
        &gateway,
        Some(desktop_target(&out)),
        "{\"version\":\"1.0\"}",
    );
    assert_eq!(payload["success"], true);
    // 桌面回执逐字不变：仍是路径字符串，不是 URL。
    assert_eq!(payload["filePath"], out.to_string_lossy().as_ref());
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "{\"version\":\"1.0\"}"
    );
    assert!(
        gateway.calls().is_empty(),
        "桌面导出必须仍走 std::fs 原路，一次都不该碰插件网关"
    );
}

#[test]
fn only_a_real_cancel_is_reported_as_cancelled() {
    let dir = TestDir::new("polaris-backup-export-cancel");
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));

    // 正向对照：这套判据分得清「取消」与「写成了」——上面两条全绿的同时，这一条必须是 cancelled。
    let payload = finish_export(&gateway, None, "{}");
    assert_eq!(payload["success"], false);
    assert_eq!(payload["errorCode"], "cancelled");
    assert!(gateway.contents().is_none());
}

#[test]
fn a_write_failure_is_reported_as_write_failed_not_as_cancelled() {
    // 第二条正向对照：喂一个已知打不开的目标，证明失败真的报得出来，而不是被吃成取消。
    let dir = TestDir::new("polaris-backup-export-failure");
    let gateway = RecordingGateway::new(dir.path().join("no-such-dir").join("doc"));

    let payload = finish_export(&gateway, Some(saf_file_path()), "{}");
    assert_eq!(payload["errorCode"], "writeFailed");
}

#[test]
fn importing_from_a_content_uri_is_not_reported_as_cancelled_and_really_reads() {
    let dir = TestDir::new("polaris-backup-import-uri");
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));
    gateway.seed("{\"version\":\"1.1\",\"config\":{}}");

    let outcome = read_import_source(&gateway, Some(saf_file_path()));

    assert_eq!(
        outcome,
        ImportSource::Loaded {
            raw: "{\"version\":\"1.1\",\"config\":{}}".to_string(),
            file_path: SAF_URI.to_string(),
        },
        "URI 目标必须真的读到正文，且回执是能二次打开的那个 URI"
    );
    assert_eq!(gateway.calls(), vec![SAF_URI.to_string()]);
}

#[test]
fn importing_from_a_local_path_still_goes_through_std_fs() {
    // 桌面负侧：只喂**裸路径**。
    let dir = TestDir::new("polaris-backup-import-path");
    let src = dir.path().join("in.polaris-backup");
    std::fs::write(&src, "{\"version\":\"1.0\"}").unwrap();
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));

    assert_eq!(
        read_import_source(&gateway, Some(desktop_target(&src))),
        ImportSource::Loaded {
            raw: "{\"version\":\"1.0\"}".to_string(),
            file_path: src.to_string_lossy().into_owned(),
        }
    );
    assert!(gateway.calls().is_empty(), "桌面导入必须仍走 std::fs 原路");
}

#[test]
fn import_cancel_and_import_read_failure_stay_distinguishable() {
    let dir = TestDir::new("polaris-backup-import-negative");
    let gateway = RecordingGateway::new(dir.path().join("no-such-dir").join("doc"));

    assert_eq!(read_import_source(&gateway, None), ImportSource::Cancelled);
    assert_eq!(
        read_import_source(&gateway, Some(saf_file_path())),
        ImportSource::ReadFailed,
        "读不出来是 readFailed，不是 cancelled —— 两者混同正是本批修掉的缺陷"
    );
}

/* ────── 二次打开（`backup_import_apply`）：本批第一版一条判据都没有的那条腿 ────── */
//
// 它是五条腿里唯一真正执行整类替换的那条。pick / 预览成功而 apply 失败，用户看到的是
// 「选好了、类目也列出来了、按恢复没反应」。第一版把它留在命令体里没抽缝：实测把入参改成
// `file_path.trim_start_matches("content://")` 后**全仓门照绿**，而真机上每一次恢复都会失败。

#[test]
fn reapplying_a_content_uri_goes_through_the_gateway_and_really_reads() {
    let dir = TestDir::new("polaris-backup-apply-uri");
    let gateway = RecordingGateway::new(dir.path().join("saf-doc"));
    gateway.seed("{\"version\":\"1.1\",\"config\":{}}");

    // 喂的是**字符串**，而且是 `backup_import_pick` 真会回给前端的那一个（`display_of` 的输出）——
    // 这条同时钉住 `display_of` → 前端 → `FilePath::from_str` 的整条往返。
    let echoed = crate::commands::picked_file::display_of(&saf_file_path());
    assert_eq!(echoed, SAF_URI, "前提：pick 回给前端的就是这个字符串");

    assert_eq!(
        read_apply_source(&gateway, &echoed).expect("content URI 必须读得出来"),
        "{\"version\":\"1.1\",\"config\":{}}"
    );
    assert_eq!(
        gateway.calls(),
        vec![SAF_URI.to_string()],
        "content URI 必须经插件网关二次打开 —— 剥 scheme / 当路径解析都会让恢复 100% 失败"
    );
}

#[test]
fn reapplying_a_desktop_path_never_touches_the_gateway() {
    // 桌面负侧：`backup_import_pick` 在桌面回的是 `path.to_string_lossy()`，原样喂回来。
    let dir = TestDir::new("polaris-backup-apply-path");
    let src = dir.path().join("in.polaris-backup");
    std::fs::write(&src, "{\"version\":\"1.0\"}").unwrap();
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));

    let echoed = crate::commands::picked_file::display_of(&desktop_target(&src));
    assert_eq!(echoed, src.to_string_lossy().as_ref());
    assert_eq!(
        read_apply_source(&gateway, &echoed).expect("桌面路径必须读得出来"),
        "{\"version\":\"1.0\"}"
    );
    assert!(
        gateway.calls().is_empty(),
        "桌面二次打开必须仍走 std::fs 原路，一次都不该碰插件网关"
    );
}

#[test]
fn reapplying_a_windows_drive_letter_is_a_path_not_a_url() {
    // `FilePath::from_str` 把**单字母 scheme** 留给 Windows 盘符
    //（`tauri-plugin-fs-2.5.1/src/file_path.rs:189-196`）。这是桌面侧唯一没被别处断言过的分流点：
    // 判错的话 Windows 上每一次恢复都会被送去插件网关。
    let dir = TestDir::new("polaris-backup-apply-windows");
    let gateway = RecordingGateway::new(dir.path().join("must-stay-untouched"));

    // 本机读不到这个路径（这里不是要它成功），要的是**它一次都不许碰网关**。
    let outcome = read_apply_source(&gateway, "C:\\Users\\chen\\polaris.polaris-backup");

    assert!(
        outcome.is_err(),
        "本机不存在该路径 ⇒ 必须报错，而不是静默返回空串"
    );
    assert!(
        gateway.calls().is_empty(),
        "Windows 盘符必须落 Path 侧 —— 被当成 URL 送进网关，Windows 上恢复会全线失败"
    );
}

#[test]
fn a_reapply_read_failure_is_reported_and_not_swallowed() {
    // 正向对照：上面两条都是成功路径，这一条喂一个已知打不开的目标，证明这组判据分得清
    // 「读到了」与「读不到」——而不是对任何输入都点头。
    let dir = TestDir::new("polaris-backup-apply-negative");
    let gateway = RecordingGateway::new(dir.path().join("no-such-dir").join("doc"));

    assert!(read_apply_source(&gateway, SAF_URI).is_err());
    assert_eq!(
        gateway.calls(),
        vec![SAF_URI.to_string()],
        "失败也必须是「经网关试过了」，不是「压根没走到那一步」"
    );
}

#[tokio::test]
async fn android_backup_preserves_bindings_without_polling_active_network_inventory() {
    let names = import_interface_names_from(polaris_helper_proto::Platform::Android, async {
        panic!("active Network snapshot is not an interface inventory")
    })
    .await;
    assert!(names.is_none());
    let mut config = json!({"networkInterfaces": {"proxy": "rmnet0"}});
    let cleared = names.as_ref().map_or(0, |names| {
        sanitize_unavailable_interface_bindings(
            &mut config,
            names,
            &[BackupCategory::GeneralSettings],
        )
    });
    assert_eq!(cleared, 0);
    assert_eq!(config["networkInterfaces"]["proxy"], "rmnet0");
}

#[tokio::test]
async fn desktop_backup_inventory_keeps_empty_success_distinct_from_failure() {
    assert_eq!(
        import_interface_names_from(polaris_helper_proto::Platform::Linux, async { Ok(vec![]) })
            .await,
        Some(BTreeSet::new())
    );
    assert!(
        import_interface_names_from(polaris_helper_proto::Platform::Linux, async {
            Err("failed".into())
        })
        .await
        .is_none()
    );
    let names = import_interface_names_from(polaris_helper_proto::Platform::Linux, async {
        Ok(vec![crate::commands::system::NetworkInterfaceInfo {
            name: "eth0".into(),
            display_name: "Ethernet".into(),
            is_up: false,
            addresses: vec![],
        }])
    })
    .await
    .unwrap();
    assert_eq!(names, BTreeSet::from(["eth0".to_owned()]));
}
