use super::*;
use rustix::fd::OwnedFd;
use rustix::fs::{self, AtFlags, FileType, FlockOperation, Mode, OFlags};
use std::io::Read;
#[cfg(any(target_os = "ios", test))]
use std::io::Write;
use std::path::{Component, Path};
fn directory(parent: &OwnedFd, name: &str) -> io::Result<OwnedFd> {
    Ok(fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?)
}
const STAMP: &str = ".polaris-directory-incarnation";
fn verify_stamp(target: &OwnedFd, incarnation: &str) -> io::Result<()> {
    let fd = fs::openat(
        target,
        STAMP,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    let stat = fs::fstat(&fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
        || stat.st_size != 33
    {
        return Err(unknown());
    }
    let mut text = String::new();
    std::fs::File::from(fd).take(34).read_to_string(&mut text)?;
    if text != format!("{incarnation}\n") {
        return Err(unknown());
    }
    Ok(())
}
// Marker fields and lock names use the existing u64 representation. libc's
// device/inode aliases vary by target (Apple dev_t is signed i32); preserve the
// original casts, including signed widening, at this one conversion boundary.
#[allow(clippy::unnecessary_cast)]
fn device_id(stat: &fs::Stat) -> u64 {
    stat.st_dev as u64
}
#[allow(clippy::unnecessary_cast)]
fn inode_id(stat: &fs::Stat) -> u64 {
    stat.st_ino as u64
}
fn same(stat: &fs::Stat, dev: u64, ino: u64) -> bool {
    device_id(stat) == dev && inode_id(stat) == ino
}
fn absent(parent: &OwnedFd, name: &str) -> io::Result<bool> {
    match fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
        Ok(_) => Ok(false),
        Err(rustix::io::Errno::NOENT) => Ok(true),
        Err(e) => Err(e.into()),
    }
}
fn open_absolute(path: &Path) -> io::Result<OwnedFd> {
    if !path.is_absolute() {
        return Err(unknown());
    }
    let mut fd = fs::open(
        "/",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    for part in path.components() {
        match part {
            Component::RootDir => {}
            Component::Normal(name) => {
                fd = fs::openat(
                    &fd,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?;
            }
            _ => return Err(unknown()),
        }
    }
    Ok(fd)
}
fn open_locked(control: &OwnedFd, name: &str, exclusive: bool) -> io::Result<OwnedFd> {
    let fd = fs::openat(
        control,
        name,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )?;
    let stat = fs::fstat(&fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
    {
        return Err(unknown());
    }
    fs::flock(
        &fd,
        if exclusive {
            FlockOperation::NonBlockingLockExclusive
        } else {
            FlockOperation::NonBlockingLockShared
        },
    )?;
    if !same(
        &fs::statat(control, name, AtFlags::SYMLINK_NOFOLLOW)?,
        device_id(&stat),
        inode_id(&stat),
    ) {
        return Err(unknown());
    }
    Ok(fd)
}
fn read_marker_at(control: &OwnedFd, name: &str) -> io::Result<DirectoryMarker> {
    let fd = fs::openat(
        control,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    let stat = fs::fstat(&fd)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
        || stat.st_nlink != 1
        || stat.st_mode & 0o777 != 0o600
        || stat.st_size > 16384
    {
        return Err(unknown());
    }
    let mut bytes = Vec::new();
    std::fs::File::from(fd)
        .take(16385)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16384 {
        return Err(unknown());
    }
    serde_json::from_slice(&bytes).map_err(|_| unknown())
}
pub(crate) struct DirectoryLease {
    #[cfg(any(target_os = "ios", test))]
    exclusive: bool,
    root: OwnedFd,
    control: OwnedFd,
    _lock: OwnedFd,
    #[cfg(any(target_os = "ios", test))]
    lifetime_name: String,
    _admission: Option<OwnedFd>,
    _selection_admission: Option<OwnedFd>,
    marker_name: String,
    #[cfg(any(target_os = "ios", test))]
    stem: String,
    server_id: String,
    root_path: std::path::PathBuf,
}
impl DirectoryLease {
    /// Select exactly as the managed Go constructor: existing logical data
    /// wins; otherwise select the isolated physical namespace. Never mkdir a node.
    pub(crate) fn acquire(config: &Path, server_id: &str, exclusive: bool) -> io::Result<Self> {
        #[cfg(target_os = "ios")]
        {
            return Self::acquire_mapped(config, server_id, exclusive);
        }
        #[cfg(not(target_os = "ios"))]
        Self::acquire_root(config, server_id, exclusive, "tailscale")
    }
    fn admission(config: &Path, root_name: &str) -> io::Result<(OwnedFd, OwnedFd, OwnedFd)> {
        let parent = open_absolute(config)?;
        match fs::mkdirat(&parent, root_name, Mode::RWXU) {
            Ok(()) => fs::fsync(&parent)?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
        let root = directory(&parent, root_name)?;
        let root_stat = fs::fstat(&root)?;
        let control_name = format!(
            ".polaris-directory-control-{}-{}",
            device_id(&root_stat),
            inode_id(&root_stat)
        );
        match fs::mkdirat(&parent, &control_name, Mode::RWXU) {
            Ok(()) => fs::fsync(&parent)?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(e) => return Err(e.into()),
        }
        let control = directory(&parent, &control_name)?;
        let admission = open_locked(&control, "root.admission.lock", true)?;
        fs::fsync(&control)?;
        Ok((root, control, admission))
    }
    fn acquire_root(
        config: &Path,
        server_id: &str,
        exclusive: bool,
        root_name: &str,
    ) -> io::Result<Self> {
        polaris_mesh::tailscale_state::tailscale_state_dir(config, server_id)
            .map_err(|_| unknown())?;
        Self::acquire_selected(
            config,
            server_id,
            exclusive,
            root_name,
            Self::admission(config, root_name)?,
        )
    }
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn acquire_mapped(
        config: &Path,
        server_id: &str,
        exclusive: bool,
    ) -> io::Result<Self> {
        polaris_mesh::tailscale_state::tailscale_state_dir(config, server_id)
            .map_err(|_| unknown())?;
        let selected = Self::admission(config, "tailscale")?;
        match fs::statat(&selected.0, server_id, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => {
                Self::acquire_selected(config, server_id, exclusive, "tailscale", selected)
            }
            Ok(_) => Err(unknown()),
            Err(rustix::io::Errno::NOENT) => {
                // Retain logical admission through acquisition of the actual
                // physical lease, exactly matching Go's lock order.
                let physical = Self::acquire_root(config, server_id, exclusive, "tailscale-g4-v1");
                drop(selected);
                physical
            }
            Err(e) => Err(e.into()),
        }
    }
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn actual_directory(
        config: &Path,
        server_id: &str,
    ) -> io::Result<std::path::PathBuf> {
        polaris_mesh::tailscale_state::tailscale_state_dir(config, server_id)
            .map_err(|_| unknown())?;
        let selected = Self::admission(config, "tailscale")?;
        let root_name = match fs::statat(&selected.0, server_id, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => "tailscale",
            Ok(_) => return Err(unknown()),
            Err(rustix::io::Errno::NOENT) => "tailscale-g4-v1",
            Err(e) => return Err(e.into()),
        };
        Ok(config.join(root_name).join(server_id))
    }
    pub(crate) fn acquire_intent(
        config: &Path,
        intent: &DirectoryDeletionIntent,
    ) -> io::Result<Self> {
        if Path::new(&intent.state_directory)
            != config.join("tailscale-g4-v1").join(&intent.server_id)
        {
            return Err(unknown());
        }
        // Same logical -> physical lock order as Go mapping; keep both
        // admissions until the configured-alias check and commit complete.
        let (_logical, _control, selection) = Self::admission(config, "tailscale")?;
        let mut lease = Self::acquire_root(config, &intent.server_id, true, "tailscale-g4-v1")?;
        lease._selection_admission = Some(selection);
        Ok(lease)
    }
    pub(crate) fn directory_path(&self) -> std::path::PathBuf {
        self.root_path.join(&self.server_id)
    }
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn poison_domain(config: &Path) -> io::Result<()> {
        let (root, control, _admission) = Self::admission(config, "tailscale-g4-v1")?;
        let parent = open_absolute(config)?;
        let fd = fs::openat(
            &control,
            ".domain-unsafe",
            OFlags::WRONLY | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::RUSR | Mode::WUSR,
        )?;
        let stat = fs::fstat(&fd)?;
        if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile
            || stat.st_nlink != 1
            || stat.st_mode & 0o777 != 0o600
        {
            return Err(unknown());
        }
        let mut file = std::fs::File::from(fd);
        file.write_all(b"unsafe\n")?;
        file.sync_all()?;
        fs::fsync(&control)?;
        fs::fsync(&parent)?;
        fs::fsync(&root)?;
        let root_stat = fs::fstat(&root)?;
        let control_stat = fs::fstat(&control)?;
        let control_name = format!(
            ".polaris-directory-control-{}-{}",
            device_id(&root_stat),
            inode_id(&root_stat)
        );
        if !same(
            &fs::statat(&parent, "tailscale-g4-v1", AtFlags::SYMLINK_NOFOLLOW)?,
            device_id(&root_stat),
            inode_id(&root_stat),
        ) || !same(
            &fs::statat(&parent, &control_name, AtFlags::SYMLINK_NOFOLLOW)?,
            device_id(&control_stat),
            inode_id(&control_stat),
        ) || !same(
            &fs::statat(&control, ".domain-unsafe", AtFlags::SYMLINK_NOFOLLOW)?,
            device_id(&stat),
            inode_id(&stat),
        ) {
            return Err(unknown());
        }
        // Safe Rust Drop does not expose close errors. This negative marker
        // authorizes no cleanup: synced presence and anchors permanently revoke
        // proof even if a subsequent descriptor close reports an error.
        Ok(())
    }
    fn require_safe_domain(&self) -> io::Result<()> {
        if !absent(&self.control, ".domain-unsafe")? {
            return Err(unknown());
        }
        #[cfg(target_os = "ios")]
        if self.root_path.file_name().and_then(|s| s.to_str()) != Some("tailscale-g4-v1") {
            return Err(unknown());
        }
        Ok(())
    }
    fn acquire_selected(
        config: &Path,
        server_id: &str,
        exclusive: bool,
        root_name: &str,
        selected: (OwnedFd, OwnedFd, OwnedFd),
    ) -> io::Result<Self> {
        let raw_stem = format!(
            "sha256-{}",
            polaris_updater::sha256_hex(server_id.as_bytes())
        );
        let root_path = config.join(root_name);
        let (root, control, admission) = selected;
        let root_stat = fs::fstat(&root)?;
        // All aliases resolve their shared physical object while holding the
        // same root admission lock. Existing unmarked objects use inode locks;
        // managed aliases use the one validated original marker's lock.
        let target_stat = match fs::statat(&root, server_id, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Directory => Some(stat),
            Ok(_) => return Err(unknown()),
            Err(rustix::io::Errno::NOENT) => None,
            Err(e) => return Err(e.into()),
        };
        let mut selected: Option<String> = None;
        if let Some(target) = &target_stat {
            for entry in fs::Dir::read_from(&control)? {
                let entry = entry?;
                let Ok(name) = entry.file_name().to_str() else {
                    continue;
                };
                let Some(candidate_stem) = name.strip_suffix(".json") else {
                    continue;
                };
                let Some(hash) = candidate_stem.strip_prefix("sha256-") else {
                    continue;
                };
                if !polaris_mesh::tailscale_state::directory_retirement::lower_hex(hash, 64) {
                    continue;
                }
                let marker = read_marker_at(&control, name)?;
                if marker.version != 1 || !same(&root_stat, marker.root_device, marker.root_inode) {
                    return Err(unknown());
                }
                if !same(target, marker.directory_device, marker.directory_inode) {
                    continue;
                }
                // Never trust numeric inode claims alone: after inode reuse the
                // original marker basename must still resolve to this object.
                polaris_mesh::tailscale_state::tailscale_state_dir(config, &marker.server_id)
                    .map_err(|_| unknown())?;
                if candidate_stem
                    != format!(
                        "sha256-{}",
                        polaris_updater::sha256_hex(marker.server_id.as_bytes())
                    )
                {
                    return Err(unknown());
                }
                let original = match fs::statat(&root, &marker.server_id, AtFlags::SYMLINK_NOFOLLOW)
                {
                    Ok(stat) => stat,
                    Err(rustix::io::Errno::NOENT) => continue,
                    Err(e) => return Err(e.into()),
                };
                if FileType::from_raw_mode(original.st_mode) != FileType::Directory
                    || !same(&original, device_id(target), inode_id(target))
                {
                    continue;
                }
                let live = directory(&root, server_id)?;
                if verify_stamp(&live, &marker.incarnation).is_err() {
                    continue;
                }
                if selected.replace(candidate_stem.to_owned()).is_some() {
                    return Err(unknown());
                }
            }
        }
        let stem = selected.unwrap_or(raw_stem);
        let marker_name = format!("{stem}.json");
        let marker = match read_marker_at(&control, &marker_name) {
            Ok(marker)
                if target_stat.as_ref().is_some_and(|target| {
                    !same(target, marker.directory_device, marker.directory_inode)
                        || !same(&root_stat, marker.root_device, marker.root_inode)
                }) =>
            {
                None
            }
            Ok(marker)
                if target_stat.is_some()
                    && directory(&root, server_id)
                        .and_then(|target| verify_stamp(&target, &marker.incarnation))
                        .is_err() =>
            {
                None
            }
            Ok(marker) => Some(marker),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        let lifetime_name = match (&marker, &target_stat) {
            (None, Some(target)) => {
                format!("legacy-{}-{}.lock", device_id(target), inode_id(target))
            }
            _ => format!("{stem}.lock"),
        };
        let lock = open_locked(&control, &lifetime_name, exclusive)?;
        let mut lease = Self {
            #[cfg(any(target_os = "ios", test))]
            exclusive,
            root,
            control,
            _lock: lock,
            #[cfg(any(target_os = "ios", test))]
            lifetime_name,
            _admission: Some(admission),
            _selection_admission: None,
            marker_name,
            #[cfg(any(target_os = "ios", test))]
            stem,
            server_id: server_id.to_owned(),
            root_path,
        };
        if !exclusive {
            if let Some(marker) = marker {
                let target = directory(&lease.root, server_id)?;
                if marker.version != 1
                    || marker.phase != "Open"
                    || !marker.delete_intent_nonce.is_empty()
                    || !polaris_mesh::tailscale_state::directory_retirement::lower_hex(
                        &marker.incarnation,
                        32,
                    )
                    || !polaris_mesh::tailscale_state::directory_retirement::lower_hex(
                        &marker.directory_epoch,
                        32,
                    )
                    || !same(&root_stat, marker.root_device, marker.root_inode)
                    || !same(
                        &fs::fstat(&target)?,
                        marker.directory_device,
                        marker.directory_inode,
                    )
                {
                    return Err(unknown());
                }
            }
            lease._admission.take();
        }
        Ok(lease)
    }
    /// Once a commit is durable, keep its lifetime EX until journal ack,
    /// but permit the next directory in the batch to enter root admission.
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn release_admission_after_commit(&mut self) {
        self._admission.take();
        self._selection_admission.take();
    }
    fn read_marker(&self) -> io::Result<DirectoryMarker> {
        read_marker_at(&self.control, &self.marker_name)
    }
    #[cfg(any(target_os = "ios", test))]
    fn write_marker(&self, marker: &DirectoryMarker) -> io::Result<()> {
        let name = format!("{}.tmp-{}", self.marker_name, uuid::Uuid::new_v4().simple());
        let fd = fs::openat(
            &self.control,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let mut file = std::fs::File::from(fd);
        file.write_all(&serde_json::to_vec(marker).map_err(io::Error::other)?)?;
        file.sync_all()?;
        fs::renameat(&self.control, &name, &self.control, &self.marker_name)?;
        fs::fsync(&self.control)?;
        Ok(())
    }
    /// New incarnations are allocated only for absent targets. Existing unmarked
    /// directories are legacy and can never be silently adopted for deletion proof.
    #[cfg(test)]
    pub(crate) fn create_new(&self) -> io::Result<()> {
        if !self.exclusive {
            return Err(unknown());
        }
        match self.read_marker() {
            Ok(m) if m.phase == "Deleted" => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => return Err(unknown()),
        }
        fs::mkdirat(&self.root, &self.server_id, Mode::RWXU)?;
        let target_fd = directory(&self.root, &self.server_id)?;
        let root = fs::fstat(&self.root)?;
        let target = fs::fstat(&target_fd)?;
        let marker = DirectoryMarker {
            version: 1,
            server_id: self.server_id.clone(),
            incarnation: uuid::Uuid::new_v4().simple().to_string(),
            directory_epoch: uuid::Uuid::new_v4().simple().to_string(),
            root_device: device_id(&root),
            root_inode: inode_id(&root),
            directory_device: device_id(&target),
            directory_inode: inode_id(&target),
            phase: "Open".into(),
            delete_intent_nonce: String::new(),
        };
        let stamp = fs::openat(
            &target_fd,
            STAMP,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )?;
        let mut stamp = std::fs::File::from(stamp);
        stamp.write_all(format!("{}\n", marker.incarnation).as_bytes())?;
        stamp.sync_all()?;
        fs::fsync(&target_fd)?;
        fs::fsync(&self.root)?;
        self.write_marker(&marker)
    }
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn snapshot_intent(&self) -> io::Result<DirectoryDeletionIntent> {
        let _admission = if self._admission.is_none() {
            Some(open_locked(&self.control, "root.admission.lock", true)?)
        } else {
            None
        };
        self.require_safe_domain()?;
        let m = self.read_marker()?;
        let target = directory(&self.root, &self.server_id)?;
        if m.phase != "Open"
            || m.version != 1
            || m.server_id != self.server_id
            || !m.delete_intent_nonce.is_empty()
            || !same(&fs::fstat(&self.root)?, m.root_device, m.root_inode)
            || !same(&fs::fstat(&target)?, m.directory_device, m.directory_inode)
        {
            return Err(unknown());
        }
        verify_stamp(&target, &m.incarnation)?;
        let intent = DirectoryDeletionIntent {
            server_id: self.server_id.clone(),
            incarnation: m.incarnation.clone(),
            directory_identity: m.identity(),
            state_directory: self
                .root_path
                .join(&self.server_id)
                .to_string_lossy()
                .into_owned(),
            delete_intent_nonce: uuid::Uuid::new_v4().simple().to_string(),
        };
        if !m.matches(&intent) {
            return Err(unknown());
        }
        Ok(intent)
    }
    /// Re-check actual configured directory objects, not just ID strings.
    /// APFS case/Unicode aliases of any surviving node prevent deletion.
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn ensure_no_configured_alias(
        &self,
        intent: &DirectoryDeletionIntent,
        current: &serde_json::Value,
    ) -> io::Result<()> {
        if !self.exclusive {
            return Err(unknown());
        }
        let marker = self.read_marker()?;
        if !marker.matches(intent) {
            return Err(unknown());
        }
        let config = self.root_path.parent().ok_or_else(unknown)?;
        // Both namespaces are real filesystem objects, never a string alias
        // for one another. Any surviving configured alias in either retains data.
        for root_name in ["tailscale", "tailscale-g4-v1"] {
            let root = match open_absolute(&config.join(root_name)) {
                Ok(root) => root,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            for node in current
                .get("servers")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(id) = node.get("id").and_then(serde_json::Value::as_str) else {
                    return Err(unknown());
                };
                if polaris_mesh::tailscale_state::tailscale_state_dir(Path::new("/"), id).is_err() {
                    continue;
                }
                match fs::statat(&root, id, AtFlags::SYMLINK_NOFOLLOW) {
                    Ok(stat) if FileType::from_raw_mode(stat.st_mode) == FileType::Symlink => {
                        return Err(unknown())
                    }
                    Ok(stat) if same(&stat, marker.directory_device, marker.directory_inode) => {
                        return Err(unknown())
                    }
                    Ok(_) | Err(rustix::io::Errno::NOENT) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }
    pub(crate) fn committed_intent(&self, intent: &DirectoryDeletionIntent) -> io::Result<bool> {
        self.require_safe_domain()?;
        let marker = self.read_marker()?;
        Ok(marker.matches(intent)
            && matches!(marker.phase.as_str(), "Retiring" | "Removing" | "Deleted"))
    }
    /// Resume only a previously admitted durable commit. Open markers never
    /// acquire authority from disk, an empty census, or successful locking.
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn resume_committed(&self, intent: &DirectoryDeletionIntent) -> io::Result<()> {
        if !self.exclusive {
            return Err(unknown());
        }
        let marker = self.read_marker()?;
        if !matches!(marker.phase.as_str(), "Retiring" | "Removing" | "Deleted")
            || !marker.matches(intent)
        {
            return Err(unknown());
        }
        let epochs = std::collections::BTreeSet::from([marker.directory_epoch]);
        self.delete_bound(intent, &epochs)
    }
    /// Called only after the caller checked its opaque native receipt and current journal/config.
    /// The exclusive lease remains held through journal persistence by the caller.
    #[cfg(any(target_os = "ios", test))]
    pub(crate) fn delete_bound(
        &self,
        intent: &DirectoryDeletionIntent,
        receipt_epochs: &std::collections::BTreeSet<String>,
    ) -> io::Result<()> {
        if !self.exclusive || self.lifetime_name != format!("{}.lock", self.stem) {
            return Err(unknown());
        }
        self.require_safe_domain()?;
        let mut marker = self.read_marker()?;
        if !marker.matches(intent)
            || !receipt_epochs.contains(&marker.directory_epoch)
            || intent.server_id != self.server_id
            || self.root_path.join(&self.server_id) != Path::new(&intent.state_directory)
            || !same(
                &fs::fstat(&self.root)?,
                marker.root_device,
                marker.root_inode,
            )
        {
            return Err(unknown());
        }
        let quarantine = format!("{}.retired-{}", self.stem, marker.incarnation);
        if marker.phase == "Deleted" {
            // A new object at either name is never this intent's retry target.
            if !absent(&self.root, &self.server_id)? || !absent(&self.control, &quarantine)? {
                return Err(unknown());
            }
            return Ok(());
        }
        if marker.phase == "Open" {
            let original = directory(&self.root, &self.server_id)?;
            if !same(
                &fs::fstat(&original)?,
                marker.directory_device,
                marker.directory_inode,
            ) {
                return Err(unknown());
            }
            verify_stamp(&original, &marker.incarnation)?;
            marker.phase = "Retiring".into();
            marker.delete_intent_nonce = intent.delete_intent_nonce.clone();
            self.write_marker(&marker)?;
        }
        let target = match directory(&self.control, &quarantine) {
            Ok(fd) => {
                if !absent(&self.root, &self.server_id)? {
                    return Err(unknown());
                }
                fd
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                if marker.phase == "Removing" {
                    // Final unlink already started in quarantine. Never rename a
                    // newly created object from the public basename into this phase.
                    if !absent(&self.root, &self.server_id)? {
                        return Err(unknown());
                    }
                    marker.phase = "Deleted".into();
                    return self.write_marker(&marker);
                }
                let original = match directory(&self.root, &self.server_id) {
                    Ok(fd) => fd,
                    Err(e)
                        if e.kind() == io::ErrorKind::NotFound
                            && matches!(marker.phase.as_str(), "Retiring" | "Removing") =>
                    {
                        // Crash after unlink but before the Deleted marker: the durable
                        // tombstone and same intent keep the missing old incarnation closed.
                        marker.phase = "Deleted".into();
                        return self.write_marker(&marker);
                    }
                    Err(e) => return Err(e),
                };
                if !same(
                    &fs::fstat(&original)?,
                    marker.directory_device,
                    marker.directory_inode,
                ) {
                    return Err(unknown());
                }
                // Validate physical incarnation before moving any public object.
                verify_stamp(&original, &marker.incarnation)?;
                fs::renameat(&self.root, &self.server_id, &self.control, &quarantine)?;
                fs::fsync(&self.root)?;
                fs::fsync(&self.control)?;
                directory(&self.control, &quarantine)?
            }
            Err(e) => return Err(e),
        };
        if !same(
            &fs::fstat(&target)?,
            marker.directory_device,
            marker.directory_inode,
        ) {
            return Err(unknown());
        }
        if marker.phase == "Retiring" {
            verify_stamp(&target, &marker.incarnation)?;
            empty_directory(&target, marker.directory_device, true)?;
            marker.phase = "Removing".into();
            self.write_marker(&marker)?;
        }
        // Removing is a durable, already-authorized final unlink stage.
        // Never recurse after the binding stamp may have been removed.
        for entry in fs::Dir::read_from(&target)? {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name != b"." && name != b".." && name != STAMP.as_bytes() {
                return Err(unknown());
            }
        }
        match verify_stamp(&target, &marker.incarnation) {
            Ok(()) => fs::unlinkat(&target, STAMP, AtFlags::empty())?,
            Err(e) if e.kind() == io::ErrorKind::NotFound && marker.phase == "Removing" => {}
            Err(e) => return Err(e),
        }
        fs::fsync(&target)?;
        let named = fs::statat(&self.control, &quarantine, AtFlags::SYMLINK_NOFOLLOW)?;
        if !same(&named, marker.directory_device, marker.directory_inode) {
            return Err(unknown());
        }
        fs::unlinkat(&self.control, &quarantine, AtFlags::REMOVEDIR)?;
        fs::fsync(&self.control)?;
        marker.phase = "Deleted".into();
        self.write_marker(&marker)
    }
}
#[cfg(any(target_os = "ios", test))]
fn empty_directory(fd: &OwnedFd, device: u64, keep_stamp: bool) -> io::Result<()> {
    for entry in fs::Dir::read_from(fd)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_bytes() == b"."
            || name.to_bytes() == b".."
            || keep_stamp && name.to_bytes() == STAMP.as_bytes()
        {
            continue;
        }
        let stat = fs::statat(fd, name, AtFlags::SYMLINK_NOFOLLOW)?;
        if device_id(&stat) != device {
            return Err(unknown());
        }
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            let child = fs::openat(
                fd,
                name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            if !same(&fs::fstat(&child)?, device_id(&stat), inode_id(&stat)) {
                return Err(unknown());
            }
            empty_directory(&child, device, false)?;
            fs::unlinkat(fd, name, AtFlags::REMOVEDIR)?;
        } else {
            // Unlink symlinks themselves, never traverse their destinations.
            fs::unlinkat(fd, name, AtFlags::empty())?;
        }
    }
    fs::fsync(fd)?;
    Ok(())
}

#[cfg(test)]
mod tests;
