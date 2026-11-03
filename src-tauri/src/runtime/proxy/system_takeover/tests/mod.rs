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

struct MutationCounter {
    clears: Arc<AtomicU64>,
    enables: Arc<AtomicU64>,
}
impl SystemProxyClearer for MutationCounter {
    fn ensure_cleared(&mut self) -> bool {
        self.clears.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn detect_foreign_proxy(&self) -> Option<String> {
        None
    }
    fn enable_system_proxy(&mut self, _: &ProxyEnableRequest) -> Result<(), String> {
        self.enables.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn recover_from_marker(&mut self) -> Result<bool, String> {
        Ok(false)
    }
}

#[tokio::test]
async fn cancelled_clear_waiter_cannot_clear_a_successor_marker() {
    use polaris_core_supervisor::lifecycle_gate::LifecycleGate;
    let clears = Arc::new(AtomicU64::new(0));
    let enables = Arc::new(AtomicU64::new(0));
    let takeover = SystemProxyTakeover::new(Box::new(MutationCounter {
        clears: clears.clone(),
        enables: enables.clone(),
    }));
    let gate = Arc::new(LifecycleGate::default());
    let generation = gate.generation();
    let observed_gate = gate.clone();
    let controller = takeover.controller.clone();
    let checked_rx = {
        let mut held = controller.lock().unwrap();
        let (checked_tx, checked_rx) = tokio::sync::oneshot::channel();
        let mut cleanup = Box::pin(takeover.clear_when(None, move || {
            let current = observed_gate.try_generation() == Some(generation);
            let _ = checked_tx.send(current);
            current
        }));
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(std::future::Future::poll(cleanup.as_mut(), &mut cx).is_pending());
        drop(cleanup); // spawn_blocking survives cancellation, while still behind controller lock.
        gate.bump_generation();
        // Model successor enable under the same controller mutex before the old worker obtains it.
        held.enable_system_proxy(&ProxyEnableRequest {
            address: "127.0.0.1".into(),
            http_port: 7890,
            socks_port: 7890,
            bypass_list: vec![],
        })
        .unwrap();
        checked_rx
    }; // Release the controller before any await; the cancelled worker remains fenced.
    assert!(!checked_rx.await.unwrap());
    // Acquiring the controller again proves no lock was leaked; zero old mutation occurred.
    drop(controller.lock().unwrap());
    assert_eq!(enables.load(Ordering::SeqCst), 1);
    assert_eq!(clears.load(Ordering::SeqCst), 0);
    assert!(
        !takeover.clear_when(None, || false).await,
        "unknown must fail closed"
    );
    assert_eq!(clears.load(Ordering::SeqCst), 0);
    let current = gate.generation();
    assert!(
        takeover
            .clear_when(None, move || gate.try_generation() == Some(current))
            .await
    );
    assert_eq!(clears.load(Ordering::SeqCst), 1);
}

fn prepared_proxy_fixture() -> PreparedProxyEffect {
    let snapshot = polaris_system_integration::proxy::ProxyTransactionSnapshot::default();
    serde_json::from_value(serde_json::json!({
        "marker_path": "/proxy-live.json", "marker": null,
        "before": snapshot, "target": snapshot, "request": null,
    })).unwrap()
}

struct TypedProxyCounter {
    writes: Arc<AtomicU64>,
    prepared: PreparedProxyEffect,
}
impl SystemProxyClearer for TypedProxyCounter {
    fn ensure_cleared(&mut self) -> bool { false }
    fn detect_foreign_proxy(&self) -> Option<String> { None }
    fn enable_system_proxy(&mut self, _: &ProxyEnableRequest) -> Result<(), String> { unreachable!("live must not use legacy enable") }
    fn recover_from_marker(&mut self) -> Result<bool, String> { Ok(false) }
    fn prepare_host_effect(&self, _: Option<&ProxyEnableRequest>) -> Result<PreparedProxyEffect, String> { Ok(self.prepared.clone()) }
    fn host_effect_facts_match(&self, prepared: &PreparedProxyEffect) -> Result<bool, String> {
        Ok(serde_json::to_value(prepared).unwrap() == serde_json::to_value(&self.prepared).unwrap())
    }
    fn apply_host_effect(&mut self, prepared: &PreparedProxyEffect, operation_id: &str) -> TakeoverReceipt {
        assert!(self.host_effect_facts_match(prepared).unwrap());
        self.writes.fetch_add(1, Ordering::SeqCst);
        TakeoverReceipt {
            operation_id: operation_id.to_owned(), outcome: TakeoverOutcome::Applied,
            marker_id: Some("original-marker".into()), cleanup_pending: false, reason: None,
        }
    }
}

#[tokio::test]
async fn typed_proxy_checks_original_session_epoch_and_owner_inside_controller_lock() {
    let writes = Arc::new(AtomicU64::new(0));
    let prepared = prepared_proxy_fixture();
    let takeover = Arc::new(SystemProxyTakeover::new(Box::new(TypedProxyCounter { writes: writes.clone(), prepared: prepared.clone() })));
    let epoch = takeover.host_epoch();
    let controller = takeover.controller.clone();
    let held = controller.lock().unwrap();
    let worker = takeover.clone();
    let worker_plan = prepared.clone();
    let (queued_tx, queued_rx) = std::sync::mpsc::channel();
    let task = tokio::task::spawn_blocking(move || {
        queued_tx.send(()).unwrap();
        worker.apply_host_effect_when(&worker_plan, "old-operation", epoch, || true)
    });
    queued_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    let query = takeover.begin_session_query();
    assert!(takeover.cancel_session_query(query));
    drop(held);
    assert_eq!(task.await.unwrap().outcome, TakeoverOutcome::Superseded);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    let epoch = takeover.host_epoch();
    assert_eq!(takeover.apply_host_effect_when(&prepared, "revoked-owner", epoch, || false).outcome, TakeoverOutcome::Superseded);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    let result = takeover.apply_host_effect_when(&prepared, "current-owner", epoch, || true);
    assert_eq!(result.outcome, TakeoverOutcome::Applied);
    assert_eq!(result.operation_id, "current-owner");
    assert_eq!(result.marker_id.as_deref(), Some("original-marker"));
    assert_eq!(writes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn dropped_typed_proxy_waiter_does_not_cancel_the_queued_owner() {
    let writes = Arc::new(AtomicU64::new(0));
    let prepared = prepared_proxy_fixture();
    let takeover = Arc::new(SystemProxyTakeover::new(Box::new(TypedProxyCounter { writes: writes.clone(), prepared: prepared.clone() })));
    let epoch = takeover.host_epoch();
    let controller = takeover.controller.clone();
    let held = controller.lock().unwrap();
    let worker = takeover.clone();
    let (queued_tx, queued_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let observer = tokio::task::spawn_blocking(move || {
        queued_tx.send(()).unwrap();
        let receipt = worker.apply_host_effect_when(&prepared, "retained-operation", epoch, || true);
        done_tx.send(receipt).unwrap();
    });
    queued_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    drop(observer); // detach only; the real owner still retains controller + operation
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    drop(held);
    let result = done_rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(result.outcome, TakeoverOutcome::Applied);
    assert_eq!(result.operation_id, "retained-operation");
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    drop(controller.lock().unwrap());
}

#[test]
fn typed_proxy_missing_adapter_never_calls_startup_enable() {
    let writes = Arc::new(AtomicU64::new(0));
    let takeover = SystemProxyTakeover::new(Box::new(EnableCounter(writes.clone())));
    assert!(takeover.prepare_host_effect(None).is_err());
    let result = takeover.apply_host_effect_when(&prepared_proxy_fixture(), "no-adapter", takeover.host_epoch(), || true);
    assert_eq!(result.outcome, TakeoverOutcome::Unsupported);
    assert!(!result.cleanup_pending);
    assert_eq!(writes.load(Ordering::SeqCst), 0);
}

#[test]
fn shared_live_and_startup_proxy_projection_preserves_effective_platform_and_bypass() {
    let config: UserConfig = serde_json::from_value(serde_json::json!({
        "servers":[], "selectedServerId":null, "proxyModeType":"systemProxy",
        "mixedPort":8123, "bypassLAN":true, "bypassLANList":["internal.example"],
    })).unwrap();
    for platform in [Platform::Mac, Platform::Win, Platform::Linux] {
        let request = runtime_system_proxy_request(&config, platform).unwrap();
        assert_eq!(request.address, "127.0.0.1");
        assert_eq!(request.http_port, 8123);
        assert_eq!(request.socks_port, 8123);
        assert!(request.bypass_list.iter().any(|entry| entry == "internal.example"));
    }
    assert!(runtime_system_proxy_request(&config, Platform::Android).is_none());
    assert!(runtime_system_proxy_request(&config, Platform::Ios).is_none());
}
