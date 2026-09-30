//! 进程级正常退出协调。所有可控桌面退出先关闭 Start admission，再确认本地 owner 全量收口。
//! 准备失败保留 custody 与运行时；只有持有 Ready 才能提交退出、重启或 detached 安装。
//! 崩溃、强杀及平台直接终止不在此门的保证范围内。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tauri::Manager;
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::runtime::AppRuntime;
use crate::startup::{LightweightState, QuitState, RestartState};

const EXIT_PREPARE_BUDGET: Duration = Duration::from_secs(15);

type RuntimeIdentity = [usize; 2];

#[derive(Default)]
enum ExitPhase {
    #[default]
    Pending,
    Ready(RuntimeIdentity),
    Committed,
}

#[derive(Default)]
pub(crate) struct ExitCleanupState {
    phase: Arc<Mutex<ExitPhase>>,
    committed: Arc<AtomicBool>,
}

/// 私有构造、绑定实际 runtime，且从准备成功到外部动作完成始终持有协调锁。
/// 安装脚本 spawn 失败时 drop 只释放锁，Ready 与永久关闭的 admission 仍保留供重试。
pub(crate) struct DesktopExitReady {
    phase: OwnedMutexGuard<ExitPhase>,
    committed: Arc<AtomicBool>,
    runtime: RuntimeIdentity,
}

#[async_trait]
trait ExitPorts: Sync {
    fn identity(&self) -> RuntimeIdentity;
    fn fence_main(&self) -> Result<(), String>;
    fn fence_login(&self) -> Result<(), String>;
    fn fence_temp(&self) -> Result<(), String>;
    fn fence_check(&self) -> Result<(), String>;
    async fn drain_main(&self) -> Result<(), String>;
    async fn drain_login(&self) -> Result<(), String>;
    async fn drain_temp(&self) -> Result<(), String>;
    async fn drain_check(&self) -> Result<(), String>;
}

fn fence_all(ports: &impl ExitPorts) -> Result<(), String> {
    // 即使第一条失败，也必须尝试关闭其它 admission；这里没有任何 await。
    let results = [
        ports.fence_main(),
        ports.fence_login(),
        ports.fence_temp(),
        ports.fence_check(),
    ];
    collect_errors(results)
}

fn collect_errors<const N: usize>(results: [Result<(), String>; N]) -> Result<(), String> {
    let errors: Vec<_> = results.into_iter().filter_map(Result::err).collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

impl ExitCleanupState {
    async fn prepare(
        &self,
        ports: &impl ExitPorts,
        budget: Duration,
    ) -> Result<DesktopExitReady, String> {
        fence_all(ports)?;
        let runtime = ports.identity();
        tokio::time::timeout(budget, async {
            let mut phase = self.phase.clone().lock_owned().await;
            match &*phase {
                ExitPhase::Committed => return Err("退出已提交".into()),
                ExitPhase::Ready(ready_runtime) if *ready_runtime != runtime => {
                    return Err("退出凭据与当前运行时不匹配".into());
                }
                ExitPhase::Ready(_) => {}
                ExitPhase::Pending => {
                    // 每个 owner 都得到 drain 机会；任何 Pending/Unknown/Err 均不能产 Ready。
                    let (main, login, temp, check) = tokio::join!(
                        ports.drain_main(),
                        ports.drain_login(),
                        ports.drain_temp(),
                        ports.drain_check()
                    );
                    collect_errors([main, login, temp, check])?;
                    *phase = ExitPhase::Ready(runtime);
                }
            }
            Ok(DesktopExitReady {
                phase,
                committed: self.committed.clone(),
                runtime,
            })
        })
        .await
        .map_err(|_| "等待本地内核关闭超时".to_string())?
    }
}

impl DesktopExitReady {
    fn commit(mut self, runtime: RuntimeIdentity, commit: impl FnOnce()) -> Result<(), String> {
        if runtime != self.runtime
            || !matches!(&*self.phase, ExitPhase::Ready(id) if *id == runtime)
        {
            return Err("退出凭据失效".into());
        }
        *self.phase = ExitPhase::Committed;
        // AppHandle.exit 可能同步进入事件回调；先公布已提交，使兜底不会再次排队。
        self.committed.store(true, Ordering::SeqCst);
        commit();
        Ok(())
    }
}

#[cfg(not(target_os = "android"))]
struct DesktopExitPorts {
    proxy: Arc<crate::runtime::proxy::ProxyRuntime>,
    mesh: Arc<crate::runtime::mesh::MeshRuntime>,
}

#[cfg(not(target_os = "android"))]
impl DesktopExitPorts {
    fn from_runtime(runtime: &AppRuntime) -> Self {
        Self {
            proxy: runtime.proxy.clone(),
            mesh: runtime.mesh.clone(),
        }
    }
}

#[cfg(not(target_os = "android"))]
#[async_trait]
impl ExitPorts for DesktopExitPorts {
    fn identity(&self) -> RuntimeIdentity {
        [
            Arc::as_ptr(&self.proxy) as usize,
            Arc::as_ptr(&self.mesh) as usize,
        ]
    }

    fn fence_main(&self) -> Result<(), String> {
        self.proxy.begin_shutdown()
    }

    fn fence_login(&self) -> Result<(), String> {
        self.mesh.begin_shutdown();
        Ok(())
    }

    fn fence_temp(&self) -> Result<(), String> {
        crate::runtime::speedtest::begin_shutdown();
        Ok(())
    }

    fn fence_check(&self) -> Result<(), String> {
        polaris_core_supervisor::begin_check_shutdown().map_err(|error| error.to_string())
    }

    async fn drain_main(&self) -> Result<(), String> {
        self.proxy.shutdown_for_exit().await
    }

    async fn drain_login(&self) -> Result<(), String> {
        self.mesh.shutdown_for_exit().await
    }

    async fn drain_temp(&self) -> Result<(), String> {
        crate::runtime::speedtest::shutdown_for_exit().await
    }

    async fn drain_check(&self) -> Result<(), String> {
        polaris_core_supervisor::shutdown_checks_for_exit()
            .await
            .map_err(|error| error.to_string())
    }
}

/// 唯一桌面退出准备门；没有装配实际 runtime 不代表 NoOwner，须明确失败。
pub(crate) async fn prepare_desktop_exit(
    app: &tauri::AppHandle,
) -> Result<DesktopExitReady, String> {
    #[cfg(not(target_os = "android"))]
    {
        let Some(runtime) = app.try_state::<AppRuntime>() else {
            crate::runtime::speedtest::begin_shutdown();
            let _ = polaris_core_supervisor::begin_check_shutdown();
            return Err("运行时尚未装配，无法确认本地内核已关闭".into());
        };
        let ports = DesktopExitPorts::from_runtime(&runtime);
        let Some(state) = app.try_state::<ExitCleanupState>() else {
            fence_all(&ports)?;
            return Err("退出协调尚未装配，无法确认本地内核已关闭".into());
        };
        state.prepare(&ports, EXIT_PREPARE_BUDGET).await
    }
    #[cfg(target_os = "android")]
    {
        let _ = app;
        Err("桌面退出准备不适用于 Android".into())
    }
}

pub(crate) enum ExitKind {
    Quit,
    Restart,
    Final,
}

/// 不可逆业务关闭、RestartState 消费及 clean marker 全部属于实际提交腿。
pub(crate) fn commit_desktop_exit(
    app: &tauri::AppHandle,
    ready: DesktopExitReady,
    kind: ExitKind,
) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        let runtime = app
            .try_state::<AppRuntime>()
            .ok_or_else(|| "运行时已不可用，拒绝提交退出".to_string())?;
        let identity = DesktopExitPorts::from_runtime(&runtime).identity();
        ready.commit(identity, || {
            app.state::<QuitState>().0.store(true, Ordering::SeqCst);
            app.state::<RestartState>()
                .0
                .store(matches!(kind, ExitKind::Restart), Ordering::SeqCst);
            runtime.subscription_create().shutdown_begin();
            runtime.subscription_parse().shutdown();
            runtime.subscription_create().shutdown_wait();
            mark_clean_exit(app);
            match kind {
                ExitKind::Quit => app.exit(0),
                ExitKind::Restart => app.request_restart(),
                ExitKind::Final => {}
            }
        })
    }
    #[cfg(target_os = "android")]
    {
        let _ = (app, ready, kind);
        Err("桌面退出提交不适用于 Android".into())
    }
}

pub(crate) fn exit_is_committed(app: &tauri::AppHandle) -> bool {
    app.try_state::<ExitCleanupState>()
        .is_some_and(|state| state.committed.load(Ordering::SeqCst))
}

pub(crate) async fn request_quit(app: &tauri::AppHandle) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        let ready = prepare_desktop_exit(app).await?;
        commit_desktop_exit(app, ready, ExitKind::Quit)
    }
    #[cfg(target_os = "android")]
    {
        app.state::<QuitState>().0.store(true, Ordering::SeqCst);
        app.exit(0);
        Ok(())
    }
}

/// 菜单与原生窗口没有 IPC 回包表面，失败须保留窗口并给出可见原因和重试动作。
pub(crate) fn queue_quit(app: &tauri::AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = request_quit(&app).await {
            if exit_is_committed(&app) {
                return; // 并发的另一个退出请求已经成功提交。
            }
            log::error!("退出被阻止: {error}");
            crate::show_main_window(&app);
            use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
            app.dialog()
                .message(crate::i18n::t(
                    crate::i18n::app_lang(&app),
                    crate::i18n::key::NATIVE_EXIT_BLOCKED,
                ))
                .title("Polaris")
                .kind(MessageDialogKind::Error)
                .show(|_| {});
        }
    });
}

/// C16 轻量驻留豁免必须先于准备门；消费陈旧转场位不影响显式退出。
pub(crate) enum ExitRequestedAction {
    PreserveLightweight,
    Exit,
}

pub(crate) fn exit_requested_action(app: &tauri::AppHandle) -> ExitRequestedAction {
    let lightweight = app
        .state::<LightweightState>()
        .0
        .swap(false, Ordering::SeqCst);
    let quitting = app.state::<QuitState>().0.load(Ordering::SeqCst);
    if lightweight && !quitting && crate::tray::tray_present(app) {
        ExitRequestedAction::PreserveLightweight
    } else {
        ExitRequestedAction::Exit
    }
}

/// 最终 Exit 无法 veto。仅作同门 best effort；Unknown/超时/错误不写正常退出标记。
pub(crate) fn final_exit_best_effort(app: &tauri::AppHandle) {
    #[cfg(not(target_os = "android"))]
    {
        if exit_is_committed(app) {
            return;
        }
        tauri::async_runtime::block_on(async {
            match prepare_desktop_exit(app).await {
                Ok(ready) => {
                    if let Err(error) = commit_desktop_exit(app, ready, ExitKind::Final) {
                        log::error!("平台已终止应用，退出提交未确认: {error}");
                    }
                }
                Err(error) => log::error!("平台已终止应用，本地内核关闭未确认: {error}"),
            }
        });
    }
    #[cfg(target_os = "android")]
    run_android_exit_once(app);
}

/// Android 沿用原退出腿，本批桌面 admission gate 不改变移动端生命周期。
#[cfg(target_os = "android")]
pub(crate) fn run_android_exit_once(app: &tauri::AppHandle) {
    if app
        .state::<ExitCleanupState>()
        .committed
        .swap(true, Ordering::SeqCst)
    {
        return;
    }
    if let Some(runtime) = app.try_state::<AppRuntime>() {
        runtime.subscription_create().shutdown_begin();
        runtime.subscription_parse().shutdown();
        runtime.subscription_create().shutdown_wait();
    }
    mark_clean_exit(app);
    let killed = crate::runtime::speedtest::kill_inflight_temp_cores();
    if killed > 0 {
        log::warn!("退出清理：强杀了 {killed} 个在飞测速临时核");
    }
    if let Some(runtime) = app.try_state::<AppRuntime>() {
        let proxy = runtime.proxy.clone();
        tauri::async_runtime::block_on(async move {
            if let Err(error) = proxy.stop().await {
                log::error!("退出清理：停核失败（不阻断 Android 退出）: {error}");
            }
        });
    }
}

fn mark_clean_exit(app: &tauri::AppHandle) {
    let restarting = app
        .try_state::<RestartState>()
        .is_some_and(|s| s.0.swap(false, Ordering::SeqCst));
    if let Some(runtime) = app.try_state::<AppRuntime>() {
        crate::clean_exit::mark_unless_restarting(runtime.config.dir(), restarting);
    }
}

#[cfg(test)]
mod tests;
