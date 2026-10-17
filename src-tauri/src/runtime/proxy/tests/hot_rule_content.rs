use super::*;
use polaris_singbox_grpc::daemon::{RuleSetLoadStatus, RuleSetStatusResponse};

#[test]
fn rule_content_scope_and_ack_are_explicit() {
    let old = BTreeMap::from([
        ("a.json".into(), "old".into()),
        ("a.dns.json".into(), "old".into()),
    ]);
    let mut new = old.clone();
    new.insert("a.json".into(), "new".into());
    assert_eq!(
        single_rule_content_update(&old, &new),
        Some(("a.json", "new"))
    );
    new.insert("a.dns.json".into(), "new".into());
    assert!(single_rule_content_update(&old, &new).is_none());
    new.remove("a.dns.json");
    assert!(single_rule_content_update(&old, &new).is_none());
    let mut status = RuleSetStatusResponse {
        instance_started_at_unix_nano: 42,
        rule_sets: vec![RuleSetLoadStatus {
            tag: "custom-rule-a".into(),
            active_digest: "new".into(),
            revision: 1,
            watcher_state: "watching".into(),
            last_error: String::new(),
            closed: false,
        }],
    };
    assert!(rule_content_acknowledged(
        &status,
        "custom-rule-a",
        "new",
        42
    ));
    assert!(!rule_content_acknowledged(
        &status,
        "custom-rule-a",
        "old",
        42
    ));
    assert!(!rule_content_acknowledged(
        &status,
        "custom-rule-a",
        "new",
        43
    ));
    status.rule_sets[0].watcher_state = "failed".into();
    assert!(!rule_content_acknowledged(
        &status,
        "custom-rule-a",
        "new",
        42
    ));
    status.rule_sets[0].watcher_state = "watching".into();
    status.rule_sets[0].closed = true;
    assert!(!rule_content_acknowledged(
        &status,
        "custom-rule-a",
        "new",
        42
    ));
}

#[cfg(unix)]
#[test]
fn runtime_config_atomic_replacement_is_private_and_preserves_link_targets() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = std::env::temp_dir().join(format!(
        "polaris-private-{}",
        polaris_store::fs::random_tmp_suffix()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let runtime = dir.join("runtime.json");
    let target = dir.join("unrelated.json");
    std::fs::write(&target, "unrelated").unwrap();
    std::fs::write(&runtime, "old secret").unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o644)).unwrap();
    write_private_runtime_config(&runtime, "synthetic secret").unwrap();
    assert_eq!(
        std::fs::metadata(&runtime).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    std::fs::remove_file(&runtime).unwrap();
    symlink(&target, &runtime).unwrap();
    write_private_runtime_config(&runtime, "next secret").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "unrelated");
    assert!(!std::fs::symlink_metadata(&runtime)
        .unwrap()
        .file_type()
        .is_symlink());
    std::fs::remove_file(&runtime).unwrap();
    std::fs::hard_link(&target, &runtime).unwrap();
    write_private_runtime_config(&runtime, "third secret").unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "unrelated");
    let alias = dir.join("alias");
    symlink(&dir, &alias).unwrap();
    assert!(write_private_runtime_config(&alias.join("secret.json"), "no").is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}
