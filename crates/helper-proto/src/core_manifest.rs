//! Platform-independent core identity and strict installation JSON.
//! App/helper hash the exact same framed bytes using their existing SHA256 implementation.
use crate::{core_payload, Platform};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoreId(String);

impl CoreId {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("core-id-invalid");
        }
        Ok(Self(value.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoreFile {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreManifest {
    pub platform: String,
    pub version: String,
    pub provider_fingerprint: String,
    pub files: Vec<CoreFile>,
    pub helper_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InstalledCoreManifest {
    pub schema: String,
    pub id: String,
    pub app_version: String,
    pub core: CoreManifest,
}

impl InstalledCoreManifest {
    pub fn parse(raw: &[u8], sha256: impl FnOnce(&[u8]) -> String) -> Result<Self, &'static str> {
        if raw.len() > 64 * 1024 {
            return Err("core-manifest-too-large");
        }
        let value: Self = serde_json::from_slice(raw).map_err(|_| "core-manifest-invalid")?;
        value.validate(sha256)?;
        Ok(value)
    }
    pub fn validate(&self, sha256: impl FnOnce(&[u8]) -> String) -> Result<(), &'static str> {
        if self.schema != "polaris-core-manifest-v1"
            || self.app_version.is_empty()
            || self.app_version.len() > 128
            || !self
                .app_version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
        {
            return Err("core-manifest-invalid");
        }
        let declared = CoreId::parse(&self.id)?;
        if self.core.id(sha256)? != declared {
            return Err("core-id-mismatch");
        }
        Ok(())
    }
    pub fn to_json(&self) -> Result<Vec<u8>, &'static str> {
        serde_json::to_vec(self).map_err(|_| "core-manifest-invalid")
    }
}

impl CoreManifest {
    pub fn validate(&self) -> Result<(), &'static str> {
        let platform = match self.platform.as_str() {
            "linux" => Platform::Linux,
            "win" => Platform::Win,
            "mac-x64" | "mac-arm64" => Platform::Mac,
            _ => return Err("core-platform-invalid"),
        };
        if self.version.is_empty()
            || self.version.len() > 128
            || !self
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-+".contains(&b))
        {
            return Err("core-version-invalid");
        }
        CoreId::parse(&self.provider_fingerprint)?;
        if let Some(hash) = &self.helper_sha256 {
            CoreId::parse(hash)?;
        }
        let mut names = std::collections::BTreeSet::new();
        for file in &self.files {
            if !core_payload::name_allowed(&file.name, platform)
                || !names.insert(file.name.as_str())
                || file.bytes == 0
                || file.bytes > 512 * 1024 * 1024
            {
                return Err("core-files-invalid");
            }
            CoreId::parse(&file.sha256)?;
        }
        let required = 1 + usize::from(core_payload::sidecar_filename(platform).is_some());
        if names.len() != required
            || !names.contains(core_payload::core_filename(platform))
            || core_payload::sidecar_filename(platform).is_some_and(|name| !names.contains(name))
        {
            return Err("core-files-incomplete");
        }
        Ok(())
    }

    /// Helper/appVersion are installation metadata, not the kernel content identity.
    /// Every variable field is length-framed, and files are sorted by exact name.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, &'static str> {
        self.validate()?;
        fn field(out: &mut Vec<u8>, value: &[u8]) {
            out.extend_from_slice(&(value.len() as u64).to_be_bytes());
            out.extend_from_slice(value);
        }
        let mut out = b"polaris-core-manifest-v1\0".to_vec();
        field(&mut out, self.platform.as_bytes());
        field(&mut out, self.version.as_bytes());
        field(&mut out, self.provider_fingerprint.as_bytes());
        let mut files: Vec<_> = self.files.iter().collect();
        files.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend_from_slice(&(files.len() as u64).to_be_bytes());
        for file in files {
            field(&mut out, file.name.as_bytes());
            out.extend_from_slice(&file.bytes.to_be_bytes());
            field(&mut out, file.sha256.as_bytes());
        }
        Ok(out)
    }

    pub fn id(&self, sha256: impl FnOnce(&[u8]) -> String) -> Result<CoreId, &'static str> {
        CoreId::parse(&sha256(&self.canonical_bytes()?))
    }
}

#[cfg(test)]
mod tests;
