//! Android's independent libbox host for stopped-state speed tests.
//! The on-disk config is credential-free; check and start get the same
//! authenticated in-memory derivative. This host never enters ProxyRuntime.

use super::{authenticated_android_temp_config, TempCoreCleanupUnknown, TempCoreDeps};
use crate::runtime::proxy::android_bridge::{self, SpeedtestStartError, TransientSpeedtestState};
use crate::runtime::tailscale_login_core::{ConfigChecker, LoginCoreChild, LoginCoreSpawner};
use async_trait::async_trait;
use polaris_config_engine::singbox::InboundUser;
use polaris_core_supervisor::{PortExclusions, SpawnError, SpawnRequest};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS_EPOCH: OnceLock<Result<String, String>> = OnceLock::new();

fn next_instance_id() -> Result<String, String> {
    let epoch = PROCESS_EPOCH
        .get_or_init(crate::commands::config::generate_local_api_secret)
        .as_ref()
        .map_err(Clone::clone)?;
    let seq = NEXT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |seq| {
            seq.checked_add(1)
        })
        .map_err(|_| "Android 测速实例序号已耗尽".to_owned())?;
    Ok(format!("{epoch}:{seq:016x}"))
}

struct AndroidSpeedtestSpawner {
    auth: Arc<InboundUser>,
}

struct AndroidSpeedtestChecker {
    auth: Arc<InboundUser>,
}

struct AndroidSpeedtestChild {
    instance_id: String,
    closed: bool,
}

impl TempCoreDeps {
    pub fn production_android(
        config_dir: PathBuf,
        exclusions: PortExclusions,
        log_level: String,
        auth: InboundUser,
    ) -> Self {
        let mut deps = Self::production(config_dir, exclusions, log_level);
        let auth = Arc::new(auth);
        deps.spawner = Arc::new(AndroidSpeedtestSpawner {
            auth: Arc::clone(&auth),
        });
        deps.checker = Arc::new(AndroidSpeedtestChecker { auth });
        // SpawnRequest still needs a path, but Android invokes the bundled
        // libbox factory. Never probe for the desktop sing-box executable.
        deps.resolve_binary = Arc::new(|| Ok(PathBuf::from("android-libbox")));
        deps
    }
}

fn spawn_error(message: String) -> SpawnError {
    SpawnError::Spawn {
        bin: PathBuf::from("android-libbox"),
        source: std::io::Error::other(message),
    }
}

#[async_trait]
impl ConfigChecker for AndroidSpeedtestChecker {
    async fn check(&self, _binary: &Path, config_path: &Path) -> Result<(), String> {
        let raw = std::fs::read_to_string(config_path)
            .map_err(|_| "Android 测速临时配置读取失败".to_owned())?;
        let config = authenticated_android_temp_config(&raw, &self.auth)?;
        match android_bridge::check_config(&config).await {
            polaris_core_supervisor::config_gate::ConfigCheckVerdict::Accepted => Ok(()),
            _ => Err("Android 测速临时配置校验失败或不可用".to_owned()),
        }
    }
}

#[async_trait]
impl LoginCoreSpawner for AndroidSpeedtestSpawner {
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        let raw = std::fs::read_to_string(&req.config)
            .map_err(|_| spawn_error("Android 测速临时配置读取失败".to_owned()))?;
        let config = authenticated_android_temp_config(&raw, &self.auth).map_err(spawn_error)?;
        let id = next_instance_id().map_err(spawn_error)?;
        match android_bridge::start_transient_speedtest(&id, &config).await {
            Ok(()) => Ok(Box::new(AndroidSpeedtestChild {
                instance_id: id,
                closed: false,
            })),
            Err(SpeedtestStartError::Failed(message)) => Err(spawn_error(message)),
            Err(SpeedtestStartError::CleanupUnknown(message)) => Err(SpawnError::Spawn {
                bin: PathBuf::from("android-libbox"),
                source: std::io::Error::other(TempCoreCleanupUnknown(message)),
            }),
        }
    }
}

impl Drop for AndroidSpeedtestChild {
    fn drop(&mut self) {
        if !self.closed {
            let id = self.instance_id.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = android_bridge::close_transient_speedtest(&id).await;
                });
            }
        }
    }
}

#[async_trait]
impl LoginCoreChild for AndroidSpeedtestChild {
    fn pid(&self) -> Option<u32> {
        None
    }

    async fn wait(&mut self) {
        loop {
            match android_bridge::transient_speedtest_status(&self.instance_id).await {
                Ok(
                    TransientSpeedtestState::Closed
                    | TransientSpeedtestState::CleanupUnknown
                    | TransientSpeedtestState::Unknown,
                )
                | Err(_) => return,
                Ok(
                    TransientSpeedtestState::Starting
                    | TransientSpeedtestState::Running
                    | TransientSpeedtestState::Closing,
                ) => {}
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }

    async fn terminate(&mut self) {
        let _ = self.close_confirmed().await;
    }

    async fn close_confirmed(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        android_bridge::close_transient_speedtest(&self.instance_id).await?;
        self.closed = true;
        Ok(())
    }
}
