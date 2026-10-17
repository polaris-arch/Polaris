use super::*;
#[test]
fn ports_reject_lossy_numbers_and_keep_valid_boundaries() {
    for raw in [
        "0",
        "65536",
        "4294967297",
        "18446744073709551615",
        "1.5",
        "-1",
        ".inf",
        ".nan",
    ] {
        let value: Value = serde_yaml::from_str(raw).unwrap();
        assert_eq!(port(&value), None, "{raw}");
        let proxies: Value = serde_yaml::from_str(&format!("- {{name: node, type: ss, server: host.example, port: {raw}, cipher: aes-256-gcm, password: pw}}" )).unwrap();
        assert!(
            parse_clash_proxies(&proxies, "sub", "now", &mut || "id".into())
                .servers
                .is_empty(),
            "{raw}"
        );
    }
    for raw in ["1", "65535", "443.0", "'443'"] {
        assert!(port(&serde_yaml::from_str(raw).unwrap()).is_some(), "{raw}");
    }
    assert_eq!(num(&serde_yaml::from_str("4294967296").unwrap()), None);
}
