use super::*;
fn linux() -> CoreManifest {
    CoreManifest {
        platform: "linux".into(),
        version: "1.15.0-alpha.12.polaris.1".into(),
        provider_fingerprint: "a".repeat(64),
        helper_sha256: None,
        files: vec![
            CoreFile {
                name: "sing-box".into(),
                bytes: 10,
                sha256: "b".repeat(64),
            },
            CoreFile {
                name: "libcronet.so".into(),
                bytes: 20,
                sha256: "c".repeat(64),
            },
        ],
    }
}
#[test]
fn canonical_order_and_install_metadata_do_not_change_core_identity() {
    let first = linux();
    let mut second = first.clone();
    second.files.reverse();
    second.helper_sha256 = Some("d".repeat(64));
    assert_eq!(first.canonical_bytes(), second.canonical_bytes());
    second.files[0].bytes += 1;
    assert_ne!(first.canonical_bytes(), second.canonical_bytes());
}
#[test]
fn malformed_or_incomplete_payload_cannot_be_named_as_a_core() {
    for name in [
        "../sing-box",
        "libcronet.dll.",
        "polaris-helper",
        "sing-box.exe",
    ] {
        let mut value = linux();
        value.files[0].name = name.into();
        assert!(value.validate().is_err());
    }
    let mut value = linux();
    value.files.pop();
    assert!(value.validate().is_err());
    assert!(CoreId::parse(&"A".repeat(64)).is_err());
}
