use super::*;
use std::sync::mpsc;
use std::time::Duration;

struct ControlledReader {
    chunks: mpsc::Receiver<Vec<u8>>,
    next_read: mpsc::Sender<()>,
}

impl Read for ControlledReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.next_read.send(()).unwrap();
        let bytes = self.chunks.recv().unwrap();
        assert!(bytes.len() <= buffer.len());
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}

#[test]
fn exact_preopened_writer_revoke_leaves_open_pipe_draining_without_late_writes() {
    let _serial = PREOPENED_TEST_LOCK.lock().unwrap();
    let dir = temp_dir("custody-revoke");
    std::fs::create_dir_all(&dir).unwrap();
    let current = dir.join("core.log");
    let rotated = dir.join("core.log.1");
    std::fs::write(&current, b"prior").unwrap();
    std::fs::write(&rotated, b"").unwrap();
    let (chunks_tx, chunks_rx) = mpsc::channel();
    let (reads_tx, reads_rx) = mpsc::channel();
    let custody = spawn_pipe_loggers_with_preopened_files_custodied(
        Some(ControlledReader {
            chunks: chunks_rx,
            next_read: reads_tx,
        }),
        None::<std::io::Empty>,
        PreopenedLogFiles::new(open_read_write(&current), open_read_write(&rotated)),
        64,
    );
    assert!(
        std::fs::read(&current).unwrap().is_empty(),
        "Fresh initialized before returning custody"
    );
    reads_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    chunks_tx.send(b"old-live".to_vec()).unwrap();
    reads_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    custody.revoke().unwrap(); // Reader is already waiting on an unclosed pipe; no EOF join.
    assert_eq!(std::fs::read(&current).unwrap(), b"old-live");

    let successor = spawn_pipe_loggers_with_preopened_files_custodied(
        None::<std::io::Empty>,
        None::<std::io::Empty>,
        PreopenedLogFiles::new(open_read_write(&current), open_read_write(&rotated)),
        64,
    );
    successor
        .writer
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .write_chunk(b"new-live")
        .unwrap();
    chunks_tx.send(b"late-old".to_vec()).unwrap();
    reads_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(std::fs::read(&current).unwrap(), b"new-live");
    assert_eq!(std::fs::read(&rotated).unwrap(), b"old-live");
    custody.revoke().unwrap(); // Old repeated revoke cannot revoke the successor.
    assert!(successor.writer.lock().unwrap().is_some());
    successor.revoke().unwrap();
    chunks_tx.send(Vec::new()).unwrap();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn writer_revoke_waits_for_in_progress_write_and_poison_stays_unknown() {
    let dir = temp_dir("custody-inflight-write");
    let path = dir.join("core.log");
    let custody = PipeLogCustody {
        writer: Arc::new(Mutex::new(Some(PipeLogWriter::Path(
            RotatingFile::open(&path, 64, OpenMode::Append).unwrap(),
        )))),
    };
    let (held_tx, held_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::scope(|scope| {
        let writer = custody.writer.clone();
        scope.spawn(move || {
            let mut guard = writer.lock().unwrap();
            held_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            guard
                .as_mut()
                .unwrap()
                .write_chunk(b"delayed-inflight")
                .unwrap();
        });
        held_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        scope.spawn(|| {
            done_tx.send(custody.revoke()).unwrap();
        });
        assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"delayed-inflight");
    });
    let writer = custody.writer.clone();
    assert!(std::thread::spawn(move || {
        let _guard = writer.lock().unwrap();
        panic!("controlled writer poison");
    })
    .join()
    .is_err());
    assert!(custody.revoke().is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn no_writer_custody_revokes_without_waiting_for_pipe_eof() {
    let (chunks_tx, chunks_rx) = mpsc::channel();
    let (reads_tx, reads_rx) = mpsc::channel();
    let custody = spawn_pipe_drainers_custodied(
        Some(ControlledReader {
            chunks: chunks_rx,
            next_read: reads_tx,
        }),
        None::<std::io::Empty>,
    );
    reads_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    custody.revoke().unwrap();
    chunks_tx.send(b"still-drained".to_vec()).unwrap();
    reads_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    chunks_tx.send(Vec::new()).unwrap();
}
