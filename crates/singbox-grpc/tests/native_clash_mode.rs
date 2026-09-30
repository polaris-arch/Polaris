//! Optional contract test against the actual bundled sing-box executable. It starts only a
//! loopback management service; no TUN, system proxy, or external traffic is configured.

#[path = "../../config-engine/tests/support/kernel_run.rs"]
mod kernel_run;

use polaris_singbox_grpc::{Endpoint, SingBoxApiClient};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct Core(Child);
impl Drop for Core {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
#[ignore = "requires POLARIS_SINGBOX_PATH or the locally bundled Linux sing-box executable"]
async fn native_core_lists_and_switches_the_two_compiled_modes() {
    if !kernel_run::kernel_run_or_skip("native_core_lists_and_switches_the_two_compiled_modes") {
        return;
    }
    let binary = std::env::var("POLARIS_SINGBOX_PATH").unwrap_or_else(|_| {
        format!(
            "{}/../../resources/linux/sing-box",
            env!("CARGO_MANIFEST_DIR")
        )
    });
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let path = std::env::temp_dir().join(format!(
        "polaris-native-clash-mode-{}-{port}.json",
        std::process::id()
    ));
    let config = format!(
        r#"{{
      "log": {{"level":"error"}},
      "outbounds": [{{"type":"direct","tag":"direct"}}],
      "route": {{"rules": [
        {{"clash_mode":"normal","action":"route","outbound":"direct"}},
        {{"clash_mode":"mesh-direct","action":"route","outbound":"direct"}}
      ],"final":"direct"}},
      "experimental": {{"clash_api":{{"default_mode":"normal"}}}},
      "services": [{{"type":"api","listen":"127.0.0.1","listen_port":{port}}}]
    }}"#
    );
    std::fs::write(&path, config).unwrap();
    let child = kernel_run::with_run(Command::new(binary))
        .arg("-c")
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut core = Core(child);
    let client = SingBoxApiClient::connect(Endpoint::new("127.0.0.1", port), "")
        .await
        .unwrap();
    let mut status = None;
    for _ in 0..60 {
        if let Ok(found) = client.get_clash_mode_status().await {
            status = Some(found);
            break;
        }
        assert!(
            core.0.try_wait().unwrap().is_none(),
            "native core exited before management API was ready"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let before = status.expect("native core did not expose GetClashModeStatus");
    assert_eq!(before.mode_list, ["mesh-direct", "normal"]);
    assert_eq!(before.current_mode, "normal");
    client.set_clash_mode("mesh-direct").await.unwrap();
    assert_eq!(
        client.get_clash_mode_status().await.unwrap().current_mode,
        "mesh-direct"
    );
    client.set_clash_mode("normal").await.unwrap();
    assert_eq!(
        client.get_clash_mode_status().await.unwrap().current_mode,
        "normal"
    );
    drop(core);
    std::fs::remove_file(path).unwrap();
}
