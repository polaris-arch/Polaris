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
        #[cfg(target_os = "macos")]
        crate::protected_install_path::no_extended_acl(&file)?;
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

/// Private transaction backup. The single byte discriminator preserves absence
/// without inventing a live credential. Call only under the installer lease.
#[cfg(unix)]
pub fn snapshot_registration(
    support: &Path,
    caller: &CallerIdentity,
    backup: &Path,
) -> io::Result<()> {
    snapshot_registration_with_owner(support, caller, backup, OwnerPolicy::privileged())
}

#[cfg(unix)]
fn snapshot_registration_with_owner(
    support: &Path,
    caller: &CallerIdentity,
    backup: &Path,
    policy: OwnerPolicy,
) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    policy.check(support)?;
    let parent = backup
        .parent()
        .ok_or_else(|| io::Error::other("backup parent missing"))?;
    policy.check(parent)?;
    let mut bytes = vec![0u8];
    let users = support.join("users");
    let destination = users.join(caller.key());
    for directory in [&users, &destination] {
        match std::fs::symlink_metadata(directory) {
            Ok(_) => {
                policy.check(directory)?;
                let _ = PayloadDir::open(directory, policy.uid)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => break,
            Err(error) => return Err(error),
        }
    }
    let record = destination.join("registration");
    match std::fs::symlink_metadata(&record) {
        Ok(_) => {
            let directory = PayloadDir::open(&destination, policy.uid)?;
            let (file, len) = directory.open_regular("registration")?;
            #[cfg(target_os = "macos")]
            crate::protected_install_path::no_extended_acl(&file)?;
            let metadata = file.metadata()?;
            if metadata.mode() & 0o7077 != 0 {
                return Err(io::Error::other(
                    "registration is not private or has special mode bits",
                ));
            }
            if len > 64 * 1024 {
                return Err(io::Error::other("registration too large"));
            }
            bytes[0] = 1;
            bytes.extend_from_slice(&(metadata.mode() & 0o777).to_le_bytes());
            bytes.extend_from_slice(&metadata.gid().to_le_bytes());
            file.take(64 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 != len + 9 {
                return Err(io::Error::other("registration changed size"));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(backup)?;
    #[cfg(target_os = "macos")]
    crate::protected_install_path::no_extended_acl(&file)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::File::open(parent)?.sync_all()
}

#[cfg(unix)]
pub fn restore_registration(
    support: &Path,
    caller: &CallerIdentity,
    backup: &Path,
) -> io::Result<()> {
    restore_registration_with_owner(support, caller, backup, OwnerPolicy::privileged())
}

#[cfg(unix)]
fn restore_registration_with_owner(
    support: &Path,
    caller: &CallerIdentity,
    backup: &Path,
    policy: OwnerPolicy,
) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    policy.check(support)?;
    let parent = backup
        .parent()
        .ok_or_else(|| io::Error::other("backup parent missing"))?;
    let objects = PayloadDir::open(parent, policy.uid)?;
    let name = backup
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| io::Error::other("invalid backup name"))?;
    let (file, len) = objects.open_regular(name)?;
    #[cfg(target_os = "macos")]
    crate::protected_install_path::no_extended_acl(&file)?;
    if len == 0 || len > 64 * 1024 + 9 {
        return Err(io::Error::other("invalid registration backup size"));
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 10).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != len {
        return Err(io::Error::other("registration backup changed size"));
    }
    let users = support.join("users");
    let directory = users.join(caller.key());
    let record = directory.join("registration");
    if bytes == [0] {
        if !directory.try_exists()? {
            return Ok(());
        }
        policy.check(&users)?;
        let objects = PayloadDir::open(&directory, policy.uid)?;
        match std::fs::symlink_metadata(&record) {
            Ok(_) => {
                let _ = objects.open_regular("registration")?;
                std::fs::remove_file(&record)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        return std::fs::File::open(&directory)?.sync_all();
    }
    if bytes.first() != Some(&1) || bytes.len() < 9 {
        return Err(io::Error::other("invalid registration backup marker"));
    }
    let mode = u32::from_le_bytes(
        bytes[1..5]
            .try_into()
            .map_err(|_| io::Error::other("invalid backup mode"))?,
    );
    let gid = u32::from_le_bytes(
        bytes[5..9]
            .try_into()
            .map_err(|_| io::Error::other("invalid backup group"))?,
    );
    if mode & !0o700 != 0 {
        return Err(io::Error::other("invalid private registration mode"));
    }
    policy.check(&users)?;
    let _directory = PayloadDir::open(&directory, policy.uid)?;
    let temporary = directory.join(format!(".restore-{}.new", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    #[cfg(target_os = "macos")]
    crate::protected_install_path::no_extended_acl(&file)?;
    file.write_all(&bytes[9..])?;
    nix::unistd::fchown(&file, None, Some(nix::unistd::Gid::from_raw(gid)))?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    file.sync_all()?;
    std::fs::rename(&temporary, &record)?;
    std::fs::File::open(&directory)?.sync_all()
}

#[cfg(test)]
#[path = "user_control/tests/mod.rs"]
mod tests;
