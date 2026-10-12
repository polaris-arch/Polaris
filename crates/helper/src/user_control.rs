//! Administrative registration per native caller. A request never chooses this key.
use crate::{core_objects::PayloadDir, core_store::OwnerPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePeer {
    pub identity: CallerIdentity,
    pub administrator: bool,
}
use polaris_helper_proto::core_protocol::CallerIdentity;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

fn read_record(
    support: &Path,
    caller: &CallerIdentity,
    name: &str,
    maximum: u64,
) -> io::Result<String> {
    if let CallerIdentity::Windows(sid) = caller {
        if CallerIdentity::windows(sid).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid native SID",
            ));
        }
    }
    let directory = support.join("users").join(caller.key());
    let policy = OwnerPolicy::privileged();
    for path in [
        support.to_path_buf(),
        support.join("users"),
        directory.clone(),
        directory.join(name),
    ] {
        policy.check(&path)?;
    }
    let objects = PayloadDir::open(&directory, policy.uid)?;
    let (file, bytes) = objects.open_regular(name)?;
    if bytes > maximum {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "user registration too large",
        ));
    }
    let mut raw = String::new();
    file.take(maximum + 1).read_to_string(&mut raw)?;
    Ok(raw)
}

pub fn token(support: &Path, caller: &CallerIdentity) -> io::Result<String> {
    let value = read_record(support, caller, "helper.token", 128)?;
    if value.len() < 32 || value.contains(['\n', '\r', '\0']) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid registered token",
        ));
    }
    Ok(value)
}

pub fn conf_dir(support: &Path, caller: &CallerIdentity) -> io::Result<PathBuf> {
    let raw = read_record(support, caller, "config-dir", 32 * 1024)?;
    let path = PathBuf::from(&raw);
    if raw.is_empty() || raw.contains(['\n', '\r', '\0']) || !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid registered config directory",
        ));
    }
    match caller {
        CallerIdentity::Unix(uid) => {
            let _ = PayloadDir::open(&path, Some(*uid))?;
        }
        CallerIdentity::Windows(sid) => {
            #[cfg(windows)]
            {
                use crate::platform::windows::{ops::ProcOps, winproc::WinProcOps};
                let sec = WinProcOps::default()
                    .read_object_security(&raw)
                    .map_err(|_| {
                        io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            "user config owner unavailable",
                        )
                    })?;
                if sec.owner_sid.as_deref() != Some(sid.as_str()) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "user config owner differs",
                    ));
                }
            }
            #[cfg(not(windows))]
            {
                let _ = sid;
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Windows user registry on another target",
                ));
            }
        }
    }
    Ok(path)
}
