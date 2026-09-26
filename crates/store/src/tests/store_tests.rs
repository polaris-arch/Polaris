use super::super::*;
use std::path::Path;

const CFG: &str = "/data/config.json";

#[test]
fn load_missing_file_returns_default_and_marks_was_missing() {
    // 新装：文件不存在 → 返回默认配置 + was_missing=true（调用方可安全落盘默认）。
    let fs = MockFs::default();
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(!res.loaded_from_disk);
    assert!(res.was_missing);
    assert!(res.error.is_none());
    // 默认配置有 expected 字段
    assert_eq!(res.config["proxyMode"], serde_json::json!("smart"));
    assert_eq!(
        res.config["proxyModeType"],
        serde_json::json!("systemProxy")
    );
    assert_eq!(res.config["mixedPort"], serde_json::json!(7890));
}

#[test]
fn load_valid_config_round_trips_through_save() {
    // save → load 往返：落盘的配置能原样加载回来（关键字段不丢）。
    let fs = MockFs::default();
    let cfg = serde_json::json!({
        "proxyMode": "smart",
        "proxyModeType": "tun",
        "logLevel": "warn",
        "mixedPort": 8080,
        "controlPort": 9091,
        "servers": [{"id":"s1","name":"HK","protocol":"trojan","address":"1.2.3.4","port":443,"password":"pw"}],
        "customRules": [{"id":"r1","type":"domain","values":["a.com"],"action":"direct","enabled":true}],
        "tunConfig": {"mtu":1400,"autoRoute":true,"strictRoute":false}
    });
    ConfigStore::save(&fs, Path::new(CFG), &cfg, "abcdef012345").unwrap();
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(res.loaded_from_disk);
    assert!(res.error.is_none());
    assert_eq!(res.config["proxyMode"], serde_json::json!("smart"));
    assert_eq!(res.config["proxyModeType"], serde_json::json!("tun"));
    assert_eq!(res.config["mixedPort"], serde_json::json!(8080));
    assert_eq!(res.config["servers"].as_array().unwrap().len(), 1);
    assert_eq!(res.config["customRules"].as_array().unwrap().len(), 1);
}

#[test]
fn two_tailscale_userspace_nodes_survive_real_save_load() {
    let fs = MockFs::default();
    let cfg = serde_json::json!({
        "proxyMode": "smart",
        "proxyModeType": "systemProxy",
        "mixedPort": 8080,
        "controlPort": 9091,
        "logLevel": "warn",
        "tunConfig": {"mtu":1400,"autoRoute":true,"strictRoute":false},
        "servers": [
            {"id":"ts-a", "name":"Tailnet A", "protocol":"tailscale",
             "tailscaleSettings":{"reverseMesh":false,"controlUrl":"https://a.example"}},
            {"id":"ts-b", "name":"Tailnet B", "protocol":"tailscale",
             "tailscaleSettings":{"reverseMesh":false,"controlUrl":"https://b.example"}}
        ]
    });
    ConfigStore::save(&fs, Path::new(CFG), &cfg, "abcdef012345").unwrap();
    let loaded = ConfigStore::load(&fs, Path::new(CFG));
    assert!(loaded.error.is_none());
    let nodes = loaded.config["servers"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["id"], "ts-a");
    assert_eq!(nodes[1]["id"], "ts-b");
    assert_eq!(
        nodes[1]["tailscaleSettings"]["controlUrl"],
        "https://b.example"
    );
}

#[test]
fn load_corrupt_json_returns_default_and_does_not_overwrite_disk() {
    // 维度7 #7 核心：坏 JSON → 内存回落默认，**磁盘真实文件原样保留**（绝不覆盖）。
    let corrupt = "{ this is not valid json";
    let fs = MockFs::default().with(Path::new(CFG), corrupt);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(!res.loaded_from_disk);
    assert!(!res.was_missing);
    assert!(matches!(res.error, Some(StoreError::Parse(_))));
    // 磁盘真实文件原样保留（未被默认配置覆盖）
    assert_eq!(fs.snapshot(Path::new(CFG)).as_deref(), Some(corrupt));
}

#[test]
fn load_bad_field_keeps_good_fields_and_preserves_disk() {
    // 维度7 #7：坏字段（servers 非 array）被跳过，好字段（customRules）保留；
    // 磁盘原文件原样保留（sanitize 只改内存 Value，不落盘）。
    let original = r#"{
            "proxyMode": "smart",
            "proxyModeType": "systemProxy",
            "logLevel": "info",
            "mixedPort": 7890,
            "servers": "not-an-array",
            "customRules": [{"id":"r1","type":"domain","values":["a.com"],"action":"proxy","enabled":true}],
            "tunConfig": {"mtu":1350,"autoRoute":true,"strictRoute":true}
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), original);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(res.loaded_from_disk);
    assert!(res.error.is_none());
    // 坏字段已从内存配置剔除（servers 缺失 → finalize 填 []）
    assert_eq!(res.config["servers"], serde_json::json!([]));
    // 好字段保留
    assert_eq!(res.config["customRules"].as_array().unwrap().len(), 1);
    // 磁盘原文件原样保留（load 不落盘；迁移落盘由调用方 best-effort 决定）
    assert_eq!(fs.snapshot(Path::new(CFG)).as_deref(), Some(original));
}

#[test]
fn load_bad_server_dropped_good_server_kept() {
    // 维度7 #7：逐节点 sanitize——坏节点剔除、好节点保留。
    let json = r#"{
            "proxyMode": "smart",
            "proxyModeType": "tun",
            "logLevel": "info",
            "mixedPort": 7890,
            "servers": [
                {"id":"good","name":"Good","protocol":"trojan","address":"1.2.3.4","port":443,"password":"pw"},
                {"id":"","name":"Bad","protocol":"trojan"},
                {"id":"unknown","name":"U","protocol":"nonexistent","address":"5.6.7.8","port":80}
            ],
            "tunConfig": {"mtu":1350,"autoRoute":true,"strictRoute":true}
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), json);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(res.loaded_from_disk);
    let servers = res.config["servers"].as_array().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(servers[0]["id"], serde_json::json!("good"));
}

#[test]
fn save_rejects_invalid_config() {
    // save 前复跑 validate：proxyMode 非法 → Err，不落盘。
    let fs = MockFs::default();
    let bad = serde_json::json!({
        "proxyMode": "invalid-mode",
        "proxyModeType": "tun",
        "logLevel": "info",
        "tunConfig": {"mtu":1350,"autoRoute":true,"strictRoute":true}
    });
    let res = ConfigStore::save(&fs, Path::new(CFG), &bad, "abcdef012345");
    assert!(matches!(res, Err(StoreError::Validation(_))));
    assert!(!fs.exists(Path::new(CFG)));
}

#[test]
fn save_uses_atomic_tmp_then_rename() {
    // 原子写：经 tmp 文件 → rename，不直接覆盖（防半写截断）。
    let fs = MockFs::default();
    let cfg = default_config();
    ConfigStore::save(&fs, Path::new(CFG), &cfg, "abcdef012345").unwrap();
    let ops = fs.operations();
    // 应有 Write(tmp) + Rename(tmp→final)
    assert!(ops.iter().any(|o| matches!(
        o,
        FsOp::Write(p, _) if p.to_string_lossy().ends_with("abcdef012345.tmp")
    )));
    assert!(ops.iter().any(|o| matches!(
        o,
        FsOp::Rename(from, _) if from.to_string_lossy().ends_with("abcdef012345.tmp")
    )));
    // 最终文件存在
    assert!(fs.exists(Path::new(CFG)));
}

#[test]
fn load_migrates_old_format_and_is_idempotent() {
    // 维度7 #54：旧格式 → 迁移 → 新格式；二次 load（已迁移）幂等。
    let old = r#"{
            "proxyMode": "smart",
            "proxyModeType": "TUN",
            "logLevel": "info",
            "tunConfig": {"mtu":1350,"stack":"system","autoRoute":true,"strictRoute":true},
            "subscriptionUpdateViaProxy": true
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), old);
    let res1 = ConfigStore::load(&fs, Path::new(CFG));
    assert!(res1.loaded_from_disk);
    assert!(res1.migration_delta.changed, "首次加载有迁移变更");
    // 迁移效果
    // 遗留 `stack` 被删、且不再写 `tunStackMigrated`（TUN stack 已随上游弃用移除）。
    assert!(res1.config["tunConfig"].get("stack").is_none());
    assert!(res1.config.get("tunStackMigrated").is_none());
    assert_eq!(
        res1.config["subscriptionProxyPolicy"],
        serde_json::json!("proxy")
    );
    assert!(!res1
        .config
        .as_object()
        .unwrap()
        .contains_key("subscriptionUpdateViaProxy"));
    assert_eq!(
        res1.config["dnsConfig"]["fakeIpToggleMigrated"],
        serde_json::json!(true)
    );

    // 模拟落盘迁移后配置 → 二次 load 幂等（migration_delta.changed=false）
    let migrated = res1.config.clone();
    let fs2 = MockFs::default().with(Path::new(CFG), migrated.to_string());
    let res2 = ConfigStore::load(&fs2, Path::new(CFG));
    assert!(res2.loaded_from_disk);
    assert!(!res2.migration_delta.changed, "已迁移配置二次加载无变更");
    // 内容稳定（幂等）
    assert_eq!(res2.config, migrated);
}

#[test]
fn load_validation_failure_returns_default_preserves_disk() {
    // validate 失败（缺 tunConfig）→ 回落默认，磁盘原文件保留。
    let bad = r#"{
            "proxyMode": "smart",
            "proxyModeType": "tun",
            "logLevel": "info",
            "mixedPort": 7890
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), bad);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(!res.loaded_from_disk);
    assert!(matches!(res.error, Some(StoreError::Validation(_))));
    // 磁盘原文件保留
    assert_eq!(fs.snapshot(Path::new(CFG)).as_deref(), Some(bad));
    // 内存回落默认配置
    assert_eq!(res.config["proxyMode"], serde_json::json!("smart"));
}

#[test]
fn load_corrupt_wires_backup_corrupt() {
    // 该修（HIGH）：损坏配置 load 失败分支须调 backup_corrupt（此前 load 只回落默认、从不备份）。
    let corrupt = "{ this is not valid json";
    let fs = MockFs::default().with(Path::new(CFG), corrupt);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(!res.loaded_from_disk);
    // 原文件原样保留（copy 不覆盖）
    assert_eq!(fs.snapshot(Path::new(CFG)).as_deref(), Some(corrupt));
    // 生成了一份 config.corrupt-<ts>.json，内容 == 损坏原文件
    let dir = Path::new("/data");
    let backups: Vec<String> = fs
        .list_dir(dir)
        .unwrap()
        .into_iter()
        .filter(|n| n.starts_with("config.corrupt-") && n.ends_with(".json"))
        .collect();
    assert_eq!(backups.len(), 1, "损坏配置须备份一份快照");
    assert_eq!(
        fs.snapshot(&dir.join(&backups[0])).as_deref(),
        Some(corrupt),
        "备份内容须 == 损坏原文件（保留人工恢复机会）"
    );
}

#[test]
fn load_validation_failure_wires_backup_corrupt() {
    // validate 失败分支（缺 tunConfig）同样须备份（非仅 JSON 坏）。
    let bad = r#"{"proxyMode":"smart","proxyModeType":"tun","logLevel":"info","mixedPort":7890}"#;
    let fs = MockFs::default().with(Path::new(CFG), bad);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(!res.loaded_from_disk);
    let dir = Path::new("/data");
    let n = fs
        .list_dir(dir)
        .unwrap()
        .into_iter()
        .filter(|n| n.starts_with("config.corrupt-") && n.ends_with(".json"))
        .count();
    assert_eq!(n, 1, "校验失败也须备份");
}

#[test]
fn load_corrupt_prunes_backups_to_two() {
    // 保留最近 2 份：预置 3 份旧备份 + 本次损坏再产 1 份 → prune 后仅剩 2。
    let corrupt = "{ broken";
    let dir = Path::new("/data");
    let mut fs = MockFs::default().with(Path::new(CFG), corrupt);
    for i in 1..=3u64 {
        fs = fs.with(
            &dir.join(format!("config.corrupt-{i:014}-000000000.json")),
            "old",
        );
    }
    let _ = ConfigStore::load(&fs, Path::new(CFG));
    let backups: Vec<String> = fs
        .list_dir(dir)
        .unwrap()
        .into_iter()
        .filter(|n| n.starts_with("config.corrupt-") && n.ends_with(".json"))
        .collect();
    assert_eq!(backups.len(), 2, "prune 后仅保留最近 2 份损坏备份");
}

#[test]
fn load_backs_up_before_rule_migration() {
    // 该修（MED）：含旧 DomainRule 的配置 load 时须先落 .pre-rule-migration.bak（判据看原始 JSON，
    // 因 sanitize 会丢弃无 type 的旧规则；备份保原始供回滚）。
    let legacy = r#"{
            "proxyMode":"smart","proxyModeType":"systemProxy","logLevel":"info","mixedPort":7890,
            "tunConfig":{"mtu":1350,"autoRoute":true,"strictRoute":true},
            "customRules":[{"id":"a","domains":["x.com"],"action":"proxy","enabled":true}]
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), legacy);
    let _ = ConfigStore::load(&fs, Path::new(CFG));
    let bak = Path::new("/data/config.json.pre-rule-migration.bak");
    assert_eq!(
        fs.snapshot(bak).as_deref(),
        Some(legacy),
        "迁移前须备份原始配置（含旧规则）"
    );
}

#[test]
fn load_preserves_legacy_domain_rule_through_migration() {
    // 项1 修（HIGH · sanitize-strips-legacy-rules-before-migrate）：含 1 条旧 DomainRule（无 type + domains）
    // 的配置过 load 后规则须**存活**（迁移为 domainSuffix Rule），而非被 sanitize 在 migrate 之前剥光 →
    // customRules==[] 静默全丢。变异牙：打断 `sanitize_custom_rules` 的旧规则放行（回归到「无 type 即丢弃」）→
    // migrate 阶段无旧规则可迁 → rules.len()==0，本测转红。
    let legacy = r#"{
            "proxyMode":"smart","proxyModeType":"systemProxy","logLevel":"info","mixedPort":7890,
            "tunConfig":{"mtu":1350,"autoRoute":true,"strictRoute":true},
            "customRules":[{"id":"legacy-1","domains":["example.com"],"action":"proxy","enabled":true}]
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), legacy);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(res.loaded_from_disk);
    let rules = res.config["customRules"].as_array().unwrap();
    assert_eq!(
        rules.len(),
        1,
        "旧规则迁移后存活（非 sanitize 先剥光致静默全丢）"
    );
    assert_eq!(
        rules[0]["type"],
        serde_json::json!("domainSuffix"),
        "旧 DomainRule 迁移为 domainSuffix Rule"
    );
    assert_eq!(rules[0]["id"], serde_json::json!("legacy-1"), "id 保留");
    assert_eq!(
        rules[0]["values"][0],
        serde_json::json!("example.com"),
        "domains → values"
    );
}

#[test]
fn load_no_rule_backup_for_modern_rules() {
    // 幂等/无误备：新 shape 规则（含 type）不触发 .pre-rule-migration.bak。
    let modern = r#"{
            "proxyMode":"smart","proxyModeType":"systemProxy","logLevel":"info","mixedPort":7890,
            "tunConfig":{"mtu":1350,"autoRoute":true,"strictRoute":true},
            "customRules":[{"id":"a","type":"domainSuffix","values":["x.com"],"action":"proxy","enabled":true}]
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), modern);
    let _ = ConfigStore::load(&fs, Path::new(CFG));
    assert!(
        fs.snapshot(Path::new("/data/config.json.pre-rule-migration.bak"))
            .is_none(),
        "新 shape 规则不该产生迁移备份"
    );
}

/// 🔴 新装配置**不得**写死 MTU（2026-08-05 起）。
///
/// 此前按平台写 mac 1400 / 其余 1350。一旦落盘成具体数，此后默认值再怎么改都追不上这台机器——
/// 存量 1350/1400 正因此需要 `migrate_tun_mtu` 清一遍。本轮默认值从「栈 × 平台」改成单一
/// `DEFAULT_TUN_MTU`，正是这条不变量让存量配置无需再迁移一次。
///
/// 缺席 = 自动，由 config-engine 在生成期取 `tun_config::DEFAULT_TUN_MTU`。
#[test]
fn default_config_leaves_mtu_absent() {
    let cfg = default_config();
    assert!(
        cfg["tunConfig"].get("mtu").is_none(),
        "新装不得把 MTU 冻在磁盘上：{}",
        cfg["tunConfig"]
    );
    // 新装同时置迁移标记：否则首次启动会对一份本就正确的配置再跑一次迁移（无害但 changed=true，
    // 会白写一次盘 + 白发一次 configChanged）。
    assert_eq!(cfg["tunMtuMigrated"], serde_json::json!(true));
    // 新装不得再写 TUN stack 相关的任何键（已随上游弃用移除）。
    assert!(
        cfg["tunConfig"].get("stack").is_none(),
        "新装不得写 stack：{}",
        cfg["tunConfig"]
    );
    assert!(
        cfg.get("tunStackMigrated").is_none(),
        "新装不得写 tunStackMigrated"
    );
    assert_eq!(
        cfg["keepTrayMenuWarm"],
        serde_json::json!(true),
        "托盘 warm 默认开启，以约 30–40MB renderer 常驻换取无冷启滞后的托盘交互"
    );
}

#[test]
fn backup_corrupt_copies_without_overwriting_original() {
    // 损坏备份：copy 到 config.corrupt-<ts>.json，原文件不动。
    let corrupt = "{ broken";
    let fs = MockFs::default().with(Path::new(CFG), corrupt);
    ConfigStore::backup_corrupt(&fs, Path::new(CFG), "2026-07-15T00-00-00Z");
    let backup = Path::new("/data/config.corrupt-2026-07-15T00-00-00Z.json");
    assert_eq!(fs.snapshot(backup).as_deref(), Some(corrupt));
    // 原文件仍在
    assert_eq!(fs.snapshot(Path::new(CFG)).as_deref(), Some(corrupt));
}

/// 🔴 端到端：带**非法** `tunConfig.stack` 的旧配置必须正常加载（不走「校验失败 → 备份 + 回落默认」），
/// 迁移后遗留键消失；save 路径（`canonicalize_for_save` / `save`，备份导入先走它）同样不得拒收。
///
/// 回落默认是本类缺陷最伤的形态：用户的节点、规则整份被默认配置顶掉，只因为一个已经没有意义的键。
/// 对照：同一份配置把 `proxyMode` 改坏 → 必须回落（证明本用例确实走在校验链上，而不是校验被旁路）。
#[test]
fn legacy_invalid_tun_stack_loads_and_saves_without_validation_error() {
    let legacy = r#"{
            "proxyMode": "smart",
            "proxyModeType": "tun",
            "logLevel": "info",
            "mixedPort": 7890,
            "servers": [{"id":"s1","name":"HK","protocol":"trojan","address":"1.2.3.4","port":443,"password":"pw"}],
            "tunConfig": {"stack":"bogus","autoRoute":true,"strictRoute":false},
            "tunStackMigrated": true
        }"#;
    let fs = MockFs::default().with(Path::new(CFG), legacy);
    let res = ConfigStore::load(&fs, Path::new(CFG));
    assert!(
        res.error.is_none(),
        "遗留非法 stack 不得导致加载失败：{:?}",
        res.error
    );
    assert!(res.loaded_from_disk, "不得回落默认配置");
    assert!(
        res.migration_delta.changed,
        "删了遗留键就必须报变更，调用方才会落盘"
    );
    assert_eq!(
        res.config["servers"].as_array().unwrap().len(),
        1,
        "用户数据不得丢"
    );
    assert_eq!(
        res.config["tunConfig"]["strictRoute"],
        serde_json::json!(false)
    );
    assert!(res.config["tunConfig"].get("stack").is_none());
    assert!(res.config.get("tunStackMigrated").is_none());

    // save 路径不跑迁移链：遗留键原样保留，但绝不能因它校验失败。
    let raw: serde_json::Value = serde_json::from_str(legacy).unwrap();
    let canonical = ConfigStore::canonicalize_for_save(&raw).expect("save 路径不得拒收遗留 stack");
    assert_eq!(canonical["tunConfig"]["stack"], serde_json::json!("bogus"));
    ConfigStore::save(&fs, Path::new(CFG), &raw, "abcdef012345").expect("save 不得拒收遗留 stack");

    // 对照：真正非法的字段仍会让加载回落 —— 证明上面的「不回落」不是因为校验被整体旁路。
    let broken = legacy.replace(r#""proxyMode": "smart""#, r#""proxyMode": "nope""#);
    assert_ne!(broken, legacy, "对照输入没改到");
    let fs2 = MockFs::default().with(Path::new(CFG), &broken);
    let res2 = ConfigStore::load(&fs2, Path::new(CFG));
    assert!(!res2.loaded_from_disk, "非法 proxyMode 必须回落默认");
    assert!(matches!(res2.error, Some(StoreError::Validation(_))));
}
