use super::*;
use crate::test_support::TestDir;

fn rt(dir: &Path) -> UpdaterRuntime {
    UpdaterRuntime::new(dir.to_path_buf())
}

#[test]
fn bundled_core_version_uses_the_same_selected_build_as_http() {
    assert_eq!(
        bundled_core_version(),
        crate::runtime::core_version::selected_bundled_version()
    );
    assert!(!bundled_core_version().is_empty());
}

#[test]
fn state_file_roundtrip_is_atomic_and_survives_reload() {
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    let r = rt(tmp.path());
    assert!(r.state().skipped_version.is_none());

    r.mutate_state(|s| s.skipped_version = Some("4.2.5".into()))
        .unwrap();
    assert_eq!(r.state().skipped_version.as_deref(), Some("4.2.5"));

    // 新实例重读磁盘 → 状态持久化生效。
    let r2 = rt(tmp.path());
    assert_eq!(r2.state().skipped_version.as_deref(), Some("4.2.5"));
    // 无 .tmp 残件。
    assert!(!tmp.path().join("update-state.json.tmp").exists());
}

#[test]
fn corrupt_state_file_degrades_to_empty_not_panic() {
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    std::fs::write(tmp.path().join("update-state.json"), b"{not json").unwrap();
    // 损坏文件不得 panic（更新域瘫掉 ≠ App 起不来）。
    let r = rt(tmp.path());
    assert!(r.state().skipped_version.is_none());
    // 仍可正常写回（覆盖损坏文件）。
    r.mutate_state(|s| s.skipped_version = Some("1.0.0".into()))
        .unwrap();
    assert_eq!(
        rt(tmp.path()).state().skipped_version.as_deref(),
        Some("1.0.0")
    );
}

/// 旧版本写下的内核更新字段不得让状态文件读失败，写回后它们从盘上消失。
#[test]
fn legacy_core_update_fields_are_ignored_and_dropped_on_rewrite() {
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    let path = tmp.path().join("update-state.json");
    std::fs::write(
        &path,
        br#"{"skippedVersion":"1.2.3","lastCheckAt":1,
            "staged":{"version":"1.13.0","dir":"/x/core-staged","stagedAt":"t"},
            "crossBandNotifiedVersion":"1.14.0",
            "pendingChangeNotice":{"previousVersion":"1.13.0","currentVersion":"1.14.0"}}"#,
    )
    .unwrap();
    let r = rt(tmp.path());
    assert_eq!(r.state().skipped_version.as_deref(), Some("1.2.3"));

    r.mutate_state(|s| s.skipped_version = Some("1.2.4".into()))
        .unwrap();
    let rewritten: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(rewritten, serde_json::json!({ "skippedVersion": "1.2.4" }));
}

#[test]
fn core_version_readers_are_asymmetric() {
    //   read_core_version_line 探测失败 → ""（诚实失败）
    //   read_core_version      探测失败 → 回落配套版本（会伪装成「现役内核 = 配套版本」）
    // 二者若同语义，内核信息卡就分不出「读到了配套版本」与「什么都没读到」。
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    let r = rt(tmp.path());
    // 未注入核路径 = 探测必失败（本机不 spawn 真核，符合「真实安装核不在本机跑」纪律）。
    assert!(r.core_binary_path().is_none());

    assert_eq!(r.read_core_version_line(), "", "失败置空的读法必须返回空串");
    assert_eq!(
        r.read_core_version(),
        r.bundled_core_version(),
        "回落的读法必须回落配套版本"
    );
    // 两者**必须不同**——这正是不对称本身。
    assert_ne!(
        r.read_core_version_line(),
        r.read_core_version(),
        "双读法失败语义一旦对称，展示读数就不能再当证据用"
    );
}

/// 嵌入清单所选桌面发布合同的 sourceTree 由运行时访问器原样给出；无效选择与移动端不给。
#[test]
fn bundled_patch_set_comes_from_the_selected_release_contract() {
    let contract: serde_json::Value = serde_json::from_str(&crate::test_support::repo_file(
        "scripts/desktop-core/release-contract.json",
    ))
    .unwrap();
    let expected = contract["sourceTree"].as_str().unwrap();
    assert_eq!(
        bundled_core_patch_set_from_manifest(CORE_MANIFEST_JSON, false).as_deref(),
        Some(expected)
    );
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    assert_eq!(rt(tmp.path()).bundled_core_patch_set(), Some(expected));
    assert_eq!(
        bundled_core_patch_set_from_manifest(CORE_MANIFEST_JSON, true),
        None
    );
    let mut invalid: serde_json::Value = serde_json::from_str(CORE_MANIFEST_JSON).unwrap();
    invalid["desktopRelease"]["contractSha256"] = serde_json::json!("f".repeat(64));
    assert_eq!(
        bundled_core_patch_set_from_manifest(&invalid.to_string(), false),
        None
    );
    assert_eq!(
        bundled_core_patch_set_from_manifest("{not json", false),
        None
    );
}

/// 升级目标与来源必须分开；意外版本变化不等于目标版本已运行。
#[test]
fn portable_handoff_status_uses_the_target_and_never_claims_same_version_completion() {
    let mut record = PendingPortableUpdate {
        archive: "Polaris_1.2.3_x64-win-Portable.zip".into(),
        program_dir: "dir".into(),
        from_version: "1.2.2".into(),
        target_version: Some("1.2.3".into()),
    };
    assert_eq!(
        record.handoff_status("1.2.2", true),
        Some(PortableHandoffStatus::AwaitingTargetVersion)
    );
    assert_eq!(
        record.handoff_status("1.2.3", true),
        None,
        "仅目标版本已运行时不恢复升级卡"
    );
    assert_eq!(
        record.handoff_status("1.2.4", true),
        Some(PortableHandoffStatus::CompletionUnverified)
    );
    assert_eq!(record.handoff_status("1.2.2", false), None);
    record.from_version = "1.2.3".into();
    assert_eq!(
        record.handoff_status("1.2.3", true),
        Some(PortableHandoffStatus::CompletionUnverified),
        "同版本修复：版本相同不证明未覆盖，也不证明完成"
    );
    let legacy: PendingPortableUpdate =
        serde_json::from_str(r#"{"archive":"old.zip","programDir":"dir","fromVersion":"1.2.2"}"#)
            .unwrap();
    assert_eq!(legacy.target_version, None);
    assert_eq!(
        legacy.handoff_status("1.2.3", true),
        Some(PortableHandoffStatus::CompletionUnverified)
    );
    record.target_version = None;
    assert_eq!(
        record.handoff_status("1.2.4", true),
        Some(PortableHandoffStatus::CompletionUnverified),
        "旧记录没有目标，不能仅凭版本不同假报已完成"
    );
}

/// 同版本已覆盖/未覆盖重启都不假报成功或未完成；沿显式重新检查清除提示，ZIP 保留。
#[test]
fn same_version_repair_reminder_can_be_explicitly_cleared_without_deleting_the_archive() {
    for replaced in [false, true] {
        let dir = TestDir::new("polaris-portable-handoff-status-");
        let archive = dir.join("Polaris_1.2.3_x64-win-Portable.zip");
        std::fs::write(&archive, b"retained zip bytes").unwrap();
        let program = dir.join("program");
        std::fs::create_dir(&program).unwrap();
        std::fs::write(
            program.join("polaris.exe"),
            if replaced {
                "replacement bytes"
            } else {
                "unreplaced bytes"
            },
        )
        .unwrap();
        let u = rt(&dir);
        u.mutate_state(|s| {
            s.pending_portable_update = Some(PendingPortableUpdate {
                archive: archive.to_string_lossy().into_owned(),
                program_dir: program.to_string_lossy().into_owned(),
                from_version: "1.2.3".into(),
                target_version: Some("1.2.3".into()),
            })
        })
        .unwrap();
        let reopened = rt(&dir);
        let record = reopened.state().pending_portable_update.unwrap();
        assert_eq!(
            record.handoff_status("1.2.3", archive.is_file()),
            Some(PortableHandoffStatus::CompletionUnverified)
        );
        reopened.clear_portable_handoff().unwrap();
        assert!(
            rt(&dir).state().pending_portable_update.is_none(),
            "显式清除必须持久化"
        );
        assert_eq!(std::fs::read(&archive).unwrap(), b"retained zip bytes");
    }
}

/// 写失败不能报告提示已清除，也不能仅清内存而让重启后旧提示重现。
#[test]
fn failed_portable_reminder_clear_keeps_memory_disk_and_archive() {
    let dir = TestDir::new("polaris-portable-clear-failure-");
    let archive = dir.join("Polaris_1.2.3_x64-win-Portable.zip");
    std::fs::write(&archive, b"retained zip bytes").unwrap();
    let u = rt(&dir);
    u.mutate_state(|s| {
        s.pending_portable_update = Some(PendingPortableUpdate {
            archive: archive.to_string_lossy().into_owned(),
            program_dir: dir.join("program").to_string_lossy().into_owned(),
            from_version: "1.2.3".into(),
            target_version: Some("1.2.3".into()),
        });
    })
    .unwrap();
    let before = u.state().pending_portable_update;
    let backup = dir.join("original-update-state.json");
    std::fs::rename(&u.state_path, &backup).unwrap();
    std::fs::create_dir(&u.state_path).unwrap();
    assert!(u.clear_portable_handoff().is_err());
    assert_eq!(u.state().pending_portable_update, before);
    std::fs::remove_dir(&u.state_path).unwrap();
    std::fs::rename(&backup, &u.state_path).unwrap();
    assert_eq!(rt(&dir).state().pending_portable_update, before);
    assert_eq!(std::fs::read(archive).unwrap(), b"retained zip bytes");
}

/// 留底随更新状态文件往返；老文件里没有这个字段时读成「没有」，写回也不凭空多出它。
#[test]
fn pending_portable_update_round_trips_through_the_state_file() {
    let mut state: UpdateStateFile = serde_json::from_str(r#"{"skippedVersion":"1.0.0"}"#).unwrap();
    assert_eq!(state.pending_portable_update, None);
    assert_eq!(
        serde_json::to_value(&state).unwrap(),
        serde_json::json!({ "skippedVersion": "1.0.0" })
    );
    state.pending_portable_update = Some(PendingPortableUpdate {
        archive: "a.zip".to_owned(),
        program_dir: "dir".to_owned(),
        from_version: "1.2.2".to_owned(),
        target_version: Some("1.2.3".to_owned()),
    });
    let written = serde_json::to_value(&state).unwrap();
    assert_eq!(
        written["pendingPortableUpdate"],
        serde_json::json!({ "archive": "a.zip", "programDir": "dir", "fromVersion": "1.2.2", "targetVersion": "1.2.3" })
    );
    let read_back: UpdateStateFile = serde_json::from_value(written).unwrap();
    assert_eq!(
        read_back.pending_portable_update,
        state.pending_portable_update
    );
}

#[test]
fn failed_state_publication_does_not_mutate_memory_or_disk() {
    let tmp = TestDir::new("polaris-updater-failed-publication-");
    let runtime = rt(tmp.path());
    runtime
        .mutate_state(|state| state.skipped_version = Some("1.2.3".into()))
        .unwrap();
    // Block the staging path without depending on permission behavior as root.
    std::fs::create_dir(tmp.path().join("update-state.json.tmp")).unwrap();
    assert!(runtime
        .mutate_state(|state| state.skipped_version = Some("9.9.9".into()))
        .is_err());
    assert_eq!(runtime.state().skipped_version.as_deref(), Some("1.2.3"));
    assert_eq!(
        rt(tmp.path()).state().skipped_version.as_deref(),
        Some("1.2.3")
    );
}

#[test]
fn version_reader_uses_shared_bounded_probe_without_changing_fallback_semantics() {
    let source = crate::test_support::crate_code("runtime/updater.rs");
    let body = crate::commands::guard_scan::impl_method_body(
        &source,
        "    pub fn read_core_version_line(",
    );
    assert!(body.contains("self.core_version_probe.first_line(&bin)"));
    assert!(!body.contains(".output()"));
}
