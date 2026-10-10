use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;

use super::*;
use crate::commands::guard_scan::top_level_fn_body;
use crate::test_support::{crate_code, crate_root_code};

struct FakePorts {
    admission_closed: [AtomicBool; 4],
    custody: [AtomicUsize; 4],
    results: StdMutex<[Result<(), String>; 4]>,
    pending: AtomicBool,
    trace: StdMutex<Vec<&'static str>>,
}

impl FakePorts {
    fn new() -> Self {
        Self {
            admission_closed: std::array::from_fn(|_| AtomicBool::new(false)),
            custody: std::array::from_fn(|_| AtomicUsize::new(1)),
            results: StdMutex::new(std::array::from_fn(|_| Ok(()))),
            pending: AtomicBool::new(false),
            trace: StdMutex::new(Vec::new()),
        }
    }

    fn fence(&self, index: usize, event: &'static str) -> Result<(), String> {
        self.admission_closed[index].store(true, Ordering::SeqCst);
        self.trace.lock().unwrap().push(event);
        Ok(())
    }

    async fn drain(&self, index: usize, event: &'static str) -> Result<(), String> {
        assert!(self
            .admission_closed
            .iter()
            .all(|a| a.load(Ordering::SeqCst)));
        self.trace.lock().unwrap().push(event);
        if self.pending.load(Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        let result = self.results.lock().unwrap()[index].clone();
        if result.is_ok() {
            self.custody[index].store(0, Ordering::SeqCst);
        }
        result
    }
}

#[async_trait]
impl ExitPorts for FakePorts {
    fn identity(&self) -> RuntimeIdentity {
        [1, 2]
    }

    fn fence_main(&self) -> Result<(), String> {
        self.fence(0, "main-fence")
    }

    fn fence_login(&self) -> Result<(), String> {
        self.fence(1, "login-fence")
    }

    fn fence_temp(&self) -> Result<(), String> {
        self.fence(2, "temp-fence")
    }

    fn fence_check(&self) -> Result<(), String> {
        self.fence(3, "check-fence")
    }

    async fn drain_check(&self) -> Result<(), String> {
        self.drain(3, "check-drain").await
    }

    async fn drain_main(&self) -> Result<(), String> {
        self.drain(0, "main-drain").await
    }

    async fn drain_login(&self) -> Result<(), String> {
        self.drain(1, "login-drain").await
    }

    async fn drain_temp(&self) -> Result<(), String> {
        self.drain(2, "temp-drain").await
    }
}

#[tokio::test]
async fn every_admission_is_fenced_before_any_drain_and_prepare_has_no_commit_effects() {
    let state = ExitCleanupState::default();
    let ports = FakePorts::new();
    let ready = state.prepare(&ports, Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        &ports.trace.lock().unwrap()[..4],
        &["main-fence", "login-fence", "temp-fence", "check-fence"]
    );
    assert!(!state.committed.load(Ordering::SeqCst));
    assert!(ports.custody.iter().all(|c| c.load(Ordering::SeqCst) == 0));
    drop(ready);
    assert!(!state.committed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn any_unknown_or_error_preserves_custody_and_allows_a_later_successful_retry() {
    for failed_owner in 0..4 {
        let state = ExitCleanupState::default();
        let ports = FakePorts::new();
        ports.results.lock().unwrap()[failed_owner] = Err("same birth close unknown".into());
        let effects = AtomicUsize::new(0);
        assert!(state.prepare(&ports, Duration::from_secs(1)).await.is_err());
        assert_eq!(ports.custody[failed_owner].load(Ordering::SeqCst), 1);
        assert!(!state.committed.load(Ordering::SeqCst));
        assert_eq!(effects.load(Ordering::SeqCst), 0);
        ports.results.lock().unwrap()[failed_owner] = Ok(());
        let ready = state.prepare(&ports, Duration::from_secs(1)).await.unwrap();
        ready
            .commit(ports.identity(), || {
                effects.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        assert_eq!(effects.load(Ordering::SeqCst), 1);
        assert_eq!(ports.custody[failed_owner].load(Ordering::SeqCst), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn pending_or_timeout_never_produces_ready_or_retires_custody() {
    let state = ExitCleanupState::default();
    let ports = FakePorts::new();
    ports.pending.store(true, Ordering::SeqCst);
    let error = state
        .prepare(&ports, Duration::from_millis(50))
        .await
        .err()
        .unwrap();
    assert!(error.contains("超时"));
    assert!(ports.custody.iter().all(|c| c.load(Ordering::SeqCst) == 1));
    assert!(!state.committed.load(Ordering::SeqCst));
    ports.pending.store(false, Ordering::SeqCst);
    assert!(state.prepare(&ports, Duration::from_secs(1)).await.is_ok());
}

#[tokio::test]
async fn detached_spawn_failure_keeps_ready_and_admissions_closed_for_retry() {
    let state = ExitCleanupState::default();
    let ports = FakePorts::new();
    let ready = state.prepare(&ports, Duration::from_secs(1)).await.unwrap();
    let drains = ports.trace.lock().unwrap().len();
    // The detached script failed before commit; dropping its held Ready is the production path.
    drop(ready);
    assert!(!state.committed.load(Ordering::SeqCst));
    assert!(ports
        .admission_closed
        .iter()
        .all(|a| a.load(Ordering::SeqCst)));
    let retry = state.prepare(&ports, Duration::from_secs(1)).await.unwrap();
    assert_eq!(ports.trace.lock().unwrap().len(), drains + 4);
    retry.commit(ports.identity(), || {}).unwrap();
    assert!(state.committed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn runtime_bound_ready_rejects_another_runtime_without_commit_effects() {
    let state = ExitCleanupState::default();
    let ports = FakePorts::new();
    let ready = state.prepare(&ports, Duration::from_secs(1)).await.unwrap();
    let effects = AtomicUsize::new(0);
    assert!(ready
        .commit([7, 8], || {
            effects.fetch_add(1, Ordering::SeqCst);
        })
        .is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert!(!state.committed.load(Ordering::SeqCst));
}

#[tokio::test]
async fn concurrent_exit_requests_commit_once_and_cannot_spawn_two_installers() {
    let state = ExitCleanupState::default();
    let ports = FakePorts::new();
    let effects = AtomicUsize::new(0);
    let request = || async {
        let ready = state.prepare(&ports, Duration::from_secs(1)).await?;
        // Actual exit / restart / detached spawn effects must occur while this Ready owns the lock.
        ready.commit(ports.identity(), || {
            effects.fetch_add(1, Ordering::SeqCst);
        })
    };
    let (first, second) = tokio::join!(request(), request());
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[test]
fn exit_requested_preserves_c16_then_vetoes_unprepared_exit_without_business_shutdown() {
    let main = top_level_fn_body(&crate_root_code(), "pub fn run() {");
    let start = main
        .find("tauri::RunEvent::ExitRequested { api, .. } => {")
        .unwrap();
    let end = main.find("tauri::RunEvent::Exit => {").unwrap();
    let leg = &main[start..end];
    let c16 = leg
        .find("ExitRequestedAction::PreserveLightweight")
        .unwrap();
    let c16_return = leg.find("return;").unwrap();
    let committed = leg
        .find("!exit_lifecycle::exit_is_committed(app_handle)")
        .unwrap();
    let veto = leg[committed..].find("api.prevent_exit();").unwrap() + committed;
    let prepare = leg.find("exit_lifecycle::queue_quit(app_handle);").unwrap();
    assert!(c16 < c16_return && c16_return < committed && committed < veto && veto < prepare);
    assert!(!leg.contains("shutdown_begin") && !leg.contains("mark_clean_exit"));
    assert!(main[end..].contains("exit_lifecycle::final_exit_best_effort(app_handle);"));
}

#[test]
fn final_platform_exit_fences_dns_before_drain_but_normal_quit_keeps_its_flush() {
    let source = crate_code("exit_lifecycle.rs");
    let final_exit = top_level_fn_body(&source, "pub(crate) fn final_exit_best_effort(");
    let committed = final_exit.find("if exit_is_committed(app)").unwrap();
    let early_return = final_exit[committed..].find("return;").unwrap() + committed;
    let fence = final_exit
        .find("windows_session::begin_final_exit()")
        .unwrap();
    let drain = final_exit.find("prepare_desktop_exit(app)").unwrap();
    assert!(committed < early_return && early_return < fence && fence < drain);
    let prepare = top_level_fn_body(&source, "pub(crate) async fn prepare_desktop_exit(");
    assert!(!prepare.contains("begin_final_exit"));
}

#[test]
fn only_ready_commit_owns_irreversible_shutdown_marker_and_restart_request() {
    let source = crate_code("exit_lifecycle.rs");
    let prepare = top_level_fn_body(&source, "pub(crate) async fn prepare_desktop_exit(");
    assert!(!prepare.contains("mark_clean_exit") && !prepare.contains("shutdown_begin"));
    let commit = top_level_fn_body(&source, "pub(crate) fn commit_desktop_exit(");
    let ready = commit.find("ready.commit(identity, || {").unwrap();
    let begin = commit
        .find("subscription_create().shutdown_begin()")
        .unwrap();
    let parser = commit.find("subscription_parse().shutdown()").unwrap();
    let wait = commit
        .find("subscription_create().shutdown_wait()")
        .unwrap();
    let mark = commit.find("mark_clean_exit(app);").unwrap();
    let restart = commit.find("app.request_restart()").unwrap();
    assert!(ready < begin && begin < parser && parser < wait && wait < mark && mark < restart);
    assert!(commit.find("RestartState").unwrap() < mark);
    let mark = top_level_fn_body(&source, "fn mark_clean_exit(");
    assert!(mark.contains("RestartState") && mark.contains("mark_unless_restarting"));
}

#[test]
fn restart_and_all_quit_entrypoints_consume_the_shared_prepare_gate() {
    let restart = top_level_fn_body(
        &crate_code("commands/window.rs"),
        "pub async fn app_restart(",
    );
    assert!(
        restart.find("prepare_desktop_exit(&app).await").unwrap()
            < restart.find("commit_desktop_exit(").unwrap()
    );
    assert!(restart.contains("ExitKind::Restart"));
    let request = top_level_fn_body(
        &crate_code("exit_lifecycle.rs"),
        "pub(crate) async fn request_quit(",
    );
    assert!(
        request.find("prepare_desktop_exit(app).await?").unwrap()
            < request
                .find("commit_desktop_exit(app, ready, ExitKind::Quit)")
                .unwrap()
    );
    let tray = top_level_fn_body(&crate_code("tray/commands.rs"), "pub async fn tray_quit(");
    assert!(tray.contains("request_quit(&app).await") && !tray.contains("app.exit("));
    let menu = top_level_fn_body(&crate_code("app_tray.rs"), "pub(crate) fn run_menu_action(");
    assert!(
        menu.contains("MenuAction::Quit => crate::exit_lifecycle::queue_quit(app)")
            && !menu.contains("app.exit(")
    );
    assert!(!crate_root_code().contains("app_handle.exit(0)"));
}

#[test]
fn ios_host_quit_and_restart_preserve_ne_without_desktop_owner_receipts() {
    fn cfg_body<'a>(source: &'a str, predicate: &str) -> &'a str {
        let at = source.find(predicate).unwrap();
        let open = at + source[at..].find('{').unwrap();
        let mut depth = 0;
        for (index, byte) in source.as_bytes().iter().enumerate().skip(open) {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &source[open + 1..index];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced cfg block");
    }
    let exit = crate_code("exit_lifecycle.rs");
    for signature in [
        "pub(crate) async fn request_quit(",
        "pub(crate) fn final_exit_best_effort(",
    ] {
        let body = top_level_fn_body(&exit, signature);
        let ios = cfg_body(&body, "#[cfg(target_os = \"ios\")]");
        for forbidden in [
            "prepare_desktop_exit",
            "commit_desktop_exit",
            "run_android_exit_once",
            "mark_clean_exit",
            "shutdown_for_exit",
            "proxy.stop",
        ] {
            assert!(
                !ios.contains(forbidden),
                "iOS host exit reached {forbidden}"
            );
        }
    }
    let restart = top_level_fn_body(
        &crate_code("commands/window.rs"),
        "pub async fn app_restart(",
    );
    let ios = cfg_body(&restart, "#[cfg(target_os = \"ios\")]");
    assert!(ios.contains("app.request_restart()"));
    assert!(!ios.contains("prepare_desktop_exit") && !ios.contains("commit_desktop_exit"));
    let desktop = cfg_body(
        &restart,
        "#[cfg(not(any(target_os = \"android\", target_os = \"ios\")))]",
    );
    assert!(
        desktop.find("prepare_desktop_exit").unwrap()
            < desktop.find("commit_desktop_exit").unwrap()
    );
    let run = crate_root_code();
    assert!(run.contains("#[cfg(not(any(target_os = \"android\", target_os = \"ios\")))]\n            if !exit_lifecycle::exit_is_committed"));
}

#[test]
fn updater_must_prepare_before_detached_spawn_with_no_running_shortcut() {
    let body = top_level_fn_body(
        &crate_code("commands/updater/app_update.rs"),
        "pub async fn update_install(",
    );
    let android = body
        .find("if plan.platform == update_install::InstallPlatform::Android")
        .unwrap();
    let prepare = body.find("prepare_desktop_exit(&app).await").unwrap();
    let spawn = body
        .find("update_install::spawn_detached_script(&dir, spec)")
        .unwrap();
    let commit = body.find("commit_desktop_exit(").unwrap();
    assert!(android < prepare && prepare < spawn && spawn < commit);
    assert!(!body.contains("proxy.status().running") && !body.contains("proxy.stop().await"));
    assert!(body[prepare..spawn].contains("return Ok(ApiResponse::err_with_code("));
}
