//! Closed verification-session data custody. This module cannot install a provider,
//! create an execution owner, change the selected contract, or retire a native birth.
#[cfg(any(target_os = "macos", windows))]
use super::HelperRuntime;
use super::CASE_NAMES;
use polaris_core_supervisor::admitted_payload::{check_selected_capability, ExecPurpose};
use polaris_helper::core_objects::PayloadDir;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const MAX_RETAINED_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 2048;
const MAX_RECORD_BYTES: u64 = 64 * 1024;
const OWNER: &str = "session-owner.json";
const SUITE_LOCK: &str = "suite.lock";
const CASE_LOCK: &str = "case.lock";

fn failure(stage: &str, code: &str, detail: impl std::fmt::Display) -> String {
    println!(
        "POLARIS_NATIVE_PREREQUISITE {}",
        json!({
            "schema":"polaris-native-prerequisite-v1", "state":"unmet", "stage":stage,
            "code":code, "detail":detail.to_string(), "processId":std::process::id(),
            "compiledAppBuildId":option_env!("POLARIS_BUILD_ID").unwrap_or("unavailable"),
            "platform":crate::runtime::core_identity::current_platform().ok(),
            "requiredCases":CASE_NAMES, "required":true, "publicationEligible":false
        })
    );
    format!("native prerequisite unmet: {stage}/{code}")
}

/// Read-only production predicate, before any directory, listener or runtime constructor.
pub(super) fn capability_preflight() -> Result<(), String> {
    super::super::core_binary::dev_core_binary_override()
        .map(drop)
        .map_err(|e| failure("selection", "path-override-refused", e))?;
    if polaris_core_supervisor::selected_core::selection_lane()
        != polaris_core_supervisor::selected_core::SelectionLane::Formal
    {
        return Err(failure(
            "selection",
            "formal-selection-required",
            "no development fallback",
        ));
    }
    for purpose in [ExecPurpose::CheckConfig, ExecPurpose::RunMain] {
        check_selected_capability(purpose)
            .map_err(|e| failure("capability", e.kind.status_token(), e))?;
    }
    Ok(())
}

fn caller() -> Result<String, String> {
    #[cfg(unix)]
    {
        let uid = nix::unistd::getuid().as_raw();
        if uid == 0 {
            return Err("session holder requires the ordinary caller, not root".into());
        }
        Ok(polaris_helper_proto::core_protocol::CallerIdentity::Unix(uid).key())
    }
    #[cfg(windows)]
    {
        let value = polaris_helper::platform::windows::native_caller::current_user()
            .map_err(|e| e.to_string())?
            .key();
        if value == "sS-1-5-18" {
            return Err("session holder requires the ordinary caller, not SYSTEM".into());
        }
        Ok(value)
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err("native caller unavailable".into())
    }
}
fn root() -> Result<PathBuf, String> {
    #[cfg(unix)]
    let parent = std::env::temp_dir()
        .canonicalize()
        .map_err(|e| e.to_string())?;
    #[cfg(windows)]
    let parent = {
        // canonicalize yields a verbatim path on Windows; the real helper only
        // admits ordinary drive-absolute config names. Reuse that syntax gate.
        let path = std::env::temp_dir();
        let probe = path.join("polaris-session-path-observation");
        if !polaris_helper::platform::windows::logic::cfg_allowed(
            probe.to_str().ok_or("native temp path is not Unicode")?,
            path.to_str().ok_or("native temp path is not Unicode")?,
        ) {
            return Err("native temp path is not a supported ordinary config namespace".into());
        }
        path
    };
    #[cfg(not(any(unix, windows)))]
    let parent = return Err("native verification data unsupported".into());
    let key = polaris_updater::sha256_hex(caller()?.as_bytes());
    Ok(parent.join(format!("polaris-native-production-verification-{key}")))
}
fn private_directory(path: &Path) -> Result<PayloadDir, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if m.uid() != nix::unistd::getuid().as_raw() || m.mode() & 0o077 != 0 {
            return Err("verification directory must be private to the native caller".into());
        }
        PayloadDir::open(path, Some(m.uid())).map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let f = OpenOptions::new()
            .read(true)
            .custom_flags(0x02200000)
            .share_mode(3)
            .open(path)
            .map_err(|e| e.to_string())?;
        let m = f.metadata().map_err(|e| e.to_string())?;
        use std::os::windows::fs::MetadataExt;
        if !m.is_dir() || m.file_attributes() & 0x400 != 0 {
            return Err("verification reparse directory refused".into());
        }
        let sid = caller()?;
        let sec = polaris_helper::platform::windows::winproc::file_security(&f)
            .map_err(|e| e.to_string())?;
        if !polaris_helper::platform::windows::coreacl::is_private_user_config(&sec, &sid[1..]) {
            return Err("verification directory SID/isolation differs".into());
        }
        PayloadDir::open(path, None).map_err(|e| e.to_string())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err("verification data unsupported".into())
    }
}
fn create_directory(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(path)
            .map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    {
        polaris_helper::core_store::create_current_user_data_directory(path)
            .map_err(|e| e.to_string())?;
    }
    private_directory(path).map(drop)
}
fn create_file(path: &Path) -> Result<File, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| e.to_string())
    }
    #[cfg(windows)]
    {
        polaris_helper::core_store::create_current_user_data_file(path).map_err(|e| e.to_string())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Err("verification data unsupported".into())
    }
}
fn record(path: &Path, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err("verification record exceeds budget".into());
    }
    let mut f = create_file(path)?;
    f.write_all(&bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| e.to_string())
}
fn read_record(dir: &PayloadDir, name: &str) -> Result<Value, String> {
    let (f, bytes) = dir.open_regular(name).map_err(|e| e.to_string())?;
    if bytes > MAX_RECORD_BYTES {
        return Err("verification record exceeds budget".into());
    }
    let mut raw = Vec::new();
    f.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|e| e.to_string())?;
    if raw.len() as u64 > MAX_RECORD_BYTES {
        return Err("verification record grew beyond budget".into());
    }
    serde_json::from_slice(&raw).map_err(|e| e.to_string())
}
fn lock_file(root: &Path, dir: &PayloadDir, name: &str) -> Result<File, String> {
    // CREATE_NEW only on genuine absence. A link/type/permission error is never repaired.
    let f = match std::fs::symlink_metadata(root.join(name)) {
        Ok(_) => dir
            .open_regular(name)
            .map(|v| v.0)
            .map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => create_file(&root.join(name))?,
        Err(e) => return Err(e.to_string()),
    };
    f.try_lock()
        .map_err(|e| format!("verification slot busy/unknown: {e}"))?;
    Ok(f)
}
fn retained_size(root: &Path) -> Result<u64, String> {
    fn visit(path: &Path, depth: usize, count: &mut usize, total: &mut u64) -> Result<(), String> {
        if depth > 32 || *count >= MAX_ENTRIES {
            return Err("verification retained-entry budget exceeded".into());
        }
        *count += 1;
        let m = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err("retained reparse object refused".into());
            }
        }
        if m.file_type().is_symlink() {
            return Err("retained linked object refused".into());
        }
        if m.is_file() {
            *total = total.checked_add(m.len()).ok_or("retained size overflow")?;
        } else if m.is_dir() {
            for e in std::fs::read_dir(path).map_err(|e| e.to_string())? {
                visit(
                    &e.map_err(|e| e.to_string())?.path(),
                    depth + 1,
                    count,
                    total,
                )?;
            }
        } else {
            return Err("retained special object refused".into());
        }
        if *total > MAX_RETAINED_BYTES {
            return Err("verification retained-byte budget exceeded; preserve and stop".into());
        }
        Ok(())
    }
    let mut count = 0;
    let mut bytes = 0;
    visit(root, 0, &mut count, &mut bytes)?;
    Ok(bytes)
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Owner {
    schema: String,
    session_id: String,
    compiled_app_build_id: String,
    caller: String,
    platform: String,
    core_id: String,
    provider_fingerprint: String,
    core_files: Vec<polaris_helper_proto::core_manifest::CoreFile>,
    helper_sha256: Option<String>,
    cleaner_sha256: Option<String>,
    required_cases: Vec<String>,
}
fn selected_owner() -> Result<Owner, String> {
    let platform = crate::runtime::core_identity::current_platform()?.to_string();
    let m = crate::runtime::core_identity::selected_manifest(&platform)?;
    let id = m
        .id(polaris_updater::sha256_hex)
        .map_err(|e| e.to_string())?;
    Ok(Owner {
        schema: "polaris-native-session-owner-v1".into(),
        session_id: String::new(),
        compiled_app_build_id: option_env!("POLARIS_BUILD_ID")
            .unwrap_or("unavailable")
            .into(),
        caller: caller()?,
        platform,
        core_id: id.as_str().into(),
        provider_fingerprint: m.provider_fingerprint,
        core_files: m.files,
        helper_sha256: m.helper_sha256,
        cleaner_sha256: m.cleaner_sha256,
        required_cases: CASE_NAMES.iter().map(|v| (*v).into()).collect(),
    })
}
fn matches_selection(a: &Owner, b: &Owner) -> bool {
    let mut a = a.clone();
    a.session_id.clear();
    a == *b
}
fn provider_ready(root: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "macos", windows))]
    {
        let h = HelperRuntime::new(root.to_path_buf());
        let s = h.status();
        if !s.installed || !s.ready || s.upgradeable || s.needs_repair {
            return Err(
                "installed selected provider or native caller registration unavailable".into(),
            );
        }
        if s.selected_core
            .as_ref()
            .is_none_or(|c| c.state != crate::runtime::helper::SelectedCoreState::Present)
        {
            return Err("selected CoreID not present in provider".into());
        }
        if !provider_idle(root)? {
            return Err("native provider birth is occupied or unknown".into());
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = root;
    }
    Ok(())
}

fn provider_idle(root: &Path) -> Result<bool, String> {
    #[cfg(any(target_os = "macos", windows))]
    {
        // A plaintext session checkpoint cannot supply this observation.
        let token = polaris_helper_client::read_token(&root.join("helper-client.token"));
        #[cfg(target_os = "macos")]
        let c = polaris_helper_client::check_config::SystemMacCheckClient::system(token)?;
        #[cfg(windows)]
        let c = polaris_helper_client::check_config::SystemWindowsCheckClient::system(token)?;
        Ok(matches!(
            c.native_birth_status()?,
            polaris_helper_proto::NativeBirthStatus::Empty
        ))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = root;
        Ok(true)
    }
}

/// Long-lived ordinary process holds the one fixed per-user slot. stdin accepts
/// only the closed teardown action (EOF also tears down); no path/credential input.
pub(super) fn hold_session() -> Result<(), String> {
    capability_preflight()?;
    let mut owner =
        selected_owner().map_err(|e| failure("selection", "compiled-selection-unavailable", e))?;
    let root = root().map_err(|e| failure("session", "caller-unavailable", e))?;
    match std::fs::symlink_metadata(&root) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => create_directory(&root)
            .map_err(|e| failure("session", "private-directory-unavailable", e))?,
        Err(e) => return Err(failure("session", "private-directory-unavailable", e)),
    }
    let dir = private_directory(&root)
        .map_err(|e| failure("session", "private-directory-unavailable", e))?;
    let _suite =
        lock_file(&root, &dir, SUITE_LOCK).map_err(|e| failure("session", "slot-busy", e))?;
    // A retained previous owner is not silently retired, even if its process died.
    if std::fs::symlink_metadata(root.join(OWNER)).is_ok() {
        return Err(failure(
            "session",
            "retained-owner",
            "explicit owner cleanup required; no stale-owner guessing",
        ));
    }
    let before = retained_size(&root).map_err(|e| failure("session", "retention-budget", e))?;
    owner.session_id = uuid::Uuid::new_v4().to_string();
    let ready = provider_ready(&root);
    // Missing registration/provider produces a preparation descriptor, never an
    // active session owner. A later explicitly authorized installation may use
    // this same stable root without deleting/forging a stale native session.
    if ready.is_ok() {
        record(
            &root.join(OWNER),
            &serde_json::to_value(&owner).map_err(|e| e.to_string())?,
        )?;
        for i in 0..CASE_NAMES.len() {
            create_directory(&root.join(format!("case-{i}")))?;
        }
    }
    println!(
        "POLARIS_NATIVE_SESSION {}",
        json!({"schema":"polaris-native-session-v1","state":if ready.is_ok(){"ready"}else{"unmet"},
        "code":if ready.is_ok(){"ready"}else{"provider-unavailable"},"detail":ready.err(),"processId":std::process::id(),
        "owner":owner,"configRoot":root,"provider":"compiled-platform-production-route","registrationScope":"enforced-by-production-start","executionAuthorized":false,
        "slots":1,"maxRetainedBytes":MAX_RETAINED_BYTES,"retainedBytesBefore":before,"required":true,"publicationEligible":false})
    );
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut input = String::new();
    let action = std::io::stdin().take(32).read_to_string(&mut input);
    let action_ok = action.is_ok() && (input.is_empty() || input == "teardown\n");
    // The case lock proves no case process still holds this coordination slot.
    // It never proves native exit: require each started original owner's drain too.
    let case_lock = lock_file(&root, &dir, CASE_LOCK);
    let mut drained = case_lock.is_ok();
    let mut started = 0;
    for i in 0..CASE_NAMES.len() {
        let start = format!("case-{i}-started.json");
        let end = format!("case-{i}-drained.json");
        match std::fs::symlink_metadata(root.join(&start)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                started += 1;
                let a = read_record(&dir, &start);
                let b = read_record(&dir, &end);
                drained &= matches!((a,b),(Ok(a),Ok(b)) if a==b && a["sessionId"]==owner.session_id && a["case"]==CASE_NAMES[i]);
            }
            Err(_) => drained = false,
        }
    }
    let endpoint_idle = provider_idle(&root).ok();
    let retained = retained_size(&root);
    let ok =
        action_ok && drained && (started == 0 || endpoint_idle == Some(true)) && retained.is_ok();
    println!(
        "POLARIS_NATIVE_SESSION_TEARDOWN {}",
        json!({"schema":"polaris-native-session-teardown-v1",
        "sessionId":owner.session_id,"processId":std::process::id(),"startedCases":started,
        "caseDrainCheckpointsMatched":drained,"providerIdle":endpoint_idle,"state":if ok {if started==0 {"retained-unopened"} else {"retained"}} else {"cleanup-unconfirmed"},
        "directoryState":"retained","retainedBytes":retained.as_ref().ok(),"maxRetainedBytes":MAX_RETAINED_BYTES,
        "retentionReason":"no descriptor-bound delete API; preserve token, registration-bound root and original case data for explicit owner cleanup",
        "evidenceKind":"data-coordination-only","registrationChanged":false,"providerInstalled":false,"publicationEligible":false})
    );
    if !ok {
        return Err("verification session teardown unconfirmed; original data retained".into());
    }
    Ok(())
}

pub(super) struct CaseSession {
    pub(super) config_root: PathBuf,
    pub(super) case_dir: PathBuf,
    root: PathBuf,
    receipt: Value,
    _case_lock: File,
    _directory: PayloadDir,
}
impl CaseSession {
    pub(super) fn open(case: &str) -> Result<Self, String> {
        capability_preflight()?;
        let i = CASE_NAMES
            .iter()
            .position(|v| *v == case)
            .ok_or("unknown native case")?;
        let root = root()?;
        let directory = private_directory(&root)?;
        let suite = directory
            .open_regular(SUITE_LOCK)
            .map_err(|e| e.to_string())?
            .0;
        match suite.try_lock() {
            Err(std::fs::TryLockError::WouldBlock) => {}
            Err(e) => return Err(failure("session", "suite-lock-unknown", e)),
            Ok(()) => {
                return Err(failure(
                    "session",
                    "suite-owner-missing",
                    "run the closed session holder first",
                ))
            }
        }
        let case_lock = lock_file(&root, &directory, CASE_LOCK)?;
        retained_size(&root)?;
        let owner: Owner =
            serde_json::from_value(read_record(&directory, OWNER)?).map_err(|e| e.to_string())?;
        if uuid::Uuid::parse_str(&owner.session_id).is_err()
            || !matches_selection(&owner, &selected_owner()?)
        {
            return Err(failure(
                "session",
                "owner-mismatch",
                "compiled selection or native caller differs",
            ));
        }
        provider_ready(&root).map_err(|e| failure("provider", "provider-unavailable", e))?;
        let case_dir = root.join(format!("case-{i}"));
        private_directory(&case_dir)?;
        let receipt =
            json!({"sessionId":owner.session_id,"case":case,"processId":std::process::id()});
        record(&root.join(format!("case-{i}-started.json")), &receipt)?;
        Ok(Self {
            config_root: root.clone(),
            case_dir,
            root,
            receipt,
            _case_lock: case_lock,
            _directory: directory,
        })
    }
    /// Called only after the original runtime/check owners both actually drained.
    /// This record coordinates retention; it is never an execution/retirement issuer.
    pub(super) fn drained(&self, case: &str) -> Result<(), String> {
        let i = CASE_NAMES
            .iter()
            .position(|v| *v == case)
            .ok_or("unknown native case")?;
        record(
            &self.root.join(format!("case-{i}-drained.json")),
            &self.receipt,
        )
    }
}
