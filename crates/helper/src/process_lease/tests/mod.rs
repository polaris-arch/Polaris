use super::*;
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn file(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
}

#[test]
fn descriptor_identity_mode_and_shared_description_lifetime() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lease");
    let original = file(&path);
    let retained = original.try_clone().unwrap();
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    apply(&path, original.as_raw_fd(), false).unwrap();
    drop(original); // Simulates loss of the controller's descriptor.
    assert!(apply(&path, contender.as_raw_fd(), false).is_err());
    drop(retained); // The last inherited description must close first.
    apply(&path, contender.as_raw_fd(), false).unwrap();
    apply(&path, contender.as_raw_fd(), true).unwrap();
    let other_path = root.path().join("other");
    let other = file(&other_path);
    assert!(apply(&path, other.as_raw_fd(), false).is_err());
    assert!(apply(&path, -1, false).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert!(apply(&path, contender.as_raw_fd(), false).is_err());
    assert!(path.exists()); // No stale-owner guess or unlink fallback.
}
#[test]
fn live_inherited_child_keeps_lease_after_controller_description_closes() {
    use nix::fcntl::{fcntl, FcntlArg, FdFlag};
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("lease");
    let owner = file(&path);
    apply(&path, owner.as_raw_fd(), false).unwrap();
    let flags = fcntl(&owner, FcntlArg::F_GETFD).unwrap();
    fcntl(
        &owner,
        FcntlArg::F_SETFD(FdFlag::from_bits_truncate(flags) & !FdFlag::FD_CLOEXEC),
    )
    .unwrap();
    let mut child = std::process::Command::new("/bin/sh")
        .args(["-c", "read -r finish"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(owner);
    let contender = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let blocked = apply(&path, contender.as_raw_fd(), false).is_err();
    child.stdin.take().unwrap().write_all(b"finish\n").unwrap();
    assert!(child.wait().unwrap().success());
    assert!(
        blocked,
        "inherited child still owns the original file description"
    );
    apply(&path, contender.as_raw_fd(), false).unwrap();
}
