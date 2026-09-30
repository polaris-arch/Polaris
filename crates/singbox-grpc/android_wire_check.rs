// Build-time Android backend wire check. Shared by build.rs and pure tests;
// the existing component verifier owns source/input/BuildInfo admission, and
// APK consumption binds the packaged library to that verified AAR's bytes.
#[allow(dead_code)]
mod android_wire_check {
    use super::proto_wire_check::{verdict_for_core_bytes, WireVerdict};
    use std::path::Path;
    use std::process::Command;

    // Python3 is already required by the Android component verifier. Only the
    // fixed archive entry is read; stdout contains library bytes exclusively.
    const READ_LIBRARY: &str = r#"
import sys, zipfile
try:
    with zipfile.ZipFile(sys.argv[1]) as archive:
        entries = [item for item in archive.infolist() if item.filename == sys.argv[2]]
        if len(entries) != 1:
            raise ValueError('selected JNI entry must exist exactly once')
        entry = entries[0]
        if entry.is_dir() or not 0 < entry.file_size <= 256 * 1024 * 1024:
            raise ValueError('selected JNI library size/type is invalid')
        data = archive.read(entry)  # Complete read also validates the ZIP CRC.
        if len(data) != entry.file_size:
            raise ValueError('selected JNI library length differs')
    sys.stdout.buffer.write(data)
except Exception as error:
    print('AAR JNI read failed: ' + str(error), file=sys.stderr)
    sys.exit(1)
"#;

    /// Check the exact target ABI library. Neither descriptor absence nor a
    /// different wire layout permits falling back to a desktop binary.
    pub fn check_android_aar(aar: &Path, target_arch: &str) -> Result<(), String> {
        let (abi, class, machine) = match target_arch {
            "aarch64" => ("arm64-v8a", 2, 183u16),
            "arm" => ("armeabi-v7a", 1, 40),
            "x86" => ("x86", 1, 3),
            "x86_64" => ("x86_64", 2, 62),
            other => {
                return Err(format!(
                    "unsupported Android Cargo target architecture: {other}"
                ))
            }
        };
        let entry = format!("jni/{abi}/libbox.so");
        // This cfg selects the host interpreter executable only. ABI selection
        // above always uses the explicit Cargo target architecture.
        let interpreter = if cfg!(windows) { "python" } else { "python3" };
        let output = Command::new(interpreter)
            .arg("-c")
            .arg(READ_LIBRARY)
            .arg(aar)
            .arg(&entry)
            .output()
            .map_err(|error| format!("cannot read {} {entry}: {error}", aar.display()))?;
        if !output.status.success() || !output.stderr.is_empty() {
            return Err(format!(
                "{} {entry}: archive reader {}: {}",
                aar.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let bytes = output.stdout;
        if bytes.len() < 20
            || &bytes[..4] != b"\x7fELF"
            || bytes[4] != class
            || bytes[5] != 1
            || u16::from_le_bytes([bytes[16], bytes[17]]) != 3 // ET_DYN, a JNI shared library.
            || u16::from_le_bytes([bytes[18], bytes[19]]) != machine
        {
            return Err(format!(
                "{entry}: ELF shared-library ABI differs from Cargo target {target_arch}"
            ));
        }
        match verdict_for_core_bytes(&bytes) {
            WireVerdict::Match => Ok(()),
            WireVerdict::Mismatch(report) => Err(format!("{entry}: wire mismatch: {report}")),
            WireVerdict::Unobservable(report) => {
                Err(format!("{entry}: wire descriptor unavailable: {report}"))
            }
        }
    }
}
