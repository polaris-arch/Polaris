use super::*;

#[test]
fn source_admission_is_literal_loopback_without_redirect_authority() {
    let token = "a".repeat(64);
    assert!(parse_source("http://127.0.0.1:38111", &token).is_ok());
    for origin in [
        "https://127.0.0.1:38111",
        "http://localhost:38111",
        "http://127.0.0.1:80",
        "http://user@127.0.0.1:38111",
        "http://127.0.0.1:38111/other",
        "http://127.0.0.1:38111/?x=y",
        "http://example.com:38111",
    ] {
        assert!(parse_source(origin, &token).is_err(), "{origin}");
    }
    assert!(parse_source("http://127.0.0.1:38111", "short").is_err());
}

#[test]
fn asset_transport_is_scoped_to_original_app_release_url() {
    let prefix = "https://github.com/polaris-arch/Polaris/releases/download/";
    assert!(asset_path(&format!(
        "{prefix}v1.0.1/Polaris_1.0.1_x64-win-Portable.zip"
    ))
    .is_ok());
    for path in [
        "v1/../x",
        "../x",
        "v1/%2e%2e",
        "v1/x?other=1",
        "v1/",
        "v1/x/y",
    ] {
        assert!(asset_path(&format!("{prefix}{path}")).is_err());
    }
    assert!(asset_path("http://127.0.0.1:38111/asset").is_err());
}
