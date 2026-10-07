use super::*;

struct NoSystemProxy;
impl crate::runtime::proxy::system_takeover::SystemProxyClearer for NoSystemProxy {
    fn ensure_cleared(&mut self) -> bool {
        false
    }
    fn detect_foreign_proxy(&self) -> Option<String> {
        None
    }
    fn enable_system_proxy(
        &mut self,
        _: &polaris_system_integration::proxy_ops::ProxyEnableRequest,
    ) -> Result<(), String> {
        panic!("F1 fixture cannot mutate OS proxy")
    }
    fn recover_from_marker(&mut self) -> Result<bool, String> {
        Ok(false)
    }
}
struct NoNetwork;
#[async_trait::async_trait]
impl polaris_dns_race::DohPost for NoNetwork {
    async fn post_dns_message(&self, _: &str, _: Vec<u8>) -> Result<Vec<u8>, String> {
        panic!("F1 fixture cannot use host network")
    }
}
fn runtime() -> (Arc<ProxyRuntime>, crate::test_support::TestDir) {
    let dir = crate::test_support::TestDir::new("polaris-f1-census-");
    let runtime = Arc::new(ProxyRuntime::new(
        Arc::new(crate::runtime::config::ConfigManager::new(
            dir.to_path_buf(),
        )),
        Arc::new(
            crate::runtime::helper::HelperRuntime::never_installed_for_tests(dir.to_path_buf()),
        ),
        Arc::new(crate::runtime::mesh::MeshRuntime::new(dir.to_path_buf())),
        Box::new(NoSystemProxy),
        Arc::new(NoNetwork),
    ));
    (runtime, dir)
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[tokio::test]
async fn queued_original_dispatch_cancel_and_drop_never_clear_pause() {
    const MARKER: &str = "POLARIS_F1_QUEUED_PAUSE_CHILD";
    if std::env::var_os(MARKER).is_none() {
        let test = format!(
            "{}::queued_original_dispatch_cancel_and_drop_never_clear_pause",
            module_path!().split_once("::").unwrap().1
        );
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test, "--nocapture"])
            .env(MARKER, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"));
        return;
    }
    let (runtime, _dir) = runtime();
    let generation = runtime.core_generation();
    let (_, claim) = runtime
        .guarded_start_completion(&serde_json::Value::Null, Some(generation))
        .unwrap_or_else(|_| panic!("original queue admission failed"));
    let claim = claim.unwrap();
    let pause = runtime.pause_pc_producers(generation).await.unwrap();
    let membership = pause.seal(&runtime).unwrap();
    assert_eq!(
        membership.member_counts().0,
        1,
        "real original queued row is retained before first poll"
    );
    assert!(membership.main_native_phases()[0].1.is_empty());
    assert!(pause.surrender(&runtime).unwrap_err().contains("dispatch"));
    let retry = pause.clone();
    drop(claim);
    drop(pause);
    assert!(retry.surrender(&runtime).is_err());
    assert!(runtime
        .explicit_start_completion(serde_json::Value::Null)
        .is_err());
    assert!(
        runtime.pause_pc_producers(generation).await.is_err(),
        "no fresh lease replaces the original cutoff"
    );
    assert!(runtime.normal_start.lock().unwrap().pause.is_some());
}
