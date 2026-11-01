//! Closed Helper CheckConfig transport. Correlations name a registry row, never privilege.
use crate::{core_manifest::CoreId, HelperBirthToken};

pub const COMMAND: &str = "check-config-custody-v1";
pub const MAX_CONFIG_BYTES: usize = 8 * 1024 * 1024;
pub const CHUNK_BYTES: usize = 64 * 1024;
pub const OUTPUT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckKey {
    pub core: CoreId,
    pub correlation: HelperBirthToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}
impl OutputStream {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "stdout" => Some(Self::Stdout),
            "stderr" => Some(Self::Stderr),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckRequest {
    Begin {
        core: CoreId,
        bytes: usize,
        sha256: [u8; 32],
    },
    Append {
        key: CheckKey,
        offset: usize,
        bytes: Vec<u8>,
    },
    Commit {
        key: CheckKey,
    },
    Status {
        key: CheckKey,
    },
    ReadOutput {
        key: CheckKey,
        stream: OutputStream,
        offset: usize,
    },
    Cancel {
        key: CheckKey,
    },
    Release {
        key: CheckKey,
    },
}

fn number(s: &str) -> Option<usize> {
    let n = s.parse::<usize>().ok()?;
    (n.to_string() == s).then_some(n)
}
pub fn hex(bytes: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(H[(b >> 4) as usize] as char);
        s.push(H[(b & 15) as usize] as char);
    }
    s
}
pub fn unhex(s: &str, limit: usize) -> Option<Vec<u8>> {
    if s.len() > limit.checked_mul(2)? || !s.len().is_multiple_of(2) {
        return None;
    }
    let d = |b| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| Some((d(p[0])? << 4) | d(p[1])?))
        .collect()
}
impl CheckRequest {
    pub fn key(&self) -> Option<&CheckKey> {
        match self {
            Self::Begin { .. } => None,
            Self::Append { key, .. }
            | Self::Commit { key }
            | Self::Status { key }
            | Self::ReadOutput { key, .. }
            | Self::Cancel { key }
            | Self::Release { key } => Some(key),
        }
    }
    pub fn core(&self) -> &CoreId {
        match self {
            Self::Begin { core, .. } => core,
            _ => &self.key().expect("non-Begin has key").core,
        }
    }
    pub fn write_args(&self, out: &mut Vec<String>) {
        let op = match self {
            Self::Begin { .. } => "begin",
            Self::Append { .. } => "append",
            Self::Commit { .. } => "commit",
            Self::Status { .. } => "status",
            Self::ReadOutput { .. } => "read",
            Self::Cancel { .. } => "cancel",
            Self::Release { .. } => "release",
        };
        out.push(op.into());
        out.push(self.core().as_str().into());
        if let Some(key) = self.key() {
            out.push(key.correlation.to_wire());
        }
        match self {
            Self::Begin { bytes, sha256, .. } => {
                out.push(bytes.to_string());
                out.push(hex(sha256));
            }
            Self::Append { offset, bytes, .. } => {
                out.push(offset.to_string());
                out.push(hex(bytes));
            }
            Self::ReadOutput { stream, offset, .. } => {
                out.push(stream.wire().into());
                out.push(offset.to_string());
            }
            _ => {}
        }
    }
    /// Exact argument cardinality, canonical numbers/hex, and bounded chunks.
    pub fn parse(args: &[String]) -> Option<Self> {
        let (op, rest) = args.split_first()?;
        let core = CoreId::parse(rest.first()?).ok()?;
        if op == "begin" {
            let [_, len, digest] = rest else { return None };
            let bytes = number(len)?;
            if bytes == 0 || bytes > MAX_CONFIG_BYTES {
                return None;
            }
            return Some(Self::Begin {
                core,
                bytes,
                sha256: unhex(digest, 32)?.try_into().ok()?,
            });
        }
        let key = CheckKey {
            core,
            correlation: HelperBirthToken::parse_wire(rest.get(1)?)?,
        };
        match (op.as_str(), rest) {
            ("append", [_, _, offset, data]) => {
                let bytes = unhex(data, CHUNK_BYTES)?;
                if bytes.is_empty() {
                    return None;
                }
                let offset = number(offset)?;
                if offset.checked_add(bytes.len())? > MAX_CONFIG_BYTES {
                    return None;
                }
                Some(Self::Append { key, offset, bytes })
            }
            ("commit", [_, _]) => Some(Self::Commit { key }),
            ("status", [_, _]) => Some(Self::Status { key }),
            ("read", [_, _, stream, offset]) => {
                let offset = number(offset)?;
                if offset > OUTPUT_BYTES {
                    return None;
                }
                Some(Self::ReadOutput {
                    key,
                    stream: OutputStream::parse(stream)?,
                    offset,
                })
            }
            ("cancel", [_, _]) => Some(Self::Cancel { key }),
            ("release", [_, _]) => Some(Self::Release { key }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Receiving,
    Running,
    Done,
    Rejected,
    TimedOut,
    Cancelled,
    OutputFailed,
    SpawnFailed,
    Unknown,
}
impl CheckStatus {
    pub fn wire(self) -> &'static str {
        match self {
            Self::Receiving => "receiving",
            Self::Running => "running",
            Self::Done => "done",
            Self::Rejected => "rejected",
            Self::TimedOut => "timed-out",
            Self::Cancelled => "cancelled",
            Self::OutputFailed => "output-failed",
            Self::SpawnFailed => "spawn-failed",
            Self::Unknown => "unknown",
        }
    }
    pub fn terminal(self) -> bool {
        !matches!(self, Self::Receiving | Self::Running | Self::Unknown)
    }
    fn parse(s: &str) -> Option<Self> {
        [
            Self::Receiving,
            Self::Running,
            Self::Done,
            Self::Rejected,
            Self::TimedOut,
            Self::Cancelled,
            Self::OutputFailed,
            Self::SpawnFailed,
            Self::Unknown,
        ]
        .into_iter()
        .find(|v| v.wire() == s)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckReply {
    Begun {
        key: CheckKey,
    },
    Appended {
        key: CheckKey,
        received: usize,
    },
    Status {
        key: CheckKey,
        status: CheckStatus,
        stdout: usize,
        stderr: usize,
    },
    Output {
        key: CheckKey,
        stream: OutputStream,
        offset: usize,
        bytes: Vec<u8>,
    },
    Released {
        key: CheckKey,
    },
}
impl CheckReply {
    pub fn key(&self) -> &CheckKey {
        match self {
            Self::Begun { key }
            | Self::Appended { key, .. }
            | Self::Status { key, .. }
            | Self::Output { key, .. }
            | Self::Released { key } => key,
        }
    }
    pub fn to_wire(&self) -> String {
        let k = self.key();
        let key = format!("{} {}", k.core.as_str(), k.correlation.to_wire());
        match self {
            Self::Begun { .. } => format!("OK check-config begun {key}"),
            Self::Appended { received, .. } => format!("OK check-config appended {key} {received}"),
            Self::Status {
                status,
                stdout,
                stderr,
                ..
            } => format!(
                "OK check-config status {key} {} {stdout} {stderr}",
                status.wire()
            ),
            Self::Output {
                stream,
                offset,
                bytes,
                ..
            } => format!(
                "OK check-config output {key} {} {offset} {}",
                stream.wire(),
                if bytes.is_empty() {
                    "-".into()
                } else {
                    hex(bytes)
                }
            ),
            Self::Released { .. } => format!("OK check-config released {key}"),
        }
    }
    pub fn parse(line: &str) -> Option<Self> {
        let a: Vec<_> = line.split(' ').collect();
        if a.len() < 5 || a[0] != "OK" || a[1] != "check-config" {
            return None;
        }
        let key = CheckKey {
            core: CoreId::parse(a[3]).ok()?,
            correlation: HelperBirthToken::parse_wire(a[4])?,
        };
        let r = match (a[2], &a[5..]) {
            ("begun", []) => Self::Begun { key },
            ("appended", [n]) => {
                let n = number(n)?;
                if n > MAX_CONFIG_BYTES {
                    return None;
                }
                Self::Appended { key, received: n }
            }
            ("status", [s, o, e]) => {
                let (stdout, stderr) = (number(o)?, number(e)?);
                if stdout > OUTPUT_BYTES || stderr > OUTPUT_BYTES {
                    return None;
                }
                Self::Status {
                    key,
                    status: CheckStatus::parse(s)?,
                    stdout,
                    stderr,
                }
            }
            ("output", [s, o, b]) => {
                let offset = number(o)?;
                let bytes = if *b == "-" {
                    Vec::new()
                } else {
                    let bytes = unhex(b, CHUNK_BYTES)?;
                    if bytes.is_empty() {
                        return None;
                    }
                    bytes
                };
                if offset.checked_add(bytes.len())? > OUTPUT_BYTES {
                    return None;
                }
                Self::Output {
                    key,
                    stream: OutputStream::parse(s)?,
                    offset,
                    bytes,
                }
            }
            ("released", []) => Self::Released { key },
            _ => return None,
        };
        (r.to_wire() == line).then_some(r)
    }
}

#[cfg(test)]
mod tests;
