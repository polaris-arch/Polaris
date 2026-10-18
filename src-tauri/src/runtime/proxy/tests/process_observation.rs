//! OS ground truth for the controlled real-core tests. Observation failure must
//! fail the test, never masquerade as a dead process or an empty orphan census.

#[cfg(not(windows))]
fn process_ids(command: &str, args: &[&str]) -> Vec<u32> {
    let output = std::process::Command::new(command)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("{command} process observation failed: {error}"));
    parse_process_ids(output.status.code(), &output.stdout, &output.stderr)
}

#[cfg(not(windows))]
fn parse_process_ids(code: Option<i32>, stdout: &[u8], stderr: &[u8]) -> Vec<u32> {
    assert!(
        stderr.is_empty(),
        "process observation diagnostic: {}",
        String::from_utf8_lossy(stderr)
    );
    let stdout = std::str::from_utf8(stdout).expect("process observation must be UTF-8");
    if code == Some(1) {
        assert!(
            stdout.trim().is_empty(),
            "no-match exit must have no process rows"
        );
        return Vec::new();
    }
    assert_eq!(code, Some(0), "process observation command failed");
    let ids: Vec<u32> = stdout
        .lines()
        .map(|line| {
            let pid = line
                .trim()
                .parse::<u32>()
                .expect("process observation row must be a PID");
            assert_ne!(pid, 0, "process observation cannot return PID zero");
            pid
        })
        .collect();
    assert!(
        !ids.is_empty(),
        "successful process observation must return rows"
    );
    ids
}

#[cfg(not(windows))]
pub(super) fn ps_alive(pid: u32) -> bool {
    assert_ne!(pid, 0, "cannot observe PID zero");
    let ids = process_ids("ps", &["-p", &pid.to_string(), "-o", "pid="]);
    assert!(
        ids.iter().all(|observed| *observed == pid),
        "ps returned another PID"
    );
    !ids.is_empty()
}

#[cfg(not(windows))]
pub(super) fn singbox_proc_count() -> usize {
    process_ids("pgrep", &["-x", "sing-box"]).len()
}

// The proxy subtree forbids unsafe code. Keep all native calls behind the
// existing Windows process boundary, exposed only for controlled tests.
#[cfg(windows)]
use crate::runtime::windows_process::tests::controlled_test_observation as native;

#[cfg(windows)]
pub(super) fn ps_alive(pid: u32) -> bool {
    native::alive(pid)
}

#[cfg(windows)]
pub(super) fn singbox_proc_count() -> usize {
    native::count()
}

#[test]
fn current_process_is_observed_by_os() {
    assert!(ps_alive(std::process::id()));
    let _ = singbox_proc_count(); // An unavailable observer must panic, not report zero.
}

#[test]
#[cfg(windows)]
fn exited_owned_process_is_not_alive_even_while_its_handle_is_retained() {
    let mut child = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "exit 0"])
        .spawn()
        .expect("spawn owned native observation fixture");
    let pid = child.id();
    assert!(child.wait().expect("wait owned fixture").success());
    assert!(
        !ps_alive(pid),
        "exited process must be dead, including a retained process object"
    );
}

#[test]
#[cfg(not(windows))]
fn process_command_errors_and_malformed_output_fail_closed() {
    assert_eq!(parse_process_ids(Some(0), b" 123\n", b""), vec![123]);
    assert!(parse_process_ids(Some(1), b"", b"").is_empty());
    for (code, stdout, stderr) in [
        (None, b"".as_slice(), b"".as_slice()),
        (Some(2), b"", b""),
        (Some(0), b"", b""),
        (Some(0), b"pid 123", b""),
        (Some(0), b"0", b""),
        (Some(1), b"123", b""),
        (Some(1), b"", b"permission denied"),
        (Some(0), b"\xff", b""),
    ] {
        assert!(std::panic::catch_unwind(|| parse_process_ids(code, stdout, stderr)).is_err());
    }
}
