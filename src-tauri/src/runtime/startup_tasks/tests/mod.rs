use super::*;
use serde_json::json;

// ── decide_auto_connect ───────────────────────────────────────────────────
#[test]
fn auto_connect_enabled_with_selected_server() {
    let cfg = json!({ "autoConnect": true, "selectedServerId": "srv-1" });
    assert_eq!(
        decide_auto_connect(&cfg, false),
        AutoConnectDecision::Connect {
            server_id: "srv-1".to_string()
        }
    );
}

#[test]
fn auto_connect_enabled_without_server_is_warn_branch() {
    // 开关开但没选 → NoServerSelected（对齐 上游 warn 日志分支，不静默当 Disabled）。
    assert_eq!(
        decide_auto_connect(&json!({ "autoConnect": true }), false),
        AutoConnectDecision::NoServerSelected
    );
    assert_eq!(
        decide_auto_connect(
            &json!({ "autoConnect": true, "selectedServerId": "" }),
            false
        ),
        AutoConnectDecision::NoServerSelected,
        "空串视同未选"
    );
    assert_eq!(
        decide_auto_connect(
            &json!({ "autoConnect": true, "selectedServerId": 42 }),
            false
        ),
        AutoConnectDecision::NoServerSelected,
        "非字符串视同未选"
    );
}

#[test]
fn auto_connect_disabled_by_default_and_on_bad_types() {
    assert_eq!(
        decide_auto_connect(&json!({}), false),
        AutoConnectDecision::Disabled,
        "缺字段 → 关"
    );
    assert_eq!(
        decide_auto_connect(
            &json!({ "autoConnect": false, "selectedServerId": "s" }),
            false
        ),
        AutoConnectDecision::Disabled
    );
    assert_eq!(
        decide_auto_connect(
            &json!({ "autoConnect": "true", "selectedServerId": "s" }),
            false
        ),
        AutoConnectDecision::Disabled,
        "非 bool → 关（不做字符串 truthy 推断）"
    );
}

/// 系统拉起的核在跑（Android always-on / 开机自动连接）⇒ 不论 `autoConnect`，都收编。
/// 反向对照：同一份配置、`system_core_running=false` 时 `autoConnect:false` 仍是 `Disabled` ——
/// 证明是这一位、而不是配置本身把决策翻过来的。
#[test]
fn system_started_core_is_adopted_regardless_of_auto_connect() {
    let off = json!({ "autoConnect": false, "selectedServerId": "srv-1" });
    assert_eq!(
        decide_auto_connect(&off, true),
        AutoConnectDecision::AdoptSystemCore {
            server_id: "srv-1".to_string()
        }
    );
    assert_eq!(
        decide_auto_connect(&off, false),
        AutoConnectDecision::Disabled
    );
    assert_eq!(
        decide_auto_connect(&json!({ "selectedServerId": "srv-1" }), true),
        AutoConnectDecision::AdoptSystemCore {
            server_id: "srv-1".to_string()
        },
        "缺 autoConnect 字段同样收编"
    );
    assert_eq!(
        decide_auto_connect(
            &json!({ "autoConnect": false, "selectedServerId": "" }),
            true
        ),
        AutoConnectDecision::NoServerSelected,
        "没有选中节点就无从按当前配置起核 —— 落 warn 分支，不静默"
    );
}

// ── should_auto_check_update ──────────────────────────────────────────────
#[test]
fn auto_check_update_defaults_to_true() {
    assert!(should_auto_check_update(&json!({})), "缺字段 → 开");
    assert!(should_auto_check_update(
        &json!({ "autoCheckUpdate": true })
    ));
    assert!(
        should_auto_check_update(&json!({ "autoCheckUpdate": "no" })),
        "非 bool → 开（!== false 语义）"
    );
    assert!(!should_auto_check_update(
        &json!({ "autoCheckUpdate": false })
    ));
}

#[test]
fn auto_download_defaults_to_off_and_needs_explicit_true() {
    assert!(
        !should_auto_download_update(&json!({})),
        "缺字段 → 关（几十 MB 流量不能替用户做主）"
    );
    assert!(!should_auto_download_update(
        &json!({ "autoDownloadUpdate": false })
    ));
    assert!(
        !should_auto_download_update(&json!({ "autoDownloadUpdate": "true" })),
        "非 bool → 关（不做字符串 truthy 推断）"
    );
    assert!(should_auto_download_update(
        &json!({ "autoDownloadUpdate": true })
    ));
}

/// 🟡 **`autoDownloadUpdate` 与 `autoCheckUpdate` 方向相反，且前者不得越过后者。**
///
/// 「不得越过」是结构性的（下载腿挂在检查腿内部），此处钉住的是两个缺省方向不同这件事 ——
/// 把 `should_auto_download_update` 抄成 `!= Some(false)` 的形态会让它转红。
#[test]
fn auto_download_and_auto_check_defaults_point_opposite_ways() {
    let empty = json!({});
    assert!(should_auto_check_update(&empty), "检查缺省开（只读、免费）");
    assert!(
        !should_auto_download_update(&empty),
        "下载缺省关（几十 MB，可能在计费网络上）"
    );
}

// ── auto_download_applicable（复用安装侧同一判定）─────────────────────────
#[test]
fn auto_download_skips_assets_that_could_never_be_installed_here() {
    let exe = std::path::Path::new("/opt/polaris/polaris");
    // Linux 安装态（无 APPIMAGE）+ .deb → 装得上。
    assert!(auto_download_applicable("linux", "polaris_1.2.3_amd64.deb", exe, None, None).is_ok());
    // Linux 安装态 + AppImage 资产 → 形态错配，跳过（下了也只能交系统）。
    assert!(
        auto_download_applicable("linux", "Polaris-1.2.3.AppImage", exe, None, None).is_err(),
        "deb 安装态拿到 AppImage 属错配，不该白下"
    );
    // AppImage 运行态 + .deb → **安全闸**（绝不自动提权装 deb）→ 跳过。
    let appimage = std::path::Path::new("/home/u/Polaris.AppImage");
    assert!(auto_download_applicable(
        "linux",
        "polaris_1.2.3_amd64.deb",
        exe,
        Some(appimage),
        None
    )
    .is_err());
    // 不认识的资产后缀 → 跳过。
    assert!(auto_download_applicable("linux", "polaris-1.2.3.tar.gz", exe, None, None).is_err());
    // 空文件名 → 跳过（不猜）。
    assert!(auto_download_applicable("linux", "", exe, None, None).is_err());
    // macOS dmg / Windows exe → 装得上。
    assert!(auto_download_applicable(
        "macos",
        "Polaris-1.2.3.dmg",
        std::path::Path::new("/Applications/Polaris.app/Contents/MacOS/polaris"),
        None,
        None
    )
    .is_ok());
    assert!(auto_download_applicable(
        "windows",
        "Polaris-Setup-1.2.3.exe",
        std::path::Path::new("C:\\Program Files\\Polaris\\polaris.exe"),
        None,
        None
    )
    .is_ok());
    // Windows 便携版只认发布侧固定命名（手动覆盖那条腿）；带 portable 的任意 ZIP 不会被猜成资产。
    // 安装器装不到便携目录 ⇒ 不下。
    let portable = std::path::Path::new("D:\\Tools\\Polaris\\polaris.exe");
    assert!(auto_download_applicable(
        "windows",
        "Polaris_1.2.3_x64-win-Portable.zip",
        portable,
        None,
        Some(portable)
    )
    .is_ok());
    assert!(auto_download_applicable(
        "windows",
        "polaris-portable-1.2.3.zip",
        portable,
        None,
        Some(portable)
    )
    .is_err());
    assert!(auto_download_applicable(
        "windows",
        "Polaris-Setup-1.2.3.exe",
        portable,
        None,
        Some(portable)
    )
    .is_err());
}

// ── should_notify_helper_upgradeable ──────────────────────────────────────
fn helper_status(installed: bool, ready: bool, upgradeable: bool) -> HelperStatusSnapshot {
    HelperStatusSnapshot {
        supported: true,
        installed,
        ready,
        upgradeable,
        ..HelperStatusSnapshot::default()
    }
}

#[test]
fn helper_upgradeable_notified_only_when_installed_and_upgradeable() {
    assert!(should_notify_helper_upgradeable(&helper_status(
        true, true, true
    )));
    assert!(
        !should_notify_helper_upgradeable(&helper_status(true, true, false)),
        "已是最新 → 不发（白发会让前端白拉一次 status）"
    );
    assert!(
        !should_notify_helper_upgradeable(&helper_status(false, false, false)),
        "未安装 → 不发（该引导用户「安装」而非「升级」）"
    );
    assert!(
        !should_notify_helper_upgradeable(&HelperStatusSnapshot::default()),
        "缺省态（不支持/未装）一律不发"
    );
}

/// 🟡 **四条启动腿必须各占各的时刻**——本文件自己立的错峰约定，此前出口 IP 首探与自动连接
/// 双双 2s、正面违反。
///
/// 撞点的后果不止是启动瞬间的资源峰值：自动连接会起核，起核腿随即排一发 4s 后的重探，与同刻起跑
/// 的首探腿形成竞态（落地顺序另由 `commands::misc` 的世代闸兜底，但两条腿本就不该同刻发车）。
///
/// **变异锁**：把 `EXIT_IP_PROBE_DELAY_MS` 改回 `2_000`、或把 helper 探测排到 5s → 本条转红。
#[test]
fn startup_leg_delays_are_all_distinct() {
    let delays = [
        ("自动连接", AUTO_CONNECT_DELAY_MS),
        ("出口 IP 首探", EXIT_IP_PROBE_DELAY_MS),
        ("自动检查更新", AUTO_CHECK_UPDATE_DELAY_MS),
        ("helper 可升级探测", HELPER_UPGRADEABLE_DELAY_MS),
    ];
    for (i, (name_a, a)) in delays.iter().enumerate() {
        for (name_b, b) in &delays[i + 1..] {
            assert_ne!(
                a, b,
                "「{name_a}」与「{name_b}」都排在 {a}ms —— 违反本文件的启动腿错峰约定"
            );
        }
    }
}
