use super::*;

#[test]
fn pending_node_edits_cannot_disclose_running_hosts_or_auth_headers() {
    let report = assemble_diagnostic_report(&DiagnosticReportSource {
        user_config: json!({"servers": [{"address": "new-node.example", "name": "New node"}]}),
        singbox_config: json!({
            "outbounds": [{"type": "vless", "tag": "Old private node", "server": "old-node.example", "uuid": "synthetic-uuid",
                "tls": {"server_name": "old-cover.example"},
                "transport": {"headers": {"Host": ["old-host.example"],
                    "Authorization": ["synthetic-auth"], "Proxy-Authorization": "synthetic-proxy-auth",
                    "Cookie": "synthetic-cookie", "Set-Cookie": ["synthetic-set-cookie"],
                    "X-Diagnostic": "keep-label"}}}],
            "endpoints": [{"type": "masque-client", "server": "203.0.113.25", "version": 3,
                "tls": {"server_name": "old-endpoint.example"}, "path": "/masque"}],
            "dns": {"servers": [{"server": "technical-dns.example"}]},
            "route": {"rules": [{"domain": ["technical-rule.example"]}]}
        }),
        app_log_tail: "dial old-node.example via old-cover.example".into(),
        singbox_log_tail: "dial 203.0.113.25 old-endpoint.example old-host.example".into(),
        ..Default::default()
    });
    for private in [
        "new-node.example",
        "old-node.example",
        "old-cover.example",
        "old-host.example",
        "203.0.113.25",
        "old-endpoint.example",
        "Old private node",
        "synthetic-uuid",
        "synthetic-auth",
        "synthetic-proxy-auth",
        "synthetic-cookie",
        "synthetic-set-cookie",
    ] {
        assert!(
            !report.contains(private),
            "private sentinel survived redaction"
        );
    }
    for technical in [
        "technical-dns.example",
        "technical-rule.example",
        "keep-label",
        "/masque",
        "vless",
        "masque-client",
    ] {
        assert!(
            report.contains(technical),
            "technical diagnostic field was lost"
        );
    }
}

#[test]
fn deleted_disk_node_and_empty_disk_config_still_redact_running_ipv6() {
    let report = assemble_diagnostic_report(&DiagnosticReportSource {
        user_config: json!({"servers": []}),
        singbox_config: json!({"outbounds": [{"type": "socks", "server": "2001:db8::42"}]}),
        singbox_log_tail: "dial [2001:db8::42]:443".into(),
        ..Default::default()
    });
    assert!(
        !report.contains("2001:db8::42"),
        "private sentinel survived redaction"
    );
    assert_eq!(report.matches("<ip-1>").count(), 2);
}

#[test]
fn structured_standard_auth_headers_are_case_and_separator_independent() {
    let config = json!({"servers": [{"masqueClientSettings": {"headers": {
        "aUtHoRiZaTiOn": "synthetic-a", "proxy_authorization": ["synthetic-b"],
        "COOKIE": "synthetic-c", "set_cookie": ["synthetic-d"], "Accept": "application/json"
    }}}]});
    let report = assemble_diagnostic_report(&DiagnosticReportSource {
        user_config: config,
        ..Default::default()
    });
    for secret in ["synthetic-a", "synthetic-b", "synthetic-c", "synthetic-d"] {
        assert!(
            !report.contains(secret),
            "private sentinel survived redaction"
        );
    }
    assert!(report.contains("application/json"));
}

#[test]
fn removed_saved_custom_node_keeps_applied_secret_declarations() {
    let applied = json!({"servers": [{"protocol": "custom", "customSettings": {
        "secretKeys": ["tenantCredential"], "outbound": {"tenantCredential": "private-value"}
    }}]});
    let report = assemble_diagnostic_report(&DiagnosticReportSource {
        user_config: json!({"servers": []}),
        running_secret_keys: crate::redact::collect_custom_secret_keys(&applied),
        singbox_config: json!({"outbounds": [{"type": "custom", "tenantCredential": "private-value", "technicalFlag": true}]}),
        ..Default::default()
    });
    assert!(!report.contains("private-value"));
    assert!(report.contains("technicalFlag"));
}

#[test]
fn unmatched_runtime_omits_values_but_still_masks_old_identity_in_logs() {
    let report = assemble_diagnostic_report(&DiagnosticReportSource {
        user_config: json!({"servers": []}),
        singbox_config: json!({"outbounds": [{"server": "old-private.example", "unknownCredential": "private-value"}]}),
        unverified_runtime: true,
        singbox_log_tail: "dial old-private.example".into(),
        ..Default::default()
    });
    assert!(!report.contains("private-value"));
    assert!(!report.contains("old-private.example"));
    assert!(report.contains("已省略该段"));
}
