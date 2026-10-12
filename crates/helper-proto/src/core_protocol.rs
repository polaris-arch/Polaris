//! Content-pinned requests and receipts. No source path or source bytes cross IPC.
use crate::{
    core_manifest::CoreId,
    request::StartParams,
    response::{LinuxBirthStart, NativeBirthStart},
};

/// Identity is supplied by native IPC credentials, never by request arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerIdentity {
    Unix(u32),
    Windows(String),
}
impl CallerIdentity {
    pub fn windows(sid: &str) -> Option<Self> {
        let fields: Vec<_> = sid.split('-').collect();
        if sid.len() > 184
            || !(4..=18).contains(&fields.len())
            || fields[0] != "S"
            || fields[1] != "1"
            || fields[2..].iter().any(|part| {
                part.is_empty()
                    || !part.bytes().all(|byte| byte.is_ascii_digit())
                    || part.parse::<u64>().is_err()
                    || part
                        .parse::<u64>()
                        .is_ok_and(|value| value.to_string() != *part)
            })
            || fields[2]
                .parse::<u64>()
                .is_ok_and(|value| value > 0x0000_ffff_ffff_ffff)
            || fields[3..].iter().any(|part| part.parse::<u32>().is_err())
        {
            return None;
        }
        Some(Self::Windows(sid.into()))
    }
    pub fn key(&self) -> String {
        match self {
            Self::Unix(uid) => format!("u{uid}"),
            Self::Windows(sid) => format!("s{sid}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedStartParams {
    pub id: CoreId,
    pub common: StartParams,
}

impl PinnedStartParams {
    /// id/cfg/log/fwd/[parentPid], with no aliases, duplicate or trailing lines.
    pub fn parse(lines: &[String]) -> Option<Self> {
        if !(4..=5).contains(&lines.len()) {
            return None;
        }
        let id = CoreId::parse(&lines[0]).ok()?;
        let cfg = &lines[1];
        let log = &lines[2];
        if cfg.is_empty()
            || [cfg, log]
                .into_iter()
                .any(|value| value.contains(['\n', '\r', '\0']))
        {
            return None;
        }
        let fwd = match lines[3].as_str() {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        let parent_pid = match lines.get(4).map(String::as_str) {
            None | Some("") => None,
            Some(value) => {
                let pid = value.parse::<std::num::NonZeroU32>().ok()?;
                if pid.to_string() != value {
                    return None;
                }
                Some(pid.get())
            }
        };
        Some(Self {
            id,
            common: StartParams {
                cfg: cfg.clone(),
                log: log.clone(),
                fwd,
                parent_pid,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoreStatus {
    Present { id: CoreId, version: String },
    Absent { id: CoreId },
    Invalid { id: CoreId, reason: String },
}
impl CoreStatus {
    pub fn to_wire(&self) -> String {
        match self {
            Self::Present { id, version } => format!("OK core-present {} {version}", id.as_str()),
            Self::Absent { id } => format!("OK core-absent {}", id.as_str()),
            Self::Invalid { id, reason } => format!("OK core-invalid {} {reason}", id.as_str()),
        }
    }
    pub fn parse(token: &str, rest: &str) -> Option<Self> {
        let parts: Vec<_> = rest.split(' ').collect();
        let id = CoreId::parse(parts.first()?).ok()?;
        let safe_token = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".+_-".contains(&byte))
        };
        match (token, parts.as_slice()) {
            ("core-present", [_, version]) if safe_token(version) => Some(Self::Present {
                id,
                version: (*version).into(),
            }),
            ("core-absent", [_]) => Some(Self::Absent { id }),
            ("core-invalid", [_, reason]) if safe_token(reason) => Some(Self::Invalid {
                id,
                reason: (*reason).into(),
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinnedBirthStart {
    Linux(LinuxBirthStart),
    Native(NativeBirthStart),
}

#[cfg(test)]
mod tests;
