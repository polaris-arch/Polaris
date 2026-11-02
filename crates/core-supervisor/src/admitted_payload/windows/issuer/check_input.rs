//! Original SYSTEM Check input. CREATE_NEW debt survives a crashed issuer.
use super::*;
use std::io::Write;

fn path(owner: &Cohort) -> Result<PathBuf, AdmissionError> {
    Ok(owner
        .root
        .parent()
        .ok_or_else(unavailable)?
        .join(".validation-check"))
}
pub(super) fn require_debt_absent(owner: &Cohort) -> Result<(), AdmissionError> {
    match fs::symlink_metadata(path(owner)?) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err(AdmissionError::new(
            AdmissionErrorKind::Unknown,
            "SYSTEM Check snapshot remains unconfirmed",
        )),
    }
}
pub(in super::super) struct CheckSnapshot {
    owner: Arc<Cohort>,
    path: PathBuf,
    file: Option<File>,
    id: Identity,
    sealed: bool,
    retired: bool,
}
impl CheckSnapshot {
    pub(in super::super) fn create(owner: Arc<Cohort>) -> Result<Self, AdmissionError> {
        system_actor()?;
        if !owner.is_current() {
            return Err(unavailable());
        }
        let path = path(&owner)?;
        let sd = Descriptor::private()?;
        let file = File::from(owned(unsafe {
            CreateFileW(
                wide(&path)?.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | DELETE,
                0,
                &sd.attributes(),
                CREATE_NEW,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        })?);
        private_parent(&file)?;
        let id = identity(&file, true)?;
        Ok(Self {
            owner,
            path,
            file: Some(file),
            id,
            sealed: false,
            retired: false,
        })
    }
    fn current(&self) -> Result<(), AdmissionError> {
        system_actor()?;
        if self.retired || !self.owner.is_current() {
            return Err(unavailable());
        }
        let f = self.file.as_ref().ok_or_else(unavailable)?;
        let id = identity(f, true)?;
        if id.volume != self.id.volume || id.id != self.id.id {
            return Err(error("Check input identity changed"));
        }
        private_parent(f)?;
        Ok(())
    }
    pub(in super::super) fn append(
        &mut self,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), AdmissionError> {
        self.current()?;
        let f = self.file.as_mut().ok_or_else(unavailable)?;
        if self.sealed
            || bytes.is_empty()
            || offset
                .checked_add(bytes.len())
                .is_none_or(|n| n > 8 * 1024 * 1024)
            || f.metadata()?.len() != offset as u64
        {
            return Err(error("Check input offset/size differs"));
        }
        f.write_all(bytes)?;
        Ok(())
    }
    pub(in super::super) fn seal(
        &mut self,
        len: usize,
        digest: &[u8; 32],
    ) -> Result<(), AdmissionError> {
        use sha2::Digest;
        self.current()?;
        let f = self.file.as_mut().ok_or_else(unavailable)?;
        if self.sealed || len == 0 || len > 8 * 1024 * 1024 || f.metadata()?.len() != len as u64 {
            return Err(error("Check input length differs"));
        }
        f.sync_all()?;
        f.seek(SeekFrom::Start(0))?;
        let mut hash = sha2::Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let n = f.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        if hash.finalize().as_slice() != digest {
            return Err(error("Check input digest differs"));
        }
        let expected = identity(f, true)?;
        drop(self.file.take());
        // Protected SYSTEM parent, same exact FileID, no WRITE/DELETE sharing.
        let sealed = File::from(owned(unsafe {
            CreateFileW(
                wide(&self.path)?.as_ptr(),
                GENERIC_READ | DELETE | READ_CONTROL,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        })?);
        if identity(&sealed, true)? != expected {
            return Err(error("Check sealed input changed"));
        }
        self.file = Some(sealed);
        self.sealed = true;
        Ok(())
    }
    pub(in super::super) fn input(&self) -> Result<File, AdmissionError> {
        self.current()?;
        if !self.sealed {
            return Err(error("Check input not sealed"));
        }
        let mut f = self.file.as_ref().ok_or_else(unavailable)?.try_clone()?;
        f.seek(SeekFrom::Start(0))?;
        Ok(f)
    }
    pub(in super::super) fn retire(&mut self) -> Result<(), AdmissionError> {
        if self.retired {
            return Ok(());
        }
        self.current()?;
        let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
        let f = self.file.as_ref().ok_or_else(unavailable)?;
        check(unsafe {
            SetFileInformationByHandle(
                f.as_raw_handle(),
                FileDispositionInfo,
                (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
                mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            )
        })?;
        // Only original registry calls this after actual wait and both reader joins.
        drop(self.file.take());
        self.retired = true;
        Ok(())
    }
}
