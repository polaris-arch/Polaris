use super::super::{builtin_geo_rulesets, SeedOptions};

#[test]
fn android_uses_extracted_apk_assets_under_app_data() {
    let config = std::path::Path::new("/data/user/10/com.polaris2.app/polaris");
    assert_eq!(
        super::super::android_bundled_data_dir(config).unwrap(),
        std::path::PathBuf::from("/data/user/10/com.polaris2.app/bundled-geo")
    );
    let kotlin = crate::test_support::crate_file(
        "gen/android/app/src/main/java/com/polaris2/app/vpn/BundledRules.kt",
    );
    assert!(kotlin.contains("File(context.dataDir, EXTRACTED_DIR)"));
    assert!(kotlin.contains("EXTRACTED_DIR = \"bundled-geo\""));
    assert!(kotlin.contains("ASSET_DIR = \"_up_/resources/data\""));
}

/// 随包目录解析必须在**当前仓库布局**下命中 `resources/data`（开发态第 ④ 候选）。
/// 这条不测，`seed_builtin_rule_sets_into` 会一路静默 early-return（「解析不到就跳过」），
/// T1 等于没做——而且日志只有一行 warn，极易被当噪音忽略。
#[test]
fn resolves_repo_resources_data_dir() {
    let dir = super::super::resolve_bundled_data_dir().expect("开发态应解析到 resources/data");
    assert!(dir.is_dir(), "{} 应是目录", dir.display());
    assert!(
        dir.join("geosite-cn.srs").is_file(),
        "{} 下应有随包 geosite-cn.srs",
        dir.display()
    );
}

/// **随包资源完整性门**（本批最贴近根因的一条）：拿**真实** `resources/data` 播种到临时目录，
/// `still_missing` 必须为空 —— 即每个 `builtin_geo_rulesets()` 条目都有一份魔数有效的出厂副本。
///
/// 原始缺陷正是「`resources/data` 零 `.srs` 随包」，而此前全仓**没有任何门**盯着这件事：金样把
/// 文件存在性 stub 成恒真，于是「出厂就少文件」这条腿完全在射程外。本用例是它的守门人——
/// 谁再往 `builtin_geo_rulesets()` 加 tag 却忘了补 `.srs`，这里立刻转红，不必等真机全量直连。
#[test]
fn real_bundled_resources_cover_every_builtin_tag() {
    let bundled = super::super::resolve_bundled_data_dir().expect("开发态应解析到 resources/data");
    let runtime =
        std::env::temp_dir().join(format!("polaris-geoseed-realbundle-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&runtime);

    let report = super::super::seed_builtin_rule_sets(&bundled, &runtime, &SeedOptions::default());

    assert!(
        report.still_missing.is_empty(),
        "随包 resources/data 缺少这些内置 geo 的出厂副本：{:?}\n\
             （后果：runtime rules 目录种不满 → route builder fail-closed 剪掉引用它们的规则）",
        report.still_missing
    );
    assert!(!report.seeded.is_empty(), "空目录首播不该零落盘");
    let _ = std::fs::remove_dir_all(&runtime);
}

/// **R1：release 构建必须剔除源码仓候选**（`CARGO_MANIFEST_DIR/../resources/data`）。
///
/// 留着它，打包机上（仓库 rsync 到本机 + `.app` 装进同机 `/Applications`）即便 bundle 里一个
/// `.srs` 都没有，播种也会经这条腿从源码仓成功 ⇒ 验证者看到 28 个 `.srs` 判「打包态 OK」，
/// 而没有仓库的终端用户拿到零 `.srs`。**这是「打包态验证假绿」的产地。**
///
/// 变异锁：把 `filter_repo_candidate` 的 `if is_release` 改成 `if false` → 转红。
#[test]
fn drops_repo_candidate_only_in_release() {
    use std::path::PathBuf;
    let manifest = std::path::Path::new("/opt/build/polaris/src-tauri");
    let bundle_hit =
        PathBuf::from("/Applications/Polaris.app/Contents/Resources/_up_/resources/data");
    let repo_hit = manifest.join("..").join("resources").join("data");
    let all = vec![bundle_hit.clone(), repo_hit.clone()];

    let debug_kept = super::super::filter_repo_candidate(all.clone(), Some(manifest), false);
    assert!(
        debug_kept.contains(&repo_hit),
        "开发态必须保留源码仓候选（否则 `cargo run` 起不来）：{debug_kept:?}"
    );

    let release_kept = super::super::filter_repo_candidate(all, Some(manifest), true);
    assert!(
        !release_kept.contains(&repo_hit),
        "release 必须剔除源码仓候选，否则打包机上「bundle 里没有 .srs」会被源码仓假绿掩盖：{release_kept:?}"
    );
    assert!(
        release_kept.contains(&bundle_hit),
        "剔除不得误伤真正的 bundle 候选：{release_kept:?}"
    );
}

/// **打包期断言与真值表的对账**：`src-tauri/build.rs` 的 `EXPECTED_SRS_COUNT` 是
/// `builtin_geo_rulesets()` 条目数的副本（build script 不引 config-engine，见那里的注释）。
/// 副本必须有门盯着，否则「往表里加 tag → 打包断言仍按旧数放行」= 门有洞。
///
/// 变异锁：把 build.rs 的常量改成 27 或 29 → 本测转红。
#[test]
fn build_rs_expected_count_matches_builtin_table() {
    let build_rs = concat!(env!("CARGO_MANIFEST_DIR"), "/build.rs");
    let src = std::fs::read_to_string(build_rs).expect("读 build.rs");
    let marker = "const EXPECTED_SRS_COUNT: usize = ";
    let declared: usize = src
        .split_once(marker)
        .and_then(|(_, rest)| rest.split(';').next())
        .and_then(|n| n.trim().parse().ok())
        .expect("build.rs 应声明 EXPECTED_SRS_COUNT（打包期随包 geo 资源断言的数量判据）");
    assert_eq!(
        declared,
        builtin_geo_rulesets().len(),
        "build.rs 的 EXPECTED_SRS_COUNT 与 builtin_geo_rulesets() 条目数漂移了：\
             改真值表必须同步改那个常量，否则打包期断言会按旧数量放行（少的那几个 .srs 照样出包）"
    );
}

/// Exercise the exact shipped iOS layout through the production resolver and
/// seeder, not a repo fallback or a bare resources directory beside the App.
#[test]
#[cfg(not(target_os = "android"))]
fn ios_app_assets_resolve_and_seed_all_28_without_history_or_development_tree() {
    let root = super::tmp_root("ios-app-assets");
    let app = root.join("Containers/Bundle/Application/fixture/Polaris.app");
    let shipped = app.join("assets/_up_/resources/data");
    std::fs::create_dir_all(&shipped).expect("create real iOS assets layout");
    std::fs::write(app.join("polaris"), b"inert executable location").expect("create App location");
    let files = super::all_builtin_files();
    assert_eq!(
        files.len(),
        28,
        "formal iOS carrier binds all 28 bundled geo files"
    );
    for (ordinal, name) in files.iter().enumerate() {
        let mut bytes = b"SRS\x01ios-shipped-geo-".to_vec();
        bytes.extend_from_slice(&(ordinal as u32).to_le_bytes());
        std::fs::write(shipped.join(name), bytes).expect("write actual App resource");
    }
    assert!(!app.join("_up_/resources/data").exists());
    assert!(!app.join("resources/data").exists());
    let candidates = super::super::bundled_data_candidates_for(Some(&app), None, true);
    assert_eq!(candidates.first(), Some(&shipped));
    let resolved =
        super::super::resolve_data_candidates(candidates).expect("shipped iOS directory resolves");
    assert_eq!(resolved, shipped);
    let runtime = root.join("Containers/Data/Application/fixture/config/rules");
    let report = super::super::seed_builtin_rule_sets(&resolved, &runtime, &SeedOptions::default());
    assert_eq!(report.seeded.len(), 28);
    assert!(report.still_missing.is_empty());
    for name in &files {
        assert!(super::super::is_valid_srs_file(&runtime.join(name)));
        assert_eq!(
            std::fs::read(runtime.join(name)).unwrap(),
            std::fs::read(shipped.join(name)).unwrap()
        );
    }
    let again = super::super::seed_builtin_rule_sets(&resolved, &runtime, &SeedOptions::default());
    assert!(again.seeded.is_empty());
    assert!(again.still_missing.is_empty());
    std::fs::remove_dir_all(root).expect("remove owned fixture only");
}

#[test]
#[cfg(not(target_os = "android"))]
fn ios_shipped_assets_precede_legacy_and_release_never_uses_development_geo() {
    let root = super::tmp_root("ios-assets-order");
    let app = root.join("Polaris.app");
    let shipped = app.join("assets/_up_/resources/data");
    let legacy = app.join("_up_/resources/data");
    let manifest = root.join("development/src-tauri");
    let development = manifest.join("../resources/data");
    for dir in [&shipped, &legacy, &manifest, &development] {
        std::fs::create_dir_all(dir).unwrap();
    }
    for name in super::all_builtin_files() {
        std::fs::write(shipped.join(&name), b"SRS\x01new App").unwrap();
        std::fs::write(legacy.join(&name), b"SRS\x01old legacy").unwrap();
        std::fs::write(development.join(name), b"SRS\x01developer tree").unwrap();
    }
    let candidates = super::super::bundled_data_candidates_for(Some(&app), Some(&manifest), true);
    assert!(!candidates.contains(&development));
    let resolved = super::super::resolve_data_candidates(candidates).unwrap();
    assert_eq!(resolved, shipped);
    let runtime = root.join("runtime/rules");
    let report = super::super::seed_builtin_rule_sets(&resolved, &runtime, &SeedOptions::default());
    assert_eq!(report.seeded.len(), 28);
    assert!(report.still_missing.is_empty());
    for name in super::all_builtin_files() {
        assert_eq!(
            std::fs::read(runtime.join(name)).unwrap(),
            b"SRS\x01new App"
        );
    }
    std::fs::remove_dir_all(&shipped).unwrap();
    std::fs::remove_dir_all(&legacy).unwrap();
    assert!(
        super::super::resolve_data_candidates(super::super::bundled_data_candidates_for(
            Some(&app),
            Some(&manifest),
            true
        ))
        .is_none(),
        "release cannot manufacture a shipped resource hit from the development tree"
    );
    std::fs::remove_dir_all(root).expect("remove owned fixture only");
}
