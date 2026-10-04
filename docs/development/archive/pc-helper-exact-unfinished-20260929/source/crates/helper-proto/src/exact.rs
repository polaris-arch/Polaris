//! Managed core commands use separate wire names so an old helper cannot silently run legacy
//! `start` or `stop` semantics. All fields are single-line ASCII tokens.

/// The prepared plan and artifacts that authorize one managed core start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactBinding {
    pub run_ref: String,
    pub plan_id: String,
    pub plan_digest: String,
    pub artifact_digest: String,
    pub config_sha256: String,
    pub core_sha256: String,
}

impl ExactBinding {
    pub const FIELD_COUNT: usize = 6;

    #[must_use]
    pub fn valid(&self) -> bool {
        token(&self.run_ref)
            && token(&self.plan_id)
            && digest(&self.plan_digest)
            && digest(&self.artifact_digest)
            && digest(&self.config_sha256)
            && digest(&self.core_sha256)
    }

    pub fn write_lines(&self, out: &mut Vec<String>) {
        out.extend([
            self.run_ref.clone(),
            self.plan_id.clone(),
            self.plan_digest.clone(),
            self.artifact_digest.clone(),
            self.config_sha256.clone(),
            self.core_sha256.clone(),
        ]);
    }

    #[must_use]
    pub fn parse(fields: &[&str]) -> Option<Self> {
        let [run_ref, plan_id, plan_digest, artifact_digest, config_sha256, core_sha256] = fields
        else {
            return None;
        };
        let result = Self {
            run_ref: (*run_ref).into(),
            plan_id: (*plan_id).into(),
            plan_digest: (*plan_digest).into(),
            artifact_digest: (*artifact_digest).into(),
            config_sha256: (*config_sha256).into(),
            core_sha256: (*core_sha256).into(),
        };
        result.valid().then_some(result)
    }
}

/// A helper-owned child instance. `daemon_birth` changes on daemon restart; `process_birth`
/// comes from the OS, not from the request or PID. This is an observation, not an OS takeover ACK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactReceipt {
    pub binding: ExactBinding,
    /// Digest of the bytes actually passed to the core after helper-side path relocation.
    pub launched_config_sha256: String,
    /// Versioned digest of every helper-owned input object used for this launch.
    pub launch_closure_digest: String,
    pub daemon_birth: String,
    pub pid: u32,
    pub process_birth: String,
}

impl ExactReceipt {
    pub const FIELD_COUNT: usize = ExactBinding::FIELD_COUNT + 5;

    #[must_use]
    pub fn valid(&self) -> bool {
        self.binding.valid()
            && digest(&self.launched_config_sha256)
            && digest(&self.launch_closure_digest)
            && token(&self.daemon_birth)
            && self.pid > 0
            && token(&self.process_birth)
    }

    pub fn write_lines(&self, out: &mut Vec<String>) {
        self.binding.write_lines(out);
        out.extend([
            self.launched_config_sha256.clone(),
            self.launch_closure_digest.clone(),
            self.daemon_birth.clone(),
            self.pid.to_string(),
            self.process_birth.clone(),
        ]);
    }

    #[must_use]
    pub fn parse(fields: &[&str]) -> Option<Self> {
        if fields.len() != Self::FIELD_COUNT {
            return None;
        }
        let binding = ExactBinding::parse(&fields[..ExactBinding::FIELD_COUNT])?;
        let result = Self {
            binding,
            launched_config_sha256: fields[6].into(),
            launch_closure_digest: fields[7].into(),
            daemon_birth: fields[8].into(),
            pid: fields[9].parse().ok()?,
            process_birth: fields[10].into(),
        };
        result.valid().then_some(result)
    }

    #[must_use]
    pub fn to_wire_fields(&self) -> String {
        let mut fields = Vec::with_capacity(Self::FIELD_COUNT);
        self.write_lines(&mut fields);
        fields.join(" ")
    }
}

/// A missing child slot alone cannot prove exit. Exact stop is only a stop request until a
/// separate OS process-instance observation returns `Exited`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExactResult {
    Started(ExactReceipt),
    Running(ExactReceipt),
    StopRequested(ExactReceipt),
    /// Same daemon re-observed that the requested OS process instance no longer exists.
    Exited(ExactReceipt),
    Mismatch,
    Unknown,
}

#[must_use]
pub fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[must_use]
pub fn token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_receipt_rejects_missing_birth_and_bad_digests() {
        let binding = ExactBinding {
            run_ref: "run-1".into(),
            plan_id: "plan-1".into(),
            plan_digest: "a".repeat(64),
            artifact_digest: "b".repeat(64),
            config_sha256: "c".repeat(64),
            core_sha256: "d".repeat(64),
        };
        let receipt = ExactReceipt {
            binding: binding.clone(),
            launched_config_sha256: "e".repeat(64),
            launch_closure_digest: "f".repeat(64),
            daemon_birth: "daemon-1".into(),
            pid: 42,
            process_birth: "12345".into(),
        };
        assert_eq!(
            ExactReceipt::parse(
                &receipt
                    .to_wire_fields()
                    .split_whitespace()
                    .collect::<Vec<_>>()
            ),
            Some(receipt.clone())
        );
        assert!(!ExactReceipt {
            daemon_birth: String::new(),
            ..receipt.clone()
        }
        .valid());
        assert!(!ExactReceipt {
            launch_closure_digest: "bad".into(),
            ..receipt
        }
        .valid());
        assert!(!ExactBinding {
            plan_digest: "bad".into(),
            ..binding
        }
        .valid());
    }

    #[test]
    fn exact_wire_keeps_optional_parent_pid_slot_and_rejects_malformed_receipts() {
        use crate::{ExactStartParams, Request, Response, ResponseKind, StartParams};
        let binding = ExactBinding {
            run_ref: "run-1".into(),
            plan_id: "plan-1".into(),
            plan_digest: "a".repeat(64),
            artifact_digest: "b".repeat(64),
            config_sha256: "c".repeat(64),
            core_sha256: "d".repeat(64),
        };
        let request = Request::StartExact(ExactStartParams {
            common: StartParams {
                cfg: "/tmp/config.json".into(),
                log: String::new(),
                fwd: false,
                parent_pid: None,
            },
            binding: binding.clone(),
        });
        assert_eq!(request.command_name(), "start-exact");
        let lines = request.args_lines();
        assert_eq!(
            lines[3], "",
            "missing parent PID must occupy its own wire line"
        );
        assert_eq!(lines[4], binding.run_ref);
        let receipt = ExactReceipt {
            binding,
            launched_config_sha256: "e".repeat(64),
            launch_closure_digest: "f".repeat(64),
            daemon_birth: "daemon-1".into(),
            pid: 42,
            process_birth: "12345".into(),
        };
        for result in [
            ExactResult::Started(receipt.clone()),
            ExactResult::Running(receipt.clone()),
            ExactResult::StopRequested(receipt.clone()),
            ExactResult::Exited(receipt.clone()),
        ] {
            let response = Response::Ok(ResponseKind::Exact(result));
            assert_eq!(Response::parse(&response.to_wire_line()), response);
        }
        assert_eq!(
            Response::parse("OK exact-started 42"),
            Response::Ok(ResponseKind::Exact(ExactResult::Unknown)),
        );
        assert_ne!(Request::StopExact(receipt).command_name(), "stop");
    }
}
