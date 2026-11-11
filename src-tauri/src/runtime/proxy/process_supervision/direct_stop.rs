//! Native stop I/O for a locally owned desktop Child.

use std::future::Future;
use std::io;
use std::process::ExitStatus;
use std::task::{Context, Poll};

use crate::runtime::core_child::Child;

pub(in crate::runtime::proxy) trait DirectStopIo: Send + Sync {
    fn start_kill(&self, child: &mut Child) -> io::Result<()>;

    fn poll_wait(&self, child: &mut Child, cx: &mut Context<'_>) -> Poll<io::Result<ExitStatus>> {
        // Tokio wait is cancellation-safe: its process handle and cached exit
        // live in Child, rather than in this temporary borrowing future.
        let wait = child.wait();
        tokio::pin!(wait);
        wait.poll(cx)
    }

    fn terminate(&self, child: &mut Child) -> io::Result<()> {
        #[cfg(windows)]
        {
            // Windows has no graceful signal here. Kill the owned handle;
            // running taskkill under the slot lock would block and use a PID.
            child.start_kill()
        }
        #[cfg(not(windows))]
        {
            if let Some(pid) = child.id() {
                super::send_signal(pid, polaris_core_supervisor::Signal::Sigterm);
            }
            Ok(())
        }
    }
}

pub(super) struct NativeStopIo;

impl DirectStopIo for NativeStopIo {
    fn start_kill(&self, child: &mut Child) -> io::Result<()> {
        child.start_kill()
    }
}
