//! Fixed-purpose Windows service/support cleanup. No caller-selected target.
//!
//! This does not authenticate an unsigned distribution. A modified per-user
//! carrier remains a distribution risk; UAC and local image custody do not
//! turn it into a publisher-authenticated artifact.

pub const SERVICE: &str = "PolarisHelper";
pub const SUPPORT_PARENT: &str = r"C:\ProgramData";
pub const SUPPORT_LEAF: &str = "Polaris";
pub const IMAGE_NAME: &str = "polaris-cleaner.exe";

pub mod install;

#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Completed(i32),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Refused,
    Partial,
    Cancelled,
    LaunchFailed,
}

impl Outcome {
    pub const fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Refused => 2,
            Self::Partial => 3,
            Self::Cancelled => 20,
            Self::LaunchFailed => 23,
        }
    }

    pub const fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Success,
            2 => Self::Refused,
            3 => Self::Partial,
            20 => Self::Cancelled,
            _ => Self::LaunchFailed,
        }
    }
}

/// Only these roles are accepted. Paths, service names, PIDs and commands
/// cannot cross this boundary, including after elevation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Launcher,
    Worker,
    InstallWorker(std::path::PathBuf),
    SweepInstalledCores,
}

pub fn role(args: impl IntoIterator<Item = std::ffi::OsString>) -> Option<Role> {
    let mut args = args.into_iter();
    let role = match args.next()?.to_str()? {
        "--launch" => Role::Launcher,
        "--worker" => Role::Worker,
        "--sweep-installed-cores" => Role::SweepInstalledCores,
        "--install-worker" => {
            if args.next()?.to_str()? != "--request" {
                return None;
            }
            let path = std::path::PathBuf::from(args.next()?);
            if !path.is_absolute() || path.file_name()?.to_str()? != "install-request.json" {
                return None;
            }
            Role::InstallWorker(path)
        }
        _ => return None,
    };
    args.next().is_none().then_some(role)
}

// Native security descriptors are reduced to this policy. Unsupported ACEs
// and missing/null DACLs fail closed; inherited-only ACEs do not apply here.
pub const WRITE_MASK: u32 = 0x500d_0156;
// Attribute/create rights on an ancestor do not replace an existing child.
// This narrower mask is valid only with Chain's pinned, non-reparse relative IO;
// named image execution also retains a child at every ancestor (see Image).
// Attribute-only handles are NOT excluded by FILE_SHARE_READ and can set a
// reparse point on an empty directory. Never apply this mask to a payload leaf.
pub const REPLACE_MASK: u32 = 0x500d_0040;
pub const SYSTEM: &str = "S-1-5-18";
pub const ADMINS: &str = "S-1-5-32-544";
pub const TRUSTED_INSTALLER: &str =
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";

#[derive(Debug)]
pub struct Ace {
    pub kind: u8,
    pub flags: u8,
    pub mask: u32,
    pub sid: String,
}

fn appcontainer_principal(sid: &str) -> bool {
    // Package/capability SIDs are the restricting half of AppContainer access,
    // not an independent grant to an ordinary foreign user. Do not trust their
    // owners, other authority-15 namespaces, or malformed SID text.
    // https://learn.microsoft.com/windows/win32/secauthz/implementing-an-appcontainer
    let Some(rest) = sid.strip_prefix("S-1-15-") else {
        return false;
    };
    let mut parts = rest.split('-');
    if !matches!(parts.next(), Some("2" | "3")) {
        return false;
    }
    let tail: Vec<_> = parts.collect();
    !tail.is_empty()
        && tail.len() <= 14
        && tail.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|c| c.is_ascii_digit())
                && part.parse::<u32>().is_ok()
        })
}

pub fn trusted_security(
    owner: &str,
    aces: Option<&[Ace]>,
    caller: Option<&str>,
    ancestor: bool,
) -> bool {
    let allowed = |sid: &str| {
        sid == SYSTEM
            || sid == ADMINS
            || caller == Some(sid)
            || (ancestor && sid == TRUSTED_INSTALLER)
    };
    if !allowed(owner) {
        return false;
    }
    let Some(aces) = aces else {
        return false;
    };
    aces.iter().all(|ace| {
        if ace.flags & 8 != 0 {
            return true;
        }
        match ace.kind {
            1 => true, // Deny grants nothing. Actual handle access is still required.
            0 => {
                // ALL ordinary grants must still pass this conjunction. A
                // capability ACE cannot cancel a foreign user's dangerous ACE
                // or promote a capability/package SID into an allowed owner.
                allowed(&ace.sid)
                    || appcontainer_principal(&ace.sid)
                    || ace.mask & if ancestor { REPLACE_MASK } else { WRITE_MASK } == 0
            }
            _ => false,
        }
    })
}

#[cfg(any(windows, test))]
mod service;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{launch_bundle, launch_current, run_worker};
#[cfg(windows)]
pub use windows::{launch_install, run_install_worker, sweep_installed_cores};

#[cfg(test)]
mod tests;
