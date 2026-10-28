//! Shared exact-start checks for the three privileged helper platforms.

use polaris_helper_proto::ExactReceipt;
use std::sync::OnceLock;

/// Identifies this daemon process, including a restart at the same PID/build.
#[must_use]
pub fn daemon_birth() -> &'static str {
    static BIRTH: OnceLock<String> = OnceLock::new();
    BIRTH.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// Compare every instance field. A PID alone, or an `Already` response, never passes.
#[must_use]
pub fn matches(expected: &ExactReceipt, current: &ExactReceipt) -> bool {
    expected.valid()
        && current.valid()
        && expected == current
        && expected.daemon_birth == daemon_birth()
}

#[cfg(target_os = "linux")]
#[must_use]
pub fn linux_instance_exited(receipt: &ExactReceipt) -> Option<bool> {
    if receipt.daemon_birth != daemon_birth() || !receipt.valid() {
        return None;
    }
    match linux_process_birth(receipt.pid) {
        Some(birth) => Some(birth != receipt.process_birth),
        None => match std::fs::metadata(format!("/proc/{}", receipt.pid)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(true),
            _ => None,
        },
    }
}

#[cfg(target_os = "macos")]
#[must_use]
pub fn mac_process_birth(pid: u32) -> Option<String> {
    crate::platform::macos::proc_start::exact_process_birth(pid)
        .map(|birth| hex::encode(birth.as_bytes()))
}

#[cfg(target_os = "macos")]
#[must_use]
pub fn mac_instance_exited(receipt: &ExactReceipt) -> Option<bool> {
    if receipt.daemon_birth != daemon_birth() || !receipt.valid() {
        return None;
    }
    match mac_process_birth(receipt.pid) {
        Some(birth) => Some(birth != receipt.process_birth),
        None if !crate::platform::macos::proc_start::kill_zero_exists(receipt.pid) => Some(true),
        None => None,
    }
}

#[cfg(target_os = "linux")]
#[must_use]
pub fn linux_process_birth(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm is parenthesized and may contain spaces or ')'; split only after its final ')'.
    let (_, tail) = stat.rsplit_once(") ")?;
    // tail starts at field 3 (state); starttime is field 22.
    let ticks = tail.split_whitespace().nth(19)?;
    ticks.parse::<u64>().ok().filter(|value| *value > 0)?;
    Some(ticks.to_owned())
}
