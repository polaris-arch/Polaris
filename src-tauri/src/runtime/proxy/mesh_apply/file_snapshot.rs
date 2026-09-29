//! Identity of the exact file object used by artifact and rule-source reads.
//! A pathname's metadata and a read handle's metadata must be compared using
//! an OS file identity, not size/timestamps alone. Windows std MetadataExt's
//! by-handle identity accessors are unstable, so obtain FileIdInfo from the
//! same open handle as its metadata. FileIdInfo retains ReFS's full 128-bit ID.

use std::fs::{self, File, Metadata};
use std::io;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
};

#[derive(Debug)]
pub(super) struct FileSnapshot {
    metadata: Metadata,
    #[cfg(unix)]
    identity: (u64, u64),
    #[cfg(windows)]
    identity: (u64, [u8; 16]),
}

impl FileSnapshot {
    pub(super) fn path(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        {
            // Query metadata and identity through one handle to the path
            // object itself. Opening a reparse point's target here would make
            // a path-swap check compare the wrong file.
            Self::opened(&Self::open_path(path)?)
        }
        #[cfg(not(windows))]
        {
            Self::from_metadata(fs::symlink_metadata(path)?)
        }
    }

    pub(super) fn opened(file: &File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        #[cfg(windows)]
        {
            let identity = crate::windows_file_id::identity(file)?;
            Ok(Self { metadata, identity })
        }
        #[cfg(not(windows))]
        {
            Self::from_metadata(metadata)
        }
    }

    #[cfg(windows)]
    pub(super) fn open_path(path: &Path) -> io::Result<File> {
        let mut options = fs::OpenOptions::new();
        options
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
        options.open(path)
    }

    #[cfg(not(windows))]
    fn from_metadata(metadata: Metadata) -> io::Result<Self> {
        #[cfg(unix)]
        {
            let identity = (metadata.dev(), metadata.ino());
            Ok(Self { metadata, identity })
        }
        #[cfg(not(unix))]
        {
            Ok(Self { metadata })
        }
    }

    pub(super) fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    pub(super) fn is_file(&self) -> bool {
        self.metadata.is_file()
    }

    pub(super) fn is_dir(&self) -> bool {
        self.metadata.is_dir()
    }

    pub(super) fn is_reparse(&self) -> bool {
        #[cfg(windows)]
        {
            self.metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        }
        #[cfg(not(windows))]
        {
            self.metadata.file_type().is_symlink()
        }
    }

    pub(super) fn len(&self) -> u64 {
        self.metadata.len()
    }

    pub(super) fn same_identity(&self, other: &Self) -> bool {
        #[cfg(any(unix, windows))]
        {
            self.identity == other.identity
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = other;
            false // Unknown file identity cannot establish trust.
        }
    }

    pub(super) fn same_snapshot(&self, other: &Self) -> bool {
        self.same_identity(other)
            && self.len() == other.len()
            && matches!((self.metadata.modified(), other.metadata.modified()), (Ok(a), Ok(b)) if a == b)
            && {
                #[cfg(unix)]
                {
                    self.metadata.ctime() == other.metadata.ctime()
                        && self.metadata.ctime_nsec() == other.metadata.ctime_nsec()
                }
                #[cfg(not(unix))]
                {
                    true
                }
            }
    }
}

#[cfg(test)]
mod tests;
