use super::*;
mod probe;
use self::probe::{spawn_no_network_probe, Fault, FinishReason, ProbeError};
use std::os::fd::AsRawFd;
use std::time::Duration;

fn fixture_inputs(config: &[u8]) -> LinuxProtectedInputs {
    let executable = read_linux_binary_source(&std::env::current_exe().unwrap()).unwrap();
    prepare_linux_protected_inputs(
        executable,
        config.to_vec(),
        &[],
        LinuxInputProfile::NoNetworkUnitFixtureV1,
        vec![MutableObligation::CacheWriterLeaseAndSelectorReadback],
    )
    .unwrap()
}

/// Fixed, no-network Rust fixture, invoked only by private spawn_no_network_probe.
/// Modes come from sealed stdin; no cache, state, DNS, sockets or descendants.
#[test]
#[ignore = "private child fixture, executed by exact_spawn tests"]
fn no_network_fixture() {
    use std::io::Write;
    let mut config = Vec::new();
    std::io::stdin()
        .take(MAX_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut config)
        .unwrap();
    assert!(!config.is_empty() && config.len() <= MAX_CONFIG_BYTES);
    let executable = std::fs::read("/proc/self/exe").unwrap();
    println!(
        "EXEC={} CONFIG={} CWD={} ENV={}",
        hex_digest(&digest(&executable)),
        hex_digest(&digest(&config)),
        std::env::current_dir().unwrap().display(),
        std::env::vars_os().count()
    );
    if config[0] == b's' {
        std::thread::sleep(Duration::from_secs(30));
    }
    if config[0] == b'f' {
        let out = std::thread::spawn(|| {
            let mut out = std::io::stdout().lock();
            for _ in 0..1024 {
                out.write_all(&[b'o'; 1024]).unwrap();
            }
            out.flush().unwrap();
        });
        let mut err = std::io::stderr().lock();
        for _ in 0..1024 {
            err.write_all(&[b'e'; 1024]).unwrap();
        }
        err.flush().unwrap();
        out.join().unwrap();
    }
}

#[test]
fn protected_images_are_sealed_and_profile_specific() {
    let mut inputs = fixture_inputs(b"payload");
    assert_eq!(inputs.config_digest(), &digest(b"payload"));
    assert_eq!(
        inputs.loader_trust(),
        LoaderTrust::HostSystemLoaderAndLibraries
    );
    assert!(inputs.binary.as_raw_fd() >= 3 && inputs.config.as_raw_fd() >= 3);
    assert!(inputs.binary.write_all(b"changed").is_err());
    assert!(inputs.config.write_all(b"changed").is_err());
    assert!(inputs.config.set_len(0).is_err());
    assert_eq!(
        inputs.validate_protection(LinuxInputProfile::B609PlainTcpCheckV1),
        Err(PrepareError::Unsupported)
    );
    for file in [&inputs.binary, &inputs.config] {
        let flags = fcntl(file, FcntlArg::F_GETFD).unwrap();
        assert_ne!(flags & nix::libc::FD_CLOEXEC, 0);
    }
}

#[test]
fn reject_scripts_rules_empty_and_oversized_inputs() {
    let binary = read_linux_binary_source(&std::env::current_exe().unwrap()).unwrap();
    assert!(matches!(
        prepare_linux_protected_inputs(
            b"#!/bin/sh\necho bad".to_vec(),
            b"{}".to_vec(),
            &[],
            LinuxInputProfile::NoNetworkUnitFixtureV1,
            vec![]
        ),
        Err(PrepareError::InvalidExecutable)
    ));
    assert!(matches!(
        prepare_linux_protected_inputs(
            binary.clone(),
            vec![],
            &[],
            LinuxInputProfile::NoNetworkUnitFixtureV1,
            vec![]
        ),
        Err(PrepareError::ResourceBudget)
    ));
    assert!(matches!(
        prepare_linux_protected_inputs(
            binary.clone(),
            vec![0; MAX_CONFIG_BYTES + 1],
            &[],
            LinuxInputProfile::NoNetworkUnitFixtureV1,
            vec![]
        ),
        Err(PrepareError::ResourceBudget)
    ));
    assert!(matches!(
        prepare_linux_protected_inputs(
            binary.clone(),
            b"{}".to_vec(),
            &[("a".into(), vec![1])],
            LinuxInputProfile::NoNetworkUnitFixtureV1,
            vec![]
        ),
        Err(PrepareError::Unsupported)
    ));
    assert!(matches!(
        prepare_linux_protected_inputs(
            binary,
            b"{}".to_vec(),
            &[],
            LinuxInputProfile::B609PlainTcpCheckV1,
            vec![]
        ),
        Err(PrepareError::BinaryMismatch)
    ));
}

#[tokio::test]
async fn source_symlink_rejected_and_replacement_cannot_change_protected_bytes() {
    use std::os::unix::fs::DirBuilderExt;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("polaris-s4-image-{}-{unique}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let source = root.join("source");
    let link = root.join("link");
    let original = read_linux_binary_source(&std::env::current_exe().unwrap()).unwrap();
    std::fs::write(&source, &original).unwrap();
    std::os::unix::fs::symlink(&source, &link).unwrap();
    assert!(read_linux_binary_source(&link).is_err());
    let mut inputs = prepare_linux_protected_inputs(
        read_linux_binary_source(&source).unwrap(),
        b"original".to_vec(),
        &[],
        LinuxInputProfile::NoNetworkUnitFixtureV1,
        vec![],
    )
    .unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::write(&source, b"replacement").unwrap();
    let mut held = Vec::new();
    inputs.binary.read_to_end(&mut held).unwrap();
    assert_eq!(held, original);
    let original_config_digest = *inputs.config_digest();
    let (mut custody, observed) =
        spawn_no_network_probe(inputs, Duration::from_secs(8), None, Fault::None).unwrap();
    let result = custody.result().await.unwrap();
    assert!(result.reaped && result.exit_code == Some(0));
    assert!(String::from_utf8_lossy(&result.stdout.prefix)
        .contains(&format!("CONFIG={}", hex_digest(&original_config_digest))));
    assert_eq!(result.binary_digest, digest(&original));
    assert!(custody.settle().await.unwrap().reaped);
    assert!(observed.await.unwrap().unwrap().reaped);
    std::fs::remove_file(source).unwrap();
    std::fs::remove_file(link).unwrap();
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn source_stat_fifo_replacement_is_bounded_without_writer() {
    use std::os::unix::fs::DirBuilderExt;
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("polaris-s4-fifo-{}-{unique}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&root)
        .unwrap();
    let source = root.join("source");
    std::fs::write(&source, b"original regular bytes").unwrap();
    let worker_path = source.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = read_linux_binary_source_with_stat_hook(&worker_path, || {
            std::fs::remove_file(&worker_path).unwrap();
            nix::unistd::mkfifo(&worker_path, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();
            ready_tx.send(()).unwrap();
        });
        result_tx.send(result).unwrap();
    });
    ready_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let result = result_rx.recv_timeout(Duration::from_secs(1));
    // A regressed blocking open must fail the deadline without hanging the
    // test process. Only that failure branch introduces a FIFO writer.
    let _unblock_on_failure = result.is_err().then(|| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nix::libc::O_NONBLOCK | nix::libc::O_NOFOLLOW)
            .open(&source)
            .unwrap()
    });
    worker.join().unwrap();
    std::fs::remove_file(&source).unwrap();
    std::fs::remove_dir(root).unwrap();
    assert_eq!(result.unwrap(), Err(PrepareError::Unsupported));
}

#[tokio::test]
async fn actual_fixture_reads_protected_original_bytes_and_drains_both_large_pipes() {
    let inputs = fixture_inputs(b"f-original-config");
    let config_digest = *inputs.config_digest();
    let binary_digest = *inputs.binary_digest();
    let (mut custody, observed) =
        spawn_no_network_probe(inputs, Duration::from_secs(8), None, Fault::None).unwrap();
    let result = custody.result().await.unwrap();
    assert_eq!(result.reason, FinishReason::Exited);
    assert_eq!(result.exit_code, Some(0));
    assert!(result.reaped && !result.cleanup_uncertain);
    assert_eq!(result.config_digest, config_digest);
    assert_eq!(result.binary_digest, binary_digest);
    assert!(result.stdout.bytes > 1024 * 1024 && result.stderr.bytes == 1024 * 1024);
    assert!(result.stdout.prefix.len() <= 512 && result.stderr.prefix.len() <= 512);
    let prefix = String::from_utf8_lossy(&result.stdout.prefix);
    assert!(prefix.contains(&format!("CONFIG={}", hex_digest(&config_digest))));
    assert!(prefix.contains(&format!("EXEC={}", hex_digest(&binary_digest))));
    assert!(prefix.contains("CWD=/ ENV=0"));
    assert_eq!(observed.await.unwrap().unwrap().pid, result.pid);
    assert!(custody.settle().await.unwrap().reaped);
}

#[tokio::test]
async fn cancellation_before_spawn_and_after_transfer_keeps_real_reap_owner() {
    let (custody, observed) = spawn_no_network_probe(
        fixture_inputs(b"s"),
        Duration::from_secs(8),
        None,
        Fault::None,
    )
    .unwrap();
    drop(custody);
    assert!(matches!(
        observed.await.unwrap(),
        Err(ProbeError::CancelledBeforeSpawn)
    ));
    let (spawn_tx, spawn_rx) = tokio::sync::oneshot::channel();
    let (custody, observed) = spawn_no_network_probe(
        fixture_inputs(b"s"),
        Duration::from_secs(8),
        Some(spawn_tx),
        Fault::None,
    )
    .unwrap();
    let pid = spawn_rx.await.unwrap();
    let mut transferred = custody;
    transferred.cancel();
    let result = transferred.result().await.unwrap();
    assert_eq!(result.pid, pid);
    assert_eq!(result.reason, FinishReason::Cancelled);
    assert!(result.reaped);
    assert!(transferred.settle().await.unwrap().reaped);
    assert!(observed.await.unwrap().unwrap().reaped);
}

#[tokio::test]
async fn cancelled_wait_future_and_timeout_are_actually_reaped() {
    let (spawn_tx, spawn_rx) = tokio::sync::oneshot::channel();
    let (mut custody, observed) = spawn_no_network_probe(
        fixture_inputs(b"s"),
        Duration::from_secs(8),
        Some(spawn_tx),
        Fault::None,
    )
    .unwrap();
    let pid = spawn_rx.await.unwrap();
    let waiter = tokio::spawn(async move { custody.result().await });
    waiter.abort();
    let _ = waiter.await;
    let result = tokio::time::timeout(Duration::from_secs(5), observed)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(result.pid, pid);
    assert!(result.reaped);
    assert_eq!(result.reason, FinishReason::Cancelled);
    let (mut custody, observed) =
        spawn_no_network_probe(fixture_inputs(b"s"), Duration::ZERO, None, Fault::None).unwrap();
    let result = custody.result().await.unwrap();
    assert!(result.reaped);
    assert_eq!(result.reason, FinishReason::TimedOut);
    assert!(custody.settle().await.unwrap().reaped);
    assert!(observed.await.unwrap().unwrap().reaped);
}

#[tokio::test]
async fn cancelled_borrowed_result_future_reaps_while_custody_is_retained() {
    for polled in [false, true] {
        let (spawn_tx, spawn_rx) = tokio::sync::oneshot::channel();
        let (mut custody, observed) = spawn_no_network_probe(
            fixture_inputs(b"s"),
            Duration::from_secs(30),
            Some(spawn_tx),
            Fault::None,
        )
        .unwrap();
        let pid = spawn_rx.await.unwrap();
        if polled {
            assert!(
                tokio::time::timeout(Duration::from_millis(25), custody.result())
                    .await
                    .is_err()
            );
        } else {
            drop(custody.result());
        }
        // This deadline is much shorter than the probe's own timeout. Keep the
        // custody alive: only cancellation of its borrowed wait can trigger kill.
        let observed = tokio::time::timeout(Duration::from_secs(3), observed)
            .await
            .expect("borrowed result cancellation must promptly request cleanup")
            .unwrap()
            .unwrap();
        assert_eq!(observed.pid, pid);
        assert_eq!(observed.reason, FinishReason::Cancelled);
        assert!(observed.reaped && !observed.cleanup_uncertain);
        let result = custody.result().await.unwrap();
        assert_eq!(result.pid, pid);
        assert_eq!(result.reason, FinishReason::Cancelled);
        assert!(result.reaped && !result.cleanup_uncertain);
        let settled = custody.settle().await.unwrap();
        assert!(settled.reaped && !settled.cleanup_uncertain);
    }
}

#[tokio::test]
async fn failure_spawn_and_synthetic_wait_error_never_report_success() {
    let (mut custody, observed) = spawn_no_network_probe(
        fixture_inputs(b"s"),
        Duration::from_secs(8),
        None,
        Fault::SpawnFailure,
    )
    .unwrap();
    assert!(matches!(custody.result().await, Err(ProbeError::Spawn)));
    assert!(matches!(custody.settle().await, Err(ProbeError::Spawn)));
    assert!(matches!(observed.await.unwrap(), Err(ProbeError::Spawn)));
    let (mut custody, observed) = spawn_no_network_probe(
        fixture_inputs(b"s"),
        Duration::from_secs(8),
        None,
        Fault::WaitFailureOnce,
    )
    .unwrap();
    assert!(matches!(
        custody.result().await,
        Err(ProbeError::CleanupUncertain)
    ));
    let settled = custody.settle().await.unwrap();
    assert!(settled.reaped && settled.cleanup_uncertain);
    assert!(observed.await.unwrap().unwrap().cleanup_uncertain);
}

/// Separate, ordinary executable: it never opens or closes protected images.
#[test]
#[ignore = "private child fixture for an unrelated executable, executed by exact_spawn tests"]
fn unrelated_no_network_fixture() {
    use std::io::Write;
    std::io::stdout()
        .write_all(b"\nPOLARIS_UNRELATED_USERLAND_READY\n")
        .unwrap();
    std::io::stdout().flush().unwrap();
    loop {
        std::thread::park();
    }
}

struct UnrelatedObservation {
    pid: u32,
    executable: std::path::PathBuf,
    targets: Vec<(std::path::PathBuf, std::path::PathBuf)>,
}

async fn observe_unrelated_child(
    stdin: std::process::Stdio,
) -> Result<UnrelatedObservation, String> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    let mut child =
        tokio::process::Command::new(std::env::current_exe().map_err(|error| error.to_string())?)
            .args([
                "--exact",
                "exact_spawn::tests::unrelated_no_network_fixture",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .stdin(stdin)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| error.to_string())?;
    let observation = async {
        let pid = child.id().ok_or("missing unrelated child pid")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("missing unrelated child stdout")?;
        let ready = async {
            let mut reader = BufReader::new(stdout.take(4096));
            let mut line = Vec::new();
            loop {
                line.clear();
                if reader.read_until(b'\n', &mut line).await? == 0 {
                    return Err(std::io::Error::other("missing userland READY"));
                }
                if line == b"POLARIS_UNRELATED_USERLAND_READY\n" {
                    return Ok(());
                }
            }
        };
        // spawn() can return during exec's CLOEXEC sweep on some hosts. The
        // child's own marker proves this scan is after entering userland.
        tokio::time::timeout(Duration::from_secs(2), ready)
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())?;
        let executable =
            std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|error| error.to_string())?;
        let mut targets = Vec::new();
        for entry in
            std::fs::read_dir(format!("/proc/{pid}/fd")).map_err(|error| error.to_string())?
        {
            let path = entry.map_err(|error| error.to_string())?.path();
            let target = std::fs::read_link(&path).map_err(|error| error.to_string())?;
            targets.push((path, target));
        }
        Ok(UnrelatedObservation {
            pid,
            executable,
            targets,
        })
    }
    .await;
    // Finish physical cleanup before any observation or assertion can fail.
    let killed = child.start_kill();
    let waited = child.wait().await;
    killed.map_err(|error| format!("unrelated child kill: {error}; wait={waited:?}"))?;
    waited.map_err(|error| format!("unrelated child wait: {error}"))?;
    observation
}

#[tokio::test]
async fn unrelated_child_does_not_inherit_protected_images() {
    let _images = fixture_inputs(b"original");
    let observation = observe_unrelated_child(std::process::Stdio::null())
        .await
        .unwrap();
    for (fd, target) in &observation.targets {
        assert!(
            !target.to_string_lossy().contains("polaris-s4-"),
            "pid={} exe={} fd={} target={}",
            observation.pid,
            observation.executable.display(),
            fd.display(),
            target.display()
        );
    }
}

#[tokio::test]
async fn deliberately_passed_protected_stdin_is_detected_after_userland_ready() {
    let images = fixture_inputs(b"deliberately inherited");
    let stdin = File::from(rustix::io::fcntl_dupfd_cloexec(&images.config, 3).unwrap());
    let observation = observe_unrelated_child(std::process::Stdio::from(stdin))
        .await
        .unwrap();
    // Command deliberately maps this CLOEXEC copy to inheritable fd0 only in
    // this child. No parent descriptor becomes inheritable during parallel tests.
    assert!(observation.targets.iter().any(|(fd, target)| {
        fd.file_name().is_some_and(|name| name == "0")
            && target.to_string_lossy().contains("polaris-s4-config")
    }));
}
