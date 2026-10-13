//! Bounded metadata child capture; no network, native core, or device probe.
use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Stream {
    bytes: Vec<u8>,
    observed: usize,
    exceeded: bool,
    done: bool,
    error: Option<String>,
}

fn pump(
    mut pipe: impl Read + Send + 'static,
    maximum: usize,
) -> (Arc<Mutex<Stream>>, JoinHandle<()>) {
    let state = Arc::new(Mutex::new(Stream::default()));
    let shared = state.clone();
    let worker = thread::spawn(move || {
        let mut block = [0u8; 8192];
        loop {
            let remaining = maximum - shared.lock().unwrap().bytes.len();
            let count = block.len().min(remaining.saturating_add(1));
            let read = pipe.read(&mut block[..count]);
            let mut value = shared.lock().unwrap();
            match read {
                Ok(0) => break,
                Ok(n) => {
                    value.observed += n;
                    value.bytes.extend_from_slice(&block[..n.min(remaining)]);
                    if n > remaining {
                        value.exceeded = true;
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    value.error = Some(error.to_string());
                    break;
                }
            }
        }
        shared.lock().unwrap().done = true;
    });
    (state, worker)
}

pub(super) struct Output {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub status: Option<ExitStatus>,
    pub failure: Option<String>,
    pub stdout_observed: usize,
    pub stderr_observed: usize,
    pub cleanup_complete: bool,
}

fn terminate_owned(child: &mut Child) {
    #[cfg(unix)]
    {
        // The leader has not been reaped on an abort, so its process-group ID
        // cannot be reused. Kill the isolated metadata group, including a child
        // that inherited a pipe; never signal either WG peer or the test group.
        if let Ok(mut signal) = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            let until = Instant::now() + Duration::from_millis(250);
            while Instant::now() < until {
                if !matches!(signal.try_wait(), Ok(None)) {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            if matches!(signal.try_wait(), Ok(None)) {
                let _ = signal.kill();
                let reap_until = Instant::now() + Duration::from_millis(100);
                while Instant::now() < reap_until {
                    if !matches!(signal.try_wait(), Ok(None)) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }
    let _ = child.kill();
}

pub(super) fn capture(
    command: &mut Command,
    timeout: Duration,
    stdout_max: usize,
    stderr_max: usize,
) -> io::Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = command.spawn()?;
    let (stdout, out_worker) = pump(child.stdout.take().unwrap(), stdout_max);
    let (stderr, err_worker) = pump(child.stderr.take().unwrap(), stderr_max);
    let mut status = None;
    let mut failure = None;
    loop {
        let out = stdout.lock().unwrap();
        let err = stderr.lock().unwrap();
        if out.exceeded || err.exceeded {
            failure = Some("metadata stream limit exceeded".into());
        } else if out.error.is_some() || err.error.is_some() {
            failure = Some("metadata pipe read failed".into());
        }
        let streams_done = out.done && err.done;
        drop(out);
        drop(err);
        if failure.is_some() {
            break;
        }
        // Do not reap an exited leader while a descendant still holds a pipe:
        // its reserved ID protects the group teardown on deadline/overflow.
        if streams_done {
            match child.try_wait() {
                Ok(Some(value)) => {
                    status = Some(value);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    failure = Some(error.to_string());
                    break;
                }
            }
        }
        if started.elapsed() >= timeout {
            failure = Some("metadata deadline exceeded".into());
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    if failure.is_some() {
        terminate_owned(&mut child);
    }
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        if status.is_none() {
            status = child.try_wait().ok().flatten();
        }
        if status.is_some() && out_worker.is_finished() && err_worker.is_finished() {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let cleanup_complete = status.is_some() && out_worker.is_finished() && err_worker.is_finished();
    if !cleanup_complete && failure.is_none() {
        failure = Some("metadata cleanup deadline exceeded".into());
    }
    // Never block on a join or wait after the teardown deadline. An unfinished
    // stream/child is explicitly unverified; the original failure was emitted
    // before this helper ran and the WG assertion still fails.
    if out_worker.is_finished() {
        let _ = out_worker.join();
    }
    if err_worker.is_finished() {
        let _ = err_worker.join();
    }
    let out = stdout.lock().unwrap();
    let err = stderr.lock().unwrap();
    Ok(Output {
        stdout: out.bytes.clone(),
        stderr: err.bytes.clone(),
        status,
        failure,
        stdout_observed: out.observed,
        stderr_observed: err.observed,
        cleanup_complete,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }
    #[test]
    fn exact_stream_caps_and_exit_are_retained() {
        let value = capture(
            &mut shell("printf 12345678; printf abc >&2"),
            Duration::from_secs(1),
            8,
            3,
        )
        .unwrap();
        assert_eq!(value.stdout, b"12345678");
        assert_eq!(value.stderr, b"abc");
        assert!(
            value.failure.is_none() && value.status.unwrap().success() && value.cleanup_complete
        );
    }
    #[test]
    fn stdout_or_stderr_flood_is_bounded_and_unverified() {
        for script in [
            "while :; do printf 123456789; done",
            "while :; do printf 123456789 >&2; done",
        ] {
            let started = Instant::now();
            let value = capture(&mut shell(script), Duration::from_secs(1), 8, 8).unwrap();
            assert!(value.failure.as_deref().unwrap().contains("limit"));
            assert!(value.stdout.len() <= 8 && value.stderr.len() <= 8);
            assert!(value.stdout_observed <= 9 && value.stderr_observed <= 9);
            assert!(value.cleanup_complete && started.elapsed() < Duration::from_secs(3));
        }
    }
    #[test]
    fn quiet_child_or_exited_leader_with_inherited_pipes_cannot_block_capture() {
        for script in ["sleep 30", "sleep 30 & exit 0"] {
            let started = Instant::now();
            let value = capture(&mut shell(script), Duration::from_millis(100), 64, 64).unwrap();
            assert!(value.failure.as_deref().unwrap().contains("deadline"));
            assert!(value.cleanup_complete && started.elapsed() < Duration::from_secs(3));
        }
    }
}
