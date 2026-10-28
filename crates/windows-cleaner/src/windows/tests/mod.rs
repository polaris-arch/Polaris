use super::*;
use std::os::windows::fs::OpenOptionsExt;
fn parent(path: &Path) -> File {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .unwrap()
}
#[test]
fn relative_open_rejects_traversal_ads_and_separators() {
    let temp = tempfile::tempdir().unwrap();
    let dir = parent(temp.path());
    for name in [
        "..",
        ".",
        "nested\\file",
        "nested/file",
        "file:stream",
        "",
        "a\0b",
    ] {
        assert!(
            child(&dir, OsStr::new(name), true, false, null_mut()).is_err(),
            "{name:?}"
        );
    }
}
#[test]
fn pinned_object_blocks_replacement_and_writes_until_close() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("payload");
    std::fs::write(&path, b"fixture").unwrap();
    let dir = parent(temp.path());
    let pinned = child(&dir, OsStr::new("payload"), false, false, null_mut()).unwrap();
    let id = identity(&pinned).unwrap();
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    assert!(std::fs::rename(&path, temp.path().join("replacement")).is_err());
    assert_eq!(identity(&pinned).unwrap(), id);
    drop(pinned);
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_ok());
}
#[test]
fn hard_link_refused_without_mutating_either_name() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    std::fs::write(&first, b"sentinel").unwrap();
    std::fs::hard_link(&first, &second).unwrap();
    assert!(child(
        &parent(temp.path()),
        OsStr::new("first"),
        true,
        false,
        null_mut()
    )
    .is_err());
    assert_eq!(std::fs::read(&second).unwrap(), b"sentinel");
}
#[test]
fn directory_enumeration_and_deletion_use_opened_object() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("sentinel"), b"keep").unwrap();
    std::fs::write(temp.path().join("delete-me"), b"delete").unwrap();
    let dir = parent(temp.path());
    let names = entries(&dir).unwrap();
    assert!(names.contains(&OsString::from("sentinel")));
    assert!(names.contains(&OsString::from("delete-me")));
    let target = child(&dir, OsStr::new("delete-me"), true, false, null_mut()).unwrap();
    delete_same(&target).unwrap();
    drop(target);
    assert!(!temp.path().join("delete-me").exists());
    assert_eq!(
        std::fs::read(temp.path().join("sentinel")).unwrap(),
        b"keep"
    );
}
#[test]
fn delayed_exit_file_hold_retries_relative_open_after_release() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("held-file");
    std::fs::write(&path, b"fixture").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .unwrap();
    // Proxy for a helper that announced STOPPED while its process/file
    // handle remains alive. Disposable files only; no SCM or Polaris IO.
    let first_error = child(
        &parent(temp.path()),
        OsStr::new("held-file"),
        true,
        false,
        null_mut(),
    )
    .unwrap_err();
    assert!(crate::service::temporary_file_error(&first_error));
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(held);
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut attempts = 1; // The held-object attempt above must fail before release.
    crate::service::retry_cleanup(
        || {
            attempts += 1;
            let dir = parent(temp.path());
            let target = child(&dir, OsStr::new("held-file"), true, false, null_mut())?;
            delete_same(&target)
        },
        || bounded_pause(deadline),
    )
    .unwrap();
    release.join().unwrap();
    assert!(attempts > 1);
    assert!(!path.exists());
}
#[test]
fn persistent_file_hold_times_out_without_deleting_object() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("held-file");
    std::fs::write(&path, b"fixture").unwrap();
    let held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(150);
    let error = crate::service::retry_cleanup(
        || {
            let dir = parent(temp.path());
            let target = child(&dir, OsStr::new("held-file"), true, false, null_mut())?;
            delete_same(&target)
        },
        || bounded_pause(deadline),
    )
    .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
    drop(held);
    assert_eq!(std::fs::read(path).unwrap(), b"fixture");
}
struct ImageSection {
    _mapping: OwnedHandle,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}
impl ImageSection {
    fn open(path: &Path) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)?;
        let raw = unsafe {
            CreateFileMappingW(
                handle(&file),
                null(),
                PAGE_READONLY | SEC_IMAGE,
                0,
                0,
                null(),
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mapping = OwnedHandle(raw);
        let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            return Err(io::Error::last_os_error());
        }
        // Close the original file: only a real SEC_IMAGE section/view holds
        // the copied PE. No ordinary sharing lock substitutes for this case.
        drop(file);
        Ok(Self {
            _mapping: mapping,
            view,
        })
    }
}
impl Drop for ImageSection {
    fn drop(&mut self) {
        assert_ne!(unsafe { UnmapViewOfFile(self.view) }, 0);
    }
}
fn image_fixture() -> (tempfile::TempDir, PathBuf, String) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("image.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
    let (sid, _) = token_info().unwrap();
    (temp, path, sid)
}
fn delete_image_fixture(path: &Path, sid: &str) -> io::Result<()> {
    // Explicit fixture-only caller SID; production support trees always
    // require SYSTEM/Admin ownership. Reopen/revalidate the ancestor chain
    // and this single-object plan on every attempt, just as production does.
    let chain = Chain::open(path.parent().unwrap(), Some(sid))?;
    let target = child(
        chain.last(),
        OsStr::new("image.exe"),
        true,
        false,
        null_mut(),
    )?;
    let id = identity(&target)?;
    security(&target, Some(sid), false)?;
    if identity(&target)? != id {
        return Err(error("fixture identity changed"));
    }
    delete_same(&target)
}
#[test]
fn sec_image_delayed_release_retries_native_status_after_full_revalidation() {
    let (temp, path, sid) = image_fixture();
    std::thread::scope(|scope| {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mapped_path = &path;
        let worker = scope.spawn(move || {
            let mapping = ImageSection::open(mapped_path).unwrap();
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            std::thread::sleep(Duration::from_millis(150));
            drop(mapping);
        });
        ready_rx.recv().unwrap();
        let first = delete_image_fixture(&path, &sid).unwrap_err();
        assert_eq!(
            first
                .get_ref()
                .unwrap()
                .downcast_ref::<crate::service::DeleteStatus>()
                .unwrap()
                .status,
            STATUS_CANNOT_DELETE
        );
        assert!(crate::service::temporary_file_error(&first));
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        crate::service::retry_cleanup(
            || delete_image_fixture(&path, &sid),
            || bounded_pause(deadline),
        )
        .unwrap();
        worker.join().unwrap();
    });
    assert!(!path.exists());
    drop(temp);
}
#[test]
fn sec_image_persistent_mapping_times_out_without_unlinking_file() {
    let (_temp, path, sid) = image_fixture();
    let mapping = ImageSection::open(&path).unwrap();
    let deadline = Instant::now() + Duration::from_millis(150);
    let e = crate::service::retry_cleanup(
        || delete_image_fixture(&path, &sid),
        || bounded_pause(deadline),
    )
    .unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    assert!(path.exists());
    drop(mapping);
}
#[test]
fn native_delete_without_delete_right_is_access_denied_and_never_retried() {
    let (_temp, path, _sid) = image_fixture();
    let mut attempts = 0;
    let e = crate::service::retry_cleanup(
        || {
            attempts += 1;
            // Actual kernel access-check rejection, not a synthetic error 5.
            let file = std::fs::File::open(&path)?;
            delete_same(&file)
        },
        || panic!("real permission denial must not wait"),
    )
    .unwrap_err();
    let status = e
        .get_ref()
        .unwrap()
        .downcast_ref::<crate::service::DeleteStatus>()
        .unwrap();
    assert_eq!(status.status, STATUS_ACCESS_DENIED);
    assert_eq!(attempts, 1);
    assert!(path.exists());
}
#[test]
fn junction_is_refused_and_external_sentinel_is_intact() {
    // A directory junction needs no developer mode or elevation. Build
    // its reparse buffer only in disposable fixture directories.
    let temp = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("sentinel"), b"keep").unwrap();
    let junction = temp.path().join("junction");
    std::fs::create_dir(&junction).unwrap();
    let mount = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&junction)
        .unwrap();
    let substitute = format!(r"\??\{}", external.path().display())
        .encode_utf16()
        .collect::<Vec<_>>();
    let print = external
        .path()
        .as_os_str()
        .encode_wide()
        .collect::<Vec<_>>();
    let path_bytes = (substitute.len() + print.len() + 2) * 2;
    let mut data = Vec::new();
    data.extend_from_slice(&0xa0000003u32.to_le_bytes());
    data.extend_from_slice(&((8 + path_bytes) as u16).to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    for word in [
        0,
        (substitute.len() * 2) as u16,
        ((substitute.len() + 1) * 2) as u16,
        (print.len() * 2) as u16,
    ] {
        data.extend_from_slice(&word.to_le_bytes());
    }
    for word in substitute
        .into_iter()
        .chain(Some(0))
        .chain(print)
        .chain(Some(0))
    {
        data.extend_from_slice(&word.to_le_bytes());
    }
    let mut returned = 0;
    bool_ok(unsafe {
        DeviceIoControl(
            handle(&mount),
            0x000900a4,
            data.as_ptr().cast(),
            data.len() as u32,
            null_mut(),
            0,
            &mut returned,
            null_mut(),
        )
    })
    .unwrap();
    drop(mount);
    assert!(child(
        &parent(temp.path()),
        OsStr::new("junction"),
        true,
        false,
        null_mut()
    )
    .is_err());
    assert_eq!(
        std::fs::read(external.path().join("sentinel")).unwrap(),
        b"keep"
    );
    std::fs::remove_dir(junction).unwrap();
}
