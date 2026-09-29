//! A private strict-check profile for the pinned b609 Linux x86_64 core.
//! The result is a checked candidate, never a launch permit or stop receipt.
//! The dynamic ELF still trusts the host loader and system libraries.

use crate::runtime::config::ApplyInputSnapshot;
use crate::runtime::proxy::mesh_apply::candidate::SealedCandidate;
use crate::runtime::proxy::ProxyRuntime;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateCheckError {
    Unsupported,
    SnapshotChanged,
    CheckFailed,
    TimedOut,
    CleanupUncertain,
}

/// Evidence is valid only for env={}, cwd=/, sealed binary and config memfds,
/// config at child fd 0, and the exact argv below. It cannot be reused for a
/// later spawn with inherited environment, working directory, or resources.
#[allow(dead_code)]
pub(crate) struct CheckedCandidate {
    candidate: SealedCandidate,
    binary_sha256: String,
    config_sha256: String,
    execution_profile: &'static str,
}

impl CheckedCandidate {
    pub(super) fn candidate(&self) -> &SealedCandidate {
        &self.candidate
    }

    pub(super) fn binary_sha256(&self) -> &str {
        &self.binary_sha256
    }

    pub(super) fn config_sha256(&self) -> &str {
        &self.config_sha256
    }

    pub(super) fn execution_profile(&self) -> &str {
        self.execution_profile
    }
}

pub(super) const PINNED_B609_LINUX_X86_64_SHA256: &str =
    "64f6d8613f9c7d42ef9a8e90dd9fca7290f176c5b482714915c04353911559c0";
pub(super) const CHECK_PROFILE: &str =
    "linux-x86_64-b609:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0";

/// This API has no route to manifest publication or ManagedDirectBirth.
/// Unknown platforms and missing kernel capabilities fail closed.
pub(crate) async fn strict_check_pinned_linux_candidate(
    runtime: &ProxyRuntime,
    snapshot: &ApplyInputSnapshot,
    candidate: SealedCandidate,
    binary_path: &Path,
) -> Result<CheckedCandidate, CandidateCheckError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        linux::check(runtime, snapshot, candidate, binary_path).await
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (runtime, snapshot, candidate, binary_path);
        Err(CandidateCheckError::Unsupported)
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux {
    use crate::runtime::config::ApplyInputSnapshot;
    use crate::runtime::proxy::mesh_apply::candidate::check::{
        CandidateCheckError, CheckedCandidate, CHECK_PROFILE, PINNED_B609_LINUX_X86_64_SHA256,
    };
    use crate::runtime::proxy::mesh_apply::candidate::SealedCandidate;
    use crate::runtime::proxy::mesh_apply::file_snapshot::FileSnapshot;
    use crate::runtime::proxy::ProxyRuntime;
    use nix::fcntl::{fcntl, FcntlArg, SealFlag};
    use nix::sys::memfd::{memfd_create, MFdFlags};
    use nix::sys::stat::{fchmod, Mode};
    use std::fs::{File, OpenOptions};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::process::Command;
    use tokio::sync::oneshot;

    const MAX_BINARY_BYTES: u64 = 96 * 1024 * 1024;
    const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;
    const CHECK_TIMEOUT: Duration = Duration::from_secs(8);
    const REQUIRED_SEALS: SealFlag = SealFlag::F_SEAL_WRITE
        .union(SealFlag::F_SEAL_GROW)
        .union(SealFlag::F_SEAL_SHRINK)
        .union(SealFlag::F_SEAL_SEAL);

    fn seal(file: &File) -> Result<(), CandidateCheckError> {
        fcntl(file, FcntlArg::F_ADD_SEALS(REQUIRED_SEALS))
            .map_err(|_| CandidateCheckError::Unsupported)?;
        let actual =
            fcntl(file, FcntlArg::F_GET_SEALS).map_err(|_| CandidateCheckError::Unsupported)?;
        if actual & REQUIRED_SEALS.bits() != REQUIRED_SEALS.bits() {
            return Err(CandidateCheckError::Unsupported);
        }
        Ok(())
    }

    pub(super) fn sealed_config(bytes: &[u8]) -> Result<File, CandidateCheckError> {
        if bytes.is_empty() || bytes.len() > MAX_CONFIG_BYTES {
            return Err(CandidateCheckError::Unsupported);
        }
        let fd = memfd_create(
            "polaris-strict-check-config",
            MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
        )
        .map_err(|_| CandidateCheckError::Unsupported)?;
        let mut file = File::from(fd);
        file.write_all(bytes)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        seal(&file)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| CandidateCheckError::Unsupported)?;
        let digest = polaris_updater::verify::sha256_reader_hex(&mut file)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        if digest != polaris_updater::verify::sha256_hex(bytes) {
            return Err(CandidateCheckError::Unsupported);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| CandidateCheckError::Unsupported)?;
        Ok(file)
    }

    pub(super) fn sealed_binary(path: &Path) -> Result<(File, FileSnapshot), CandidateCheckError> {
        if !path.is_absolute() {
            return Err(CandidateCheckError::Unsupported);
        }
        let before = FileSnapshot::path(path).map_err(|_| CandidateCheckError::Unsupported)?;
        if !before.is_file() || before.is_reparse() || before.len() > MAX_BINARY_BYTES {
            return Err(CandidateCheckError::Unsupported);
        }
        let mut source = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        let opened = FileSnapshot::opened(&source).map_err(|_| CandidateCheckError::Unsupported)?;
        if !before.same_snapshot(&opened) {
            return Err(CandidateCheckError::Unsupported);
        }
        let fd = memfd_create(
            "polaris-strict-check-b609",
            MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING,
        )
        .map_err(|_| CandidateCheckError::Unsupported)?;
        let mut file = File::from(fd);
        let copied = std::io::copy(
            &mut Read::by_ref(&mut source).take(MAX_BINARY_BYTES + 1),
            &mut file,
        )
        .map_err(|_| CandidateCheckError::Unsupported)?;
        if copied == 0 || copied > MAX_BINARY_BYTES || copied != opened.len() {
            return Err(CandidateCheckError::Unsupported);
        }
        let after = FileSnapshot::path(path).map_err(|_| CandidateCheckError::Unsupported)?;
        let source_after =
            FileSnapshot::opened(&source).map_err(|_| CandidateCheckError::Unsupported)?;
        if !opened.same_snapshot(&source_after) || !opened.same_snapshot(&after) {
            return Err(CandidateCheckError::Unsupported);
        }
        fchmod(&file, Mode::S_IRUSR | Mode::S_IXUSR)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        seal(&file)?;
        file.seek(SeekFrom::Start(0))
            .map_err(|_| CandidateCheckError::Unsupported)?;
        let digest = polaris_updater::verify::sha256_reader_hex(&mut file)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        if digest != PINNED_B609_LINUX_X86_64_SHA256 {
            return Err(CandidateCheckError::Unsupported);
        }
        // Keep the executable out of child fd 0 even when the host has closed
        // its standard descriptors. This safe duplication preserves CLOEXEC.
        let high_fd: OwnedFd = rustix::io::fcntl_dupfd_cloexec(&file, 3)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        Ok((File::from(high_fd), opened))
    }

    async fn reap_after_kill(child: &mut tokio::process::Child) -> Result<(), CandidateCheckError> {
        let _ = child.start_kill();
        // Even if kill reports that the process already exited, wait owns the
        // status and reaps it. No successful check can flow through this path.
        match child.wait().await {
            Ok(_) => Ok(()),
            Err(_) if child.try_wait().ok().flatten().is_some() => Ok(()),
            Err(_) => Err(CandidateCheckError::CleanupUncertain),
        }
    }

    pub(super) async fn run_check(
        binary: File,
        config: File,
        timeout: Duration,
    ) -> Result<(), CandidateCheckError> {
        let binary_path = format!("/proc/self/fd/{}", binary.as_raw_fd());
        // Atomic CLOEXEC duplication matters while unrelated app children may
        // spawn concurrently; only Command's fd-0 mapping becomes inheritable.
        let child_stdin = rustix::io::fcntl_dupfd_cloexec(&config, 3)
            .map(File::from)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        let mut command = Command::new(binary_path);
        command
            .args(["--disable-color", "check", "-c", "/proc/self/fd/0"])
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::from(child_stdin))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        supervise(command, binary, config, timeout, None).await
    }

    /// The sole asynchronous owner of the check Child and both sealed files.
    /// The optional PID notification is used only by cancellation tests; it
    /// never changes the production check profile or returns readiness.
    pub(super) async fn supervise(
        mut command: Command,
        binary: File,
        config: File,
        timeout: Duration,
        spawned: Option<oneshot::Sender<u32>>,
    ) -> Result<(), CandidateCheckError> {
        command.kill_on_drop(true);
        // The detached owner retains both sealed images and the Child even if
        // its caller is cancelled. A dropped receiver requests kill+reap.
        let (mut tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let _binary = binary;
            let _config = config;
            if tx.is_closed() {
                return;
            }
            let mut child = match command.spawn() {
                Ok(child) => child,
                Err(_) => {
                    let _ = tx.send(Err(CandidateCheckError::Unsupported));
                    return;
                }
            };
            if let (Some(notify), Some(pid)) = (spawned, child.id()) {
                let _ = notify.send(pid);
            }
            let result = tokio::select! {
                biased;
                _ = tx.closed() => {
                    match reap_after_kill(&mut child).await {
                        Ok(()) => Err(CandidateCheckError::CheckFailed),
                        Err(error) => Err(error),
                    }
                }
                _ = tokio::time::sleep(timeout) => {
                    match reap_after_kill(&mut child).await {
                        Ok(()) => Err(CandidateCheckError::TimedOut),
                        Err(error) => Err(error),
                    }
                }
                status = child.wait() => {
                    match status {
                        Ok(status) if status.success() => Ok(()),
                        _ => Err(CandidateCheckError::CheckFailed),
                    }
                }
            };
            let _ = tx.send(result);
        });
        rx.await.unwrap_or(Err(CandidateCheckError::CheckFailed))
    }

    pub(super) async fn check(
        runtime: &ProxyRuntime,
        snapshot: &ApplyInputSnapshot,
        candidate: SealedCandidate,
        binary_path: &Path,
    ) -> Result<CheckedCandidate, CandidateCheckError> {
        check_with_timeout(runtime, snapshot, candidate, binary_path, CHECK_TIMEOUT).await
    }

    pub(super) async fn check_with_timeout(
        runtime: &ProxyRuntime,
        snapshot: &ApplyInputSnapshot,
        candidate: SealedCandidate,
        binary_path: &Path,
        timeout: Duration,
    ) -> Result<CheckedCandidate, CandidateCheckError> {
        runtime
            .config
            .admit_mesh_apply_snapshot(snapshot)
            .map_err(|_| CandidateCheckError::SnapshotChanged)?;
        // The observed b609 check profile covers plain TCP VLESS here. TLS
        // engine/ECH/reality branches need separate syscall/resource evidence.
        let server = snapshot
            .raw()
            .get("servers")
            .and_then(serde_json::Value::as_array)
            .and_then(|servers| servers.first())
            .ok_or(CandidateCheckError::Unsupported)?;
        if server.get("tlsSettings").is_some()
            || server.get("realitySettings").is_some()
            || !matches!(
                server.get("security").and_then(serde_json::Value::as_str),
                None | Some("none")
            )
        {
            return Err(CandidateCheckError::Unsupported);
        }
        let closure = &candidate.materialized.closure;
        if !closure.rule_files.is_empty()
            || candidate.facts.raw_document_sha256 != snapshot.raw_document_sha256()
            || candidate.facts.config_version != snapshot.config_version()
            || candidate.facts.input_state_revision != snapshot.state().revision
            || closure.config_sha256 != candidate.facts.final_config_sha256
            || polaris_updater::verify::sha256_hex(&closure.config_bytes) != closure.config_sha256
        {
            return Err(CandidateCheckError::Unsupported);
        }
        let config = sealed_config(&closure.config_bytes)?;
        let (binary, source_identity) = sealed_binary(binary_path)?;
        run_check(binary, config, timeout).await?;
        let after =
            FileSnapshot::path(binary_path).map_err(|_| CandidateCheckError::Unsupported)?;
        if !source_identity.same_snapshot(&after) {
            return Err(CandidateCheckError::Unsupported);
        }
        runtime
            .config
            .admit_mesh_apply_snapshot(snapshot)
            .map_err(|_| CandidateCheckError::SnapshotChanged)?;
        Ok(CheckedCandidate {
            config_sha256: closure.config_sha256.clone(),
            binary_sha256: PINNED_B609_LINUX_X86_64_SHA256.into(),
            execution_profile: CHECK_PROFILE,
            candidate,
        })
    }
}
