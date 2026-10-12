//! Administrative version/byte receipt. No installed executable is invoked.
use crate::{core_objects::PayloadDir, core_store::OwnerPolicy};
use polaris_helper_proto::core_manifest::InstalledHelperRecord;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;

pub fn may_replace(
    version: &str,
    protocol: u32,
    candidate: &str,
    candidate_protocol: u32,
) -> io::Result<bool> {
    let old = semver::Version::parse(version)
        .map_err(|_| io::Error::other("invalid installed helper version"))?;
    let new = semver::Version::parse(candidate)
        .map_err(|_| io::Error::other("invalid candidate helper version"))?;
    Ok(old <= new && protocol <= candidate_protocol)
}
fn hash(path: &Path) -> io::Result<String> {
    use sha2::Digest;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("helper parent unavailable"))?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("invalid helper path"))?;
    let owner = OwnerPolicy::privileged();
    owner.check(parent)?;
    owner.check(path)?;
    let objects = PayloadDir::open(parent, owner.uid)?;
    let (file, length) = objects.open_regular(name)?;
    if length == 0 || length > 512 * 1024 * 1024 {
        return Err(io::Error::other("invalid helper size"));
    }
    let mut file = file.take(length + 1);
    let mut hash = sha2::Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        hash.update(&buffer[..count]);
    }
    if bytes != length {
        return Err(io::Error::other("helper changed size"));
    }
    Ok(hex::encode(hash.finalize()))
}
pub fn policy(binary: &Path, metadata: &Path) -> io::Result<&'static str> {
    if !binary.try_exists()? {
        return Ok("replace");
    }
    let actual = hash(binary)?;
    if !metadata.try_exists()? {
        return Ok("replace-legacy");
    }
    let owner = OwnerPolicy::privileged();
    let parent = metadata
        .parent()
        .ok_or_else(|| io::Error::other("receipt parent unavailable"))?;
    let name = metadata
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("invalid receipt basename"))?;
    owner.check(parent)?;
    owner.check(metadata)?;
    let objects = PayloadDir::open(parent, owner.uid)?;
    let (file, bytes) = objects.open_regular(name)?;
    if bytes > 4096 {
        return Err(io::Error::other("helper receipt too large"));
    }
    let mut raw = Vec::new();
    file.take(4097).read_to_end(&mut raw)?;
    let record = InstalledHelperRecord::parse(&raw).map_err(io::Error::other)?;
    if record.sha256 != actual {
        return Err(io::Error::other("installed helper receipt differs"));
    }
    if may_replace(
        &record.version,
        record.protocol,
        env!("CARGO_PKG_VERSION"),
        polaris_helper_proto::proto_version::CURRENT,
    )? {
        Ok("replace")
    } else {
        Ok("retain")
    }
}
pub fn record(binary: &Path, metadata: &Path) -> io::Result<()> {
    let record = InstalledHelperRecord {
        schema: "polaris-helper-installed-v1".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: polaris_helper_proto::proto_version::CURRENT,
        build_identity: polaris_helper_proto::build_identity::current().into(),
        sha256: hash(binary)?,
    };
    let parent = metadata
        .parent()
        .ok_or_else(|| io::Error::other("helper metadata parent unavailable"))?;
    OwnerPolicy::privileged().check(parent)?;
    let temporary = parent.join(format!(".helper-record-{}.new", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(&record.to_json().map_err(io::Error::other)?)?;
        file.sync_all()?;
        #[cfg(windows)]
        crate::platform::windows::winproc::seal_store_object(&temporary, false)?;
        fs::rename(&temporary, metadata)
    })();
    if temporary.try_exists()? {
        let _ = fs::remove_file(temporary);
    }
    result
}
#[cfg(test)]
mod tests;
