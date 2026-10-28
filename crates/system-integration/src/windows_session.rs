//! Read-only Windows session state, shared by the app and the SYSTEM helper.
//!
//! The OS metric describes this process's session only (SYSTEM helper uses session 0). Do not
//! latch that metric: Windows clears it when ending the session is cancelled.
//! Only a confirmed final app Exit permanently fences that process's DNS work.

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

static FINAL_EXIT: Mutex<bool> = Mutex::new(false);
static QUERY_END: AtomicBool = AtomicBool::new(false);

/// The App's own reversible session query; never a SYSTEM helper session receipt.
pub fn begin_query() {
    QUERY_END.store(true, Ordering::SeqCst);
}

/// Clear only a cancelled App query, never the permanent final-exit fence.
pub fn cancel_query() {
    QUERY_END.store(false, Ordering::SeqCst);
}

/// Fence queued DNS work before the app starts its final, non-vetoable cleanup.
pub fn begin_final_exit() {
    fence_final_exit(&FINAL_EXIT);
}

fn fence_final_exit(final_exit: &Mutex<bool>) {
    match final_exit.lock() {
        Ok(mut ending) => *ending = true,
        Err(poison) => {
            *poison.into_inner() = true;
            log::error!("Windows DNS session admission poisoned; keeping it closed");
        }
    }
}

/// No child process, registry change or network operation is involved.
pub fn is_ending() -> bool {
    QUERY_END.load(Ordering::SeqCst)
        || FINAL_EXIT.lock().map_or(true, |ending| *ending)
        || os_is_ending()
}

#[allow(unsafe_code, reason = "read-only GetSystemMetrics query")]
fn os_is_ending() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SHUTTINGDOWN};
    // SAFETY: GetSystemMetrics takes a constant index and borrows no resources.
    unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

/// Keep the final Exit fence and optional worker creation in one critical section.
/// Release before waiting for the native cache call; this is not a remote-session receipt.
pub(crate) fn admit_launch() -> io::Result<MutexGuard<'static, bool>> {
    admit_if_open(&FINAL_EXIT, || {
        QUERY_END.load(Ordering::SeqCst) || os_is_ending()
    })
}

fn admit_if_open(
    final_exit: &Mutex<bool>,
    os_ending: impl FnOnce() -> bool,
) -> io::Result<MutexGuard<'_, bool>> {
    let ending = final_exit
        .lock()
        .map_err(|_| io::Error::other("Windows DNS session admission poisoned"))?;
    if *ending || os_ending() {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Windows session ending; DNS flush not launched",
        ));
    }
    Ok(ending)
}

#[cfg(test)]
mod tests;
