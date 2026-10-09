use super::*;

/// 桌面三平台上的真值表（存盘档位在那里是真实存在的三种接管形态，逐档算数）。
#[test]
fn should_enable_system_proxy_only_for_systemproxy_on_desktop() {
    for platform in [Platform::Mac, Platform::Win, Platform::Linux] {
        assert!(should_enable_system_proxy(
            ProxyModeType::SystemProxy,
            platform
        ));
        assert!(!should_enable_system_proxy(ProxyModeType::Tun, platform));
        assert!(!should_enable_system_proxy(ProxyModeType::Manual, platform));
    }
}

/// **Android：三个存盘档位得到同一个答案，而那个答案是「不去动系统代理」。**
///
/// 正面断言写成「三档各自产出什么」而不是「不许调 enable」：后者会被「什么都没发生」骗过
/// （函数恒 false 也全绿），前者要求 `SystemProxy` 那一档**确实**从 true 翻成 false ——
/// 那一档正是全新安装 / 备份恢复的 Android 客户端拿到的存盘值，也是今天真机上那条
/// 「系统代理启用失败，流量未经代理」假错误的来源。
#[test]
fn android_never_enables_system_proxy_whatever_is_stored() {
    for mode in [
        ProxyModeType::SystemProxy,
        ProxyModeType::Tun,
        ProxyModeType::Manual,
    ] {
        assert!(
            !should_enable_system_proxy(mode, Platform::Android),
            "Android + 存盘 {mode:?}：那个平台没有「系统代理」这个承载物（proxy_ops 的 Android 臂\
             全是 Err），去调 enable 只会给用户弹一条与现实相反的错误"
        );
    }
    // 反向对照：同一个 `SystemProxy` 档在桌面上必须仍然为真，否则上面那条「全 false」
    // 可能只是因为谓词整个坏掉了。
    assert!(should_enable_system_proxy(
        ProxyModeType::SystemProxy,
        Platform::Linux
    ));
}

#[test]
fn restart_system_proxy_cleanup_truth_table() {
    use ProxyModeType::{Manual, SystemProxy, Tun};
    let platform = Platform::Linux;
    assert!(should_clear_system_proxy_between_restart(
        Some(SystemProxy),
        Some(Tun),
        platform
    ));
    assert!(should_clear_system_proxy_between_restart(
        Some(SystemProxy),
        Some(Manual),
        platform
    ));
    assert!(!should_clear_system_proxy_between_restart(
        Some(SystemProxy),
        Some(SystemProxy),
        platform
    ));
    for old in [None, Some(Tun), Some(Manual)] {
        for new in [None, Some(SystemProxy), Some(Tun), Some(Manual)] {
            assert!(
                !should_clear_system_proxy_between_restart(old, new, platform),
                "非 systemProxy 旧会话不得清系统代理：old={old:?} new={new:?}"
            );
        }
    }
    assert!(!should_clear_system_proxy_between_restart(
        Some(SystemProxy),
        None,
        platform
    ));
}

/// Android：重启空窗里**任何**新旧档位组合都不该去清系统代理。
///
/// 上一条在桌面上有 3 组为真；本条要求在 Android 上那 3 组全部翻成假 —— 因为那个平台上
/// 「旧会话留下的系统代理」这件事不存在，去清它只会在 `proxy_ops` 的 Android 臂上拿一条 Err。
#[test]
fn android_never_clears_system_proxy_between_restarts() {
    use ProxyModeType::{Manual, SystemProxy, Tun};
    let modes = [None, Some(SystemProxy), Some(Tun), Some(Manual)];
    for old in modes {
        for new in modes {
            assert!(
                !should_clear_system_proxy_between_restart(old, new, Platform::Android),
                "Android + old={old:?} new={new:?}：两侧生效值恒 Tun，跨模式离开 systemProxy 不成立"
            );
        }
    }
    // 反向对照：同一组输入在桌面上必须有为真的那一格（否则上面的「全假」没有信息量）。
    assert!(should_clear_system_proxy_between_restart(
        Some(SystemProxy),
        Some(Tun),
        Platform::Linux
    ));
}

struct EnableCounter(Arc<AtomicU64>);
impl SystemProxyClearer for EnableCounter {
    fn ensure_cleared(&mut self) -> bool {
        false
    }
    fn detect_foreign_proxy(&self) -> Option<String> {
        None
    }
    fn enable_system_proxy(&mut self, _: &ProxyEnableRequest) -> Result<(), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn recover_from_marker(&mut self) -> Result<bool, String> {
        Ok(false)
    }
}

#[tokio::test]
async fn enable_queued_before_query_cannot_write_after_cancellation_reopens_admission() {
    let count = Arc::new(AtomicU64::new(0));
    let takeover = SystemProxyTakeover::new(Box::new(EnableCounter(count.clone())));
    let request = ProxyEnableRequest {
        address: "127.0.0.1".into(),
        http_port: 7890,
        socks_port: 7890,
        bypass_list: vec![],
    };
    let controller = takeover.controller.clone();
    let guard = controller.lock().unwrap();
    let mut enable = std::pin::pin!(takeover.enable(request.clone(), None));
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    assert!(std::future::Future::poll(enable.as_mut(), &mut cx).is_pending());
    let query = takeover.begin_session_query();
    assert!(takeover.cancel_session_query(query));
    drop(guard);
    assert!(enable.await.unwrap().is_err());
    assert_eq!(count.load(Ordering::SeqCst), 0);
    takeover
        .enable(request.clone(), None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let current = takeover.begin_session_query();
    assert!(takeover.enable(request, None).await.unwrap().is_err());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(takeover.cancel_session_query(current));
}
