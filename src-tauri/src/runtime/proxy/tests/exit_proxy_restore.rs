use super::*;
use polaris_system_integration::exec::{Command, CommandOutput, CommandRunner};
use polaris_system_integration::proxy::{
    ProxyMarker, ProxyMarkerPhase, ProxyMarkerRead, StdMarkerFs, WindowsProxyRegistrySnapshot,
    WindowsRegistryDwordValue, WindowsRegistryStringValue,
};
use polaris_system_integration::proxy_ops::{
    windows_enable_values, SystemProxyController, SystemProxyOpsImpl, WindowsProxyRegistryValues,
    WindowsProxyRegistryWriter, WindowsProxyWriterError,
};

struct NoHostCommands;

impl CommandRunner for NoHostCommands {
    fn run(&self, _cmd: &Command, _timeout: Duration) -> Result<CommandOutput, String> {
        Err("test executor: host commands disabled".into())
    }
}

struct RegistryFixture {
    current: Mutex<WindowsProxyRegistrySnapshot>,
    fail_restore: AtomicBool,
    fail_capture: AtomicBool,
    ignore_restore: AtomicBool,
    fail_notify: AtomicBool,
    restores: std::sync::atomic::AtomicUsize,
}

impl WindowsProxyRegistryWriter for RegistryFixture {
    fn capture(&self) -> Result<WindowsProxyRegistrySnapshot, WindowsProxyWriterError> {
        if self.fail_capture.load(Ordering::SeqCst) {
            return Err(WindowsProxyWriterError::other("registry read denied"));
        }
        Ok(self.current.lock().unwrap().clone())
    }

    fn write(&self, values: &WindowsProxyRegistryValues) -> Result<(), WindowsProxyWriterError> {
        *self.current.lock().unwrap() = WindowsProxyRegistrySnapshot {
            proxy_server: WindowsRegistryStringValue::PresentValue(values.proxy_server.clone()),
            proxy_override: if values.proxy_override.is_empty() {
                WindowsRegistryStringValue::PresentEmpty
            } else {
                WindowsRegistryStringValue::PresentValue(values.proxy_override.clone())
            },
            proxy_enable: WindowsRegistryDwordValue::PresentValue(values.proxy_enable),
        };
        Ok(())
    }

    fn restore(
        &self,
        original: &WindowsProxyRegistrySnapshot,
    ) -> Result<(), WindowsProxyWriterError> {
        self.restores.fetch_add(1, Ordering::SeqCst);
        if self.fail_restore.load(Ordering::SeqCst) {
            return Err(WindowsProxyWriterError::other("registry restore denied"));
        }
        if !self.ignore_restore.load(Ordering::SeqCst) {
            *self.current.lock().unwrap() = original.clone();
        }
        Ok(())
    }

    fn notify_settings_changed(&self) -> Result<(), WindowsProxyWriterError> {
        if self.fail_notify.load(Ordering::SeqCst) {
            Err(WindowsProxyWriterError::other(
                "consumer notification failed",
            ))
        } else {
            Ok(())
        }
    }
}

fn fixture() -> (
    Arc<ProxyRuntime>,
    TestDir,
    ProxyMarker<StdMarkerFs>,
    Arc<RegistryFixture>,
    WindowsProxyRegistrySnapshot,
) {
    let dir = fresh_test_dir();
    let original = WindowsProxyRegistrySnapshot {
        proxy_server: WindowsRegistryStringValue::PresentValue("proxy.corp:3128".into()),
        proxy_override: WindowsRegistryStringValue::Absent,
        proxy_enable: WindowsRegistryDwordValue::PresentValue(1),
    };
    let writer = Arc::new(RegistryFixture {
        current: Mutex::new(original.clone()),
        fail_restore: AtomicBool::new(false),
        fail_capture: AtomicBool::new(false),
        ignore_restore: AtomicBool::new(false),
        fail_notify: AtomicBool::new(false),
        restores: std::sync::atomic::AtomicUsize::new(0),
    });
    let marker_path = dir
        .join(polaris_system_integration::PROXY_MARKER_FILENAME)
        .to_string_lossy()
        .into_owned();
    let marker = ProxyMarker::new(StdMarkerFs, marker_path.clone());
    let mut controller = SystemProxyController::new(
        SystemProxyOpsImpl::with_platform(NoHostCommands, Platform::Win)
            .with_windows_registry_writer(writer.clone(), true),
        ProxyMarker::new(StdMarkerFs, marker_path),
    );
    controller
        .enable(&ProxyEnableRequest {
            address: "127.0.0.1".into(),
            http_port: 7890,
            socks_port: 7890,
            bypass_list: vec![],
        })
        .unwrap();
    let rt = Arc::new(ProxyRuntime::new(
        Arc::new(ConfigManager::new(dir.clone())),
        Arc::new(HelperRuntime::never_installed_for_tests(dir.clone())),
        Arc::new(MeshRuntime::new(dir.clone())),
        Box::new(controller),
        Arc::new(NoNetworkDoh),
    ));
    (rt, dir, marker, writer, original)
}

#[tokio::test]
async fn exit_proxy_restore_failure_retains_marker_and_retry_restores_before_success() {
    let (rt, _dir, marker, writer, original) = fixture();
    writer.fail_restore.store(true, Ordering::SeqCst);
    let error = rt.shutdown_for_exit().await.unwrap_err();
    assert!(error.contains("系统代理恢复未确认") && error.contains("registry restore denied"));
    assert!(*rt.desktop_shutdown.lock().unwrap());
    assert!(
        rt.child.lock().unwrap().is_empty(),
        "core drain is independent of proxy restore"
    );
    assert_ne!(writer.capture().unwrap(), original);
    assert!(
        matches!(marker.read_checked(), ProxyMarkerRead::CurrentValidated(m)
        if m.phase == ProxyMarkerPhase::Restoring
            && m.exact_original.as_ref().unwrap().windows_registry.as_ref() == Some(&original))
    );

    writer.fail_restore.store(false, Ordering::SeqCst);
    rt.shutdown_for_exit().await.unwrap();
    assert_eq!(writer.capture().unwrap(), original);
    assert_eq!(marker.read_checked(), ProxyMarkerRead::Missing);
    assert_eq!(writer.restores.load(Ordering::SeqCst), 2);
    rt.shutdown_for_exit().await.unwrap();
    assert_eq!(
        writer.restores.load(Ordering::SeqCst),
        2,
        "no marker is a confirmed no-op"
    );
}

#[tokio::test]
async fn exit_proxy_restore_preserves_external_changes_instead_of_overwriting_them() {
    let (rt, _dir, marker, writer, _) = fixture();
    writer
        .write(&windows_enable_values(&ProxyEnableRequest {
            address: "proxy.other".into(),
            http_port: 3129,
            socks_port: 3129,
            bypass_list: vec![],
        }))
        .unwrap();
    let external = writer.capture().unwrap();
    rt.shutdown_for_exit().await.unwrap();
    assert_eq!(writer.capture().unwrap(), external);
    assert_eq!(writer.restores.load(Ordering::SeqCst), 0);
    assert_eq!(marker.read_checked(), ProxyMarkerRead::Missing);
}

#[tokio::test]
async fn cancelled_session_query_stops_normally_and_reopens_without_a_permanent_exit_fence() {
    let (rt, _dir, marker, writer, original) = fixture();
    let epoch = rt.begin_windows_session_query().unwrap();
    rt.restore_system_proxy_for_exit().await.unwrap();
    assert_eq!(writer.capture().unwrap(), original);
    assert_eq!(marker.read_checked(), ProxyMarkerRead::Missing);
    assert!(!*rt.desktop_shutdown.lock().unwrap());
    assert!(rt
        .start(serde_json::json!({}))
        .await
        .unwrap_err()
        .message
        .contains("shutting down"));
    rt.stop_for_session_cancel().await.unwrap();
    assert!(rt.cancel_windows_session_query(epoch));
    assert!(!*rt.desktop_shutdown.lock().unwrap());
    assert!(!rt.system_proxy.session_query_active());
}

#[tokio::test]
async fn stale_session_cancel_cannot_reopen_a_newer_query_and_failed_restore_keeps_the_marker() {
    let (rt, _dir, marker, writer, _) = fixture();
    let older = rt.begin_windows_session_query().unwrap();
    let newer = rt.begin_windows_session_query().unwrap();
    assert!(!rt.cancel_windows_session_query(older));
    assert!(rt.windows_session_query_is_current(newer));
    writer.fail_restore.store(true, Ordering::SeqCst);
    assert!(rt.restore_system_proxy_for_exit().await.is_err());
    assert!(matches!(
        marker.read_checked(),
        ProxyMarkerRead::CurrentValidated(_)
    ));
    assert!(rt.cancel_windows_session_query(newer));
    assert!(!*rt.desktop_shutdown.lock().unwrap());
}

#[tokio::test]
async fn restore_success_without_registry_effect_is_rejected_and_keeps_retry_authority() {
    let (rt, _dir, marker, writer, original) = fixture();
    writer.ignore_restore.store(true, Ordering::SeqCst);
    let error = rt.restore_system_proxy_for_exit().await.unwrap_err();
    assert!(error.contains("read-back mismatch"));
    assert_ne!(writer.capture().unwrap(), original);
    assert!(
        matches!(marker.read_checked(), ProxyMarkerRead::CurrentValidated(m) if m.phase == ProxyMarkerPhase::Restoring)
    );
    writer.ignore_restore.store(false, Ordering::SeqCst);
    rt.restore_system_proxy_for_exit().await.unwrap();
    assert_eq!(writer.capture().unwrap(), original);
    assert_eq!(marker.read_checked(), ProxyMarkerRead::Missing);
}

#[tokio::test]
async fn legacy_marker_native_read_failure_is_not_disabled_and_does_not_discard_the_marker() {
    let (rt, _dir, marker, writer, _) = fixture();
    std::fs::write(
        marker.path(),
        br#"{"our_host_port":"127.0.0.1:7890","at":0}"#,
    )
    .unwrap();
    assert!(matches!(marker.read_checked(), ProxyMarkerRead::Legacy(_)));
    writer.fail_capture.store(true, Ordering::SeqCst);
    let error = rt.restore_system_proxy_for_exit().await.unwrap_err();
    assert!(error.contains("registry read denied"));
    assert!(matches!(marker.read_checked(), ProxyMarkerRead::Legacy(_)));
    writer.fail_capture.store(false, Ordering::SeqCst);
    rt.restore_system_proxy_for_exit().await.unwrap();
    assert_eq!(
        writer.capture().unwrap().proxy_enable,
        WindowsRegistryDwordValue::PresentValue(0)
    );
    assert_eq!(marker.read_checked(), ProxyMarkerRead::Missing);
}

#[tokio::test]
async fn restored_registry_alone_cannot_erase_a_failed_consumer_notification_on_retry() {
    let (rt, _dir, marker, writer, original) = fixture();
    writer.fail_notify.store(true, Ordering::SeqCst);
    for _ in 0..2 {
        assert!(rt
            .restore_system_proxy_for_exit()
            .await
            .unwrap_err()
            .contains("consumer notification failed"));
        assert_eq!(writer.capture().unwrap(), original);
        assert!(
            matches!(marker.read_checked(), ProxyMarkerRead::CurrentValidated(m) if m.phase == ProxyMarkerPhase::Restoring)
        );
    }
    assert_eq!(
        writer.restores.load(Ordering::SeqCst),
        1,
        "retry confirms consumers without overwriting restored keys"
    );
    writer.fail_notify.store(false, Ordering::SeqCst);
    rt.restore_system_proxy_for_exit().await.unwrap();
    assert_eq!(marker.read_checked(), ProxyMarkerRead::Missing);
}

#[tokio::test]
async fn deferred_query_failure_returns_while_cancel_lock_is_held_and_reconciles_later() {
    deferred_cancel_under_contention(false).await;
}

#[tokio::test]
async fn deferred_old_query_failure_cannot_reopen_a_newer_query_after_lock_contention() {
    deferred_cancel_under_contention(true).await;
}

async fn deferred_cancel_under_contention(superseded: bool) {
    let (rt, _dir, _marker, _writer, _) = fixture();
    let epoch = rt.begin_windows_session_query().unwrap();
    let current = if superseded {
        rt.begin_windows_session_query().unwrap()
    } else {
        epoch
    };
    let (done, received) = std::sync::mpsc::channel();
    let task = {
        let admission = rt.desktop_shutdown.lock().unwrap();
        let (returned, callback_return) = std::sync::mpsc::channel();
        let worker_rt = rt.clone();
        let callback = std::thread::spawn(move || {
            // Exercise the same observed-failure return path used by QUERY.
            let result = crate::session_restore::wait(Duration::from_secs(2), |send| {
                std::thread::spawn(move || {
                    crate::session_restore::complete(
                        send,
                        Err("restore denied".to_string()),
                        |_| {},
                    );
                });
            });
            assert!(matches!(result, Ok(Err(_))));
            let task = worker_rt.defer_windows_session_query_cancel(epoch, move |reopened| {
                done.send(reopened).unwrap();
            });
            returned.send(task).unwrap();
        });
        // This is an OS-backed deadline, independent of the async executor.
        // A regression to synchronous cancellation fails here and drops the lock.
        let task = callback_return
            .recv_timeout(Duration::from_secs(3))
            .expect("QUERY failure must return while cancellation lock remains held");
        callback.join().unwrap();
        assert!(received.try_recv().is_err());
        assert!(rt.windows_session_query_is_current(current));
        drop(admission);
        task
    };
    task.await.unwrap();
    assert_eq!(
        received.recv_timeout(Duration::from_secs(1)).unwrap(),
        !superseded
    );
    assert_eq!(rt.system_proxy.session_query_active(), superseded);
    if superseded {
        assert!(!rt.windows_session_query_is_current(epoch));
    }
}
