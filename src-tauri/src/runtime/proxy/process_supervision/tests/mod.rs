use super::{verify_writer_census, WriterNodeScope, WriterRunScope};

fn run(nonce: char, terminal: bool) -> WriterRunScope {
    WriterRunScope {
        nonce: nonce.to_string().repeat(64),
        digest: "d".repeat(64),
        terminal,
        complete: true,
        nodes: vec![WriterNodeScope {
            tag: "ts".into(),
            directory: "/state/tailscale/node".into(),
            file: "/state/tailscale/node/tailscaled.state".into(),
            terminal,
        }],
    }
}

#[test]
fn refreshed_original_accepts_reload_stop_while_start_only_census_rejects_it() {
    let original = vec![run('a', true), run('b', false)];
    let terminal = vec![run('a', true), run('b', true)];
    let expected = vec![(
        "ts".into(),
        "/state/tailscale/node".into(),
        "/state/tailscale/node/tailscaled.state".into(),
    )];
    assert!(verify_writer_census(&original, &terminal, &"d".repeat(64), &expected).is_ok());
    assert!(verify_writer_census(&original[..1], &terminal, &"d".repeat(64), &expected).is_err());
    assert!(verify_writer_census(&original, &terminal[1..], &"d".repeat(64), &expected).is_err());
    let mut unknown = terminal.clone();
    unknown[0].terminal = false;
    assert!(verify_writer_census(&original, &unknown, &"d".repeat(64), &expected).is_err());
}

#[test]
fn same_digest_physical_to_logical_history_keeps_each_nonce_scope() {
    let mut historical = run('a', true);
    historical.nodes[0].directory = "/state/tailscale-g4-v1/node".into();
    historical.nodes[0].file = "/state/tailscale-g4-v1/node/tailscaled.state".into();
    let original = vec![historical.clone(), run('b', false)];
    let terminal = vec![historical, run('b', true)];
    let expected = vec![(
        "ts".into(),
        "/state/tailscale/node".into(),
        "/state/tailscale/node/tailscaled.state".into(),
    )];
    assert!(verify_writer_census(&original, &terminal, &"d".repeat(64), &expected).is_ok());
    let mut rewritten = terminal.clone();
    rewritten[0].nodes = rewritten[1].nodes.clone();
    assert!(verify_writer_census(&original, &rewritten, &"d".repeat(64), &expected).is_err());
    let old_scope = vec![(
        "ts".into(),
        "/state/tailscale-g4-v1/node".into(),
        "/state/tailscale-g4-v1/node/tailscaled.state".into(),
    )];
    assert!(verify_writer_census(&original, &terminal, &"d".repeat(64), &old_scope).is_err());
    let mut missing = terminal.clone();
    missing.remove(0);
    assert!(verify_writer_census(&original, &missing, &"d".repeat(64), &expected).is_err());
}

#[test]
fn ios_managed_start_keeps_logical_bytes_off_shared_disk_and_fallback_revokes_first() {
    let startup = polaris_source_probe::crate_source_in(
        env!("CARGO_MANIFEST_DIR"),
        "runtime/proxy/startup.rs",
    );
    assert!(startup.contains("start_managed_prepared(&start)"));
    assert!(startup.contains(
        "#[cfg(not(target_os = \"ios\"))]\n            std::fs::write(config_path, &json)"
    ));
    let login = polaris_source_probe::crate_source_in(
        env!("CARGO_MANIFEST_DIR"),
        "runtime/tailscale_login_core.rs",
    );
    let launch = login.split("async fn launch_attempt(").nth(1).unwrap();
    assert!(
        launch
            .find("DirectoryLease::poison_domain(user_data)")
            .unwrap()
            < launch.find("let _start_guard").unwrap()
    );
}

#[test]
fn failed_cleanup_refinement_still_dispatches_and_retains_claims() {
    // Source wiring negative control; native tests exercise nonce, reload,
    // dispatch failure/retry and successor rejection. None are run here.
    let source = polaris_source_probe::crate_source_in(
        env!("CARGO_MANIFEST_DIR"),
        "runtime/proxy/process_supervision.rs",
    );
    let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
    let prepare = production
        .find("let target = match &owner.stop_target")
        .unwrap();
    let downgrade = production[prepare..]
        .find("target.without_cleanup_proof()")
        .unwrap()
        + prepare;
    let persist = production[downgrade..]
        .find("*current = owner.clone()")
        .unwrap()
        + downgrade;
    let dispatch = production[persist..]
        .find("stop_target(target).await?")
        .unwrap()
        + persist;
    let retain = production[dispatch..]
        .find("!target.cleanup_proof_eligible()")
        .unwrap()
        + dispatch;
    let release = production[retain..]
        .find("release_tailscale_main_states_if_token")
        .unwrap()
        + retain;
    assert!(
        prepare < downgrade
            && downgrade < persist
            && persist < dispatch
            && dispatch < retain
            && retain < release
    );
    assert!(production[prepare..downgrade].contains(").is_err()"));
    assert!(!production[prepare..dispatch].contains("owner.ready ="));
}

#[test]
fn production_stop_uses_distinct_stop_target_without_republishing_ready() {
    // Wiring guard only; actual native nonce/reload state-machine tests live
    // beside StopSessionTarget. This is not an executed integration claim.
    let source = polaris_source_probe::crate_source_in(
        env!("CARGO_MANIFEST_DIR"),
        "runtime/proxy/process_supervision.rs",
    );
    let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
    assert!(production.contains("prepare_ready_stop(ready)"));
    assert!(production.contains("prepare_observed_stop(observed)"));
    assert!(production.contains("stop_target(target)"));
    assert!(!production.contains("owner.ready = Some("));
    assert!(polaris_source_probe::crate_source_in(
        env!("CARGO_MANIFEST_DIR"),
        "runtime/proxy/pending_changes.rs"
    )
    .contains("owner.stop_target.as_ref()?"));
    assert!(polaris_source_probe::crate_source_in(
        env!("CARGO_MANIFEST_DIR"),
        "runtime/proxy/prerequisite.rs"
    )
    .contains(".stop_target"));
}
