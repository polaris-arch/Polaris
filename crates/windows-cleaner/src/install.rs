//! The production Windows installation transaction. Native IO is injected at
//! the operation boundary, so recovery tests execute this very same body.
use serde::{Deserialize, Serialize};
use std::io;

pub const MAX_REQUEST: usize = 64 * 1024;
pub const MAX_FILE: u64 = 512 * 1024 * 1024;

/// Embedded only by the maintenance BIN, after the Helper LIB has built. CoreId
/// excludes installation helper/cleaner hashes, so this has no self-hash cycle.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub core_id: String,
    pub helper_sha256: String,
}
impl Anchor {
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > 1024 {
            return Err(io::Error::other("maintenance anchor too large"));
        }
        let a: Self = serde_json::from_slice(bytes)
            .map_err(|_| io::Error::other("maintenance anchor unavailable"))?;
        for value in [&a.core_id, &a.helper_sha256] {
            polaris_helper_proto::core_manifest::CoreId::parse(value).map_err(io::Error::other)?;
        }
        Ok(a)
    }
    pub fn admits(&self, request: &Request) -> io::Result<()> {
        if self.core_id != request.core_id || self.helper_sha256 != request.helper_sha256 {
            return Err(io::Error::other(
                "request differs from independently embedded producer anchor",
            ));
        }
        Ok(())
    }
}

/// Data only. No service, target root, executable, command, PID or arbitrary argv.
/// The secret is written to a private held file; never format this structure.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub stage: String,
    pub original_sid: String,
    pub config: String,
    pub core_id: String,
    pub manifest_sha256: String,
    pub helper_sha256: String,
    pub cleaner_sha256: String,
    pub token: String,
    pub stage_identity: [u32; 3],
    pub source_identities: Vec<SourceIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub name: String,
    pub object: [u32; 3],
    pub bytes: u64,
}

pub(crate) fn local_path(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() <= 32760
        && b.len() > 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && matches!(b[2], b'\\' | b'/')
        && !v.contains(['\0', '\n', '\r', '"'])
        && !v[2..].contains(':')
        && !v.split(['\\', '/']).any(|p| p == "." || p == "..")
}

impl Request {
    pub fn validate(&self) -> io::Result<()> {
        use polaris_helper_proto::{core_manifest::CoreId, core_protocol::CallerIdentity};
        if !local_path(&self.stage)
            || !local_path(&self.config)
            || CallerIdentity::windows(&self.original_sid).is_none()
            || self.token.len() != 64
            || !self.token.bytes().all(|b| b.is_ascii_hexdigit())
            || [
                &self.core_id,
                &self.manifest_sha256,
                &self.helper_sha256,
                &self.cleaner_sha256,
            ]
            .into_iter()
            .any(|h| CoreId::parse(h).is_err())
        {
            return Err(io::Error::other("invalid native installation request"));
        }
        if self.source_identities.len() > 8 {
            return Err(io::Error::other("too many source objects"));
        }
        Ok(())
    }

    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_REQUEST {
            return Err(io::Error::other("request too large"));
        }
        // Struct deserialization rejects duplicate as well as unknown fields.
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| io::Error::other("invalid request JSON"))?;
        value.validate()?;
        Ok(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub account: String,
    pub command: String,
    pub display: String,
    pub description: String,
    pub owner: String,
    pub group: String,
    pub dacl: String,
    pub service_type: u32,
    pub start_type: u32,
    pub error_control: u32,
    pub tag: u32,
    pub load_group: String,
    pub empty_dependencies: bool,
    pub recovery_reset: u32,
    pub recovery_command: String,
    pub reboot_message: String,
    pub action_types: Vec<u32>,
    pub action_delays: Vec<u32>,
    pub non_crash: bool,
    pub sid_type: u32,
    pub launch_protected: u32,
    pub empty_privileges: bool,
}
impl Policy {
    pub fn supported(&self, command: &str, expected_dacl: &str) -> bool {
        matches!(
            self.account.as_str(),
            "LocalSystem" | "NT AUTHORITY\\SYSTEM"
        ) && self.command == command
            && self.display == crate::SERVICE
            && matches!(self.owner.as_str(), crate::SYSTEM | crate::ADMINS)
            && !self.group.is_empty()
            && self.dacl == expected_dacl
            && self.service_type == 16
            && matches!(self.start_type, 2 | 3)
            && self.error_control == 1
            && self.tag == 0
            && self.load_group.is_empty()
            && self.empty_dependencies
            && self.empty_privileges
            && self.recovery_reset == 86400
            && self.recovery_command.is_empty()
            && self.reboot_message.is_empty()
            && self.action_types == [1, 1, 1]
            && self.action_delays == [5000, 5000, 5000]
            && !self.non_crash
            && self.sid_type == 0
            && self.launch_protected == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Replace,
    Retain,
}

/// These flags are shared with the adapter and persist through every error.
#[derive(Default, Debug)]
pub struct Transaction {
    pub admitted: bool,
    pub mutated: bool,
    pub registration_changed: bool,
    pub create_attempted: bool,
    pub created: bool,
    pub keep_recovery: bool,
}
pub trait Ops {
    fn uncertain(&self) -> bool {
        false
    }
    fn inspect(&mut self) -> io::Result<Option<State>>;
    fn prepare(&mut self) -> io::Result<()>;
    fn install_core(&mut self) -> io::Result<()>;
    fn selection(&mut self) -> io::Result<Selection>;
    fn retained_capability(&mut self, old: Option<State>) -> io::Result<()>;
    fn same_registration(&mut self) -> io::Result<bool>;
    fn retain_probe(&mut self) -> io::Result<bool>;
    fn snapshot(&mut self, replace: bool) -> io::Result<()>;
    /// false is a definite busy/deferred reply; Err may have reserved admission.
    fn admit(&mut self) -> io::Result<bool>;
    fn revalidate(&mut self) -> io::Result<()>;
    fn stop(&mut self) -> io::Result<()>;
    fn register_user(&mut self) -> io::Result<()>;
    fn replace_payload(&mut self) -> io::Result<()>;
    fn create(&mut self) -> io::Result<()>;
    fn configure_fresh(&mut self) -> io::Result<()>;
    fn start(&mut self) -> io::Result<()>;
    fn stop_candidate(&mut self, old: Option<State>, tx: &Transaction) -> io::Result<()>;
    fn restore_payload(&mut self) -> io::Result<()>;
    fn restore_registration(&mut self) -> io::Result<()>;
    fn restore_previous(&mut self, old: Option<State>) -> io::Result<()>;
    fn cancel_admission(&mut self) -> io::Result<()>;
    /// Remove transaction only when recovery was confirmed. Release lock last.
    fn finish(&mut self, keep: bool) -> io::Result<()>;
}

/// The original 20 recovery cases exercise this production controller logic,
/// not a fixture-side copy of the decision to retain/delete the transaction.
pub trait RecoveryOps {
    fn recovery_policy(&mut self) -> io::Result<()>;
    fn recovery_start(&mut self) -> io::Result<()>;
    fn status_begin(&mut self) -> io::Result<()>;
    fn status_wait_running(&mut self) -> io::Result<()>;
    fn status_refresh(&mut self) -> io::Result<Option<State>>;
    fn status_end(&mut self);
    fn final_state(&mut self) -> io::Result<Option<State>>;
}
pub fn restore_service(ops: &mut impl RecoveryOps, old: Option<State>) -> io::Result<()> {
    let Some(required) = old else {
        return Ok(());
    };
    ops.recovery_policy()?;
    if required == State::Running {
        ops.recovery_start()?;
        ops.status_begin()?;
        let controller = (|| -> io::Result<()> {
            ops.status_wait_running()?;
            if ops.status_refresh()? != Some(State::Running) {
                return Err(io::Error::other("rollback running refresh not confirmed"));
            }
            Ok(())
        })();
        ops.status_end(); // Exactly once even when wait/refresh failed.
        controller?;
    }
    if ops.final_state()? != Some(required) {
        return Err(io::Error::other("rollback original state not confirmed"));
    }
    ops.recovery_policy()
}

pub fn run(ops: &mut impl Ops) -> i32 {
    let mut tx = Transaction::default();
    let mut stage = 81;
    let mut old = None;
    let attempt = (|| -> io::Result<()> {
        old = ops.inspect()?; // Before managed mutation, including directories.
        ops.prepare()?;
        stage = 82;
        ops.install_core()?;
        stage = 84;
        let retain = ops.selection()? == Selection::Retain;
        if retain {
            ops.retained_capability(old)?;
            if old == Some(State::Running) && ops.same_registration()? && ops.retain_probe()? {
                return Ok(()); // Still runs production finish below.
            }
        }
        ops.snapshot(!retain)?;
        if old == Some(State::Running) {
            stage = 75;
            // A failed reply may have consumed the reservation. Cancellation is
            // required even when admission's completion could not be observed.
            tx.admitted = true;
            if !ops.admit()? {
                tx.admitted = false;
                return Err(io::Error::other("upgrade deferred"));
            }
        }
        stage = 85;
        ops.revalidate()?;
        tx.mutated = true; // BEFORE stop; no scoped/shadowed rollback flag.
        ops.stop()?;
        stage = 83;
        tx.registration_changed = true;
        ops.register_user()?;
        stage = 86;
        if !retain {
            ops.replace_payload()?;
        }
        if old.is_none() {
            tx.create_attempted = true;
            ops.create()?;
            tx.created = true;
            ops.configure_fresh()?;
        }
        ops.revalidate()?;
        ops.start()?;
        tx.admitted = false;
        Ok(())
    })();
    let mut code = 0;
    if attempt.is_err() {
        code = stage;
        let recovery = (|| -> io::Result<()> {
            if ops.uncertain() {
                return Err(io::Error::other("native child completion unknown"));
            }
            if tx.mutated {
                ops.stop_candidate(old, &tx)?;
                ops.restore_payload()?;
                if tx.registration_changed {
                    ops.restore_registration()?;
                }
                ops.restore_previous(old)?;
            } else if tx.admitted {
                ops.cancel_admission()?;
            }
            Ok(())
        })();
        if recovery.is_err() {
            tx.keep_recovery = true;
            code = 87;
        }
    }
    if ops.finish(tx.keep_recovery).is_err() {
        // RecoveryRequired is sticky; cleanup must not mask it.
        if code != 87 {
            code = 88;
        }
    }
    code
}

#[cfg(test)]
mod tests;
