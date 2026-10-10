//! Static reachability complements behavioral fixture tests; it is not native acceptance.
use polaris_source_probe::{crate_source_in, mask_comments_and_strings};
fn source(path: &str) -> String {
    crate_source_in(env!("CARGO_MANIFEST_DIR"), path)
}
fn compact(source: &str) -> String {
    mask_comments_and_strings(source)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect()
}
fn body(source: &str, marker: &str) -> String {
    let source = mask_comments_and_strings(source);
    let start = source.find(marker).expect("production hook anchor missing");
    let first = start + source[start..].find('{').unwrap();
    let mut depth = 0;
    for (offset, c) in source[first..].char_indices() {
        if c == '{' {
            depth += 1
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                return compact(&source[first..first + offset + 1]);
            }
        }
    }
    panic!("production hook body unclosed")
}
#[test]
fn original_sources_invalidate_before_debounce() {
    let monitor = source("runtime/proxy/network_monitor.rs");
    let windows = body(&monitor, "async fn network_watcher_once(");
    let unix = body(&monitor, "async fn route_network_watcher_once(");
    assert!(windows.contains("Some(())=>{self.coex_network_receipt(generation,token);deadline="));
    assert!(unix
        .contains("ifupdate.observed_event{self.coex_network_receipt(generation,token);deadline="));
    assert!(windows.contains("self.coex.watcher(generation,token,true)"));
    assert!(unix.contains("self.coex.watcher(generation,token,true)"));
    let loop_body = body(&monitor, "async fn network_watcher_loop(");
    assert!(loop_body.contains("self.coex.watcher(generation,token,false)"));
    let original_handler = body(&monitor, "async fn handle_network_change(");
    assert!(!original_handler.contains("coex"));
}
#[test]
fn successful_claims_ready_failure_and_crash_hooks_remain_reachable() {
    let lifecycle = source("runtime/proxy/lifecycle.rs");
    let claim = body(&lifecycle, "fn claim_generation(");
    assert!(claim.contains(
        "ifletSome(generation)=generation{self.coex.claim(generation,kind==LifecycleKind::Start)"
    ));
    let start = body(&lifecycle, "async fn start_guarded_with_completion(");
    assert!(start.contains("publish_committed_ready_main(my_gen,||{"));
    assert!(start.contains("self.coex_committed_ready(my_gen)"));
    assert!(start.contains("self.coex.fail_start(my_gen)"));
    assert!(start.contains("StartAttempt::new("));
    assert!(start.contains("coex_attempt.generation.set(my_gen)"));
    let crash = body(
        &source("runtime/proxy/recovery.rs"),
        "async fn reset_crashed_run_state(",
    );
    assert!(
        crash.find("self.coex.terminal(my_gen,true)").unwrap()
            < crash
                .find("self.mesh.exit_route_reset_state().await")
                .unwrap()
    );
}
#[test]
fn all_applied_config_commits_use_one_binding_boundary() {
    let startup = compact(&source("runtime/proxy/startup.rs"));
    let hot = compact(&source("runtime/proxy/hot_switch.rs"));
    assert_eq!(
        startup.matches("self.commit_coex_current_config(").count(),
        1
    );
    assert_eq!(hot.matches("self.commit_coex_current_config(").count(), 4);
    assert!(!startup.contains("current_config.write()"));
    assert!(!hot.contains("current_config.write()"));
    let facade = source("runtime/proxy.rs");
    let commit = body(&facade, "fn commit_coex_current_config(");
    assert!(commit.contains("self.current_config.write()"));
    assert!(commit.contains("self.coex.commit_config(&mutcurrent,value)"));
    assert!(commit.contains("self.coex.config_unavailable()"));
}
#[test]
fn shared_service_and_main_only_state_route_are_registered() {
    let worker = source("commands/coexistence_snapshot.rs");
    assert_eq!(compact(&worker).matches("staticSERVICE:").count(), 1);
    assert!(body(&worker, "async fn coex_readonly_snapshot(").contains("shared_service()"));
    let lib = compact(&source("lib.rs"));
    assert!(lib.contains("coex_runtime_get_state,"));
    assert!(lib.contains("coex_runtime_refresh,"));
    for name in ["coex_runtime_get_state(", "coex_runtime_refresh("] {
        let command = body(&worker, name);
        assert!(command.contains("!allowed_window(window.label())"));
    }
    let facade = body(
        &source("runtime/proxy.rs"),
        "fn emit_coex_state(&self, state:",
    );
    assert!(facade.contains("emit_to_main("));
    assert!(!facade.contains("broadcast("));
    let observer = compact(&source("runtime/proxy/coex_observer.rs"));
    assert!(observer.contains("service:snapshot::shared_service()"));
    for forbidden in [
        "handle_network_change(",
        "reconcile_system_dns",
        "schedule_restart(",
        "classify(",
        "NativeWindowsSource",
    ] {
        assert!(!observer.contains(forbidden), "{forbidden}");
    }
    let run = body(&source("runtime/proxy/coex_observer.rs"), "fn run(&self)");
    assert!(run.find("end_binding").unwrap() < run.find("admission.finish()").unwrap());
}
