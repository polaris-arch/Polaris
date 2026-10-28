use super::*;
use serde_json::{json, Value};
use tempfile::tempdir;

fn file_entry(name: &str, bytes: &[u8]) -> Value {
    json!({"relativePath":name,"sha256":hash(bytes),"bytes":bytes.len()})
}

fn setup(with_rule: bool) -> (tempfile::TempDir, ExactBinding, PathBuf, PathBuf, PathBuf) {
    let temp = tempdir().unwrap();
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    let core = temp.path().join("sing-box");
    fs::write(&core, b"exact-test-binary").unwrap();
    let plan = b"{\"plan\":1}";
    fs::write(source.join("plan.json"), plan).unwrap();
    let rules = b"{\"version\":1,\"rules\":[]}";
    let rule_path = source.join("rules/rs-0000.json");
    let mut rule_entries = Vec::new();
    let mut route = json!({"rules":[],"final":"direct"});
    if with_rule {
        fs::create_dir(source.join("rules")).unwrap();
        fs::write(&rule_path, rules).unwrap();
        route["rule_set"] = json!([{
            "tag":"local", "type":"local", "format":"source",
            "path":rule_path.to_string_lossy()
        }]);
        rule_entries.push(file_entry("rules/rs-0000.json", rules));
    }
    let config = json!({
        "log":{"level":"info","timestamp":true},
        "inbounds":[],"outbounds":[{"type":"direct","tag":"direct"}],
        "route":route
    });
    let config_raw = serde_json::to_vec_pretty(&config).unwrap();
    fs::write(source.join("config.json"), &config_raw).unwrap();
    let manifest = json!({
        "schemaVersion":1,"planId":"plan-1","planDigest":hash(plan),
        "generatorVersion":"test","config":file_entry("config.json", &config_raw),
        "plan":file_entry("plan.json",plan),"ruleFiles":rule_entries
    });
    let manifest_raw = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(source.join("manifest.json"), &manifest_raw).unwrap();
    let binding = ExactBinding {
        run_ref: "run-1".into(),
        plan_id: "plan-1".into(),
        plan_digest: hash(plan),
        artifact_digest: hash(&manifest_raw),
        config_sha256: hash(&config_raw),
        core_sha256: hash(b"exact-test-binary"),
    };
    let target = temp.path().join("helper-private");
    (temp, binding, source.join("config.json"), core, target)
}

#[test]
fn snapshot_rewrites_rule_path_and_binds_launched_bytes() {
    let (_temp, binding, config, core, target) = setup(true);
    let proof = stage(&config, &core, &target, &binding).unwrap();
    let launched = fs::read(&proof.config_path).unwrap();
    let parsed: Value = serde_json::from_slice(&launched).unwrap();
    assert_eq!(
        parsed["route"]["rule_set"][0]["path"],
        target.join("rules/rs-0000.json").to_string_lossy().as_ref()
    );
    assert_eq!(proof.launched_config_sha256, hash(&launched));
    assert_ne!(proof.launched_config_sha256, binding.config_sha256);
    assert_eq!(
        fs::read(target.join("rules/rs-0000.json")).unwrap(),
        b"{\"version\":1,\"rules\":[]}"
    );
    assert_eq!(proof.executable_path, target.join("sing-box"));
    assert_eq!(proof.launch_closure_digest.len(), 64);
}

#[test]
fn source_rule_change_fails_before_snapshot_creation() {
    let (_temp, binding, config, core, target) = setup(true);
    fs::write(
        config.parent().unwrap().join("rules/rs-0000.json"),
        b"changed",
    )
    .unwrap();
    assert!(matches!(
        stage(&config, &core, &target, &binding),
        Err(SnapshotError::HashMismatch)
    ));
    assert!(!target.exists());
}

#[test]
fn source_changes_after_staging_cannot_change_launched_objects() {
    let (_temp, binding, config, core, target) = setup(true);
    let proof = stage(&config, &core, &target, &binding).unwrap();
    let launched = fs::read(&proof.config_path).unwrap();
    let executable = fs::read(&proof.executable_path).unwrap();
    fs::write(&config, b"changed config").unwrap();
    fs::write(&core, b"changed core").unwrap();
    fs::write(
        config.parent().unwrap().join("rules/rs-0000.json"),
        b"changed rule",
    )
    .unwrap();
    assert_eq!(fs::read(&proof.config_path).unwrap(), launched);
    assert_eq!(fs::read(&proof.executable_path).unwrap(), executable);
    assert_eq!(
        fs::read(target.join("rules/rs-0000.json")).unwrap(),
        b"{\"version\":1,\"rules\":[]}"
    );
    assert_eq!(proof.launched_config_sha256, hash(&launched));
}

#[test]
fn plan_digest_must_bind_the_checked_plan_bytes() {
    let (_temp, mut binding, config, core, target) = setup(false);
    binding.plan_digest = "b".repeat(64);
    let manifest_path = config.parent().unwrap().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["planDigest"] = json!(binding.plan_digest);
    let raw = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(&manifest_path, &raw).unwrap();
    binding.artifact_digest = hash(&raw);
    assert!(matches!(
        stage(&config, &core, &target, &binding),
        Err(SnapshotError::HashMismatch)
    ));
    assert!(!target.exists());
}

#[test]
fn source_config_change_and_unknown_field_fail_closed() {
    let (_temp, binding, config, core, target) = setup(false);
    fs::write(&config, b"{}").unwrap();
    assert!(matches!(
        stage(&config, &core, &target, &binding),
        Err(SnapshotError::HashMismatch)
    ));
    assert!(!target.exists());
    let (_temp, mut binding, config, core, target) = setup(false);
    let mut value: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    value["outbounds"][0]["future_file"] = json!("secret.txt");
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    fs::write(&config, &bytes).unwrap();
    let manifest_path = config.parent().unwrap().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["config"] = file_entry("config.json", &bytes);
    let raw = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(manifest_path, &raw).unwrap();
    binding.config_sha256 = hash(&bytes);
    binding.artifact_digest = hash(&raw);
    assert!(matches!(
        stage(&config, &core, &target, &binding),
        Err(SnapshotError::Unsupported)
    ));
    assert!(!target.exists());
}

#[test]
fn writable_tailscale_state_remains_unsupported() {
    let (_temp, mut binding, config, core, target) = setup(false);
    let mut value: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
    value["endpoints"] = json!([{
        "type":"tailscale","tag":"tailnet-a",
        "state_directory":"/tmp/state-a","taildrop_directory":"/tmp/state-a/Taildrop"
    }]);
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    fs::write(&config, &bytes).unwrap();
    let manifest_path = config.parent().unwrap().join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
    manifest["config"] = file_entry("config.json", &bytes);
    let raw = serde_json::to_vec_pretty(&manifest).unwrap();
    fs::write(manifest_path, &raw).unwrap();
    binding.config_sha256 = hash(&bytes);
    binding.artifact_digest = hash(&raw);
    assert!(matches!(
        stage(&config, &core, &target, &binding),
        Err(SnapshotError::Unsupported)
    ));
    assert!(!target.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn sidecar_is_part_of_the_executable_closure() {
    let (_temp, binding, config, core, target) = setup(false);
    let first = stage(&config, &core, &target, &binding).unwrap();
    fs::write(core.parent().unwrap().join("libcronet.so"), b"sidecar-v1").unwrap();
    let second_target = target.with_file_name("helper-private-2");
    let second = stage(&config, &core, &second_target, &binding).unwrap();
    assert_ne!(first.launch_closure_digest, second.launch_closure_digest);
    assert_eq!(
        fs::read(second_target.join("libcronet.so")).unwrap(),
        b"sidecar-v1"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn broken_or_linked_sidecar_is_not_misreported_as_absent() {
    use std::os::unix::fs::symlink;
    let (_temp, binding, config, core, target) = setup(false);
    let sidecar = core.parent().unwrap().join("libcronet.so");
    symlink("missing-cronet", &sidecar).unwrap();
    assert!(matches!(
        stage(&config, &core, &target, &binding),
        Err(SnapshotError::Unsupported)
    ));
    assert!(!target.exists());
}

#[cfg(unix)]
#[test]
fn protected_entry_rejects_unprivileged_destination_without_writing() {
    let (_temp, binding, config, core, target) = setup(false);
    let result = stage_protected(&config, &core, target.parent().unwrap(), &target, &binding);
    assert!(matches!(result, Err(SnapshotError::Unsupported)));
    assert!(!target.exists());
}
