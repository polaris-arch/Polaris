//! Pure archive/ABI rejection controls. Synthetic headers below are negative
//! metadata fixtures and never claim a real backend, source receipt or wire PASS.
include!("../proto_wire_check.rs");
include!("../android_wire_check.rs");

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "polaris-android-wire-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn archive(path: &Path, bytes: &[u8], entry: &str, duplicate: bool, deflate: bool) {
    let payload = path.with_extension("bytes");
    std::fs::write(&payload, bytes).unwrap();
    let result = Command::new(if cfg!(windows) { "python" } else { "python3" })
        .args([
            "-c",
            r#"
import sys, warnings, zipfile
warnings.simplefilter('ignore', UserWarning)
compression = zipfile.ZIP_DEFLATED if sys.argv[5] == 'deflate' else zipfile.ZIP_STORED
with zipfile.ZipFile(sys.argv[1], 'w', compression=compression) as archive:
    data = open(sys.argv[2], 'rb').read()
    archive.writestr(sys.argv[3], data)
    if sys.argv[4] == 'duplicate':
        archive.writestr(sys.argv[3], data)
"#,
        ])
        .arg(path)
        .arg(&payload)
        .arg(entry)
        .arg(if duplicate { "duplicate" } else { "single" })
        .arg(if deflate { "deflate" } else { "stored" })
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn header(class: u8, machine: u16) -> Vec<u8> {
    let mut bytes = vec![0; 64];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = class;
    bytes[5] = 1;
    bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes
}

#[test]
fn unknown_target_and_missing_archive_are_rejected() {
    let dir = Scratch::new();
    let path = dir.0.join("missing.aar");
    assert!(android_wire_check::check_android_aar(&path, "riscv64")
        .unwrap_err()
        .contains("unsupported Android Cargo target"));
    assert!(android_wire_check::check_android_aar(&path, "aarch64")
        .unwrap_err()
        .contains("AAR JNI read failed"));
}

#[test]
fn selected_entry_is_required_and_duplicates_are_rejected() {
    let dir = Scratch::new();
    let path = dir.0.join("entries.aar");
    archive(&path, b"negative fixture", "jni/x86/libbox.so", false, true);
    assert!(android_wire_check::check_android_aar(&path, "aarch64")
        .unwrap_err()
        .contains("exist exactly once"));
    archive(
        &path,
        b"negative fixture",
        "jni/arm64-v8a/libbox.so",
        true,
        true,
    );
    assert!(android_wire_check::check_android_aar(&path, "aarch64")
        .unwrap_err()
        .contains("exist exactly once"));
}

#[test]
fn corrupt_zip_crc_is_rejected_before_elf_or_descriptor_validation() {
    let dir = Scratch::new();
    let path = dir.0.join("crc.aar");
    let payload = b"unique-negative-crc-fixture";
    archive(&path, payload, "jni/arm64-v8a/libbox.so", false, false);
    let mut bytes = std::fs::read(&path).unwrap();
    let start = bytes
        .windows(payload.len())
        .position(|part| part == payload)
        .unwrap();
    bytes[start] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(android_wire_check::check_android_aar(&path, "aarch64")
        .unwrap_err()
        .contains("Bad CRC"));
}

#[test]
fn wrong_magic_class_endianness_type_or_machine_is_rejected() {
    let dir = Scratch::new();
    let path = dir.0.join("abi.aar");
    for (offset, value) in [(0, 0), (4, 1), (5, 2), (16, 2), (18, 62)] {
        let mut bytes = header(2, 183);
        bytes[offset] = value;
        archive(&path, &bytes, "jni/arm64-v8a/libbox.so", false, true);
        assert!(
            android_wire_check::check_android_aar(&path, "aarch64")
                .unwrap_err()
                .contains("ELF shared-library ABI differs"),
            "offset={offset}"
        );
    }
}

#[test]
fn all_target_abis_require_an_observable_descriptor_after_elf_matches() {
    let dir = Scratch::new();
    let path = dir.0.join("no-descriptor.aar");
    for (arch, abi, class, machine) in [
        ("aarch64", "arm64-v8a", 2, 183),
        ("arm", "armeabi-v7a", 1, 40),
        ("x86", "x86", 1, 3),
        ("x86_64", "x86_64", 2, 62),
    ] {
        archive(
            &path,
            &header(class, machine),
            &format!("jni/{abi}/libbox.so"),
            false,
            true,
        );
        assert!(
            android_wire_check::check_android_aar(&path, arch)
                .unwrap_err()
                .contains("wire descriptor unavailable"),
            "target={arch}"
        );
    }
}

#[test]
fn build_script_routes_by_cargo_target_to_the_fixed_gradle_aar() {
    let source =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs")).unwrap();
    let android = source.split("// 覆盖轴").next().unwrap();
    assert!(
        android.contains("std::env::var(\"CARGO_CFG_TARGET_OS\").as_deref() == Ok(\"android\")")
    );
    assert!(android.contains("std::env::var(\"CARGO_CFG_TARGET_ARCH\")"));
    assert!(android.contains("src-tauri/gen/android/app/libs/libbox.aar"));
    assert!(android.contains("cargo:rerun-if-changed={}\", aar.display()"));
    assert!(android.contains("android_wire_check::check_android_aar(&aar, &arch)"));
    assert!(android.contains("PROFILE\").as_deref() == Ok(\"release\")"));
    assert!(android.contains("panic!(\"Android Release wire guard refused:"));
    assert!(android.contains("        return;"));
}
