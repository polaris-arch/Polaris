#[test]
fn custom_host_arrays_are_redacted_in_complete_report() {
    let config = serde_json::json!({"servers":[{"id":"n", "name":"example node", "protocol":"custom", "customSettings":{"outbound":{"transport":{"headers":{"Host":["private-cover.example", 42], "X-Label":["public-label"]}}, "nested":[{"server_name":"private-nested.example"}]}}}]});
    let report = crate::diagnostic_report::assemble_diagnostic_report(
        &crate::diagnostic_report::DiagnosticReportSource {
            user_config: config.clone(),
            singbox_config: config,
            ..Default::default()
        },
    );
    assert!(!report.contains("private-cover.example"));
    assert!(!report.contains("private-nested.example"));
    assert!(report.contains("public-label"));
}
