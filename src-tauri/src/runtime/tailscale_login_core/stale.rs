//! Leftovers of a login core whose host process died: its secret-bearing temporary config and,
//! on desktop, the detached child itself. Both are identified only by this app's own exact
//! artifacts: the fixed config file name inside the app config directory, and a child whose
//! argv is this app's core binary running exactly such a config. Nothing is matched by name.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use polaris_core_supervisor::{CoreProcess, Signal};

const LOGIN_CONFIG_PREFIX: &str = "tailscale-login-";
const LOGIN_CONFIG_SUFFIX: &str = ".json";

/// Rejects an admission that would otherwise share a state directory with a live leftover.
pub(super) const STALE_LOGIN_CORE_ALIVE: &str = "staleLoginCoreAlive";

/// SIGTERM → grace → SIGKILL window for a leftover login core.
const STALE_LOGIN_KILL_GRACE: Duration = Duration::from_millis(1_500);

/// `tailscale-login-<sanitized id>-<epoch>.json` → `(id, epoch)`. Any other name is not ours.
pub(super) fn login_config_identity(name: &str) -> Option<(&str, u64)> {
    let stem = name
        .strip_prefix(LOGIN_CONFIG_PREFIX)?
        .strip_suffix(LOGIN_CONFIG_SUFFIX)?;
    let (id, epoch) = stem.rsplit_once('-')?;
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        || epoch.is_empty()
        || !epoch.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some((id, epoch.parse().ok()?))
}

/// File names in the config directory that are login configs of no registered login.
pub(super) fn stale_login_config_names<'a>(
    names: impl IntoIterator<Item = &'a str>,
    dir: &Path,
    live: &[PathBuf],
) -> Vec<&'a str> {
    names
        .into_iter()
        .filter(|name| login_config_identity(name).is_some())
        .filter(|name| !live.iter().any(|path| *path == dir.join(name)))
        .collect()
}

/// Remove leftover login configs. The caller holds the state gate, so a config written by a
/// login that is not yet registered cannot exist. Returns how many files were removed.
pub(super) fn sweep_stale_login_configs(dir: &Path, live: &[PathBuf]) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::debug!(target: super::LOGIN_CORE_LOG_TARGET, "登录临时配置清扫：配置目录不可读，未执行");
        return 0;
    };
    let names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    let mut removed = 0;
    for name in stale_login_config_names(names.iter().map(String::as_str), dir, live) {
        // `remove_file` unlinks a symlink itself and refuses a directory.
        match std::fs::remove_file(dir.join(name)) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => log::warn!(
                target: super::LOGIN_CORE_LOG_TARGET,
                "登录临时配置清扫：一份遗留文件未能删除（{:?}）",
                error.kind()
            ),
        }
    }
    if removed == 0 {
        log::debug!(target: super::LOGIN_CORE_LOG_TARGET, "登录临时配置清扫：无遗留文件");
    } else {
        log::info!(
            target: super::LOGIN_CORE_LOG_TARGET,
            "登录临时配置清扫：删除 {removed} 份上次遗留的文件"
        );
    }
    removed
}

/// Whether this process is a login core this app spawned: argv is exactly
/// `<our binary> run -c <config dir>/tailscale-login-<id>-<epoch>.json ...`.
pub(super) fn is_stale_login_core(process: &CoreProcess, binary: &Path, config_dir: &Path) -> bool {
    if let [program, run, flag, config, ..] = process.cmdline.as_slice() {
        let config = Path::new(config);
        return Path::new(program) == binary
            && run == "run"
            && flag == "-c"
            && config.parent() == Some(config_dir)
            && config
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| login_config_identity(name).is_some());
    }
    // `ps` joins argv with spaces and both paths may contain spaces, so match the exact prefix.
    // A raw line only ever comes from macOS `ps`, so the separator after the config directory
    // is that line's `/`, not whatever the compiling host uses.
    let prefix = format!("{} run -c {}", binary.display(), config_dir.display());
    let Some(rest) = process
        .raw
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_prefix('/'))
    else {
        return false;
    };
    let Some(end) = rest.find(LOGIN_CONFIG_SUFFIX) else {
        return false;
    };
    let (name, tail) = rest.split_at(end + LOGIN_CONFIG_SUFFIX.len());
    login_config_identity(name).is_some() && (tail.is_empty() || tail.starts_with(' '))
}

/// Pids of leftover login cores, never one of this registry's own registered children.
pub(super) fn stale_login_core_pids(
    candidates: &[CoreProcess],
    binary: &Path,
    config_dir: &Path,
    inflight: &[u32],
) -> Vec<u32> {
    candidates
        .iter()
        .filter(|process| is_stale_login_core(process, binary, config_dir))
        .map(|process| process.pid)
        .filter(|pid| !inflight.contains(pid))
        .collect()
}

/// A leftover is this user's own process. One that another user started is neither ended nor
/// a reason to refuse; an owner that cannot be read is treated the same way.
pub(super) fn owned_by_this_user(owner: Option<u32>, me: Option<u32>) -> bool {
    matches!((owner, me), (Some(owner), Some(me)) if owner == me)
}

/// Process table access for the sweep; fakes replace all three in tests.
pub(super) struct StaleSweepIo<'a> {
    pub(super) scan: &'a (dyn Fn() -> Vec<CoreProcess> + Send + Sync),
    pub(super) owner: &'a (dyn Fn(u32) -> Option<u32> + Send + Sync),
    pub(super) signal: &'a (dyn Fn(u32, Signal) + Send + Sync),
}

/// Terminate this user's leftover login cores and confirm they are gone. A pid is signalled
/// again only while a fresh scan still shows the same exact argv and owner under it.
pub(super) async fn sweep_stale_login_cores(
    io: &StaleSweepIo<'_>,
    me: Option<u32>,
    grace: Duration,
    binary: &Path,
    config_dir: &Path,
    inflight: &[u32],
) -> Result<usize, String> {
    let (scan, signal) = (io.scan, io.signal);
    let select = || -> Vec<u32> {
        stale_login_core_pids(&scan(), binary, config_dir, inflight)
            .into_iter()
            .filter(|pid| owned_by_this_user((io.owner)(*pid), me))
            .collect()
    };
    let victims = select();
    if victims.is_empty() {
        return Ok(0);
    }
    let remaining = || -> Vec<u32> {
        select()
            .into_iter()
            .filter(|pid| victims.contains(pid))
            .collect()
    };
    for pid in &victims {
        signal(*pid, Signal::Sigterm);
    }
    tokio::time::sleep(grace).await;
    let stubborn = remaining();
    if stubborn.is_empty() {
        return Ok(victims.len());
    }
    for pid in &stubborn {
        signal(*pid, Signal::Sigkill);
    }
    tokio::time::sleep(grace).await;
    if remaining().is_empty() {
        Ok(victims.len())
    } else {
        Err(STALE_LOGIN_CORE_ALIVE.into())
    }
}

/// Runs before a login core or a logout is admitted, under the registry's state gate.
#[async_trait]
pub trait StaleLoginCoreSweeper: Send + Sync {
    /// `inflight` are this registry's registered children. Returns how many leftovers ended.
    async fn sweep(&self, binary: &Path, inflight: &[u32]) -> Result<usize, String>;
}

/// Production sweeper. The process scan is empty on platforms without one, so nothing is
/// signalled there and a leftover login core survives until the user ends it.
pub struct ProcessStaleLoginSweeper {
    config_dir: PathBuf,
}

impl ProcessStaleLoginSweeper {
    #[must_use]
    pub fn new(config_dir: PathBuf) -> Self {
        Self { config_dir }
    }
}

#[async_trait]
impl StaleLoginCoreSweeper for ProcessStaleLoginSweeper {
    async fn sweep(&self, binary: &Path, inflight: &[u32]) -> Result<usize, String> {
        #[cfg(unix)]
        let signal = crate::runtime::proxy::send_signal;
        #[cfg(not(unix))]
        let signal = |_: u32, _: Signal| {};
        sweep_stale_login_cores(
            &StaleSweepIo {
                scan: &polaris_core_supervisor::scan_running_cores,
                owner: &polaris_core_supervisor::process_owner_uid,
                signal: &signal,
            },
            polaris_core_supervisor::process_owner_uid(std::process::id()),
            STALE_LOGIN_KILL_GRACE,
            binary,
            &self.config_dir,
            inflight,
        )
        .await
    }
}
