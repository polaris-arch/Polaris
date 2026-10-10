use super::*;

fn asset(name: &str, size: u64) -> GithubAsset {
    GithubAsset {
        name: name.to_string(),
        browser_download_url: format!("https://github.com/o/r/releases/download/x/{name}"),
        size,
        digest: None,
    }
}

#[test]
fn parse_asset_digest_accepts_only_wellformed_sha256() {
    let hex = "a".repeat(64);
    assert_eq!(
        parse_asset_digest(&format!("sha256:{hex}")),
        Some(hex.clone())
    );
    // 大写规范化为小写（便于簿记比对；verify_hex_digest 本身大小写不敏感）。
    assert_eq!(
        parse_asset_digest(&format!("sha256:{}", "AB".repeat(32))),
        Some("ab".repeat(32))
    );
    // **逃逸用例**：长度不对 / 非 hex / 换算法 / 无前缀 / 空 → 一律 None。
    // 若被当成 sha256 传给 verify_hex_digest，会把「拿不到可用摘要」伪装成「校验失败」。
    assert_eq!(parse_asset_digest("sha256:abcd"), None);
    assert_eq!(
        parse_asset_digest(&format!("sha256:{}", "z".repeat(64))),
        None
    );
    assert_eq!(parse_asset_digest(&format!("blake3:{hex}")), None);
    assert_eq!(parse_asset_digest(&hex), None);
    assert_eq!(parse_asset_digest(""), None);
}

#[test]
fn check_app_update_threads_digest_into_sha256_but_tolerates_absence() {
    let hex = "b".repeat(64);
    let json = format!(
        r#"[{{"tag_name":"v2.0.0","published_at":"2024-05-01T00:00:00Z","assets":[
                {{"name":"Polaris_2.0.0_amd64-linux.deb","browser_download_url":"https://x/d","size":9,
                  "digest":"sha256:{hex}"}}]}}]"#
    );
    let r = check_app_update(
        &json,
        "1.0.0",
        false,
        None,
        AssetPlatform::Linux,
        AssetArch::X64,
        false,
    )
    .unwrap();
    let AppUpdateCheck::Available(info) = r else {
        panic!("应有更新");
    };
    assert_eq!(info.sha256.as_deref(), Some(hex.as_str()));

    // 旧 release 无 digest → sha256=None，但**仍然报有更新**（缺摘要不阻断更新）。
    let json = r#"[{"tag_name":"v2.0.0","published_at":"2024-05-01T00:00:00Z","assets":[
            {"name":"Polaris_2.0.0_amd64-linux.deb","browser_download_url":"https://x/d","size":9}]}]"#;
    let r = check_app_update(
        json,
        "1.0.0",
        false,
        None,
        AssetPlatform::Linux,
        AssetArch::X64,
        false,
    )
    .unwrap();
    let AppUpdateCheck::Available(info) = r else {
        panic!("缺 digest 不得让更新消失");
    };
    assert_eq!(info.sha256, None);
}

#[test]
fn asset_digest_is_optional_and_parses_when_present() {
    // 旧 release 无 digest 字段 → 必须解析成功（缺摘要不是错误，回落 Content-Length 校验）。
    let no_digest = r#"[{"tag_name":"v2.0.0","published_at":"2024-05-01T00:00:00Z",
            "assets":[{"name":"Polaris_2.0.0_amd64-linux.deb","browser_download_url":"https://x/d","size":1}]}]"#;
    let rs: Vec<GithubRelease> = serde_json::from_str(no_digest).unwrap();
    assert_eq!(rs[0].assets[0].digest, None);

    let with_digest = r#"[{"tag_name":"v2.0.0","published_at":"2024-05-01T00:00:00Z",
            "assets":[{"name":"a.deb","browser_download_url":"https://x/d","size":1,
            "digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000"}]}]"#;
    let rs: Vec<GithubRelease> = serde_json::from_str(with_digest).unwrap();
    assert!(rs[0].assets[0]
        .digest
        .as_deref()
        .unwrap()
        .starts_with("sha256:"));
}

// ── 更新源常量 / URL ──────────────────────────────────────────────────────

#[test]
fn source_repo_constant_is_the_polaris_repo() {
    // 更新源仓库是全链路的锚：改错 = 检查更新指向错误的 repo（拉不到或拉到别人的 release）。
    assert_eq!(APP_UPDATE_REPO, ("polaris-arch", "Polaris"));
    assert_eq!(
        github_releases_api_url("polaris-arch", "Polaris"),
        "https://api.github.com/repos/polaris-arch/Polaris/releases"
    );
    // 便携资产前缀是三处（package.yml 产出 / 本模块选包 / verify-packaging.mjs 断言）
    // 共用的命名契约，改它必须三处同改，故在此钉死字面值。
    assert_eq!(PORTABLE_ZIP_SUFFIX, "x64-win-Portable.zip");
    // Android APK 的尾缀同理，两处（android.yml 的 `release-apk` job 产出 / 本模块选包）
    // 共用；下面 `the_ci_asset_name_is_exactly_what_the_selector_picks` 从 workflow 原文
    // 反向对拍一次，这里只钉本侧的字面值。
    assert_eq!(ANDROID_APK_SUFFIX, "arm64-v8a-android.apk");
}

#[test]
fn platform_and_arch_mapping_from_std_env_consts() {
    assert_eq!(
        AssetPlatform::from_os("windows"),
        Some(AssetPlatform::Windows)
    );
    assert_eq!(AssetPlatform::from_os("macos"), Some(AssetPlatform::Macos));
    assert_eq!(AssetPlatform::from_os("linux"), Some(AssetPlatform::Linux));
    assert_eq!(
        AssetPlatform::from_os("android"),
        Some(AssetPlatform::Android)
    );
    assert_eq!(AssetPlatform::from_os("freebsd"), None);
    assert_eq!(AssetArch::from_arch("x86_64"), AssetArch::X64);
    assert_eq!(AssetArch::from_arch("aarch64"), AssetArch::Arm64);
    assert_eq!(AssetArch::from_arch("riscv64"), AssetArch::Other);
}

// ── App 安装包资产选择真值表（findSuitableUpdateAsset）────────────────────

/// 一个 release 的**真实资产集**（6 个，= README「各恰好一个」那张表 + `package.yml` 的实际产物名）。
///
/// 🔴 用真产物名，不用理想化名字：本函数此前的 Windows 测试用的是
/// `Polaris-0.2.0-win-x64-portable.exe` —— 一个**打包链从未产出过**的名字（便携产物是 zip）。
/// 测试在虚构输入上绿，真资产集下 loose 形态却恒命中 NSIS setup，缺陷因此活过了「全绿」。
fn release_assets() -> Vec<GithubAsset> {
    vec![
        asset("Polaris_0.2.0_x64-win-setup.exe", 100),
        asset("Polaris_0.2.0_x64-win-Portable.zip", 90),
        asset("Polaris_0.2.0_aarch64-mac.dmg", 110),
        asset("Polaris_0.2.0_x64-mac.dmg", 111),
        asset("Polaris_0.2.0_amd64-linux.deb", 80),
        asset("Polaris_0.2.0_amd64-linux.AppImage", 81),
    ]
}

/// 🔴 回归门（2026-07-22 修 #72 形态错配本体）：便携形态必须拿到**便携产物**。
///
/// 变异探针（每条覆盖一条独立逃逸路径，单条不足）：
///  - loose 分支改回共用 `.exe && contains("win")` 候选集 ⇒ ①③ 转红；
///  - loose 分支尾部补任何一级 `.or_else(...)` 回落到 `.exe` ⇒ ② 转红；
///  - 两个分支对调 ⇒ ①④ 转红；
///  - 前缀判据写成大小写不敏感 / `contains(".zip")` 而非 `ends_with` ⇒ ⑤⑥ 转红。
#[test]
fn update_asset_windows_loose_picks_portable_zip_never_an_installer() {
    let assets = release_assets();

    // ① 便携形态：拿到 zip，**不是** setup。
    let loose = find_suitable_update_asset(&assets, AssetPlatform::Windows, AssetArch::X64, true)
        .expect("便携形态必须能选到 Polaris_<版本>_x64-win-Portable.zip");
    assert_eq!(
        loose.name, "Polaris_0.2.0_x64-win-Portable.zip",
        "便携用户拿到安装器 = #72 形态错配本体（装出与便携副本并存的第二份程序）"
    );

    // ② 便携产物缺失（portable 那步挂了）：**必须 None**，绝不回落任一 `.exe`。
    //    宁可不更新，也不发错形态包 —— 与 macOS「不发错架构包」同一条纪律。
    let no_zip: Vec<GithubAsset> = release_assets()
        .into_iter()
        .filter(|a| !a.name.ends_with(".zip"))
        .collect();
    assert!(
        find_suitable_update_asset(&no_zip, AssetPlatform::Windows, AssetArch::X64, true).is_none(),
        "便携形态选不到 zip 时必须返回 None，不得回落 NSIS setup"
    );

    // ③ 安装形态：仍然是 downloadBootstrapper 安装器，**不是** zip（命名契约一字未动）。
    let installed =
        find_suitable_update_asset(&assets, AssetPlatform::Windows, AssetArch::X64, false)
            .expect("安装形态必须能选到 bootstrapper");
    assert_eq!(installed.name, "Polaris_0.2.0_x64-win-setup.exe");

    // ④ 安装形态下便携 zip 存在也绝不被选中（两条规则不相交）。
    assert!(!installed.name.ends_with(".zip"));

    // ⑤ 前缀大小写敏感：非 `package.yml` 那个字面名的 zip 不得被当成便携产物。
    let wrong_case = vec![asset("Polaris-Portable-v0.2.0.zip", 90)];
    assert!(
        find_suitable_update_asset(&wrong_case, AssetPlatform::Windows, AssetArch::X64, true)
            .is_none(),
        "判据须与 package.yml 的字面产物名同口径（大小写敏感）"
    );

    // ⑥ `--clobber` 失效产生的 `.zip.1` 重复资产不得被选中（判据是 `ends_with`，不是 `contains`）。
    let clobber_dupe = vec![asset("Polaris_0.2.0_x64-win-Portable.zip.1", 90)];
    assert!(
        find_suitable_update_asset(&clobber_dupe, AssetPlatform::Windows, AssetArch::X64, true)
            .is_none(),
        "`.zip.1` 不是可用产物，不得被便携规则命中"
    );
}

#[test]
fn update_asset_windows_none_when_no_win_exe() {
    let assets = vec![
        asset("Polaris-0.2.0.dmg", 100),
        asset("Polaris_0.2.0_amd64-linux.AppImage", 100),
    ];
    assert!(
        find_suitable_update_asset(&assets, AssetPlatform::Windows, AssetArch::X64, false)
            .is_none()
    );
    // 便携形态同样无适配（没有 zip）。
    assert!(
        find_suitable_update_asset(&assets, AssetPlatform::Windows, AssetArch::X64, true).is_none()
    );
}

/// 非 Windows 平台的选包**不受本次改动影响**：同一份真实资产集下 mac 双架构各自命中、
/// Linux 双形态各自命中。（防「顺手把 zip 规则套到别的平台」这类逃逸。）
#[test]
fn update_asset_other_platforms_unaffected_by_windows_portable_rule() {
    let assets = release_assets();
    let arm =
        find_suitable_update_asset(&assets, AssetPlatform::Macos, AssetArch::Arm64, true).unwrap();
    assert_eq!(arm.name, "Polaris_0.2.0_aarch64-mac.dmg");
    let x64 =
        find_suitable_update_asset(&assets, AssetPlatform::Macos, AssetArch::X64, false).unwrap();
    assert_eq!(x64.name, "Polaris_0.2.0_x64-mac.dmg");
    let loose_linux =
        find_suitable_update_asset(&assets, AssetPlatform::Linux, AssetArch::X64, true).unwrap();
    assert_eq!(loose_linux.name, "Polaris_0.2.0_amd64-linux.AppImage");
    let inst_linux =
        find_suitable_update_asset(&assets, AssetPlatform::Linux, AssetArch::X64, false).unwrap();
    assert_eq!(inst_linux.name, "Polaris_0.2.0_amd64-linux.deb");
}

/// 正常路径：双 dmg 齐全时两个架构各自命中，互不交叉。
///
/// 名字用 `package.yml` 的 `Normalize macOS release asset name` 真正产出的命名，
/// 避免测试用理想化名字通过、真产物名不通过。
#[test]
fn update_asset_macos_picks_own_arch_dmg_when_both_present() {
    let assets = vec![
        asset("Polaris_0.2.0_aarch64-mac.dmg", 100),
        asset("Polaris_0.2.0_x64-mac.dmg", 100),
    ];
    let arm =
        find_suitable_update_asset(&assets, AssetPlatform::Macos, AssetArch::Arm64, false).unwrap();
    assert_eq!(arm.name, "Polaris_0.2.0_aarch64-mac.dmg");
    let x64 =
        find_suitable_update_asset(&assets, AssetPlatform::Macos, AssetArch::X64, false).unwrap();
    assert_eq!(x64.name, "Polaris_0.2.0_x64-mac.dmg");
    // 形态（loose_form）不参与 macOS 选包：两个形态选到同一份。
    assert_eq!(
        find_suitable_update_asset(&assets, AssetPlatform::Macos, AssetArch::Arm64, true)
            .unwrap()
            .name,
        arm.name
    );
}

/// 🔴 回归门（2026-07-21 用户裁定「宁可不更新，也不发错架构包」）：
/// release 里某个架构的 dmg 缺失（该 mac job 挂掉）时，**必须返回 None**，
/// 不得回落到另一架构那份 —— 否则 x64 用户会静默拿到 arm64 包。
///
/// 这两条是把 `.or_else(|| assets.iter().find(|a| a.name.ends_with(".dmg")))`
/// 加回去就会转红的变异探针（两个方向各一条，单向探针会漏掉对称的另一半）。
#[test]
fn update_asset_macos_returns_none_rather_than_cross_arch_dmg() {
    // 只剩 arm64 → x64 请求返回 None（不得拿到 arm64 包）。
    let only_arm = vec![asset("Polaris_0.2.0_aarch64-mac.dmg", 100)];
    assert!(
        find_suitable_update_asset(&only_arm, AssetPlatform::Macos, AssetArch::X64, false)
            .is_none(),
        "x64 请求在只有 arm64 dmg 时必须返回 None，不得回落跨架构包"
    );
    // 只剩 x64 → arm64 请求返回 None（对称方向）。
    let only_x64 = vec![asset("Polaris_0.2.0_x64-mac.dmg", 100)];
    assert!(
        find_suitable_update_asset(&only_x64, AssetPlatform::Macos, AssetArch::Arm64, false)
            .is_none(),
        "arm64 请求在只有 x64 dmg 时必须返回 None，不得回落跨架构包"
    );
    // 无架构标记的裸 dmg（改名步失效的产物）同样不得被任一架构选中。
    let untagged = vec![asset("Polaris_0.2.0_aarch64.dmg", 100)];
    assert!(
        find_suitable_update_asset(&untagged, AssetPlatform::Macos, AssetArch::Arm64, false)
            .is_none()
    );
    assert!(
        find_suitable_update_asset(&untagged, AssetPlatform::Macos, AssetArch::X64, false)
            .is_none()
    );
}

#[test]
fn update_asset_linux_loose_picks_appimage_installed_picks_deb() {
    let assets = vec![
        asset("Polaris_0.2.0_amd64-linux.deb", 100),
        asset("Polaris_0.2.0_amd64-linux.AppImage", 90),
    ];
    assert!(
        find_suitable_update_asset(&assets, AssetPlatform::Linux, AssetArch::X64, true)
            .unwrap()
            .name
            .ends_with(".AppImage")
    );
    assert!(
        find_suitable_update_asset(&assets, AssetPlatform::Linux, AssetArch::X64, false)
            .unwrap()
            .name
            .ends_with(".deb")
    );
    // installed 但只有 AppImage → 回落 AppImage。
    let only_img = vec![asset("Polaris_0.2.0_amd64-linux.AppImage", 90)];
    assert!(
        find_suitable_update_asset(&only_img, AssetPlatform::Linux, AssetArch::X64, false)
            .unwrap()
            .name
            .ends_with(".AppImage")
    );
}

// ── App 检查全链路（check_app_update）─────────────────────────────────────

/// 两个 release 的样本 JSON（GitHub 原始形状）：0.1.0（旧）+ 0.2.0（新，含多平台资产）。
fn sample_releases_json() -> String {
    r#"[
          {
            "tag_name": "v0.2.0",
            "name": "Polaris 0.2.0",
            "body": "新版说明",
            "prerelease": false,
            "published_at": "2024-05-01T12:00:00Z",
            "assets": [
              {"name": "Polaris_0.2.0_aarch64-mac.dmg", "browser_download_url": "https://x/mac", "size": 12345},
              {"name": "Polaris_0.2.0_x64-win-setup.exe", "browser_download_url": "https://x/win", "size": 999},
              {"name": "Polaris_0.2.0_amd64-linux.deb", "browser_download_url": "https://x/deb", "size": 555}
            ]
          },
          {
            "tag_name": "v0.1.0",
            "name": "Polaris 0.1.0",
            "prerelease": false,
            "published_at": "2024-01-01T00:00:00Z",
            "assets": []
          }
        ]"#
        .to_string()
}

#[test]
fn check_app_update_returns_available_with_faithful_fields() {
    let json = sample_releases_json();
    let r = check_app_update(
        &json,
        "0.1.0",
        false,
        None,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    match r {
        AppUpdateCheck::Available(info) => {
            assert_eq!(
                info.version, "v0.2.0",
                "version 保留原始 tag（含 v，对齐 上游）"
            );
            assert_eq!(info.title, "Polaris 0.2.0");
            assert_eq!(info.release_notes, "新版说明");
            assert_eq!(info.download_url, "https://x/mac");
            assert_eq!(info.file_size, 12345);
            assert_eq!(info.published_at, "2024-05-01T12:00:00Z");
            assert!(!info.is_prerelease);
            assert_eq!(info.file_name, "Polaris_0.2.0_aarch64-mac.dmg");
        }
        AppUpdateCheck::NoUpdate => panic!("应发现 0.2.0 更新"),
    }
}

#[test]
fn check_app_update_no_update_when_current_is_latest() {
    let json = sample_releases_json();
    // 当前已是 0.2.0 → 无更新。
    let r = check_app_update(
        &json,
        "0.2.0",
        false,
        None,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert_eq!(r, AppUpdateCheck::NoUpdate);
}

#[test]
fn resolve_current_app_release_returns_only_the_same_channel_latest() {
    let json = sample_releases_json();
    let current = resolve_current_app_release(
        &json,
        "0.2.0",
        false,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert!(matches!(current, AppUpdateCheck::Available(ref i) if i.version == "v0.2.0"));

    let older = resolve_current_app_release(
        &json,
        "0.3.0",
        false,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert_eq!(older, AppUpdateCheck::NoUpdate, "不得借重装入口降级");

    let newer = resolve_current_app_release(
        &json,
        "0.1.0",
        false,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert_eq!(
        newer,
        AppUpdateCheck::NoUpdate,
        "真正的新版本应走普通更新入口"
    );
}

#[test]
fn check_app_update_skipped_version_is_no_update() {
    let json = sample_releases_json();
    // 跳过 0.2.0（存的是去 v 的版本）→ 无更新。
    let r = check_app_update(
        &json,
        "0.1.0",
        false,
        Some("0.2.0"),
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert_eq!(r, AppUpdateCheck::NoUpdate);

    // W8 反例（同口径门的另一半）：存**原始 tag**（`v0.2.0`，修复前两个写点的实存形态）
    // ⇒ 与比较侧 strip_v 后的值永不相等 ⇒ 照常报 Available。若有人把比较侧改回原始 tag
    // 来「修」跳过，这半句转红——口径必须由写侧归一化（stored_skip_version），
    // 不是把比侧改脏。
    let raw = check_app_update(
        &json,
        "0.1.0",
        false,
        Some("v0.2.0"),
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert!(
        matches!(raw, AppUpdateCheck::Available(_)),
        "原始 tag 不该命中跳过——命中了说明比较侧被改回原始 tag 口径"
    );
}

#[test]
fn check_app_update_prerelease_filtered_unless_included() {
    let json = r#"[
          {"tag_name":"v0.3.0-beta.1","prerelease":true,"published_at":"2024-06-01T00:00:00Z",
           "assets":[{"name":"Polaris_0.3.0_aarch64-mac.dmg","browser_download_url":"https://x/beta","size":1}]},
          {"tag_name":"v0.2.0","prerelease":false,"published_at":"2024-05-01T00:00:00Z",
           "assets":[{"name":"Polaris_0.2.0_aarch64-mac.dmg","browser_download_url":"https://x/stable","size":1}]}
        ]"#;
    // include_prerelease=false → beta 被过滤，最新正式 = 0.2.0。
    let stable = check_app_update(
        json,
        "0.1.0",
        false,
        None,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert!(matches!(stable, AppUpdateCheck::Available(ref i) if i.version == "v0.2.0"));
    // include_prerelease=true → 取最新发布 = beta。
    let beta = check_app_update(
        json,
        "0.1.0",
        true,
        None,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert!(matches!(beta, AppUpdateCheck::Available(ref i) if i.version == "v0.3.0-beta.1"));
}

#[test]
fn check_app_update_no_suitable_asset_is_no_update() {
    // 有更新但当前平台无适配资产（release 只有 mac dmg，平台是 windows）→ 无更新（非报错）。
    let json = sample_releases_json();
    let r = check_app_update(
        &json,
        "0.1.0",
        false,
        None,
        AssetPlatform::Windows,
        AssetArch::X64,
        false,
    )
    .unwrap();
    // 注：sample 里有 win-x64.exe → windows 应能匹配到 setup 包，故这里改用只有 dmg 的 release 断言无资产。
    // （sample 覆盖三平台，windows 有 setup → 会命中。故此断言其实是 Available；用独立样本测无资产。）
    assert!(matches!(r, AppUpdateCheck::Available(_)));

    let mac_only = r#"[{"tag_name":"v0.2.0","prerelease":false,"published_at":"2024-05-01T00:00:00Z",
          "assets":[{"name":"Polaris_0.2.0_aarch64-mac.dmg","browser_download_url":"https://x/mac","size":1}]}]"#;
    let none = check_app_update(
        mac_only,
        "0.1.0",
        false,
        None,
        AssetPlatform::Windows,
        AssetArch::X64,
        false,
    )
    .unwrap();
    assert_eq!(none, AppUpdateCheck::NoUpdate);
}

/// 全链路：Windows 便携形态请求 → `updateInfo` 指向便携 zip（下载 URL / 文件名都是它）。
///
/// 只测 `find_suitable_update_asset` 不够：`check_app_update` 是宿主真正调的入口，
/// 形态参数得**一路传到底**才有意义（少传一层 = 选包器修好了、用户仍拿安装器）。
#[test]
fn check_app_update_windows_loose_form_yields_portable_zip() {
    let json = r#"[{"tag_name":"v0.2.0","prerelease":false,"published_at":"2024-05-01T00:00:00Z",
          "assets":[
            {"name":"Polaris_0.2.0_x64-win-setup.exe","browser_download_url":"https://x/win","size":1},
            {"name":"Polaris_0.2.0_x64-win-Portable.zip","browser_download_url":"https://x/zip","size":3}]}]"#;

    let loose = check_app_update(
        json,
        "0.1.0",
        false,
        None,
        AssetPlatform::Windows,
        AssetArch::X64,
        true,
    )
    .unwrap();
    let AppUpdateCheck::Available(info) = loose else {
        panic!("便携形态应发现更新（便携 zip 在 release 里）");
    };
    assert_eq!(info.file_name, "Polaris_0.2.0_x64-win-Portable.zip");
    assert_eq!(info.download_url, "https://x/zip");

    // 安装形态在同一份 release 上仍拿 bootstrapper。
    let installed = check_app_update(
        json,
        "0.1.0",
        false,
        None,
        AssetPlatform::Windows,
        AssetArch::X64,
        false,
    )
    .unwrap();
    let AppUpdateCheck::Available(info) = installed else {
        panic!("安装形态应发现更新");
    };
    assert_eq!(info.file_name, "Polaris_0.2.0_x64-win-setup.exe");

    // 便携产物缺失 ⇒ 便携用户如实「无更新」，**不是**被发安装器。
    let no_zip = r#"[{"tag_name":"v0.2.0","prerelease":false,"published_at":"2024-05-01T00:00:00Z",
          "assets":[{"name":"Polaris_0.2.0_x64-win-setup.exe","browser_download_url":"https://x/win","size":1}]}]"#;
    let r = check_app_update(
        no_zip,
        "0.1.0",
        false,
        None,
        AssetPlatform::Windows,
        AssetArch::X64,
        true,
    )
    .unwrap();
    assert_eq!(r, AppUpdateCheck::NoUpdate);
}

#[test]
fn check_app_update_malformed_json_errors() {
    let err = check_app_update(
        "{not json",
        "0.1.0",
        false,
        None,
        AssetPlatform::Linux,
        AssetArch::X64,
        false,
    )
    .unwrap_err();
    assert!(matches!(err, ManifestError::ParseJson(_)));
}

#[test]
fn check_app_update_empty_releases_is_no_update() {
    let r = check_app_update(
        "[]",
        "0.1.0",
        false,
        None,
        AssetPlatform::Linux,
        AssetArch::X64,
        false,
    )
    .unwrap();
    assert_eq!(r, AppUpdateCheck::NoUpdate);
}

/*
 * ── 「只比版本」那条检查腿（Android 等无安装包目标的平台）────────────────────────────
 *
 * 这一组守的缺陷是**结构性静默**：`AssetPlatform::from_os("android")` 返 `None`，
 * 而 `check_app_update` 的资产选择在这一档恒 `None` ⇒ 整条检查恒答「已是最新」。
 * 命令层此前更早一步就 `return hasUpdate:false`，连请求都不发。
 * 下面第一条正是那个缺陷的直接对照：**同一份 release，有资产的平台报有更新，Android 也必须报有更新**。
 */

/// Android 这一档的 release JSON：本仓今天不出 APK 资产（`.github/workflows/android.yml`），
/// 故样本里**故意一个 Android 能用的资产都没有**。
fn desktop_only_release_json() -> String {
    r#"[{"tag_name":"v9.9.9","name":"Polaris 9.9.9","body":"notes","prerelease":false,
          "published_at":"2026-09-01T00:00:00Z","assets":[
            {"name":"Polaris_9.9.9_x64-win-setup.exe","browser_download_url":"https://x/win","size":7}]}]"#
        .to_string()
}

#[test]
fn check_app_update_release_only_reports_update_without_any_asset() {
    let json = desktop_only_release_json();

    // 反向对照：拿「要选资产」的那条腿去问 Android 那一档会得到什么 —— 没有 apk 资产 ⇒ 无更新。
    // （命令层此前更早一步就早退，症状相同：结构性恒「已是最新」。）
    let via_assets = check_app_update(
        &json,
        "1.0.0",
        false,
        None,
        AssetPlatform::Macos,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    assert!(
        matches!(via_assets, AppUpdateCheck::NoUpdate),
        "对照塌了：这份样本本该选不出 mac 资产"
    );

    // 正题：只比版本那条腿必须如实报「有更新」。
    let r = check_app_update_release_only(&json, "1.0.0", false, None).unwrap();
    let AppUpdateCheck::Available(info) = r else {
        panic!("有比当前新的 release，却报了「已是最新」——Android 上的更新检查恒假就是这个形态");
    };
    assert_eq!(info.version, "v9.9.9");
    assert_eq!(info.title, "Polaris 9.9.9");
    assert_eq!(info.release_notes, "notes");
    assert_eq!(info.published_at, "2026-09-01T00:00:00Z");
    assert!(!info.is_prerelease);
}

#[test]
fn check_app_update_release_only_has_no_asset_fields() {
    // 三个资产字段必须**如实为空**：这一档没有选中的资产，编一个 URL 出来会让下载腿
    // 去下一个不存在的东西。用户出口是发布页链接。
    let r =
        check_app_update_release_only(&desktop_only_release_json(), "1.0.0", false, None).unwrap();
    let AppUpdateCheck::Available(info) = r else {
        panic!("应有更新");
    };
    assert_eq!(info.download_url, "");
    assert_eq!(info.file_name, "");
    assert_eq!(info.file_size, 0);
    assert_eq!(info.sha256, None);
}

#[test]
fn check_app_update_release_only_shares_the_first_four_gates() {
    let json = desktop_only_release_json();
    // ① 已是最新 / 更旧 ⇒ 无更新（不是「有资产就报」）。
    assert!(matches!(
        check_app_update_release_only(&json, "9.9.9", false, None).unwrap(),
        AppUpdateCheck::NoUpdate
    ));
    assert!(matches!(
        check_app_update_release_only(&json, "10.0.0", false, None).unwrap(),
        AppUpdateCheck::NoUpdate
    ));
    // ② 用户跳过了这个版本 ⇒ 无更新（比较侧同样走 strip_v）。
    assert!(matches!(
        check_app_update_release_only(&json, "1.0.0", false, Some("9.9.9")).unwrap(),
        AppUpdateCheck::NoUpdate
    ));
    // ③ 通道过滤：预发布版在正式通道上不算候选，打开通道才算。
    let pre = r#"[{"tag_name":"v9.9.9-beta.1","prerelease":true,
          "published_at":"2026-09-01T00:00:00Z","assets":[]}]"#;
    assert!(matches!(
        check_app_update_release_only(pre, "1.0.0", false, None).unwrap(),
        AppUpdateCheck::NoUpdate
    ));
    assert!(matches!(
        check_app_update_release_only(pre, "1.0.0", true, None).unwrap(),
        AppUpdateCheck::Available(_)
    ));
    // ④ JSON 坏了 ⇒ 报错，**不是**「已是最新」（把解析失败折成无更新是本模块最不能犯的错）。
    assert!(check_app_update_release_only("not json", "1.0.0", false, None).is_err());
}

// ── Android 资产选择（2026-09-13：APK 开始作为 release 资产发布）────────────────

/// 一个**同时带桌面与 Android 资产**的 release 资产集。
///
/// APK 名取的是 `.github/workflows/android.yml` 的 `release-apk` job 真会产出的那个形态
/// （`Polaris_<版本>_arm64-v8a-android.apk`），不是理想化名字 —— 同一条纪律见
/// [`release_assets`] 的 🔴：在虚构输入上绿的选包测试挡不住真产物上的错配。
fn release_assets_with_apk() -> Vec<GithubAsset> {
    let mut assets = release_assets();
    assets.push(asset("Polaris_0.2.0_arm64-v8a-android.apk", 210));
    assets
}

/// 本夹具含 ARMv8 原生包；`loose_form` 在 Android 态上不参与。
///
/// 变异探针（每条各覆盖一条独立逃逸路径）：
///  - 放开非 ARM 架构闸 ⇒ ③ 转红（x86_64 模拟器会拿到 arm64 包）；
///  - 把 `ends_with(ANDROID_APK_SUFFIX)` 换成 `ends_with(".apk")` ⇒ ④ 转红（拿到别的架构包）；
///  - 给 Android 分支补任何一级回落（`.or_else(|| assets.first())`）⇒ ⑤ 转红。
#[test]
fn update_asset_android_picks_the_arm64_apk_and_nothing_else() {
    let assets = release_assets_with_apk();

    // ① arm64：拿到 APK。
    let picked =
        find_suitable_update_asset(&assets, AssetPlatform::Android, AssetArch::Arm64, false)
            .expect("arm64 Android 必须选得到 APK");
    assert_eq!(picked.name, "Polaris_0.2.0_arm64-v8a-android.apk");

    // ② `loose_form` 不参与：Android 应用只有一种形态（由系统包管理器装的）。
    let loose = find_suitable_update_asset(&assets, AssetPlatform::Android, AssetArch::Arm64, true)
        .expect("loose_form 不该改变 Android 的结果");
    assert_eq!(loose.name, picked.name);

    // ③ 非 arm64（x86_64 模拟器）：**无包**，且绝不回落到 arm64 那个包。
    assert!(
        find_suitable_update_asset(&assets, AssetPlatform::Android, AssetArch::X64, false)
            .is_none(),
        "x86_64 Android 上选出了包 —— 正式发布只含 ARMv8/ARMv7，发给模拟器的必然装不上"
    );
    assert!(
        find_suitable_update_asset(&assets, AssetPlatform::Android, AssetArch::Other, false)
            .is_none()
    );

    // ④ 另一个架构的 APK **不许**被 arm64 选中（后缀判据不是「是个 apk 就行」）。
    let wrong_arch = vec![asset("polaris-0.2.0-android-x86_64.apk", 200)];
    assert!(
        find_suitable_update_asset(&wrong_arch, AssetPlatform::Android, AssetArch::Arm64, false)
            .is_none(),
        "按 `.apk` 而不是按后缀契约选包 ⇒ 会把 x86_64 包发给真机"
    );

    // ⑤ 一个 APK 都没有（2026-09-13 之前的每一个 release 都长这样）：无包，**不回落**到
    //    桌面那些资产。这一档不是「没有更新」，而是「有更新但没有你能装的包」——
    //    分辨这两件事是 `update_check` 的 Android 腿再问一次
    //    `check_app_update_release_only` 的全部理由。
    assert!(
        find_suitable_update_asset(
            &release_assets(),
            AssetPlatform::Android,
            AssetArch::Arm64,
            false
        )
        .is_none(),
        "没有 APK 却选出了东西 —— 那会让下载腿去下一个 .exe/.dmg"
    );
}

/// 🔴 **跨文件命名契约的对拍**：CI 真会产出的那个资产名，必须正好是选包器会选中的那个。
///
/// # 为什么必须是这条形态（而不是两边各钉一个字面量）
///
/// 两端各写一份字面量、各自有测试，是本仓反复吃过亏的形态：改一边、另一边的测试照绿，
/// 而故障是**静默**的 —— 选包器选不中 ⇒ `AppUpdateCheck::NoUpdate` ⇒ Android 上「检查更新」
/// 永远回答「已是最新」，一句错误都不报。故这里把 workflow 的**原文**当输入：
/// 取出那一行 `asset_name=…`，把 `${version}` 展开成一个真实版本号，喂进真的选包器。
///
/// 取材自检（`assert!(line.contains(...))`）承重：workflow 改形状 ⇒ 取不到那一行 ⇒ 当场 panic，
/// 而不是在一个空串上恒真。
#[test]
fn the_ci_asset_name_is_exactly_what_the_selector_picks() {
    const WORKFLOW: &str = ".github/workflows/android.yml";
    let yaml = polaris_source_probe::expect_marker(
        polaris_source_probe::repo_file!(WORKFLOW),
        WORKFLOW,
        "name: Android",
    );

    // 产出侧：`release-apk` job 里那一行改名。整行取出来，不做模糊匹配。
    let line = yaml
        .lines()
        .map(str::trim)
        .find(|l| l.starts_with("asset_name="))
        .unwrap_or_else(|| {
            panic!(
                "{WORKFLOW}：找不到 `asset_name=` 那一行 —— 资产名的产出侧换形状了，\
                 本条对拍的取材面塌了（而塌了之后 Android 的更新检查会静默恒假）。"
            )
        });
    assert_eq!(
        line, "asset_name=\"Polaris_${version}_${abi}-android.apk\"",
        "{WORKFLOW}：资产名的产出侧变了。它与 `ANDROID_APK_SUFFIX` 是同一条契约的两端，\
         漂一个字符的后果不是报错，是 Android 上永远查不到更新。"
    );

    // 把它展开成一个真实文件名（CI 那一行的 `${version}` 来自包里的 versionName）。
    let name = line
        .trim_start_matches("asset_name=")
        .trim_matches('"')
        .replace("${version}", "9.9.9")
        .replace("${abi}", "arm64-v8a");
    assert_eq!(name, "Polaris_9.9.9_arm64-v8a-android.apk");

    // 消费侧：真的选包器必须选中它。
    let assets = vec![asset(&name, 210)];
    let picked =
        find_suitable_update_asset(&assets, AssetPlatform::Android, AssetArch::Arm64, false)
            .unwrap_or_else(|| {
                panic!("选包器选不中 CI 真会产出的那个资产名 `{name}` —— 命名契约两端已经漂了")
            });
    assert_eq!(picked.name, name);

    // 反向对照：把契约后缀改掉一个字符，选包器就该选不中（证明上面那条不是恒真）。
    let drifted = vec![asset(&name.replace("arm64-v8a", "aarch64"), 210)];
    assert!(
        find_suitable_update_asset(&drifted, AssetPlatform::Android, AssetArch::Arm64, false)
            .is_none(),
        "对照塌了：选包器对任何 .apk 都点头，那这条对拍证明不了什么"
    );
}

/// 全链路：Android 上「有新版本且有 APK」走资产腿，「有新版本但没 APK」由只比版本那条腿兜。
///
/// 这两档合起来就是 `update_check` 的 Android 分支：先问资产腿，`NoUpdate` 时再问一次
/// 只比版本那条。两条腿共用前四道闸，故「它说有、这条说没有」只可能是资产那一步的差别。
#[test]
fn check_app_update_android_end_to_end_with_and_without_an_apk_asset() {
    let with_apk = r#"[{"tag_name":"v9.9.9","name":"Polaris 9.9.9","body":"notes","prerelease":false,
          "published_at":"2026-09-01T00:00:00Z","assets":[
            {"name":"Polaris_9.9.9_arm64-v8a-android.apk","browser_download_url":"https://x/apk",
             "size":210,"digest":"sha256:c3d4e5f6a7b8091a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d5e6f70819a"}]}]"#;
    let r = check_app_update(
        with_apk,
        "1.0.0",
        false,
        None,
        AssetPlatform::Android,
        AssetArch::Arm64,
        false,
    )
    .unwrap();
    let AppUpdateCheck::Available(info) = r else {
        panic!("有 APK 资产却报了「已是最新」");
    };
    assert_eq!(info.file_name, "Polaris_9.9.9_arm64-v8a-android.apk");
    assert_eq!(info.download_url, "https://x/apk");
    assert_eq!(info.file_size, 210);
    // 摘要必须一路穿到底：下载腿的强校验判据就是它（没有它这一档只剩 Content-Length）。
    assert_eq!(
        info.sha256.as_deref(),
        Some("c3d4e5f6a7b8091a2b3c4d5e6f70819a2b3c4d5e6f70819a2b3c4d5e6f70819a")
    );

    // 没有 APK 的那一档（2026-09-13 之前的每一个 release）：资产腿说「没有」，
    // 只比版本那条腿说「有，但三个资产字段为空」——前端据此只画「打开发布页」，不画「下载」。
    let json = desktop_only_release_json();
    assert!(matches!(
        check_app_update(
            &json,
            "1.0.0",
            false,
            None,
            AssetPlatform::Android,
            AssetArch::Arm64,
            false
        )
        .unwrap(),
        AppUpdateCheck::NoUpdate
    ));
    let AppUpdateCheck::Available(fallback) =
        check_app_update_release_only(&json, "1.0.0", false, None).unwrap()
    else {
        panic!("兜底腿也说没有 —— 那用户会被告知「已是最新」，而新版本确实存在");
    };
    assert_eq!(fallback.version, "v9.9.9");
    assert_eq!(fallback.download_url, "");
}

#[test]
fn android_armv7_native_split_precedes_universal_and_emulators_never_match() {
    assert_eq!(AssetArch::from_arch("arm"), AssetArch::Armv7);
    let assets = vec![
        asset("Polaris_1.0.0_universal-android.apk", 300),
        asset("Polaris_1.0.0_armeabi-v7a-android.apk", 100),
        asset("Polaris_1.0.0_arm64-v8a-android.apk", 200),
    ];
    for (arch, expected) in [
        (AssetArch::Armv7, "Polaris_1.0.0_armeabi-v7a-android.apk"),
        (AssetArch::Arm64, "Polaris_1.0.0_arm64-v8a-android.apk"),
    ] {
        assert_eq!(
            find_suitable_update_asset(&assets, AssetPlatform::Android, arch, false)
                .unwrap()
                .name,
            expected
        );
        assert_eq!(
            find_suitable_update_asset(&assets[..1], AssetPlatform::Android, arch, false)
                .unwrap()
                .name,
            assets[0].name
        );
    }
    for arch in [AssetArch::X64, AssetArch::Other] {
        assert!(find_suitable_update_asset(&assets, AssetPlatform::Android, arch, false).is_none());
    }
}

#[test]
fn every_actual_public_name_from_the_producer_contract_is_selectable() {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/release-assets.mjs");
    let output = std::process::Command::new("node")
        .arg(script)
        .args(["names", "9.9.9", "release-all"])
        .output()
        .expect("run read-only public-name generator");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let names = String::from_utf8(output.stdout).unwrap();
    let assets: Vec<_> = names.lines().map(|name| asset(name, 100)).collect();
    assert_eq!(assets.len(), 9);
    for (platform, arch, loose, suffix) in [
        (
            AssetPlatform::Windows,
            AssetArch::X64,
            false,
            "x64-win-setup.exe",
        ),
        (
            AssetPlatform::Windows,
            AssetArch::X64,
            true,
            "x64-win-Portable.zip",
        ),
        (AssetPlatform::Macos, AssetArch::X64, false, "x64-mac.dmg"),
        (
            AssetPlatform::Macos,
            AssetArch::Arm64,
            false,
            "aarch64-mac.dmg",
        ),
        (
            AssetPlatform::Linux,
            AssetArch::X64,
            false,
            "amd64-linux.deb",
        ),
        (
            AssetPlatform::Linux,
            AssetArch::X64,
            true,
            "amd64-linux.AppImage",
        ),
        (
            AssetPlatform::Android,
            AssetArch::Arm64,
            false,
            "arm64-v8a-android.apk",
        ),
        (
            AssetPlatform::Android,
            AssetArch::Armv7,
            false,
            "armeabi-v7a-android.apk",
        ),
    ] {
        let picked = find_suitable_update_asset(&assets, platform, arch, loose).unwrap();
        assert_eq!(picked.name, format!("Polaris_9.9.9_{suffix}"));
    }
}

#[test]
fn old_public_names_and_malformed_version_names_are_not_upgrade_candidates() {
    for (name, platform, arch, loose) in [
        (
            "polaris-portable-v1.0.0.zip",
            AssetPlatform::Windows,
            AssetArch::X64,
            true,
        ),
        (
            "Polaris_1.0.0_x64-mac-x64.dmg",
            AssetPlatform::Macos,
            AssetArch::X64,
            false,
        ),
        (
            "Polaris_1.0.0_amd64.deb",
            AssetPlatform::Linux,
            AssetArch::X64,
            false,
        ),
        (
            "polaris-1.0.0-android-arm64.apk",
            AssetPlatform::Android,
            AssetArch::Arm64,
            false,
        ),
        (
            "Polaris__arm64-v8a-android.apk",
            AssetPlatform::Android,
            AssetArch::Arm64,
            false,
        ),
        (
            "Polaris_1.0.0_x64-win-portable.zip",
            AssetPlatform::Windows,
            AssetArch::X64,
            true,
        ),
    ] {
        assert!(
            find_suitable_update_asset(&[asset(name, 100)], platform, arch, loose).is_none(),
            "{name}"
        );
    }
}
