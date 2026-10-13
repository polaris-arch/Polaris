//! Administrative registration per native caller. A request never chooses this key.
use crate::{core_objects::PayloadDir, core_store::OwnerPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePeer {
    pub identity: CallerIdentity,
    pub administrator: bool,
}

pub fn parse_identity(key: &str) -> Option<CallerIdentity> {
    if let Some(uid) = key.strip_prefix('u') {
        let parsed = uid.parse::<u32>().ok()?;
        return (parsed.to_string() == uid).then_some(CallerIdentity::Unix(parsed));
    }
    CallerIdentity::windows(key.strip_prefix('s')?)
}

pub fn register(
    support: &Path,
    caller: &CallerIdentity,
    config: &Path,
    token_file: &Path,
) -> io::Result<()> {
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    let text = config
        .to_str()
        .ok_or_else(|| io::Error::other("invalid config path"))?;
    if !config.is_absolute() || text.len() > 32 * 1024 || text.contains(['\n', '\r', '\0']) {
        return Err(io::Error::other("invalid config path"));
    }
    match caller {
        CallerIdentity::Unix(uid) => {
            let _ = PayloadDir::open(config, Some(*uid))?;
        }
        CallerIdentity::Windows(sid) => {
            #[cfg(windows)]
            {
                let directory = PayloadDir::open(config, None)?;
                if !crate::platform::windows::coreacl::is_private_user_config(
                    &directory.security()?,
                    sid,
                ) {
                    return Err(io::Error::other("config owner or isolation differs"));
                }
            }
            #[cfg(not(windows))]
            {
                let _ = sid;
                return Err(io::Error::other(
                    "Windows user registration on another target",
                ));
            }
        }
    }
    let owner = OwnerPolicy::privileged();
    owner.check(support)?;
    let users = support.join("users");
    let destination = users.join(caller.key());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !users.try_exists()? {
            fs::create_dir(&users)?;
            fs::set_permissions(&users, fs::Permissions::from_mode(0o755))?;
        }
        if !destination.try_exists()? {
            fs::create_dir(&destination)?;
            fs::set_permissions(&destination, fs::Permissions::from_mode(0o700))?;
        }
        if fs::symlink_metadata(&destination)?.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::other(
                "user credential directory must be private",
            ));
        }
    }
    #[cfg(windows)]
    {
        use crate::platform::windows::{coreacl, ops::ProcOps, winproc::WinProcOps};
        let sec = WinProcOps::default()
            .read_object_security(&destination.to_string_lossy())
            .map_err(|_| io::Error::other("precreated private user directory unavailable"))?;
        if coreacl::grants_non_privileged_read_execute(&sec) {
            return Err(io::Error::other("user credential directory is not private"));
        }
    }
    owner.check(&users)?;
    owner.check(&destination)?;
    owner.check(token_file)?;
    let token_parent = token_file
        .parent()
        .ok_or_else(|| io::Error::other("token parent unavailable"))?;
    let token_name = token_file
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::other("token name unavailable"))?;
    let objects = PayloadDir::open(token_parent, owner.uid)?;
    let (file, bytes) = objects.open_regular(token_name)?;
    if !(32..=128).contains(&bytes) {
        return Err(io::Error::other("invalid staged token size"));
    }
    let mut token = String::new();
    file.take(129).read_to_string(&mut token)?;
    if !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(io::Error::other("invalid staged token"));
    }
    // One private root record commits config and token together. Two separate
    // renames would leave an unusable half-registration on interruption.
    let raw = format!("{}\n{text}\n{token}\n", caller.key());
    let temporary = destination.join(format!(".registration-{}.new", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result: io::Result<()> = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(raw.as_bytes())?;
        file.sync_all()?;
        #[cfg(windows)]
        crate::platform::windows::winproc::seal_store_object(&temporary, false)?;
        fs::rename(&temporary, destination.join("registration"))?;
        #[cfg(unix)]
        std::fs::File::open(&destination)?.sync_all()?;
        Ok(())
    })();
    if temporary.try_exists()? {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(())
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
    #[cfg(windows)]
    for sec in [
        objects.security()?,
        crate::platform::windows::winproc::file_security(&file)?,
    ] {
        if crate::platform::windows::coreacl::grants_non_privileged_read_execute(&sec) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "user registration is not private",
            ));
        }
    }
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

fn registration(support: &Path, caller: &CallerIdentity) -> io::Result<(PathBuf, String)> {
    let raw = read_record(support, caller, "registration", 34 * 1024)?;
    parse_registration(&raw, caller)
}

fn parse_registration(raw: &str, caller: &CallerIdentity) -> io::Result<(PathBuf, String)> {
    let lines: Vec<_> = raw.split_terminator('\n').collect();
    if !raw.ends_with('\n')
        || lines.len() != 3
        || lines[0] != caller.key()
        || lines[1].is_empty()
        || lines[1].len() > 32 * 1024
        || lines[1].contains(['\r', '\0'])
        || !(32..=128).contains(&lines[2].len())
        || !lines[2].bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid native user registration",
        ));
    }
    let path = PathBuf::from(lines[1]);
    if !path.is_absolute() {
        return Err(io::Error::other("invalid registered config directory"));
    }
    Ok((path, lines[2].to_owned()))
}

pub fn token(support: &Path, caller: &CallerIdentity) -> io::Result<String> {
    registration(support, caller).map(|(_, token)| token)
}

pub fn conf_dir(support: &Path, caller: &CallerIdentity) -> io::Result<PathBuf> {
    let (path, _) = registration(support, caller)?;
    match caller {
        CallerIdentity::Unix(uid) => {
            let _ = PayloadDir::open(&path, Some(*uid))?;
        }
        CallerIdentity::Windows(sid) => {
            #[cfg(windows)]
            {
                let directory = PayloadDir::open(&path, None)?;
                if !crate::platform::windows::coreacl::is_private_user_config(
                    &directory.security()?,
                    sid,
                ) {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "user config owner or isolation differs",
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

#[cfg(test)]
#[path = "user_control/tests/mod.rs"]
mod tests;
