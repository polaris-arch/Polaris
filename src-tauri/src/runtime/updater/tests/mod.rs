use super::*;
use crate::test_support::TestDir;

fn rt(dir: &Path) -> UpdaterRuntime {
    UpdaterRuntime::new(dir.to_path_buf())
}

#[test]
fn bundled_core_version_parses_from_embedded_manifest() {
    // 编译期嵌入的 core-manifest.json 必须解析出非空基线 —— 它是 C6 全部决策的锚。
    // 若清单改名/改结构，这条即刻转红（而非在运行期静默按空基线跑）。
    let v = bundled_core_version();
    assert!(!v.is_empty(), "bundledCoreVersion 不得为空");
    assert!(
        v.starts_with(|c: char| c.is_ascii_digit()),
        "基线应是版本号，实得: {v}"
    );
    // 上面两条分辨不出基线回落（回落值同样非空）。嵌入清单必须被认作冻结清单。
    let manifest: serde_json::Value = serde_json::from_str(CORE_MANIFEST_JSON).unwrap();
    let source = &manifest["sourceBuild"];
    let expected = source["version"].as_str().expect("sourceBuild.version");
    assert_eq!(
        frozen_source_build_version(source, manifest["bundledCoreVersion"].as_str().unwrap()),
        Some(expected),
        "嵌入的 core-manifest 未被认作冻结清单，随包基线会回落"
    );
}

const PATCHED_FIXTURE: [&str; 3] = [
    "example.com/patched",
    "example.com/second",
    "example.com/tiny",
];

fn source_baseline_fixture() -> serde_json::Value {
    source_baseline_fixture_with(&PATCHED_FIXTURE)
}

fn source_baseline_fixture_with(patched: &[&str]) -> serde_json::Value {
    let platforms: serde_json::Map<String, serde_json::Value> = ["linux", "win", "mac-x64", "mac-arm64"]
        .into_iter()
        .map(|key| {
            let transport_required = if key == "linux" { vec!["example.com/transport", "example.com/linux-transport"] }
                else { vec!["example.com/transport"] };
            let transport_absent = if key == "linux" { vec![] } else { vec!["example.com/linux-transport"] };
            (key.to_owned(), serde_json::json!({ "buildTree": "1".repeat(40), "binarySha256": null,
                "patchedModules": { "requiredLinked": patched, "allowedAbsent": [] },
                "transportModules": { "requiredLinked": transport_required, "confirmedAbsent": transport_absent } }))
        }).collect();
    serde_json::json!({ "bundledCoreVersion": "1.0.0", "windowsBuild": { "version": "1.0.0.polaris.1" },
        "sourceBuild": { "sourceManifestSha256": "a".repeat(64), "provisionerSha256": "b".repeat(64),
            "sourceReceiptFingerprint": "c".repeat(64), "moduleGraphSha256": "d".repeat(64),
            "patchedSourceTree": "e".repeat(40), "buildTree": "f".repeat(40),
            "version": "1.0.0.polaris.2", "dependencyModules": patched,
            "transportPins": { "example.com/transport": "v1.0.0", "example.com/linux-transport": "v1.2.0" },
            "platforms": platforms } })
}

#[test]
fn frozen_source_baseline_covers_all_desktops_without_changing_android() {
    // 依赖补丁为空的清单同样是冻结清单。
    for fixture in [source_baseline_fixture(), source_baseline_fixture_with(&[])] {
        let raw = fixture.to_string();
        for windows in [false, true] {
            assert_eq!(
                bundled_core_version_from_manifest(&raw, windows, false).unwrap(),
                "1.0.0.polaris.2"
            );
        }
        assert_eq!(
            bundled_core_version_from_manifest(&raw, false, true).unwrap(),
            "1.0.0"
        );
    }
}

#[test]
fn partial_source_inputs_preserve_the_original_platform_baselines() {
    for field in [
        "sourceManifestSha256",
        "provisionerSha256",
        "sourceReceiptFingerprint",
        "moduleGraphSha256",
        "patchedSourceTree",
        "buildTree",
        "version",
        "dependencyModules",
        "transportPins",
        "platforms",
    ] {
        let mut fixture = source_baseline_fixture();
        fixture["sourceBuild"][field] = serde_json::Value::Null;
        let raw = fixture.to_string();
        assert_eq!(
            bundled_core_version_from_manifest(&raw, false, false).unwrap(),
            "1.0.0"
        );
        assert_eq!(
            bundled_core_version_from_manifest(&raw, true, false).unwrap(),
            "1.0.0.polaris.1"
        );
    }
    for key in ["linux", "win", "mac-x64", "mac-arm64"] {
        for field in ["patchedModules", "transportModules"] {
            let mut fixture = source_baseline_fixture();
            fixture["sourceBuild"]["platforms"][key][field] = serde_json::Value::Null;
            for (windows, expected) in [(false, "1.0.0"), (true, "1.0.0.polaris.1")] {
                assert_eq!(
                    bundled_core_version_from_manifest(&fixture.to_string(), windows, false)
                        .unwrap(),
                    expected
                );
            }
        }
    }
}

#[test]
fn malformed_source_input_identity_is_not_a_frozen_baseline() {
    let cases = [
        ("version", serde_json::json!("1.0.0.polaris.0")),
        ("version", serde_json::json!("1.0.1.polaris.2")),
        ("sourceManifestSha256", serde_json::json!("A".repeat(64))),
        ("sourceManifestSha256", serde_json::json!(["a".repeat(64)])),
        (
            "transportPins",
            serde_json::json!({ "example.com/tiny": 1 }),
        ),
        (
            "dependencyModules",
            serde_json::json!(["example.com/tiny", "example.com/tiny"]),
        ),
        (
            "dependencyModules",
            serde_json::json!(["../module+invalid"]),
        ),
    ];
    for (field, value) in cases {
        let mut fixture = source_baseline_fixture();
        fixture["sourceBuild"][field] = value;
        assert_eq!(
            bundled_core_version_from_manifest(&fixture.to_string(), false, false).unwrap(),
            "1.0.0"
        );
    }
    let mut fixture = source_baseline_fixture();
    fixture["sourceBuild"]["platforms"]
        .as_object_mut()
        .unwrap()
        .remove("mac-x64");
    assert_eq!(
        bundled_core_version_from_manifest(&fixture.to_string(), false, false).unwrap(),
        "1.0.0"
    );
    for key in ["linux", "win", "mac-x64", "mac-arm64"] {
        for (field, policy) in [
            (
                "patchedModules",
                serde_json::json!({ "requiredLinked": ["example.com/patched", "example.com/patched", "example.com/second", "example.com/tiny"], "allowedAbsent": [] }),
            ),
            (
                "patchedModules",
                serde_json::json!({ "requiredLinked": ["example.com/patched", "example.com/tiny"], "allowedAbsent": ["example.com/second", "example.com/patched"] }),
            ),
            (
                "patchedModules",
                serde_json::json!({ "requiredLinked": ["example.com/tiny", "example.com/second"], "allowedAbsent": [] }),
            ),
            (
                "patchedModules",
                serde_json::json!({ "requiredLinked": ["example.com/patched", "example.com/second", "example.com/tiny", "unknown/module"], "allowedAbsent": [] }),
            ),
            (
                "patchedModules",
                serde_json::json!({ "requiredLinked": ["example.com/patched", "example.com/tiny", "example.com/second"], "allowedAbsent": [], "allowUnknownMissing": true }),
            ),
            (
                "transportModules",
                serde_json::json!({ "requiredLinked": ["example.com/transport", "example.com/transport"], "confirmedAbsent": ["example.com/linux-transport"] }),
            ),
            (
                "transportModules",
                serde_json::json!({ "requiredLinked": ["example.com/transport"], "confirmedAbsent": ["example.com/transport", "example.com/linux-transport"] }),
            ),
            (
                "transportModules",
                serde_json::json!({ "requiredLinked": [], "confirmedAbsent": ["example.com/linux-transport"] }),
            ),
            (
                "transportModules",
                serde_json::json!({ "requiredLinked": ["example.com/transport"], "confirmedAbsent": ["example.com/linux-transport", "unknown/transport"] }),
            ),
            (
                "transportModules",
                serde_json::json!({ "requiredLinked": ["example.com/transport", "example.com/linux-transport"], "confirmedAbsent": [], "allowUnknownMissing": true }),
            ),
        ] {
            let mut fixture = source_baseline_fixture();
            fixture["sourceBuild"]["platforms"][key][field] = policy;
            for (windows, expected) in [(false, "1.0.0"), (true, "1.0.0.polaris.1")] {
                assert_eq!(
                    bundled_core_version_from_manifest(&fixture.to_string(), windows, false)
                        .unwrap(),
                    expected
                );
            }
        }
    }
    // 分区完整、不相交，仅因 allowedAbsent 非空：任一平台、任一模块都回落。
    for key in ["linux", "win", "mac-x64", "mac-arm64"] {
        for absent in PATCHED_FIXTURE {
            let required: Vec<_> = PATCHED_FIXTURE
                .into_iter()
                .filter(|module| *module != absent)
                .collect();
            let mut fixture = source_baseline_fixture();
            fixture["sourceBuild"]["platforms"][key]["patchedModules"] =
                serde_json::json!({ "requiredLinked": required, "allowedAbsent": [absent] });
            assert_eq!(
                bundled_core_version_from_manifest(&fixture.to_string(), false, false).unwrap(),
                "1.0.0",
                "{key} {absent}"
            );
        }
    }
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

/// 嵌入清单的补丁集标识就是 `sourceBuild.patchedSourceTree`，且运行时访问器原样给出。
#[test]
fn bundled_patch_set_comes_from_the_embedded_manifest() {
    let manifest: serde_json::Value = serde_json::from_str(CORE_MANIFEST_JSON).unwrap();
    let expected = manifest["sourceBuild"]["patchedSourceTree"]
        .as_str()
        .expect("sourceBuild.patchedSourceTree");
    assert_eq!(expected.len(), 40);
    assert_eq!(
        bundled_core_patch_set_from_manifest(CORE_MANIFEST_JSON, false).as_deref(),
        Some(expected)
    );
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    assert_eq!(rt(tmp.path()).bundled_core_patch_set(), Some(expected));
}

/// 清单没被认作冻结清单（版本回落上游基线）时不给补丁集标识；移动端恒不给。
#[test]
fn patch_set_is_withheld_when_the_manifest_is_not_frozen() {
    let frozen = source_baseline_fixture();
    let raw = frozen.to_string();
    let tree = frozen["sourceBuild"]["patchedSourceTree"].as_str().unwrap();
    assert_eq!(
        bundled_core_patch_set_from_manifest(&raw, false).as_deref(),
        Some(tree),
        "对照：冻结清单必须给出标识，否则下面几条的 None 没有信息量"
    );
    assert_eq!(bundled_core_patch_set_from_manifest(&raw, true), None);

    let mut partial = frozen.clone();
    partial["sourceBuild"]["moduleGraphSha256"] = serde_json::Value::Null;
    assert_eq!(
        bundled_core_patch_set_from_manifest(&partial.to_string(), false),
        None
    );
    let mut absent = frozen;
    absent.as_object_mut().unwrap().remove("sourceBuild");
    assert_eq!(
        bundled_core_patch_set_from_manifest(&absent.to_string(), false),
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
