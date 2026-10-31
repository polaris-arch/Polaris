//! Object checks and bounded, non-recursive operations for one repair journal.
//! Read handles identify objects; they do not freeze bytes or establish a lease.

use super::super::{hash_reader, invalid, CoreStore};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

pub(super) const MAX_ENTRIES: usize = 16;
pub(super) const MAX_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Identity(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Regular {
    pub identity: Identity,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Directory {
    pub identity: Identity,
    pub files: BTreeMap<String, Regular>,
}

struct Anchor {
    path: PathBuf,
    file: File,
    identity: Identity,
}

/// All mutations are beneath these held store objects. Their parents remain
/// trusted only under the parent's actual mutation/lease guards and policy.
pub(super) struct Namespace<'a> {
    pub store: &'a CoreStore,
    anchors: Vec<Anchor>,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    protected: Option<crate::protected_install_path::ProtectedPath>,
}

impl<'a> Namespace<'a> {
    pub fn open(store: &'a CoreStore) -> io::Result<Self> {
        if !cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "repair transaction unsupported on this target",
            ));
        }
        if !store.root.is_absolute() {
            return Err(invalid("repair store path must be absolute"));
        }
        #[cfg(windows)]
        validate_windows_ancestors(store)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let protected = if store.owner.uid == Some(0) {
            // Existing root-store policy validates every ancestor and macOS ACL.
            Some(crate::protected_install_path::ProtectedPath::open(
                &store.root,
                true,
            )?)
        } else {
            // Non-root stores are data-only stores, not trusted execution roots.
            None
        };
        let mut value = Self {
            store,
            anchors: Vec::new(),
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            protected,
        };
        for path in [
            store.root.clone(),
            store.root.join("versions"),
            store.root.join("manifests"),
            store.root.join(".staging"),
            store.root.join(".trash"),
            store.root.join("leases"),
        ] {
            value.anchor(&path)?;
        }
        value.check()?;
        Ok(value)
    }

    pub fn anchor(&mut self, path: &Path) -> io::Result<()> {
        self.check()?;
        let file = self.open_object(path, true)?;
        let identity = identity(&file)?;
        self.anchors.push(Anchor {
            path: path.to_path_buf(),
            file,
            identity,
        });
        Ok(())
    }

    pub fn check(&self) -> io::Result<()> {
        #[cfg(windows)]
        validate_windows_ancestors(self.store)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(protected) = &self.protected {
            protected.unchanged()?;
        }
        for anchor in &self.anchors {
            if identity(&anchor.file)? != anchor.identity
                || identity(&self.open_object(&anchor.path, true)?)? != anchor.identity
            {
                return Err(invalid("repair namespace object changed"));
            }
        }
        Ok(())
    }

    pub fn open_object(&self, path: &Path, directory: bool) -> io::Result<File> {
        self.store.owner.check(path)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        let file = if self.store.owner.uid == Some(0) {
            crate::protected_install_path::ProtectedPath::open(path, directory)?.file
        } else {
            open_nofollow(path, directory)?
        };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let file = open_nofollow(path, directory)?;
        let metadata = file.metadata()?;
        if metadata.is_dir() != directory || (!directory && !metadata.is_file()) {
            return Err(invalid("repair object type differs"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if self
                .store
                .owner
                .uid
                .is_some_and(|uid| metadata.uid() != uid)
                || metadata.mode() & 0o022 != 0
                || (!directory && metadata.nlink() != 1)
            {
                return Err(invalid(
                    "repair object ownership, mode or link count differs",
                ));
            }
        }
        #[cfg(windows)]
        {
            if let Some(data) = &self.store.owner.data {
                data.check_held(&file, directory)?;
            } else {
                use crate::platform::windows::coreacl::judge_object;
                use crate::platform::windows::winproc::file_security;
                let security = file_security(&file)?;
                if !judge_object("repair-object", &security).is_empty() {
                    return Err(invalid("repair object ACL differs"));
                }
                if directory {
                    // A parent can be non-writable while an INHERIT_ONLY ACE would
                    // expose its new children before sealing. Reuse the existing
                    // ACL judge on that grant as effective; never rewrite the ACL.
                    let mut child_security = security;
                    if let Some(aces) = child_security.dacl.as_mut() {
                        for ace in aces {
                            ace.flags &= !crate::platform::windows::coreacl::ACE_FLAG_INHERIT_ONLY;
                        }
                    }
                    if !judge_object("repair-child-inheritance", &child_security).is_empty() {
                        return Err(invalid("repair parent grants unsafe child writes"));
                    }
                }
            }
            if !directory && link_count(&file)? != 1 {
                return Err(invalid("repair object has multiple links"));
            }
        }
        Ok(file)
    }

    pub fn regular(&self, path: &Path, max: u64) -> io::Result<Regular> {
        self.check()?;
        let mut file = self.open_object(path, false)?;
        let identity = identity(&file)?;
        let bytes = file.metadata()?.len();
        if bytes > max {
            return Err(invalid("repair object exceeds bounded size"));
        }
        let sha256 = hash_reader(&mut file, bytes)?;
        if file.metadata()?.len() != bytes || identity != self.identity_at(path, false)? {
            return Err(invalid("repair object changed while hashing"));
        }
        Ok(Regular {
            identity,
            bytes,
            sha256,
        })
    }

    pub fn identity_at(&self, path: &Path, directory: bool) -> io::Result<Identity> {
        identity(&self.open_object(path, directory)?)
    }

    pub fn directory(&self, path: &Path) -> io::Result<Directory> {
        self.check()?;
        let directory = self.open_object(path, true)?;
        let id = identity(&directory)?;
        let mut payload = crate::core_objects::PayloadDir::open(path, self.store.owner.uid)?;
        let mut files = BTreeMap::new();
        let mut total = 0u64;
        for name in payload.entry_names(MAX_ENTRIES)? {
            let name = name
                .into_string()
                .map_err(|_| invalid("repair member name is not UTF-8"))?;
            leaf(&name)?;
            let record = self.regular(&path.join(&name), MAX_BYTES - total)?;
            total += record.bytes;
            if files.insert(name, record).is_some() {
                return Err(invalid("duplicate repair member"));
            }
        }
        if self.identity_at(path, true)? != id {
            return Err(invalid("repair directory changed during enumeration"));
        }
        self.check()?;
        Ok(Directory {
            identity: id,
            files,
        })
    }

    pub fn read(&self, path: &Path, max: usize) -> io::Result<Vec<u8>> {
        self.check()?;
        let file = self.open_object(path, false)?;
        let mut raw = Vec::new();
        file.take(max as u64 + 1).read_to_end(&mut raw)?;
        if raw.len() > max {
            return Err(invalid("repair record too large"));
        }
        Ok(raw)
    }

    pub fn create(
        &self,
        path: &Path,
        raw: &[u8],
        created: &mut Option<Identity>,
    ) -> io::Result<()> {
        self.check()?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH,
            };
            options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH);
        }
        let mut file = options.open(path)?;
        *created = Some(identity(&file)?);
        // Parent is already protected. Seal before writing bytes, never repair
        // permissions on an arbitrary pre-existing object.
        self.store.owner.mode(path, 0o600)?;
        file.write_all(raw)?;
        file.sync_all()?;
        if identity(&file)? != self.identity_at(path, false)? {
            return Err(invalid("new repair record replaced"));
        }
        self.check()
    }

    pub fn create_directory(&self, path: &Path, created: &mut Option<Identity>) -> io::Result<()> {
        self.check()?;
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(path)?;
        *created = Some(identity(&open_nofollow(path, true)?)?);
        self.store.owner.mode(path, 0o700)?;
        Ok(())
    }

    /// Before any active rename, only the two create_new records of this call
    /// may be discarded. A substituted or unaccounted object keeps the site.
    pub fn discard_draft(
        &mut self,
        path: &Path,
        directory: &Identity,
        records: &[(&str, Option<&Identity>)],
    ) -> io::Result<()> {
        self.check()?;
        if self.identity_at(path, true)? != *directory {
            return Err(invalid("draft directory replaced"));
        }
        let mut held = crate::core_objects::PayloadDir::open(path, self.store.owner.uid)?;
        for name in held.entry_names(2)? {
            let name = name
                .to_str()
                .ok_or_else(|| invalid("invalid draft member"))?;
            let (_, expected) = records
                .iter()
                .find(|(key, _)| *key == name)
                .ok_or_else(|| invalid("foreign draft member"))?;
            let expected =
                expected.ok_or_else(|| invalid("draft member has no creation identity"))?;
            if self.identity_at(&path.join(name), false)? != *expected {
                return Err(invalid("draft member replaced"));
            }
        }
        for (name, expected) in records {
            let file = path.join(name);
            if exists(&file)? {
                let expected = expected.ok_or_else(|| invalid("unknown draft member"))?;
                self.check()?;
                if self.identity_at(&file, false)? != *expected {
                    return Err(invalid("draft member replaced before removal"));
                }
                fs::remove_file(file)?;
            }
        }
        // PayloadDir can deny delete sharing on some systems. It is no longer
        // needed after the bounded enumeration; release it before rmdir.
        drop(held);
        if self.anchors.iter().any(|anchor| anchor.path == path) {
            self.release_anchor(path)?;
        }
        self.check()?;
        if self.identity_at(path, true)? != *directory {
            return Err(invalid("draft directory changed before removal"));
        }
        fs::remove_dir(path)?;
        self.sync(&self.store.root.join(".staging"))
    }

    pub fn sync(&self, path: &Path) -> io::Result<()> {
        self.check()?;
        sync_directory(path)?;
        self.check()
    }

    pub fn sync_regular(&self, path: &Path, expected: &Regular) -> io::Result<()> {
        self.check()?;
        if self.regular(path, MAX_BYTES)? != *expected {
            return Err(invalid("repair sync object differs"));
        }
        #[cfg(not(windows))]
        let file = self.open_object(path, false)?;
        #[cfg(windows)]
        let file = {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::*;
            OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?
        };
        if identity(&file)? != expected.identity {
            return Err(invalid("repair sync handle differs"));
        }
        file.sync_all()?;
        if self.regular(path, MAX_BYTES)? != *expected {
            return Err(invalid("repair sync object changed"));
        }
        Ok(())
    }

    #[cfg(windows)]
    pub fn sync_held_lease(&self, path: &Path, lease: &File) -> io::Result<()> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::*;
        self.check()?;
        let expected = identity(lease)?;
        let named = self.open_object(path, false)?;
        if identity(&named)? != expected {
            return Err(invalid("publication lease changed before flush"));
        }
        // Shared readers retain their read-only occupancy handle and lock.
        // Windows FlushFileBuffers requires WRITE, so open a separate handle
        // ONLY for this explicit publication barrier; never reacquire/unlock
        // the actual occupancy lease or read its locked byte range.
        let writable = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if !writable.metadata()?.is_file()
            || identity(&writable)? != expected
            || link_count(&writable)? != 1
        {
            return Err(invalid("publication flush handle differs from held lease"));
        }
        self.store.owner.check(path)?;
        writable.sync_all()?;
        if identity(lease)? != expected || self.identity_at(path, false)? != expected {
            return Err(invalid("publication lease changed during flush"));
        }
        self.check()
    }

    pub fn release_anchor(&mut self, path: &Path) -> io::Result<()> {
        self.check()?;
        let index = self
            .anchors
            .iter()
            .position(|anchor| anchor.path == path)
            .ok_or_else(|| invalid("repair anchor missing"))?;
        // Only the finished journal may leave its staging name. Fixed namespace
        // anchors stay live through the final archive rename and directory sync.
        if path.parent() != Some(self.store.root.join(".staging").as_path()) {
            return Err(invalid("cannot release a store namespace anchor"));
        }
        self.anchors.remove(index);
        Ok(())
    }

    pub fn rename(&self, from: &Path, to: &Path, id: &Identity, directory: bool) -> io::Result<()> {
        self.check()?;
        if self.identity_at(from, directory)? != *id {
            return Err(invalid("repair rename source changed"));
        }
        absent(to)?;
        rename_exclusive(from, to)?;
        // The operation may already have occurred if a following sync fails.
        // Reconciliation uses object identity, not the syscall return alone.
        if self.identity_at(to, directory)? != *id {
            return Err(invalid("repair rename destination differs"));
        }
        let archive = from.parent() == Some(self.store.root.join(".staging").as_path())
            && to.parent() == Some(self.store.root.join(".trash").as_path());
        if archive {
            super::checkpoint(super::Point::ArchiveDirectorySync)?;
            self.sync(to)?;
            super::checkpoint(super::Point::ArchiveStagingSync)?;
        }
        self.sync(
            from.parent()
                .ok_or_else(|| invalid("missing rename parent"))?,
        )?;
        if archive {
            super::checkpoint(super::Point::ArchiveTrashSync)?;
        }
        self.sync(
            to.parent()
                .ok_or_else(|| invalid("missing rename parent"))?,
        )
    }
}

#[cfg(windows)]
fn validate_windows_ancestors(store: &CoreStore) -> io::Result<()> {
    if let Some(data) = &store.owner.data {
        return data.guard();
    }
    use crate::platform::windows::{coreacl, winproc};
    let mut path = PathBuf::new();
    let mut count = 0usize;
    for component in store.root.components() {
        count += 1;
        if count > 64 || matches!(component, Component::CurDir | Component::ParentDir) {
            return Err(invalid("invalid repair store ancestry"));
        }
        path.push(component);
        if !path.is_absolute() {
            continue;
        }
        let file = open_nofollow(&path, true)?;
        let security = winproc::file_security(&file)?;
        if !coreacl::judge_object("repair-ancestor", &security).is_empty() {
            return Err(invalid("untrusted repair store ancestor ACL"));
        }
    }
    Ok(())
}

pub(super) fn absent(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "repair destination occupied",
        )),
    }
}

pub(super) fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn leaf(value: &str) -> io::Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value.contains(['/', '\\', ':', '\0'])
        || value.ends_with(['.', ' '])
        || value.chars().any(char::is_control)
        || !matches!(
            (
                Path::new(value).components().next(),
                Path::new(value).components().nth(1)
            ),
            (Some(Component::Normal(_)), None)
        )
    {
        return Err(invalid("invalid repair member name"));
    }
    let base = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && matches!(base.as_bytes()[3], b'1'..=b'9'))
    {
        return Err(invalid("reserved repair member name"));
    }
    Ok(())
}

pub(super) fn open_nofollow(path: &Path, directory: bool) -> io::Result<File> {
    if !cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "no protected repair object API",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(
            nix::libc::O_NOFOLLOW
                | nix::libc::O_CLOEXEC
                | nix::libc::O_NONBLOCK
                | if directory { nix::libc::O_DIRECTORY } else { 0 },
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
        use windows_sys::Win32::Storage::FileSystem::*;
        options.custom_flags(
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
        );
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        let file = options.open(path)?;
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(invalid("repair object is a reparse point"));
        }
        return Ok(file);
    }
    #[cfg(not(windows))]
    options.open(path)
}

#[cfg(unix)]
pub(super) fn identity(file: &File) -> io::Result<Identity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(Identity(format!(
        "u{:x}-{:x}",
        metadata.dev(),
        metadata.ino()
    )))
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "query identity and link count of a held store HANDLE"
)]
fn information(
    file: &File,
) -> io::Result<windows_sys::Win32::Storage::FileSystem::BY_HANDLE_FILE_INFORMATION> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut value = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
    // SAFETY: File owns the live HANDLE; value is written only on API success.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle().cast(), value.as_mut_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful API initialized the complete structure.
    Ok(unsafe { value.assume_init() })
}

#[cfg(windows)]
pub(super) fn identity(file: &File) -> io::Result<Identity> {
    let value = information(file)?;
    Ok(Identity(format!(
        "w{:x}-{:x}-{:x}",
        value.dwVolumeSerialNumber, value.nFileIndexHigh, value.nFileIndexLow
    )))
}

#[cfg(windows)]
fn link_count(file: &File) -> io::Result<u32> {
    Ok(information(file)?.nNumberOfLinks)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> io::Result<()> {
    open_nofollow(path, true)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(path: &Path) -> io::Result<()> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::*;
    // FlushFileBuffers requires GENERIC_WRITE. Do not convert an unsupported
    // filesystem/directory flush into success or flush an entire system volume.
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    if !file.metadata()?.is_dir()
        || file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return Err(invalid("repair sync directory is a reparse point"));
    }
    file.sync_all()
}

#[cfg(not(any(unix, windows)))]
pub(super) fn identity(_file: &File) -> io::Result<Identity> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no repair object identity API",
    ))
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_path: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no repair directory barrier API",
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_exclusive(_from: &Path, _to: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no exclusive repair rename API",
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[allow(
    unsafe_code,
    reason = "atomic same-filesystem rename without replacement"
)]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let from =
        CString::new(from.as_os_str().as_bytes()).map_err(|_| invalid("NUL in repair path"))?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(|_| invalid("NUL in repair path"))?;
    #[cfg(target_os = "linux")]
    // SAFETY: both C paths live across the call; RENAME_NOREPLACE forbids overwrite.
    let result = unsafe {
        nix::libc::syscall(
            nix::libc::SYS_renameat2,
            nix::libc::AT_FDCWD,
            from.as_ptr(),
            nix::libc::AT_FDCWD,
            to.as_ptr(),
            nix::libc::RENAME_NOREPLACE,
        )
    };
    #[cfg(target_os = "macos")]
    // SAFETY: both C paths live across the call; RENAME_EXCL forbids overwrite.
    let result =
        unsafe { nix::libc::renamex_np(from.as_ptr(), to.as_ptr(), nix::libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod native_tests {
    use super::*;

    #[test]
    fn exclusive_rename_preserves_an_occupied_destination_and_held_object_identity() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let target = root.path().join("target");
        fs::write(&source, b"source").unwrap();
        fs::write(&target, b"target").unwrap();
        let held = open_nofollow(&source, false).unwrap();
        let selected = identity(&held).unwrap();
        assert!(rename_exclusive(&source, &target).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"source");
        assert_eq!(fs::read(&target).unwrap(), b"target");
        fs::remove_file(&target).unwrap();
        rename_exclusive(&source, &target).unwrap();
        assert_eq!(
            identity(&open_nofollow(&target, false).unwrap()).unwrap(),
            selected
        );
        assert_eq!(identity(&held).unwrap(), selected);
    }

    #[test]
    fn native_hardlink_count_and_directory_barrier_type_are_observed() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let alias = root.path().join("alias");
        fs::write(&source, b"keep").unwrap();
        fs::hard_link(&source, &alias).unwrap();
        let held = open_nofollow(&source, false).unwrap();
        assert_eq!(
            identity(&held).unwrap(),
            identity(&open_nofollow(&alias, false).unwrap()).unwrap()
        );
        #[cfg(windows)]
        assert_eq!(link_count(&held).unwrap(), 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(held.metadata().unwrap().nlink(), 2);
        }
        assert!(sync_directory(&source).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"keep");
    }
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "same-volume write-through move without replacement"
)]
fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: both NUL-terminated paths live across this call. No replace,
    // cross-volume copy, reboot or delayed operation flags are permitted.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
