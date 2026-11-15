use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

mod custody;

static PREOPENED_TEST_LOCK: Mutex<()> = Mutex::new(());

fn temp_dir(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "polaris-log-budget-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn runtime_rotation_keeps_two_hard_bounded_generations() {
    let dir = temp_dir("rotate");
    let path = dir.join("core.log");
    let mut file = RotatingFile::open(&path, 8, OpenMode::Append).unwrap();
    file.write_chunk(b"12345678").unwrap();
    file.write_chunk(b"abcdefgh").unwrap();
    file.write_chunk(b"XYZ").unwrap();
    drop(file);

    assert_eq!(std::fs::read(&path).unwrap(), b"XYZ");
    assert_eq!(std::fs::read(rotated_path(&path)).unwrap(), b"abcdefgh");
    assert!(std::fs::metadata(&path).unwrap().len() <= 8);
    assert!(std::fs::metadata(rotated_path(&path)).unwrap().len() <= 8);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn oversized_record_keeps_tail_without_breaking_budget() {
    let dir = temp_dir("oversized");
    let path = dir.join("core.log");
    let mut file = RotatingFile::open(&path, 5, OpenMode::Append).unwrap();
    file.write_chunk(b"0123456789").unwrap();
    drop(file);
    assert_eq!(std::fs::read(&path).unwrap(), b"56789");
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn fresh_mode_separates_helper_sessions() {
    let dir = temp_dir("fresh");
    let path = dir.join("startup.log");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(&path, b"old\n").unwrap();
    let mut file = RotatingFile::open(&path, 32, OpenMode::Fresh).unwrap();
    file.write_chunk(b"new\n").unwrap();
    drop(file);
    assert_eq!(std::fs::read(rotated_path(&path)).unwrap(), b"old\n");
    assert_eq!(std::fs::read(&path).unwrap(), b"new\n");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn read_tail_spans_old_and_current_in_order() {
    let dir = temp_dir("read");
    let path = dir.join("core.log");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(rotated_path(&path), b"old-1234").unwrap();
    std::fs::write(&path, b"new").unwrap();
    assert_eq!(read_rotated_tail(&path, 8).unwrap(), b"-1234new");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn pipe_logger_open_hook_receives_the_actual_writer_file() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let dir = temp_dir("open-hook");
    let path = dir.join("startup.log");
    let called = AtomicBool::new(false);
    spawn_pipe_loggers_with_file::<std::io::Empty, std::io::Empty, _>(
        None,
        None,
        &path,
        32,
        |opened| {
            let file = opened.expect("writer 应已成功打开");
            assert!(file.metadata().is_ok(), "回调拿到的 fd 必须仍有效");
            called.store(true, Ordering::SeqCst);
        },
    );
    assert!(called.load(Ordering::SeqCst));
    assert!(path.is_file());
    let _ = std::fs::remove_dir_all(dir);
}

fn open_read_write(path: &Path) -> File {
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap()
}

#[test]
fn preopened_rotation_never_reopens_replaced_paths() {
    let _serial = PREOPENED_TEST_LOCK.lock().unwrap();
    let dir = temp_dir("preopened-path-replacement");
    std::fs::create_dir_all(&dir).unwrap();
    let current_path = dir.join("startup.log");
    let rotated_path = dir.join("startup.log.1");
    std::fs::write(&current_path, b"old-session").unwrap();
    std::fs::write(&rotated_path, b"older-session").unwrap();

    let files = PreopenedLogFiles::new(
        open_read_write(&current_path),
        open_read_write(&rotated_path),
    );
    let mut writer = PreopenedRotatingFile::open(files, 8, OpenMode::Fresh).unwrap();

    let pinned_current = dir.join("pinned-current");
    let pinned_rotated = dir.join("pinned-rotated");
    std::fs::rename(&current_path, &pinned_current).unwrap();
    std::fs::rename(&rotated_path, &pinned_rotated).unwrap();
    std::fs::write(&current_path, b"replacement-current").unwrap();
    std::fs::write(&rotated_path, b"replacement-rotated").unwrap();

    writer.write_chunk(b"12345678").unwrap();
    writer.write_chunk(b"new").unwrap();
    drop(writer);

    assert_eq!(
        std::fs::read(&current_path).unwrap(),
        b"replacement-current"
    );
    assert_eq!(
        std::fs::read(&rotated_path).unwrap(),
        b"replacement-rotated"
    );
    assert_eq!(std::fs::read(&pinned_current).unwrap(), b"new");
    assert_eq!(std::fs::read(&pinned_rotated).unwrap(), b"12345678");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn preopened_fresh_mode_trims_both_generations_before_writing() {
    let _serial = PREOPENED_TEST_LOCK.lock().unwrap();
    let dir = temp_dir("preopened-fresh");
    std::fs::create_dir_all(&dir).unwrap();
    let current_path = dir.join("startup.log");
    let rotated_path = dir.join("startup.log.1");
    std::fs::write(&current_path, b"0123456789").unwrap();
    std::fs::write(&rotated_path, b"abcdefghij").unwrap();

    let files = PreopenedLogFiles::new(
        open_read_write(&current_path),
        open_read_write(&rotated_path),
    );
    let mut writer = PreopenedRotatingFile::open(files, 5, OpenMode::Fresh).unwrap();
    writer.write_chunk(b"new").unwrap();
    drop(writer);

    assert_eq!(std::fs::read(&current_path).unwrap(), b"new");
    assert_eq!(std::fs::read(&rotated_path).unwrap(), b"56789");
    assert!(std::fs::metadata(&current_path).unwrap().len() <= 5);
    assert!(std::fs::metadata(&rotated_path).unwrap().len() <= 5);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn newer_preopened_session_discards_late_bytes_from_old_pipe() {
    let _serial = PREOPENED_TEST_LOCK.lock().unwrap();
    let dir = temp_dir("preopened-session-generation");
    std::fs::create_dir_all(&dir).unwrap();
    let current_path = dir.join("startup.log");
    let rotated_path = dir.join("startup.log.1");
    std::fs::write(&current_path, b"first").unwrap();
    std::fs::write(&rotated_path, b"").unwrap();

    let mut old = PreopenedRotatingFile::open(
        PreopenedLogFiles::new(
            open_read_write(&current_path),
            open_read_write(&rotated_path),
        ),
        16,
        OpenMode::Fresh,
    )
    .unwrap();
    old.write_chunk(b"old-live").unwrap();

    let mut new = PreopenedRotatingFile::open(
        PreopenedLogFiles::new(
            open_read_write(&current_path),
            open_read_write(&rotated_path),
        ),
        16,
        OpenMode::Fresh,
    )
    .unwrap();
    old.write_chunk(b"late-old").unwrap();
    new.write_chunk(b"new-live").unwrap();
    drop((old, new));

    assert_eq!(std::fs::read(&current_path).unwrap(), b"new-live");
    assert_eq!(std::fs::read(&rotated_path).unwrap(), b"old-live");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn failed_new_preopened_session_still_retires_old_writer() {
    let _serial = PREOPENED_TEST_LOCK.lock().unwrap();
    let dir = temp_dir("preopened-failed-session-generation");
    std::fs::create_dir_all(&dir).unwrap();
    let current_path = dir.join("startup.log");
    let rotated_path = dir.join("startup.log.1");
    std::fs::write(&current_path, b"first").unwrap();
    std::fs::write(&rotated_path, b"").unwrap();

    let mut old = PreopenedRotatingFile::open(
        PreopenedLogFiles::new(
            open_read_write(&current_path),
            open_read_write(&rotated_path),
        ),
        32,
        OpenMode::Fresh,
    )
    .unwrap();
    old.write_chunk(b"old-live").unwrap();

    let failed = PreopenedRotatingFile::open(
        PreopenedLogFiles::new(
            File::open(&current_path).unwrap(),
            open_read_write(&rotated_path),
        ),
        32,
        OpenMode::Fresh,
    );
    assert!(failed.is_err());
    old.write_chunk(b"late-old").unwrap();
    drop(old);

    assert_eq!(std::fs::read(&current_path).unwrap(), b"old-live");
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn open_refuses_symlink_without_truncating_its_target() {
    let dir = temp_dir("symlink");
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("protected");
    let path = dir.join("core.log");
    let original = b"root-owned-content-must-stay-intact";
    std::fs::write(&target, original).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();

    let error = RotatingFile::open(&path, 8, OpenMode::Append).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(std::fs::read(&target).unwrap(), original);
    assert!(std::fs::symlink_metadata(&path)
        .unwrap()
        .file_type()
        .is_symlink());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn open_tightens_current_and_rotated_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_dir("permissions");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("core.log");
    let rotated = rotated_path(&path);
    std::fs::write(&path, b"current-secret").unwrap();
    std::fs::write(&rotated, b"old-secret").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::set_permissions(&rotated, std::fs::Permissions::from_mode(0o644)).unwrap();

    let file = RotatingFile::open(&path, 64, OpenMode::Append).unwrap();
    drop(file);
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&rotated).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(unix)]
fn assert_fifo_operation_rejected(
    fifo: &Path,
    operation: impl FnOnce() -> std::io::Result<()> + Send + 'static,
) {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
    use std::time::Duration;

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || done_tx.send(operation()).unwrap());
    let result = match done_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(result) => result,
        Err(error) => {
            // Unblock a regressed blocking open so the failing test leaves no stuck worker.
            let _peer = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(fifo);
            worker.join().unwrap();
            panic!("log operation blocked on FIFO: {error}");
        }
    };
    worker.join().unwrap();
    assert!(result.is_err(), "non-regular log object was accepted");
    let metadata = std::fs::symlink_metadata(fifo).unwrap();
    assert!(metadata.file_type().is_fifo());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o644);
}

#[cfg(unix)]
#[test]
fn path_opens_reject_fifo_without_peer_or_permission_changes() {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    for (label, operation) in [
        (
            "fresh",
            (|path: &Path| RotatingFile::open(path, 8, OpenMode::Fresh).map(drop))
                as fn(&Path) -> std::io::Result<()>,
        ),
        ("trim", |path| trim_file_to_tail(path, 8)),
        ("append-no-reader", |path| open_append(path).map(drop)),
        ("append-with-reader", |path| {
            let _reader = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(path)?;
            open_append(path).map(drop)
        }),
        ("read", |path| read_file_tail(path, 8).map(drop)),
    ] {
        let dir = temp_dir(label);
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("core.log");
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        std::fs::set_permissions(&fifo, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path = fifo.clone();
        assert_fifo_operation_rejected(&fifo, move || operation(&path));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn current_and_rotated_fifo_cannot_block_rotation_or_tail_reads() {
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    for replace_rotated in [false, true] {
        let dir = temp_dir("rotate-fifo");
        let path = dir.join("core.log");
        let mut writer = RotatingFile::open(&path, 4, OpenMode::Append).unwrap();
        writer.write_chunk(b"live").unwrap();
        let fifo = if replace_rotated {
            rotated_path(&path)
        } else {
            path.clone()
        };
        if fifo.exists() {
            std::fs::remove_file(&fifo).unwrap();
        }
        assert!(std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        std::fs::set_permissions(&fifo, std::fs::Permissions::from_mode(0o644)).unwrap();
        let fresh_path = path.clone();
        assert_fifo_operation_rejected(&fifo, move || {
            RotatingFile::open(&fresh_path, 4, OpenMode::Fresh).map(drop)
        });
        assert_fifo_operation_rejected(&fifo, move || writer.write_chunk(b"x"));

        let regular = if replace_rotated {
            path.clone()
        } else {
            rotated_path(&path)
        };
        std::fs::write(&regular, b"safe").unwrap();
        assert_eq!(read_rotated_tail(&path, 16).unwrap(), b"safe");
        assert!(std::fs::symlink_metadata(&fifo)
            .unwrap()
            .file_type()
            .is_fifo());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
