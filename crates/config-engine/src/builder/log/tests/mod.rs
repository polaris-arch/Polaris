use super::*;

fn deps(privacy: bool, platform: Platform, path: Option<&str>) -> LogBuildDeps<'_> {
    LogBuildDeps {
        privacy_mode: privacy,
        platform,
        log_file_path: path,
    }
}

#[test]
fn system_proxy_no_output() {
    let input = LogConfigInput {
        proxy_mode_type: ProxyModeType::SystemProxy,
        ..Default::default()
    };
    let cfg = build_log_config(&input, &deps(false, Platform::Linux, Some("/tmp/sb.log")));
    assert_eq!(cfg.level, "info");
    assert!(cfg.timestamp);
    assert!(cfg.disabled.is_none());
    assert!(cfg.output.is_none(), "systemProxy 不写文件");
}

#[test]
fn tun_linux_writes_output() {
    let input = LogConfigInput {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let cfg = build_log_config(
        &input,
        &deps(false, Platform::Linux, Some("/fake/singbox.log")),
    );
    assert_eq!(cfg.output.as_deref(), Some("/fake/singbox.log"));
}

#[test]
fn tun_mac_writes_output() {
    let input = LogConfigInput {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let cfg = build_log_config(&input, &deps(false, Platform::Mac, Some("/fake/sb.log")));
    assert_eq!(cfg.output.as_deref(), Some("/fake/sb.log"));
}

/// TUN 下**每一个** [`Platform`] 变体都必须写日志文件。
///
/// # 为什么要有这条（它守的是一次「本次改动引入的回归」，不是旧缺陷）
///
/// Android 上核是进程内 libbox，没有子进程可接管道 ⇒ stdout 更加不可捕获 ⇒ 日志落盘是导出
/// 诊断报告时拿到核原文的唯一途径。K10 之前 Android 走 `Platform::parse("android") == Other`
/// 顺带得到正确行为；给 Android 具名之后那条腿断了，谓词若不同步补 `Platform::Android`，
/// Android 会**静默停止落盘**（配置里 `log.output` 变成 null，日志页与诊断都不会报错）。
///
/// 2026-09-06 加 `Platform::Ios` 时本条按设计红了一次（谓词的允许清单里没有 Ios），
/// 逼出了同一个答案：iOS 上核在 NE 扩展进程内，app 进程连它的 stdout 都不在同一个进程树上，
/// 落盘（写进 App Group 共享容器）同样是导出诊断拿到核原文的唯一途径。
///
/// 判据取全变体而不是列举：新增平台时这条会红，逼着回答「这个平台捕不捕得到核 stdout」。
#[test]
fn every_platform_writes_the_log_file_under_tun() {
    let input = LogConfigInput {
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    for platform in Platform::ALL.iter().copied() {
        let cfg = build_log_config(&input, &deps(false, platform, Some("/fake/sb.log")));
        assert_eq!(
            cfg.output.as_deref(),
            Some("/fake/sb.log"),
            "{platform:?}: TUN 下核 stdout 不可捕获，必须落文件"
        );
    }

    // 反向对照：**桌面**上非 TUN 一律不落文件（否则上面那条会被「谓词恒真」骗过）。
    //
    // Android 与 iOS 都不在这条对照里，而且它们的答案是相反的 —— 见下面那半。把它们留在这个
    // 循环里就等于把「移动端只有 TUN 一种接管形态」这条事实断言成它的反面。
    // （两者被排除的**依据不同**：`ProxyModeType::effective_on` 对 Android 与 Ios 各有一条独立臂，
    //  理由分别是「本仓不发 mixed」与「发了也没人能指过来」，见该函数。）
    let non_tun = LogConfigInput {
        proxy_mode_type: ProxyModeType::SystemProxy,
        ..Default::default()
    };
    let mut desktop_seen = 0usize;
    for platform in Platform::ALL
        .iter()
        .copied()
        .filter(|p| !matches!(p, Platform::Android | Platform::Ios))
    {
        assert!(
            build_log_config(&non_tun, &deps(false, platform, Some("/fake/sb.log")))
                .output
                .is_none(),
            "{platform:?}: systemProxy 下核有管道，不该写文件"
        );
        desktop_seen += 1;
    }
    // 取材面自检：全被过滤掉的话上面的否定断言恒真、这条对照零信息量。
    assert_eq!(
        desktop_seen,
        Platform::ALL.len() - 2,
        "非移动端变体只跑到 {desktop_seen} 个，反向对照塌了"
    );

    // **Android：三个 `proxy_mode_type` 全都要落文件**。
    //
    // 那里的接管方式只有 `VpnService` 一种（判据全在 `ProxyModeType::effective_on`），而
    // `proxy_mode_type` 的缺省值是 `SystemProxy` ⇒ 照裸值判会让全新安装 / 备份恢复的 Android
    // 客户端**一条核日志都不落盘**，而落盘正是那里拿到核原文的唯一途径（进程内 libbox，
    // 没有子进程管道可接）。症状是「导出诊断里核日志是空的」，离成因很远，所以钉在这里。
    // **iOS 同形（2026-09-06）**：三个 `proxy_mode_type` 全都要落文件，理由与 Android 同向、
    // 依据不同（`effective_on` 的 `Ios` 臂：iOS 上「手动」没人能指过来、「系统代理」无 API 面）。
    // 两个平台各跑一遍而不是并成一个循环：并起来就等于断言「同一个理由」。
    for (platform, who) in [(Platform::Android, "Android"), (Platform::Ios, "iOS")] {
        for mode in [
            ProxyModeType::SystemProxy,
            ProxyModeType::Manual,
            ProxyModeType::Tun,
        ] {
            let input = LogConfigInput {
                proxy_mode_type: mode,
                ..Default::default()
            };
            assert_eq!(
                build_log_config(&input, &deps(false, platform, Some("/fake/sb.log")))
                    .output
                    .as_deref(),
                Some("/fake/sb.log"),
                "{who} + {mode:?}: 接管方式在这个平台上只有一种，日志落盘不能跟着存量档位走"
            );
        }
    }
}

#[test]
fn privacy_raises_level() {
    let input = LogConfigInput {
        log_level: LogLevel::Debug,
        proxy_mode_type: ProxyModeType::SystemProxy,
        ..Default::default()
    };
    let cfg = build_log_config(&input, &deps(true, Platform::Linux, None));
    assert_eq!(cfg.level, "warn", "隐私模式 debug → warn");
}

#[test]
fn disable_log_file_short_circuits() {
    let input = LogConfigInput {
        disable_log_file: true,
        proxy_mode_type: ProxyModeType::Tun,
        ..Default::default()
    };
    let cfg = build_log_config(&input, &deps(false, Platform::Linux, Some("/fake/sb.log")));
    assert_eq!(cfg.disabled, Some(true));
    assert!(cfg.output.is_none(), "disabled 时不写 output");
}

#[test]
fn manual_mode_no_output() {
    let input = LogConfigInput {
        proxy_mode_type: ProxyModeType::Manual,
        ..Default::default()
    };
    let cfg = build_log_config(&input, &deps(false, Platform::Linux, Some("/fake/sb.log")));
    assert!(cfg.output.is_none(), "manual 模式 stdout 直喂不写文件");
}
