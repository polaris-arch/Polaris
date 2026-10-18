use super::*;
#[test]
fn only_complete_empty_observations_attest_idle() {
    assert_eq!(classify_unix(1, b"", b""), LegacyUse::Idle);
    assert_eq!(classify_unix(0, b"", b""), LegacyUse::Idle);
    for (code, out, err) in [
        (1, &b""[..], &b"permission denied"[..]),
        (0, &b"p42\nn/path"[..], &b"partial scan"[..]),
        (2, &b""[..], &b""[..]),
        (0, &b"n/path"[..], &b""[..]),
    ] {
        assert_eq!(classify_unix(code, out, err), LegacyUse::Unknown);
    }
    assert_eq!(classify_unix(0, b"p42\nn/path\n", b""), LegacyUse::InUse);
}
