//! Ordinary App process on macOS/Windows: one correctness hash of every selected
//! member against the compiled manifest, then execution by path. Not a security
//! boundary: a same-user writer can replace the App itself.
use super::*;
use std::io::Read;

#[derive(Debug)]
pub(super) struct DirectMembers {
    binary: PathBuf,
    stamps: Vec<(PathBuf, std::fs::Metadata)>,
}

fn unchanged(path: &Path, before: &std::fs::Metadata) -> bool {
    std::fs::metadata(path)
        .is_ok_and(|now| now.len() == before.len() && now.modified().ok() == before.modified().ok())
}

pub(super) fn protect_members(
    input: &UntrustedPayload,
    policy: &SelectedPolicy,
) -> Result<DirectMembers, AdmissionError> {
    use sha2::Digest;
    let directory = input.binary.parent().ok_or_else(|| {
        AdmissionError::new(AdmissionErrorKind::Mismatch, "missing member directory")
    })?;
    if !policy
        .manifest
        .files
        .iter()
        .any(|f| input.binary.file_name() == Some(std::ffi::OsStr::new(&f.name)))
    {
        return Err(AdmissionError::new(
            AdmissionErrorKind::Mismatch,
            "unexpected selected executable name",
        ));
    }
    let mut stamps = Vec::with_capacity(policy.manifest.files.len());
    for expected in &policy.manifest.files {
        let path = directory.join(&expected.name);
        let file = std::fs::File::open(&path).map_err(|error| {
            let mut error = AdmissionError::from(error);
            error.member = Some(expected.name.clone());
            error
        })?;
        let metadata = file.metadata()?;
        let mut hash = sha2::Sha256::new();
        let mut source = file.take(expected.bytes + 1);
        let mut buffer = [0u8; 64 * 1024];
        let mut length = 0u64;
        loop {
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            length += count as u64;
            hash.update(&buffer[..count]);
        }
        if length != expected.bytes || lowercase_hex(&hash.finalize()) != expected.sha256 {
            let mut error = AdmissionError::new(
                AdmissionErrorKind::Mismatch,
                "selected member differs from compiled Core",
            );
            error.member = Some(expected.name.clone());
            return Err(error);
        }
        if !unchanged(&path, &metadata) {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Changed,
                "selected member changed while hashing",
            ));
        }
        stamps.push((path, metadata));
    }
    Ok(DirectMembers {
        binary: input.binary.clone(),
        stamps,
    })
}

impl DirectMembers {
    pub(super) fn is_current(&self) -> bool {
        self.stamps
            .iter()
            .all(|(path, before)| unchanged(path, before))
    }
    pub(super) fn command(&self) -> tokio::process::Command {
        // Core loads its sidecar from its own directory first, which is the
        // hashed member above.
        tokio::process::Command::new(&self.binary)
    }
}
