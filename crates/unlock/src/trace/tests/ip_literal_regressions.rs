use super::*;
#[test]
fn ipv6_trace_accepts_valid_embedded_ipv4_and_rejects_stray_colons() {
    for ip in [
        "::ffff:192.0.2.1",
        "2001:db8::192.0.2.1",
        "::",
        "2001:db8::1",
    ] {
        assert!(is_valid_ip(ip));
        assert!(parse_trace(&format!("ip={ip}\nloc=US\n")).is_some());
    }
    for ip in [
        ":1::2",
        "1::2:",
        ":::1",
        "1:2:3:4:5:6:7:8:",
        "::ffff:999.0.0.1",
    ] {
        assert!(!is_valid_ip(ip));
        assert!(parse_trace(&format!("ip={ip}\nloc=US\n")).is_none());
    }
}
