//! macOS/Windows native child receipts. Linux keeps its separate wire family.

use super::{ResponseKind, StartTiming, hex_encode, parse_birth_start_timing};
use crate::HelperBirthTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBirthStart {
    Started {
        target: HelperBirthTarget,
        timing: Option<StartTiming>,
        created: Option<u64>,
    },
    Already {
        target: HelperBirthTarget,
    },
    NotAdmittedPending {
        target: HelperBirthTarget,
    },
    NotAdmittedUnknown {
        target: Option<HelperBirthTarget>,
    },
}

/// Empty is only this helper's current view. It cannot certify an old birth stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeBirthStatus {
    Running {
        target: HelperBirthTarget,
        created: Option<u64>,
        image: Option<String>,
    },
    Stopping {
        target: HelperBirthTarget,
    },
    /// `None` covers legacy custody that cannot safely be assigned a fresh birth.
    Unknown {
        target: Option<HelperBirthTarget>,
    },
    Empty,
}

/// Stopped requires native wait success and completion of that birth's platform tail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeBirthStop {
    Stopped {
        target: HelperBirthTarget,
    },
    Pending {
        target: HelperBirthTarget,
    },
    Unknown {
        target: HelperBirthTarget,
    },
    Mismatch {
        requested: HelperBirthTarget,
        current: HelperBirthTarget,
    },
}

fn pair(target: HelperBirthTarget) -> String {
    format!("{} {}", target.pid, target.birth.to_wire())
}

pub(super) fn to_wire(kind: &ResponseKind) -> String {
    match kind {
        ResponseKind::NativeBirthStart(NativeBirthStart::Started {
            target,
            timing,
            created,
        }) => {
            let mut line = format!("OK native-birth-started {}", pair(*target));
            if let Some(t) = timing {
                line.push_str(&format!(
                    " forwarding_ms={} process_ms={} job_ms={} log_handoff_ms={} total_ms={}",
                    t.forwarding_ms, t.process_ms, t.job_ms, t.log_handoff_ms, t.total_ms
                ));
            }
            if let Some(created) = created {
                line.push_str(&format!(" created={created}"));
            }
            line
        }
        ResponseKind::NativeBirthStart(NativeBirthStart::Already { target }) => {
            format!("OK native-birth-already {}", pair(*target))
        }
        ResponseKind::NativeBirthStart(NativeBirthStart::NotAdmittedPending { target }) => format!(
            "OK native-birth-start-not-admitted pending {}",
            pair(*target)
        ),
        ResponseKind::NativeBirthStart(NativeBirthStart::NotAdmittedUnknown { target }) => {
            optional_target("native-birth-start-not-admitted unknown", *target)
        }
        ResponseKind::NativeBirthStatus(NativeBirthStatus::Running {
            target,
            created,
            image,
        }) => {
            let mut line = format!("OK native-birth-status running {}", pair(*target));
            if let Some(created) = created {
                line.push_str(&format!(" created={created}"));
            }
            if let Some(image) = image {
                line.push_str(&format!(" image={}", hex_encode(image.as_bytes())));
            }
            line
        }
        ResponseKind::NativeBirthStatus(NativeBirthStatus::Stopping { target }) => {
            format!("OK native-birth-status stopping {}", pair(*target))
        }
        ResponseKind::NativeBirthStatus(NativeBirthStatus::Unknown { target }) => {
            optional_target("native-birth-status unknown", *target)
        }
        ResponseKind::NativeBirthStatus(NativeBirthStatus::Empty) => {
            "OK native-birth-status empty".into()
        }
        ResponseKind::NativeBirthStop(NativeBirthStop::Stopped { target }) => {
            format!("OK native-birth-stopped {}", pair(*target))
        }
        ResponseKind::NativeBirthStop(NativeBirthStop::Pending { target }) => {
            format!("OK native-birth-stop-pending {}", pair(*target))
        }
        ResponseKind::NativeBirthStop(NativeBirthStop::Unknown { target }) => {
            format!("OK native-birth-stop-unknown {}", pair(*target))
        }
        ResponseKind::NativeBirthStop(NativeBirthStop::Mismatch { requested, current }) => format!(
            "OK native-birth-stop-mismatch {} {}",
            pair(*requested),
            pair(*current)
        ),
        _ => unreachable!("native response family only"),
    }
}

fn optional_target(prefix: &str, target: Option<HelperBirthTarget>) -> String {
    target.map_or_else(
        || format!("OK {prefix}"),
        |t| format!("OK {prefix} {}", pair(t)),
    )
}

fn target(fields: &[&str]) -> Option<HelperBirthTarget> {
    let [pid, birth] = fields else {
        return None;
    };
    HelperBirthTarget::parse_wire(pid, birth)
}

fn created(field: &str) -> Option<u64> {
    let value = field.strip_prefix("created=")?;
    let parsed: u64 = value.parse().ok()?;
    (parsed.to_string() == value).then_some(parsed)
}

pub(super) fn parse(token: &str, tail: &str) -> Option<ResponseKind> {
    let fields: Vec<_> = tail.split_whitespace().collect();
    Some(match token {
        "native-birth-started" => {
            let [pid, birth, rest @ ..] = fields.as_slice() else {
                return None;
            };
            let target = HelperBirthTarget::parse_wire(pid, birth)?;
            let (metrics, created) = match rest.last() {
                Some(last) if last.starts_with("created=") => {
                    (&rest[..rest.len() - 1], Some(created(last)?))
                }
                _ => (rest, None),
            };
            let timing = if metrics.is_empty() {
                None
            } else {
                Some(parse_birth_start_timing(metrics)?)
            };
            ResponseKind::NativeBirthStart(NativeBirthStart::Started {
                target,
                timing,
                created,
            })
        }
        "native-birth-already" => ResponseKind::NativeBirthStart(NativeBirthStart::Already {
            target: target(&fields)?,
        }),
        "native-birth-start-not-admitted" => {
            ResponseKind::NativeBirthStart(match fields.as_slice() {
                ["pending", rest @ ..] => NativeBirthStart::NotAdmittedPending {
                    target: target(rest)?,
                },
                ["unknown"] => NativeBirthStart::NotAdmittedUnknown { target: None },
                ["unknown", rest @ ..] => NativeBirthStart::NotAdmittedUnknown {
                    target: Some(target(rest)?),
                },
                _ => return None,
            })
        }
        "native-birth-status" => ResponseKind::NativeBirthStatus(match fields.as_slice() {
            ["empty"] => NativeBirthStatus::Empty,
            ["running", pid, birth, rest @ ..] => {
                let target = HelperBirthTarget::parse_wire(pid, birth)?;
                let (created, image) = match rest {
                    [] => (None, None),
                    [c] if c.starts_with("created=") => (Some(created(c)?), None),
                    [i] if i.starts_with("image=") => (None, Some(decode_image(i)?)),
                    [c, i] => (Some(created(c)?), Some(decode_image(i)?)),
                    _ => return None,
                };
                NativeBirthStatus::Running {
                    target,
                    created,
                    image,
                }
            }
            ["stopping", rest @ ..] => NativeBirthStatus::Stopping {
                target: target(rest)?,
            },
            ["unknown"] => NativeBirthStatus::Unknown { target: None },
            ["unknown", rest @ ..] => NativeBirthStatus::Unknown {
                target: Some(target(rest)?),
            },
            _ => return None,
        }),
        "native-birth-stopped" => ResponseKind::NativeBirthStop(NativeBirthStop::Stopped {
            target: target(&fields)?,
        }),
        "native-birth-stop-pending" => ResponseKind::NativeBirthStop(NativeBirthStop::Pending {
            target: target(&fields)?,
        }),
        "native-birth-stop-unknown" => ResponseKind::NativeBirthStop(NativeBirthStop::Unknown {
            target: target(&fields)?,
        }),
        "native-birth-stop-mismatch" => {
            let [pid, birth, current_pid, current_birth] = fields.as_slice() else {
                return None;
            };
            ResponseKind::NativeBirthStop(NativeBirthStop::Mismatch {
                requested: HelperBirthTarget::parse_wire(pid, birth)?,
                current: HelperBirthTarget::parse_wire(current_pid, current_birth)?,
            })
        }
        _ => return None,
    })
}

fn decode_image(field: &str) -> Option<String> {
    let hex = field.strip_prefix("image=")?;
    let decoded = super::hex_decode(hex)?;
    String::from_utf8(decoded).ok()
}
