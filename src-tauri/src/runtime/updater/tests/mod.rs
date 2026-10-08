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

#[test]
fn pending_change_notice_show_then_ack_clears_once() {
    // 「弹一次非每启」：写入 → 读到 → ack 清除 → 再读为空。
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    let r = rt(tmp.path());
    r.mutate_state(|s| {
        s.pending_change_notice = Some(PendingChangeNotice {
            previous_version: "1.13.13".into(),
            current_version: "1.14.0".into(),
        });
    })
    .unwrap();
    assert!(r.state().pending_change_notice.is_some());

    r.mutate_state(|s| s.pending_change_notice = None).unwrap();
    assert!(r.state().pending_change_notice.is_none());
    // 重启后仍为空（不复活 → 不会每次启动重弹）。
    assert!(rt(tmp.path()).state().pending_change_notice.is_none());
}

#[test]
fn core_version_readers_are_asymmetric_the_150_f1_trap() {
    // **本仓最要紧的一条不变式**（Polaris issue #150 review F1）：
    //   read_core_version_line 探测失败 → ""（诚实失败）
    //   read_core_version      探测失败 → 回落随包基线（会伪装成「活核=基线」）
    // 二者若同语义，reseed 校验就会把「重读失败」当成「换核成功」→ 带旧核硬跑退回死循环。
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    let r = rt(tmp.path());
    // 未注入核路径 = 探测必失败（本机不 spawn 真核，符合「真实安装核不在本机跑」纪律）。
    assert!(r.core_binary_path().is_none());

    assert_eq!(
        r.read_core_version_line(),
        "",
        "失败置空的读法必须返回空串——它是 classify_reseed_result 的唯一合法入参来源"
    );
    assert_eq!(
        r.read_core_version(),
        r.bundled_core_version(),
        "回落基线的读法必须回落基线（刻意保留的上游陷阱语义）"
    );
    // 两者**必须不同**——这正是不对称本身。
    assert_ne!(
        r.read_core_version_line(),
        r.read_core_version(),
        "双读法失败语义一旦对称，#150 F1 的防线即失效"
    );
}

#[test]
fn core_build_kind_unknown_when_probe_fails() {
    // 探测失败 → 版本行空 → classify_core_build 视为 unknown（**不硬判 fork**、更不判 official）。
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    let r = rt(tmp.path());
    assert_eq!(r.core_build_kind(), CoreBuildKind::Unknown);
}

#[test]
fn decide_core_override_never_reseeds_when_probe_fails() {
    // 失败安全：探测不到活核 → unknown → 绝不 reseed（不覆盖用户的核）。
    let tmp = TestDir::new("polaris-updater-runtime-test-");
    let r = rt(tmp.path());
    let d = r.decide_core_override_for("1.13.13");
    assert!(!d.reseed, "unknown 构建绝不 reseed");
    assert!(d.warn, "旧于基线的 unknown 核应 warn");
}
