// The formal App builder verifies the exact component slice/source/BuildID
// before Cargo. Check that same static input against the existing wire table;
// iOS must never fall back to a desktop executable during release compilation.
#[allow(dead_code)]
mod apple_wire_check {
    use super::proto_wire_check::{WireVerdict, verdict_for_core_bytes};
    use std::path::Path;

    pub fn check_bytes(bytes: &[u8]) -> Result<(), String> {
        if !bytes.starts_with(b"!<arch>\n") || bytes.len() > 1024 * 1024 * 1024 {
            return Err("actual bounded Apple static archive required".into());
        }
        match verdict_for_core_bytes(bytes) {
            WireVerdict::Match => Ok(()),
            WireVerdict::Mismatch(report) => Err(format!("Apple static wire mismatch: {report}")),
            WireVerdict::Unobservable(report) => {
                Err(format!("Apple static descriptor unavailable: {report}"))
            }
        }
    }

    pub fn check_apple_archive(path: &Path) -> Result<(), String> {
        let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > 1024 * 1024 * 1024 {
            return Err("regular bounded selected Apple archive required".into());
        }
        check_bytes(&std::fs::read(path).map_err(|error| error.to_string())?)
    }
}
