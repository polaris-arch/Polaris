//! Standalone, metadata-only preparation capsule. This is not a Start wire
//! command, admission, authenticated identity, or consumed-closure witness.

use crate::Platform;

pub const CAPSULE_VERSION: u16 = 1;
pub const CAPABILITY_VERSION: u16 = 1;
pub const MAX_CAPSULE_BYTES: usize = 4096;
pub const MAX_CONFIG_BYTES: u32 = 8 * 1024 * 1024;
pub const MAX_BINARY_BYTES: u32 = 96 * 1024 * 1024;
pub const MAX_IMMUTABLE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_RULES: u16 = 512;
const MAGIC: &[u8] = b"polaris-exact-start-capsule\0";
const CLOSURE_DOMAIN: &[u8] = b"polaris-exact-immutable-closure-v1\0";

/// Check, fixture probe, and a future production run are separate capabilities.
/// There is deliberately no production-run profile in this preparation ABI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionProfile {
    LinuxB609PlainTcpCheckV1,
    LinuxNoNetworkFixtureV1,
}

impl ExecutionProfile {
    const fn code(self) -> u16 {
        match self {
            Self::LinuxB609PlainTcpCheckV1 => 1,
            Self::LinuxNoNetworkFixtureV1 => 2,
        }
    }

    fn from_code(code: u16) -> Result<Self, CapsuleError> {
        match code {
            1 => Ok(Self::LinuxB609PlainTcpCheckV1),
            2 => Ok(Self::LinuxNoNetworkFixtureV1),
            _ => Err(CapsuleError::UnsupportedProfile),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExactStartCapability {
    Unsupported,
}

/// Neither new nor old helpers have a production exact-start consumer yet.
/// This says nothing about their existing ordinary/native-birth commands.
#[must_use]
pub const fn exact_start_capability(_platform: Platform) -> ExactStartCapability {
    ExactStartCapability::Unsupported
}

/// Non-secret declarations only. Actual resource consumers must recompute
/// digests and budgets from the bytes they protect, not trust this metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactStartMetadata {
    pub nonce: [u8; 32],
    pub subject_uid: u32,
    pub plan_id: String,
    pub plan_digest: [u8; 32],
    pub candidate_run_id: String,
    pub config_digest: [u8; 32],
    pub binary_digest: [u8; 32],
    pub manifest_ref: String,
    pub manifest_digest: [u8; 32],
    pub closure_digest: [u8; 32],
    pub profile: ExecutionProfile,
    pub config_bytes: u32,
    pub binary_bytes: u32,
    pub rule_count: u16,
    pub immutable_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapsuleError {
    ResourceBudget,
    InvalidToken,
    InvalidManifest,
    InvalidRulePath,
    DuplicateRule,
    Truncated,
    TrailingBytes,
    InvalidMagic,
    UnsupportedVersion,
    UnsupportedProfile,
    SubjectMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExactStartCapsule {
    metadata: ExactStartMetadata,
}

fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

impl ExactStartMetadata {
    pub fn validate(&self) -> Result<(), CapsuleError> {
        if !safe_token(&self.plan_id) || !safe_token(&self.candidate_run_id) {
            return Err(CapsuleError::InvalidToken);
        }
        if self.manifest_ref.len() > 512
            || self.manifest_ref != format!("mesh-routes/plans/{}/manifest.json", self.plan_id)
        {
            return Err(CapsuleError::InvalidManifest);
        }
        let minimum = u64::from(self.config_bytes)
            .checked_add(u64::from(self.binary_bytes))
            .ok_or(CapsuleError::ResourceBudget)?;
        if self.config_bytes == 0
            || self.config_bytes > MAX_CONFIG_BYTES
            || self.binary_bytes == 0
            || self.binary_bytes > MAX_BINARY_BYTES
            || self.rule_count > MAX_RULES
            || self.immutable_bytes < minimum
            || self.immutable_bytes > MAX_IMMUTABLE_BYTES
        {
            return Err(CapsuleError::ResourceBudget);
        }
        Ok(())
    }
}

fn put_string(out: &mut Vec<u8>, value: &str) {
    // Every string is bounded by validate(), well below u16::MAX.
    out.extend_from_slice(&(value.len() as u16).to_be_bytes());
    out.extend_from_slice(value.as_bytes());
}

impl ExactStartCapsule {
    pub fn new(metadata: ExactStartMetadata) -> Result<Self, CapsuleError> {
        metadata.validate()?;
        Ok(Self { metadata })
    }

    #[must_use]
    pub fn metadata(&self) -> &ExactStartMetadata {
        &self.metadata
    }

    /// Fixed field order; never used by the legacy line protocol.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let m = &self.metadata;
        let mut out = Vec::with_capacity(512);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&CAPSULE_VERSION.to_be_bytes());
        out.extend_from_slice(&CAPABILITY_VERSION.to_be_bytes());
        out.extend_from_slice(&m.nonce);
        out.extend_from_slice(&m.subject_uid.to_be_bytes());
        put_string(&mut out, &m.plan_id);
        out.extend_from_slice(&m.plan_digest);
        put_string(&mut out, &m.candidate_run_id);
        out.extend_from_slice(&m.config_digest);
        out.extend_from_slice(&m.binary_digest);
        put_string(&mut out, &m.manifest_ref);
        out.extend_from_slice(&m.manifest_digest);
        out.extend_from_slice(&m.closure_digest);
        out.extend_from_slice(&m.profile.code().to_be_bytes());
        out.extend_from_slice(&m.config_bytes.to_be_bytes());
        out.extend_from_slice(&m.binary_bytes.to_be_bytes());
        out.extend_from_slice(&m.rule_count.to_be_bytes());
        out.extend_from_slice(&m.immutable_bytes.to_be_bytes());
        debug_assert!(out.len() <= MAX_CAPSULE_BYTES);
        out
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CapsuleError> {
        if bytes.len() > MAX_CAPSULE_BYTES {
            return Err(CapsuleError::ResourceBudget);
        }
        let mut input = Input(bytes);
        if input.take(MAGIC.len())? != MAGIC {
            return Err(CapsuleError::InvalidMagic);
        }
        if input.u16()? != CAPSULE_VERSION || input.u16()? != CAPABILITY_VERSION {
            return Err(CapsuleError::UnsupportedVersion);
        }
        let metadata = ExactStartMetadata {
            nonce: input.array()?,
            subject_uid: input.u32()?,
            plan_id: input.string(128)?,
            plan_digest: input.array()?,
            candidate_run_id: input.string(128)?,
            config_digest: input.array()?,
            binary_digest: input.array()?,
            manifest_ref: input.string(512)?,
            manifest_digest: input.array()?,
            closure_digest: input.array()?,
            profile: ExecutionProfile::from_code(input.u16()?)?,
            config_bytes: input.u32()?,
            binary_bytes: input.u32()?,
            rule_count: input.u16()?,
            immutable_bytes: u64::from_be_bytes(input.array()?),
        };
        if !input.0.is_empty() {
            return Err(CapsuleError::TrailingBytes);
        }
        Self::new(metadata)
    }

    /// Consistency only: a matching tuple does not establish authentication,
    /// nonce freshness, admission, resource consumption, or an actual birth.
    pub fn validate_against(&self, expected: &ExactStartMetadata) -> Result<(), CapsuleError> {
        expected.validate()?;
        if self.metadata != *expected {
            return Err(CapsuleError::SubjectMismatch);
        }
        Ok(())
    }
}

struct Input<'a>(&'a [u8]);

impl<'a> Input<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], CapsuleError> {
        let (value, rest) = self
            .0
            .split_at_checked(len)
            .ok_or(CapsuleError::Truncated)?;
        self.0 = rest;
        Ok(value)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], CapsuleError> {
        self.take(N)?
            .try_into()
            .map_err(|_| CapsuleError::Truncated)
    }

    fn u16(&mut self) -> Result<u16, CapsuleError> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    fn u32(&mut self) -> Result<u32, CapsuleError> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    fn string(&mut self, max: usize) -> Result<String, CapsuleError> {
        let len = usize::from(self.u16()?);
        if len > max {
            return Err(CapsuleError::ResourceBudget);
        }
        let value = self.take(len)?;
        if !value.is_ascii() {
            return Err(CapsuleError::InvalidToken);
        }
        String::from_utf8(value.to_vec()).map_err(|_| CapsuleError::InvalidToken)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImmutableRuleMetadata {
    pub relative_path: String,
    pub bytes: u64,
    pub digest: [u8; 32],
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 512
        && path.is_ascii()
        && path.split('/').all(|part| {
            safe_token(part)
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

/// Canonical domain-separated hash input for config plus immutable rules.
/// Callers hash these bytes with their existing SHA256 implementation. Source
/// paths, mutable files and the separately bound executable are not this closure.
pub fn closure_identity_bytes(
    config_digest: [u8; 32],
    config_bytes: u32,
    rules: &[ImmutableRuleMetadata],
) -> Result<Vec<u8>, CapsuleError> {
    if config_bytes == 0 || config_bytes > MAX_CONFIG_BYTES || rules.len() > usize::from(MAX_RULES)
    {
        return Err(CapsuleError::ResourceBudget);
    }
    let mut sorted: Vec<_> = rules.iter().collect();
    sorted.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    let mut total = u64::from(config_bytes);
    let mut previous: Option<&str> = None;
    let mut out = CLOSURE_DOMAIN.to_vec();
    out.extend_from_slice(&config_bytes.to_be_bytes());
    out.extend_from_slice(&config_digest);
    out.extend_from_slice(&(sorted.len() as u16).to_be_bytes());
    for rule in sorted {
        if !safe_relative_path(&rule.relative_path) {
            return Err(CapsuleError::InvalidRulePath);
        }
        if previous == Some(rule.relative_path.as_str()) {
            return Err(CapsuleError::DuplicateRule);
        }
        total = total
            .checked_add(rule.bytes)
            .ok_or(CapsuleError::ResourceBudget)?;
        if rule.bytes == 0 || total > MAX_IMMUTABLE_BYTES {
            return Err(CapsuleError::ResourceBudget);
        }
        put_string(&mut out, &rule.relative_path);
        out.extend_from_slice(&rule.bytes.to_be_bytes());
        out.extend_from_slice(&rule.digest);
        previous = Some(&rule.relative_path);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
