use super::*;
#[test]
fn windows_contract_is_exact_compiled_run_main_only() {
    let source = "a".repeat(40);
    let tree = "b".repeat(40);
    let policy = serde_json::json!({"schema":"polaris-trusted-execution-v1","sourceCommit":source,"sourceTree":tree,"loader":LOADER,"purposes":["run-main-v1"]});
    assert_eq!(
        SelectedPolicy::declaration(&policy, &source, &tree, "win")
            .unwrap()
            .0,
        vec![ExecPurpose::RunMain]
    );
    for field in ["sourceCommit", "sourceTree", "loader", "purposes", "schema"] {
        let mut bad = policy.clone();
        bad[field] = serde_json::Value::Null;
        assert!(SelectedPolicy::declaration(&bad, &source, &tree, "win").is_err());
    }
    let mut bad = policy.clone();
    bad["allowUntrusted"] = true.into();
    assert!(SelectedPolicy::declaration(&bad, &source, &tree, "win").is_err());
    bad = policy;
    bad["purposes"] = serde_json::json!(["run-main-v1", "run-login-v1"]);
    assert!(SelectedPolicy::declaration(&bad, &source, &tree, "win").is_err());
}
#[test]
fn ordinary_primary_token_cannot_issue_or_adopt_any_file() {
    // No fixture SID, policy or authority constructor bypasses the actual
    // primary-token check. SYSTEM is an unperformed service acceptance gate.
    assert!(
        issuer::system_actor().is_err(),
        "this ordinary test must not run as SYSTEM"
    );
}
