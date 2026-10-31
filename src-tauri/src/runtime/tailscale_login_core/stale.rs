//! Leftovers of a login core whose host process died: its secret-bearing temporary config and,
//! on desktop, the detached child itself. Both are identified only by this app's own exact
//! artifacts: the fixed config file name inside the app config directory, and a child that is
//! a core running exactly such a config. Nothing is matched by process name alone.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use polaris_core_supervisor::CoreProcess;
#[cfg(any(windows, test))]
use polaris_core_supervisor::ProcessOwner;
#[cfg(any(not(windows), test))]
use polaris_core_supervisor::Signal;

const LOGIN_CONFIG_PREFIX: &str = "tailscale-login-";
const LOGIN_CONFIG_SUFFIX: &str = ".json";

/// Rejects an admission that would otherwise share a state directory with a live leftover.
pub(super) const STALE_LOGIN_CORE_ALIVE: &str = "staleLoginCoreAlive";

/// Unix signal escalation / Windows same-handle termination retry window.
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

/// Remove leftover login configs after the process sweep admitted cleanup. The state gate
/// excludes this registry's unregistered writers; the sweep must also exclude old hosts'
/// matching writers, including those whose owner cannot be established.
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
        // An exact old config names its same-epoch cache companion. Remove the cache first;
        // no JSON-supplied path or wildcard can expand this cleanup into user TS state.
        // `remove_file` unlinks a symlink itself and refuses a directory.
        match super::remove_login_artifacts(&dir.join(name), true) {
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

/// Whether this process is a login core this app spawned: its command line is
/// `<any absolute path named like our core> run -c <config dir>/tailscale-login-<id>-<epoch>.json ...`.
///
/// Only the file name of `binary` is used. Where the core binary lives is not stable between
/// sessions (AppImage mount point, macOS translocation, reinstall), so a leftover from an
/// earlier session would never match a full-path comparison; the config directory is stable
/// and private to this user's app. The shape test is the main-core sweep's
/// ([`polaris_core_supervisor::app_run_config_name`]).
pub(super) fn is_stale_login_core(process: &CoreProcess, binary: &Path, config_dir: &Path) -> bool {
    let Some(core_filename) = binary.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    polaris_core_supervisor::app_run_config_name(process, core_filename, config_dir)
        .is_some_and(|name| login_config_identity(name).is_some())
}

/// Pids of leftover login cores, never one of this registry's own registered children.
#[cfg(any(not(windows), test))]
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

/// Only a proven same-user leftover may be signalled. A matching private config still in
/// use by any other or unreadable owner must instead block admission and artifact cleanup.
#[cfg(any(not(windows), test))]
pub(super) fn owned_by_this_user(owner: Option<u32>, me: Option<u32>) -> bool {
    matches!((owner, me), (Some(owner), Some(me)) if owner == me)
}

/// Process table access for the sweep; fakes replace all three in tests.
#[cfg(any(not(windows), test))]
pub(super) struct StaleSweepIo<'a> {
    pub(super) scan: &'a (dyn Fn() -> Vec<CoreProcess> + Send + Sync),
    pub(super) owner: &'a (dyn Fn(u32) -> Option<u32> + Send + Sync),
    pub(super) signal: &'a (dyn Fn(u32, Signal) + Send + Sync),
}

/// Terminate this user's leftover login cores and confirm they are gone. A pid is signalled
/// again only while a fresh scan still shows the same exact argv and owner under it.
#[cfg(any(not(windows), test))]
pub(super) async fn sweep_stale_login_cores(
    io: &StaleSweepIo<'_>,
    me: Option<u32>,
    grace: Duration,
    binary: &Path,
    config_dir: &Path,
    inflight: &[u32],
) -> Result<usize, String> {
    let (scan, signal) = (io.scan, io.signal);
    let select = || -> Result<Vec<u32>, String> {
        let candidates = stale_login_core_pids(&scan(), binary, config_dir, inflight);
        if candidates
            .iter()
            .any(|pid| !owned_by_this_user((io.owner)(*pid), me))
        {
            return Err(STALE_LOGIN_CORE_ALIVE.into());
        }
        Ok(candidates)
    };
    let victims = select()?;
    if victims.is_empty() {
        return Ok(0);
    }
    let remaining = || -> Result<Vec<u32>, String> {
        let candidates = select()?;
        // A new matching writer cannot be treated as an exited original or signalled as
        // one. It still prevents the following config/cache sweep from taking its files.
        if candidates.iter().any(|pid| !victims.contains(pid)) {
            return Err(STALE_LOGIN_CORE_ALIVE.into());
        }
        Ok(candidates)
    };
    for pid in &victims {
        signal(*pid, Signal::Sigterm);
    }
    tokio::time::sleep(grace).await;
    let stubborn = remaining()?;
    if stubborn.is_empty() {
        return Ok(victims.len());
    }
    for pid in &stubborn {
        signal(*pid, Signal::Sigkill);
    }
    tokio::time::sleep(grace).await;
    if remaining()?.is_empty() {
        Ok(victims.len())
    } else {
        Err(STALE_LOGIN_CORE_ALIVE.into())
    }
}

/// Windows keeps the scanned process object, not just its pid. Production callbacks operate
/// only on its original held handle; tests can model births without opening real processes.
#[cfg(any(windows, test))]
pub(super) struct WindowsStaleSweepIo<'a> {
    pub(super) scan: &'a (dyn Fn() -> Vec<CoreProcess> + Send + Sync),
    pub(super) end: &'a (dyn Fn(&CoreProcess) -> bool + Send + Sync),
    pub(super) running: &'a (dyn Fn(&CoreProcess) -> Option<bool> + Send + Sync),
}

/// End only exact login-shaped leftovers whose token SID was proven equal to ours. Unlike
/// the main-core sweep, SYSTEM is not admitted merely because it is elevated. An unreadable
/// owner is never termination authority. Matching non-targets must have an original-handle
/// exit observation before admission can proceed to delete their config/cache artifacts.
///
/// Targets are frozen once. Both termination attempts and the exit observations refer to
/// those original objects, even if a pid is later reused for the same command line. The
/// native end callback checks creation time again; its bool is never an exit receipt.
#[cfg(any(windows, test))]
pub(super) async fn sweep_windows_stale_login_cores(
    io: &WindowsStaleSweepIo<'_>,
    grace: Duration,
    binary: &Path,
    config_dir: &Path,
    inflight: &[u32],
) -> Result<usize, String> {
    let mut pending = Vec::new();
    let mut unknown = 0;
    let mut other = 0;
    let mut elevated = 0;
    let mut blocked = false;
    for process in (io.scan)() {
        if process.pid == 0
            || process.pid == std::process::id()
            || inflight.contains(&process.pid)
            || !is_stale_login_core(&process, binary, config_dir)
        {
            continue;
        }
        match process.owner {
            ProcessOwner::SameUser => {
                pending.push(process);
                continue;
            }
            ProcessOwner::Unknown => unknown += 1,
            ProcessOwner::Other => other += 1,
            ProcessOwner::Elevated => elevated += 1,
        }
        // Query the held original even for a non-target, but never request termination.
        // Missing custody or a live foreign owner is not evidence that its files are free.
        blocked |= (io.running)(&process) != Some(false);
    }
    // Non-target handles have already been dropped. Never print argv, SID or config secrets.
    if unknown + other + elevated > 0 {
        log::warn!(
            target: super::LOGIN_CORE_LOG_TARGET,
            "登录临时核清扫：不终止属主未知 {unknown}、其他用户 {other}、SYSTEM {elevated} 个同形进程；同私有配置需原句柄退出确认"
        );
    }
    if blocked {
        return Err(STALE_LOGIN_CORE_ALIVE.into());
    }
    let selected = pending.len();
    for _ in 0..2 {
        pending.retain(|process| match (io.running)(process) {
            Some(false) => false,
            Some(true) => {
                // Windows has no SIGTERM here. This requests termination of this one process,
                // with a second same-handle attempt after the grace window if still needed.
                let _ = (io.end)(process);
                true
            }
            // A missing handle is unknown custody, never permission to fall back to the pid.
            None => true,
        });
        if pending.is_empty() {
            return Ok(selected);
        }
        // No detached task: dropping this future cannot send a late retry outside the gate.
        tokio::time::sleep(grace).await;
    }
    if pending
        .iter()
        .all(|process| (io.running)(process) == Some(false))
    {
        Ok(selected)
    } else {
        log::warn!(
            target: super::LOGIN_CORE_LOG_TARGET,
            "登录临时核清扫：原进程尚未确认退出，拒绝本次准入"
        );
        Err(STALE_LOGIN_CORE_ALIVE.into())
    }
}

/// Runs before a login core or a logout is admitted, under the registry's state gate.
#[async_trait]
pub trait StaleLoginCoreSweeper: Send + Sync {
    /// `inflight` are this registry's registered children. Returns how many leftovers ended.
    /// Success excludes unretired exact private-config matches, including non-target owners;
    /// it is not global NoOwner for unrelated argv or directories.
    async fn sweep(&self, binary: &Path, inflight: &[u32]) -> Result<usize, String>;
}

/// Production sweeper. Windows consumes the scanner's SID owner and original process hold;
/// Unix retains its uid/signal path. An unreadable owner is never permission to end a process.
/// Mobile has no separate login-core processes to scan. None of these scans proves NoOwner.
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
        #[cfg(windows)]
        {
            sweep_windows_stale_login_cores(
                &WindowsStaleSweepIo {
                    scan: &polaris_core_supervisor::scan_running_cores,
                    end: &|process| process.hold.terminate(),
                    running: &|process| process.hold.is_running(),
                },
                STALE_LOGIN_KILL_GRACE,
                binary,
                &self.config_dir,
                inflight,
            )
            .await
        }
        #[cfg(not(windows))]
        {
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
}
