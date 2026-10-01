//! 瞬态 Tailscale 登录核配置的随包核 schema 门。
//! `check` 仅解析与初始化配置；不 Start、不连接控制面、不需要 Tailscale 账号。

#[path = "../../config-engine/tests/support/core_locator.rs"]
mod core_locator;

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use polaris_config_engine::user_config::server_config::{
    MeshInboundGrant, MeshInboundNetwork, MeshInboundPolicy, MeshInboundTarget, Protocol,
    ServerConfig, TailscaleSettings,
};
use polaris_mesh::{
    build_tailscale_login_config, login_config_to_json, TailscaleLoginApiService,
    TAILSCALE_LOGIN_ENDPOINT_TAG,
};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let tick = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "polaris-tailscale-login-check-{}-{tick}",
            std::process::id()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check(core: &Path, config: &Path) {
    let output = core_locator::command_for_core(core)
        .arg("--disable-color")
        .arg("check")
        .arg("-c")
        .arg(config)
        .output()
        .unwrap_or_else(|error| panic!("无法执行随包核 check: {error}"));
    assert!(
        output.status.success(),
        "瞬态 Tailscale 登录配置被随包核拒绝：{}\n{}",
        config.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn bundled_core_accepts_isolated_tailscale_login_config() {
    let Some(core) = core_locator::core_or_skip("Tailscale 瞬态登录核配置 check") else {
        return;
    };
    let dir = TestDir::new();
    let allow = MeshInboundPolicy::Allowlist {
        rules: vec![MeshInboundGrant {
            source_cidrs: vec!["100.64.0.2/32".into()],
            network: MeshInboundNetwork::Both,
            ports: vec!["53".into()],
            target: MeshInboundTarget::Local,
            target_cidrs: vec![],
        }],
    };
    for (index, (name, policy)) in [
        ("", None),
        ("renamed-node", Some(allow)),
        ("blocked-node", Some(MeshInboundPolicy::Block)),
    ]
    .into_iter()
    .enumerate()
    {
        let server = ServerConfig {
            id: format!("ts{index}"),
            name: name.into(),
            protocol: Protocol::Tailscale,
            tailscale_settings: Some(Box::new(TailscaleSettings {
                auth_key: Some("test-only-placeholder-key".into()),
                control_url: Some("https://headscale.example".into()),
                ..Default::default()
            })),
            mesh_inbound_policy: policy,
            ..Default::default()
        };
        let api = TailscaleLoginApiService {
            port: 19090,
            secret: "test-only-api-secret".into(),
        };
        let generated = build_tailscale_login_config(&server, &dir.0, &api).unwrap();
        let value = login_config_to_json(&generated);
        assert_eq!(value["endpoints"][0]["tag"], TAILSCALE_LOGIN_ENDPOINT_TAG);
        assert_eq!(
            value["route"]["rules"][0]["inbound"][0],
            TAILSCALE_LOGIN_ENDPOINT_TAG
        );
        assert_eq!(
            value["dns"]["rules"][0]["inbound"][0],
            TAILSCALE_LOGIN_ENDPOINT_TAG
        );
        let path = dir.0.join(format!("login-{index}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        check(&core, &path);
    }
}
