#![allow(
    unsafe_code,
    reason = "native ACL fixtures mutate only anonymous temporary test files"
)]
use super::*;
use std::ffi::c_void;
use std::os::fd::AsRawFd;

unsafe extern "C" {
    fn acl_init(count: i32) -> *mut c_void;
    fn acl_free(value: *mut c_void) -> i32;
    fn acl_set_fd_np(fd: i32, acl: *mut c_void, kind: i32) -> i32;
    fn acl_create_entry(acl: *mut *mut c_void, entry: *mut *mut c_void) -> i32;
    fn acl_set_tag_type(entry: *mut c_void, tag: u32) -> i32;
    fn acl_set_qualifier(entry: *mut c_void, qualifier: *const c_void) -> i32;
    fn acl_get_permset(entry: *mut c_void, set: *mut *mut c_void) -> i32;
    fn acl_add_perm(set: *mut c_void, permission: u32) -> i32;
    fn mbr_uid_to_uuid(uid: u32, uuid: *mut u8) -> i32;
}

fn anonymous_file() -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = std::env::temp_dir().join(format!(
        "polaris-native-acl-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&name)?;
    std::fs::remove_file(name)?;
    Ok(file)
}

#[test]
fn native_absent_acl_is_accepted_but_invalid_fd_is_not() {
    let file = anonymous_file().unwrap();
    // A fresh anonymous fixture has no ACL; both policies accept absence.
    assert!(read_acl(file.as_raw_fd()).unwrap().is_none());
    validate_acl(&file, false).unwrap();
    no_extended_acl(&file).unwrap();
    assert_eq!(
        read_acl(-1).unwrap_err().raw_os_error(),
        Some(nix::libc::EBADF)
    );
}

#[test]
fn native_empty_acl_is_accepted_and_allow_write_is_rejected() {
    let file = anonymous_file().unwrap();
    // SAFETY: every pointer is owned by this fixture and remains live through
    // the corresponding native call. Only the tempfile descriptor is changed.
    unsafe {
        let mut acl = acl_init(1);
        assert!(!acl.is_null());
        assert_eq!(acl_set_fd_np(file.as_raw_fd(), acl, 0x100), 0);
        validate_acl(&file, false).unwrap();
        no_extended_acl(&file).unwrap();
        let mut entry = std::ptr::null_mut();
        assert_eq!(acl_create_entry(&mut acl, &mut entry), 0);
        assert_eq!(acl_set_tag_type(entry, 1), 0);
        let mut uuid = [0u8; 16];
        assert_eq!(mbr_uid_to_uuid(nix::libc::geteuid(), uuid.as_mut_ptr()), 0);
        assert_eq!(acl_set_qualifier(entry, uuid.as_ptr().cast()), 0);
        let mut permissions = std::ptr::null_mut();
        assert_eq!(acl_get_permset(entry, &mut permissions), 0);
        assert_eq!(acl_add_perm(permissions, 1 << 2), 0);
        assert_eq!(acl_set_fd_np(file.as_raw_fd(), acl, 0x100), 0);
        assert!(validate_acl(&file, false).is_err());
        assert!(no_extended_acl(&file).is_err());
        assert_eq!(acl_free(acl), 0);
    }
}
