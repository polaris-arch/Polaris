//! The only execution admission gate. Disk manifests, paths, hashes, and open
//! Files are untrusted inputs. Only this module can create an execution owner.
use polaris_helper_proto::core_manifest::{CoreId, CoreManifest};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod unsupported;
#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
use unsupported as platform;
#[cfg(windows)]
use windows as platform;
#[cfg(windows)]
pub use windows::{WindowsChildCustody, WindowsMaintenanceGuard, WindowsStartedChild};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecPurpose {
    CheckConfig,
    TlsInventory,
    RunMain,
    RunLogin,
    RunSpeedtest,
}
impl ExecPurpose {
    fn contract_name(self) -> &'static str {
        match self {
            Self::CheckConfig => "check-config-v1",
            Self::TlsInventory => "tls-inventory-v1",
            Self::RunMain => "run-main-v1",
            Self::RunLogin => "run-login-v1",
            Self::RunSpeedtest => "run-speedtest-v1",
        }
    }
    pub(crate) fn is_run(self) -> bool {
        matches!(self, Self::RunMain | Self::RunLogin | Self::RunSpeedtest)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionErrorKind {
    Missing,
    Mismatch,
    Changed,
    UnsupportedExactObject,
    UnsupportedCapability,
    LoaderPolicy,
    Busy,
    Unknown,
    Io,
}
impl AdmissionErrorKind {
    /// Stable safe token in the existing CoreStatus.reason slot. This is a
    /// rejection classification, never an execution credential.
    pub fn status_token(self) -> &'static str {
        match self {
            Self::Missing => "admission-missing",
            Self::Mismatch => "admission-mismatch",
            Self::Changed => "admission-changed",
            Self::UnsupportedExactObject => "admission-unsupported-exact-object",
            Self::UnsupportedCapability => "admission-unsupported-capability",
            Self::LoaderPolicy => "admission-loader-policy",
            Self::Busy => "admission-busy",
            Self::Unknown => "admission-unknown",
            Self::Io => "admission-io",
        }
    }
    pub fn from_status_token(token: &str) -> Option<Self> {
        Some(match token {
            "admission-missing" => Self::Missing,
            "admission-mismatch" => Self::Mismatch,
            "admission-changed" => Self::Changed,
            "admission-unsupported-exact-object" => Self::UnsupportedExactObject,
            "admission-unsupported-capability" => Self::UnsupportedCapability,
            "admission-loader-policy" => Self::LoaderPolicy,
            "admission-busy" => Self::Busy,
            "admission-unknown" => Self::Unknown,
            "admission-io" => Self::Io,
            _ => return None,
        })
    }
    pub fn is_payload_damage(self) -> bool {
        matches!(self, Self::Missing | Self::Mismatch | Self::Changed)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    ReinstallCurrentApp,
    RepairHelperInstallation,
    RetryAfterStop,
    None,
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("CORE_ADMISSION_{kind:?}: {detail}; recovery={recovery:?}")]
pub struct AdmissionError {
    pub kind: AdmissionErrorKind,
    pub purpose: Option<ExecPurpose>,
    pub member: Option<String>,
    pub recovery: RecoveryAction,
    pub detail: String,
}
impl AdmissionError {
    pub fn unsupported_exact_object() -> Self {
        #[cfg(windows)]
        return windows::unavailable();
        #[cfg(not(windows))]
        Self::new(
            AdmissionErrorKind::UnsupportedExactObject,
            "pre-execution exact-object protection is not proved on this platform",
        )
    }
    pub(crate) fn new(kind: AdmissionErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            purpose: None,
            member: None,
            recovery: if matches!(
                kind,
                AdmissionErrorKind::UnsupportedExactObject
                    | AdmissionErrorKind::UnsupportedCapability
                    | AdmissionErrorKind::LoaderPolicy
                    | AdmissionErrorKind::Io
            ) {
                RecoveryAction::None
            } else if matches!(kind, AdmissionErrorKind::Busy | AdmissionErrorKind::Unknown) {
                RecoveryAction::RetryAfterStop
            } else {
                RecoveryAction::ReinstallCurrentApp
            },
        }
    }
}
impl From<std::io::Error> for AdmissionError {
    fn from(value: std::io::Error) -> Self {
        Self::new(
            if value.kind() == std::io::ErrorKind::NotFound {
                AdmissionErrorKind::Missing
            } else {
                AdmissionErrorKind::Io
            },
            value.to_string(),
        )
    }
}
impl From<AdmissionError> for std::io::Error {
    fn from(value: AdmissionError) -> Self {
        Self::other(value)
    }
}

/// A candidate, never an execution credential. Expected identity is NOT accepted
/// from a caller: the gate obtains it from its own compiled selection.
#[derive(Debug, Clone)]
pub struct UntrustedPayload {
    binary: PathBuf,
    #[cfg(target_os = "macos")]
    root_helper: bool,
}
impl UntrustedPayload {
    pub fn bundled(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            #[cfg(target_os = "macos")]
            root_helper: false,
        }
    }
}

#[derive(Debug, Clone)]
struct SelectedPolicy {
    manifest: CoreManifest,
    id: CoreId,
    source: String,
    tree: String,
    digest: String,
    purposes: Vec<ExecPurpose>,
    loader: String,
}
impl SelectedPolicy {
    fn compiled() -> Result<Self, AdmissionError> {
        // Immutable compiled metadata only: no File, namespace observation or
        // protected cohort lives in this cache. The execution slot stays unique.
        static POLICY: OnceLock<Result<SelectedPolicy, AdmissionError>> = OnceLock::new();
        POLICY.get_or_init(Self::read_compiled).clone()
    }
    fn read_compiled() -> Result<Self, AdmissionError> {
        use crate::selected_core;
        let platform = selected_core::current_platform()
            .map_err(|e| AdmissionError::new(AdmissionErrorKind::UnsupportedExactObject, e))?;
        if !matches!(platform, "linux" | "win" | "mac-arm64" | "mac-x64") {
            return Err(AdmissionError::unsupported_exact_object());
        }
        if selected_core::COMPILED_VALIDATION_SOURCE != b"null" {
            return Err(AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "candidate source has no compiled trusted-execution policy",
            ));
        }
        // The executor embeds the selected pure core identity before its own
        // installer/hash carrier exists. Helper/cleaner are installation data,
        // deliberately excluded by CoreManifest::canonical_bytes. Never make a
        // Helper hash depend on an installation record containing that same hash.
        let manifest = selected_core::selected_pure_manifest(platform)
            .map_err(|e| AdmissionError::new(AdmissionErrorKind::Mismatch, e))?;
        let id = manifest
            .id(sha256_hex)
            .map_err(|e| AdmissionError::new(AdmissionErrorKind::Mismatch, e))?;
        let contract: serde_json::Value = serde_json::from_str(
            &selected_core::selected_contract()
                .map_err(|e| AdmissionError::new(AdmissionErrorKind::Mismatch, e))?,
        )
        .map_err(|e| AdmissionError::new(AdmissionErrorKind::Mismatch, e.to_string()))?;
        let row = &contract["platforms"][platform];
        let policy = &row["trustedExecution"];
        let source = row["sourceCommit"].as_str().ok_or_else(|| {
            AdmissionError::new(AdmissionErrorKind::Mismatch, "missing selected source")
        })?;
        let tree = row["sourceTree"]
            .as_str()
            .or_else(|| contract["sourceTree"].as_str())
            .ok_or_else(|| {
                AdmissionError::new(AdmissionErrorKind::Mismatch, "missing selected tree")
            })?;
        let (purposes, loader) = Self::declaration(policy, source, tree, platform)?;
        Ok(Self {
            manifest,
            id,
            source: source.into(),
            tree: tree.into(),
            digest: sha256_hex(&serde_json::to_vec(policy).expect("policy JSON")),
            purposes,
            loader,
        })
    }
    fn declaration(
        policy: &serde_json::Value,
        source: &str,
        tree: &str,
        platform: &str,
    ) -> Result<(Vec<ExecPurpose>, String), AdmissionError> {
        // Only a future compiled, source-bound Helper/private-cohort selection
        // can reach the Windows issuer. The current release has no such policy.
        // An arbitrary path, receipt or environment never supplies this value.
        if platform == "win" {
            #[cfg(windows)]
            return windows::declaration(policy, source, tree);
            #[cfg(not(windows))]
            return Err(AdmissionError::new(
                AdmissionErrorKind::UnsupportedExactObject,
                "Windows exact-object execution is unavailable on this host",
            ));
        }
        const FIELDS: [&str; 5] = ["schema", "sourceCommit", "sourceTree", "loader", "purposes"];
        if !policy.as_object().is_some_and(|object| {
            object.len() == FIELDS.len() && FIELDS.iter().all(|key| object.contains_key(*key))
        }) {
            return Err(AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "trusted-execution v1 fields differ",
            ));
        }
        if policy["schema"] != "polaris-trusted-execution-v1"
            || policy["sourceCommit"] != source
            || policy["sourceTree"] != tree
        {
            return Err(AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "selected source has no exact trusted-execution policy",
            ));
        }
        let loader = policy["loader"].as_str().ok_or_else(|| {
            AdmissionError::new(AdmissionErrorKind::LoaderPolicy, "missing loader policy")
        })?;
        if platform == "linux" && loader != "linux-sealed-fd-v1" {
            return Err(AdmissionError::new(
                AdmissionErrorKind::LoaderPolicy,
                "explicit sealed FD loader required",
            ));
        }
        if matches!(platform, "mac-arm64" | "mac-x64")
            && (loader != "macos-root-private-v1"
                || policy["purposes"] != serde_json::json!(["run-main-v1"]))
        {
            return Err(AdmissionError::new(
                AdmissionErrorKind::LoaderPolicy,
                "Mac private execution is limited to root Helper RunMain",
            ));
        }
        let declared = policy["purposes"].as_array().ok_or_else(|| {
            AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "missing execution purposes",
            )
        })?;
        let mut purposes = Vec::new();
        for purpose in [
            ExecPurpose::CheckConfig,
            ExecPurpose::TlsInventory,
            ExecPurpose::RunMain,
            ExecPurpose::RunLogin,
            ExecPurpose::RunSpeedtest,
        ] {
            if declared
                .iter()
                .any(|v| v.as_str() == Some(purpose.contract_name()))
            {
                purposes.push(purpose);
            }
        }
        if purposes.len() != declared.len() {
            return Err(AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "unknown or repeated execution purpose",
            ));
        }
        Ok((purposes, loader.to_owned()))
    }
    fn allows(&self, purpose: ExecPurpose) -> Result<(), AdmissionError> {
        if self.purposes.contains(&purpose) {
            Ok(())
        } else {
            let mut error = AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "selected source does not support this execution purpose",
            );
            error.purpose = Some(purpose);
            Err(error)
        }
    }
}

/// A single protected, complete cohort. No serde, public fields, File/path/hash
/// constructors, or externally implementable authority traits.
#[derive(Debug)]
pub struct AdmittedPayload {
    policy: SelectedPolicy,
    candidate: UntrustedPayload,
    protected: platform::ProtectedMembers,
    retired: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    _fixture_directory: Option<FixtureDirectory>,
}
impl AdmittedPayload {
    pub fn id(&self) -> &CoreId {
        &self.policy.id
    }
    pub fn manifest(&self) -> &CoreManifest {
        &self.policy.manifest
    }
    pub fn source_commit(&self) -> &str {
        &self.policy.source
    }
    pub fn source_tree(&self) -> &str {
        &self.policy.tree
    }
    pub fn policy_digest(&self) -> &str {
        &self.policy.digest
    }
    pub fn diagnostic_path(&self) -> &Path {
        &self.candidate.binary
    }
    pub fn binary_sha256(&self) -> &str {
        &self
            .policy
            .manifest
            .files
            .iter()
            .find(|f| f.name == "sing-box" || f.name == "sing-box.exe")
            .expect("selected binary")
            .sha256
    }
    pub fn is_current(&self) -> bool {
        !self.retired.load(std::sync::atomic::Ordering::Acquire) && self.protected.is_current()
    }
    pub fn prepare(
        self: &Arc<Self>,
        purpose: ExecPurpose,
    ) -> Result<PreparedExecution, AdmissionError> {
        self.policy.allows(purpose).map_err(|mut e| {
            e.purpose = Some(purpose);
            e
        })?;
        if !self.is_current() {
            let mut error = AdmissionError::new(
                AdmissionErrorKind::Changed,
                "selected payload namespace or protection changed",
            );
            error.purpose = Some(purpose);
            return Err(error);
        }
        Ok(PreparedExecution {
            payload: Arc::clone(self),
            purpose,
            store_lease: None,
        })
    }
}

/// One selected slot, single flight. Invalid old owners are removed BEFORE a
/// replacement is attempted; a failed admission cannot lend an old cohort.
static SLOT: OnceLock<Mutex<Option<Arc<AdmittedPayload>>>> = OnceLock::new();
/// Read-only compiled capability hint for UI/preflight. It cannot produce an
/// owner, prove filesystem identity, or authorize a native birth.
pub fn check_selected_capability(purpose: ExecPurpose) -> Result<(), AdmissionError> {
    #[cfg(target_os = "macos")]
    {
        let mut error = AdmissionError::unsupported_exact_object();
        error.purpose = Some(purpose);
        return Err(error);
    }
    #[cfg(not(target_os = "macos"))]
    SelectedPolicy::compiled()
        .and_then(|p| p.allows(purpose))
        .map_err(|mut e| {
            e.purpose = Some(purpose);
            e
        })
}
pub fn invalidate_selected() {
    invalidate_slot(SLOT.get_or_init(Default::default));
}

fn invalidate_slot(mutex: &Mutex<Option<Arc<AdmittedPayload>>>) {
    let mut slot = mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(old) = slot.take() {
        old.retired
            .store(true, std::sync::atomic::Ordering::Release);
    }
    // An explicit retirement discards the complete slot even after a failed
    // observer; no stale cohort/lease may prevent trusted repair forever.
    mutex.clear_poison();
}

/// Retire queued selected-source plans on every exit, including an early error
/// or unwind. This is only invalidation: it grants no admission, mutation lock,
/// maintenance reservation, or authority to release an actual child's lease.
/// Callers must independently hold their existing mutation/operation guards.
pub fn with_selected_retirement_on_exit<T>(operation: impl FnOnce() -> T) -> T {
    with_retirement_on_exit(SLOT.get_or_init(Default::default), operation)
}

fn with_retirement_on_exit<T>(
    slot: &Mutex<Option<Arc<AdmittedPayload>>>,
    operation: impl FnOnce() -> T,
) -> T {
    struct RetireOnExit<'a>(&'a Mutex<Option<Arc<AdmittedPayload>>>);
    impl Drop for RetireOnExit<'_> {
        fn drop(&mut self) {
            invalidate_slot(self.0);
        }
    }
    let _retirement = RetireOnExit(slot);
    operation()
}

pub fn admit_selected(
    input: UntrustedPayload,
    purpose: ExecPurpose,
) -> Result<Arc<AdmittedPayload>, AdmissionError> {
    #[cfg(target_os = "macos")]
    {
        let _ = input;
        let mut error = AdmissionError::unsupported_exact_object();
        error.purpose = Some(purpose);
        return Err(error);
    }
    #[cfg(not(target_os = "macos"))]
    admit_locked(
        SLOT.get_or_init(Default::default),
        input,
        purpose,
        SelectedPolicy::compiled,
    )
}
/// The existing privileged Helper's RunMain entry. Input paths are still data:
/// the provider requires actual root credentials, a protected CoreStore chain,
/// the compiled id and a fresh, private, independently copied execution cohort.
#[cfg(target_os = "macos")]
pub fn admit_macos_root_helper_main(
    mut input: UntrustedPayload,
) -> Result<Arc<AdmittedPayload>, AdmissionError> {
    macos::require_root().map_err(|mut error| {
        error.purpose = Some(ExecPurpose::RunMain);
        error
    })?;
    input.root_helper = true;
    admit_locked(
        SLOT.get_or_init(Default::default),
        input,
        ExecPurpose::RunMain,
        SelectedPolicy::compiled,
    )
}

fn admit_locked(
    mutex: &Mutex<Option<Arc<AdmittedPayload>>>,
    input: UntrustedPayload,
    purpose: ExecPurpose,
    load: impl FnOnce() -> Result<SelectedPolicy, AdmissionError>,
) -> Result<Arc<AdmittedPayload>, AdmissionError> {
    let mut slot = mutex.lock().map_err(|_| {
        let mut error = AdmissionError::new(AdmissionErrorKind::Unknown, "admission slot poisoned");
        error.purpose = Some(purpose);
        error
    })?;
    admit_into(&mut slot, input, purpose, load).map_err(|mut error| {
        error.purpose = Some(purpose);
        error
    })
}

fn admit_into(
    slot: &mut Option<Arc<AdmittedPayload>>,
    input: UntrustedPayload,
    purpose: ExecPurpose,
    load: impl FnOnce() -> Result<SelectedPolicy, AdmissionError>,
) -> Result<Arc<AdmittedPayload>, AdmissionError> {
    if let Some(payload) = slot.as_ref() {
        if payload.candidate.binary == input.binary
            && {
                #[cfg(target_os = "macos")]
                {
                    payload.candidate.root_helper == input.root_helper
                }
                #[cfg(not(target_os = "macos"))]
                {
                    true
                }
            }
            && payload.is_current()
        {
            payload.policy.allows(purpose)?;
            return Ok(Arc::clone(payload));
        }
    }
    if let Some(old) = slot.take() {
        old.retired
            .store(true, std::sync::atomic::Ordering::Release);
    }
    let policy = load()?;
    policy.allows(purpose)?;
    let protected = platform::protect_members(&input, &policy)?;
    let payload = Arc::new(AdmittedPayload {
        policy,
        candidate: input,
        protected,
        retired: std::sync::atomic::AtomicBool::new(false),
        #[cfg(test)]
        _fixture_directory: None,
    });
    payload.prepare(purpose)?;
    *slot = Some(Arc::clone(&payload));
    Ok(payload)
}

/// A purpose-bound plan can only be issued by the admission owner.
///
/// ```compile_fail
/// use polaris_core_supervisor::admitted_payload::{PreparedExecution,ExecPurpose};
/// fn elevate(mut execution:PreparedExecution) {
///     execution.purpose=ExecPurpose::RunMain;
/// }
/// ```
#[derive(Debug)]
pub struct PreparedExecution {
    payload: Arc<AdmittedPayload>,
    purpose: ExecPurpose,
    store_lease: Option<Arc<std::fs::File>>,
}
impl PreparedExecution {
    /// Occupancy data added only after this execution was independently admitted.
    /// This does not create/change a purpose, policy, payload or execution owner.
    #[cfg(target_os = "linux")]
    pub fn with_store_lease(mut self, lease: Arc<std::fs::File>) -> Result<Self, AdmissionError> {
        self.payload.prepare(self.purpose)?;
        // Validate through the same existing native attachment primitive. A
        // dedicated child duplicate is attached again to the actual command.
        let mut command = self.payload.protected.command()?.into_std();
        platform::attach_store_lease(&mut command, &lease)?;
        self.store_lease = Some(lease);
        Ok(self)
    }
    pub(crate) fn lease_owner(&self) -> Option<Arc<dyn Send + Sync>> {
        self.store_lease
            .as_ref()
            .map(|lease| Arc::clone(lease) as Arc<dyn Send + Sync>)
    }
    pub fn diagnostic_path(&self) -> &Path {
        self.payload.diagnostic_path()
    }
    pub fn purpose(&self) -> ExecPurpose {
        self.purpose
    }
    pub(crate) fn payload(&self) -> Arc<AdmittedPayload> {
        Arc::clone(&self.payload)
    }
    #[cfg(target_os = "linux")]
    pub(crate) fn sealed_check_parts(
        &self,
    ) -> Result<(tokio::process::Command, std::fs::File, Arc<AdmittedPayload>), AdmissionError>
    {
        if self.purpose != ExecPurpose::CheckConfig {
            let mut error = AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "sealed validation requires CheckConfig purpose",
            );
            error.purpose = Some(self.purpose);
            return Err(error);
        }
        let mut command = self.command()?;
        command
            .args(["--disable-color", "check", "-c", "/proc/self/fd/0"])
            .current_dir("/")
            .kill_on_drop(true);
        Ok((
            command,
            self.payload.protected.binary_for_custody()?,
            self.payload(),
        ))
    }
    pub(crate) fn command(&self) -> Result<tokio::process::Command, AdmissionError> {
        self.payload.prepare(self.purpose)?;
        let mut command = self.payload.protected.command().map_err(|mut error| {
            error.purpose = Some(self.purpose);
            error
        })?;
        #[cfg(target_os = "linux")]
        if let Some(lease) = &self.store_lease {
            platform::attach_store_lease(command.as_std_mut(), lease)?;
        }
        Ok(command)
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    lowercase_hex(&sha2::Sha256::digest(bytes))
}
fn lowercase_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut hex, "{byte:02x}").expect("String write");
    }
    hex
}

pub(crate) struct ReapOwner(Option<Arc<AdmittedPayload>>, Option<Arc<dyn Send + Sync>>);
impl ReapOwner {
    pub(crate) fn new(owner: Option<Arc<AdmittedPayload>>) -> Self {
        Self(owner, None)
    }
    pub(crate) fn with_lease(mut self, lease: Option<Arc<dyn Send + Sync>>) -> Self {
        self.1 = lease;
        self
    }
    pub(crate) fn reaped(&mut self) {
        self.0.take();
        self.1.take();
    }
}
impl Drop for ReapOwner {
    fn drop(&mut self) {
        if let Some(owner) = self.0.take() {
            std::mem::forget(owner);
        }
        if let Some(owner) = self.1.take() {
            std::mem::forget(owner);
        }
    }
}

/// Native Helper options are data; no caller receives a mutable Command or can
/// replace the loader environment/pre-exec policy. No serialized credential.
#[cfg(target_os = "linux")]
pub struct LinuxHelperIdentity {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}
#[cfg(target_os = "linux")]
impl PreparedExecution {
    pub fn spawn_linux_helper_run(
        self,
        config: &Path,
        identity: LinuxHelperIdentity,
        pipe: bool,
        store_lease: &std::fs::File,
        lease_owner: Arc<dyn Send + Sync>,
    ) -> Result<StdCustodiedChild, AdmissionError> {
        if !self.purpose.is_run() {
            return Err(AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "validation plan cannot authorize Helper Run",
            ));
        }
        let mut command = self.command()?.into_std();
        command
            .arg("run")
            .arg("-c")
            .arg(config)
            .stdin(std::process::Stdio::null());
        if let Some(parent) = config.parent() {
            command.current_dir(parent);
        }
        if pipe {
            command
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
        } else {
            command
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
        }
        platform::attach_store_lease(&mut command, store_lease)?;
        platform::attach_helper_identity(&mut command, identity);
        let child = command.spawn()?;
        Ok(StdCustodiedChild {
            child: Some(child),
            owner: Some(self.payload),
            lease_owner: Some(lease_owner),
        })
    }
}
#[cfg(target_os = "macos")]
impl PreparedExecution {
    /// Data arguments are limited to the existing Helper run/config/log path.
    /// No mutable command or environment escapes this purpose-bound factory.
    pub fn spawn_macos_root_helper_main(
        self,
        config: &Path,
        working_directory: &Path,
        pipe: bool,
        store_lease: &std::fs::File,
        lease_owner: Arc<dyn Send + Sync>,
    ) -> Result<StdCustodiedChild, AdmissionError> {
        if self.purpose != ExecPurpose::RunMain
            || !self.payload.candidate.root_helper
            || !config.is_absolute()
            || !working_directory.is_absolute()
        {
            return Err(AdmissionError::new(
                AdmissionErrorKind::UnsupportedCapability,
                "root Helper factory requires an exact RunMain plan, config and working directory",
            ));
        }
        // Retirement and actual birth share the existing single-flight slot.
        // A queued plan must not pass a check, lose its generation, then spawn.
        let slot = SLOT.get_or_init(Default::default).lock().map_err(|_| {
            AdmissionError::new(
                AdmissionErrorKind::Unknown,
                "admission slot poisoned before birth",
            )
        })?;
        if !slot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &self.payload))
        {
            return Err(AdmissionError::new(
                AdmissionErrorKind::Changed,
                "queued root Helper execution generation was replaced",
            ));
        }
        self.payload.prepare(self.purpose)?;
        let mut command = self.payload.protected.root_command()?;
        macos::configure_main_command(&mut command, config, working_directory, pipe);
        let native_lease =
            macos::attach_store_lease(&mut command, store_lease, &self.payload.protected.lease)?;
        self.payload.prepare(self.purpose)?;
        let lease_owner = Arc::new((native_lease, lease_owner)) as Arc<dyn Send + Sync>;
        let child = command.spawn()?;
        Ok(StdCustodiedChild {
            child: Some(child),
            owner: Some(self.payload),
            lease_owner: Some(lease_owner),
        })
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub struct StdCustodiedChild {
    lease_owner: Option<Arc<dyn Send + Sync>>,
    child: Option<std::process::Child>,
    owner: Option<Arc<AdmittedPayload>>,
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
impl StdCustodiedChild {
    pub fn id(&self) -> u32 {
        self.child.as_ref().expect("owned Child").id()
    }
    #[cfg(target_os = "macos")]
    pub fn kill(&mut self) -> std::io::Result<()> {
        self.child.as_mut().expect("owned Child").kill()
    }
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.as_mut().expect("owned Child").try_wait()
    }
    pub fn take_stdout(&mut self) -> Option<OwnedReader<std::process::ChildStdout>> {
        let child = self.child.as_mut()?;
        child.stdout.take().map(|reader| OwnedReader {
            reader,
            _owner: Arc::clone(self.owner.as_ref().expect("admitted owner")),
            _lease_owner: self.lease_owner.as_ref().cloned(),
        })
    }
    pub fn take_stderr(&mut self) -> Option<OwnedReader<std::process::ChildStderr>> {
        let child = self.child.as_mut()?;
        child.stderr.take().map(|reader| OwnedReader {
            reader,
            _owner: Arc::clone(self.owner.as_ref().expect("admitted owner")),
            _lease_owner: self.lease_owner.as_ref().cloned(),
        })
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for StdCustodiedChild {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let owner = self.owner.take();
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        // Original helper custody normally reaps first. A lost caller still
        // retains the admitted cohort through an actual native wait.
        let mut owner = ReapOwner::new(owner);
        owner.1 = self.lease_owner.take();
        let _ = std::thread::Builder::new()
            .name("core-reap".into())
            .spawn(move || {
                if child.wait().is_ok() {
                    owner.reaped();
                }
            });
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub struct OwnedReader<R> {
    _lease_owner: Option<Arc<dyn Send + Sync>>,
    reader: R,
    _owner: Arc<AdmittedPayload>,
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
impl<R: std::io::Read> std::io::Read for OwnedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

#[cfg(test)]
#[derive(Debug)]
struct FixtureDirectory(PathBuf);
#[cfg(test)]
impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn fixture(bytes: &[u8], purposes: Vec<ExecPurpose>) -> Arc<AdmittedPayload> {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::temp_dir().canonicalize().unwrap().join(format!(
        "polaris-admitted-fixture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("sing-box"), bytes).unwrap();
    std::fs::set_permissions(
        directory.join("sing-box"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let sidecar = b"private-test-sidecar";
    std::fs::write(directory.join("libcronet.so"), sidecar).unwrap();
    let manifest = CoreManifest {
        platform: "linux".into(),
        version: "fixture".into(),
        provider_fingerprint: "a".repeat(64),
        helper_sha256: None,
        cleaner_sha256: None,
        files: vec![
            polaris_helper_proto::core_manifest::CoreFile {
                name: "sing-box".into(),
                bytes: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            },
            polaris_helper_proto::core_manifest::CoreFile {
                name: "libcronet.so".into(),
                bytes: sidecar.len() as u64,
                sha256: sha256_hex(sidecar),
            },
        ],
    };
    let policy = SelectedPolicy {
        id: manifest.id(sha256_hex).unwrap(),
        manifest,
        source: "b".repeat(40),
        tree: "c".repeat(40),
        digest: "d".repeat(64),
        purposes,
        loader: "linux-sealed-fd-v1".into(),
    };
    let candidate = UntrustedPayload::bundled(directory.join("sing-box"));
    let protected = platform::protect_members(&candidate, &policy).unwrap();
    Arc::new(AdmittedPayload {
        policy,
        candidate,
        protected,
        retired: std::sync::atomic::AtomicBool::new(false),
        _fixture_directory: Some(FixtureDirectory(directory)),
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests;

#[cfg(all(test, not(windows)))]
#[test]
fn foreign_windows_declaration_is_explicitly_unsupported() {
    let error = SelectedPolicy::declaration(&serde_json::Value::Null, "", "", "win")
        .expect_err("a foreign Windows declaration cannot grant execution");
    assert_eq!(error.kind, AdmissionErrorKind::UnsupportedExactObject);
}
