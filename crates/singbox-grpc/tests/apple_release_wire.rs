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

fn object_archive(cpu: u32, subtype: u32, platform: u32) -> Vec<u8> {
    let mut object = b"\xcf\xfa\xed\xfe".to_vec();
    for value in [
        cpu,
        subtype,
        1,
        1,
        24,
        0,
        0,
        0x32,
        24,
        platform,
        15 << 16,
        27 << 16,
        0,
    ] {
        object.extend_from_slice(&value.to_le_bytes());
    }
    let name = b"go.o";
    let size = name.len() + object.len();
    let mut archive = b"!<arch>\n".to_vec();
    archive.extend_from_slice(
        format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            "#1/4", 0, 0, 0, "644", size
        )
        .as_bytes(),
    );
    archive.extend_from_slice(name);
    archive.extend_from_slice(&object);
    if !size.is_multiple_of(2) {
        archive.push(b'\n');
    }
    archive
}

#[test]
fn selected_slice_must_match_cargo_cpu_and_platform_before_descriptor_scan() {
    for (target, cpu, subtype, platform) in [
        ("aarch64-apple-ios", 0x0100_000c, 0, 2),
        ("aarch64-apple-ios-sim", 0x0100_000c, 0, 7),
        ("x86_64-apple-ios", 0x0100_0007, 3, 7),
    ] {
        let actual = object_archive(cpu, subtype, platform);
        assert!(apple_wire_check::check_bytes_for_target(&actual, target)
            .unwrap_err()
            .contains("descriptor unavailable"));
        let wrong_cpu = object_archive(
            if cpu == 0x0100_000c {
                0x0100_0007
            } else {
                0x0100_000c
            },
            subtype,
            platform,
        );
        assert!(apple_wire_check::check_bytes_for_target(&wrong_cpu, target)
            .unwrap_err()
            .contains("CPU/subtype/type"));
        let wrong_platform = object_archive(cpu, subtype, if platform == 2 { 7 } else { 2 });
        assert!(
            apple_wire_check::check_bytes_for_target(&wrong_platform, target)
                .unwrap_err()
                .contains("platform differs")
        );
    }
}

#[test]
fn a_whole_fat_container_cannot_supply_the_first_architectures_descriptor() {
    let mut raw = b"\xca\xfe\xba\xbe".to_vec();
    raw.extend_from_slice(&object_archive(0x0100_000c, 0, 7));
    assert!(
        apple_wire_check::check_bytes_for_target(&raw, "x86_64-apple-ios")
            .unwrap_err()
            .contains("selected thin")
    );
}

#[test]
fn truncated_or_unsupported_target_cannot_pass_selected_slice_guard() {
    let mut archive = object_archive(0x0100_000c, 0, 2);
    archive.pop();
    assert!(apple_wire_check::check_bytes_for_target(&archive, "aarch64-apple-ios").is_err());
    assert!(apple_wire_check::check_bytes_for_target(&archive, "aarch64-apple-darwin").is_err());
}
