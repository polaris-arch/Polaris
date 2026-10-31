//! File preparation and cleanup through the production login transaction, with no real core.
use super::*;

type ArtifactPaths = (PathBuf, PathBuf);

fn prepared_artifacts(config_path: &Path) -> ArtifactPaths {
    let config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(config_path).unwrap()).unwrap();
    let cache = PathBuf::from(
        config["experimental"]["cache_file"]["path"]
            .as_str()
            .unwrap(),
    );
    assert_eq!(cache, login_cache_path(config_path));
    assert_eq!(cache.parent(), config_path.parent());
    // A fresh node's state remains lazy: the cache may be opened before that constructor runs.
    let state = Path::new(config["endpoints"][0]["state_directory"].as_str().unwrap());
    assert!(!state.exists());
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&cache)
        .expect("cache must already exist before check/spawn, without create permission");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for file in [config_path, cache.as_path()] {
            assert_eq!(
                std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    (config_path.to_owned(), cache)
}

struct ArtifactChecker {
    ok: bool,
    seen: Arc<Mutex<Vec<ArtifactPaths>>>,
}

#[async_trait]
impl ConfigChecker for ArtifactChecker {
    async fn check(&self, _binary: &Path, config: &Path) -> Result<(), String> {
        self.seen.lock().unwrap().push(prepared_artifacts(config));
        if self.ok {
            Ok(())
        } else {
            Err("fixture rejects configuration".into())
        }
    }
}

struct ArtifactSpawner {
    base: Arc<FakeSpawner>,
}

#[async_trait]
impl LoginCoreSpawner for ArtifactSpawner {
    async fn spawn(&self, request: SpawnRequest) -> Result<Box<dyn LoginCoreChild>, SpawnError> {
        prepared_artifacts(&request.config);
        self.base.spawn(request).await
    }
}

#[tokio::test]
async fn fresh_default_node_prepares_private_cache_before_check_and_spawn_without_ts_directory() {
    let root = temp_ud();
    // Supply native path components: the Windows directory guard deliberately rejects '/'.
    let user_data = root.join("not-created").join("app-config");
    assert!(!user_data.exists());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = LoginCoreRegistry::with_deps(
        Arc::new(ArtifactSpawner {
            base: fake_spawner(vec![], false, false),
        }),
        Arc::new(ArtifactChecker {
            ok: true,
            seen: seen.clone(),
        }),
        fake_subscriber(false),
        Arc::new(|| Ok(PathBuf::from("/fake/sing-box"))),
        Duration::from_secs(60),
    );
    let node = ts_server("fresh-default", "Tailscale");
    assert!(node.tailscale_settings.is_none());
    registry.prepare(&node.id, "fresh-login").await.unwrap();
    match registry
        .start_attempt(
            &node,
            &user_data,
            request("fresh-login"),
            &offline,
            Arc::new(FakeEmitter::default()),
        )
        .await
    {
        StartLoginOutcome::Started => {}
        StartLoginOutcome::Failed(reason) => panic!("fresh login failed: {reason}"),
        _ => panic!("fresh login did not start"),
    }
    let artifacts = seen.lock().unwrap().clone();
    assert_eq!(artifacts.len(), 1);
    let (config, cache) = &artifacts[0];
    assert!(config.is_file() && cache.is_file());
    assert!(!user_data.join("tailscale").exists());
    assert!(!user_data.join("tailscale-windows-v1").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&user_data).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    registry
        .cancel_attempt(&node.id, "fresh-login")
        .await
        .unwrap();
    assert!(!config.exists() && !cache.exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn check_and_spawn_failures_remove_owned_cache_and_config_but_keep_main_cache() {
    for check_ok in [false, true] {
        let user_data = temp_ud();
        let main_cache = user_data.join("cache.db");
        std::fs::write(&main_cache, "main-cache-sentinel").unwrap();
        let base = fake_spawner(vec![], false, true);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let registry = LoginCoreRegistry::with_deps(
            Arc::new(ArtifactSpawner { base: base.clone() }),
            Arc::new(ArtifactChecker {
                ok: check_ok,
                seen: seen.clone(),
            }),
            fake_subscriber(false),
            Arc::new(|| Ok(PathBuf::from("/fake/sing-box"))),
            Duration::from_secs(60),
        );
        registry.prepare("ts1", "failed-login").await.unwrap();
        let outcome = registry
            .start_attempt(
                &ts_server("ts1", "Tailscale"),
                &user_data,
                request("failed-login"),
                &offline,
                Arc::new(FakeEmitter::default()),
            )
            .await;
        let expected = if check_ok {
            "processStartFailed"
        } else {
            "configurationCheckFailed"
        };
        assert!(matches!(outcome, StartLoginOutcome::Failed(reason) if reason == expected));
        assert_eq!(
            base.count.load(Ordering::SeqCst),
            if check_ok { 1 } else { 0 }
        );
        for (config, cache) in seen.lock().unwrap().iter() {
            assert!(!config.exists() && !cache.exists());
        }
        assert_eq!(
            std::fs::read_to_string(&main_cache).unwrap(),
            "main-cache-sentinel"
        );
        assert!(!registry.shared.contains("ts1"));
        assert!(!user_data.join("tailscale").exists());
        std::fs::remove_dir_all(user_data).unwrap();
    }
}

#[tokio::test]
async fn cache_collision_rejects_before_check_or_spawn_without_deleting_existing_file() {
    let user_data = temp_ud();
    let base = fake_spawner(vec![], false, false);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let registry = LoginCoreRegistry::with_deps(
        base.clone(),
        Arc::new(ArtifactChecker {
            ok: true,
            seen: seen.clone(),
        }),
        fake_subscriber(false),
        Arc::new(|| Ok(PathBuf::from("/fake/sing-box"))),
        Duration::from_secs(60),
    );
    registry.prepare("ts1", "collision").await.unwrap();
    let config = login_config_path(&user_data, "ts1", registry.epoch.load(Ordering::SeqCst));
    let cache = login_cache_path(&config);
    std::fs::write(&cache, "existing-file-sentinel").unwrap();
    let outcome = registry
        .start_attempt(
            &ts_server("ts1", "Tailscale"),
            &user_data,
            request("collision"),
            &offline,
            Arc::new(FakeEmitter::default()),
        )
        .await;
    assert!(matches!(outcome, StartLoginOutcome::Failed(reason) if reason == "configWriteFailed"));
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(base.count.load(Ordering::SeqCst), 0);
    assert!(!config.exists());
    assert_eq!(
        std::fs::read_to_string(&cache).unwrap(),
        "existing-file-sentinel"
    );
    assert!(!registry.shared.contains("ts1"));
    std::fs::remove_dir_all(user_data).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn cache_symlink_is_rejected_without_touching_its_target_or_the_link() {
    let user_data = temp_ud();
    let target = user_data.join("outside-state-sentinel");
    std::fs::write(&target, "do-not-change").unwrap();
    let base = fake_spawner(vec![], false, false);
    let registry = reg_with(
        base.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    registry.prepare("ts1", "symlink").await.unwrap();
    let config = login_config_path(&user_data, "ts1", registry.epoch.load(Ordering::SeqCst));
    let cache = login_cache_path(&config);
    std::os::unix::fs::symlink(&target, &cache).unwrap();
    let outcome = registry
        .start_attempt(
            &ts_server("ts1", "Tailscale"),
            &user_data,
            request("symlink"),
            &offline,
            Arc::new(FakeEmitter::default()),
        )
        .await;
    assert!(matches!(outcome, StartLoginOutcome::Failed(reason) if reason == "configWriteFailed"));
    assert_eq!(base.count.load(Ordering::SeqCst), 0);
    assert!(!config.exists());
    assert!(std::fs::symlink_metadata(&cache)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "do-not-change");
    std::fs::remove_dir_all(user_data).unwrap();
}

#[tokio::test]
async fn unconfirmed_child_close_keeps_both_files_until_confirmed_retry() {
    let user_data = temp_ud();
    let base = fake_spawner(vec![], false, false);
    let registry = reg_with(
        base.clone(),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    registry.prepare("ts1", "held-close").await.unwrap();
    assert!(matches!(
        registry
            .start_attempt(
                &ts_server("ts1", "Tailscale"),
                &user_data,
                request("held-close"),
                &offline,
                Arc::new(FakeEmitter::default())
            )
            .await,
        StartLoginOutcome::Started
    ));
    let config = sole_login_config(&user_data);
    let cache = login_cache_path(&config);
    base.spawned.lock().unwrap()[0]
        .close_failures
        .store(1, Ordering::SeqCst);
    assert!(registry.cancel_login("ts1").await.is_err());
    assert!(registry.shared.contains("ts1"));
    assert!(config.is_file() && cache.is_file());
    assert!(registry.cancel_login("ts1").await.unwrap());
    assert!(!registry.shared.contains("ts1"));
    assert!(!config.exists() && !cache.exists());
    std::fs::remove_dir_all(user_data).unwrap();
}

#[tokio::test]
async fn cache_cleanup_failure_retains_config_and_claim_after_child_has_closed() {
    let user_data = temp_ud();
    let registry = reg_with(
        fake_spawner(vec![], false, false),
        fake_subscriber(false),
        true,
        Duration::from_secs(60),
    );
    registry.prepare("ts1", "held-cache").await.unwrap();
    assert!(matches!(
        registry
            .start_attempt(
                &ts_server("ts1", "Tailscale"),
                &user_data,
                request("held-cache"),
                &offline,
                Arc::new(FakeEmitter::default())
            )
            .await,
        StartLoginOutcome::Started
    ));
    let config = sole_login_config(&user_data);
    let cache = login_cache_path(&config);
    std::fs::remove_file(&cache).unwrap();
    std::fs::create_dir(&cache).unwrap();
    assert!(registry.cancel_login("ts1").await.is_err());
    assert!(registry.shared.contains("ts1"));
    assert!(config.is_file() && cache.is_dir());
    std::fs::remove_dir(&cache).unwrap();
    assert!(registry.cancel_login("ts1").await.unwrap());
    assert!(!registry.shared.contains("ts1"));
    assert!(!config.exists());
    std::fs::remove_dir_all(user_data).unwrap();
}

#[test]
fn stale_pair_cleanup_preserves_live_pair_unpaired_files_and_state() {
    let user_data = temp_ud();
    let old = login_config_path(&user_data, "ts1", 1);
    let live = login_config_path(&user_data, "ts1", 2);
    let unpaired = login_cache_path(&login_config_path(&user_data, "ts2", 3));
    let sqlite_neighbor = old.with_extension("cache.db-wal");
    let state = user_data.join("tailscale/ts1/tailscaled.state");
    std::fs::create_dir_all(state.parent().unwrap()).unwrap();
    let old_cache = login_cache_path(&old);
    let live_cache = login_cache_path(&live);
    for file in [
        &old,
        &live,
        &old_cache,
        &live_cache,
        &unpaired,
        &sqlite_neighbor,
        &state,
    ] {
        std::fs::write(file, "preserve-unless-owned-stale-pair").unwrap();
    }
    assert_eq!(
        super::super::stale::sweep_stale_login_configs(&user_data, std::slice::from_ref(&live)),
        1
    );
    assert!(!old.exists() && !login_cache_path(&old).exists());
    for file in [&live, &live_cache, &unpaired, &sqlite_neighbor, &state] {
        assert!(file.is_file());
    }
    std::fs::remove_dir_all(user_data).unwrap();
}
