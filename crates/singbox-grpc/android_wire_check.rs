// Build-time Android backend wire check. Shared by build.rs and pure tests;
// the existing component verifier owns source/input/BuildInfo admission, and
// APK consumption binds the packaged library to that verified AAR's bytes.
#[allow(dead_code)]
mod android_wire_check {
    use super::proto_wire_check::{repo_root, verdict_for_core_bytes, WireVerdict};
    use std::path::Path;
    use std::process::Command;

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
        // The shared Android ZIP reader returns only the checked JNI bytes.
        let output = Command::new("node")
            .arg(repo_root().join("scripts/verify-apk.mjs"))
            .arg("--aar-wire-member")
            .arg(aar)
            .arg(abi)
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
