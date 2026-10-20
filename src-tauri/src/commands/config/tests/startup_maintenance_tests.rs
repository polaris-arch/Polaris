use super::super::*;
use crate::runtime::config::ConfigManager;
use crate::test_support::TestDir;

fn temp_dir(tag: &str) -> TestDir {
    TestDir::new(&format!("polaris-maint-{tag}-"))
}

/// 有效配置模板（能过 validate → load 成功走 loaded_from_disk）。
fn valid_config() -> Value {
    json!({
        "proxyMode": "global",
        "proxyModeType": "systemProxy",
        "logLevel": "info",
        "mixedPort": 7890,
        "controlPort": 9090,
        "tunConfig": {"mtu": 1350, "autoRoute": true, "strictRoute": true}
    })
}

#[test]
fn clash_secret_is_32_lowercase_hex_and_unique() {
    // 该修（HIGH）：CSPRNG 16B → 32 位小写 hex（对齐 上游 randomBytes(16).toString('hex')）。
    let a = generate_local_api_secret().unwrap();
    assert_eq!(a.len(), 32, "16 字节 → 32 hex");
    assert!(a
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    let b = generate_local_api_secret().unwrap();
    assert_ne!(a, b, "连续生成不得相同（CSPRNG）");
}

#[test]
fn backfill_generates_and_persists_secret_when_missing() {
    // 缺 clashApiSecret → 回填随机值并**落盘持久化**（跨会话稳定，供外部客户端复用）。
    let dir = temp_dir("secret-gen");
    std::fs::write(dir.join("config.json"), valid_config().to_string()).unwrap();
    let mgr = ConfigManager::new(dir.clone());
    backfill_secret_and_privacy(&mgr).unwrap();
    // 落盘（新 ConfigManager 从盘重载）后 secret 存在且非空。
    let mgr2 = ConfigManager::new(dir.clone());
    let secret = mgr2.load_full().unwrap()["clashApiSecret"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert_eq!(secret.len(), 32, "回填的 secret 须落盘持久化");
    // 幂等：二次维护不改已有 secret（稳定，不每次 load 重生成）。
    backfill_secret_and_privacy(&mgr2).unwrap();
    let mgr3 = ConfigManager::new(dir.clone());
    assert_eq!(
        mgr3.load_full().unwrap()["clashApiSecret"]
            .as_str()
            .unwrap(),
        secret,
        "已有 secret 须稳定不变"
    );
}

#[test]
fn backfill_preserves_existing_secret() {
    // 已有 secret → 不覆盖（幂等门；打断「!has_secret」判定即转红）。
    let dir = temp_dir("secret-keep");
    let mut cfg = valid_config();
    cfg["clashApiSecret"] = json!("deadbeefdeadbeefdeadbeefdeadbeef");
    std::fs::write(dir.join("config.json"), cfg.to_string()).unwrap();
    let mgr = ConfigManager::new(dir.clone());
    backfill_secret_and_privacy(&mgr).unwrap();
    let mgr2 = ConfigManager::new(dir.clone());
    assert_eq!(
        mgr2.load_full().unwrap()["clashApiSecret"]
            .as_str()
            .unwrap(),
        "deadbeefdeadbeefdeadbeefdeadbeef",
        "已有 secret 不得被覆盖"
    );
}

#[test]
fn backfill_does_not_overwrite_corrupt_config() {
    // 数据保护红线：损坏配置 load 回落默认，但维护**绝不** save 默认覆盖损坏原文件。
    let dir = temp_dir("corrupt-guard");
    let corrupt = "{ not valid json at all";
    std::fs::write(dir.join("config.json"), corrupt).unwrap();
    let mgr = ConfigManager::new(dir.clone());
    assert!(backfill_secret_and_privacy(&mgr).is_err());
    // 原损坏文件须原样保留（未被默认+secret 覆盖）。
    assert_eq!(
        std::fs::read_to_string(dir.join("config.json")).unwrap(),
        corrupt,
        "损坏配置绝不被维护覆盖"
    );
}

#[test]
fn f29_legacy_plaintext_migrated_to_scrypt_file_losslessly() {
    // 旧明文 privacyPassword 无损迁移为 scrypt 独立文件——密码不丢，锁不失效，且盘上明文被 scrub。
    let dir = temp_dir("f29");
    let mut cfg = valid_config();
    cfg["privacyPassword"] = json!("legacy-secret-42");
    std::fs::write(dir.join("config.json"), cfg.to_string()).unwrap();
    let mgr = ConfigManager::new(dir.clone());
    backfill_secret_and_privacy(&mgr).unwrap();
    // scrypt 文件已落；盘上明文已被清空（save_full 用 migrate 抹空的 cfg 覆盖）。
    let path = polaris_store::privacy_lock::lock_path(&dir);
    assert!(path.exists(), "须已落 scrypt 独立文件");
    let mgr2 = ConfigManager::new(dir.clone());
    let reloaded = mgr2.load_full().unwrap();
    assert_eq!(
        reloaded["privacyPassword"],
        json!(""),
        "盘上明文须被清空（无残留）"
    );
    assert!(
        reloaded.get(PRIVACY_PASSWORD_HASH_KEY).is_none(),
        "迁移落文件、不再写 config 里的 SHA-256 键"
    );
    assert!(
        unlock_core(&mgr2, "legacy-secret-42").unwrap(),
        "旧明文须能解锁（无损迁移）"
    );
    assert!(!unlock_core(&mgr2, "wrong").unwrap(), "错误密码不得解锁");
}

#[test]
fn f29_skips_when_scrypt_file_already_present() {
    // 已有 scrypt 文件（用户已设新密码）→ 旧明文不得覆盖（`!has_scrypt_file && !has_legacy` 门；打断即转红）。
    let dir = temp_dir("f29-skip");
    let mgr = ConfigManager::new(dir.clone());
    set_password_core(&mgr, "real-password", false).unwrap(); // scrypt 文件
                                                              // 手动往盘塞 legacy 明文（模拟旧残留 + 新密码并存）。
    let cfg_path = dir.join("config.json");
    let mut on_disk: Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    on_disk
        .as_object_mut()
        .unwrap()
        .insert("privacyPassword".into(), json!("stale-plaintext"));
    std::fs::write(&cfg_path, on_disk.to_string()).unwrap();
    backfill_secret_and_privacy(&mgr).unwrap();
    // 既有密码仍有效，旧明文未顶掉它。
    let mgr2 = ConfigManager::new(dir.clone());
    assert!(
        unlock_core(&mgr2, "real-password").unwrap(),
        "既有密码不被旧明文顶替"
    );
    assert!(
        !unlock_core(&mgr2, "stale-plaintext").unwrap(),
        "旧明文不得成为新密码"
    );
}

#[test]
fn failed_privacy_maintenance_still_persists_api_secret_and_remains_retryable() {
    for initial_secret in [None, Some("")] {
        for unreadable in [false, true] {
            let dir = temp_dir("independent-secret");
            let mut cfg = valid_config();
            if let Some(secret) = initial_secret {
                cfg["clashApiSecret"] = json!(secret);
            }
            std::fs::write(dir.join("config.json"), cfg.to_string()).unwrap();
            let mgr = ConfigManager::new(dir.clone());
            let credential = privacy_lock_path(&mgr);
            if unreadable {
                std::fs::create_dir(&credential).unwrap();
            } else {
                std::fs::write(&credential, "{broken").unwrap();
            }
            let done = AtomicBool::new(false);
            assert!(run_startup_maintenance(&mgr, &done).is_err());
            assert!(!done.load(Ordering::SeqCst));
            let disk: Value =
                serde_json::from_str(&std::fs::read_to_string(mgr.path()).unwrap()).unwrap();
            let secret = disk["clashApiSecret"].as_str().unwrap().to_owned();
            assert_eq!(secret.len(), 32);
            // Explicit fixture repair, never a real user's credential.
            if unreadable {
                std::fs::remove_dir(&credential).unwrap();
            } else {
                std::fs::remove_file(&credential).unwrap();
            }
            assert!(run_startup_maintenance(&mgr, &done).is_ok());
            assert!(done.load(Ordering::SeqCst));
            assert_eq!(mgr.current().unwrap()["clashApiSecret"], secret);
        }
    }
}

#[test]
fn plaintext_survives_failed_scrypt_migration_without_blocking_api_secret_publication() {
    let dir = temp_dir("plaintext-migration-retry");
    let mut cfg = valid_config();
    cfg["privacyPassword"] = json!("legacy-password");
    std::fs::write(dir.join("config.json"), cfg.to_string()).unwrap();
    let mgr = ConfigManager::new(dir.clone());
    std::fs::write(privacy_lock_path(&mgr), "{broken").unwrap();
    assert!(backfill_secret_and_privacy(&mgr).is_err());
    assert!(unlock_core(&mgr, "legacy-password").is_err());
    let disk: Value = serde_json::from_str(&std::fs::read_to_string(mgr.path()).unwrap()).unwrap();
    assert!(!disk["clashApiSecret"].as_str().unwrap().is_empty());
    assert!(verify_password(
        disk[PRIVACY_PASSWORD_HASH_KEY].as_str().unwrap(),
        "legacy-password"
    ));
    assert_ne!(disk["privacyPassword"], "legacy-password");
    std::fs::remove_file(privacy_lock_path(&mgr)).unwrap();
    backfill_secret_and_privacy(&mgr).unwrap();
    assert!(unlock_core(&mgr, "legacy-password").unwrap());
}

fn duplicate_identity_cold_recovery(plain_password: bool) {
    let dir = temp_dir("duplicate-cold-recovery");
    let mut cfg = valid_config();
    cfg["servers"] = json!([
        {"id":"same","name":"A","protocol":"trojan","address":"a.invalid","port":443,"password":"fixture-a"},
        {"id":"same","name":"B","protocol":"trojan","address":"b.invalid","port":443,"password":"fixture-b"}
    ]);
    if plain_password {
        cfg["clashApiSecret"] = json!("existing-fixture-secret");
        cfg["privacyPassword"] = json!("fixture-legacy-password");
    } else {
        cfg["clashApiSecret"] = json!("");
    }
    std::fs::write(dir.join("config.json"), cfg.to_string()).unwrap();
    let mgr = ConfigManager::new(dir.clone());
    let done = AtomicBool::new(false);
    // config_get must remain fail-closed; the independent recovery projection is available.
    assert!(run_startup_maintenance(&mgr, &done).is_err());
    assert!(!done.load(Ordering::SeqCst));
    let before_repair: Value = serde_json::from_slice(&std::fs::read(mgr.path()).unwrap()).unwrap();
    assert_eq!(
        before_repair, cfg,
        "failed maintenance must not migrate the ambiguous document"
    );
    assert!(
        !privacy_lock_path(&mgr).exists(),
        "identity failure must precede privacy migration"
    );
    assert!(
        mgr.load_full().is_err(),
        "ordinary load cannot publish ambiguous identities"
    );
    let after_load: Value = serde_json::from_slice(&std::fs::read(mgr.path()).unwrap()).unwrap();
    assert_eq!(after_load, cfg);
    let preview = serde_json::to_value(mgr.identity_conflicts().unwrap()).unwrap();
    assert_eq!(preview["groups"].as_array().unwrap().len(), 1);
    let mut expected = cfg.clone();
    expected["servers"][1]["id"] = json!("recovered-fixture-id");
    mgr.repair_server_identities(
        &serde_json::from_value::<crate::runtime::config::IdentityRepairRequest>(json!({
            "version": preview["version"],
            "choices": [{"indices":[0,1],"keeperIndex":0}]
        }))
        .unwrap(),
        || "recovered-fixture-id".into(),
    )
    .unwrap();
    let repaired: Value = serde_json::from_slice(&std::fs::read(mgr.path()).unwrap()).unwrap();
    assert_eq!(
        repaired, expected,
        "repair changes only the explicitly chosen nonkeeper ID"
    );
    assert!(
        serde_json::to_value(mgr.identity_conflicts().unwrap()).unwrap()["groups"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    run_startup_maintenance(&mgr, &done).unwrap();
    assert!(done.load(Ordering::SeqCst));
    let mut frontend = mgr.load_full().unwrap();
    apply_frontend_view(&mut frontend);
    assert!(frontend["clashApiSecret"]
        .as_str()
        .is_some_and(|s| !s.trim().is_empty()));
    assert!(frontend.get("privacyPassword").is_none());
    if plain_password {
        assert!(
            polaris_store::privacy_lock::has(&polaris_store::StdFs, &privacy_lock_path(&mgr))
                .unwrap()
        );
    }
}

#[test]
fn cold_duplicate_ids_with_blank_api_secret_have_independent_recovery() {
    duplicate_identity_cold_recovery(false);
}

#[test]
fn cold_duplicate_ids_with_pending_privacy_migration_have_independent_recovery() {
    duplicate_identity_cold_recovery(true);
}
