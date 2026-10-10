use super::*;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

/// 「这个 plan 必须真的产出脚本」的测试侧断言口。
///
/// **不写 `.unwrap()`**：`build_install_script` 返 `None` 只可能来自 [`InstallPlatform::Android`]
/// （交系统安装器）与 [`InstallPlatform::WindowsPortable`]（用户手动覆盖），两者本来就没有脚本。
/// 下面这批用例喂的全是脚本腿，拿到 `None` 说明分派被改坏了，
/// 而 `called Option::unwrap() on a None value` 一个字都说不出来 —— 报错要点名是哪个平台掉了。
fn script_of(plan: &InstallPlan) -> ScriptSpec {
    build_install_script(plan)
        .unwrap_or_else(|| panic!("{:?} 应当有脚本腿，却拿到 None", plan.platform))
}

// ── 资产形态分类 ──

#[test]
fn classify_installer_covers_all_four_shapes_case_insensitively() {
    assert_eq!(
        classify_installer("Polaris-Setup-1.0.exe"),
        Some(InstallerKind::WinExe)
    );
    assert_eq!(
        classify_installer("Polaris-1.0.dmg"),
        Some(InstallerKind::Dmg)
    );
    assert_eq!(
        classify_installer("Polaris-1.0.AppImage"),
        Some(InstallerKind::AppImage)
    );
    assert_eq!(
        classify_installer("polaris-1.0.appimage"),
        Some(InstallerKind::AppImage)
    );
    assert_eq!(
        classify_installer("polaris_1.0_amd64.deb"),
        Some(InstallerKind::Deb)
    );
    // 认不出的一律 None（**不猜**：猜错就是拿错脚本去改宿主应用本体）。
    assert_eq!(classify_installer("polaris-1.0.tar.gz"), None);
    assert_eq!(classify_installer("README"), None);
}

// ── 运行形态判定 ──

#[test]
fn detect_run_form_truth_table() {
    assert_eq!(
        detect_run_form("linux", Some(Path::new("/a.AppImage")), None),
        RunForm::Loose
    );
    assert_eq!(detect_run_form("linux", None, None), RunForm::Installed);
    assert_eq!(
        detect_run_form("windows", None, Some(Path::new("C:\\p.exe"))),
        RunForm::Loose
    );
    // 无便携标记（`portable.marker` 不在 exe 同级，或被用户删了）→ Installed（保守：推安装器）。
    assert_eq!(detect_run_form("windows", None, None), RunForm::Installed);
    assert_eq!(detect_run_form("macos", None, None), RunForm::Loose);
}

// ── .app 包路径推导 ──

#[test]
fn mac_app_bundle_from_exe_matches_only_real_bundle_layout() {
    assert_eq!(
        mac_app_bundle_from_exe(Path::new(
            "/Applications/Polaris.app/Contents/MacOS/polaris"
        )),
        Some(p("/Applications/Polaris.app"))
    );
    // 非 bundle 布局 → None（**不瞎猜**，回退 open DMG 手动拖拽）。
    assert_eq!(
        mac_app_bundle_from_exe(Path::new("/usr/local/bin/polaris")),
        None
    );
    assert_eq!(
        mac_app_bundle_from_exe(Path::new("/A/Polaris.app/Contents/MacOS/")),
        None
    );
    // 尾段含 `/`（多层）→ 不匹配。
    assert_eq!(
        mac_app_bundle_from_exe(Path::new("/A/Polaris.app/Contents/MacOS/sub/polaris")),
        None
    );
}

// ── 安装计划真值表（含跨形态错配逃逸用例）──

/// Windows 形态的样本路径一律用**反斜杠**：计划决策对路径只做字符串运算（[`parent_dir_of`] /
/// [`file_name_of`]），故同一批断言在 Linux、macOS、Windows 宿主上给出同一个答案。
const WIN_PORTABLE_EXE: &str = "D:\\Tools\\Polaris\\polaris.exe";
const WIN_PORTABLE_ZIP: &str =
    "C:\\Users\\u\\AppData\\Local\\Polaris\\updates\\Polaris_1.2.3_x64-win-Portable.zip";
const WIN_SETUP_EXE: &str =
    "C:\\Users\\u\\AppData\\Local\\Polaris\\updates\\polaris-1.2.3-win-setup.exe";

fn dir_text(plan: &InstallPlan) -> Option<String> {
    plan.portable_dir
        .as_ref()
        .map(|d| d.to_string_lossy().into_owned())
}

#[test]
fn path_string_helpers_do_not_depend_on_the_host_separator() {
    for (path, parent, name) in [
        (WIN_PORTABLE_EXE, Some("D:\\Tools\\Polaris"), "polaris.exe"),
        (
            "D:/Tools/Polaris/polaris.exe",
            Some("D:/Tools/Polaris"),
            "polaris.exe",
        ),
        // 盘符根：保留结尾分隔符（`D:` 单独拿去打开是「D 盘的当前目录」，不是根）。
        ("D:\\polaris.exe", Some("D:\\"), "polaris.exe"),
        (
            "\\\\nas\\share\\Polaris\\polaris.exe",
            Some("\\\\nas\\share\\Polaris"),
            "polaris.exe",
        ),
        ("/opt/polaris/polaris", Some("/opt/polaris"), "polaris"),
        ("/polaris", Some("/"), "polaris"),
        // 没有目录段：不猜。
        ("polaris.exe", None, "polaris.exe"),
        ("", None, ""),
    ] {
        assert_eq!(parent_dir_of(path), parent, "parent_dir_of({path:?})");
        assert_eq!(file_name_of(path), name, "file_name_of({path:?})");
    }
}

#[test]
fn portable_zip_is_recognised_by_the_packaging_name_only() {
    assert_eq!(
        classify_installer("Polaris_1.2.3_x64-win-Portable.zip"),
        Some(InstallerKind::PortableZip)
    );
    assert_eq!(
        classify_installer("Polaris_0.9.0-beta.1_x64-win-Portable.zip"),
        Some(InstallerKind::PortableZip)
    );
    // 与选包侧同口径（正式版本名 + 便携后缀，大小写敏感）：别的 zip 一律不认，不许被当成便携包去「覆盖解压」。
    for other in [
        "polaris-portable.zip",
        "polaris-portable-1.2.3.zip",
        "Polaris_1.2.3_x64-win-portable.zip",
        "Polaris_1.2.3_arm64-win-Portable.zip",
        "Polaris_invalid_x64-win-Portable.zip",
        "Polaris-Portable-1.2.3.zip",
        "polaris-portable-1.2.3.7z",
        "Polaris_1.2.3_x64-win-Portable.zip.sha256",
        "sing-box-1.12.0-windows-amd64.zip",
        "geosite.zip",
    ] {
        assert_eq!(classify_installer(other), None, "{other}");
    }
    // 名字里带 portable 的 exe 仍是 exe（不是便携压缩包）。
    assert_eq!(
        classify_installer("polaris-portable-1.2.3.exe"),
        Some(InstallerKind::WinExe)
    );
}

/// 选包侧真实发布资产必须走停核/退出准备后的手动覆盖计划，而不是 UnknownAsset 交系统。
#[test]
fn published_portable_asset_routes_from_selection_to_manual_handoff() {
    use polaris_updater::{find_suitable_update_asset, AssetArch, AssetPlatform, GithubAsset};
    for name in [
        "Polaris_1.2.3_x64-win-Portable.zip",
        "Polaris_1.2.3-beta.1_x64-win-Portable.zip",
    ] {
        let assets = [GithubAsset {
            name: name.into(),
            browser_download_url: String::new(),
            size: 0,
            digest: None,
        }];
        let selected =
            find_suitable_update_asset(&assets, AssetPlatform::Windows, AssetArch::X64, true)
                .expect("正式便携发布资产必须能被选中");
        assert_eq!(
            polaris_updater::github::portable_zip_version(&selected.name),
            Some(if name.contains("beta") {
                "1.2.3-beta.1"
            } else {
                "1.2.3"
            }),
            "交接记录的目标版本必须来自同一正式发布命名合同",
        );
        let exe = Path::new(WIN_PORTABLE_EXE);
        let archive = PathBuf::from(format!("C:\\Temp\\{}", selected.name));
        let plan = decide_install_plan("windows", RunForm::Loose, &archive, exe, None, Some(exe))
            .expect("选中的正式便携资产不得落入 UnknownAsset");
        assert_eq!(plan.platform, InstallPlatform::WindowsPortable);
        assert_eq!(plan.portable_dir, Some(p("D:\\Tools\\Polaris")));
        assert_eq!(
            install_advisory(&plan),
            Some(InstallAdvisory::PortableManualReplace)
        );
        assert!(build_install_script(&plan).is_none());
    }
}

#[test]
fn plan_windows_portable_zip_is_a_manual_replace_into_the_program_dir() {
    let exe = Path::new(WIN_PORTABLE_EXE);
    let plan = decide_install_plan(
        "windows",
        RunForm::Loose,
        Path::new(WIN_PORTABLE_ZIP),
        exe,
        None,
        Some(exe),
    )
    .expect("便携运行形态 + 便携压缩包必须有计划");
    assert_eq!(plan.platform, InstallPlatform::WindowsPortable);
    // 覆盖目标是**程序所在目录**（不是空串、不是整条 exe 路径）。
    assert_eq!(dir_text(&plan).as_deref(), Some("D:\\Tools\\Polaris"));
    assert_eq!(plan.installer_path, p(WIN_PORTABLE_ZIP));
    // 这条腿没有脚本，且必须先告知。
    assert!(build_install_script(&plan).is_none());
    assert_eq!(
        install_advisory(&plan),
        Some(InstallAdvisory::PortableManualReplace)
    );

    // 正斜杠写法同一结论。
    let exe = Path::new("D:/Tools/Polaris/polaris.exe");
    let plan = decide_install_plan(
        "windows",
        RunForm::Loose,
        Path::new("C:/Temp/Polaris_1.2.3_x64-win-Portable.zip"),
        exe,
        None,
        Some(exe),
    )
    .unwrap();
    assert_eq!(dir_text(&plan).as_deref(), Some("D:/Tools/Polaris"));
}

#[test]
fn plan_windows_setup_is_for_the_installed_form_only() {
    let plan = decide_install_plan(
        "windows",
        RunForm::Installed,
        Path::new(WIN_SETUP_EXE),
        Path::new("C:\\Users\\u\\AppData\\Local\\Polaris\\polaris.exe"),
        None,
        None,
    )
    .unwrap();
    assert_eq!(plan.platform, InstallPlatform::WindowsSetup);
    assert!(plan.portable_dir.is_none());
    assert!(build_install_script(&plan).is_some());
}

/// Windows 两种形态与两种资产的四格里，只有对角线两格有计划。
///
/// 另外两格都是错配：便携运行拿到安装器（跑它会在别处装出第二份程序），安装态拿到便携压缩包
/// （安装目录不该被手动覆盖）。便携形态下推不出程序目录时同样不猜。
#[test]
fn plan_windows_rejects_the_off_diagonal_form_and_asset_pairs() {
    let exe = Path::new(WIN_PORTABLE_EXE);
    for (form, asset, portable) in [
        (RunForm::Loose, WIN_SETUP_EXE, Some(exe)),
        (RunForm::Installed, WIN_PORTABLE_ZIP, None),
        // 形态与证据自相矛盾 / 没有目录段：推不出覆盖目标。
        (RunForm::Loose, WIN_PORTABLE_ZIP, None),
        (
            RunForm::Loose,
            WIN_PORTABLE_ZIP,
            Some(Path::new("polaris.exe")),
        ),
    ] {
        assert!(
            matches!(
                decide_install_plan("windows", form, Path::new(asset), exe, None, portable),
                Err(InstallReject::FormMismatch { .. })
            ),
            "{form:?} + {asset} + {portable:?} 必须判错配"
        );
    }
    // 别的系统拿到便携压缩包：没有任何腿能处理它。
    for os in ["linux", "macos", "android"] {
        assert!(
            matches!(
                decide_install_plan(
                    os,
                    RunForm::Loose,
                    Path::new("/tmp/Polaris_1.2.3_x64-win-Portable.zip"),
                    Path::new("/usr/bin/polaris"),
                    Some(Path::new("/home/u/Polaris.AppImage")),
                    None,
                ),
                Err(InstallReject::FormMismatch { .. })
            ),
            "{os} 上的便携压缩包必须判错配"
        );
    }
}

#[test]
fn plan_macos_carries_bundle_or_falls_back() {
    let plan = decide_install_plan(
        "macos",
        RunForm::Loose,
        Path::new("/tmp/Polaris-1.2-mac-arm64.dmg"),
        Path::new("/Applications/Polaris.app/Contents/MacOS/polaris"),
        None,
        None,
    )
    .unwrap();
    assert_eq!(plan.platform, InstallPlatform::Macos);
    assert_eq!(plan.app_bundle_path, Some(p("/Applications/Polaris.app")));

    // 定位不到 bundle → 计划仍成立，但 app_bundle_path=None（脚本走 open DMG 手动拖拽）。
    let plan = decide_install_plan(
        "macos",
        RunForm::Loose,
        Path::new("/tmp/x.dmg"),
        Path::new("/usr/local/bin/polaris"),
        None,
        None,
    )
    .unwrap();
    assert!(plan.app_bundle_path.is_none());
}

#[test]
fn plan_linux_appimage_requires_appimage_env() {
    let plan = decide_install_plan(
        "linux",
        RunForm::Loose,
        Path::new("/tmp/Polaris-1.2.AppImage"),
        Path::new("/tmp/.mount_x/polaris"),
        Some(Path::new("/home/u/Apps/Polaris.AppImage")),
        None,
    )
    .unwrap();
    assert_eq!(plan.platform, InstallPlatform::LinuxAppImage);
    assert_eq!(
        plan.appimage_target,
        Some(p("/home/u/Apps/Polaris.AppImage"))
    );

    // **逃逸用例**：loose 形态但 $APPIMAGE 缺失 → 无覆盖目标，必须拒绝（否则会覆盖到 exe_path，
    // 而 AppImage 运行时的 exe_path 在 /tmp/.mount_* 只读挂载里 —— 覆盖它毫无意义且必失败）。
    let r = decide_install_plan(
        "linux",
        RunForm::Loose,
        Path::new("/tmp/Polaris-1.2.AppImage"),
        Path::new("/tmp/.mount_x/polaris"),
        None,
        None,
    );
    assert!(matches!(r, Err(InstallReject::FormMismatch { .. })));
}

#[test]
fn plan_rejects_cross_form_mismatch_and_never_escalates_to_root() {
    // **最要紧的逃逸用例**（§8.1 点名）：AppImage 运行形态 + .deb 资产。
    // 若这里放行，就会在 AppImage 用户机器上 `pkexec apt-get install` —— 提权装出第二份。
    let r = decide_install_plan(
        "linux",
        RunForm::Loose,
        Path::new("/tmp/polaris_1.2_amd64.deb"),
        Path::new("/tmp/.mount_x/polaris"),
        Some(Path::new("/home/u/Polaris.AppImage")),
        None,
    );
    match r {
        Err(InstallReject::FormMismatch {
            ref installer,
            ref form,
            ..
        }) => {
            assert_eq!(installer, "polaris_1.2_amd64.deb");
            assert_eq!(*form, RunForm::Loose);
        }
        other => panic!("AppImage 形态拿到 .deb 必须拒绝，实得: {other:?}"),
    }

    // 反向：deb 安装态 + AppImage 资产 → 同样拒绝。
    assert!(matches!(
        decide_install_plan(
            "linux",
            RunForm::Installed,
            Path::new("/tmp/Polaris.AppImage"),
            Path::new("/usr/bin/polaris"),
            None,
            None,
        ),
        Err(InstallReject::FormMismatch { .. })
    ));

    // 跨 OS 错配：Linux 上拿到 .dmg / .exe → 拒绝。
    for name in ["/tmp/x.dmg", "/tmp/x.exe"] {
        assert!(
            matches!(
                decide_install_plan(
                    "linux",
                    RunForm::Installed,
                    Path::new(name),
                    Path::new("/usr/bin/polaris"),
                    None,
                    None
                ),
                Err(InstallReject::FormMismatch { .. })
            ),
            "{name} 在 Linux 上必须被拒"
        );
    }
    // macOS 上拿到 .deb → 拒绝。
    assert!(matches!(
        decide_install_plan(
            "macos",
            RunForm::Loose,
            Path::new("/tmp/x.deb"),
            Path::new("/A/P.app/Contents/MacOS/p"),
            None,
            None
        ),
        Err(InstallReject::FormMismatch { .. })
    ));
}

#[test]
fn plan_rejects_unknown_asset() {
    assert!(matches!(
        decide_install_plan(
            "linux",
            RunForm::Installed,
            Path::new("/tmp/x.tar.gz"),
            Path::new("/usr/bin/p"),
            None,
            None
        ),
        Err(InstallReject::UnknownAsset { .. })
    ));
}

// ── 安装前告知（ad-hoc 签名 / 提权）──

fn plan_of(platform: InstallPlatform) -> InstallPlan {
    InstallPlan {
        platform,
        installer_path: p("/tmp/x"),
        exe_path: p("/usr/bin/polaris"),
        portable_dir: None,
        app_bundle_path: Some(p("/Applications/Polaris.app")),
        appimage_target: Some(p("/home/u/P.AppImage")),
    }
}

#[test]
fn advisory_is_required_wherever_os_will_block_or_prompt() {
    // 用户拍板走 ad-hoc 签名 ⇒ mac/win 都会被 OS 拦一道，**必须**提前告知可执行的下一步。
    assert_eq!(
        install_advisory(&plan_of(InstallPlatform::Macos)),
        Some(InstallAdvisory::MacosGatekeeper)
    );
    assert_eq!(
        install_advisory(&plan_of(InstallPlatform::WindowsSetup)),
        Some(InstallAdvisory::WindowsSmartScreen)
    );
    // 便携：没有安装器可被 SmartScreen 拦；要告知的是「应用会先停核并退出，退出后再覆盖」。
    assert_eq!(
        install_advisory(&plan_of(InstallPlatform::WindowsPortable)),
        Some(InstallAdvisory::PortableManualReplace)
    );
    // deb：polkit 提权框（= 上游 confirmDebElevation），必须在停代理**之前**确认。
    assert_eq!(
        install_advisory(&plan_of(InstallPlatform::LinuxDeb)),
        Some(InstallAdvisory::DebElevation)
    );
    // AppImage：无签名校验、无提权 → 唯一无需告知的路径。
    assert_eq!(
        install_advisory(&plan_of(InstallPlatform::LinuxAppImage)),
        None
    );
}

#[test]
fn advisory_keys_are_distinct_and_stable() {
    // key 是前端 i18n 的契约面；四者必须互不相同（撞 key = 弹错说明）。
    let keys = [
        InstallAdvisory::DebElevation.key(),
        InstallAdvisory::WindowsSmartScreen.key(),
        InstallAdvisory::MacosGatekeeper.key(),
        InstallAdvisory::PortableManualReplace.key(),
    ];
    let mut sorted = keys.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 4, "advisory key 必须互不相同");
    // 前端按这个字面量分流到「手动覆盖」那张卡，改名即两端失配。
    assert_eq!(
        InstallAdvisory::PortableManualReplace.key(),
        "portableManualReplace"
    );
}

// ── 脚本生成 ──

#[test]
fn windows_vbs_is_utf16le_with_bom() {
    // **变异防线**：若 utf16le_with_bom 退化成 `s.into_bytes()`（UTF-8），中文用户名路径会被
    // wscript 按系统代码页解释 → 找不到文件 → 更新静默失败。
    let plan = InstallPlan {
        installer_path: p("C:\\用户\\更新\\polaris-1.2-win-setup.exe"),
        ..plan_of(InstallPlatform::WindowsSetup)
    };
    let spec = script_of(&plan);
    assert_eq!(
        &spec.bytes[..2],
        &[0xFF, 0xFE],
        "VBS 必须以 UTF-16LE BOM 开头"
    );
    assert_eq!(
        spec.program, "wscript.exe",
        "必须用 wscript（无窗口），非 cscript"
    );
    // UTF-16LE：ASCII 字符后必跟 0x00。
    assert_eq!(spec.bytes[2], b'S');
    assert_eq!(spec.bytes[3], 0x00);
    // 解回文本验证内容。
    let units: Vec<u16> = spec.bytes[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let text = String::from_utf16(&units).unwrap();
    assert!(
        text.contains("C:\\\\用户\\\\更新\\\\polaris-1.2-win-setup.exe"),
        "路径反斜杠须双写"
    );
    assert!(text.contains("\r\n"), "VBS 行分隔符须是 CRLF");
}

#[test]
fn windows_setup_script_passes_update_flag() {
    let spec = script_of(&plan_of(InstallPlatform::WindowsSetup));
    let units: Vec<u16> = spec.bytes[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let text = String::from_utf16(&units).unwrap();
    // `/UPDATE` 不可省：Tauri NSIS 模板的 `$UpdateMode` 靠它，缺了会让应用内更新静默降级成
    // 全新安装模式（弹卸载/重装选择页 + 重跑 WebView2 安装 + 卸载器侧清理闸失效）。
    assert!(text.contains("/UPDATE"), "NSIS setup 必须带 /UPDATE");
    // 同时钉死**不得**再出现 electron-builder 的 `--updated`：这条门此前锁的正是那个错 flag，
    // 于是「传了个 Tauri 根本不认的参数」看起来像验过了。反向断言让退回旧写法直接转红。
    assert!(
        !text.contains("--updated"),
        "`--updated` 是 electron-builder 的约定，Tauri 不认（会静默降级成全新安装）"
    );
}

#[test]
fn mac_script_must_clear_quarantine_and_resign_adhoc() {
    // **变异验证（用户点名）**：删掉 quarantine 清除步骤 → 本测试必须转红。
    // ad-hoc 签名下不清 quarantine = 用户点了更新、装完打不开（最差体验）。
    let spec = script_of(&plan_of(InstallPlatform::Macos));
    let s = String::from_utf8(spec.bytes).unwrap();
    assert!(
        s.contains("xattr -dr com.apple.quarantine \"$DEST\""),
        "ad-hoc 签名下必须清 quarantine，否则 Gatekeeper 拦「身份不明的开发者」"
    );
    assert!(
        s.contains("codesign --force --deep --sign - \"$DEST\""),
        "签名校验不过时必须 ad-hoc 重签（`-s -` 无需任何证书）"
    );
    // 清 quarantine 必须在 `open` **之前**（顺序颠倒 = 先被拦一次）。
    let q = s.find("xattr -dr com.apple.quarantine").unwrap();
    let o = s.find("open \"$DEST\"").unwrap();
    assert!(q < o, "清 quarantine 必须早于启动新版");
    // 兜底指引：若仍起不来，把 .app 亮给用户（可右键→打开放行）。
    assert!(
        s.contains("open -R \"$DEST\""),
        "启动失败须给出用户可执行的下一步"
    );
    // 原子性：mv-swap，绝不先 rm 目标。
    assert!(
        !s.contains("rm -rf \"$DEST\""),
        "绝不先毁目标再建（brick 风险）"
    );
    assert!(s.contains("hdiutil attach"), "须挂载 DMG");
    assert!(s.contains("hdiutil detach"), "须卸载 DMG（否则残留挂载点）");
}

/// 成功判据必须落在 `$STAGE`（被移走 = 真替换过），不得落在 `$BAK` 不在场上。
///
/// `[ ! -d "$BAK" ]` 是**三种状态共有**的：真成功、第二步 mv 失败后已回滚、第一步 mv 就失败
/// （含提权密码框被取消，$BAK 从未产生）。后两种落进成功分支 = 删掉新版与 DMG、把旧版拉起来，
/// 而调用方已经向前端回了 success。
#[test]
fn mac_script_success_branch_keys_on_stage_not_bak() {
    let spec = script_of(&plan_of(InstallPlatform::Macos));
    let s = String::from_utf8(spec.bytes).unwrap();

    assert!(
        s.contains(r#"if [ -d "$DEST" ] && [ ! -d "$STAGE" ]; then"#),
        "成功判据没落在 $STAGE 上：\n{s}"
    );
    assert!(
        !s.contains(r#"[ ! -d "$BAK" ]"#),
        "还在用 $BAK 不在场当成功判据 —— 失败已回滚与提权被取消都满足它"
    );

    // 破坏性收尾必须落在**成功分支体内**。
    // 不做全局位置比较：脚本更早还有一条合法的早退腿（找不到 `.app` 时 `rm -f "$DMG"; exit 0`），
    // 全局 `find` 会撞上它 —— 实测本门第一版就是这么误红的。
    let cond = s.find(r#"[ ! -d "$STAGE" ]"#).expect("上一条已断言存在");
    let branch_end = s[cond..].find("\nelse\n").map_or(s.len(), |i| cond + i);
    let branch = &s[cond..branch_end];
    for destructive in [r#"rm -f "$DMG""#, r#"open "$DEST""#, r#"rm -rf "$STAGE""#] {
        assert!(
            branch.contains(destructive),
            "`{destructive}` 不在成功分支体内（跑到判据之外 = 失败时也会执行）：\n{branch}"
        );
    }

    // 失败侧的退路必须还在：重新打开 DMG 让用户手动拖拽。
    // **判据必须落在 else 分支体内**：脚本更早还有两处 `open "$DMG"`（挂载失败早退、
    // 定位不到 bundle 的回退），裸 `contains` 会被它们喂饱 —— 实测本门第一版就是这样，
    // 把 else 里那行删掉照样绿。
    let else_at = s[cond..]
        .find("\nelse\n")
        .map(|i| cond + i)
        .expect("成功分支没有 else —— 失败时无退路");
    let else_end = s[else_at..].find("\nfi").map_or(s.len(), |i| else_at + i);
    let else_body = &s[else_at..else_end];
    assert!(
        else_body.contains(r#"open "$DMG""#),
        "else 分支的手动拖拽退路没了 —— 失败时用户无路可走：\n{else_body}"
    );

    // 自检：$STAGE 确实是被 mv 走的那个（否则上面的判据在语义上不成立）。
    assert!(
        s.contains(r#"mv "$STAGE" "$DEST""#),
        "$STAGE 不再是被移走的对象，本门的整个前提失效"
    );
}

#[test]
fn mac_script_falls_back_to_open_dmg_without_bundle() {
    let plan = InstallPlan {
        app_bundle_path: None,
        ..plan_of(InstallPlatform::Macos)
    };
    let s = String::from_utf8(script_of(&plan).bytes).unwrap();
    assert!(s.contains("open '/tmp/x'"));
    // 定位不到 bundle 时**绝不**瞎猜路径去 mv。
    assert!(!s.contains("mv "), "定位不到 .app 时不得做任何替换");
}

#[test]
fn linux_scripts_match_form() {
    let s = String::from_utf8(script_of(&plan_of(InstallPlatform::LinuxAppImage)).bytes).unwrap();
    assert!(s.contains("chmod +x \"$DEST\""), "覆盖后必须补执行位");
    assert!(!s.contains("pkexec"), "AppImage 路径绝不提权");

    let s = String::from_utf8(script_of(&plan_of(InstallPlatform::LinuxDeb)).bytes).unwrap();
    assert!(
        s.contains("pkexec apt-get install"),
        "deb 须走 apt 原位升级"
    );
    assert!(
        s.contains("xdg-open"),
        "提权被取消须回退到打开下载目录，不静默"
    );
}

#[test]
fn sh_quote_neutralizes_injection() {
    // 路径里的单引号是命令注入面（脚本以 root 跑 deb 分支）。
    assert_eq!(sh_quote("/tmp/a b"), "'/tmp/a b'");
    assert_eq!(
        sh_quote("/tmp/x';rm -rf /;'"),
        r"'/tmp/x'\'';rm -rf /;'\'''"
    );
    let plan = InstallPlan {
        installer_path: p("/tmp/x';touch /tmp/pwned;'"),
        ..plan_of(InstallPlatform::LinuxDeb)
    };
    let s = String::from_utf8(script_of(&plan).bytes).unwrap();
    assert!(
        !s.contains("DEB='/tmp/x';touch"),
        "单引号必须被转义，不得逃出字面量"
    );
}

#[test]
fn utf16le_with_bom_roundtrips_non_ascii() {
    let bytes = utf16le_with_bom("中文A");
    assert_eq!(&bytes[..2], &[0xFF, 0xFE]);
    let units: Vec<u16> = bytes[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    assert_eq!(String::from_utf16(&units).unwrap(), "中文A");
}

#[test]
fn script_generation_is_deterministic() {
    // 快照断言的前提：同一 plan 恒得同一字节（脚本里不得掺时间戳/随机数；$$ 是 shell 运行期取的）。
    let plan = plan_of(InstallPlatform::Macos);
    let a = script_of(&plan);
    let b = script_of(&plan);
    assert_eq!(a, b);
}

// ── Android：交系统安装器（W-21）─────────────────────────────────────────────

/// `.apk` 必须被认成一种资产形态。
///
/// 不认识 ⇒ [`decide_install_plan`] 落 [`InstallReject::UnknownAsset`] ⇒ command 层回退
/// `shell.open` 一个 APK 文件 —— 那在 Android 上恰好**也会**弹出系统安装器，于是这条腿
/// 「看起来能用」，而权限判据、私有目录判据、不静默失败判据一条都没跑过。
/// 这就是为什么本条断言不是走过场：错的那一侧不会当场失败，它会安静地降级。
#[test]
fn apk_is_a_recognised_installer_shape() {
    assert_eq!(
        classify_installer("polaris-1.2.0.apk"),
        Some(InstallerKind::Apk)
    );
    assert_eq!(
        classify_installer("Polaris-1.2.0.APK"),
        Some(InstallerKind::Apk)
    );
    // 反向：别的后缀不许被认成 apk。
    assert_eq!(classify_installer("polaris.apk.sig"), None);
}

/// Android + `.apk` ⇒ [`InstallPlatform::Android`]，且 **`run_form` 不改变结果**。
///
/// 后半条是正面断言而不是废话：Android 上没有「便携 vs 安装态」这个轴，而
/// [`detect_run_form`] 对未具名平台恒返 `Installed`。哪天有人给 Android 加一条 run_form 判据，
/// 这条会红并要求他先回答「Android 上那个轴是什么意思」。
#[test]
fn android_apk_plans_the_system_installer_regardless_of_run_form() {
    for form in [RunForm::Installed, RunForm::Loose] {
        let plan = decide_install_plan(
            "android",
            form,
            Path::new("/data/user/0/com.polaris2.app/cache/updates/polaris-1.2.0.apk"),
            Path::new("/system/bin/app_process64"),
            None,
            None,
        )
        .expect("android + .apk 必须有计划");
        assert_eq!(plan.platform, InstallPlatform::Android, "form={form:?}");
        // 桌面各腿的字段一个都不该被填 —— 填了说明有人在按桌面的形状想这条腿。
        assert_eq!(plan.portable_dir, None);
        assert_eq!(plan.app_bundle_path, None);
        assert_eq!(plan.appimage_target, None);
    }
}

/// 错配两个方向都必须被拒（**两条一起断，缺一条就只守住一半**）。
///
/// - 桌面拿到 `.apk`：没有任何桌面腿能装它，回退交系统是唯一正确处置；
/// - Android 拿到 `.deb` / `.exe`：同理。第一版只断了前者，而后者才是「资产命名规则改了、
///   Android 腿开始收到别的平台的包」那条真实路径。
#[test]
fn apk_and_desktop_assets_never_cross_over() {
    for os in ["linux", "windows", "macos"] {
        assert!(
            matches!(
                decide_install_plan(
                    os,
                    RunForm::Installed,
                    Path::new("/tmp/polaris.apk"),
                    Path::new("/usr/bin/p"),
                    None,
                    None
                ),
                Err(InstallReject::FormMismatch { .. })
            ),
            "{os} 上的 .apk 必须判错配"
        );
    }
    for asset in ["/tmp/polaris.deb", "/tmp/polaris.exe", "/tmp/polaris.dmg"] {
        assert!(
            matches!(
                decide_install_plan(
                    "android",
                    RunForm::Installed,
                    Path::new(asset),
                    Path::new("/system/bin/app_process64"),
                    None,
                    None
                ),
                Err(InstallReject::FormMismatch { .. })
            ),
            "android 上的 {asset} 必须判错配"
        );
    }
}

/// 🔴 **变异锁：Android 不许有脚本腿。**
///
/// 变异：把 `build_install_script` 的 Android 臂改成造一个空 `ScriptSpec` ⇒ 本条红。
/// 那个变异在真机上的后果不是崩，是 command 层 `spawn` 一个空脚本、拿到 rc=0、
/// 然后如实汇报「安装脚本已起，应用即将退出」—— 应用退了，什么都没装。
#[test]
fn android_has_no_install_script() {
    assert!(
        build_install_script(&plan_of(InstallPlatform::Android)).is_none(),
        "Android 走的是系统安装器，不许凭空造一个脚本出来"
    );
    // Windows 便携同样没有脚本腿（用户手动覆盖）；凭空造一个出来的后果与上面相同。
    assert!(
        build_install_script(&plan_of(InstallPlatform::WindowsPortable)).is_none(),
        "Windows 便携走的是手动覆盖，不许凭空造一个脚本出来"
    );
    // 正面对照：另外四种必须**都**有脚本 —— 否则「返 None」这条断言可能是因为函数整个塌了。
    for platform in [
        InstallPlatform::WindowsSetup,
        InstallPlatform::Macos,
        InstallPlatform::LinuxAppImage,
        InstallPlatform::LinuxDeb,
    ] {
        assert!(
            build_install_script(&plan_of(platform)).is_some(),
            "{platform:?} 必须仍有脚本腿"
        );
    }
}

/// Android 不出 advisory —— 理由是**判据分辨率**，不是漏写。
///
/// advisory 是一次预告（动手前猜 OS 会不会拦）。Win/mac 上那个猜恒真，Android 上不是：
/// 「安装未知应用」按应用授权且**可能早就给过了**。恒定的预告会对一半用户说一句不成立的话。
/// 真正的判据是 Kotlin 侧当场读的 `canRequestPackageInstalls()`，结果经
/// `ApkHandoff.reason` 回来。
///
/// 🔴 这条断言**不能单独存在**：它只说「这里没有预告」，说不了「那边真的有判据」。
/// 与它成对的那半在 `src-tauri/tests/android_native_surface_wiring.rs`
/// （安装器不静默失败门：Kotlin 侧必须在 startActivity 之前问过权限、且没授权时给出可读回报）。
/// 只有本条时，把 Kotlin 那段权限判据整段删掉，这里照样绿。
#[test]
fn android_defers_the_advisory_to_a_readable_system_fact() {
    assert_eq!(install_advisory(&plan_of(InstallPlatform::Android)), None);
}

#[test]
fn windows_install_requires_successful_synchronous_native_process_wait() {
    let wait = windows_process_wait(12345);
    assert_eq!(wait.matches("GetProcessById(").count(), 1);
    assert!(wait.contains("GetProcessById(12345)"));
    let query = wait.find("GetProcessById(12345)").unwrap();
    let handle = wait.find("$null = $process.Handle").unwrap();
    let native_wait = wait.find("$process.WaitForExit(120000)").unwrap();
    assert!(query < handle && handle < native_wait);
    let query_error = &wait[query..handle];
    assert!(query_error.contains("$queryError.GetType() -eq [System.ArgumentException]"));
    assert!(query_error.contains("exit 1"));
    let wait_error = &wait[handle..];
    assert!(
        wait_error.contains("exit 2"),
        "timeout must refuse installing"
    );
    assert!(wait_error.contains("catch { exit 1 }"));
    assert!(!wait_error.contains("ArgumentException"));
    assert!(wait_error.contains("$process.Dispose()"));
    for forbidden in ["Stop-Process", ".Kill(", "Get-Process", "Start-Sleep"] {
        assert!(!wait.contains(forbidden));
    }

    let bytes: Vec<u8> = windows_process_wait(std::process::id())
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let encoded = crate::runtime::mesh::base64_encode(&bytes);
    let platform = InstallPlatform::WindowsSetup;
    let text = build_windows_setup_vbs(&plan_of(platform));
    assert!(text.contains(&format!(
        "-NoProfile -NonInteractive -EncodedCommand {encoded}"
    )));
    assert!(text.contains("%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"));
    let synchronous = text.find("WshShell.Run(waitCommand, 0, True)").unwrap();
    let refusal = text
        .find("If waitError <> 0 Or waitResult <> 0 Then WScript.Quit 1")
        .unwrap();
    assert!(synchronous < refusal);
    assert!(text[synchronous..refusal].contains("waitError = Err.Number"));
    for effect in [
        "fso.CopyFile",
        "fso.MoveFile",
        "fso.DeleteFile",
        "WshShell.Run \"",
    ] {
        if let Some(index) = text.find(effect) {
            assert!(
                refusal < index,
                "{platform:?}: {effect} precedes exit proof"
            );
        }
    }
    assert!(!text.contains("WScript.Sleep"));
}

#[test]
fn all_unix_install_paths_wait_before_any_install_or_open() {
    for plan in [
        plan_of(InstallPlatform::LinuxAppImage),
        plan_of(InstallPlatform::LinuxDeb),
        plan_of(InstallPlatform::Macos),
        InstallPlan {
            app_bundle_path: None,
            ..plan_of(InstallPlatform::Macos)
        },
    ] {
        let text = String::from_utf8(script_of(&plan).bytes).unwrap();
        assert!(text.starts_with(&format!("#!/bin/bash\n{UNIX_EXIT_WAIT}\n")));
        assert_eq!(text.matches(UNIX_EXIT_WAIT).count(), 1);
        assert!(!text.contains("sleep 2"));
    }
}

#[cfg(unix)]
mod lifetime_wait {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "polaris-installer-wait-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn marker(&self) -> PathBuf {
            self.0.join("installed")
        }
        fn command(&self) -> Command {
            let mut command = Command::new("/bin/bash");
            // This only writes a test marker: no installer, application, or real kernel runs.
            command.arg("-c").arg(format!(
                "{UNIX_EXIT_WAIT}\nprintf installed > {}",
                sh_quote(&self.marker().to_string_lossy())
            ));
            command.stdout(Stdio::null()).stderr(Stdio::null());
            command
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn wait_for_pipe_eof(
        reader: &std::os::fd::OwnedFd,
        budget: std::time::Duration,
    ) -> Result<(), String> {
        use nix::errno::Errno;
        use nix::fcntl::{fcntl, FcntlArg, OFlag};
        let deadline = std::time::Instant::now() + budget;
        let flags = fcntl(reader, FcntlArg::F_GETFL).map_err(|error| error.to_string())?;
        fcntl(
            reader,
            FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
        )
        .map_err(|error| error.to_string())?;
        loop {
            if std::time::Instant::now() >= deadline {
                return Err("pipe EOF deadline elapsed".into());
            }
            match nix::unistd::read(reader, &mut [0u8; 1]) {
                Ok(0) => return Ok(()),
                Ok(_) => return Err("pipe data is not EOF".into()),
                Err(Errno::EAGAIN | Errno::EINTR) => {}
                Err(error) => return Err(format!("pipe read failed: {error}")),
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err("pipe EOF deadline elapsed".into());
            }
            // Retry only transient syscall results; elapsed time never becomes an EOF fact.
            std::thread::sleep(remaining.min(std::time::Duration::from_millis(1)));
        }
    }

    #[test]
    fn lifetime_pipe_blocks_beyond_two_seconds_then_only_eof_admits_marker() {
        let fixture = Fixture::new();
        let (mut child, writer) = spawn_with_lifetime_pipe(&mut fixture.command()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2100));
        let prematurely_installed = fixture.marker().exists();
        let prematurely_exited = child.try_wait().unwrap().is_some();
        // Reading bytes is not EOF: count=1 or a single read must never admit installation.
        let write_result = nix::unistd::write(&writer, b"lifetime is still held");
        std::thread::sleep(std::time::Duration::from_millis(100));
        let installed_after_bytes = fixture.marker().exists();
        let exited_after_bytes = child.try_wait().unwrap().is_some();
        drop(writer);
        let status = child.wait().unwrap();
        assert!(!prematurely_installed && !prematurely_exited);
        assert!(write_result.is_ok());
        assert!(!installed_after_bytes && !exited_after_bytes);
        assert!(status.success());
        assert_eq!(std::fs::read(fixture.marker()).unwrap(), b"installed");
    }

    #[test]
    fn non_pipe_and_pipe_read_error_refuse_install_effects() {
        let fixture = Fixture::new();
        let status = fixture.command().stdin(Stdio::null()).status().unwrap();
        assert!(!status.success());
        assert!(!fixture.marker().exists());

        let (reader, writer) = lifetime_pipe().unwrap();
        // fd 0 is a pipe, but its write-only descriptor cannot produce a successful read EOF.
        let status = fixture
            .command()
            .stdin(Stdio::from(writer))
            .status()
            .unwrap();
        drop(reader);
        assert!(!status.success());
        assert!(!fixture.marker().exists());
    }

    #[test]
    fn both_pipe_ends_are_cloexec_and_spawn_failure_keeps_raii_close() {
        use nix::fcntl::{fcntl, FcntlArg, FdFlag, OFlag};
        let (reader, writer) = lifetime_pipe().unwrap();
        for fd in [&reader, &writer] {
            let flags = fcntl(fd, FcntlArg::F_GETFD).unwrap();
            assert!(FdFlag::from_bits_truncate(flags).contains(FdFlag::FD_CLOEXEC));
        }
        let fixture = Fixture::new();
        let mut missing = Command::new(fixture.0.join("missing-program"));
        let observer = reader.try_clone().unwrap();
        // Nonblocking read distinguishes EOF from a leaked live writer without hanging the test.
        fcntl(&observer, FcntlArg::F_SETFL(OFlag::O_NONBLOCK)).unwrap();
        assert!(spawn_with_pipe(&mut missing, (reader, writer)).is_err());
        // CLOEXEC closes at exec, so a concurrent fork can briefly retain a writer copy.
        // Require actual EOF within a deadline rather than classifying one EAGAIN as a leak.
        wait_for_pipe_eof(&observer, std::time::Duration::from_secs(5)).unwrap();
        // The command still owns the read end after failed spawn; the local writer was dropped.
        // Taking this read descriptor out of Command is unnecessary: its Drop closes it normally.
        assert!(!fixture.marker().exists());
    }

    #[test]
    fn pipe_eof_observer_refuses_live_writer_data_and_read_errors() {
        let (observer, writer) = lifetime_pipe().unwrap();
        assert_eq!(
            wait_for_pipe_eof(&observer, std::time::Duration::from_millis(25)).unwrap_err(),
            "pipe EOF deadline elapsed"
        );
        drop(writer);
        wait_for_pipe_eof(&observer, std::time::Duration::from_secs(5)).unwrap();

        let (observer, writer) = lifetime_pipe().unwrap();
        nix::unistd::write(&writer, b"x").unwrap();
        assert_eq!(
            wait_for_pipe_eof(&observer, std::time::Duration::from_secs(5)).unwrap_err(),
            "pipe data is not EOF"
        );
        // A write-only pipe descriptor produces EBADF, which must never be retried into success.
        assert!(
            wait_for_pipe_eof(&writer, std::time::Duration::from_secs(5))
                .unwrap_err()
                .starts_with("pipe read failed:")
        );
        drop(writer);
        wait_for_pipe_eof(&observer, std::time::Duration::from_secs(5)).unwrap();
    }

    #[test]
    fn production_writer_is_transferred_after_spawn_without_fallible_publication() {
        // Check the narrow production lifetime transfer; other tests exercise real EOF/error IO.
        let source = crate::test_support::crate_source("runtime/update_install.rs");
        let production = source.split("pub fn spawn_detached_script").nth(1).unwrap();
        let spawn = production
            .find("spawn_with_lifetime_pipe(&mut cmd)")
            .unwrap();
        let transfer = production.find("lifetime_writer.into_raw_fd()").unwrap();
        assert!(spawn < transfer);
        let success = production[spawn..transfer].split_once('?').unwrap().1;
        assert!(!success.contains(".await"));
        assert!(
            !success.contains('?'),
            "no fallible publication may close the successful writer"
        );
        assert!(!success.contains("drop(lifetime_writer)"));
    }
}

/// 被拒之后交不交系统：只有「Windows 便携形态拿到安装程序」不交 —— 系统对 `.exe` 的打开就是
/// 运行它，会在别处装出第二份程序。
///
/// **变异探针**：让 `hands_to_system` 恒真 ⇒ 首条转红；恒假 ⇒ 后四条转红。
#[test]
fn a_portable_install_never_hands_an_installer_to_the_system() {
    let mismatch = |installer: &str, os: &str, form| InstallReject::FormMismatch {
        installer: installer.to_owned(),
        os: os.to_owned(),
        form,
    };
    assert!(!mismatch("Polaris_1.2.3_x64-setup.exe", "windows", RunForm::Loose).hands_to_system());
    // 这一格就是 `decide_install_plan` 对便携形态 + 安装程序给出的那个拒绝。
    let rejected = decide_install_plan(
        "windows",
        RunForm::Loose,
        Path::new(r"C:\Cache\updates\Polaris_1.2.3_x64-setup.exe"),
        Path::new(r"D:\Polaris\polaris.exe"),
        None,
        Some(Path::new(r"D:\Polaris\polaris.exe")),
    )
    .expect_err("便携形态拿到安装程序是错配");
    assert!(!rejected.hands_to_system(), "{rejected:?}");

    for handed in [
        // 安装态拿到便携压缩包：打开只是让系统的压缩包查看器接手。
        mismatch(
            "Polaris_1.2.3_x64-win-Portable.zip",
            "windows",
            RunForm::Installed,
        ),
        // AppImage 运行拿到 `.deb`：交给系统的包管理器界面，由用户决定（不自动提权安装）。
        mismatch("polaris_1.2.3_amd64.deb", "linux", RunForm::Loose),
        mismatch("Polaris-1.2.3.AppImage", "linux", RunForm::Installed),
        InstallReject::UnknownAsset {
            file_name: "notes.txt".to_owned(),
        },
    ] {
        assert!(handed.hands_to_system(), "{handed:?}");
    }
}
