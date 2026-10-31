//! NSIS's two PRE hooks use this operation directly, under their current token.
//! Only exact installation-relative images and original process objects qualify.
use super::*;
use windows_sys::Win32::Globalization::{CompareStringOrdinal, CSTR_EQUAL};
use windows_sys::Win32::System::Diagnostics::ToolHelp::*;

pub(super) const TAILS: [&str; 2] = [
    r"_up_\resources\win\sing-box.exe",
    r"resources\win\sing-box.exe",
];
fn ordinal(a: &[u16], b: &[u16]) -> bool {
    let (Ok(alen), Ok(blen)) = (i32::try_from(a.len()), i32::try_from(b.len())) else {
        return false;
    };
    // SAFETY: counted slices remain live, including embedded non-ASCII units.
    unsafe { CompareStringOrdinal(a.as_ptr(), alen, b.as_ptr(), blen, 1) == CSTR_EQUAL }
}
fn full_path(path: &OsStr) -> io::Result<Vec<u16>> {
    let mut path: Vec<u16> = path.encode_wide().collect();
    if path.is_empty() || path.contains(&0) {
        return Err(error("invalid sweep path"));
    }
    let unc: Vec<u16> = OsStr::new(r"\\?\UNC\").encode_wide().collect();
    let ext: Vec<u16> = OsStr::new(r"\\?\").encode_wide().collect();
    if path.len() >= unc.len() && ordinal(&path[..unc.len()], &unc) {
        path = [vec![92, 92], path[unc.len()..].to_vec()].concat();
    } else if path.starts_with(&ext) {
        path = path[ext.len()..].to_vec();
    }
    path.push(0);
    let mut out = vec![0u16; 32768];
    // SAFETY: absolute normalization only, never opens/executes target images.
    let n = unsafe {
        GetFullPathNameW(
            path.as_ptr(),
            out.len() as u32,
            out.as_mut_ptr(),
            null_mut(),
        )
    };
    if n == 0 || n as usize >= out.len() {
        return Err(error("sweep normalization failed"));
    }
    out.truncate(n as usize);
    Ok(out)
}
pub(super) fn sweep_root(path: &OsStr) -> io::Result<Option<Vec<u16>>> {
    let mut full = full_path(path)?;
    while full.last() == Some(&92) {
        full.pop();
    }
    // Drive root or UNC share root: never sweep an entire volume/share.
    if full.len() == 2 && full[1] == 58 {
        return Ok(None);
    }
    if full.starts_with(&[92, 92])
        && full[2..]
            .split(|u| *u == 92)
            .filter(|p| !p.is_empty())
            .count()
            <= 2
    {
        return Ok(None);
    }
    full.push(92);
    Ok(Some(full))
}
pub(super) fn installed(root: &[u16], image: &OsStr) -> bool {
    let Ok(full) = full_path(image) else {
        return false;
    };
    full.len() >= root.len()
        && ordinal(&full[..root.len()], root)
        && TAILS.iter().any(|tail| {
            ordinal(
                &full[root.len()..],
                &OsStr::new(tail).encode_wide().collect::<Vec<_>>(),
            )
        })
}
fn process_image(process: HANDLE) -> io::Result<OsString> {
    let mut text = vec![0u16; 32768];
    let mut len = text.len() as u32;
    // SAFETY: owned process handle, bounded writable UTF16 buffer.
    bool_ok(unsafe { QueryFullProcessImageNameW(process, 0, text.as_mut_ptr(), &mut len) })?;
    if len as usize >= text.len() {
        return Err(error("process image too long"));
    }
    Ok(OsString::from_wide(&text[..len as usize]))
}
fn birth(process: HANDLE) -> io::Result<u64> {
    let mut created = unsafe { zeroed() };
    let mut exited = unsafe { zeroed() };
    let mut kernel = unsafe { zeroed() };
    let mut user = unsafe { zeroed() };
    // SAFETY: all four FILETIME outputs have storage and the original handle lives.
    bool_ok(unsafe {
        GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user)
    })?;
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}
pub(super) struct Observation {
    pub pid: u32,
    pub image: OsString,
    pub birth: u64,
}
fn observations() -> io::Result<Vec<Observation>> {
    // SAFETY: snapshot handle is independently owned; no PID is used to mutate.
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = OwnedHandle(raw);
    let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snapshot.0, &mut entry) };
    let mut out = vec![];
    if ok == 0 && io::Error::last_os_error().raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
        return Err(io::Error::last_os_error());
    }
    while ok != 0 {
        let end = entry
            .szExeFile
            .iter()
            .position(|u| *u == 0)
            .ok_or_else(|| error("unterminated process name"))?;
        if ordinal(
            &entry.szExeFile[..end],
            &OsStr::new("sing-box.exe").encode_wide().collect::<Vec<_>>(),
        ) {
            let h = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE,
                    0,
                    entry.th32ProcessID,
                )
            };
            if !h.is_null() {
                let process = OwnedHandle(h);
                if let (Ok(image), Ok(birth)) = (process_image(process.0), birth(process.0)) {
                    out.push(Observation {
                        pid: entry.th32ProcessID,
                        image,
                        birth,
                    });
                }
            }
        }
        ok = unsafe { Process32NextW(snapshot.0, &mut entry) };
        if ok == 0 && io::Error::last_os_error().raw_os_error() != Some(ERROR_NO_MORE_FILES as i32)
        {
            return Err(io::Error::last_os_error());
        }
        if out.len() > 32768 {
            return Err(error("process observation budget exceeded"));
        }
    }
    Ok(out)
}
pub(super) fn sweep(root: &[u16], observations: Vec<Observation>) -> i32 {
    let mut left = false;
    for observed in observations {
        if !installed(root, &observed.image) {
            continue;
        }
        // SAFETY: minimal rights for this operation; HANDLE is retained through
        // identity revalidation, the single terminate call and actual exit wait.
        let raw = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE | PROCESS_TERMINATE,
                0,
                observed.pid,
            )
        };
        if raw.is_null() {
            if io::Error::last_os_error().raw_os_error() != Some(ERROR_INVALID_PARAMETER as i32) {
                left = true;
            }
            continue;
        }
        let process = OwnedHandle(raw);
        let matched = (|| -> io::Result<bool> {
            let image = process_image(process.0)?;
            let born = birth(process.0)?;
            Ok(installed(root, &image)
                && born == observed.birth
                && ordinal(&full_path(&image)?, &full_path(&observed.image)?))
        })();
        match matched {
            Ok(true) => {}
            Ok(false) => continue,
            Err(_) => {
                left = true;
                continue;
            }
        }
        // No name/PID kill, shell, taskkill, SCM or ProgramData cleanup here.
        if unsafe { TerminateProcess(process.0, 1) } == 0
            || unsafe { WaitForSingleObject(process.0, 5000) } != WAIT_OBJECT_0
        {
            left = true;
        }
    }
    if left {
        2
    } else {
        0
    }
}
pub fn sweep_installed_cores() -> i32 {
    let Some(path) = std::env::var_os("POLARIS_SWEEP_ROOT") else {
        return 3;
    };
    let root = match sweep_root(&path) {
        Ok(Some(root)) => root,
        Ok(None) => return 3,
        Err(_) => return 1,
    };
    match observations() {
        Ok(items) => sweep(&root, items),
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests;
