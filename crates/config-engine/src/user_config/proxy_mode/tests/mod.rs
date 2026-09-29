use super::*;

/// [`ProxyModeType::effective_on`] 的整张表，逐个 [`Platform`] 变体钉死。
///
/// # 为什么这条不能被上层的四道门替代
///
/// 那四道门（入站金样 / 整份配置不变式 / 日志谓词 / 桌面反向对照）取材面只有 linux/darwin/win32/
/// android 四个**字符串**平台，`Platform::Other` 在它们里一次都没跑过。而 `Other` 的语义是
/// 「本仓没为这个平台答过题」—— 它必须**原样返回用户存的档位**，而不是跟着 Android 走：
/// 一个未知平台是否只有一种接管形态，本仓无从知道，替它决定就是把 Android 的事实外推成兜底。
///
/// # 2026-09-06：分叉的平台从一个变成两个（Android + iOS），`Other` 那一半一个字没动
///
/// 加 `Platform::Ios` 时本条按设计红了一次（iOS 落进 else 分支，被要求「原样返回用户存的档位」）。
/// 答案是 iOS **也**恒 Tun，但**依据与 Android 不同**，故 `effective_on` 里写的是独立臂：
/// Android 的理由是「本仓在 Android 上根本不发 mixed，连一个可指的端口都没有」，
/// iOS 的理由是「发了也没人能指过来」（没有让用户把第三方应用出流量指到本地口的设置面）。
/// 两条理由随不同的事实变动：前者随我们自己「发不发 mixed」的决定变，后者不随我们变。
///
/// 循环判据仍写成「分叉集合」而不是逐平台列举，理由与原来相同：再来一个平台时它还会红。
#[test]
fn effective_on_only_diverges_for_mobile_platforms() {
    // 生效值恒 `Tun` 的平台集合。两个成员各有各的依据（见上文与 `effective_on` 的两条独立臂），
    // 放在同一个集合里只是因为**结果**相同，不代表理由相同。
    let mobile = [Platform::Android, Platform::Ios];
    for platform in Platform::ALL.iter().copied() {
        for mode in [
            ProxyModeType::SystemProxy,
            ProxyModeType::Tun,
            ProxyModeType::Manual,
        ] {
            let got = mode.effective_on(platform);
            if mobile.contains(&platform) {
                assert_eq!(
                    got,
                    ProxyModeType::Tun,
                    "{platform:?} + {mode:?}：那里只有一种接管形态（系统授予的隧道 fd），生效值恒 Tun"
                );
            } else {
                assert_eq!(
                    got, mode,
                    "{platform:?} + {mode:?}：非移动端平台必须原样返回用户存的档位"
                );
            }
        }
    }
    // 取材面自检：`Platform::ALL` 被清空 / 循环写歪的话，上面两条断言一次都不跑也全绿。
    assert!(
        Platform::ALL.len() >= 5,
        "Platform::ALL 只有 {} 个变体，取材面塌了",
        Platform::ALL.len()
    );
    // 正面断言：**两侧都非空**。只判「分叉集合里的恒 Tun」会被「ALL 里恰好没有这两个」骗过，
    // 只判 else 那半会被「全部平台都在分叉集合里」骗过。
    for m in mobile {
        assert!(
            Platform::ALL.contains(&m),
            "{m:?} 不在 Platform::ALL 里 —— 上面那半断言一次都没跑"
        );
    }
    assert!(
        Platform::ALL.iter().any(|p| !mobile.contains(p)),
        "全部变体都落在分叉集合里 —— 「原样返回」那半断言一次都没跑"
    );
}

/// `is_tun` 与 `effective_on` 的接线：移动端两平台上**三档都**判 TUN，桌面只有 TUN 档判 TUN。
#[test]
fn is_tun_follows_the_effective_value() {
    for platform in [Platform::Android, Platform::Ios] {
        for mode in [
            ProxyModeType::SystemProxy,
            ProxyModeType::Tun,
            ProxyModeType::Manual,
        ] {
            assert!(
                mode.effective_on(platform).is_tun(),
                "{platform:?} + {mode:?} 必须判 TUN"
            );
        }
    }
    // 反向对照：桌面上非 TUN 档仍判否，否则上面那条会被「is_tun 恒真」骗过。
    assert!(!ProxyModeType::SystemProxy
        .effective_on(Platform::Linux)
        .is_tun());
    assert!(!ProxyModeType::Manual.effective_on(Platform::Mac).is_tun());
    assert!(ProxyModeType::Tun.effective_on(Platform::Win).is_tun());
}
