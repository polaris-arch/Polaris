//! Static-input rejection controls; fixtures do not claim native/device proof.
include!("../proto_wire_check.rs");
include!("../apple_wire_check.rs");

#[test]
fn apple_release_does_not_accept_desktop_or_jni_inputs() {
    for raw in [
        b"\x7fELFnegative".as_slice(),
        b"\xcf\xfa\xed\xfedesktop".as_slice(),
        b"".as_slice(),
    ] {
        assert!(apple_wire_check::check_bytes(raw)
            .unwrap_err()
            .contains("static archive required"));
    }
}

#[test]
fn archive_magic_without_real_wire_descriptor_is_not_pass() {
    let result = apple_wire_check::check_bytes(b"!<arch>\ninert archive without a descriptor");
    assert!(result.unwrap_err().contains("descriptor unavailable"));
}

#[test]
fn missing_actual_selected_archive_is_an_error() {
    let path =
        std::env::temp_dir().join(format!("polaris-missing-apple-wire-{}", std::process::id()));
    assert!(apple_wire_check::check_apple_archive(&path).is_err());
}
