//! Root provisioning and pinned validation for the shared Linux claims v2 store.
//!
//! The Go lease owner alone creates and retires birth claims. This module only
//! publishes the root-owned deployment files and checks their original inodes.
//! Existing layouts are never repaired or migrated; failed private provisioning
//! remains private for controlled recovery. No network or owner proof is issued.

use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::{Component, Path};

use nix::errno::Errno;
use nix::fcntl::{open, openat, AtFlags, OFlag};
use nix::sys::stat::{fchmod, fstatat, mkdirat, Mode};

pub const CLAIMS_DIRECTORY: &str = "/run/polaris-sing-tun-claims";
const MARKER: &str = ".protocol";
const ALLOCATOR: &str = ".allocator";
const PROTOCOL: &[u8] = b"{\"protocol\":\"polaris-sing-tun-claims\",\"version\":2,\"layout\":\"sticky-flat\",\"allocator\":\".allocator\"}\n";
const DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

/// Held directory chain and readonly deployment files. Revalidation never
/// reopens a name or adopts a replacement, including a valid-looking one.
#[derive(Debug)]
pub struct ClaimsDeployment {
    parents: Vec<File>,
    parent_names: Vec<OsString>,
    name: OsString,
    directory: File,
    marker: File,
    allocator: File,
}

/// Service initialization only. A preexisting directory must already be v2.
pub fn provision() -> io::Result<ClaimsDeployment> {
    provision_at(Path::new(CLAIMS_DIRECTORY))
}

/// Open the same fixed deployment for helper and direct-CAP consumers.
/// Missing or incompatible deployments are errors, with no fallback or writes.
pub fn open_deployment() -> io::Result<ClaimsDeployment> {
    open_deployment_at(Path::new(CLAIMS_DIRECTORY))
}

impl ClaimsDeployment {
    /// Check every held-vs-linked inode and the complete root-owned protocol.
    pub fn validate(&self) -> io::Result<()> {
        self.validate_mode(0o1777)
    }

    fn validate_mode(&self, mode: u32) -> io::Result<()> {
        validate_parents(&self.parents, &self.parent_names)?;
        validate_directory(&self.directory, mode)?;
        validate_link(
            self.parents.last().expect("root parent"),
            &self.name,
            &self.directory,
        )?;
        validate_file(&self.directory, MARKER, &self.marker, PROTOCOL)?;
        validate_file(&self.directory, ALLOCATOR, &self.allocator, b"")
    }
}

fn invalid(detail: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, detail)
}

fn validate_parent(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    let mode = metadata.mode();
    if !metadata.is_dir() || metadata.uid() != 0 || (mode & 0o22 != 0 && mode & 0o1000 == 0) {
        return Err(invalid(
            "claims parent must be a trusted root-owned directory",
        ));
    }
    Ok(())
}

fn validate_directory(file: &File, mode: u32) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o7777 != mode {
        return Err(invalid(
            "claims base has incompatible owner/type/mode; controlled deployment required",
        ));
    }
    Ok(())
}

fn validate_link(parent: &File, name: &std::ffi::OsStr, file: &File) -> io::Result<()> {
    let held = file.metadata()?;
    let linked = fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW)?;
    if held.dev() != linked.st_dev
        || held.ino() != linked.st_ino
        || held.mode() & nix::libc::S_IFMT != linked.st_mode & nix::libc::S_IFMT
    {
        return Err(invalid(
            "claims held inode no longer matches its linked name",
        ));
    }
    Ok(())
}

fn validate_parents(parents: &[File], names: &[OsString]) -> io::Result<()> {
    for (index, parent) in parents.iter().enumerate() {
        validate_parent(parent)?;
        if index != 0 {
            validate_link(&parents[index - 1], &names[index - 1], parent)?;
        }
    }
    Ok(())
}

fn open_parent(path: &Path) -> io::Result<(Vec<File>, Vec<OsString>, OsString)> {
    let mut components = path.components();
    if components.next() != Some(Component::RootDir) {
        return Err(invalid("claims path must be absolute"));
    }
    let mut names = Vec::new();
    for component in components {
        match component {
            Component::Normal(name) => names.push(name.to_os_string()),
            _ => return Err(invalid("claims path contains a non-normal component")),
        }
    }
    let name = names
        .pop()
        .ok_or_else(|| invalid("claims path has no base name"))?;
    let mut parents = vec![File::from(open("/", DIRECTORY_FLAGS, Mode::empty())?)];
    for component in &names {
        let parent = parents.last().expect("root parent");
        validate_parent(parent)?;
        let next = File::from(openat(
            parent,
            component.as_os_str(),
            DIRECTORY_FLAGS,
            Mode::empty(),
        )?);
        validate_link(parent, component, &next)?;
        parents.push(next);
    }
    validate_parents(&parents, &names)?;
    Ok((parents, names, name))
}

fn readonly_file(directory: &File, name: &str) -> io::Result<File> {
    // NONBLOCK also makes a substituted FIFO fail validation without waiting
    // for a writer; it has no effect on the required regular file.
    Ok(File::from(openat(
        directory,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC | OFlag::O_NONBLOCK,
        Mode::empty(),
    )?))
}

fn validate_file(directory: &File, name: &str, file: &File, expected: &[u8]) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o7777 != 0o444
        || metadata.len() != expected.len() as u64
    {
        return Err(invalid(
            "claims deployment file has incompatible owner/type/mode/links/size",
        ));
    }
    validate_link(directory, std::ffi::OsStr::new(name), file)?;
    let mut actual = vec![0; expected.len()];
    let mut offset = 0;
    while offset < actual.len() {
        let count = file.read_at(&mut actual[offset..], offset as u64)?;
        if count == 0 {
            return Err(invalid("claims protocol read was incomplete"));
        }
        offset += count;
    }
    if actual != expected {
        return Err(invalid("claims protocol bytes are incompatible"));
    }
    validate_link(directory, std::ffi::OsStr::new(name), file)
}

fn from_parent(
    parents: Vec<File>,
    parent_names: Vec<OsString>,
    name: OsString,
) -> io::Result<ClaimsDeployment> {
    let directory = File::from(openat(
        parents.last().expect("root parent"),
        name.as_os_str(),
        DIRECTORY_FLAGS,
        Mode::empty(),
    )?);
    from_directory(parents, parent_names, name, directory)
}

fn from_directory(
    parents: Vec<File>,
    parent_names: Vec<OsString>,
    name: OsString,
    directory: File,
) -> io::Result<ClaimsDeployment> {
    validate_parents(&parents, &parent_names)?;
    validate_link(parents.last().expect("root parent"), &name, &directory)?;
    let marker = readonly_file(&directory, MARKER)?;
    let allocator = readonly_file(&directory, ALLOCATOR)?;
    Ok(ClaimsDeployment {
        parents,
        parent_names,
        name,
        directory,
        marker,
        allocator,
    })
}

fn open_deployment_at(path: &Path) -> io::Result<ClaimsDeployment> {
    let (parents, names, name) = open_parent(path)?;
    let deployment = from_parent(parents, names, name)?;
    deployment.validate()?;
    Ok(deployment)
}

fn create_file(directory: &File, name: &str, content: &[u8]) -> io::Result<File> {
    let mut file = File::from(openat(
        directory,
        name,
        OFlag::O_RDWR | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )?);
    file.write_all(content)?;
    fchmod(&file, Mode::from_bits_truncate(0o444))?;
    file.sync_all()?;
    validate_file(directory, name, &file, content)?;
    Ok(file)
}

fn provision_at(path: &Path) -> io::Result<ClaimsDeployment> {
    if !nix::unistd::geteuid().is_root() {
        return Err(invalid("Linux claims provisioning requires root"));
    }
    let (parents, names, name) = open_parent(path)?;
    let parent = parents.last().expect("root parent");
    match mkdirat(parent, name.as_os_str(), Mode::from_bits_truncate(0o700)) {
        Err(Errno::EEXIST) => {
            let deployment = from_parent(parents, names, name)?;
            deployment.validate()?;
            return Ok(deployment);
        }
        Err(error) => return Err(error.into()),
        Ok(()) => {}
    }
    let directory = File::from(openat(
        parent,
        name.as_os_str(),
        DIRECTORY_FLAGS,
        Mode::empty(),
    )?);
    validate_parents(&parents, &names)?;
    validate_directory(&directory, 0o700)?;
    validate_link(parent, &name, &directory)?;
    let marker = create_file(&directory, MARKER, PROTOCOL)?;
    let allocator = create_file(&directory, ALLOCATOR, b"")?;
    directory.sync_all()?;
    let deployment = from_directory(parents, names, name, directory)?;
    // Keep the creation descriptors until their readonly replacements are
    // verified. Publishing never upgrades a preexisting or swapped object.
    validate_link(&deployment.directory, std::ffi::OsStr::new(MARKER), &marker)?;
    validate_link(
        &deployment.directory,
        std::ffi::OsStr::new(ALLOCATOR),
        &allocator,
    )?;
    publish_private_deployment(deployment, File::sync_all)
}

fn publish_private_deployment(
    deployment: ClaimsDeployment,
    sync: impl Fn(&File) -> io::Result<()>,
) -> io::Result<ClaimsDeployment> {
    deployment.validate_mode(0o700)?;
    fchmod(&deployment.directory, Mode::from_bits_truncate(0o1777))?;
    sync(&deployment.directory)?;
    sync(deployment.parents.last().expect("root parent"))?;
    deployment.validate()?;
    Ok(deployment)
}

#[cfg(test)]
mod tests;
