//! CI-only file/handle custody. No product entry point or selected child execution.
use super::{bool_ok, error, file_info, handle, info, OwnedHandle};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::mem::{size_of, zeroed};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::os::windows::io::FromRawHandle;
use std::path::{Component, Path, PathBuf, Prefix};
use std::ptr::null_mut;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Threading::*;

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Kind {
    File,
    Directory,
    App,
    Tool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: PathBuf,
    kind: Kind,
    #[serde(default)]
    missing: bool,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
enum Request {
    Inspect {
        files: Vec<Input>,
    },
    Hold {
        files: Vec<Input>,
    },
    Release {
        handles: Vec<usize>,
    },
    Process {
        pid: u32,
    },
    Check {
        handle: usize,
        file: Input,
    },
    Transfer {
        pid: u32,
        birth: String,
        handle: usize,
        file: Input,
    },
}

fn check_path(path: &Path, kind: Kind) -> io::Result<()> {
    let mut parts = path.components();
    let Some(Component::Prefix(prefix)) = parts.next() else {
        return Err(error("absolute Windows path required"));
    };
    if !matches!(
        prefix.kind(),
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(..) | Prefix::VerbatimUNC(..)
    ) || parts.next() != Some(Component::RootDir)
        || parts.any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(error("unsupported Windows filesystem namespace"));
    }
    if kind != Kind::Tool {
        for ancestor in path.ancestors().skip(1).filter(|parent| parent.has_root()) {
            let metadata = fs::symlink_metadata(ancestor)?;
            if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            {
                return Err(error("redirected Windows package ancestor"));
            }
        }
    }
    Ok(())
}

fn open(input: &Input, hold: bool) -> io::Result<File> {
    check_path(&input.path, input.kind)?;
    let mut options = OpenOptions::new();
    if hold {
        options
            .read(true)
            .share_mode(if matches!(input.kind, Kind::Directory | Kind::App) {
                FILE_SHARE_READ | FILE_SHARE_WRITE
            } else {
                FILE_SHARE_READ
            });
    } else {
        options
            .access_mode(FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let mut flags = if input.kind == Kind::Tool {
        0
    } else {
        FILE_FLAG_OPEN_REPARSE_POINT
    };
    if input.kind == Kind::Directory {
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    options.custom_flags(flags).open(&input.path)
}

fn windows_nanos(value: i64) -> String {
    ((i128::from(value) - 116_444_736_000_000_000) * 100).to_string()
}

fn identity(file: &File, input: &Input) -> io::Result<[String; 7]> {
    let old = if matches!(input.kind, Kind::Directory | Kind::File) {
        info(file)?
    } else {
        file_info(file)?
    };
    let directory = old.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let bytes = (u64::from(old.nFileSizeHigh) << 32) | u64::from(old.nFileSizeLow);
    if (input.kind == Kind::Directory) != directory
        || (!directory && (bytes > 512 * 1024 * 1024 || (input.kind != Kind::File && bytes == 0)))
        || (input.kind == Kind::App && !matches!(old.nNumberOfLinks, 1 | 2))
    {
        return Err(error("Windows package file type, links or size differs"));
    }
    let mut object: FILE_ID_INFO = unsafe { zeroed() };
    let mut basic: FILE_BASIC_INFO = unsafe { zeroed() };
    bool_ok(unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileIdInfo,
            (&mut object as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    })?;
    bool_ok(unsafe {
        GetFileInformationByHandleEx(
            handle(file),
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        )
    })?;
    let executable = directory
        || input
            .path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|ext| {
                ["exe", "com", "cmd", "bat"]
                    .iter()
                    .any(|name| ext.eq_ignore_ascii_case(name))
            });
    let mode = if directory { 0o040000 } else { 0o100000 }
        | if old.dwFileAttributes & FILE_ATTRIBUTE_READONLY != 0 {
            0o444
        } else {
            0o666
        }
        | if executable { 0o111 } else { 0 };
    Ok([
        object.VolumeSerialNumber.to_string(),
        u128::from_le_bytes(object.FileId.Identifier).to_string(),
        mode.to_string(),
        old.nNumberOfLinks.to_string(),
        bytes.to_string(),
        windows_nanos(basic.LastWriteTime),
        windows_nanos(basic.CreationTime),
    ])
}

fn process(pid: u32, access: u32) -> io::Result<OwnedHandle> {
    let handle = unsafe { OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(OwnedHandle(handle))
}

fn birth(process: HANDLE) -> io::Result<u64> {
    let mut times: [FILETIME; 4] = unsafe { zeroed() };
    let times = times.as_mut_ptr();
    bool_ok(unsafe { GetProcessTimes(process, times, times.add(1), times.add(2), times.add(3)) })?;
    let created = unsafe { *times };
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}

fn transfer(pid: u32, expected_birth: &str, source: usize, input: &Input) -> io::Result<File> {
    if input.kind != Kind::App || input.missing {
        return Err(error("transfer requires the exact Cargo App"));
    }
    let process = process(pid, PROCESS_DUP_HANDLE | SYNCHRONIZE)?;
    if birth(process.0)?.to_string() != expected_birth
        || unsafe { WaitForSingleObject(process.0, 0) } != WAIT_TIMEOUT
    {
        return Err(error(
            "capture process exited or changed before handle transfer",
        ));
    }
    let mut copied = null_mut();
    bool_ok(unsafe {
        DuplicateHandle(
            process.0,
            source as HANDLE,
            GetCurrentProcess(),
            &mut copied,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    })?;
    let copied = unsafe { File::from_raw_handle(copied.cast()) };
    let named = open(input, false)?;
    if identity(&copied, input)? != identity(&named, input)? {
        return Err(error("transferred Cargo App identity differs"));
    }
    Ok(copied)
}

fn admit(input: &Input, hold: bool) -> io::Result<Option<(File, [String; 7])>> {
    let held = match open(input, hold) {
        Ok(file) => file,
        Err(cause) if !hold && input.missing && cause.kind() == io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(cause) => return Err(cause),
    };
    let actual = identity(&held, input)?;
    if hold && (actual != identity(&open(input, false)?, input)?) {
        return Err(error("held Windows package path identity differs"));
    }
    Ok(Some((held, actual)))
}

pub fn serve() -> io::Result<()> {
    use std::collections::HashMap;
    const MAX_REQUEST: u64 = 8 * 1024 * 1024;
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let mut held = HashMap::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if (&mut input)
            .take(MAX_REQUEST + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            return Ok(());
        }
        if line.len() as u64 > MAX_REQUEST || line.last() != Some(&b'\n') {
            return Err(error("bounded complete native custody request required"));
        }
        let request = serde_json::from_slice(&line)?;
        let result = match request {
            Request::Inspect { files } => files
                .iter()
                .map(|input| Ok(admit(input, false)?.map(|(_, identity)| identity)))
                .collect::<io::Result<Vec<_>>>()
                .map(|identities| json!(identities)),
            Request::Hold { files } => (|| {
                let files = files
                    .iter()
                    .map(|input| admit(input, true).map(Option::unwrap))
                    .collect::<io::Result<Vec<_>>>()?;
                let handles = files
                    .into_iter()
                    .map(|(file, identity)| {
                        let raw = handle(&file) as usize;
                        held.insert(raw, file);
                        json!({ "handle": raw, "identity": identity })
                    })
                    .collect::<Vec<_>>();
                Ok(json!(handles))
            })(),
            Request::Release { handles } => {
                if handles.iter().all(|raw| held.contains_key(raw)) {
                    for raw in handles {
                        held.remove(&raw);
                    }
                    Ok(Value::Null)
                } else {
                    Err(error("unknown retained Windows handle"))
                }
            }
            Request::Process { pid } => process(pid, 0)
                .and_then(|process| birth(process.0))
                .map(|value| json!(value.to_string())),
            Request::Check { handle, file } => (|| {
                let retained = held
                    .get(&handle)
                    .ok_or_else(|| error("unknown retained Windows handle"))?;
                let id = identity(retained, &file)?;
                if id != identity(&open(&file, false)?, &file)? {
                    return Err(error("held Windows package path identity differs"));
                }
                Ok(json!(id))
            })(),
            Request::Transfer {
                pid,
                birth,
                handle: raw,
                file,
            } => (|| {
                let copied = transfer(pid, &birth, raw, &file)?;
                let identity = identity(&copied, &file)?;
                let raw = handle(&copied) as usize;
                held.insert(raw, copied);
                Ok(json!({ "handle": raw, "identity": identity }))
            })(),
        };
        let reply = match result {
            Ok(value) => json!({ "value": value }),
            Err(cause) => json!({ "error": cause.to_string() }),
        };
        serde_json::to_writer(&mut output, &reply)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}
