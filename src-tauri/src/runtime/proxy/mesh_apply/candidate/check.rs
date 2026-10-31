//! Private reviewed identities for one fixed PlainTcpCheckV1 execution contract.
//! The result is a checked candidate, never a launch permit or stop receipt.
//! The dynamic ELF still trusts the host loader and system libraries.

use crate::runtime::config::ApplyInputSnapshot;
use crate::runtime::proxy::mesh_apply::candidate::SealedCandidate;
use crate::runtime::proxy::ProxyRuntime;
#[cfg(target_os = "linux")]
use polaris_core_supervisor::exact_spawn::LinuxInputProfile;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateCheckError {
    Unsupported,
    SnapshotChanged,
    CheckFailed,
    TimedOut,
    CleanupUncertain,
}

/// Private reviewed identity data, separate from the fixed execution semantics.
/// The historical row has no compiled contract admission; it remains available
/// only to explicit byte-pinned fixtures. No row enables managed launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReviewedCheckIdentity {
    source_commit: &'static str,
    contract_sha256: Option<&'static str>,
    evidence_label: &'static str,
    #[cfg(target_os = "linux")]
    binary_sha256: &'static str,
    #[cfg(target_os = "linux")]
    input_profile: LinuxInputProfile,
    #[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
    memfd_name: &'static str,
}

static REVIEWED_CHECK_IDENTITIES: [ReviewedCheckIdentity; 10] = [
    ReviewedCheckIdentity {
        source_commit: "b609f959f57ce34416c51c7b87ce4a76f2e1df56",
        contract_sha256: None,
        evidence_label: CHECK_PROFILE,
        #[cfg(target_os = "linux")]
        binary_sha256: PINNED_B609_LINUX_X86_64_SHA256,
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::B609,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-b609",
    },
    // The fe182 sealed-execution observation used a direct outbound only.
    // Full-builder/negative checks require their own evidence; this row makes
    // no VLESS, TLS, Naive or managed-launch claim.
    ReviewedCheckIdentity {
        source_commit: "fe182431495711c09bdb3b074ed6aa6309c00cf8",
        contract_sha256: Some("62ab483f37da5a7cb44c213722e0b92736da3bd232986641dc7e65d87dc7ce8d"),
        evidence_label: FE182_CHECK_PROFILE,
        #[cfg(target_os = "linux")]
        binary_sha256: PINNED_FE182_LINUX_X86_64_SHA256,
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::FE182,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-fe182",
    },
    // c002 package hashes, BuildInfo and static identity are verified, but no
    // sealed-runtime observation exists yet. Admit only this exact compiled
    // source/contract pair to the existing Linux check to collect its own
    // evidence; fe182 observations do not transfer to this release.
    ReviewedCheckIdentity {
        source_commit: "c0023dbc6dcd7b6eb2f54dd036ae150168bd8e6e",
        contract_sha256: Some("c8230c97b5e2c8ccdcdebc29cf08dd2ade302e48b5ec6d77a3d726a452edcec2"),
        evidence_label: "linux-x86_64-c002:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "b754ac8223b27f4c20b40ced82d9ad5d58fee8fb4c5bfa48c38566e805c570ae",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::C002,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-c002",
    },
    // Exact a4d085 public archive and metadata are verified. Its compiled
    // source/contract pair admits the existing check to collect its own sealed
    // evidence; static admission grants neither runtime success nor launch.
    ReviewedCheckIdentity {
        source_commit: "a4d0858605b51b7f5b3c29a9969a6f2c3ae5a0ae",
        contract_sha256: Some("f99daa91345df468b18e9c61451cc6465ffdce137fb1adc33829053ab2b194f3"),
        evidence_label: "linux-x86_64-a4d085:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "2a7495faeb18d2666dfcc77f678057c35cc7a7bb36b03a8a16ff9ac6249b4db8",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::A4D085,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-a4d085",
    },
    // Exact814 public package/source bytes are independently bound. This row
    // admits only the existing check semantics to collect fresh runtime evidence;
    // no historical observation transfers and no managed launch is granted.
    ReviewedCheckIdentity {
        source_commit: "814da362838e96e54f904836a92c2b0d8c3ae29b",
        contract_sha256: Some("c6c1a11c1f9c7eebe8e424dbcb6e9737dfa9828e3613012622a5e0069a9ee681"),
        evidence_label: "linux-x86_64-814da:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "363261029cea32333adcdd67288b23e4ae4084efa96b7f2b157af6e9d39946ee",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::E814DA,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-814da",
    },
    // Exact c6 producer bytes and final public contract are independently bound. This row
    // admits only the existing check semantics to collect fresh runtime evidence;
    // no historical observation transfers and no managed launch is granted.
    ReviewedCheckIdentity {
        source_commit: "c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a",
        contract_sha256: Some("976a9535eea2149ec7630956a3fb8ae9183c0167b732182b2bb42de65f4045d6"),
        evidence_label: "linux-x86_64-c6c0d8:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "80e21e78875f2f1cbfb1356dd977d6043a7776179df61f57897a4d811eaa1eaf",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::C6C0D8,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-c6c0d8",
    },
    // Exact 0cfa public source/contract and downloaded ELF bytes are verified.
    // This admits only the existing check to collect fresh sealed evidence;
    // historical c6 observations do not transfer and no managed launch is granted.
    ReviewedCheckIdentity {
        source_commit: "0cfa24573ff34c26608151068334329268cfdf14",
        contract_sha256: Some("96e8cc61d7abf345145b2af230724708f9a1baea77715467e38e96277ae0ffdf"),
        evidence_label: "linux-x86_64-0cfa24:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "7fde19e331e654280e134a89c26af5be706e4364e3003a74b7619c220d5a03aa",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::E0CFA24,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-0cfa24",
    },
    // Exact S332935 source/contract and producer ELF identity, matched to
    // public archive metadata. Public consumption and sealed-runtime evidence
    // remain separate gates; no historical observation or launch right transfers.
    ReviewedCheckIdentity {
        source_commit: "332935c310e78e15319d434f33a9103329200931",
        contract_sha256: Some("98b85fea9aeb8e09f883fa092eda56aca40cc2e20b234d2ab486881d6931ee6a"),
        evidence_label: "linux-x86_64-332935:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "d34fd051f8e089610bf32e58a5cf95ac36411827e46845157777baba078893ad",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::E332935,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-332935",
    },
    // Exact S6 bfcdf96 source/contract and producer ELF identity, matched to
    // public archive metadata. Public consumption and sealed-runtime evidence
    // remain separate gates; no historical observation or launch right transfers.
    ReviewedCheckIdentity {
        source_commit: "bfcdf96b244f63e0fe84736ee4a4edd58f6b209e",
        contract_sha256: Some("66894ed01e8fb8f40b90e11427c0041fec3dc1e69be4439cd9ac0dbc8c849e5f"),
        evidence_label: "linux-x86_64-bfcdf96:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "9089e0a78616d714eeb63d052dff8505a8b21ed9ba24285ed7444067cde69988",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::BFCDF96,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-bfcdf96",
    },
    // Exact alpha.12.7 source/contract and public package ELF identity.
    // This grants check admission only; fresh runtime evidence remains pending.
    ReviewedCheckIdentity {
        source_commit: "3155c9151f553967aabcee93f09e51084fc89899",
        contract_sha256: Some("9537af1d9de8d16885022f21e70647b8c5bec920179b8c1e2727a1e8ea420fdf"),
        evidence_label: "linux-x86_64-3155c9:runtime-evidence-pending:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0",
        #[cfg(target_os = "linux")]
        binary_sha256: "1188baf18f7e4d9d4daabcf01633d72a2174ccb95ed71847d5032d857ad6679b",
        #[cfg(target_os = "linux")]
        input_profile: LinuxInputProfile::E3155C9,
        #[cfg(all(test,target_os = "linux", target_arch = "x86_64"))]
        memfd_name: "polaris-strict-check-3155c9",
    },
];

/// Check/preparation identity only. The private rows cannot be constructed from
/// an environment variable, supplied hash, helper wire profile or launch permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinuxCheckProfile(&'static ReviewedCheckIdentity);

impl LinuxCheckProfile {
    #[cfg(test)]
    pub(super) const B609: Self = Self(&REVIEWED_CHECK_IDENTITIES[0]);
    #[cfg(test)]
    pub(super) const FE182: Self = Self(&REVIEWED_CHECK_IDENTITIES[1]);
    #[cfg(test)]
    pub(super) const C002: Self = Self(&REVIEWED_CHECK_IDENTITIES[2]);
    #[cfg(test)]
    pub(super) const A4D085: Self = Self(&REVIEWED_CHECK_IDENTITIES[3]);
    #[cfg(test)]
    pub(super) const E814DA: Self = Self(&REVIEWED_CHECK_IDENTITIES[4]);
    #[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
    pub(super) const C6C0D8: Self = Self(&REVIEWED_CHECK_IDENTITIES[5]);
    #[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
    pub(super) const E0CFA24: Self = Self(&REVIEWED_CHECK_IDENTITIES[6]);
    #[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
    pub(super) const E332935: Self = Self(&REVIEWED_CHECK_IDENTITIES[7]);
    #[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
    pub(super) const BFCDF96: Self = Self(&REVIEWED_CHECK_IDENTITIES[8]);
    #[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
    pub(super) const E3155C9: Self = Self(&REVIEWED_CHECK_IDENTITIES[9]);

    pub(super) const fn execution_profile(self) -> &'static str {
        self.0.evidence_label
    }

    #[cfg(target_os = "linux")]
    pub(super) const fn binary_sha256(self) -> &'static str {
        self.0.binary_sha256
    }

    #[cfg(target_os = "linux")]
    pub(super) const fn input_profile(self) -> LinuxInputProfile {
        self.0.input_profile
    }
}

/// Only exact compiled source/contract pairs can select an automatic identity.
/// A known source with a different contract, or a new source without reviewed
/// ELF identity, is unsupported even if it claims the same semantics. Admission
/// permits collecting check evidence; it does not claim that evidence exists.
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
fn reviewed_compiled_profile(
    source_commit: &str,
    contract_sha256: &str,
) -> Result<LinuxCheckProfile, CandidateCheckError> {
    REVIEWED_CHECK_IDENTITIES
        .iter()
        .find(|identity| {
            identity.source_commit == source_commit
                && identity.contract_sha256 == Some(contract_sha256)
        })
        .map(LinuxCheckProfile)
        .ok_or(CandidateCheckError::Unsupported)
}

/// Evidence is valid only for env={}, cwd=/, sealed binary and config memfds,
/// config at child fd 0, and the exact argv below. It cannot be reused for a
/// later spawn with inherited environment, working directory, or resources.
#[allow(dead_code)]
pub(crate) struct CheckedCandidate {
    candidate: SealedCandidate,
    binary_sha256: String,
    config_sha256: String,
    profile: LinuxCheckProfile,
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
        self.profile.execution_profile()
    }

    pub(super) fn profile(&self) -> LinuxCheckProfile {
        self.profile
    }
}

#[cfg(target_os = "linux")]
pub(super) const PINNED_B609_LINUX_X86_64_SHA256: &str =
    polaris_core_supervisor::exact_spawn::PINNED_B609_LINUX_X86_64_SHA256;
#[cfg(target_os = "linux")]
pub(super) const PINNED_FE182_LINUX_X86_64_SHA256: &str =
    polaris_core_supervisor::exact_spawn::PINNED_FE182_LINUX_X86_64_SHA256;
pub(super) const CHECK_PROFILE: &str =
    "linux-x86_64-b609:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0";
pub(super) const FE182_CHECK_PROFILE: &str =
    "linux-x86_64-fe182:env-empty:cwd-root:sealed-fd0:--disable-color check -c /proc/self/fd/0";

/// The explicit historical fe182 test mode remains byte-pinned. Otherwise the
/// caller supplies only a profile matched from trusted compiled identity data.
/// Invalid or unreviewed compiled selections never fall back to B609.
#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
fn select_test_core(
    candidate_mode: Option<&std::ffi::OsStr>,
    core: Option<&std::ffi::OsStr>,
    required: bool,
    compiled_profile: Result<LinuxCheckProfile, CandidateCheckError>,
) -> Result<Option<(std::path::PathBuf, LinuxCheckProfile)>, CandidateCheckError> {
    let profile = match candidate_mode {
        None if core.is_none() && !required => return Ok(None),
        None => compiled_profile?,
        Some(value) if value == "1" => LinuxCheckProfile::FE182,
        Some(_) => return Err(CandidateCheckError::Unsupported),
    };
    match core {
        Some(path) if Path::new(path).is_absolute() => {
            Ok(Some((std::path::PathBuf::from(path), profile)))
        }
        _ => Err(CandidateCheckError::Unsupported),
    }
}

#[cfg(all(test, target_os = "linux", target_arch = "x86_64"))]
pub(super) fn selected_core_for_test() -> Option<(std::path::PathBuf, LinuxCheckProfile)> {
    let compiled_profile = crate::runtime::core_identity::selected_check_source("linux")
        .map_err(|_| CandidateCheckError::Unsupported)
        .and_then(|(source, contract)| reviewed_compiled_profile(&source, &contract));
    select_test_core(
        std::env::var_os("POLARIS_NATIVE_DNS_CANDIDATE").as_deref(),
        std::env::var_os("POLARIS_TEST_CORE").as_deref(),
        std::env::var_os("POLARIS_REQUIRE_KERNEL_GATE").is_some(),
        compiled_profile,
    )
    .expect("invalid check identity or missing absolute POLARIS_TEST_CORE for required check")
}

/// This API has no route to manifest publication or ManagedDirectBirth.
/// Unknown platforms and missing kernel capabilities fail closed.
pub(crate) async fn strict_check_pinned_linux_candidate(
    runtime: &ProxyRuntime,
    snapshot: &ApplyInputSnapshot,
    candidate: SealedCandidate,
    binary_path: &Path,
    profile: LinuxCheckProfile,
) -> Result<CheckedCandidate, CandidateCheckError> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        linux::check(runtime, snapshot, candidate, binary_path, profile).await
    }
    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    {
        let _ = (runtime, snapshot, candidate, binary_path, profile);
        Err(CandidateCheckError::Unsupported)
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod linux {
    use crate::runtime::config::ApplyInputSnapshot;
    use crate::runtime::proxy::mesh_apply::candidate::check::{
        CandidateCheckError, CheckedCandidate, LinuxCheckProfile,
    };
    use crate::runtime::proxy::mesh_apply::candidate::SealedCandidate;
    use crate::runtime::proxy::mesh_apply::file_snapshot::FileSnapshot;
    use crate::runtime::proxy::ProxyRuntime;
    use nix::fcntl::{fcntl, FcntlArg, SealFlag};
    use nix::sys::memfd::{memfd_create, MFdFlags};
    #[cfg(test)]
    use nix::sys::stat::{fchmod, Mode};
    use std::fs::File;
    #[cfg(test)]
    use std::fs::OpenOptions;
    #[cfg(test)]
    use std::io::Read;
    use std::io::{Seek, SeekFrom, Write};
    #[cfg(test)]
    use std::os::fd::OwnedFd;
    #[cfg(test)]
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::Path;
    use std::time::Duration;
    use tokio::sync::oneshot;

    #[cfg(test)]
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

    #[cfg(test)]
    pub(super) fn sealed_binary(
        path: &Path,
        profile: LinuxCheckProfile,
    ) -> Result<(File, FileSnapshot), CandidateCheckError> {
        sealed_binary_with_stat_hook(path, profile, || {})
    }

    #[cfg(test)]
    pub(super) fn sealed_binary_with_stat_hook(
        path: &Path,
        profile: LinuxCheckProfile,
        after_stat: impl FnOnce(),
    ) -> Result<(File, FileSnapshot), CandidateCheckError> {
        if !path.is_absolute() {
            return Err(CandidateCheckError::Unsupported);
        }
        let before = FileSnapshot::path(path).map_err(|_| CandidateCheckError::Unsupported)?;
        if !before.is_file() || before.is_reparse() || before.len() > MAX_BINARY_BYTES {
            return Err(CandidateCheckError::Unsupported);
        }
        after_stat();
        let mut source = OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC | nix::libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        let opened = FileSnapshot::opened(&source).map_err(|_| CandidateCheckError::Unsupported)?;
        if !opened.is_file() || opened.is_reparse() || opened.len() > MAX_BINARY_BYTES {
            return Err(CandidateCheckError::Unsupported);
        }
        if !before.same_snapshot(&opened) {
            return Err(CandidateCheckError::Unsupported);
        }
        let name = profile.0.memfd_name;
        let fd = memfd_create(name, MFdFlags::MFD_CLOEXEC | MFdFlags::MFD_ALLOW_SEALING)
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
        if digest != profile.binary_sha256() {
            return Err(CandidateCheckError::Unsupported);
        }
        // Keep the executable out of child fd 0 even when the host has closed
        // its standard descriptors. This safe duplication preserves CLOEXEC.
        let high_fd: OwnedFd = rustix::io::fcntl_dupfd_cloexec(&file, 3)
            .map_err(|_| CandidateCheckError::Unsupported)?;
        Ok((File::from(high_fd), opened))
    }

    pub(super) enum CheckExecution {
        Admitted(polaris_core_supervisor::admitted_payload::PreparedExecution),
        #[cfg(test)]
        Unadmitted,
    }
    impl From<polaris_core_supervisor::admitted_payload::PreparedExecution> for CheckExecution {
        fn from(value: polaris_core_supervisor::admitted_payload::PreparedExecution) -> Self {
            Self::Admitted(value)
        }
    }
    #[cfg(test)]
    impl From<File> for CheckExecution {
        fn from(_: File) -> Self {
            Self::Unadmitted
        }
    }
    pub(super) async fn run_check(
        execution: impl Into<CheckExecution>,
        config: File,
        timeout: Duration,
    ) -> Result<(), CandidateCheckError> {
        match execution.into() {
            CheckExecution::Admitted(execution) => {
                supervise(execution, config, timeout, None).await
            }
            #[cfg(test)]
            CheckExecution::Unadmitted => Err(CandidateCheckError::Unsupported),
        }
    }
    /// Same central native wait/cancellation custody, with a private fixed
    /// CheckConfig command. Raw File/hash/profile data cannot authorize birth.
    pub(super) async fn supervise(
        execution: polaris_core_supervisor::admitted_payload::PreparedExecution,
        config: File,
        timeout: Duration,
        spawned: Option<oneshot::Sender<u32>>,
    ) -> Result<(), CandidateCheckError> {
        use polaris_core_supervisor::config_gate::{
            supervise_owned_sealed_check, RawCheck, ValidationLifecycleError,
        };
        match supervise_owned_sealed_check(execution, config, timeout, spawned).await {
            Ok(RawCheck::Done { success: true, .. }) => Ok(()),
            Ok(RawCheck::Done { .. } | RawCheck::OutputFailed(_)) => {
                Err(CandidateCheckError::CheckFailed)
            }
            Ok(RawCheck::TimedOut { .. }) => Err(CandidateCheckError::TimedOut),
            Ok(RawCheck::SpawnFailed(_)) | Err(ValidationLifecycleError::Admission(_)) => {
                Err(CandidateCheckError::Unsupported)
            }
            Err(_) => Err(CandidateCheckError::CleanupUncertain),
        }
    }

    pub(super) async fn check(
        runtime: &ProxyRuntime,
        snapshot: &ApplyInputSnapshot,
        candidate: SealedCandidate,
        binary_path: &Path,
        profile: LinuxCheckProfile,
    ) -> Result<CheckedCandidate, CandidateCheckError> {
        check_with_timeout(
            runtime,
            snapshot,
            candidate,
            binary_path,
            profile,
            CHECK_TIMEOUT,
        )
        .await
    }

    pub(super) async fn check_with_timeout(
        runtime: &ProxyRuntime,
        snapshot: &ApplyInputSnapshot,
        candidate: SealedCandidate,
        binary_path: &Path,
        profile: LinuxCheckProfile,
        timeout: Duration,
    ) -> Result<CheckedCandidate, CandidateCheckError> {
        runtime
            .config
            .admit_mesh_apply_snapshot(snapshot)
            .map_err(|_| CandidateCheckError::SnapshotChanged)?;
        candidate
            .validate_same_candidate(
                snapshot,
                &candidate.facts.plan,
                super::super::CandidateProfile::DesktopNonTunDirectVlessV1,
            )
            .map_err(|_| CandidateCheckError::Unsupported)?;
        // Scope comes from the same owned effective config used by the full
        // builder, never a new live deps/config projection after preparation.
        if candidate.metadata().check_support
            != super::super::CheckSupport::LinuxX8664PinnedPlainTcpOnly
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
        let source_identity =
            FileSnapshot::path(binary_path).map_err(|_| CandidateCheckError::Unsupported)?;
        let payload = crate::runtime::core_identity::acquire_at(
            binary_path,
            polaris_core_supervisor::admitted_payload::ExecPurpose::CheckConfig,
        )
        .map_err(|_| CandidateCheckError::Unsupported)?;
        if payload.binary_sha256() != profile.binary_sha256() {
            return Err(CandidateCheckError::Unsupported);
        }
        let execution = crate::runtime::core_identity::prepare_admitted(
            &payload,
            polaris_core_supervisor::admitted_payload::ExecPurpose::CheckConfig,
        )
        .map_err(|_| CandidateCheckError::Unsupported)?;
        run_check(execution, config, timeout).await?;
        let after =
            FileSnapshot::path(binary_path).map_err(|_| CandidateCheckError::Unsupported)?;
        if !source_identity.same_snapshot(&after) {
            return Err(CandidateCheckError::Unsupported);
        }
        runtime
            .config
            .admit_mesh_apply_snapshot(snapshot)
            .map_err(|_| CandidateCheckError::SnapshotChanged)?;
        candidate
            .validate_same_candidate(
                snapshot,
                &candidate.facts.plan,
                super::super::CandidateProfile::DesktopNonTunDirectVlessV1,
            )
            .map_err(|_| CandidateCheckError::Unsupported)?;
        Ok(CheckedCandidate {
            config_sha256: closure.config_sha256.clone(),
            binary_sha256: profile.binary_sha256().into(),
            profile,
            candidate,
        })
    }
}
