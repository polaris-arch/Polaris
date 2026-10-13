//! Bounded, read-only observations for retiring exact legacy directories.
//! An unavailable or partial scan keeps files. This is not a machine-wide
//! no-owner receipt and never signals a process or executes a candidate binary.
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyUse {
    Idle,
    InUse,
    Unknown,
}
const OUTPUT_LIMIT: u64 = 512 * 1024;

/// Observe precisely the payload cohort. Directory handles, store locks and
/// metadata held by the manager are not evidence that a payload is in use.
pub fn payload_use(paths: &[std::path::PathBuf]) -> LegacyUse {
    if paths.is_empty()
        || paths.len() > 4
        || paths.iter().any(|path| {
            !path.is_absolute()
                || path.to_string_lossy().contains(['\n', '\r', '\0'])
                || std::fs::symlink_metadata(path).is_err()
                || std::fs::symlink_metadata(path)
                    .is_ok_and(|m| !m.is_file() || m.file_type().is_symlink())
        })
    {
        return LegacyUse::Unknown;
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let mut command = Command::new(if cfg!(target_os = "macos") {
            "/usr/sbin/lsof"
        } else {
            "/usr/bin/lsof"
        });
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .args(["-nP", "-Fpn", "--"])
            .args(paths);
        match observe_command(&mut command) {
            Ok((code, out, err)) => classify_unix(code, &out, &err),
            Err(_) => LegacyUse::Unknown,
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::process::CommandExt;
        let mut held = Vec::new();
        for path in paths {
            match std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(path)
            {
                Ok(file) => held.push(file),
                Err(error) if error.raw_os_error() == Some(32) => return LegacyUse::InUse,
                Err(_) => return LegacyUse::Unknown,
            }
        }
        let Ok(root) = native_windows_root() else {
            return LegacyUse::Unknown;
        };
        let literals = paths
            .iter()
            .map(|p| format!("'{}'", p.to_string_lossy().replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(",");
        let script = format!(
            r#"$ErrorActionPreference='Stop'; $files=@({literals}); $unknown=$false; $busy=$false; foreach($p in Get-Process) {{ try {{ foreach($m in $p.Modules) {{ foreach($file in $files) {{ if([String]::Equals($m.FileName,$file,[StringComparison]::OrdinalIgnoreCase)) {{$busy=$true}} }} }} }} catch {{$unknown=$true}} }}; if($busy){{'in-use'}} elseif($unknown){{'unknown'}} else {{'idle'}}"#
        );
        let mut command = Command::new(root.join("System32/WindowsPowerShell/v1.0/powershell.exe"));
        command
            .env_clear()
            .env("SystemRoot", &root)
            .env("PATH", root.join("System32"))
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .creation_flags(0x0800_0000);
        match observe_command(&mut command) {
            Ok((0, out, err)) if err.is_empty() => {
                match std::str::from_utf8(&out).ok().map(str::trim) {
                    Some("idle") => LegacyUse::Idle,
                    Some("in-use") => LegacyUse::InUse,
                    _ => LegacyUse::Unknown,
                }
            }
            _ => LegacyUse::Unknown,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    LegacyUse::Unknown
}

fn capture(mut input: impl Read) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(OUTPUT_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > OUTPUT_LIMIT {
        return Err(io::Error::other("legacy observation output limit"));
    }
    Ok(bytes)
}

fn observe_command(command: &mut Command) -> io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let out = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("observer stdout unavailable"))?;
    let err = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("observer stderr unavailable"))?;
    let out = std::thread::spawn(move || capture(out));
    let err = std::thread::spawn(move || capture(err));
    let until = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "legacy observation deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    let stdout = out
        .join()
        .map_err(|_| io::Error::other("legacy observer stdout panic"))??;
    let stderr = err
        .join()
        .map_err(|_| io::Error::other("legacy observer stderr panic"))??;
    Ok((status.code().unwrap_or(-1), stdout, stderr))
}

pub fn classify_unix(code: i32, stdout: &[u8], stderr: &[u8]) -> LegacyUse {
    if !stderr.is_empty() {
        return LegacyUse::Unknown;
    }
    if matches!(code, 0 | 1) && stdout.is_empty() {
        return LegacyUse::Idle;
    }
    if code != 0 {
        return LegacyUse::Unknown;
    }
    let Ok(text) = std::str::from_utf8(stdout) else {
        return LegacyUse::Unknown;
    };
    // lsof field output is authoritative only when it contains an actual pid.
    if text.lines().any(|line| {
        line.strip_prefix('p')
            .is_some_and(|pid| pid.parse::<std::num::NonZeroU32>().is_ok())
    }) {
        LegacyUse::InUse
    } else {
        LegacyUse::Unknown
    }
}

pub fn directory_use(path: &Path) -> LegacyUse {
    if !path.is_absolute()
        || path
            .as_os_str()
            .to_string_lossy()
            .contains(['\n', '\r', '\0'])
    {
        return LegacyUse::Unknown;
    }
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return LegacyUse::Unknown;
    };
    if meta.file_type().is_symlink() {
        return LegacyUse::Unknown;
    }
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let tool = if cfg!(target_os = "macos") {
            "/usr/sbin/lsof"
        } else {
            "/usr/bin/lsof"
        };
        let mut command = Command::new(tool);
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .args(["-nP", "-Fpn"]);
        if meta.is_dir() {
            command.arg("+D");
        } else {
            command.arg("--");
        }
        command.arg(path);
        match observe_command(&mut command) {
            Ok((code, out, err)) => classify_unix(code, &out, &err),
            Err(_) => LegacyUse::Unknown,
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let Ok(root) = native_windows_root() else {
            return LegacyUse::Unknown;
        };
        let tool = root.join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let quoted = path.to_string_lossy().replace('\'', "''");
        // Native process modules include loaded sidecars; any inaccessible
        // process makes the observation unknown. No process is stopped.
        let script = format!(
            r#"$ErrorActionPreference='Stop'; $prefix='{quoted}'.TrimEnd('\')+'\'; $unknown=$false; $busy=$false; try {{ foreach($p in Get-Process) {{ try {{ foreach($m in $p.Modules) {{ if($m.FileName.StartsWith($prefix,[StringComparison]::OrdinalIgnoreCase)) {{$busy=$true}} }} }} catch {{$unknown=$true}} }} }} catch {{$unknown=$true}}; if($busy){{'in-use'}} elseif($unknown){{'unknown'}} else {{'idle'}}"#
        );
        let mut command = Command::new(tool);
        command
            .env_clear()
            .env("SystemRoot", &root)
            .env("PATH", root.join("System32"))
            .env(
                "PSModulePath",
                root.join("System32/WindowsPowerShell/v1.0/Modules"),
            );
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &script,
            ])
            .creation_flags(0x0800_0000);
        match observe_command(&mut command) {
            Ok((0, out, err)) if err.is_empty() => {
                match std::str::from_utf8(&out).ok().map(str::trim) {
                    Some("idle") => LegacyUse::Idle,
                    Some("in-use") => LegacyUse::InUse,
                    _ => LegacyUse::Unknown,
                }
            }
            _ => LegacyUse::Unknown,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        LegacyUse::Unknown
    }
}

#[cfg(windows)]
#[allow(
    unsafe_code,
    reason = "read native Windows directory instead of caller environment"
)]
fn native_windows_root() -> io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = vec![0u16; 32768];
    // SAFETY: live writable UTF-16 buffer with the supplied capacity; read-only OS query.
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(std::ffi::OsString::from_wide(&buffer[..length]).into())
}

#[cfg(test)]
mod tests;
