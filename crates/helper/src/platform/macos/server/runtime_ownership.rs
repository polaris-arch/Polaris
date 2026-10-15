//! Runtime ownership return uses pinned directory/file descriptors throughout.
//! User-writable ancestor renames never redirect a later enumeration or fchown.

use nix::fcntl::{open, openat, AtFlags, OFlag};
use nix::sys::stat::{fstat, fstatat, FileStat, Mode, SFlag};
use std::fs::File;
use std::io;
use std::path::{Component, Path};

const DIRECTORY: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);
const MAX_ENTRIES: usize = 100_000;
const MAX_DEPTH: usize = 64;

fn rejected(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn open_directory(path: &Path) -> io::Result<File> {
    if !path.is_absolute() {
        return Err(rejected("absolute runtime directory required"));
    }
    let mut directory = File::from(open(Path::new("/"), DIRECTORY, Mode::empty())?);
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = File::from(openat(&directory, name, DIRECTORY, Mode::empty())?);
            }
            _ => return Err(rejected("non-normal runtime directory component")),
        }
    }
    Ok(directory)
}

fn walk_child(
    parent: &File,
    name: &std::ffi::OsStr,
    depth: usize,
    remaining: &mut usize,
    visit: &mut impl FnMut(&File, &FileStat) -> io::Result<()>,
) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    if depth > MAX_DEPTH || *remaining == 0 {
        return Err(rejected("runtime tree exceeds ownership budget"));
    }
    *remaining -= 1;
    let before = match fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(metadata) => metadata,
        Err(nix::errno::Errno::ENOENT) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let kind = SFlag::from_bits_truncate(before.st_mode) & SFlag::S_IFMT;
    if kind != SFlag::S_IFREG && kind != SFlag::S_IFDIR {
        return Err(rejected(
            "runtime ownership refuses links and special files",
        ));
    }
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK;
    let file = File::from(openat(parent, name, flags, Mode::empty())?);
    let metadata = fstat(&file)?;
    if before.st_dev != metadata.st_dev
        || before.st_ino != metadata.st_ino
        || (SFlag::from_bits_truncate(metadata.st_mode) & SFlag::S_IFMT) != kind
        || (kind == SFlag::S_IFREG && metadata.st_nlink != 1)
    {
        return Err(rejected("runtime entry identity/type/link count changed"));
    }
    visit(&file, &metadata)?;
    if kind == SFlag::S_IFDIR {
        let mut directory = nix::dir::Dir::from_fd(openat(&file, ".", DIRECTORY, Mode::empty())?)?;
        let mut names = Vec::new();
        for entry in directory.iter() {
            let entry = entry?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if names.len() == *remaining {
                return Err(rejected("runtime tree exceeds ownership budget"));
            }
            names.push(std::ffi::OsStr::from_bytes(bytes).to_owned());
        }
        for child in names {
            walk_child(&file, &child, depth + 1, remaining, visit)?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn runtime_owner(root: &File) -> io::Result<(u32, u32)> {
    let metadata = fstat(root)?;
    if super::should_skip_confdir_chown(metadata.st_uid) {
        return Err(rejected("runtime directory unexpectedly belongs to root"));
    }
    Ok((metadata.st_uid, metadata.st_gid))
}

#[cfg(target_os = "macos")]
pub(super) fn return_runtime_ownership(path: &Path) -> io::Result<()> {
    let root = open_directory(path)?;
    let (uid, gid) = runtime_owner(&root)?;
    let mut remaining = MAX_ENTRIES;
    for name in super::CHOWN_SUBDIRS.into_iter().chain(super::CHOWN_FILES) {
        walk_child(
            &root,
            name.as_ref(),
            0,
            &mut remaining,
            &mut |file, metadata| {
                if super::should_chown_entry(metadata.st_uid) {
                    std::os::unix::fs::fchown(file, Some(uid), Some(gid))?;
                }
                Ok(())
            },
        )?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn prepare_cache(path: &Path) -> io::Result<()> {
    let root = open_directory(path)?;
    let (uid, gid) = runtime_owner(&root)?;
    let flags =
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK;
    let file = File::from(openat(
        &root,
        super::CHOWN_FILES[0],
        flags,
        Mode::from_bits_truncate(0o600),
    )?);
    let metadata = fstat(&file)?;
    if (SFlag::from_bits_truncate(metadata.st_mode) & SFlag::S_IFMT) != SFlag::S_IFREG
        || metadata.st_nlink != 1
    {
        return Err(rejected("cache must be an unlinked regular-file object"));
    }
    std::os::unix::fs::fchown(&file, Some(uid), Some(gid))
}

#[cfg(test)]
mod tests;
