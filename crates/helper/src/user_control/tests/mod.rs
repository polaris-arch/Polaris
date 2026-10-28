use super::*;
#[test]
fn registration_requires_one_complete_native_config_token_record() {
    let id = CallerIdentity::Unix(1000);
    let config = std::env::temp_dir().join("polaris-inert-config");
    let token = "a".repeat(64);
    let raw = format!("u1000\n{}\n{token}\n", config.display());
    assert_eq!(parse_registration(&raw, &id).unwrap(), (config, token));
    for bad in [
        raw.trim_end().to_owned(),
        raw.replace("u1000", "u1001"),
        raw.replace(&"a".repeat(64), "not-hex"),
        format!("{raw}extra\n"),
        raw.replace("polaris-inert-config", "polaris\rconfig"),
    ] {
        assert!(parse_registration(&bad, &id).is_err());
    }
    assert!(parse_identity("u01000").is_none());
    assert!(parse_identity("sS-1-5-21-1-2-3-01000").is_none());
}
