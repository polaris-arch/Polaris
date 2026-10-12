//! B3 immutable content store. Runtime callers select an installed id, never source bytes.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core_objects::PayloadDir;
use polaris_helper_proto::core_manifest::{CoreId, InstalledCoreManifest};

static STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
pub struct OwnerPolicy {
    pub uid: Option<u32>,
}
impl OwnerPolicy {
    pub const fn privileged() -> Self {
        Self { uid: Some(0) }
    }
    pub(crate) fn check(self, path: &Path) -> io::Result<()> {
        let meta = fs::symlink_metadata(path)?;
        if meta.file_type().is_symlink() {
            return Err(invalid("linked store object"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.uid() != self.uid.ok_or_else(|| invalid("store owner unavailable"))?
                || meta.mode() & 0o022 != 0
            {
                return Err(invalid("untrusted store owner or mode"));
            }
        }
        #[cfg(windows)]
        {
            use crate::platform::windows::{coreacl, ops::ProcOps, winproc::WinProcOps};
            let name = path.to_str().ok_or_else(|| invalid("invalid store path"))?;
            let sec = WinProcOps::default()
                .read_object_security(name)
                .map_err(|_| invalid("store ACL unavailable"))?;
            if !coreacl::judge_object(name, &sec).is_empty() {
                return Err(invalid("untrusted store ACL"));
            }
        }
        #[cfg(not(any(unix, windows)))]
        {
            return Err(invalid("unsupported store platform"));
        }
        Ok(())
    }
}

pub struct CoreStore {
    root: PathBuf,
    _root_object: PayloadDir,
    owner: OwnerPolicy,
}
pub struct VerifiedCore {
    pub manifest: InstalledCoreManifest,
    pub binary: File,
    pub sidecars: Vec<File>,
    pub directory: PathBuf,
    _directory_object: PayloadDir,
    _lease: File,
}
impl std::fmt::Debug for VerifiedCore {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedCore")
            .field("id", &self.manifest.id)
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    AlreadyPresent,
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}
fn private_dir(path: &Path) -> io::Result<()> {
    fs::create_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(windows)]
    crate::platform::windows::winproc::seal_store_object(path, false)?;
    Ok(())
}
fn seal_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(windows)]
    crate::platform::windows::winproc::seal_store_object(path, true)?;
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

impl CoreStore {
    fn lease_file(&self, id: &CoreId) -> io::Result<File> {
        let dir = self.root.join("leases");
        self.owner.check(&dir)?;
        let objects = PayloadDir::open(&dir, self.owner.uid)?;
        let path = dir.join(format!("{}.lock", id.as_str()));
        self.owner.check(&path)?;
        let (file, _) = objects.open_regular(&format!("{}.lock", id.as_str()))?;
        Ok(file)
    }
    fn mutation_lock(&self) -> io::Result<File> {
        let path = self.root.join(".core-store.lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.custom_flags(
                windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT,
            );
        }
        let file = options.open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("invalid store lock"));
        }
        #[cfg(windows)]
        crate::platform::windows::winproc::seal_store_object(&path, false)?;
        self.owner.check(&path)?;
        file.try_lock()
            .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
        Ok(file)
    }
    pub fn open(root: &Path, owner: OwnerPolicy) -> io::Result<Self> {
        owner.check(root)?;
        let object = PayloadDir::open(root, owner.uid)?;
        for name in ["versions", "manifests", ".staging", ".trash", "leases"] {
            let child = root.join(name);
            owner.check(&child)?;
            let _ = PayloadDir::open(&child, owner.uid)?;
        }
        Ok(Self {
            root: root.to_path_buf(),
            _root_object: object,
            owner,
        })
    }

    /// Administrative installer only. Runtime status/open never creates directories.
    pub fn initialize(root: &Path, owner: OwnerPolicy) -> io::Result<Self> {
        owner.check(root)?;
        let _root = PayloadDir::open(root, owner.uid)?;
        for name in ["versions", "manifests", ".staging", ".trash", "leases"] {
            let child = root.join(name);
            match fs::create_dir(&child) {
                Ok(()) => {
                    private_mode(&child, 0o755)?;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
            owner.check(&child)?;
        }
        Self::open(root, owner)
    }

    pub fn verify_for_start(&self, id: &CoreId) -> io::Result<VerifiedCore> {
        let lease = self.lease_file(id)?;
        lease
            .try_lock_shared()
            .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
        let manifests = self.root.join("manifests");
        self.owner.check(&manifests)?;
        let manifest_objects = PayloadDir::open(&manifests, self.owner.uid)?;
        let name = format!("{}.json", id.as_str());
        self.owner.check(&manifests.join(&name))?;
        let (file, length) = manifest_objects.open_regular(&name)?;
        if length > 64 * 1024 {
            return Err(invalid("manifest too large"));
        }
        let mut raw = Vec::new();
        file.take(64 * 1024 + 1).read_to_end(&mut raw)?;
        let manifest =
            InstalledCoreManifest::parse(&raw, crate::core_install::sha256_hex).map_err(invalid)?;
        if manifest.id != id.as_str() {
            return Err(invalid("core id mismatch"));
        }
        let directory = self.root.join("versions").join(id.as_str());
        self.owner.check(&directory)?;
        let mut objects = PayloadDir::open(&directory, self.owner.uid)?;
        let mut actual: Vec<_> = objects
            .entry_names(3)?
            .into_iter()
            .map(|name| {
                name.into_string()
                    .map_err(|_| invalid("invalid payload name"))
            })
            .collect::<io::Result<_>>()?;
        let mut expected: Vec<_> = manifest
            .core
            .files
            .iter()
            .map(|file| file.name.clone())
            .collect();
        actual.sort();
        expected.sort();
        if actual != expected {
            return Err(invalid("payload file set differs"));
        }
        let mut binary = None;
        let mut sidecars = Vec::new();
        for expected in &manifest.core.files {
            self.owner.check(&directory.join(&expected.name))?;
            let (mut file, length) = objects.open_regular(&expected.name)?;
            if length != expected.bytes {
                return Err(invalid("payload size differs"));
            }
            let hash = hash_reader(&mut file, expected.bytes)?;
            if hash != expected.sha256 {
                return Err(invalid("payload hash differs"));
            }
            use std::io::{Seek, SeekFrom};
            file.seek(SeekFrom::Start(0))?;
            if expected.name == "sing-box" || expected.name == "sing-box.exe" {
                binary = Some(file);
            } else {
                sidecars.push(file);
            }
        }
        Ok(VerifiedCore {
            manifest,
            binary: binary.ok_or_else(|| invalid("missing binary"))?,
            sidecars,
            directory,
            _directory_object: objects,
            _lease: lease,
        })
    }

    pub fn install(
        &self,
        manifest: &InstalledCoreManifest,
        source: &Path,
        source_uid: Option<u32>,
        source_sid: Option<&str>,
    ) -> io::Result<InstallOutcome> {
        #[cfg(not(windows))]
        let _ = source_sid;
        let _lock = self.mutation_lock()?;
        manifest
            .validate(crate::core_install::sha256_hex)
            .map_err(invalid)?;
        let id = CoreId::parse(&manifest.id).map_err(invalid)?;
        let destination = self.root.join("versions").join(id.as_str());
        if destination.exists() {
            if self
                .root
                .join("manifests")
                .join(format!("{}.json", id.as_str()))
                .exists()
            {
                if let Ok(_verified) = self.verify_for_start(&id) {
                    // Reinstallation refreshes retention recency without changing
                    // the immutable manifest bytes or an already running core.
                    let receipt = self
                        .root
                        .join("manifests")
                        .join(format!("{}.json", id.as_str()));
                    let mut options = OpenOptions::new();
                    options.write(true);
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.custom_flags(nix::libc::O_NOFOLLOW);
                    }
                    let file = options.open(&receipt)?;
                    self.owner.check(&receipt)?;
                    file.set_modified(std::time::SystemTime::now())?;
                    return Ok(InstallOutcome::AlreadyPresent);
                }
                // Corrupt versions can be repaired only after excluding every
                // verifier, running Child and lazy sidecar reader by native lease.
                let lease = self.lease_file(&id)?;
                lease
                    .try_lock()
                    .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
                let receipt = self
                    .root
                    .join("manifests")
                    .join(format!("{}.json", id.as_str()));
                self.owner.check(&receipt)?;
                fs::remove_file(receipt)?;
                self.owner.check(&destination)?;
                self.trash_version(&id, &destination)?;
            }
            if destination.exists() {
                // An orphan with a lease can still be held after external damage;
                // missing metadata must never override a busy native lease.
                let lease = match self.lease_file(&id) {
                    Ok(file) => Some(file),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error),
                };
                if let Some(lease) = &lease {
                    lease
                        .try_lock()
                        .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error))?;
                }
                self.owner.check(&destination)?;
                self.trash_version(&id, &destination)?;
            }
        }
        let sources = PayloadDir::open(source, source_uid)?;
        let mut stage = None;
        for _ in 0..128 {
            let path = self.root.join(".staging").join(format!(
                "{}-{}",
                std::process::id(),
                STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match private_dir(&path) {
                Ok(()) => {
                    stage = Some(path);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        let stage = stage.ok_or_else(|| invalid("staging namespace exhausted"))?;
        let result = (|| {
            for expected in &manifest.core.files {
                let (mut input, length) = sources.open_regular(&expected.name)?;
                #[cfg(windows)]
                if let Some(expected_sid) = source_sid {
                    if crate::platform::windows::winproc::file_owner(&input)? != expected_sid {
                        return Err(invalid("source file owner differs"));
                    }
                }
                if length != expected.bytes {
                    return Err(invalid("source size differs"));
                }
                let path = stage.join(&expected.name);
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut output = options.open(&path)?;
                let mut hash = sha2::Sha256::new();
                use sha2::Digest;
                let mut total = 0u64;
                let mut buffer = [0u8; 64 * 1024];
                loop {
                    let count = input.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    total = total
                        .checked_add(count as u64)
                        .ok_or_else(|| invalid("source size overflow"))?;
                    if total > expected.bytes {
                        return Err(invalid("source grew"));
                    }
                    output.write_all(&buffer[..count])?;
                    hash.update(&buffer[..count]);
                }
                if total != expected.bytes || hex::encode(hash.finalize()) != expected.sha256 {
                    return Err(invalid("source hash differs"));
                }
                output.sync_all()?;
                #[cfg(windows)]
                crate::platform::windows::winproc::seal_store_object(&path, true)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(
                        &path,
                        fs::Permissions::from_mode(if expected.name.starts_with("sing-box") {
                            0o755
                        } else {
                            0o644
                        }),
                    )?;
                }
            }
            seal_dir(&stage)?;
            // Mutation is serialized across processes by the held store file lock.
            if destination.exists() {
                return Err(invalid("version concurrently installed"));
            }
            fs::rename(&stage, &destination)?;
            let lease_path = self
                .root
                .join("leases")
                .join(format!("{}.lock", id.as_str()));
            if !lease_path.exists() {
                let mut options = OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                options.open(&lease_path)?.sync_all()?;
                private_mode(&lease_path, 0o600)?;
            }
            let target = self
                .root
                .join("manifests")
                .join(format!("{}.json", id.as_str()));
            let temporary = self
                .root
                .join("manifests")
                .join(format!("{}.new", id.as_str()));
            let raw = manifest.to_json().map_err(invalid)?;
            if temporary.try_exists()? {
                self.owner.check(&temporary)?;
                let objects = PayloadDir::open(&self.root.join("manifests"), self.owner.uid)?;
                let _ = objects.open_regular(&format!("{}.new", id.as_str()))?;
                fs::remove_file(&temporary)?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o644);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&raw)?;
            file.sync_all()?;
            private_mode(&temporary, 0o644)?;
            fs::rename(&temporary, &target)?;
            self.verify_for_start(&id)?;
            Ok(InstallOutcome::Installed)
        })();
        if stage.exists() {
            let _ = fs::remove_dir_all(&stage);
        }
        if result.is_err() {
            let manifest_path = self
                .root
                .join("manifests")
                .join(format!("{}.json", id.as_str()));
            if !manifest_path.exists() && destination.exists() {
                let _ = fs::remove_dir_all(&destination);
            }
            let _ = fs::remove_file(
                self.root
                    .join("manifests")
                    .join(format!("{}.new", id.as_str())),
            );
        }
        result
    }

    /// Only orphan versions are eligible; callers supply every live slot id while
    /// holding their existing state lock. Retained manifests protect other App versions.
    pub fn gc(&self, live: &[CoreId]) -> io::Result<Vec<CoreId>> {
        let _lock = self.mutation_lock()?;
        let mut removed = Vec::new();
        for entry in fs::read_dir(self.root.join("versions"))? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            let Ok(id) = CoreId::parse(&name) else {
                continue;
            };
            if live.contains(&id)
                || self
                    .root
                    .join("manifests")
                    .join(format!("{name}.json"))
                    .exists()
            {
                continue;
            }
            self.owner.check(&entry.path())?;
            let lease = self.lease_file(&id)?;
            if lease.try_lock().is_err() {
                continue;
            }
            self.trash_version(&id, &entry.path())?;
            removed.push(id);
        }
        Ok(removed)
    }

    /// Keep the latest three installed manifests, plus every busy/live version.
    /// A shared lease covers verification, spawn and lazy sidecar use; no PID
    /// observation or caller-provided empty live list can override that lease.
    pub fn retain_latest_three(&self, live: &[CoreId]) -> io::Result<Vec<CoreId>> {
        let _lock = self.mutation_lock()?;
        let mut manifests = Vec::new();
        for entry in fs::read_dir(self.root.join("manifests"))? {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(id) = name
                .strip_suffix(".json")
                .and_then(|name| CoreId::parse(name).ok())
            else {
                continue;
            };
            self.owner.check(&entry.path())?;
            manifests.push((entry.metadata()?.modified()?, id, entry.path()));
        }
        manifests.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
        let mut removed = Vec::new();
        for (_, id, manifest) in manifests.into_iter().skip(3) {
            if live.contains(&id) {
                continue;
            }
            let lease = self.lease_file(&id)?;
            if lease.try_lock().is_err() {
                continue;
            }
            // Removing the manifest first makes an interrupted cleanup ineligible
            // to start. GC can finish that orphan on a later administrative run.
            fs::remove_file(manifest)?;
            let version = self.root.join("versions").join(id.as_str());
            self.owner.check(&version)?;
            self.trash_version(&id, &version)?;
            removed.push(id);
        }
        Ok(removed)
    }

    fn trash_version(&self, id: &CoreId, version: &Path) -> io::Result<()> {
        let destination = self.root.join(".trash").join(format!(
            "{}-{}-{}",
            id.as_str(),
            std::process::id(),
            STAGE_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::rename(version, &destination)?;
        fs::remove_dir_all(destination)
    }
}

fn private_mode(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        #[cfg(windows)]
        {
            crate::platform::windows::winproc::seal_store_object(path, mode & 0o044 != 0)
        }
        #[cfg(not(windows))]
        {
            let _ = (path, mode);
            Err(invalid("unsupported store platform"))
        }
    }
}

fn hash_reader(input: &mut File, expected_bytes: u64) -> io::Result<String> {
    use sha2::Digest;
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut input = input.take(expected_bytes + 1);
    let mut bytes = 0u64;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    if bytes != expected_bytes {
        return Err(invalid("payload changed size"));
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests;
