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
