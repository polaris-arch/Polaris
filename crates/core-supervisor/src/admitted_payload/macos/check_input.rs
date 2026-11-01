//! A fixed, installer-protected pending-input namespace is also conservative
//! crash debt. A successor daemon never deletes or blesses an unknown snapshot.
use super::*;
use std::io::Write;

const DIRECTORY: &str = ".validation-check";
const CONFIG: &str = "config.json";

pub(crate) fn require_check_debt_absent(store: &Path) -> Result<(), AdmissionError> {
    require_root()?;
    let _protected = ProtectedPath::open(store, true)?;
    match std::fs::symlink_metadata(store.join(DIRECTORY)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(rejected(
            AdmissionErrorKind::Unknown,
            "Root Check input/custody remains unconfirmed",
        )),
    }
}

pub(crate) struct CheckSnapshot {
    parent: ProtectedPath,
    directory: File,
    identity: (u64, u64),
    file: Option<File>,
    member: (u64, u64),
    sealed: Option<Member>,
    member_removed: bool,
    retired: bool,
}
impl CheckSnapshot {
    pub(super) fn create(parent: ProtectedPath) -> Result<Self, AdmissionError> {
        parent.unchanged()?;
        no_extended_acl(&parent.file)?;
        mkdirat(&parent.file, DIRECTORY, Mode::from_bits_truncate(0o700))
            .map_err(std::io::Error::from)?;
        parent.file.sync_all()?;
        let directory = File::from(
            openat(&parent.file, DIRECTORY, open_flags(true), Mode::empty())
                .map_err(std::io::Error::from)?,
        );
        private_mode(&directory, true, 0o700)?;
        let m = directory.metadata()?;
        let file = File::from(
            openat(
                &directory,
                CONFIG,
                OFlag::O_RDWR
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(std::io::Error::from)?,
        );
        private_mode(&file, false, 0o600)?;
        let f = file.metadata()?;
        directory.sync_all()?;
        Ok(Self {
            parent,
            directory,
            identity: (m.dev(), m.ino()),
            file: Some(file),
            member: (f.dev(), f.ino()),
            sealed: None,
            member_removed: false,
            retired: false,
        })
    }
    fn current(&self) -> Result<(), AdmissionError> {
        self.parent.unchanged()?;
        let now = ProtectedPath::open(&self.parent.path.join(DIRECTORY), true)?;
        let m = now.file.metadata()?;
        if (m.dev(), m.ino()) != self.identity {
            return Err(rejected(
                AdmissionErrorKind::Changed,
                "Root Check namespace changed",
            ));
        }
        private_mode(&now.file, true, 0o700)?;
        Ok(())
    }
    pub(crate) fn append(&mut self, offset: usize, bytes: &[u8]) -> Result<(), AdmissionError> {
        self.current()?;
        let f = self.file.as_mut().ok_or_else(|| {
            rejected(
                AdmissionErrorKind::Changed,
                "Root Check input already sealed",
            )
        })?;
        private_mode(f, false, 0o600)?;
        if f.metadata()?.len() != offset as u64 {
            return Err(rejected(
                AdmissionErrorKind::Changed,
                "Root Check input offset changed",
            ));
        }
        f.write_all(bytes)?;
        Ok(())
    }
    pub(crate) fn seal(&mut self, len: usize, digest: &[u8; 32]) -> Result<(), AdmissionError> {
        use sha2::Digest;
        self.current()?;
        let f = self.file.as_mut().ok_or_else(|| {
            rejected(
                AdmissionErrorKind::Changed,
                "Root Check input already committed",
            )
        })?;
        private_mode(f, false, 0o600)?;
        if f.metadata()?.len() != len as u64 {
            return Err(rejected(
                AdmissionErrorKind::Mismatch,
                "Root Check length differs",
            ));
        }
        f.sync_all()?;
        f.seek(SeekFrom::Start(0))?;
        let mut hash = sha2::Sha256::new();
        let mut buffer = [0; 64 * 1024];
        loop {
            let n = f.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        if hash.finalize().as_slice() != digest {
            return Err(rejected(
                AdmissionErrorKind::Mismatch,
                "Root Check digest differs",
            ));
        }
        fchmod(&*f, Mode::from_bits_truncate(0o400)).map_err(std::io::Error::from)?;
        f.sync_all()?;
        drop(self.file.take());
        let file = File::from(
            openat(&self.directory, CONFIG, open_flags(false), Mode::empty())
                .map_err(std::io::Error::from)?,
        );
        private_mode(&file, false, 0o400)?;
        let m = file.metadata()?;
        if (m.dev(), m.ino()) != self.member {
            return Err(rejected(
                AdmissionErrorKind::Changed,
                "Root Check input inode differs",
            ));
        }
        self.sealed = Some(Member {
            file,
            path: self.parent.path.join(DIRECTORY).join(CONFIG),
            stamp: stamp(&m),
        });
        Ok(())
    }
    pub(crate) fn path(&self) -> Result<&Path, AdmissionError> {
        self.current()?;
        let m = self.sealed.as_ref().ok_or_else(|| {
            rejected(
                AdmissionErrorKind::Mismatch,
                "Root Check input is not sealed",
            )
        })?;
        if !m.current() {
            return Err(rejected(
                AdmissionErrorKind::Changed,
                "Root Check sealed input changed",
            ));
        }
        Ok(&m.path)
    }
    /// Only called by the original registry after actual wait and both pipe joins,
    /// or before any child factory was entered. No Drop cleanup asserts retirement.
    pub(crate) fn retire(&mut self) -> Result<(), AdmissionError> {
        if self.retired {
            return Ok(());
        }
        self.current()?;
        if self.member_removed {
            match openat(&self.directory, CONFIG, open_flags(false), Mode::empty()) {
                Err(nix::errno::Errno::ENOENT) => {}
                _ => {
                    return Err(rejected(
                        AdmissionErrorKind::Changed,
                        "Root Check retired member reappeared",
                    ))
                }
            }
        } else {
            if let Some(member) = self.sealed.as_ref() {
                if !member.current() {
                    return Err(rejected(
                        AdmissionErrorKind::Changed,
                        "Root Check sealed retirement input changed",
                    ));
                }
            }
            let f = File::from(
                openat(&self.directory, CONFIG, open_flags(false), Mode::empty())
                    .map_err(std::io::Error::from)?,
            );
            let m = f.metadata()?;
            private_mode(&f, false, if self.sealed.is_some() { 0o400 } else { 0o600 })?;
            if (m.dev(), m.ino()) != self.member || m.nlink() != 1 || m.uid() != 0 || !m.is_file() {
                return Err(rejected(
                    AdmissionErrorKind::Changed,
                    "Root Check retirement member changed",
                ));
            }
            no_extended_acl(&f)?;
            unlinkat(&self.directory, CONFIG, UnlinkatFlags::NoRemoveDir)
                .map_err(std::io::Error::from)?;
            self.member_removed = true;
        }
        self.directory.sync_all()?;
        unlinkat(&self.parent.file, DIRECTORY, UnlinkatFlags::RemoveDir)
            .map_err(std::io::Error::from)?;
        // LAST fallible step is removal of this exact empty debt namespace.
        // A crash may conservatively resurrect it; never add a fallible flush
        // after removing the only durable Unknown witness.
        self.retired = true;
        Ok(())
    }
}
