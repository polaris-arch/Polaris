//! install-core 的源目录句柄：枚举条目名 + 按「目录句柄 + 名字」打开单个常规文件。
//!
//! 源目录由低权限客户端指定、内容由它摆放，本进程以高权限读。这里保证两件事：
//!
//! 1. [`SrcDir::open_regular`] 交回的句柄**就是**持有的那个目录里那个名字本身 —— 不是它指向的
//!    别处，不是打开之后被换掉的另一个目录里的同名文件，也不是一个会把读线程卡住的对象。
//!    调用方此后只从这个句柄读，不再按路径回头打开。
//! 2. （unix）目录与文件都**属调用者所有、且组与其他人不可写**：能把内容摆进来的只有调用者
//!    自己，它本来就读得到这些字节，高权限进程替它读不会多给出任何东西。
//!
//! 三条腿：
//!
//! - **linux / macos**：目录按 fd 持有，子项 `openat(O_NOFOLLOW | O_NONBLOCK)` + `fstat`。
//!   `O_NONBLOCK` 让命名管道的只读打开立即返回（没有它会一直等到对端出现），类型随后按 fd 判；
//!   它对常规文件的读没有影响。另拒链接数不为 1 的文件。linux 上目录用 `openat2` 打开，整条
//!   路径不穿越任何符号链接与 `/proc` 魔法链接；内核没有这个调用（`ENOSYS`）时退回
//!   `open(O_NOFOLLOW)`，只守最后一级 —— 属主判据两条路都过，不因回退放宽。
//! - **windows**：目录句柄一直持有，条目经该句柄枚举，子项用 `NtCreateFile` **相对该句柄**
//!   打开（Win32 的按路径打开会重新解析目录路径，打开之后把目录或它的某一级父目录换成联接点
//!   就能把读引到别处）。符号链接与挂载点类重解析点一律拒，判据是 std 的
//!   `FileType::is_symlink`（重解析点且标记带「名字替代」位）；压缩、云盘占位这类不指向别处的
//!   重解析点照常当普通文件。本批不取对端身份，不判属主；建硬链接在 Windows 上要求对目标有
//!   写权，不判链接数。
//! - **其余 target**（本 crate 作为依赖被编进去，但那里没有 helper）：一律拒绝。

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::Path;

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn rejected(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, reason)
}

/// 已打开的源目录。
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) struct SrcDir {
    dir: nix::dir::Dir,
    owner_uid: u32,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
const DIR_FLAGS: nix::fcntl::OFlag = nix::fcntl::OFlag::O_RDONLY
    .union(nix::fcntl::OFlag::O_DIRECTORY)
    .union(nix::fcntl::OFlag::O_NOFOLLOW)
    .union(nix::fcntl::OFlag::O_CLOEXEC);

/// 属主判据：属 `owner_uid` 所有，且组与其他人不可写。
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn check_owner(stat: &nix::sys::stat::FileStat, owner_uid: u32) -> io::Result<()> {
    if stat.st_uid != owner_uid {
        return Err(rejected("not owned by the caller"));
    }
    if stat.st_mode & 0o022 != 0 {
        return Err(rejected("writable by group or others"));
    }
    Ok(())
}

/// 只守最后一级不跟随的打开（macos 的唯一路径；linux 在内核没有 `openat2` 时的回退）。
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_dir_last_component_nofollow(path: &Path) -> io::Result<std::os::fd::OwnedFd> {
    Ok(nix::fcntl::open(
        path,
        DIR_FLAGS,
        nix::sys::stat::Mode::empty(),
    )?)
}

/// 整条路径不穿越符号链接与魔法链接的打开。
#[cfg(target_os = "linux")]
fn open_dir(path: &Path) -> io::Result<std::os::fd::OwnedFd> {
    use nix::fcntl::{openat2, OpenHow, ResolveFlag, AT_FDCWD};
    let how = OpenHow::new()
        .flags(DIR_FLAGS)
        .resolve(ResolveFlag::RESOLVE_NO_SYMLINKS | ResolveFlag::RESOLVE_NO_MAGICLINKS);
    match openat2(AT_FDCWD, path, how) {
        // 5.6 之前的内核没有这个调用。别的错误（含路径上有链接的 ELOOP）原样返回，不回退。
        Err(nix::errno::Errno::ENOSYS) => open_dir_last_component_nofollow(path),
        other => Ok(other?),
    }
}

#[cfg(target_os = "macos")]
fn open_dir(path: &Path) -> io::Result<std::os::fd::OwnedFd> {
    open_dir_last_component_nofollow(path)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl SrcDir {
    /// `owner_uid` 是已鉴权的调用者 uid；拿不到（`None`）就不收。
    pub(super) fn open(path: &Path, owner_uid: Option<u32>) -> io::Result<Self> {
        let owner_uid = owner_uid.ok_or_else(|| rejected("caller identity unavailable"))?;
        Self::from_fd(open_dir(path)?, owner_uid)
    }

    fn from_fd(fd: std::os::fd::OwnedFd, owner_uid: u32) -> io::Result<Self> {
        check_owner(&nix::sys::stat::fstat(&fd)?, owner_uid)?;
        Ok(Self {
            dir: nix::dir::Dir::from_fd(fd)?,
            owner_uid,
        })
    }

    /// 单测：打开目录之后把记下的调用者换掉，单独走到「文件属主不符」那条分支。
    #[cfg(test)]
    pub(super) fn set_owner_uid_for_test(&mut self, owner_uid: u32) {
        self.owner_uid = owner_uid;
    }

    /// 全部条目名（不含 `.` / `..`，任何类型都列出）；超过 `max` 个即失败。
    pub(super) fn entry_names(&mut self, max: usize) -> io::Result<Vec<OsString>> {
        use std::os::unix::ffi::OsStrExt;
        let mut names = Vec::new();
        for entry in self.dir.iter() {
            let entry = entry?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if names.len() == max {
                return Err(rejected("too many entries"));
            }
            names.push(std::ffi::OsStr::from_bytes(name).to_owned());
        }
        Ok(names)
    }

    /// 打开源目录里名为 `name` 的**属调用者所有的单链接常规文件**，返回句柄与打开时的大小。
    pub(super) fn open_regular(&self, name: &str) -> io::Result<(File, u64)> {
        use nix::fcntl::{openat, OFlag};
        use nix::sys::stat::{fstat, Mode, SFlag};
        let flags = OFlag::O_RDONLY
            | OFlag::O_NOFOLLOW
            | OFlag::O_NONBLOCK
            | OFlag::O_NOCTTY
            | OFlag::O_CLOEXEC;
        let fd = openat(&self.dir, name, flags, Mode::empty())?;
        let stat = fstat(&fd)?;
        if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
            return Err(rejected("not a regular file"));
        }
        if stat.st_nlink != 1 {
            return Err(rejected("file has more than one link"));
        }
        check_owner(&stat, self.owner_uid)?;
        Ok((
            File::from(fd),
            u64::try_from(stat.st_size).unwrap_or(u64::MAX),
        ))
    }
}

/// 从一块 `FILE_FULL_DIR_INFO` 链里取出文件名（UTF-16）。
///
/// 纯解析，布局偏移由调用方给（Windows 上取自 `offset_of!`）：每条记录开头 4 字节是到下一条的
/// 偏移（0 = 最后一条），`name_len_at` 处 4 字节是文件名的**字节**数，文件名从 `name_at` 起。
/// 任何越界都判整块无效，不猜。
#[cfg(any(windows, test))]
fn parse_dir_entry_names(
    buf: &[u8],
    name_len_at: usize,
    name_at: usize,
) -> io::Result<Vec<Vec<u16>>> {
    let malformed = || io::Error::new(io::ErrorKind::InvalidData, "malformed directory listing");
    let read_u32 = |at: usize| -> io::Result<usize> {
        let bytes = buf.get(at..at + 4).ok_or_else(malformed)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
    };
    let mut names = Vec::new();
    let mut record = 0usize;
    loop {
        let next = read_u32(record)?;
        let name_len = read_u32(record + name_len_at)?;
        if !name_len.is_multiple_of(2) {
            return Err(malformed());
        }
        let start = record + name_at;
        let name = buf.get(start..start + name_len).ok_or_else(malformed)?;
        names.push(
            name.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect(),
        );
        if next == 0 {
            return Ok(names);
        }
        record = record.checked_add(next).ok_or_else(malformed)?;
    }
}

/// 已打开的源目录（句柄一直持有，子项相对它打开）。
#[cfg(windows)]
pub(super) struct SrcDir(File);

#[cfg(windows)]
impl SrcDir {
    /// Windows 这条腿不判属主（见模块文档），`_owner_uid` 不用。
    pub(super) fn open(path: &Path, _owner_uid: Option<u32>) -> io::Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        };
        // FILE_FLAG_BACKUP_SEMANTICS：目录句柄要它才打得开。FILE_FLAG_OPEN_REPARSE_POINT：最后一级
        // 若是联接点/符号链接，打开的是它本身而不是它指向的目录，下面按句柄认出来拒掉。
        let dir = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        let file_type = dir.metadata()?.file_type();
        if file_type.is_symlink() {
            return Err(rejected("reparse point"));
        }
        if !file_type.is_dir() {
            return Err(rejected("not a directory"));
        }
        Ok(Self(dir))
    }

    /// 读一批目录项到 `buf`；`Ok(false)` = 已读完。
    #[allow(unsafe_code)]
    fn read_entries(&self, buf: &mut [u64], restart: bool) -> io::Result<bool> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::ERROR_NO_MORE_FILES;
        use windows_sys::Win32::Storage::FileSystem::{
            FileFullDirectoryInfo, FileFullDirectoryRestartInfo, GetFileInformationByHandleEx,
        };
        let class = if restart {
            FileFullDirectoryRestartInfo
        } else {
            FileFullDirectoryInfo
        };
        let byte_len = u32::try_from(std::mem::size_of_val(buf)).unwrap_or(u32::MAX);
        // SAFETY: 句柄来自本结构持有的 `File`，调用期间有效；`buf` 是独占借用的可写内存，
        // 长度按字节如实传入；`u64` 元素满足该信息类要求的 8 字节对齐。
        let ok = unsafe {
            GetFileInformationByHandleEx(
                self.0.as_raw_handle().cast(),
                class,
                buf.as_mut_ptr().cast(),
                byte_len,
            )
        };
        if ok != 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == i32::try_from(ERROR_NO_MORE_FILES).ok() {
            return Ok(false);
        }
        Err(error)
    }

    /// 全部条目名（不含 `.` / `..`，任何类型都列出）；超过 `max` 个即失败。
    ///
    /// 经**持有的句柄**枚举，不按路径重开目录：否则列出来的与随后打开的可以是两个目录。
    pub(super) fn entry_names(&mut self, max: usize) -> io::Result<Vec<OsString>> {
        use std::mem::offset_of;
        use std::os::windows::ffi::OsStringExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_FULL_DIR_INFO;
        let mut buf = vec![0u64; 8 * 1024];
        let mut names = Vec::new();
        let mut restart = true;
        while self.read_entries(&mut buf, restart)? {
            restart = false;
            let bytes: Vec<u8> = buf.iter().flat_map(|word| word.to_ne_bytes()).collect();
            for name in parse_dir_entry_names(
                &bytes,
                offset_of!(FILE_FULL_DIR_INFO, FileNameLength),
                offset_of!(FILE_FULL_DIR_INFO, FileName),
            )? {
                if name == [u16::from(b'.')] || name == [u16::from(b'.'); 2] {
                    continue;
                }
                if names.len() == max {
                    return Err(rejected("too many entries"));
                }
                names.push(OsString::from_wide(&name));
            }
        }
        Ok(names)
    }

    /// 相对持有的目录句柄打开 `name`（不经 Win32 路径解析，不穿越重解析点，目录打不开）。
    #[allow(unsafe_code)]
    fn open_relative(&self, name: &str) -> io::Result<File> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
        use windows_sys::Wdk::Storage::FileSystem::{
            NtCreateFile, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_REPARSE_POINT,
            FILE_SYNCHRONOUS_IO_NONALERT,
        };
        use windows_sys::Win32::Foundation::{
            RtlNtStatusToDosError, HANDLE, OBJ_CASE_INSENSITIVE, UNICODE_STRING,
        };
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_GENERIC_READ, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

        let wide: Vec<u16> = name.encode_utf16().collect();
        let byte_len =
            u16::try_from(wide.len() * 2).map_err(|_| rejected("entry name too long"))?;
        let object_name = UNICODE_STRING {
            Length: byte_len,
            MaximumLength: byte_len,
            Buffer: wide.as_ptr().cast_mut(),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: u32::try_from(std::mem::size_of::<OBJECT_ATTRIBUTES>()).unwrap_or(0),
            RootDirectory: self.0.as_raw_handle().cast(),
            ObjectName: &raw const object_name,
            Attributes: OBJ_CASE_INSENSITIVE,
            SecurityDescriptor: std::ptr::null(),
            SecurityQualityOfService: std::ptr::null(),
        };
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: 全零是 IO_STATUS_BLOCK 的合法初值（纯数据，由内核填写）。
        let mut status_block: IO_STATUS_BLOCK = unsafe { std::mem::zeroed() };
        // SAFETY: `attributes` 及其指向的 `object_name` / `wide` 在调用期间都活着；根目录句柄来自
        // 本结构持有的 `File`；两个输出指针指向本栈帧里的可写变量；其余指针参数按约定传空。
        let status = unsafe {
            NtCreateFile(
                &raw mut handle,
                FILE_GENERIC_READ,
                &raw const attributes,
                &raw mut status_block,
                std::ptr::null(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_OPEN,
                FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
                std::ptr::null(),
                0,
            )
        };
        if status < 0 {
            // SAFETY: 纯函数，把 NTSTATUS 换成 Win32 错误码。
            let code = unsafe { RtlNtStatusToDosError(status) };
            return Err(io::Error::from_raw_os_error(
                i32::try_from(code).unwrap_or(i32::MAX),
            ));
        }
        // SAFETY: 调用成功，`handle` 是一个新开的、只归我们所有的文件句柄。
        Ok(unsafe { File::from_raw_handle(handle.cast()) })
    }

    /// 打开源目录里名为 `name` 的**常规文件**，返回句柄与打开时的大小。
    pub(super) fn open_regular(&self, name: &str) -> io::Result<(File, u64)> {
        let file = self.open_relative(name)?;
        let metadata = file.metadata()?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(rejected("reparse point"));
        }
        if !file_type.is_file() {
            return Err(rejected("not a regular file"));
        }
        Ok((file, metadata.len()))
    }
}

/// 没有 helper 的 target：构造不出来，故下面两个方法不可达。
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) struct SrcDir(std::convert::Infallible);

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
impl SrcDir {
    pub(super) fn open(_path: &Path, _owner_uid: Option<u32>) -> io::Result<Self> {
        Err(io::Error::from(io::ErrorKind::Unsupported))
    }

    pub(super) fn entry_names(&mut self, _max: usize) -> io::Result<Vec<OsString>> {
        match self.0 {}
    }

    pub(super) fn open_regular(&self, _name: &str) -> io::Result<(File, u64)> {
        match self.0 {}
    }
}

#[cfg(test)]
mod tests;
