use super::*;

#[tokio::test]
async fn oversized_unterminated_line_is_bounded_and_tail_fatal_is_retained() {
    let mut input = vec![b'x'; CORE_LOG_LINE_LIMIT * 8];
    input.extend_from_slice(b" FATAL configure tun interface: set ipv4 address");
    let mut reader = BufReader::with_capacity(7, input.as_slice());
    let record = read_bounded_core_log_line(&mut reader)
        .await
        .unwrap()
        .unwrap();
    assert!(record.truncated);
    assert_eq!(record.bytes.len(), CORE_LOG_LINE_LIMIT);
    assert_eq!(
        record.fatal_facts.classify(),
        Some(CoreFatalKind::TunAddressUnavailable)
    );
    assert!(read_bounded_core_log_line(&mut reader)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn oversized_line_does_not_join_secrets_or_fatal_tokens_with_next_line() {
    let mut input = vec![b'x'; CORE_LOG_LINE_LIMIT + 1];
    input.extend_from_slice(b"secret-prefix ERROR\nmissing monitor for auto DHCP\n");
    let mut reader = BufReader::with_capacity(3, input.as_slice());
    let first = read_bounded_core_log_line(&mut reader)
        .await
        .unwrap()
        .unwrap();
    assert!(first.truncated);
    assert_eq!(first.fatal_facts.classify(), None);
    let next = read_bounded_core_log_line(&mut reader)
        .await
        .unwrap()
        .unwrap();
    assert!(!next.truncated);
    assert_eq!(next.fatal_facts.classify(), None);
    assert_eq!(
        redact_known_process_secrets("secret-prefix-suffix", &["secret-prefix-suffix".into()]),
        "[REDACTED]"
    );
}

#[tokio::test]
async fn bounded_lines_preserve_invalid_utf8_crlf_and_eof() {
    let mut reader = BufReader::with_capacity(1, &b"a\xff\r\nlast"[..]);
    let first = read_bounded_core_log_line(&mut reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.bytes, b"a\xff");
    let last = read_bounded_core_log_line(&mut reader)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(last.bytes, b"last");
    assert!(!last.truncated);
    assert!(read_bounded_core_log_line(&mut reader)
        .await
        .unwrap()
        .is_none());
}

#[test]
fn relay_log_policy_tracks_committed_runtime_not_saved_pending_desire() {
    use polaris_helper_proto::Platform;
    for mode in ["manual", "systemProxy", "tun"] {
        let (runtime, _dir) = crate::runtime::proxy::tests::test_runtime_on(Platform::Linux);
        let generation = runtime.gate.generation();
        let mut running = polaris_store::default_config();
        running["proxyModeType"] = serde_json::json!(mode);
        running["disableLogFile"] = serde_json::json!(true);
        *runtime.current_config.write().unwrap() = Some(running.clone());
        let mut saved = running.clone();
        saved["disableLogFile"] = serde_json::json!(false);
        runtime.config.save_full(&saved).unwrap();
        assert!(
            !runtime.core_log_relay_enabled(generation),
            "saving D cannot enable {mode} logs"
        );
        running["disableLogFile"] = serde_json::json!(false);
        *runtime.current_config.write().unwrap() = Some(running.clone());
        saved["disableLogFile"] = serde_json::json!(true);
        runtime.config.save_full(&saved).unwrap();
        assert!(
            runtime.core_log_relay_enabled(generation),
            "unapplied D cannot disable running {mode}"
        );
        running["disableLogFile"] = serde_json::json!(true);
        *runtime.current_config.write().unwrap() = Some(running.clone());
        assert!(!runtime.core_log_relay_enabled(generation));
        running["disableLogFile"] = serde_json::json!(false);
        *runtime.current_config.write().unwrap() = Some(running);
        assert!(runtime.core_log_relay_enabled(generation));
        assert!(
            !runtime.core_log_relay_enabled(generation.wrapping_add(1)),
            "foreign runtime generation admitted"
        );
        *runtime.current_config.write().unwrap() = None;
        assert!(
            !runtime.core_log_relay_enabled(generation),
            "missing running snapshot is not emission authority"
        );
    }
}
